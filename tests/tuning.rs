use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use fast_ssim2::{compute_ssimulacra2, PixelDescriptor, PixelSlice};
use image::{ImageFormat, Rgba, RgbaImage};
use tempfile::tempdir;
use webp::{Encoder, WebPConfig};
use webptune::cli::Args;
use webptune::encoder::{AlphaFiltering, EncoderOptions, Mode, TrialEvaluator};
use webptune::input::{self, Pixels};
use webptune::search::{self, SearchMode};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn gradient() -> Pixels {
    Pixels::Rgba(RgbaImage::from_fn(64, 16, |x, y| {
        Rgba([
            (x * 3 + y * 5) as u8,
            (x * 7 + y * 11) as u8,
            (x * 13 + y * 17) as u8,
            (x * 4 + y % 4) as u8,
        ])
    }))
}

// Separate decoder and one-shot metric, without the application's score path.
fn score(pixels: &Pixels, bytes: &[u8]) -> f64 {
    let decoded = image::load_from_memory_with_format(bytes, ImageFormat::WebP).unwrap();
    assert_eq!((decoded.width(), decoded.height()), pixels.dimensions());
    let (decoded, channels, descriptor) = if pixels.has_alpha() {
        (
            decoded.to_rgba8().into_raw(),
            4,
            PixelDescriptor::RGBA8_SRGB,
        )
    } else {
        (decoded.to_rgb8().into_raw(), 3, PixelDescriptor::RGB8_SRGB)
    };
    let (width, height) = pixels.dimensions();
    let source = PixelSlice::new(
        pixels.as_bytes(),
        width,
        height,
        width as usize * channels,
        descriptor,
    )
    .unwrap();
    let candidate = PixelSlice::new(
        &decoded,
        width,
        height,
        width as usize * channels,
        descriptor,
    )
    .unwrap();
    compute_ssimulacra2(&source, &candidate).unwrap()
}

fn encode(pixels: &Pixels, config: &WebPConfig) -> Vec<u8> {
    let (width, height) = pixels.dimensions();
    let encoder = if pixels.has_alpha() {
        Encoder::from_rgba(pixels.as_bytes(), width, height)
    } else {
        Encoder::from_rgb(pixels.as_bytes(), width, height)
    };
    encoder.encode_advanced(config).unwrap().to_vec()
}

#[test]
fn defaults_match_the_original_encoder_byte_for_byte() {
    let parsed = Args::try_parse_from(["webptune", "-i", "in", "-o", "out"]).unwrap();
    assert_eq!(parsed.encoder, EncoderOptions::default());
    for name in ["rgb.png", "rgba.png", "transparent.png"] {
        let prepared = input::load(&fixture(name), 1.0).unwrap();
        let pixels = prepared.pixels();
        let evaluator = TrialEvaluator::new(pixels).unwrap();
        for mode in [
            Mode::Lossy { quality: 0 },
            Mode::Lossy { quality: 75 },
            Mode::Lossy { quality: 100 },
            Mode::Lossless,
        ] {
            // The pre-tuning implementation's configuration, constructed independently.
            let mut config = WebPConfig::new().unwrap();
            config.lossless = i32::from(mode == Mode::Lossless);
            config.alpha_compression = i32::from(mode != Mode::Lossless);
            config.quality = match mode {
                Mode::Lossy { quality } => quality as f32,
                Mode::Lossless => 75.0,
            };
            let original = encode(pixels, &config);
            let trial = evaluator.evaluate(mode).unwrap();
            assert_eq!(trial.bytes(), original);
            assert_eq!(trial.score(), score(pixels, &original));
        }
    }
}

