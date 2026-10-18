//! Rebuild the small synthetic input corpus; no external image assets are used.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use image::{
    DynamicImage, ImageBuffer, ImageEncoder, ImageFormat, Luma, LumaA, Rgb, RgbImage, Rgba,
    RgbaImage,
};
use moxcms::ColorProfile;

fn main() -> Result<()> {
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("tests/fixtures"));
    generate(&directory)
}

fn profile_bytes(profile: &ColorProfile) -> Result<Vec<u8>> {
    let mut bytes = profile.encode()?;
    // moxcms 0.8.1 writes the current time regardless of the profile's date.
    // Replace the six big-endian u16 ICC date fields; the writer leaves the
    // optional profile ID zero, so there is no hash to recompute.
    for (field, value) in bytes[24..36]
        .chunks_exact_mut(2)
        .zip([2026_u16, 10, 18, 0, 0, 0])
    {
        field.copy_from_slice(&value.to_be_bytes());
    }
    Ok(bytes)
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
    // The loader rotates the stored 17x11 raster before fitting output bounds.
    let jpeg = fs::read(directory.join("rgb.jpg"))?;
    let mut exif =
        b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0".to_vec();
    let mut oriented = jpeg[..2].to_vec();
    oriented.extend_from_slice(&[0xff, 0xe1]);
    oriented.extend_from_slice(&u16::try_from(exif.len() + 2)?.to_be_bytes());
    oriented.append(&mut exif);
    oriented.extend_from_slice(&jpeg[2..]);
    fs::write(directory.join("oriented.jpg"), oriented)?;

    // Generated standard profiles, not copied third-party ICC assets.
    for (name, profile) in [
        ("srgb-icc.png", ColorProfile::new_srgb()),
        ("display-p3.png", ColorProfile::new_display_p3()),
    ] {
        let mut encoder = image::codecs::png::PngEncoder::new(File::create(directory.join(name))?);
        encoder.set_icc_profile(profile_bytes(&profile)?)?;
        encoder.write_image(rgba.as_raw(), 17, 11, image::ExtendedColorType::Rgba8)?;
    }
    let mut linear = ColorProfile::new_srgb();
    linear.cicp = None;
    linear.red_trc = Some(moxcms::curve_from_gamma(1.0));
    linear.green_trc = linear.red_trc.clone();
    linear.blue_trc = linear.red_trc.clone();
    for (name, webp) in [("linear-icc.jpg", false), ("linear-icc.webp", true)] {
        let file = File::create(directory.join(name))?;
        if webp {
            let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(file);
            encoder.set_icc_profile(profile_bytes(&linear)?)?;
            encoder.write_image(rgb.as_raw(), 17, 11, image::ExtendedColorType::Rgb8)?;
        } else {
            let mut encoder = image::codecs::jpeg::JpegEncoder::new(file);
            encoder.set_icc_profile(profile_bytes(&linear)?)?;
            encoder.write_image(rgb.as_raw(), 17, 11, image::ExtendedColorType::Rgb8)?;
        }
    }
    let mut encoder =
        image::codecs::png::PngEncoder::new(File::create(directory.join("gray-linear-icc.png"))?);
    encoder.set_icc_profile(profile_bytes(&ColorProfile::new_gray_with_gamma(1.0))?)?;
    encoder.write_image(&[64, 64, 128, 160], 2, 1, image::ExtendedColorType::La8)?;

    let mut info = png::Info::with_size(4, 2);
    info.color_type = png::ColorType::Rgba;
    info.bit_depth = png::BitDepth::Sixteen;
    info.source_gamma = Some(png::ScaledFloat::new(1.0));
    let mut writer = png::Encoder::with_info(File::create(directory.join("linear16.png"))?, info)?
        .write_header()?;
    let values: Vec<u8> = (0..8_u16)
        .flat_map(|i| [10_000 + i * 1000, 32_768, 50_000, 16_448])
        .flat_map(u16::to_be_bytes)
        .collect();
    writer.write_image_data(&values)?;
    writer.finish()?;
    let hdr: Vec<_> = (0..16)
        .map(|i| {
            let value = [0.0, 0.25, 0.5, 1.0, 2.0, 4.0, 16.0, 128.0][i % 8];
            Rgb([value, value, value])
        })
        .collect();
    image::codecs::hdr::HdrEncoder::new(File::create(directory.join("linear.hdr"))?)
        .encode(&hdr, 8, 2)?;

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
