//! In-memory WebP trial encoding, bounded decoding and measured evaluation.

use std::io::Cursor;
use std::time::{Duration, Instant};

use anyhow::{anyhow, ensure, Context, Result};
use image::{codecs::webp::WebPDecoder, DynamicImage, ImageDecoder, Limits};
use webp::{Encoder, WebPConfig};

use crate::input::Pixels;
use crate::limits::{checked_buffer_len, MAX_ESTIMATED_BYTES};
use crate::metric::MetricReference;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Integer quality in 0..=100, validated before invoking libwebp.
    Lossy {
        quality: u8,
    },
    Lossless,
}

/// Optional wall-clock measurements of the same operations used by `evaluate`.
/// Reference construction and copying the final encoded bytes are excluded.
#[derive(Clone, Copy, Debug, Default)]
pub struct EvaluationTimings {
    pub encode: Duration,
    pub decode: Duration,
    pub metric: Duration,
}

/// Encoded bytes and their measured decoded score; no feasibility decision yet.
#[derive(Debug)]
pub struct Trial {
    mode: Mode,
    score: f64,
    bytes: Vec<u8>,
}

impl Trial {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn score(&self) -> f64 {
        self.score
    }

    pub fn size(&self) -> usize {
        self.bytes.len()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Synthetic measured outcomes for search unit tests only.
    #[cfg(test)]
    pub(crate) fn synthetic(mode: Mode, score: f64, size: usize) -> Self {
        Self {
            mode,
            score,
            bytes: vec![0; size],
        }
    }
}

/// Borrows immutable processed pixels and constructs the metric exactly once.
///
/// Each call returns one measured trial. Callers can discard inferior bytes
/// instead of retaining all trial buffers. This layer never reads/writes files.
pub struct TrialEvaluator<'a> {
    pixels: &'a Pixels,
    metric: MetricReference,
}

impl<'a> TrialEvaluator<'a> {
    pub fn new(pixels: &'a Pixels) -> Result<Self> {
        Ok(Self {
            pixels,
            metric: MetricReference::new(pixels)?,
        })
    }

    pub fn evaluate(&self, mode: Mode) -> Result<Trial> {
        self.evaluate_inner::<false>(mode).map(|(trial, _)| trial)
    }

    /// Benchmark a trial without changing encoding, decoding or scoring policy.
    /// Ordinary evaluation does not read the clock.
    pub fn evaluate_timed(&self, mode: Mode) -> Result<(Trial, EvaluationTimings)> {
        self.evaluate_inner::<true>(mode)
    }

    fn evaluate_inner<const TIMED: bool>(&self, mode: Mode) -> Result<(Trial, EvaluationTimings)> {
        let start = TIMED.then(Instant::now);
        let quality = match mode {
            Mode::Lossy { quality } => {
                ensure!(quality <= 100, "WebP lossy quality must be in 0..=100");
                f32::from(quality)
            }
            // Matches webp::Encoder's default lossless effort setting.
            Mode::Lossless => 75.0,
        };
        let (width, height) = self.pixels.dimensions();
        // The constructor checked dimensions and exact packed buffer length;
        // the immutable borrow keeps them valid for the encoder's lifetime.
        let encoder = if self.pixels.has_alpha() {
            Encoder::from_rgba(self.pixels.as_bytes(), width, height)
        } else {
            Encoder::from_rgb(self.pixels.as_bytes(), width, height)
        };
        // Avoid encode()/encode_lossless()/encode_simple() config unwraps.
        let mut config = WebPConfig::new()
            .map_err(|_| anyhow!("cannot initialize WebP encoder configuration"))?;
        let lossless = matches!(mode, Mode::Lossless);
        config.lossless = i32::from(lossless);
        config.alpha_compression = i32::from(!lossless);
        config.quality = quality;
        let encoded = encoder
            .encode_advanced(&config)
            .map_err(|error| anyhow!("WebP encoding failed for {mode:?}: {error:?}"))?;
        let encode = start.map_or(Duration::ZERO, |start| start.elapsed());
        let start = TIMED.then(Instant::now);
        // Decode and score even lossless. Hidden RGB may change at alpha=0.
        let decoded = self
            .decode_webp(&encoded)
            .with_context(|| format!("cannot evaluate {mode:?} trial"))?;
        let decode = start.map_or(Duration::ZERO, |start| start.elapsed());
        let start = TIMED.then(Instant::now);
        let score = self
            .metric
            .score(&decoded)
            .with_context(|| format!("cannot evaluate {mode:?} trial"))?;
        let metric = start.map_or(Duration::ZERO, |start| start.elapsed());
        drop(decoded);
        Ok((
            Trial {
                mode,
                score,
                bytes: encoded.to_vec(),
            },
            EvaluationTimings {
                encode,
                decode,
                metric,
            },
        ))
    }

    /// Decode a still WebP in memory and measure its actual pixels.
    ///
    /// Checks the header against the reference before allocating pixel buffers.
    pub fn score_webp(&self, bytes: &[u8]) -> Result<f64> {
        self.metric.score(&self.decode_webp(bytes)?)
    }

    fn decode_webp(&self, bytes: &[u8]) -> Result<Pixels> {
        ensure!(!bytes.is_empty(), "WebP candidate contains no bytes");
        let mut decoder =
            WebPDecoder::new(Cursor::new(bytes)).context("cannot read WebP candidate header")?;
        ensure!(
            !decoder.has_animation(),
            "WebP candidate must be a still image"
        );
        let dimensions = self.pixels.dimensions();
        ensure!(
            decoder.dimensions() == dimensions,
            "WebP candidate dimensions {:?} differ from reference {:?}",
            decoder.dimensions(),
            dimensions
        );
        let expected = checked_buffer_len(
            dimensions.0,
            dimensions.1,
            usize::from(decoder.color_type().bytes_per_pixel()),
        )?;
        ensure!(
            decoder.total_bytes() == expected as u64,
            "inconsistent WebP candidate buffer size"
        );
        let mut limits = Limits::default();
        limits.max_image_width = Some(dimensions.0);
        limits.max_image_height = Some(dimensions.1);
        limits.max_alloc = Some(MAX_ESTIMATED_BYTES);
        limits
            .reserve(expected as u64)
            .context("WebP candidate exceeds decode budget")?;
        decoder
            .set_limits(limits)
            .context("cannot apply WebP candidate decode limits")?;
        let decoded =
            DynamicImage::from_decoder(decoder).context("cannot decode WebP candidate pixels")?;
        let pixels = if self.pixels.has_alpha() {
            // RGB bitstreams are opaque RGBA, not reference-alpha restoration.
            Pixels::Rgba(decoded.into_rgba8())
        } else {
            if decoded.color().has_alpha() {
                ensure!(
                    decoded
                        .as_rgba8()
                        .context("unexpected WebP candidate pixel layout")?
                        .pixels()
                        .all(|pixel| pixel[3] == 255),
                    "WebP candidate has transparency but the reference is opaque"
                );
            }
            Pixels::Rgb(decoded.into_rgb8())
        };
        Ok(pixels)
    }
}
