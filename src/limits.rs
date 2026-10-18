//! Allocation guards to apply to headers before decoding and before processing.
//!
//! The working-set estimate is a conservative policy, not a peak-RSS guarantee.
//! Representative Linux measurements are recorded in benchmarks/RESULTS.md;
//! codec scratch, metadata and allocator behavior can still vary across inputs.

use anyhow::{ensure, Context, Result};

/// Independent source-header pixel cap, even when downscaling aggressively.
pub const MAX_SOURCE_PIXELS: u64 = 100_000_000;
/// Estimated combined source and processing memory budget: 1 GiB.
pub const MAX_ESTIMATED_BYTES: u64 = 1024 * 1024 * 1024;
/// Validated libwebp encoder limit on each output axis.
pub const WEBP_MAX_DIMENSION: u32 = 16_383;

// Supported decoders may produce 16-bit RGBA before normalization.
const SOURCE_BYTES_PER_PIXEL: u64 = 8;
// Reserve room for RGB/RGBA buffers, both alpha backgrounds, cached reference
// pyramids, candidate metric scratch, encoder state and retained encoded bytes.
const PROCESSING_BYTES_PER_PIXEL: u64 = 256;

/// Resize by a multiplier or fit within optional maximum dimensions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Resize {
    Scale(f64),
    /// At least one positive bound is required. Images are never enlarged.
    Bounds {
        width: Option<u32>,
        height: Option<u32>,
    },
}

impl Resize {
    /// Calculate dimensions before decoding; codec and memory guards apply next.
    pub fn dimensions(self, source: (u32, u32)) -> Result<(u32, u32)> {
        match self {
            Self::Scale(scale) => scaled_dimensions(source, scale),
            Self::Bounds { width, height } => {
                pixel_count(source.0, source.1)?;
                ensure!(
                    width.is_some() || height.is_some(),
                    "at least one maximum width or height is required"
                );
                ensure!(
                    width != Some(0) && height != Some(0),
                    "maximum width and height must be positive integers"
                );
                // Choose min(1, width/source_width, height/source_height).
                // Exact u64 cross-products avoid floating-point boundary drift;
                // even products of two u32::MAX values fit in u64.
                let (mut numerator, mut denominator) = (1_u64, 1_u64);
                for (bound, axis) in [(width, source.0), (height, source.1)] {
                    if let Some(bound) = bound {
                        if u64::from(bound) * denominator < numerator * u64::from(axis) {
                            numerator = u64::from(bound);
                            denominator = u64::from(axis);
                        }
                    }
                }
                // Round to nearest pixel, with halves upward and a one-pixel
                // minimum. The chosen ratio cannot exceed either integer bound.
                let dimension = |axis| {
                    ((u64::from(axis) * numerator + denominator / 2) / denominator).max(1) as u32
                };
                Ok((dimension(source.0), dimension(source.1)))
            }
        }
    }
}

fn pixel_count(width: u32, height: u32) -> Result<u64> {
    ensure!(
        width > 0 && height > 0,
        "image dimensions must be nonzero (got {width}x{height})"
    );
    u64::from(width)
        .checked_mul(u64::from(height))
        .context("image pixel count overflow")
}

/// Apply the CLI scale contract with positive half values rounded upward.
///
/// Conversion to `u32` happens only after checking the floating-point result.
/// Codec and allocation limits apply separately to the resulting dimensions.
pub fn scaled_dimensions(source: (u32, u32), scale: f64) -> Result<(u32, u32)> {
    pixel_count(source.0, source.1)?;
    ensure!(
        scale.is_finite() && 0.0 < scale && scale <= 1.0,
        "scale must be a finite number in (0, 1]"
    );
    let dimension = |value| -> Result<u32> {
        let rounded = (f64::from(value) * scale).round().max(1.0);
        ensure!(rounded <= f64::from(u32::MAX), "scaled dimension overflow");
        Ok(rounded as u32)
    };
    Ok((dimension(source.0)?, dimension(source.1)?))
}

/// Calculate a packed row stride without truncating on 32-bit targets.
pub fn checked_row_stride(width: u32, bytes_per_pixel: usize) -> Result<usize> {
    ensure!(
        width > 0 && bytes_per_pixel > 0,
        "image width and bytes per pixel must be nonzero"
    );
    let stride = usize::try_from(width)
        .context("image width does not fit the platform address space")?
        .checked_mul(bytes_per_pixel)
        .context("image row stride overflow")?;
    ensure!(
        stride <= isize::MAX as usize,
        "image row exceeds the platform allocation limit"
    );
    Ok(stride)
}