#[test]
fn encoder_controls_reach_libwebp_and_timed_evaluation() {
    let pixels = gradient();
    let mut cases = Vec::new();
    for method in 0..=6 {
        cases.push(EncoderOptions {
            method,
            ..Default::default()
        });
    }
    for lossless_effort in [0, 100] {
        cases.push(EncoderOptions {
            lossless_effort,
            ..Default::default()
        });
    }
    for alpha_quality in [0, 37, 99] {
        cases.push(EncoderOptions {
            alpha_quality,
            ..Default::default()
        });
    }
    for alpha_filtering in [AlphaFiltering::None, AlphaFiltering::Best] {
        cases.push(EncoderOptions {
            alpha_filtering,
            ..Default::default()
        });
    }
    cases.push(EncoderOptions {
        sharp_yuv: true,
        ..Default::default()
    });
    cases.push(EncoderOptions {
        exact: true,
        ..Default::default()
    });
    cases.push(EncoderOptions {
        method: 6,
        lossless_effort: 100,
        alpha_quality: 0,
        alpha_filtering: AlphaFiltering::Best,
        sharp_yuv: true,
        exact: true,
    });
    for options in cases {
        let evaluator = TrialEvaluator::with_options(&pixels, options).unwrap();
        for mode in [Mode::Lossy { quality: 73 }, Mode::Lossless] {
            let mut config = WebPConfig::new().unwrap();
            config.method = options.method.into();
            if mode == Mode::Lossless {
                config.exact = options.exact.into();
                config.lossless = 1;
                config.alpha_compression = 0;
                config.quality = options.lossless_effort.into();
            } else {
                config.quality = 73.0;
                config.alpha_quality = options.alpha_quality.into();
                config.alpha_filtering = match options.alpha_filtering {
                    AlphaFiltering::None => 0,
                    AlphaFiltering::Fast => 1,
                    AlphaFiltering::Best => 2,
                };
                config.use_sharp_yuv = options.sharp_yuv.into();
            }
            let expected = encode(&pixels, &config);
            let trial = evaluator.evaluate(mode).unwrap();
            assert_eq!(trial.bytes(), expected, "{options:?} {mode:?}");
            assert_eq!(trial.score(), score(&pixels, &expected));
            let (timed, _) = evaluator.evaluate_timed(mode).unwrap();
            assert_eq!(timed.bytes(), trial.bytes());
            assert_eq!(timed.score(), trial.score());
        }
    }
}

#[test]
fn reduced_alpha_is_measured_and_lossless_remains_a_true_fallback() {
    let pixels = gradient();
    let full = TrialEvaluator::new(&pixels)
        .unwrap()
        .evaluate(Mode::Lossy { quality: 80 })
        .unwrap();
    let evaluator = TrialEvaluator::with_options(
        &pixels,
        EncoderOptions {
            alpha_quality: 0,
            ..Default::default()
        },
    )
    .unwrap();
    let reduced = evaluator.evaluate(Mode::Lossy { quality: 80 }).unwrap();
    let full = image::load_from_memory(full.bytes()).unwrap().to_rgba8();
    let reduced_pixels = image::load_from_memory(reduced.bytes()).unwrap().to_rgba8();
    assert!(full
        .pixels()
        .map(|p| p[3])
        .eq(pixels.as_bytes().chunks_exact(4).map(|p| p[3])));
    assert!(full
        .pixels()
        .zip(reduced_pixels.pixels())
        .any(|(a, b)| a[3] != b[3]));
    assert_eq!(reduced.score(), score(&pixels, reduced.bytes()));
    let lossless = evaluator.evaluate(Mode::Lossless).unwrap();
    let decoded = image::load_from_memory(lossless.bytes())
        .unwrap()
        .to_rgba8();
    assert!(decoded
        .pixels()
        .map(|p| p[3])
        .eq(pixels.as_bytes().chunks_exact(4).map(|p| p[3])));
    assert_eq!(lossless.score(), 100.0);
    for mode in [SearchMode::Heuristic, SearchMode::Exhaustive] {
        let result = search::search_with_mode(&evaluator, 100.0, mode).unwrap();
        assert_eq!(result.candidate().mode(), Mode::Lossless);
        assert_eq!(score(&pixels, result.candidate().bytes()), 100.0);
    }
}

