//! Reproducible Phase 7 workloads and stage timings; see benchmarks/README.md.

use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{ensure, Result};
use clap::{Parser, Subcommand};
use image::{imageops::FilterType, Rgb, RgbImage, Rgba, RgbaImage};
use webptune::encoder::TrialEvaluator;
use webptune::input::{self, Pixels};
use webptune::limits::validate_source_dimensions;
use webptune::metric::MetricReference;
use webptune::search;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate deterministic PNG workloads, independent of the timed pipeline.
    Generate {
        #[arg(long, default_value = "benchmarks/astronaut.png")]
        photo: PathBuf,
        #[arg(long, default_value = "target/benchmark-inputs")]
        directory: PathBuf,
        #[arg(long, value_delimiter = ',', default_value = "256,768,2000")]
        sizes: Vec<u32>,
    },
    /// Warm up once, then report measured searches and replayed stage totals.
    Measure {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, default_value_t = 3)]
        repeats: usize,
        #[arg(long, default_value_t = 80.0)]
        threshold: f64,
        #[arg(long, default_value_t = 1.0)]
        scale: f64,
    },
}

fn main() -> Result<()> {
    match Args::parse().command {
        Command::Generate {
            photo,
            directory,
            sizes,
        } => {
            for &size in &sizes {
                validate_source_dimensions(size, size)?;
            }
            let photo = image::open(photo)?.into_rgb8();
            std::fs::create_dir_all(&directory)?;
            for size in sizes {
                let photo = image::imageops::resize(&photo, size, size, FilterType::Lanczos3);
                photo.save(directory.join(format!("photo-{size}.png")))?;
                noisy_photo(&photo).save(directory.join(format!("noisy-photo-{size}.png")))?;
                icon(size).save(directory.join(format!("icon-{size}.png")))?;
                art(size).save(directory.join(format!("transparent-art-{size}.png")))?;
            }
        }
        Command::Measure {
            input,
            repeats,
            threshold,
            scale,
        } => {
            ensure!((1..=100).contains(&repeats), "repeats must be in 1..=100");
            let start = Instant::now();
            let prepared = input::load(&input, scale)?;
            let load_ms = milliseconds(start.elapsed());
            println!("repeat,load_ms,reference_ms,search_ms,trials,mode,score,bytes,encode_ms,decode_ms,metric_ms,uncached_metric_ms");
            for repeat in 0..=repeats {
                let start = Instant::now();
                let evaluator = TrialEvaluator::new(prepared.pixels())?;
                let reference_ms = milliseconds(start.elapsed());
                let start = Instant::now();
                let result = search::search(&evaluator, threshold)?;
                let search_ms = milliseconds(start.elapsed());
                if repeat == 0 {
                    continue;
                }
                let (mut encode, mut decode, mut metric, mut uncached) = (
                    Duration::ZERO,
                    Duration::ZERO,
                    Duration::ZERO,
                    Duration::ZERO,
                );
                // Replay precisely the tested modes, checking results against the
                // production search. Timings are not added to search_ms.
                for summary in result.trials() {
                    let (trial, timing) = evaluator.evaluate_timed(summary.mode)?;
                    ensure!(
                        trial.score() == summary.score && trial.size() == summary.size,
                        "replayed trial differs from search"
                    );
                    if trial.mode() == result.candidate().mode() {
                        ensure!(
                            trial.bytes() == result.candidate().bytes(),
                            "winner bytes differ"
                        );
                    }
                    encode += timing.encode;
                    decode += timing.decode;
                    metric += timing.metric;
                    // Decode outside this comparison: compare caching with
                    // rebuilding the same metric reference for every trial.
                    let decoded = image::load_from_memory_with_format(
                        trial.bytes(),
                        image::ImageFormat::WebP,
                    )?;
                    let decoded = if prepared.pixels().has_alpha() {
                        Pixels::Rgba(decoded.into_rgba8())
                    } else {
                        Pixels::Rgb(decoded.into_rgb8())
                    };
                    let start = Instant::now();
                    let reference = MetricReference::new(prepared.pixels())?;
                    let score = black_box(reference.score(black_box(&decoded))?);
                    uncached += start.elapsed();
                    ensure!(score == trial.score(), "cached and rebuilt metric disagree");
                }
                let candidate = result.candidate();
                println!("{repeat},{load_ms:.3},{reference_ms:.3},{search_ms:.3},{},{:?},{:.9},{},{:.3},{:.3},{:.3},{:.3}",
                    result.trials().len(), candidate.mode(), candidate.score(), candidate.size(),
                    milliseconds(encode), milliseconds(decode), milliseconds(metric), milliseconds(uncached));
            }
        }
    }
    Ok(())
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn noisy_photo(photo: &RgbImage) -> RgbImage {
    let mut state = 0x5737_2026_u32;
    RgbImage::from_fn(photo.width(), photo.height(), |x, y| {
        let mut pixel = *photo.get_pixel(x, y);
        for sample in &mut pixel.0 {
            // Fixed xorshift32; uniform integer noise in [-32, 32].
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *sample = (i32::from(*sample) + (state % 65) as i32 - 32).clamp(0, 255) as u8;
        }
        pixel
    })
}

fn icon(size: u32) -> RgbImage {
    RgbImage::from_fn(size, size, |x, y| {
        let (x, y) = (
            u64::from(x) * 32 / u64::from(size),
            u64::from(y) * 32 / u64::from(size),
        );
        Rgb(if !(3..29).contains(&x) || !(3..29).contains(&y) {
            [245, 245, 240]
        } else if (8..24).contains(&x) && (8..24).contains(&y) {
            if (14..18).contains(&x) || (14..18).contains(&y) {
                [255, 255, 255]
            } else {
                [28, 120, 84]
            }
        } else {
            [28, 70, 120]
        })
    })
}

fn art(size: u32) -> RgbaImage {
    RgbaImage::from_fn(size, size, |x, y| {
        // Integer geometry keeps the generated pixels deterministic.
        let dx = i64::from(x) * 2 - i64::from(size);
        let dy = i64::from(y) * 2 - i64::from(size);
        let distance = dx * dx + dy * dy;
        let radius = i64::from(size) * i64::from(size);
        let alpha = if distance >= radius {
            0
        } else {
            ((radius - distance) * 255 / radius) as u8
        };
        Rgba([
            ((u64::from(x) * 255 / u64::from(size)) as u8),
            ((u64::from(y) * 255 / u64::from(size)) as u8),
            if (x / (size / 16).max(1) + y / (size / 16).max(1)) % 2 == 0 {
                220
            } else {
                60
            },
            alpha,
        ])
    })
}
