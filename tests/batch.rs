//! Directory mode, independently verified output and bounded whole-image work.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Barrier, Mutex};

use clap::Parser;
use fast_ssim2::{compute_ssimulacra2, PixelDescriptor, PixelSlice};
use tempfile::{tempdir, TempDir};
use webptune::batch::BatchPlan;
use webptune::cli::Args;
use webptune::encoder::{Mode, TrialEvaluator};
use webptune::input;
use webptune::limits::Resize;
use webptune::output::WriteOutcome;

const RGB: &[u8] = include_bytes!("fixtures/rgb.png");
const RGBA: &[u8] = include_bytes!("fixtures/rgba.png");
const OLD: &[u8] = b"old destination must survive failure";

struct Fixture {
    _directory: TempDir,
    input: PathBuf,
    output: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempdir().unwrap();
        let input = directory.path().join("images");
        let output = directory.path().join("converted");
        fs::create_dir(&input).unwrap();
        fs::create_dir(&output).unwrap();
        Self {
            _directory: directory,
            input,
            output,
        }
    }

    fn add(&self, name: &str, bytes: &[u8]) {
        let path = self.input.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn run(&self, flags: &[&str]) -> Output {
        run(&self.input, &self.output, flags)
    }

    fn args(&self, flags: &[&str]) -> Args {
        Args::try_parse_from(
            [
                Path::new("webptune"),
                Path::new("--batch"),
                Path::new("-i"),
                &self.input,
                Path::new("-o"),
                &self.output,
            ]
            .into_iter()
            .chain(flags.iter().map(Path::new)),
        )
        .unwrap()
    }
}

fn run(input: &Path, output: &Path, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_webptune"))
        .args(["--batch", "-i"])
        .arg(input)
        .arg("-o")
        .arg(output)
        .args(flags)
        .output()
        .unwrap()
}

fn failure(output: &Output, diagnostic: &str) {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(diagnostic),
        "expected {diagnostic:?}: {stderr}"
    );
}

fn verify(input: &Path, output: &Path, resize: Resize, threshold: f64, flags: &[&str]) {
    let reference = input::load_with_resize(input, resize).unwrap();
    let reference = reference.pixels();
    let bytes = fs::read(output).unwrap();
    let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).unwrap();
    let (width, height) = reference.dimensions();
    assert_eq!((decoded.width(), decoded.height()), (width, height));
    let (pixels, channels, descriptor) = if reference.has_alpha() {
        let pixels = decoded.to_rgba8().into_raw();
        assert_eq!(
            pixels.iter().skip(3).step_by(4).collect::<Vec<_>>(),
            reference
                .as_bytes()
                .iter()
                .skip(3)
                .step_by(4)
                .collect::<Vec<_>>()
        );
        (pixels, 4, PixelDescriptor::RGBA8_SRGB)
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
    let single = Command::new(env!("CARGO_BIN_EXE_webptune"))
        .arg("-i")
        .arg(input)
        .args(["-o", "-", "--quiet"])
        .args(flags)
        .output()
        .unwrap();
    assert!(single.status.success(), "{single:?}");
    assert!(single.stderr.is_empty());
    assert_eq!(bytes, single.stdout);
}

#[test]
fn top_level_formats_unicode_and_content_detection_match_single_conversions() {
    let f = Fixture::new();
    let cases = [
        ("rgb.png", RGB),
        ("photo.JPEG", include_bytes!("fixtures/rgb.jpg").as_slice()),
        ("bitmap.BMP", include_bytes!("fixtures/rgb.bmp").as_slice()),
        (
            "encoded.WebP",
            include_bytes!("fixtures/rgb.webp").as_slice(),
        ),
        ("imagine șț-日本語.png", RGBA),
        ("mislabelled.jpg", RGB),
    ];
    for (name, bytes) in cases {
        f.add(name, bytes);
    }
    f.add("nested/ignored.png", RGB);
    f.add("extensionless", RGB);
    f.add("notes.txt", RGB);
    f.add(
        "unsupported.gif",
        include_bytes!("fixtures/unsupported.gif"),
    );
    fs::write(f.output.join("rgb.webp"), OLD).unwrap();
    let flags = [
        "--jobs",
        "2",
        "--scale",
        "0.5",
        "--threshold",
        "90",
        "--verbose",
    ];
    let output = f.run(&flags);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Batch complete: 6 written, 0 skipped, 0 failed"));
    assert_eq!(stderr.matches("File '").count(), 6);
    // Each verbose history stays next to its own report despite parallel work.
    for block in stderr.split("File '").skip(1) {
        assert_eq!(block.matches("Wrote WebP:").count(), 1, "{block}");
        assert!(block.contains("Trial "));
    }
    for (name, bytes) in cases {
        verify(
            &f.input.join(name),
            &f.output.join(Path::new(name).with_extension("webp")),
            Resize::Scale(0.5),
            90.0,
            &["--scale", "0.5", "--threshold", "90"],
        );
        assert_eq!(fs::read(f.input.join(name)).unwrap(), bytes);
    }
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 6);
}