/// Calculate a packed buffer length before allocating or invoking a codec.
pub fn checked_buffer_len(width: u32, height: u32, bytes_per_pixel: usize) -> Result<usize> {
    ensure!(height > 0, "image height must be nonzero");
    let len = checked_row_stride(width, bytes_per_pixel)?
        .checked_mul(
            usize::try_from(height)
                .context("image height does not fit the platform address space")?,
        )
        .context("image buffer length overflow")?;
    ensure!(
        len <= isize::MAX as usize,
        "image buffer exceeds the platform allocation limit"
    );
    Ok(len)
}

/// Validate source header dimensions before allocating a decoded image.
///
/// This does not apply WebP's axis limit: a larger source may be downscaled.
pub fn validate_source_dimensions(width: u32, height: u32) -> Result<()> {
    let pixels = pixel_count(width, height)?;
    ensure!(
        pixels <= MAX_SOURCE_PIXELS,
        "source {width}x{height} has {pixels} pixels; maximum is {MAX_SOURCE_PIXELS}"
    );
    checked_buffer_len(width, height, SOURCE_BYTES_PER_PIXEL as usize)?;
    let bytes = pixels
        .checked_mul(SOURCE_BYTES_PER_PIXEL)
        .context("source memory estimate overflow")?;
    ensure!(
        bytes <= MAX_ESTIMATED_BYTES,
        "source requires an estimated {bytes} bytes; memory budget is {MAX_ESTIMATED_BYTES} bytes"
    );
    Ok(())
}

fn estimated_processing_bytes(source: (u32, u32), output: (u32, u32)) -> Result<u64> {
    let source_bytes = pixel_count(source.0, source.1)?
        .checked_mul(SOURCE_BYTES_PER_PIXEL)
        .context("source memory estimate overflow")?;
    // fast-ssim2 reflect-pads each axis to at least eight pixels.
    let processing_bytes = pixel_count(output.0.max(8), output.1.max(8))?
        .checked_mul(PROCESSING_BYTES_PER_PIXEL)
        .context("processing memory estimate overflow")?;
    source_bytes
        .checked_add(processing_bytes)
        .context("combined memory estimate overflow")
}

/// Include orientation copies, float color preparation and bounded row scratch.
/// The original dimensions remain available for source guards; resize scratch
/// uses the oriented raster, whose axes may have exchanged places.
pub fn validate_preparation_dimensions(
    source: (u32, u32),
    oriented: (u32, u32),
    output: (u32, u32),
    alpha: bool,
    decoded_bytes_per_pixel: usize,
    orientation: bool,
    float: bool,
) -> Result<()> {
    ensure!(
        (1..=16).contains(&decoded_bytes_per_pixel),
        "decoded pixel size must be in 1..=16 bytes"
    );
    // Float preparation uses RGBA until final normalization, including opaque
    // sources. Its alpha-aware resize requires the float source allowance.
    validate_resize_dimensions(oriented, output, alpha || float)?;
    let source_pixels = pixel_count(source.0, source.1)?;
    ensure!(
        source_pixels == pixel_count(oriented.0, oriented.1)?,
        "orientation changed pixel count"
    );
    let decoded = decoded_bytes_per_pixel as u64;
    // The existing source allowance is eight bytes per pixel. Color conversion
    // may hold the decoded buffer and RGBA32F together; rotation may copy it.
    let per_pixel = (if float { decoded + 16 } else { decoded }).max(if orientation {
        decoded * 2
    } else {
        decoded
    });
    let extra = source_pixels
        .checked_mul(per_pixel.saturating_sub(SOURCE_BYTES_PER_PIXEL))
        .context("preparation memory estimate overflow")?;
    let scratch = if float {
        u64::from(oriented.0) * 6 * 4
    } else {
        0
    };
    let mut bytes = estimated_processing_bytes(source, output)?;
    if oriented != output {
        bytes = bytes
            .checked_add(checked_buffer_len(oriented.0, output.1, 16)? as u64)
            .and_then(|b| {
                b.checked_add(if alpha || float {
                    source_pixels * 16
                } else {
                    0
                })
            })
            .and_then(|b| b.checked_add(u64::from(oriented.0.max(oriented.1)) * 8))
            .context("preparation resize estimate overflow")?;
    }
    bytes = bytes
        .checked_add(extra)
        .and_then(|b| b.checked_add(scratch))
        .context("preparation memory estimate overflow")?;
    ensure!(bytes <= MAX_ESTIMATED_BYTES,
        "estimated working memory including orientation/color buffers is {bytes} bytes; budget is {MAX_ESTIMATED_BYTES} bytes; use smaller dimensions or a smaller source image");
    Ok(())
}

