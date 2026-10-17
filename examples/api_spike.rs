//! Disposable Phase 0 probe. Generates its own pixels and writes no image files.

use std::io::Cursor;

use anyhow::{anyhow, ensure, Context, Result};
use fast_ssim2::{PixelDescriptor, PixelSlice, Ssimulacra2Reference};
use image::{codecs::webp::WebPDecoder, ImageDecoder, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use webp::Encoder;

const WIDTH: u32 = 32;
const HEIGHT: u32 = 24;

fn main() -> Result<()> {
    probe_rgb()?;
    probe_rgba()?;
    probe_transparent_colors()?;
    probe_dimensions()?;
    probe_animation()?;
    println!("All Phase 0 API checks passed.");
    Ok(())
}

fn rgb_fixture() -> RgbImage {
    RgbImage::from_fn(WIDTH, HEIGHT, |x, y| {
        Rgb([(x * 7) as u8, (y * 11) as u8, ((x * 5 + y * 3) % 256) as u8])
    })
}

fn rgba_fixture() -> RgbaImage {
    let rgb = rgb_fixture();
    RgbaImage::from_fn(WIDTH, HEIGHT, |x, y| {
        let [r, g, b] = rgb.get_pixel(x, y).0;
        Rgba([r, g, b, [0, 64, 160, 255][x as usize % 4]])
    })
}

fn pixel_view(bytes: &[u8], width: u32, height: u32, alpha: bool) -> Result<PixelSlice<'_>> {
    let (channels, descriptor) = if alpha {
        (4, PixelDescriptor::RGBA8_SRGB)
    } else {
        (3, PixelDescriptor::RGB8_SRGB)
    };
    // Stride is measured in bytes per row, not pixels or total buffer length.
    Ok(PixelSlice::new(
        bytes,
        width,
        height,
        width as usize * channels,
        descriptor,
    )?)
}

fn compare(reference: &Ssimulacra2Reference, candidate: &PixelSlice<'_>) -> Result<f64> {
    let score = reference.compare(candidate)?;
    ensure!(score.is_finite(), "metric returned a non-finite score");
    Ok(score)
}

fn probe_round_trip(bytes: &[u8], width: u32, height: u32, alpha: bool) -> Result<()> {
    let source = pixel_view(bytes, width, height, alpha)?;
    let reference = Ssimulacra2Reference::new(&source)?;
    ensure!(
        compare(&reference, &source)? == 100.0,
        "identity must score 100"
    );

    let mut distorted = bytes.to_vec();
    for pixel in distorted.chunks_exact_mut(if alpha { 4 } else { 3 }) {
        for channel in &mut pixel[..3] {
            *channel = 255 - *channel;
        }
    }
    let score = compare(&reference, &pixel_view(&distorted, width, height, alpha)?)?;
    if alpha && bytes.chunks_exact(4).all(|pixel| pixel[3] == 0) {
        ensure!(score == 100.0, "fully transparent RGB must be invisible");
    } else {
        ensure!(score < 100.0, "visible distortion must lower the score");
    }

    let encoder = if alpha {
        Encoder::from_rgba(bytes, width, height)
    } else {
        Encoder::from_rgb(bytes, width, height)
    };
    for (lossless, quality) in [(false, 0.0), (false, 75.0), (false, 100.0), (true, 75.0)] {
        // encode()/encode_lossless() unwrap errors internally; use the Result API.
        let encoded = encoder
            .encode_simple(lossless, quality)
            .map_err(|error| anyhow!("WebP encode failed: {error:?}"))?;
        ensure!(!encoded.is_empty(), "encoder returned no bytes");
        let decoder = WebPDecoder::new(Cursor::new(&*encoded))?;
        ensure!(!decoder.has_animation(), "still encoder produced animation");
        ensure!(
            decoder.dimensions() == (width, height),
            "dimensions changed"
        );

        let decoded = image::load_from_memory_with_format(&encoded, ImageFormat::WebP)
            .context("decoding the WebP candidate")?;
        let pixels = if alpha {
            ensure!(decoded.color().has_alpha(), "transparency was dropped");
            decoded.to_rgba8().into_raw()
        } else {
            decoded.to_rgb8().into_raw()
        };
        let score = compare(&reference, &pixel_view(&pixels, width, height, alpha)?)?;
        if alpha {
            ensure!(
                bytes
                    .chunks_exact(4)
                    .map(|p| p[3])
                    .eq(pixels.chunks_exact(4).map(|p| p[3])),
                "alpha values changed"
            );
        } else if lossless {
            ensure!(pixels == bytes, "opaque lossless pixels changed");
            ensure!(score == 100.0, "opaque lossless must score 100");
        }
        println!(
            "{} {width}x{height} lossless={lossless} quality={quality}: {} bytes, score={score:.6}",
            if alpha { "RGBA" } else { "RGB" },
            encoded.len(),
        );
    }
    Ok(())
}

fn probe_rgb() -> Result<()> {
    probe_round_trip(rgb_fixture().as_raw(), WIDTH, HEIGHT, false)
}

fn probe_rgba() -> Result<()> {
    probe_round_trip(rgba_fixture().as_raw(), WIDTH, HEIGHT, true)
}

