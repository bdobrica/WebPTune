//! Size policies and quiet reporting at the executable boundary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use fast_ssim2::{compute_ssimulacra2, PixelDescriptor, PixelSlice};
use tempfile::{tempdir, TempDir};
use webptune::encoder::{Mode, TrialEvaluator};
use webptune::input::{self, Pixels};
use webptune::limits::Resize;

const OLD: &[u8] = b"existing destination";

struct Fixture {
    directory: TempDir,
    input: PathBuf,
    destination: PathBuf,
    original: Vec<u8>,
}

impl Fixture {
    fn new(bytes: &[u8]) -> Self {
        let directory = tempdir().unwrap();
        let input = directory.path().join("source");
        let destination = directory.path().join("output.webp");
        fs::write(&input, bytes).unwrap();
        Self {
            directory,
            input,
            destination,
            original: bytes.to_vec(),
        }
    }

    fn run(&self, stdout: bool, flags: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_webptune"))
            .arg("-i")
            .arg(&self.input)
            .arg("-o")
            .arg(if stdout {
                Path::new("-")
            } else {
                &self.destination
            })
            .args(flags)
            .output()
            .unwrap()
    }

    fn unchanged(&self, destination: bool) {
        assert_eq!(fs::read(&self.input).unwrap(), self.original);
        if destination {
            assert_eq!(fs::read(&self.destination).unwrap(), OLD);
        } else {
            assert!(!self.destination.exists());
        }
        assert_eq!(
            fs::read_dir(self.directory.path()).unwrap().count(),
            if destination { 2 } else { 1 }
        );
    }
}

// Use a separate decoder and one-shot metric, bypassing cached application scoring.
fn verify(reference: &Pixels, bytes: &[u8], threshold: f64) {
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP).unwrap();
    let (width, height) = reference.dimensions();
    assert_eq!((decoded.width(), decoded.height()), (width, height));
    let (pixels, channels, descriptor) = if reference.has_alpha() {
        let pixels = decoded.to_rgba8();
        assert_eq!(
            pixels
                .as_raw()
                .iter()
                .skip(3)
                .step_by(4)
                .copied()
                .collect::<Vec<_>>(),
            reference
                .as_bytes()
                .iter()
                .skip(3)
                .step_by(4)
                .copied()
                .collect::<Vec<_>>()
        );
        (pixels.into_raw(), 4, PixelDescriptor::RGBA8_SRGB)
    } else {
        (decoded.to_rgb8().into_raw(), 3, PixelDescriptor::RGB8_SRGB)
    };
    let source = PixelSlice::new(
        reference.as_bytes(),
        width,
        height,
        width as usize * channels,
        descriptor,
    )
    .unwrap();
    let output = PixelSlice::new(
        &pixels,
        width,
        height,
        width as usize * channels,
        descriptor,
    )
    .unwrap();
    assert!(compute_ssimulacra2(&source, &output).unwrap() >= threshold);
}

#[test]
fn byte_caps_preserve_quality_and_match_file_stdout_payloads_in_both_search_modes() {
    for (source, resize, threshold, resize_flags) in [
        (
            include_bytes!("fixtures/rgb.png").as_slice(),
            Resize::Scale(1.0),
            80.0,
            vec![],
        ),
        (
            include_bytes!("fixtures/rgba.png").as_slice(),
            Resize::Bounds {
                width: Some(9),
                height: Some(5),
            },
            100.0,
            vec!["--scale-width", "9", "--scale-height", "5"],
        ),
    ] {
        for search in ["heuristic", "exhaustive"] {
            let fixture = Fixture::new(source);
            let threshold_arg = threshold.to_string();
            let mut flags = vec!["--search", search, "-t", &threshold_arg];
            flags.extend_from_slice(&resize_flags);
            let baseline = fixture.run(true, &flags);
            assert!(baseline.status.success(), "{baseline:?}");
            assert!(!baseline.stdout.is_empty());
            let reference = input::load_with_resize(&fixture.input, resize).unwrap();
            verify(reference.pixels(), &baseline.stdout, threshold);
            assert!(baseline.stdout.len() < source.len());
            let size = baseline.stdout.len();
            fs::write(&fixture.destination, OLD).unwrap();
            for limit in [size - 1, size, size + 1] {
                let limit_arg = limit.to_string();
                let mut capped = flags.clone();
                capped.extend(["--max-bytes", &limit_arg, "--skip-if-larger"]);
                for stdout in [false, true] {
                    fs::write(&fixture.destination, OLD).unwrap();
                    let output = fixture.run(stdout, &capped);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if limit < size {
                        assert_eq!(output.status.code(), Some(1));
                        assert!(output.stdout.is_empty());
                        assert!(
                            stderr
                                .contains(&format!("{size} bytes, exceeding --max-bytes {limit}")),
                            "{stderr}"
                        );
                        assert!(!stderr.contains("Wrote WebP"));
                        fixture.unchanged(true);
                    } else {
                        assert!(output.status.success(), "{stderr}");
                        assert!(stderr.contains("Wrote WebP"));
                        if stdout {
                            assert_eq!(output.stdout, baseline.stdout);
                            fixture.unchanged(true);
                        } else {
                            assert!(output.stdout.is_empty());
                            assert_eq!(fs::read(&fixture.destination).unwrap(), baseline.stdout);
                            assert_eq!(fs::read(&fixture.input).unwrap(), source);
                            assert_eq!(fs::read_dir(fixture.directory.path()).unwrap().count(), 2);
                        }
                    }
                }
            }
        }
    }
}

