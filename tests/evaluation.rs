use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use fast_ssim2::{compute_ssimulacra2, PixelDescriptor, PixelSlice};
use image::{ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use tempfile::tempdir;
use webp_ssim_optimizer::encoder::{Mode, TrialEvaluator};
use webp_ssim_optimizer::input::{self, Pixels};
use webp_ssim_optimizer::metric::MetricReference;
use webp_ssim_optimizer::search::{self, SearchOptions};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

// Deliberately bypass the application's decoder, pixel-view helper and cached
// metric. Use the public one-shot metric API and separately calculate strides.
fn independent_score(source: &Pixels, bytes: &[u8]) -> f64 {
    let decoded = image::load_from_memory_with_format(bytes, ImageFormat::WebP).unwrap();
    let (width, height) = source.dimensions();
    assert_eq!(decoded.width(), width);
    assert_eq!(decoded.height(), height);
    let (pixels, channels, descriptor) = if source.has_alpha() {
        (
            decoded.to_rgba8().into_raw(),
            4,
            PixelDescriptor::RGBA8_SRGB,
        )
    } else {
        (decoded.to_rgb8().into_raw(), 3, PixelDescriptor::RGB8_SRGB)
    };
    let source = PixelSlice::new(
        source.as_bytes(),
        width,
        height,
        width as usize * channels,
        descriptor,
    )
    .unwrap();
    let decoded = PixelSlice::new(
        &pixels,
        width,
        height,
        width as usize * channels,
        descriptor,
    )
    .unwrap();
    compute_ssimulacra2(&source, &decoded).unwrap()
}

#[test]
fn identical_rgb_and_rgba_references_score_exactly_100() {
    for name in [
        "rgb.png",
        "rgba.png",
        "tiny-rgb.png",
        "tiny-rgba.png",
        "transparent.png",
    ] {
        for scale in [1.0, 0.5] {
            let image = input::load(&fixture(name), scale).unwrap();
            let reference = MetricReference::new(image.pixels()).unwrap();
            assert_eq!(
                reference.score(image.pixels()).unwrap(),
                100.0,
                "{name} scale={scale}"
            );
            // Repeated comparisons reuse the same cached reference.
            assert_eq!(reference.score(image.pixels()).unwrap(), 100.0);
        }
    }
}

#[test]
fn alpha_scoring_ignores_hidden_rgb_but_measures_partial_rgb_and_alpha() {
    let prepared = input::load(&fixture("rgba.png"), 1.0).unwrap();
    let Pixels::Rgba(source) = prepared.pixels() else {
        panic!("expected RGBA");
    };
    let reference = MetricReference::new(prepared.pixels()).unwrap();
    let mut hidden = source.clone();
    for pixel in hidden.pixels_mut().filter(|pixel| pixel[3] == 0) {
        pixel.0[..3].copy_from_slice(&[255, 13, 241]);
    }
    assert_eq!(reference.score(&Pixels::Rgba(hidden)).unwrap(), 100.0);
    let mut partial = source.clone();
    for pixel in partial.pixels_mut().filter(|pixel| pixel[3] == 160) {
        pixel.0[..3].copy_from_slice(&[255, 13, 241]);
    }
    assert!(reference.score(&Pixels::Rgba(partial)).unwrap() < 100.0);
    let mut alpha = source.clone();
    for pixel in alpha.pixels_mut() {
        pixel[3] = 255;
    }
    assert!(reference.score(&Pixels::Rgba(alpha)).unwrap() < 100.0);
    assert_eq!(reference.score(prepared.pixels()).unwrap(), 100.0);
}

#[test]
fn trials_match_independent_decoded_scoring_for_all_input_formats_and_scales() {
    for name in [
        "rgb.png",
        "rgb.jpg",
        "rgb.bmp",
        "rgb.webp",
        "rgba.png",
        "rgba.webp",
        "tiny-rgb.png",
        "tiny-rgba.png",
        "transparent.png",
    ] {
        for scale in [1.0, 0.5] {
            let prepared = input::load(&fixture(name), scale).unwrap();
            let source = prepared.pixels();
            let evaluator = TrialEvaluator::new(source).unwrap();
            for mode in [
                Mode::Lossy { quality: 0 },
                Mode::Lossy { quality: 75 },
                Mode::Lossy { quality: 100 },
                Mode::Lossless,
            ] {
                let trial = evaluator.evaluate(mode).unwrap();
                assert_eq!(trial.mode(), mode);
                assert!(trial.size() > 0);
                assert_eq!(trial.size(), trial.bytes().len());
                assert_eq!(
                    image::guess_format(trial.bytes()).unwrap(),
                    ImageFormat::WebP
                );
                assert!(trial.score().is_finite());
                assert_eq!(
                    trial.score(),
                    independent_score(source, trial.bytes()),
                    "{name} scale={scale} {mode:?}"
                );
                let decoded =
                    image::load_from_memory_with_format(trial.bytes(), ImageFormat::WebP).unwrap();
                if source.has_alpha() {
                    assert!(source
                        .as_bytes()
                        .chunks_exact(4)
                        .map(|p| p[3])
                        .eq(decoded.to_rgba8().pixels().map(|p| p[3])));
                } else if mode == Mode::Lossless {
                    assert_eq!(decoded.to_rgb8().as_raw(), source.as_bytes());
                    assert_eq!(trial.score(), 100.0);
                }
                let size = trial.size();
                assert_eq!(trial.into_bytes().len(), size);
            }
        }
    }
}

#[test]
fn every_integer_lossy_quality_is_supported_and_out_of_range_is_rejected() {
    for name in ["rgb.png", "rgba.png"] {
        let prepared = input::load(&fixture(name), 1.0).unwrap();
        let evaluator = TrialEvaluator::new(prepared.pixels()).unwrap();
        for quality in 0..=100 {
            assert!(evaluator
                .evaluate(Mode::Lossy { quality })
                .unwrap()
                .score()
                .is_finite());
        }
        for quality in [101, 255] {
            let error = evaluator.evaluate(Mode::Lossy { quality }).unwrap_err();
            assert!(error.to_string().contains("quality must be in 0..=100"));
        }
        assert!(evaluator.evaluate(Mode::Lossless).is_ok());
    }
}

#[test]
fn invalid_pixel_layouts_and_dimensions_fail_before_codec_or_metric_work() {
    for pixels in [
        Pixels::Rgb(RgbImage::new(0, 1)),
        Pixels::Rgba(RgbaImage::new(1, 0)),
        Pixels::Rgb(RgbImage::new(16_384, 1)),
        // ImageBuffer allows a backing buffer larger than its logical image.
        Pixels::Rgb(RgbImage::from_raw(1, 1, vec![1, 2, 3, 4]).unwrap()),
    ] {
        assert!(MetricReference::new(&pixels).is_err());
        assert!(TrialEvaluator::new(&pixels).is_err());
    }
    let source = Pixels::Rgb(RgbImage::from_pixel(2, 3, Rgb([80, 120, 160])));
    let metric = MetricReference::new(&source).unwrap();
    // Equal pixel count does not imply equal image dimensions.
    let wrong_dimensions = Pixels::Rgb(RgbImage::new(3, 2));
    assert!(metric
        .score(&wrong_dimensions)
        .unwrap_err()
        .to_string()
        .contains("dimensions"));
    let wrong_layout = Pixels::Rgba(RgbaImage::new(2, 3));
    assert!(metric
        .score(&wrong_layout)
        .unwrap_err()
        .to_string()
        .contains("layout"));
}

#[test]
fn candidate_errors_do_not_become_successful_zero_scores() {
    let prepared = input::load(&fixture("rgb.png"), 1.0).unwrap();
    let evaluator = TrialEvaluator::new(prepared.pixels()).unwrap();
    for bytes in [
        &b""[..],
        &b"not a WebP"[..],
        include_bytes!("fixtures/corrupt.png"),
    ] {
        assert!(evaluator.score_webp(bytes).is_err());
    }
    let animated = fs::read(fixture("animated.webp")).unwrap();
    assert!(evaluator
        .score_webp(&animated)
        .unwrap_err()
        .to_string()
        .contains("still image"));
    let rgba = fs::read(fixture("rgba.webp")).unwrap();
    assert!(evaluator
        .score_webp(&rgba)
        .unwrap_err()
        .to_string()
        .contains("transparency"));
    let tiny = input::load(&fixture("tiny-rgb.png"), 1.0).unwrap();
    let other = TrialEvaluator::new(tiny.pixels())
        .unwrap()
        .evaluate(Mode::Lossless)
        .unwrap();
    assert!(evaluator
        .score_webp(other.bytes())
        .unwrap_err()
        .to_string()
        .contains("dimensions"));

    let valid = evaluator.evaluate(Mode::Lossless).unwrap();
    // Find truncation after a complete header to exercise pixel-decode errors,
    // in addition to header rejection, without relying on codec byte offsets.
    let mut saw_pixel_decode_error = false;
    for end in 1..valid.size() {
        if let Err(error) = evaluator.score_webp(&valid.bytes()[..end]) {
            saw_pixel_decode_error |= error
                .to_string()
                .contains("cannot decode WebP candidate pixels");
        }
    }
    assert!(saw_pixel_decode_error);
    assert_eq!(evaluator.score_webp(valid.bytes()).unwrap(), 100.0);
}

#[test]
fn transparent_reference_scores_opaque_candidates_with_actual_opaque_alpha() {
    let prepared = input::load(&fixture("rgba.png"), 1.0).unwrap();
    let evaluator = TrialEvaluator::new(prepared.pixels()).unwrap();
    // Dropping alpha is a visible change. Do not copy reference alpha into the
    // decoded RGB candidate just to manufacture a matching layout.
    let rgb = fs::read(fixture("rgb.webp")).unwrap();
    let score = evaluator.score_webp(&rgb).unwrap();
    assert_eq!(score, independent_score(prepared.pixels(), &rgb));
    assert!(score < 100.0);
    let transparent = Pixels::Rgba(RgbaImage::from_pixel(17, 11, Rgba([1, 2, 3, 0])));
    let trial = TrialEvaluator::new(&transparent)
        .unwrap()
        .evaluate(Mode::Lossless)
        .unwrap();
    assert_eq!(
        trial.score(),
        independent_score(&transparent, trial.bytes())
    );
    assert_eq!(trial.score(), 100.0);
}

#[test]
fn evaluator_preserves_source_and_cli_writes_selected_bytes_and_verbose_trials() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("input.png");
    let output = directory.path().join("output.webp");
    let original = fs::read(fixture("rgba.png")).unwrap();
    fs::write(&input, &original).unwrap();
    fs::write(&output, b"existing destination").unwrap();
    let prepared = input::load(&input, 0.5).unwrap();
    let before = prepared.pixels().as_bytes().to_vec();
    let evaluator = TrialEvaluator::new(prepared.pixels()).unwrap();
    for mode in [Mode::Lossy { quality: 75 }, Mode::Lossless] {
        evaluator.evaluate(mode).unwrap();
    }
    let selection = search::search(&evaluator, 100.0).unwrap();
    assert_eq!(prepared.pixels().as_bytes(), before);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    for destination in [output.as_os_str(), std::ffi::OsStr::new("-")] {
        let result = Command::new(env!("CARGO_BIN_EXE_webp-ssim-optimizer"))
            .arg("-i")
            .arg(&input)
            .arg("-o")
            .arg(destination)
            .args(["-s", "0.5", "-t", "100", "-v"])
            .output()
            .unwrap();
        assert!(result.status.success());
        if destination == std::ffi::OsStr::new("-") {
            assert_eq!(result.stdout, selection.candidate().bytes());
            assert_eq!(independent_score(prepared.pixels(), &result.stdout), 100.0);
        } else {
            assert!(result.stdout.is_empty());
            let bytes = fs::read(&output).unwrap();
            assert_eq!(bytes, selection.candidate().bytes());
            assert_eq!(independent_score(prepared.pixels(), &bytes), 100.0);
        }
        let stderr = String::from_utf8(result.stderr).unwrap();
        for expected in [
            "Lossy { quality: 0 }",
            "Lossy { quality: 100 }",
            "Lossless",
            "decoded SSIMULACRA2 score",
            "Wrote WebP",
            ">= threshold 100",
        ] {
            assert!(stderr.contains(expected), "{stderr}");
        }
        assert!(stderr.contains(&format!("{} measured trials", selection.trials().len())));
        assert_eq!(
            stderr
                .lines()
                .filter(|line| line.starts_with("Trial "))
                .count(),
            selection.trials().len()
        );
        for trial in selection.trials() {
            assert!(stderr.contains(&format!(
                "Trial {:?}: {} bytes, decoded SSIMULACRA2 score {:.6}",
                trial.mode, trial.size, trial.score
            )));
        }
    }
    assert_eq!(fs::read(&input).unwrap(), original);
    assert_eq!(fs::read(&output).unwrap(), selection.candidate().bytes());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn search_returns_the_smallest_tested_feasible_bytes_with_independent_score() {
    for name in [
        "rgb.png",
        "rgba.png",
        "tiny-rgb.png",
        "tiny-rgba.png",
        "transparent.png",
    ] {
        for scale in [1.0, 0.5] {
            let prepared = input::load(&fixture(name), scale).unwrap();
            let before = prepared.pixels().as_bytes().to_vec();
            let evaluator = TrialEvaluator::new(prepared.pixels()).unwrap();
            for threshold in [0.0, 80.0, 90.0, 100.0] {
                let result = search::search(&evaluator, threshold).unwrap();
                let selected = result.candidate();
                let score = independent_score(prepared.pixels(), selected.bytes());
                assert_eq!(selected.score(), score, "{name}, {scale}, {threshold}");
                assert!(score >= threshold);
                let mut tested = [false; 101];
                for trial in result.trials() {
                    if let Mode::Lossy { quality } = trial.mode {
                        assert!(!tested[usize::from(quality)]);
                        tested[usize::from(quality)] = true;
                    }
                    // Verify the summaries against real re-evaluation, rather
                    // than trusting the selection routine's accounting alone.
                    let actual = evaluator.evaluate(trial.mode).unwrap();
                    assert_eq!(trial.size, actual.size());
                    assert_eq!(trial.score, actual.score());
                    if actual.score() >= threshold {
                        assert!(selected.size() <= actual.size());
                    }
                }
                assert!(tested[0] && tested[100]);
                assert!(result.trials().len() <= 19);
                assert_eq!(result.trials().last().unwrap().mode, Mode::Lossless);
                // Check tie breaks separately from the search comparator.
                let best = result
                    .trials()
                    .iter()
                    .filter(|trial| trial.score >= threshold)
                    .min_by_key(|trial| {
                        let rank = match trial.mode {
                            Mode::Lossy { quality } => quality,
                            Mode::Lossless => 101,
                        };
                        (trial.size, rank)
                    })
                    .unwrap();
                assert_eq!(selected.mode(), best.mode);
                assert_eq!(selected.score(), best.score);
                assert_eq!(
                    selected.bytes(),
                    evaluator.evaluate(best.mode).unwrap().bytes()
                );
            }
            assert_eq!(prepared.pixels().as_bytes(), before);
        }
    }
}

#[test]
fn minimum_search_budget_still_measures_both_endpoints_and_lossless() {
    for name in ["rgb.png", "rgba.png"] {
        let prepared = input::load(&fixture(name), 1.0).unwrap();
        let evaluator = TrialEvaluator::new(prepared.pixels()).unwrap();
        let result = search::search_with_options(
            &evaluator,
            100.0,
            SearchOptions {
                max_lossy_trials: 2,
            },
        )
        .unwrap();
        assert_eq!(
            result
                .trials()
                .iter()
                .map(|trial| trial.mode)
                .collect::<Vec<_>>(),
            [
                Mode::Lossy { quality: 0 },
                Mode::Lossy { quality: 100 },
                Mode::Lossless
            ]
        );
        assert_eq!(
            independent_score(prepared.pixels(), result.candidate().bytes()),
            100.0
        );
    }
}

#[test]
fn file_and_stdout_match_selected_bytes_and_independent_scores() {
    // Cross every format/alpha case with both scales and every requested
    // threshold at the executable boundary, rather than only testing trials.
    for (name, dimensions) in [
        ("rgb.png", (17, 11)),
        ("rgb.jpg", (17, 11)),
        ("rgb.bmp", (17, 11)),
        ("rgb.webp", (17, 11)),
        ("rgba.png", (17, 11)),
        ("rgba.webp", (17, 11)),
        ("opaque-rgba.png", (17, 11)),
        ("tiny-rgb.png", (1, 1)),
        ("tiny-rgba.png", (1, 1)),
        ("transparent.png", (3, 2)),
        ("alpha-edge.png", (4, 2)),
    ] {
        let directory = tempdir().unwrap();
        let source = directory.path().join("input.png");
        let destination = directory.path().join("output.webp");
        let original = fs::read(fixture(name)).unwrap();
        fs::write(&source, &original).unwrap();
        for scale in [1.0_f64, 0.5] {
            let prepared = input::load(&source, scale).unwrap();
            let expected_dimensions = (
                (f64::from(dimensions.0) * scale).round().max(1.0) as u32,
                (f64::from(dimensions.1) * scale).round().max(1.0) as u32,
            );
            assert_eq!(prepared.original_dimensions(), dimensions);
            assert_eq!(prepared.pixels().dimensions(), expected_dimensions);
            for threshold in [0.0, 80.0, 90.0, 100.0] {
                let selected =
                    search::search(&TrialEvaluator::new(prepared.pixels()).unwrap(), threshold)
                        .unwrap();
                let verbose = threshold == 90.0 || threshold == 100.0;
                // Test both first creation and replacement, for each case.
                if threshold == 0.0 {
                    if destination.exists() {
                        fs::remove_file(&destination).unwrap();
                    }
                } else {
                    fs::write(&destination, b"existing destination").unwrap();
                }
                for target in [destination.as_os_str(), std::ffi::OsStr::new("-")] {
                    let mut command = Command::new(env!("CARGO_BIN_EXE_webp-ssim-optimizer"));
                    command.arg("-i").arg(&source).arg("-o").arg(target).args([
                        "-s",
                        &scale.to_string(),
                        "-t",
                        &threshold.to_string(),
                    ]);
                    if verbose {
                        command.arg("-v");
                    }
                    let result = command.output().unwrap();
                    assert!(
                        result.status.success(),
                        "{name} scale={scale} threshold={threshold}: {result:?}"
                    );
                    let bytes = if target == std::ffi::OsStr::new("-") {
                        result.stdout
                    } else {
                        assert!(result.stdout.is_empty());
                        fs::read(&destination).unwrap()
                    };
                    assert_eq!(bytes, selected.candidate().bytes());
                    let decoded =
                        image::load_from_memory_with_format(&bytes, ImageFormat::WebP).unwrap();
                    assert_eq!((decoded.width(), decoded.height()), expected_dimensions);
                    if prepared.pixels().has_alpha() {
                        assert!(prepared
                            .pixels()
                            .as_bytes()
                            .chunks_exact(4)
                            .map(|p| p[3])
                            .eq(decoded.to_rgba8().pixels().map(|p| p[3])));
                    }
                    let score = independent_score(prepared.pixels(), &bytes);
                    assert_eq!(score, selected.candidate().score());
                    assert!(score >= threshold);
                    let stderr = String::from_utf8(result.stderr).unwrap();
                    assert_eq!(stderr.contains("Trial "), verbose);
                    assert!(stderr.contains(&format!("selected {:?}", selected.candidate().mode())));
                    assert!(stderr.contains(&format!("{} bytes", bytes.len())));
                    assert!(stderr.contains(&format!("score {:.6}", score)));
                    assert!(stderr.contains(&format!(
                        "{}x{} -> {}x{}",
                        prepared.original_dimensions().0,
                        prepared.original_dimensions().1,
                        prepared.pixels().dimensions().0,
                        prepared.pixels().dimensions().1
                    )));
                    assert!(
                        stderr.contains(&format!("{} measured trials", selected.trials().len()))
                    );
                    assert_eq!(fs::read(&source).unwrap(), original);
                    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
                }
            }
        }
    }
}
