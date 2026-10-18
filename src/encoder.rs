//! In-memory WebP trial encoding, bounded decoding and measured evaluation.

use std::io::Cursor;
use std::time::{Duration, Instant};

use anyhow::{anyhow, ensure, Context, Result};
use clap::{Args, ValueEnum};
use image::{codecs::webp::WebPDecoder, DynamicImage, ImageDecoder, Limits};
use webp::{Encoder, WebPConfig};

use crate::input::Pixels;
use crate::limits::{checked_buffer_len, MAX_ESTIMATED_BYTES};
use crate::metric::MetricReference;

/// Prediction used to compress the alpha plane of lossy WebP candidates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum AlphaFiltering {
    None,
    #[default]
    Fast,
    Best,
}

/// Fixed settings shared by all trials of one image. Quality remains searched.
/// Defaults reproduce the original encoder configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Args)]
pub struct EncoderOptions {
    /// WebP compression method (0 fastest, 6 slowest); applies to both modes
    #[arg(long, default_value_t = 4, value_name = "0..6", value_parser = clap::value_parser!(u8).range(0..=6), allow_hyphen_values = true)]
    pub method: u8,

    /// Compression effort for the lossless candidate (0 fastest, 100 slowest)
    #[arg(long, default_value_t = 75, value_name = "0..100", value_parser = clap::value_parser!(u8).range(0..=100), allow_hyphen_values = true)]
    pub lossless_effort: u8,

    /// Alpha quality for lossy trials (100 preserves alpha exactly); lossless stays exact
    #[arg(long, default_value_t = 100, value_name = "0..100", value_parser = clap::value_parser!(u8).range(0..=100), allow_hyphen_values = true)]
    pub alpha_quality: u8,

    /// Alpha predictive filtering for lossy trials; does not change alpha quality
    #[arg(long, value_enum, default_value = "fast", value_name = "MODE")]
    pub alpha_filtering: AlphaFiltering,

    /// Use slower, sharper RGB-to-YUV conversion for lossy trials
    #[arg(long)]
    pub sharp_yuv: bool,

    /// Preserve hidden RGB under fully transparent pixels in the lossless candidate
    #[arg(long)]
    pub exact: bool,
}

impl Default for EncoderOptions {
    fn default() -> Self {
        Self {
            method: 4,
            lossless_effort: 75,
            alpha_quality: 100,
            alpha_filtering: AlphaFiltering::Fast,
            sharp_yuv: false,
            exact: false,
        }
    }
}

impl EncoderOptions {
    /// Validate library callers too, before constructing a metric or encoding.
    pub fn validate(self) -> Result<()> {
        ensure!(self.method <= 6, "WebP method must be in 0..=6");
        ensure!(
            self.lossless_effort <= 100,
            "WebP lossless effort must be in 0..=100"
        );
        ensure!(
            self.alpha_quality <= 100,
            "WebP alpha quality must be in 0..=100"
        );
        Ok(())
    }

    fn config(self, mode: Mode) -> Result<WebPConfig> {
        let mut config = WebPConfig::new()
            .map_err(|_| anyhow!("cannot initialize WebP encoder configuration"))?;
        let lossless = matches!(mode, Mode::Lossless);
        config.lossless = i32::from(lossless);
        config.alpha_compression = i32::from(!lossless);
        config.quality = match mode {
            Mode::Lossy { quality } => {
                ensure!(quality <= 100, "WebP lossy quality must be in 0..=100");
                f32::from(quality)
            }
            Mode::Lossless => f32::from(self.lossless_effort),
        };
        config.method = i32::from(self.method);
        if !lossless {
            config.alpha_quality = i32::from(self.alpha_quality);
            config.alpha_filtering = match self.alpha_filtering {
                AlphaFiltering::None => 0,
                AlphaFiltering::Fast => 1,
                AlphaFiltering::Best => 2,
            };
            config.use_sharp_yuv = i32::from(self.sharp_yuv);
        }
        config.exact = i32::from(lossless && self.exact);
        Ok(config)
    }
}

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
    options: EncoderOptions,
}

impl<'a> TrialEvaluator<'a> {
    pub fn new(pixels: &'a Pixels) -> Result<Self> {
        Self::with_options(pixels, EncoderOptions::default())
    }

    /// Use one validated configuration for every searched quality and lossless.
    /// Alpha degradation applies only to lossy trials; the decoded score still
    /// includes alpha. Lossless remains exact apart from hidden RGB unless exact
    /// preservation is requested.
    pub fn with_options(pixels: &'a Pixels, options: EncoderOptions) -> Result<Self> {
        options.validate()?;
        Ok(Self {
            pixels,
            metric: MetricReference::new(pixels)?,
            options,
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
        let config = self.options.config(mode)?;
        let (width, height) = self.pixels.dimensions();
        // The constructor checked dimensions and exact packed buffer length;
        // the immutable borrow keeps them valid for the encoder's lifetime.
        let encoder = if self.pixels.has_alpha() {
            Encoder::from_rgba(self.pixels.as_bytes(), width, height)
        } else {
            Encoder::from_rgb(self.pixels.as_bytes(), width, height)
        };
        // Avoid encode()/encode_lossless()/encode_simple() config unwraps.
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
