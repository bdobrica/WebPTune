//! Decode and prepare a single reference image without writing output.
//!
//! EXIF orientation precedes resizing. Supported ICC/PNG color descriptions
//! become sRGB, with float processing for color-managed and 16-bit sources.
//! Linear-sRGB Radiance HDR is tone mapped to SDR. Only the final reference is
//! quantized to eight bits; metadata is discarded. Animation is rejected.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use image::codecs::{
    bmp::BmpDecoder, hdr::HdrDecoder, jpeg::JpegDecoder, png::PngDecoder, webp::WebPDecoder,
};
use image::metadata::Orientation;
use image::{
    imageops, DynamicImage, GenericImageView, ImageDecoder, ImageFormat, ImageReader, Limits,
    RgbImage, RgbaImage,
};

use crate::color::SourceColor;
use crate::limits::{
    checked_buffer_len, validate_preparation_dimensions, Resize, MAX_ESTIMATED_BYTES,
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

/// Processed reference ready for metric/encoder construction.
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
    load_with_resize(path, Resize::Scale(scale))
}

/// Prepare with a scale multiplier or aspect-preserving maximum dimensions.
///
/// Uses the same pre-decode limits and single Lanczos3 pass as [`load`].
pub fn load_with_resize(path: &Path, resize: Resize) -> Result<PreparedImage> {
    // Validate library callers before any I/O, just as Clap validates CLI input.
    resize.dimensions((1, 1))?;
    let file =
        File::open(path).with_context(|| format!("cannot read input '{}'", path.display()))?;
    let reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .with_context(|| format!("cannot identify input '{}'", path.display()))?;
    let format = reader.format();
    let mut reader = reader.into_inner();
    let mut color = None;
    let decoder: Box<dyn ImageDecoder> = match format {
        Some(ImageFormat::Png) => {
            color = Some(SourceColor::png(&mut reader)?);
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
        Some(ImageFormat::Hdr) => {
            let decoder = HdrDecoder::new(reader).context("cannot read Radiance HDR header")?;
            let metadata = decoder.metadata();
            ensure!(!metadata.custom_attributes.iter().any(|(key, _)| key == "PRIMARIES"),
                "custom Radiance HDR PRIMARIES are unsupported; provide linear sRGB samples without PRIMARIES");
            ensure!(metadata.pixel_aspect_ratio.is_none_or(|ratio| ratio == 1.0),
                "non-square Radiance HDR pixels are unsupported");
            let mut hdr = SourceColor::default();
            hdr.hdr = true;
            color = Some(hdr);
            Box::new(decoder)
        }
        Some(format) => bail!("unsupported input format {format:?}; supported still formats: JPEG, PNG, WebP, BMP, Radiance HDR"),
        None => bail!("unsupported or unrecognized input format; supported still formats: JPEG, PNG, WebP, BMP, Radiance HDR"),
    };
    prepare(decoder, resize, color)
        .with_context(|| format!("cannot prepare input '{}'", path.display()))
}

fn prepare(
    mut decoder: Box<dyn ImageDecoder>,
    resize: Resize,
    color: Option<SourceColor>,
) -> Result<PreparedImage> {
    let original_dimensions = decoder.dimensions();
    let orientation = decoder
        .orientation()
        .context("cannot read EXIF orientation")?;
    let swapped = matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    );
    let oriented_dimensions = if swapped {
        (original_dimensions.1, original_dimensions.0)
    } else {
        original_dimensions
    };
    let dimensions = resize.dimensions(oriented_dimensions)?;
    let color = match color {
        Some(color) => color,
        None => SourceColor::icc(decoder.icc_profile().context("cannot read ICC profile")?)?,
    };
    let float = color.needs_float(
        decoder.color_type().bits_per_pixel() / u16::from(decoder.color_type().channel_count()) > 8,
    );
    validate_preparation_dimensions(
        original_dimensions,
        oriented_dimensions,
        dimensions,
        decoder.color_type().has_alpha(),
        usize::from(decoder.color_type().bytes_per_pixel()),
        orientation != Orientation::NoTransforms,
        float,
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
    let mut source = DynamicImage::from_decoder(decoder).context("cannot decode image pixels")?;
    ensure!(
        source.dimensions() == original_dimensions,
        "decoded dimensions differ from header"
    );
    source.apply_orientation(orientation);
    if float {
        source = color.convert(source)?;
    }

    // Also skip resampling when rounding leaves the dimensions unchanged.
    let processed = if dimensions == oriented_dimensions {
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