/// Validate prospective output dimensions before resize/metric/codec allocation.
///
/// The estimate includes the original decode and metric padding, so downscaling
/// cannot bypass the source budget.
pub fn validate_processing_dimensions(source: (u32, u32), output: (u32, u32)) -> Result<()> {
    validate_source_dimensions(source.0, source.1)?;
    pixel_count(output.0, output.1)?;
    ensure!(output.0 <= WEBP_MAX_DIMENSION && output.1 <= WEBP_MAX_DIMENSION, "output {}x{} exceeds WebP's maximum of {WEBP_MAX_DIMENSION} pixels per axis; use a smaller --scale or tighter --scale-width/--scale-height bounds", output.0, output.1);
    checked_buffer_len(output.0, output.1, 4)?;
    let bytes = estimated_processing_bytes(source, output)?;
    ensure!(bytes <= MAX_ESTIMATED_BYTES, "estimated working memory is {bytes} bytes; budget is {MAX_ESTIMATED_BYTES} bytes; use a smaller --scale or tighter --scale-width/--scale-height bounds");
    Ok(())
}

/// Include the actual Lanczos intermediate and alpha premultiplication buffers.
///
/// The image crate samples vertically into a four-channel float buffer first.
/// Alpha resizing also needs a float source to avoid bleeding hidden RGB into
/// visible pixels. Coefficient vectors reserve up to eight bytes per source axis
/// element (including Vec growth). Output buffers fit the processing allowance.
pub fn validate_resize_dimensions(
    source: (u32, u32),
    output: (u32, u32),
    alpha: bool,
) -> Result<()> {
    validate_processing_dimensions(source, output)?;
    if source == output {
        return Ok(());
    }
    let intermediate = checked_buffer_len(source.0, output.1, 16)? as u64;
    let float_source = if alpha {
        checked_buffer_len(source.0, source.1, 16)? as u64
    } else {
        0
    };
    let weights = u64::from(source.0.max(source.1)) * 8;
    let bytes = estimated_processing_bytes(source, output)?
        .checked_add(intermediate)
        .and_then(|bytes| bytes.checked_add(float_source))
        .and_then(|bytes| bytes.checked_add(weights))
        .context("resize memory estimate overflow")?;
    ensure!(bytes <= MAX_ESTIMATED_BYTES, "estimated working memory including resize buffers is {bytes} bytes; budget is {MAX_ESTIMATED_BYTES} bytes; use a smaller --scale, tighter --scale-width/--scale-height bounds or a smaller source image");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimension_bounds_fit_landscape_portrait_and_tiny_sources() {
        for (source, width, height, expected) in [
            ((1920, 1080), Some(800), None, (800, 450)),
            ((1920, 1080), None, Some(450), (800, 450)),
            ((1920, 1080), Some(800), Some(600), (800, 450)),
            ((1920, 1080), Some(1000), Some(450), (800, 450)),
            ((1080, 1920), Some(600), Some(800), (450, 800)),
            ((1080, 1920), Some(450), None, (450, 800)),
            ((17, 11), Some(9), None, (9, 6)),
            ((17, 11), None, Some(6), (9, 6)),
            ((17, 11), Some(9), Some(5), (8, 5)),
            ((3, 1), Some(2), None, (2, 1)),
            ((1, 3), None, Some(1), (1, 1)),
            ((17, 11), Some(1), None, (1, 1)),
            ((17, 11), None, Some(1), (2, 1)),
            ((17, 11), Some(100), Some(11), (17, 11)),
            ((17, 11), Some(100), None, (17, 11)),
            ((1, 1), Some(100), Some(100), (1, 1)),
            (
                (u32::MAX, u32::MAX),
                Some(u32::MAX - 1),
                None,
                (u32::MAX - 1, u32::MAX - 1),
            ),
            ((u32::MAX, 1), Some(1), Some(u32::MAX), (1, 1)),
            ((1, u32::MAX), Some(u32::MAX), Some(1), (1, 1)),
        ] {
            let resize = Resize::Bounds { width, height };
            assert_eq!(
                resize.dimensions(source).unwrap(),
                expected,
                "{source:?} {resize:?}"
            );
        }
    }

    #[test]
    fn rounded_dimensions_never_exceed_bounds_or_enlarge_the_source() {
        for source_width in 1..=16 {
            for source_height in 1..=16 {
                for width in 1..=20 {
                    for height in 1..=20 {
                        let (actual_width, actual_height) = Resize::Bounds {
                            width: Some(width),
                            height: Some(height),
                        }
                        .dimensions((source_width, source_height))
                        .unwrap();
                        assert!((1..=width.min(source_width)).contains(&actual_width));
                        assert!((1..=height.min(source_height)).contains(&actual_height));
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_bounds_and_source_dimensions_are_rejected() {
        for (width, height) in [
            (None, None),
            (Some(0), None),
            (None, Some(0)),
            (Some(1), Some(0)),
        ] {
            assert!(Resize::Bounds { width, height }
                .dimensions((17, 11))
                .is_err());
        }
        for source in [(0, 1), (1, 0)] {
            assert!(Resize::Bounds {
                width: Some(1),
                height: None
            }
            .dimensions(source)
            .is_err());
        }
    }

    #[test]
    fn scale_rounding_tiny_and_invalid_values() {
        assert_eq!(scaled_dimensions((17, 11), 0.5).unwrap(), (9, 6));
        assert_eq!(scaled_dimensions((1, 3), 0.5).unwrap(), (1, 2));
        assert_eq!(
            scaled_dimensions((1, 1), f64::MIN_POSITIVE).unwrap(),
            (1, 1)
        );
        assert_eq!(scaled_dimensions((17, 11), 1.0).unwrap(), (17, 11));
        assert_eq!(
            scaled_dimensions((u32::MAX, u32::MAX), 1.0).unwrap(),
            (u32::MAX, u32::MAX)
        );
        for scale in [0.0, -0.1, 1.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(scaled_dimensions((1, 1), scale).is_err());
        }
        assert!(scaled_dimensions((0, 1), 0.5).is_err());
        assert!(scaled_dimensions((1, 0), 0.5).is_err());
    }

    #[test]
    fn resize_scratch_cannot_bypass_memory_budget() {
        assert!(validate_resize_dimensions((10_000, 10_000), (1000, 1000), false).is_err());
        assert!(validate_resize_dimensions((4000, 4000), (1000, 1000), true).is_ok());
        // A skinny source can have a small output but an enormous intermediate.
        assert!(validate_resize_dimensions((100_000_000, 1), (1, 1), false).is_err());
        assert!(validate_resize_dimensions((2000, 2000), (2000, 2000), true).is_ok());
    }

    #[test]
    fn packed_layout_and_overflow() {
        assert_eq!(checked_row_stride(7, 3).unwrap(), 21);
        assert_eq!(checked_buffer_len(7, 5, 4).unwrap(), 140);
        assert!(checked_buffer_len(0, 1, 4).is_err());
        assert!(checked_buffer_len(1, 0, 4).is_err());
        assert!(checked_buffer_len(1, 1, 0).is_err());
        assert!(checked_row_stride(2, usize::MAX).is_err());
        assert!(checked_buffer_len(2, 2, isize::MAX as usize / 2).is_err());
        assert!(checked_buffer_len(u32::MAX, u32::MAX, 4).is_err());
    }

    #[test]
    fn source_pixel_boundaries() {
        assert!(validate_source_dimensions(10_000, 10_000).is_ok());
        assert!(validate_source_dimensions(10_001, 10_000).is_err());
        assert!(validate_source_dimensions(u32::MAX, u32::MAX).is_err());
        assert!(validate_source_dimensions(0, 1).is_err());
        assert!(validate_source_dimensions(1, 0).is_err());
    }

    #[test]
    fn output_codec_boundaries_and_downscaled_source() {
        assert!(validate_processing_dimensions((16_383, 1), (16_383, 1)).is_ok());
        assert!(validate_processing_dimensions((1, 16_383), (1, 16_383)).is_ok());
        assert!(validate_processing_dimensions((16_384, 1), (16_384, 1)).is_err());
        assert!(validate_processing_dimensions((1, 16_384), (1, 16_384)).is_err());
        assert!(validate_processing_dimensions((20_000, 1), (10_000, 1)).is_ok());
        assert!(validate_processing_dimensions((1, 1), (0, 1)).is_err());
        assert!(validate_processing_dimensions((1, 1), (1, 0)).is_err());
    }

    #[test]
    fn combined_memory_budget_and_metric_padding() {
        assert_eq!(
            estimated_processing_bytes((1, 1), (1, 1)).unwrap(),
            8 + 8 * 8 * 256
        );
        assert!(validate_processing_dimensions((2000, 2000), (2000, 2000)).is_ok());
        let error = validate_processing_dimensions((2048, 2048), (2048, 2048)).unwrap_err();
        assert!(error.to_string().contains("smaller --scale"));
        assert!(validate_processing_dimensions((10_000, 10_000), (1000, 1000)).is_ok());
        // A small output cannot excuse an over-budget source.
        assert!(validate_processing_dimensions((10_001, 10_000), (1, 1)).is_err());
    }
}