#[test]
fn exact_preserves_hidden_rgb_and_filtering_preserves_alpha_samples() {
    let pixels = gradient();
    let options = EncoderOptions {
        exact: true,
        ..Default::default()
    };
    let exact = TrialEvaluator::with_options(&pixels, options)
        .unwrap()
        .evaluate(Mode::Lossless)
        .unwrap();
    assert_eq!(
        image::load_from_memory(exact.bytes())
            .unwrap()
            .to_rgba8()
            .as_raw(),
        pixels.as_bytes()
    );
    let ordinary = TrialEvaluator::new(&pixels)
        .unwrap()
        .evaluate(Mode::Lossless)
        .unwrap();
    assert_ne!(
        image::load_from_memory(ordinary.bytes())
            .unwrap()
            .to_rgba8()
            .as_raw(),
        pixels.as_bytes()
    );
    for alpha_filtering in [
        AlphaFiltering::None,
        AlphaFiltering::Fast,
        AlphaFiltering::Best,
    ] {
        let evaluator = TrialEvaluator::with_options(
            &pixels,
            EncoderOptions {
                alpha_filtering,
                ..options
            },
        )
        .unwrap();
        let trial = evaluator.evaluate(Mode::Lossy { quality: 75 }).unwrap();
        let decoded = image::load_from_memory(trial.bytes()).unwrap().to_rgba8();
        assert!(decoded
            .pixels()
            .map(|p| p[3])
            .eq(pixels.as_bytes().chunks_exact(4).map(|p| p[3])));
    }
}

#[test]
fn invalid_cli_and_library_settings_fail_before_output() {
    for method in ["0", "6"] {
        for quality in ["0", "100"] {
            for filter in ["none", "fast", "best"] {
                let args = Args::try_parse_from([
                    "webptune",
                    "-i",
                    "in",
                    "-o",
                    "out",
                    "--method",
                    method,
                    "--alpha-quality",
                    quality,
                    "--lossless-effort",
                    quality,
                    "--alpha-filtering",
                    filter,
                    "--sharp-yuv",
                    "--exact",
                ])
                .unwrap();
                args.encoder.validate().unwrap();
                assert_eq!(args.encoder.method, method.parse::<u8>().unwrap());
                assert_eq!(args.encoder.alpha_quality, quality.parse::<u8>().unwrap());
                assert_eq!(args.encoder.lossless_effort, quality.parse::<u8>().unwrap());
                assert!(args.encoder.sharp_yuv && args.encoder.exact);
            }
        }
    }
    let directory = tempdir().unwrap();
    let destination = directory.path().join("output.webp");
    fs::write(&destination, b"keep destination").unwrap();
    for (flag, invalid) in [
        ("--method", vec!["-1", "7", "256", "1.5", "NaN", ""]),
        (
            "--alpha-quality",
            vec!["-1", "101", "256", "1.5", "inf", ""],
        ),
        (
            "--lossless-effort",
            vec!["-1", "101", "256", "1.5", "inf", ""],
        ),
        ("--alpha-filtering", vec!["0", "BEST", "unknown", ""]),
    ] {
        for value in invalid {
            for target in [destination.as_path(), Path::new("-")] {
                let output = Command::new(env!("CARGO_BIN_EXE_webptune"))
                    .arg("-i")
                    .arg(fixture("rgba.png"))
                    .arg("-o")
                    .arg(target)
                    .args([flag, value])
                    .output()
                    .unwrap();
                assert_eq!(output.status.code(), Some(2), "{flag}={value}");
                assert!(output.stdout.is_empty());
                assert!(!output.stderr.is_empty());
                assert_eq!(fs::read(&destination).unwrap(), b"keep destination");
            }
        }
        let missing = Args::try_parse_from(["webptune", "-i", "in", "-o", "out", flag]);
        assert!(missing.is_err());
    }
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    for options in [
        EncoderOptions {
            method: 7,
            ..Default::default()
        },
        EncoderOptions {
            lossless_effort: 101,
            ..Default::default()
        },
        EncoderOptions {
            alpha_quality: 255,
            ..Default::default()
        },
    ] {
        assert!(TrialEvaluator::with_options(&gradient(), options).is_err());
    }
}