#[test]
fn recursive_parallel_exhaustive_outputs_preserve_tree_and_resize_alpha() {
    let f = Fixture::new();
    for (name, bytes) in [
        ("one.png", RGB),
        ("nested/two.png", RGBA),
        ("nested/deep/three.PNG", RGB),
        ("other/two.png", RGBA),
    ] {
        f.add(name, bytes);
    }
    let flags = [
        "--search",
        "exhaustive",
        "--scale-width",
        "9",
        "--scale-height",
        "5",
        "-t",
        "100",
    ];
    let mut batch_flags = flags.to_vec();
    batch_flags.extend(["--recursive", "--jobs", "3", "--quiet"]);
    let output = f.run(&batch_flags);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    for name in [
        "one.png",
        "nested/two.png",
        "nested/deep/three.PNG",
        "other/two.png",
    ] {
        verify(
            &f.input.join(name),
            &f.output.join(Path::new(name).with_extension("webp")),
            Resize::Bounds {
                width: Some(9),
                height: Some(5),
            },
            100.0,
            &flags,
        );
    }
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 3);
    assert_eq!(
        fs::read_dir(f.output.join("nested/deep")).unwrap().count(),
        1
    );
}

#[test]
fn individual_failures_continue_and_retain_successes_and_old_destinations() {
    for jobs in ["1", "3"] {
        let f = Fixture::new();
        f.add("a-corrupt.png", b"not an image");
        f.add("b-valid.png", RGB);
        f.add("c-animated.webp", include_bytes!("fixtures/animated.webp"));
        f.add("d-valid.png", RGBA);
        for name in ["a-corrupt.webp", "c-animated.webp"] {
            fs::write(f.output.join(name), OLD).unwrap();
        }
        let output = f.run(&["--jobs", jobs, "--quiet"]);
        failure(&output, "batch failed for 2 image(s)");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("a-corrupt.png") && stderr.contains("c-animated.webp"));
        assert!(stderr.find("a-corrupt.png").unwrap() < stderr.find("c-animated.webp").unwrap());
        assert!(!stderr.contains("Wrote WebP") && !stderr.contains("Batch complete"));
        assert!(stderr.contains("successful outputs were retained"));
        for name in ["a-corrupt.webp", "c-animated.webp"] {
            assert_eq!(fs::read(f.output.join(name)).unwrap(), OLD);
        }
        for name in ["b-valid", "d-valid"] {
            verify(
                &f.input.join(format!("{name}.png")),
                &f.output.join(format!("{name}.webp")),
                Resize::Scale(1.0),
                80.0,
                &[],
            );
        }
        assert_eq!(fs::read_dir(&f.output).unwrap().count(), 4);
        assert_eq!(
            fs::read(f.input.join("a-corrupt.png")).unwrap(),
            b"not an image"
        );
        assert_eq!(fs::read(f.input.join("b-valid.png")).unwrap(), RGB);
        assert_eq!(fs::read(f.input.join("d-valid.png")).unwrap(), RGBA);
    }
}

#[test]
fn per_image_skip_and_byte_caps_have_correct_counts_and_preserve_files() {
    let f = Fixture::new();
    f.add("write.png", RGB);
    let reference = input::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rgb.jpg"),
        1.0,
    )
    .unwrap();
    let compressed = TrialEvaluator::new(reference.pixels())
        .unwrap()
        .evaluate(Mode::Lossy { quality: 0 })
        .unwrap()
        .into_bytes();
    f.add("skip.webp", &compressed);
    fs::write(f.output.join("skip.webp"), OLD).unwrap();
    let output = f.run(&["--jobs", "2", "--skip-if-larger", "-t", "100"]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Batch complete: 1 written, 1 skipped, 0 failed"),
        "{stderr}"
    );
    assert!(stderr.contains("Skipped WebP") && stderr.contains("Wrote WebP"));
    assert_eq!(fs::read(f.output.join("skip.webp")).unwrap(), OLD);
    let written = fs::read(f.output.join("write.webp")).unwrap();
    let output = f.run(&["--jobs", "2", "--skip-if-larger", "--max-bytes", "1"]);
    failure(&output, "--max-bytes 1");
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("Batch complete: 0 written, 0 skipped, 2 failed"));
    assert_eq!(fs::read(f.output.join("skip.webp")).unwrap(), OLD);
    assert_eq!(fs::read(f.output.join("write.webp")).unwrap(), written);
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 2);
    assert_eq!(fs::read(f.input.join("skip.webp")).unwrap(), compressed);
}

#[test]
fn name_and_file_directory_collisions_fail_before_any_output() {
    for names in [
        ["same.png", "same.jpg"],
        ["Same.png", "same.JPG"],
        ["a.png", "a.webp/nested.png"],
        ["z.png", "z.webp/nested.png"],
    ] {
        let f = Fixture::new();
        f.add("0-safe.png", RGB);
        for name in names {
            f.add(name, RGB);
        }
        fs::write(f.output.join("0-safe.webp"), OLD).unwrap();
        let output = f.run(&["--recursive", "--jobs", "2"]);
        failure(&output, "collision");
        assert_eq!(fs::read(f.output.join("0-safe.webp")).unwrap(), OLD);
        assert_eq!(fs::read_dir(&f.output).unwrap().count(), 1);
    }
}

