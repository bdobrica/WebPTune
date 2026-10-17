//! Decode and prepare a single reference image without writing output.
//!
//! Pixels are treated as SDR, sRGB-like samples. ICC transforms and EXIF
//! orientation are not applied; metadata is discarded. Sixteen-bit sources
//! are reduced to eight bits after resizing. Animation is rejected through
//! the PNG/WebP decoder APIs; JPEG/BMP have no animation API here.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use image::codecs::{bmp::BmpDecoder, jpeg::JpegDecoder, png::PngDecoder, webp::WebPDecoder};
use image::{
    imageops, DynamicImage, GenericImageView, ImageDecoder, ImageFormat, ImageReader, Limits,
    RgbImage, RgbaImage,
};

use crate::limits::{
    checked_buffer_len, scaled_dimensions, validate_resize_dimensions, MAX_ESTIMATED_BYTES,
    MAX_SOURCE_PIXELS,
};

/// Packed, owned 8-bit reference pixels. Fully opaque images use RGB.
#[derive(Debug)]
pub enum Pixels {
    Rgb(RgbImage),
    Rgba(RgbaImage),
}

impl Pixels {
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::Rgb(image) => image.dimensions(),
            Self::Rgba(image) => image.dimensions(),
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Rgb(image) => image.as_raw(),
            Self::Rgba(image) => image.as_raw(),
        }
    }

    pub fn has_alpha(&self) -> bool {
        matches!(self, Self::Rgba(_))
    }
}

/// Reference ready for metric/encoder construction in Phase 3.
#[derive(Debug)]
pub struct PreparedImage {
    original_dimensions: (u32, u32),
    pixels: Pixels,
}

impl PreparedImage {
    pub fn original_dimensions(&self) -> (u32, u32) {
        self.original_dimensions
    }

    pub fn pixels(&self) -> &Pixels {
        &self.pixels
    }
}

fn decoder_limits() -> Limits {
    let mut limits = Limits::default();
    // Either nonzero axis alone cannot exceed the source pixel limit.
    limits.max_image_width = Some(MAX_SOURCE_PIXELS as u32);
    limits.max_image_height = Some(MAX_SOURCE_PIXELS as u32);
    limits.max_alloc = Some(MAX_ESTIMATED_BYTES);
    limits
}

/// Detect format from contents, inspect headers, then decode and resize once.
///
/// Source, output and resize budgets are checked before allocating decoded
/// pixels. Decoder headers/metadata may allocate earlier (notably the JPEG
/// decoder buffers compressed input). Decoder allocation limits are best effort,
/// not a process memory cap or a complete file-integrity validator.
pub fn load(path: &Path, scale: f64) -> Result<PreparedImage> {
    // Validate library callers before any I/O, just as Clap validates CLI input.
    scaled_dimensions((1, 1), scale)?;
    let file =
        File::open(path).with_context(|| format!("cannot read input '{}'", path.display()))?;
    let reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .with_context(|| format!("cannot identify input '{}'", path.display()))?;
    let format = reader.format();
    let reader = reader.into_inner();
    let decoder: Box<dyn ImageDecoder> = match format {
        Some(ImageFormat::Png) => {
            let decoder = PngDecoder::with_limits(reader, decoder_limits())
                .context("cannot read PNG header")?;
            ensure!(
                !decoder.is_apng().context("cannot inspect PNG animation")?,
                "animated PNG (APNG) input is not supported; provide a still image"
            );
            Box::new(decoder)
        }
        Some(ImageFormat::WebP) => {
            let decoder = WebPDecoder::new(reader).context("cannot read WebP header")?;
            ensure!(
                !decoder.has_animation(),
                "animated WebP input is not supported; provide a still image"
            );
            Box::new(decoder)
        }
        Some(ImageFormat::Jpeg) => {
            Box::new(JpegDecoder::new(reader).context("cannot read JPEG header")?)
        }
        Some(ImageFormat::Bmp) => {
            Box::new(BmpDecoder::new(reader).context("cannot read BMP header")?)
        }
        Some(format) => bail!("unsupported input format {format:?}; supported still formats: JPEG, PNG, WebP, BMP"),
        None => bail!("unsupported or unrecognized input format; supported still formats: JPEG, PNG, WebP, BMP"),
    };
    prepare(decoder, scale).with_context(|| format!("cannot prepare input '{}'", path.display()))
}

fn prepare(mut decoder: Box<dyn ImageDecoder>, scale: f64) -> Result<PreparedImage> {
    let original_dimensions = decoder.dimensions();
    let dimensions = scaled_dimensions(original_dimensions, scale)?;
    validate_resize_dimensions(
        original_dimensions,
        dimensions,
        decoder.color_type().has_alpha(),
    )?;
    let bytes = checked_buffer_len(
        original_dimensions.0,
        original_dimensions.1,
        usize::from(decoder.color_type().bytes_per_pixel()),
    )?;
    ensure!(
        decoder.total_bytes() == bytes as u64,
        "inconsistent decoder buffer size"
    );
    let mut limits = decoder_limits();
    limits
        .reserve(bytes as u64)
        .context("decoded image exceeds allocation budget")?;
    decoder
        .set_limits(limits)
        .context("cannot apply decoder limits")?;
    let source = DynamicImage::from_decoder(decoder).context("cannot decode image pixels")?;
    ensure!(
        source.dimensions() == original_dimensions,
        "decoded dimensions differ from header"
    );

    // Also skip resampling when rounding leaves the dimensions unchanged.
    let processed = if dimensions == original_dimensions {
        source
    } else if source.color().has_alpha() {
        resize_alpha(source, dimensions)
    } else {
        source.resize_exact(dimensions.0, dimensions.1, imageops::FilterType::Lanczos3)
    };
    let pixels = normalize(processed);
    Ok(PreparedImage {
        original_dimensions,
        pixels,
    })
}

fn resize_alpha(source: DynamicImage, dimensions: (u32, u32)) -> DynamicImage {
    // Filter premultiplied samples so invisible RGB cannot tint visible edges.
    // Keep the existing sRGB-like sample space; this is not linear-light color
    // management. Return straight alpha for WebP and fast-ssim2.
    let mut premultiplied = source.into_rgba32f();
    for pixel in premultiplied.pixels_mut() {
        for channel in 0..3 {
            pixel[channel] *= pixel[3];
        }
    }
    let mut resized = imageops::resize(
        &premultiplied,
        dimensions.0,
        dimensions.1,
        imageops::FilterType::Lanczos3,
    );
    for pixel in resized.pixels_mut() {
        let alpha = pixel[3];
        for channel in 0..3 {
            pixel[channel] = if alpha > 0.0 {
                (pixel[channel] / alpha).clamp(0.0, 1.0)
            } else {
                0.0
            };
        }
        pixel[3] = alpha.clamp(0.0, 1.0);
    }
    DynamicImage::ImageRgba32F(resized)
}

fn normalize(image: DynamicImage) -> Pixels {
    if !image.color().has_alpha() {
        return Pixels::Rgb(image.into_rgb8());
    }
    let rgba = image.into_rgba8();
    if rgba.pixels().all(|pixel| pixel[3] == u8::MAX) {
        Pixels::Rgb(DynamicImage::ImageRgba8(rgba).into_rgb8())
    } else {
        Pixels::Rgba(rgba)
    }
}
