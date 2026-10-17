//! Cached, alpha-aware SSIMULACRA2 comparisons of packed SDR pixels.

use anyhow::{ensure, Context, Result};
use fast_ssim2::{PixelDescriptor, PixelSlice, Ssimulacra2Reference};

use crate::input::Pixels;
use crate::limits::{checked_buffer_len, checked_row_stride, validate_processing_dimensions};

/// A precomputed reference, reused for every candidate of one processed image.
pub struct MetricReference {
    reference: Ssimulacra2Reference,
    dimensions: (u32, u32),
    alpha: bool,
}

impl MetricReference {
    pub fn new(pixels: &Pixels) -> Result<Self> {
        let dimensions = pixels.dimensions();
        // Library callers must obey the same codec/metric budget as the CLI.
        validate_processing_dimensions(dimensions, dimensions)?;
        let source = pixel_view(pixels)?;
        let reference = Ssimulacra2Reference::new(&source)
            .context("cannot precompute SSIMULACRA2 reference")?;
        Ok(Self {
            reference,
            dimensions,
            alpha: pixels.has_alpha(),
        })
    }

    /// Score actual decoded pixels with matching dimensions and RGB/RGBA layout.
    ///
    /// Scores are native metric values (possibly negative), never percentages
    /// or clamped values. Straight alpha is compared on both metric backgrounds.
    pub fn score(&self, candidate: &Pixels) -> Result<f64> {
        ensure!(
            candidate.dimensions() == self.dimensions,
            "candidate dimensions {:?} differ from reference {:?}",
            candidate.dimensions(),
            self.dimensions
        );
        ensure!(
            candidate.has_alpha() == self.alpha,
            "candidate RGB/RGBA layout differs from reference"
        );
        let view = pixel_view(candidate)?;
        let score = self
            .reference
            .compare(&view)
            .context("cannot score decoded candidate with SSIMULACRA2")?;
        finite_score(score)
    }
}

fn pixel_view(pixels: &Pixels) -> Result<PixelSlice<'_>> {
    let (width, height) = pixels.dimensions();
    let (channels, descriptor) = if pixels.has_alpha() {
        (4, PixelDescriptor::RGBA8_SRGB)
    } else {
        (3, PixelDescriptor::RGB8_SRGB)
    };
    let expected = checked_buffer_len(width, height, channels)?;
    ensure!(
        pixels.as_bytes().len() == expected,
        "packed pixel buffer must contain exactly {expected} bytes; got {}",
        pixels.as_bytes().len()
    );
    PixelSlice::new(
        pixels.as_bytes(),
        width,
        height,
        checked_row_stride(width, channels)?,
        descriptor,
    )
    .context("cannot construct SSIMULACRA2 pixel view")
}

fn finite_score(score: f64) -> Result<f64> {
    ensure!(score.is_finite(), "SSIMULACRA2 returned a non-finite score");
    Ok(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_scores_fail_without_clamping_valid_scores() {
        for score in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(finite_score(score).is_err());
        }
        for score in [-150.0, 0.0, 80.0, 100.0] {
            assert_eq!(finite_score(score).unwrap(), score);
        }
    }
}