#[test]
fn directory_roots_stdout_empty_selection_and_existing_path_types_are_validated() {
    let f = Fixture::new();
    f.add("a.png", RGB);
    for destination in [&f.input, &f.input.join("child"), f.input.parent().unwrap()] {
        fs::create_dir_all(destination).unwrap();
        failure(&run(&f.input, destination, &[]), "must not overlap");
    }
    failure(
        &run(&f.input, Path::new("-"), &[]),
        "stdout is not supported",
    );
    failure(
        &run(&f.input, &f.output.join("absent"), &[]),
        "existing directory",
    );
    failure(
        &run(&f.input.join("a.png"), &f.output, &[]),
        "input must be a directory",
    );
    fs::write(f.output.join("not-a-dir"), OLD).unwrap();
    failure(
        &run(&f.input, &f.output.join("not-a-dir"), &[]),
        "output must be a directory",
    );
    fs::create_dir(f.output.join("a.webp")).unwrap();
    failure(&f.run(&[]), "wrong file type");
    let empty = Fixture::new();
    empty.add("nested/only.png", RGB);
    failure(&empty.run(&[]), "no supported images");
    assert_eq!(fs::read_dir(&empty.output).unwrap().count(), 0);
    assert_eq!(fs::read(f.input.join("a.png")).unwrap(), RGB);
}

#[test]
fn cross_input_hard_link_is_rejected_during_preflight() {
    let f = Fixture::new();
    f.add("a.png", RGB);
    f.add("b.png", RGBA);
    fs::hard_link(f.input.join("b.png"), f.output.join("a.webp")).unwrap();
    failure(&f.run(&["--jobs", "2"]), "refers to an input file");
    assert_eq!(fs::read(f.input.join("a.png")).unwrap(), RGB);
    assert_eq!(fs::read(f.input.join("b.png")).unwrap(), RGBA);
    assert_eq!(fs::read(f.output.join("a.webp")).unwrap(), RGBA);
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 1);
}

#[test]
fn worker_pool_bounds_in_flight_images_and_attempts_each_exactly_once() {
    for jobs in [1, 2, 4] {
        let f = Fixture::new();
        for i in 0..12 {
            f.add(&format!("{i:02}.png"), RGB);
        }
        let args = if jobs == 1 {
            f.args(&[])
        } else {
            f.args(&["--jobs", &jobs.to_string()])
        };
        let plan = BatchPlan::new(&args).unwrap();
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let barrier = Barrier::new(jobs);
        let seen = Mutex::new(Vec::new());
        let summary = plan
            .execute(|args| {
                let concurrent = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(concurrent, Ordering::SeqCst);
                let mut seen = seen.lock().unwrap();
                let first_wave = seen.len() < jobs;
                seen.push(args.input.file_name().unwrap().to_owned());
                drop(seen);
                if first_wave {
                    barrier.wait();
                }
                active.fetch_sub(1, Ordering::SeqCst);
                match args
                    .input
                    .file_stem()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .parse::<usize>()
                    .unwrap()
                    % 3
                {
                    0 => Ok(WriteOutcome::Written),
                    1 => Ok(WriteOutcome::Skipped { input_bytes: 123 }),
                    _ => anyhow::bail!("injected failure"),
                }
            })
            .unwrap();
        assert_eq!(peak.load(Ordering::SeqCst), jobs);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        let mut seen = seen.into_inner().unwrap();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 12);
        assert_eq!(
            (summary.written, summary.skipped, summary.failures.len()),
            (4, 4, 4)
        );
        let failures: Vec<_> = summary
            .failures
            .iter()
            .map(|f| f.input.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(failures, ["02.png", "05.png", "08.png", "11.png"]);
    }
}

#[cfg(unix)]
#[test]
fn symlinks_are_not_traversed_and_output_symlinks_are_refused() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.add("a.png", RGB);
    f.add("nested/b.png", RGBA);
    symlink(&f.input, f.input.join("loop")).unwrap();
    symlink(f.input.join("a.png"), f.input.join("linked.png")).unwrap();
    let output = f.run(&["--recursive", "--quiet"]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 2);
    fs::remove_file(f.output.join("a.webp")).unwrap();
    symlink(f.input.join("a.png"), f.output.join("a.webp")).unwrap();
    failure(&f.run(&["--recursive"]), "must not be a symlink");
    assert_eq!(fs::read(f.input.join("a.png")).unwrap(), RGB);
    fs::remove_file(f.output.join("a.webp")).unwrap();
    fs::remove_dir_all(f.output.join("nested")).unwrap();
    symlink(f.input.join("nested"), f.output.join("nested")).unwrap();
    failure(&f.run(&["--recursive"]), "must not be a symlink");
    assert!(!f.output.join("a.webp").exists());
    assert_eq!(fs::read(f.input.join("nested/b.png")).unwrap(), RGBA);
}
