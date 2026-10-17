//! Rebuild the small synthetic input corpus; no external image assets are used.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use image::{DynamicImage, ImageBuffer, ImageFormat, Luma, LumaA, Rgb, RgbImage, Rgba, RgbaImage};

fn main() -> Result<()> {
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("tests/fixtures"));
    generate(&directory)
}

fn generate(directory: &Path) -> Result<()> {
    fs::create_dir_all(directory)?;
    let rgb = RgbImage::from_fn(17, 11, |x, y| {
        Rgb([
            (x * 13) as u8,
            (y * 23) as u8,
            ((x * 7 + y * 5) % 256) as u8,
        ])
    });
    for (name, format) in [
        ("rgb.png", ImageFormat::Png),
        ("rgb.jpg", ImageFormat::Jpeg),
        ("rgb.bmp", ImageFormat::Bmp),
        ("rgb.webp", ImageFormat::WebP),
    ] {
        rgb.save_with_format(directory.join(name), format)?;
    }
    let rgba = RgbaImage::from_fn(17, 11, |x, y| {
        let [r, g, b] = rgb.get_pixel(x, y).0;
        Rgba([r, g, b, [0, 64, 160, 255][x as usize % 4]])
    });
    rgba.save(directory.join("rgba.png"))?;
    rgba.save(directory.join("rgba.webp"))?;
    RgbaImage::from_fn(17, 11, |x, y| {
        let [r, g, b] = rgb.get_pixel(x, y).0;
        Rgba([r, g, b, 255])
    })
    .save(directory.join("opaque-rgba.png"))?;
    RgbImage::from_pixel(1, 1, Rgb([80, 120, 160])).save(directory.join("tiny-rgb.png"))?;
    RgbaImage::from_pixel(1, 1, Rgba([80, 120, 160, 64])).save(directory.join("tiny-rgba.png"))?;
    ImageBuffer::from_fn(3, 2, |x, y| Luma([(x * 70 + y * 10) as u8]))
        .save(directory.join("gray.png"))?;
    ImageBuffer::from_pixel(2, 1, LumaA([12_u8, 64])).save(directory.join("gray-alpha.png"))?;
    ImageBuffer::from_pixel(2, 1, Rgba([0_u16, 65535, 32896, 16448]))
        .save(directory.join("rgba16.png"))?;
    // Bright RGB hidden under alpha=0 must not tint the visible red edge.
    RgbaImage::from_fn(4, 2, |x, _| {
        if x < 2 {
            Rgba([255, 0, 0, 255])
        } else {
            Rgba([0, 255, 255, 0])
        }
    })
    .save(directory.join("alpha-edge.png"))?;
    RgbaImage::from_pixel(3, 2, Rgba([19, 90, 220, 0])).save(directory.join("transparent.png"))?;

    // Real two-frame APNG, not just an animation flag in an invalid header.
    let mut encoder = png::Encoder::new(File::create(directory.join("animated.png"))?, 17, 11);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_animated(2, 0)?;
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba.as_raw())?;
    writer.write_image_data(RgbaImage::from_pixel(17, 11, Rgba([255, 0, 0, 255])).as_raw())?;
    writer.finish()?;

    let config = webp::WebPConfig::new().map_err(|_| anyhow!("cannot initialize WebP config"))?;
    let mut animation = webp::AnimEncoder::new(17, 11, &config);
    animation.add_frame(webp::AnimFrame::from_rgb(rgb.as_raw(), 17, 11, 0));
    let second = RgbImage::from_pixel(17, 11, Rgb([255, 0, 0]));
    animation.add_frame(webp::AnimFrame::from_rgb(second.as_raw(), 17, 11, 100));
    fs::write(
        directory.join("animated.webp"),
        &*animation
            .try_encode()
            .map_err(|error| anyhow!("cannot encode animation: {error:?}"))?,
    )?;

    // Inject EXIF orientation 6 (90 degrees clockwise) into an ordinary JPEG.
    // The loader deliberately keeps the stored 17x11 raster orientation.
    let jpeg = fs::read(directory.join("rgb.jpg"))?;
    let mut exif =
        b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0".to_vec();
    let mut oriented = jpeg[..2].to_vec();
    oriented.extend_from_slice(&[0xff, 0xe1]);
    oriented.extend_from_slice(&u16::try_from(exif.len() + 2)?.to_be_bytes());
    oriented.append(&mut exif);
    oriented.extend_from_slice(&jpeg[2..]);
    fs::write(directory.join("oriented.jpg"), oriented)?;

    // A recognized but unsupported still format, plus a corrupt input.
    fs::write(
        directory.join("unsupported.gif"),
        b"GIF89a\x01\0\x01\0\0\0\0",
    )?;
    fs::write(
        directory.join("corrupt.png"),
        b"\x89PNG\r\n\x1a\ninvalid header",
    )?;
    // Keep the wrapper and DIB header intact, then change dimensions. There is
    // no large pixel allocation or payload; limits must fail before decoding.
    let mut bmp = fs::read(directory.join("rgb.bmp"))?;
    bmp.truncate(54);
    for (name, width, height) in [
        ("oversized.bmp", 10_001_i32, 10_000_i32),
        ("over-budget.bmp", 2048, 2048),
        ("wide.bmp", 20_000, 1),
    ] {
        bmp[18..22].copy_from_slice(&width.to_le_bytes());
        bmp[22..26].copy_from_slice(&height.to_le_bytes());
        fs::write(directory.join(name), &bmp)?;
    }
    // A genuine, decodable wide image proves the limit is applied after scaling.
    DynamicImage::ImageRgb8(RgbImage::from_pixel(20_000, 1, Rgb([80, 120, 160])))
        .save(directory.join("wide.png"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_reproduces_committed_fixtures_and_repeated_runs() {
        let generated = tempfile::tempdir().unwrap();
        let repeated = tempfile::tempdir().unwrap();
        generate(generated.path()).unwrap();
        generate(repeated.path()).unwrap();
        let committed = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let image_names = |directory: &Path| {
            let mut names: Vec<_> = fs::read_dir(directory)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .filter(|name| name != "README.md")
                .collect();
            names.sort();
            names
        };
        let names = image_names(&committed);
        assert_eq!(image_names(generated.path()), names);
        assert_eq!(image_names(repeated.path()), names);
        for name in names {
            let expected = fs::read(committed.join(&name)).unwrap();
            assert_eq!(
                fs::read(generated.path().join(&name)).unwrap(),
                expected,
                "{name:?}"
            );
            assert_eq!(
                fs::read(repeated.path().join(&name)).unwrap(),
                expected,
                "{name:?}"
            );
        }
    }
}
