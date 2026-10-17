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

/// Validate prospective output dimensions before resize/metric/codec allocation.
///
/// The estimate includes the original decode and metric padding, so downscaling
/// cannot bypass the source budget.
pub fn validate_processing_dimensions(source: (u32, u32), output: (u32, u32)) -> Result<()> {
    validate_source_dimensions(source.0, source.1)?;
    pixel_count(output.0, output.1)?;
    ensure!(output.0 <= WEBP_MAX_DIMENSION && output.1 <= WEBP_MAX_DIMENSION, "output {}x{} exceeds WebP's maximum of {WEBP_MAX_DIMENSION} pixels per axis; use a smaller --scale", output.0, output.1);
    checked_buffer_len(output.0, output.1, 4)?;
    let bytes = estimated_processing_bytes(source, output)?;
    ensure!(bytes <= MAX_ESTIMATED_BYTES, "estimated working memory is {bytes} bytes; budget is {MAX_ESTIMATED_BYTES} bytes; use a smaller --scale");
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
    ensure!(bytes <= MAX_ESTIMATED_BYTES, "estimated working memory including resize buffers is {bytes} bytes; budget is {MAX_ESTIMATED_BYTES} bytes; use a smaller --scale or a smaller source image");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