// A small, heavily compressed JPEG converted at q=0 yields WebP whose decoded
// pixels need more bytes when re-encoded at a threshold of 100.
fn compressed_source() -> Vec<u8> {
    let fixture = Fixture::new(include_bytes!("fixtures/rgb.jpg"));
    let reference = input::load(&fixture.input, 1.0).unwrap();
    TrialEvaluator::new(reference.pixels())
        .unwrap()
        .evaluate(Mode::Lossy { quality: 0 })
        .unwrap()
        .bytes()
        .to_vec()
}

#[test]
fn larger_results_skip_without_creating_replacing_or_writing_stdout() {
    let source = compressed_source();
    for search in ["heuristic", "exhaustive"] {
        let fixture = Fixture::new(&source);
        let flags = ["--search", search, "-t", "100"];
        let baseline = fixture.run(true, &flags);
        assert!(baseline.status.success(), "{baseline:?}");
        assert!(baseline.stdout.len() > source.len());
        for quiet in [false, true] {
            let mut skipping = flags.to_vec();
            skipping.push("--skip-if-larger");
            if quiet {
                skipping.push("--quiet");
            }
            for (stdout, existing) in [(false, false), (false, true), (true, true)] {
                if existing {
                    fs::write(&fixture.destination, OLD).unwrap();
                }
                let output = fixture.run(stdout, &skipping);
                assert!(output.status.success(), "{output:?}");
                assert!(output.stdout.is_empty());
                if quiet {
                    assert!(output.stderr.is_empty());
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    assert!(stderr.contains("Skipped WebP"), "{stderr}");
                    assert!(stderr.contains(&format!(
                        "{} bytes > input {} bytes",
                        baseline.stdout.len(),
                        source.len()
                    )));
                    assert!(!stderr.contains("Wrote WebP"));
                }
                fixture.unchanged(existing);
                if existing {
                    fs::remove_file(&fixture.destination).unwrap();
                }
            }
        }
        // A permissive cap still permits the skip; a failing cap takes priority.
        let cap = baseline.stdout.len().to_string();
        let mut capped = flags.to_vec();
        capped.extend(["--skip-if-larger", "--max-bytes", &cap, "--verbose"]);
        let output = fixture.run(false, &capped);
        assert!(output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Trial ") && stderr.contains("Skipped WebP"));
        fixture.unchanged(false);
        let limit = (baseline.stdout.len() - 1).to_string();
        let mut capped = flags.to_vec();
        capped.extend(["--skip-if-larger", "--max-bytes", &limit, "--quiet"]);
        for stdout in [false, true] {
            fs::write(&fixture.destination, OLD).unwrap();
            let output = fixture.run(stdout, &capped);
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("--max-bytes"), "{stderr}");
            assert!(!stderr.contains("Skipped WebP"));
            fixture.unchanged(true);
        }
    }
}

#[test]
fn quiet_suppresses_reports_preserves_payloads_and_keeps_errors() {
    for flag in ["--help", "--version"] {
        let output = Command::new(env!("CARGO_BIN_EXE_webptune"))
            .args(["--quiet", flag])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(String::from_utf8_lossy(&output.stdout).contains("webptune"));
    }
    let fixture = Fixture::new(include_bytes!("fixtures/rgb.png"));
    let baseline = fixture.run(true, &[]);
    assert!(baseline.status.success());
    for flag in ["-q", "--quiet"] {
        for stdout in [false, true] {
            let output = fixture.run(stdout, &[flag]);
            assert!(output.status.success(), "{output:?}");
            assert!(output.stderr.is_empty());
            if stdout {
                assert_eq!(output.stdout, baseline.stdout);
            } else {
                assert!(output.stdout.is_empty());
                assert_eq!(fs::read(&fixture.destination).unwrap(), baseline.stdout);
            }
        }
    }
    fs::write(&fixture.destination, OLD).unwrap();
    let error = fixture.run(false, &["--quiet", "--max-bytes", "1"]);
    assert_eq!(error.status.code(), Some(1));
    assert!(error.stdout.is_empty());
    assert!(String::from_utf8_lossy(&error.stderr).contains("--max-bytes"));
    fixture.unchanged(true);

    let fixture = Fixture::new(b"corrupt input");
    let error = fixture.run(false, &["--quiet", "--skip-if-larger"]);
    assert_eq!(error.status.code(), Some(1));
    assert!(error.stdout.is_empty());
    assert!(String::from_utf8_lossy(&error.stderr).contains("unsupported"));
    fixture.unchanged(false);
}

#[test]
fn invalid_size_and_reporting_flags_fail_before_output_and_keep_diagnostics() {
    let fixture = Fixture::new(include_bytes!("fixtures/rgb.png"));
    fs::write(&fixture.destination, OLD).unwrap();
    for flags in [
        vec!["--quiet", "--max-bytes", "0"],
        vec!["--quiet", "--max-bytes", "-1"],
        vec!["--quiet", "--max-bytes", "1.5"],
        vec!["--quiet", "--max-bytes", "18446744073709551616"],
        vec!["--quiet", "--max-bytes"],
        vec!["-q", "-v"],
        vec!["--verbose", "--quiet"],
    ] {
        let output = fixture.run(false, &flags);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
        fixture.unchanged(true);
    }
    // A skip cannot bypass path validation, even if the destination aliases input.
    let output = Command::new(env!("CARGO_BIN_EXE_webptune"))
        .arg("-i")
        .arg(&fixture.input)
        .arg("-o")
        .arg(&fixture.input)
        .args(["--skip-if-larger", "--quiet"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("refers to the input"));
    fixture.unchanged(true);
}