#[test]
fn cli_options_match_selection_in_both_search_modes_and_parallel_batches() {
    let directory = tempdir().unwrap();
    let sources = directory.path().join("sources");
    let outputs = directory.path().join("outputs");
    fs::create_dir(&sources).unwrap();
    fs::create_dir(&outputs).unwrap();
    let options = EncoderOptions {
        method: 6,
        lossless_effort: 100,
        alpha_quality: 23,
        alpha_filtering: AlphaFiltering::Best,
        sharp_yuv: true,
        exact: true,
    };
    let flags = [
        "--method",
        "6",
        "--lossless-effort",
        "100",
        "--alpha-quality",
        "23",
        "--alpha-filtering",
        "best",
        "--sharp-yuv",
        "--exact",
    ];
    for name in ["rgb.png", "rgba.png"] {
        fs::copy(fixture(name), sources.join(name)).unwrap();
    }
    for (mode, strategy) in [
        (SearchMode::Heuristic, "heuristic"),
        (SearchMode::Exhaustive, "exhaustive"),
    ] {
        for threshold in [80.0, 100.0] {
            let threshold = threshold.to_string();
            for name in ["rgb.png", "rgba.png"] {
                let source = sources.join(name);
                let original = fs::read(&source).unwrap();
                let reference = input::load(&source, 0.5).unwrap();
                let evaluator = TrialEvaluator::with_options(reference.pixels(), options).unwrap();
                let result =
                    search::search_with_mode(&evaluator, threshold.parse().unwrap(), mode).unwrap();
                let selected = result.candidate();
                assert!(score(reference.pixels(), selected.bytes()) >= threshold.parse().unwrap());
                let destination = outputs.join(Path::new(name).with_extension("webp"));
                for target in [destination.as_path(), Path::new("-")] {
                    let output = Command::new(env!("CARGO_BIN_EXE_webptune"))
                        .arg("-i")
                        .arg(&source)
                        .arg("-o")
                        .arg(target)
                        .args(flags)
                        .args([
                            "--scale",
                            "0.5",
                            "--threshold",
                            &threshold,
                            "--search",
                            strategy,
                            "--verbose",
                        ])
                        .output()
                        .unwrap();
                    assert!(
                        output.status.success(),
                        "{}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    assert!(String::from_utf8_lossy(&output.stderr).contains("Encoder settings:"));
                    let bytes = if target == Path::new("-") {
                        output.stdout
                    } else {
                        assert!(output.stdout.is_empty());
                        fs::read(target).unwrap()
                    };
                    assert_eq!(bytes, selected.bytes());
                }
                // A byte ceiling still fails before replacing the existing result.
                let output = Command::new(env!("CARGO_BIN_EXE_webptune"))
                    .arg("-i")
                    .arg(&source)
                    .arg("-o")
                    .arg(&destination)
                    .args(flags)
                    .args([
                        "--scale",
                        "0.5",
                        "--threshold",
                        &threshold,
                        "--search",
                        strategy,
                        "--max-bytes",
                        "1",
                        "--quiet",
                    ])
                    .output()
                    .unwrap();
                assert_eq!(output.status.code(), Some(1));
                assert!(output.stdout.is_empty() && !output.stderr.is_empty());
                assert_eq!(fs::read(&destination).unwrap(), selected.bytes());
                assert_eq!(fs::read(&source).unwrap(), original);
            }
            let expected: Vec<_> = ["rgb.webp", "rgba.webp"]
                .into_iter()
                .map(|name| fs::read(outputs.join(name)).unwrap())
                .collect();
            let batch = Command::new(env!("CARGO_BIN_EXE_webptune"))
                .args(["--batch", "--jobs", "2", "--quiet"])
                .arg("-i")
                .arg(&sources)
                .arg("-o")
                .arg(&outputs)
                .args(flags)
                .args([
                    "--scale",
                    "0.5",
                    "--threshold",
                    &threshold,
                    "--search",
                    strategy,
                ])
                .output()
                .unwrap();
            assert!(
                batch.status.success(),
                "{}",
                String::from_utf8_lossy(&batch.stderr)
            );
            assert!(batch.stdout.is_empty() && batch.stderr.is_empty());
            for (name, expected) in ["rgb.webp", "rgba.webp"].into_iter().zip(expected) {
                assert_eq!(fs::read(outputs.join(name)).unwrap(), expected);
            }
        }
    }
    assert_eq!(fs::read_dir(outputs).unwrap().count(), 2);
}