fn probe_transparent_colors() -> Result<()> {
    let source = rgba_fixture();
    let reference = Ssimulacra2Reference::new(&pixel_view(source.as_raw(), WIDTH, HEIGHT, true)?)?;
    let mut hidden = source.clone();
    for pixel in hidden.pixels_mut().filter(|p| p[3] == 0) {
        pixel.0[..3].copy_from_slice(&[255, 13, 241]);
    }
    let hidden_score = compare(
        &reference,
        &pixel_view(hidden.as_raw(), WIDTH, HEIGHT, true)?,
    )?;
    ensure!(hidden_score == 100.0, "RGB under alpha=0 must be invisible");

    let mut partial = source.clone();
    for pixel in partial.pixels_mut().filter(|p| p[3] == 160) {
        pixel.0[..3].copy_from_slice(&[255, 13, 241]);
    }
    let partial_score = compare(
        &reference,
        &pixel_view(partial.as_raw(), WIDTH, HEIGHT, true)?,
    )?;
    ensure!(
        partial_score < 100.0,
        "partial-alpha RGB changes must be visible"
    );

    let mut changed_alpha = source.clone();
    for pixel in changed_alpha.pixels_mut() {
        pixel[3] = 255;
    }
    let alpha_score = compare(
        &reference,
        &pixel_view(changed_alpha.as_raw(), WIDTH, HEIGHT, true)?,
    )?;
    ensure!(alpha_score < 100.0, "alpha changes must be visible");

    let invisible = RgbaImage::from_pixel(WIDTH, HEIGHT, Rgba([255, 0, 19, 0]));
    probe_round_trip(invisible.as_raw(), WIDTH, HEIGHT, true)?;
    println!("Alpha checks: hidden RGB={hidden_score}, partial RGB={partial_score:.6}, changed alpha={alpha_score:.6}");
    Ok(())
}

fn probe_dimensions() -> Result<()> {
    // Skinny allocations exercise the actual codec boundary without a huge image.
    for (width, height, valid) in [
        (16_383, 1, true),
        (1, 16_383, true),
        (16_384, 1, false),
        (1, 16_384, false),
    ] {
        let pixels = vec![128; width as usize * height as usize * 3];
        for lossless in [false, true] {
            let result = Encoder::from_rgb(&pixels, width, height).encode_simple(lossless, 75.0);
            if valid {
                let encoded = result.map_err(|e| anyhow!("valid dimensions rejected: {e:?}"))?;
                let decoder = WebPDecoder::new(Cursor::new(&*encoded))?;
                ensure!(
                    decoder.dimensions() == (width, height),
                    "boundary dimensions changed"
                );
            } else {
                ensure!(
                    matches!(
                        result,
                        Err(webp::WebPEncodingError::VP8_ENC_ERROR_BAD_DIMENSION)
                    ),
                    "oversized dimensions must return BAD_DIMENSION"
                );
            }
        }
    }
    // The metric reflect-pads tiny images; original dimensions must still agree.
    probe_round_trip(&[80, 120, 160], 1, 1, false)?;
    probe_round_trip(&[80, 120, 160, 64], 1, 1, true)?;
    println!("WebP dimension boundary verified: 1..=16383 on each axis.");
    Ok(())
}

fn probe_animation() -> Result<()> {
    let first = rgb_fixture();
    let second = RgbImage::from_pixel(WIDTH, HEIGHT, Rgb([255, 0, 0]));
    let config = webp::WebPConfig::new().map_err(|_| anyhow!("initializing WebP config"))?;
    let mut encoder = webp::AnimEncoder::new(WIDTH, HEIGHT, &config);
    encoder.add_frame(webp::AnimFrame::from_rgb(first.as_raw(), WIDTH, HEIGHT, 0));
    encoder.add_frame(webp::AnimFrame::from_rgb(
        second.as_raw(),
        WIDTH,
        HEIGHT,
        100,
    ));
    let encoded = encoder
        .try_encode()
        .map_err(|e| anyhow!("animation fixture encode failed: {e:?}"))?;
    let decoder = WebPDecoder::new(Cursor::new(&*encoded))?;
    ensure!(decoder.has_animation(), "animated WebP was not detected");

    let mut png = Cursor::new(Vec::new());
    first.write_to(&mut png, ImageFormat::Png)?;
    png.set_position(0);
    let decoder = image::codecs::png::PngDecoder::new(png)?;
    ensure!(!decoder.is_apng()?, "still PNG was detected as APNG");
    println!("WebP animation detection verified with a generated two-frame image.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_encode_decode_score() -> Result<()> {
        probe_rgb()
    }

    #[test]
    fn rgba_encode_decode_score() -> Result<()> {
        probe_rgba()
    }

    #[test]
    fn transparent_color_semantics() -> Result<()> {
        probe_transparent_colors()
    }

    #[test]
    fn codec_dimension_boundaries_and_tiny_images() -> Result<()> {
        probe_dimensions()
    }

    #[test]
    fn webp_and_png_animation_detection_apis() -> Result<()> {
        probe_animation()
    }
}
