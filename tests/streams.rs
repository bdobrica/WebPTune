use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use clap::Parser;
use fast_ssim2::{compute_ssimulacra2, PixelDescriptor, PixelSlice};
use tempfile::tempdir;
use webptune::batch::BatchPlan;
use webptune::cancel::{Cancellation, Cancelled};
use webptune::cli::Args;
use webptune::encoder::{Mode, TrialEvaluator};
use webptune::{input, output, search};

fn command(temp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_webptune"));
    cmd.env("TMPDIR", temp).env("TMP", temp).env("TEMP", temp);
    cmd
}

fn stdin_run(bytes: &[u8], temp: &Path, destination: &Path, flags: &[&str]) -> Output {
    let mut child = command(temp)
        .args(["-i", "-", "-o"])
        .arg(destination)
        .args(flags)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    // These fixtures fit in the pipe; errors are expected when validation fails early.
    let _ = stdin.write_all(bytes);
    drop(stdin);
    child.wait_with_output().unwrap()
}

fn successful(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn stdin_matches_files_and_independent_scores_in_both_search_modes() {
    let dir = tempdir().unwrap();
    let scratch = dir.path().join("scratch");
    fs::create_dir(&scratch).unwrap();
    for name in [
        "rgb.png",
        "rgba.png",
        "rgb.jpg",
        "rgb.bmp",
        "rgba.webp",
        "oriented.jpg",
        "display-p3.png",
        "linear16.png",
        "linear.hdr",
    ] {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let original = fs::read(&source).unwrap();
        for mode in ["heuristic", "exhaustive"] {
            let flags = [
                "--search",
                mode,
                "-t",
                "90",
                "--scale",
                "0.5",
                "--method",
                "0",
                "--alpha-quality",
                "37",
                "--exact",
            ];
            let file = command(&scratch)
                .args(["-i"])
                .arg(&source)
                .args(["-o", "-"])
                .args(flags)
                .output()
                .unwrap();
            successful(&file);
            let destination = dir.path().join("output.webp");
            fs::write(&destination, b"old").unwrap();
            for target in [destination.as_path(), Path::new("-")] {
                let result = stdin_run(&original, &scratch, target, &flags);
                successful(&result);
                let bytes = if target == Path::new("-") {
                    result.stdout
                } else {
                    assert!(result.stdout.is_empty());
                    fs::read(target).unwrap()
                };
                assert_eq!(bytes, file.stdout, "{name}, {mode}");
                let reference = input::load(&source, 0.5).unwrap();
                let pixels = reference.pixels();
                let (w, h) = pixels.dimensions();
                let decoded = image::load_from_memory(&bytes).unwrap();
                assert_eq!((decoded.width(), decoded.height()), (w, h));
                let (candidate, channels, descriptor) = if pixels.has_alpha() {
                    (
                        decoded.to_rgba8().into_raw(),
                        4,
                        PixelDescriptor::RGBA8_SRGB,
                    )
                } else {
                    (decoded.to_rgb8().into_raw(), 3, PixelDescriptor::RGB8_SRGB)
                };
                let a = PixelSlice::new(pixels.as_bytes(), w, h, w as usize * channels, descriptor)
                    .unwrap();
                let b =
                    PixelSlice::new(&candidate, w, h, w as usize * channels, descriptor).unwrap();
                assert!(compute_ssimulacra2(&a, &b).unwrap() >= 90.0);
                assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0);
            }
            assert_eq!(fs::read(&source).unwrap(), original);
        }
    }
}

#[test]
fn stdin_size_policies_use_received_bytes_and_preserve_destinations() {
    let dir = tempdir().unwrap();
    let scratch = dir.path().join("scratch");
    fs::create_dir(&scratch).unwrap();
    let tiny = include_bytes!("fixtures/tiny-rgb.png");
    let initial = stdin_run(tiny, &scratch, Path::new("-"), &["-t", "100", "-q"]);
    successful(&initial);
    let size = initial.stdout.len();
    for target in [dir.path().join("output.webp"), "-".into()] {
        fs::write(dir.path().join("output.webp"), b"old destination").unwrap();
        for cap in [size - 1, size, size + 1] {
            let capped = stdin_run(
                tiny,
                &scratch,
                &target,
                &["-t", "100", "--max-bytes", &cap.to_string(), "--quiet"],
            );
            if cap < size {
                assert_eq!(capped.status.code(), Some(1));
                assert!(capped.stdout.is_empty());
                assert!(String::from_utf8_lossy(&capped.stderr).contains("--max-bytes"));
                assert_eq!(
                    fs::read(dir.path().join("output.webp")).unwrap(),
                    b"old destination"
                );
            } else {
                successful(&capped);
                assert!(capped.stderr.is_empty());
                if target != Path::new("-") {
                    assert_eq!(fs::read(&target).unwrap(), initial.stdout);
                } else {
                    assert_eq!(capped.stdout, initial.stdout);
                }
            }
        }
        fs::write(dir.path().join("output.webp"), b"old destination").unwrap();
        // Use a deliberately short input whose lossless WebP is larger.
        let prepared = input::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rgb.png"),
            1.0,
        )
        .unwrap();
        let small = TrialEvaluator::new(prepared.pixels())
            .unwrap()
            .evaluate(Mode::Lossy { quality: 0 })
            .unwrap()
            .into_bytes();
        let baseline = stdin_run(&small, &scratch, Path::new("-"), &["-t", "100"]);
        successful(&baseline);
        assert!(baseline.stdout.len() > small.len());
        let skipped = stdin_run(
            &small,
            &scratch,
            &target,
            &["-t", "100", "--skip-if-larger"],
        );
        successful(&skipped);
        assert!(skipped.stdout.is_empty());
        assert_eq!(
            fs::read(dir.path().join("output.webp")).unwrap(),
            b"old destination"
        );
        let report = String::from_utf8_lossy(&skipped.stderr);
        assert!(
            report.contains(&format!(
                "{} bytes > input {} bytes",
                baseline.stdout.len(),
                small.len()
            )),
            "{report}"
        );
        let capped = stdin_run(
            &small,
            &scratch,
            &target,
            &["-t", "100", "--skip-if-larger", "--max-bytes", "1", "-q"],
        );
        assert_eq!(capped.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&capped.stderr).contains("--max-bytes"));
        assert_eq!(
            fs::read(dir.path().join("output.webp")).unwrap(),
            b"old destination"
        );
        // Test equality by re-optimizing selected bytes: received length equals result.
        let equal = stdin_run(
            &initial.stdout,
            &scratch,
            Path::new("-"),
            &["-t", "100", "--skip-if-larger", "-q"],
        );
        successful(&equal);
        assert_eq!(equal.stdout, initial.stdout);
    }
    assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0);
}

#[test]
fn malformed_stdin_invalid_paths_and_batch_fail_without_writes_or_spools() {
    let dir = tempdir().unwrap();
    let scratch = dir.path().join("scratch");
    fs::create_dir(&scratch).unwrap();
    let dest = dir.path().join("output.webp");
    for bytes in [
        b"".as_slice(),
        b"not an image",
        &include_bytes!("fixtures/rgba.png")[..20],
        include_bytes!("fixtures/animated.webp"),
    ] {
        fs::write(&dest, b"old").unwrap();
        let out = stdin_run(bytes, &scratch, &dest, &["-q"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(!out.stderr.is_empty());
        assert!(out.stdout.is_empty());
        assert_eq!(fs::read(&dest).unwrap(), b"old");
        assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0);
    }
    for (target, flags, diagnostic) in [
        (
            dir.path().join("absent/out.webp"),
            vec![],
            "output directory",
        ),
        (dest.clone(), vec!["--batch"], "stdin input cannot"),
    ] {
        // Keep the pipe open: validation must fail before trying to read stdin.
        let mut child = command(&scratch)
            .args(["-i", "-", "-o"])
            .arg(target)
            .args(flags)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _stdin = child.stdin.take().unwrap();
        wait_bounded(&mut child);
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains(diagnostic));
        assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0);
    }
    let literal = dir.path().join("-");
    fs::write(&literal, include_bytes!("fixtures/rgb.png")).unwrap();
    let out = command(&scratch)
        .current_dir(dir.path())
        .args(["-i", "./-", "-o", "-", "-q"])
        .output()
        .unwrap();
    successful(&out);
    assert!(!out.stdout.is_empty());
}

fn wait_bounded(child: &mut std::process::Child) {
    let start = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > std::time::Duration::from_secs(30) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("child did not stop within 30 seconds");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn library_cancellation_stops_trials_output_and_new_batch_jobs() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("in.png");
    fs::write(&source, include_bytes!("fixtures/rgb.png")).unwrap();
    let dest = dir.path().join("out.webp");
    fs::write(&dest, b"old").unwrap();
    let args = Args::parse_from([
        "webptune",
        "-i",
        source.to_str().unwrap(),
        "-o",
        dest.to_str().unwrap(),
    ]);
    let reference = input::load(&source, 1.0).unwrap();
    let cancel = Cancellation::default();
    let evaluator =
        TrialEvaluator::with_cancellation(reference.pixels(), args.encoder, &cancel).unwrap();
    let trial = evaluator.evaluate(Mode::Lossless).unwrap();
    cancel.clone().cancel();
    assert!(
        input::load_with_cancellation(&source, args.resize(), &cancel)
            .unwrap_err()
            .is::<Cancelled>()
    );
    assert!(TrialEvaluator::with_cancellation(reference.pixels(), args.encoder, &cancel).is_err());
    for mode in [
        search::SearchMode::Heuristic,
        search::SearchMode::Exhaustive,
    ] {
        assert!(search::search_with_mode(&evaluator, 80.0, mode)
            .unwrap_err()
            .chain()
            .any(|e| e.is::<Cancelled>()));
    }
    assert!(
        output::write_with_cancellation(&args, &trial, None, &cancel)
            .unwrap_err()
            .is::<Cancelled>()
    );
    assert_eq!(fs::read(&dest).unwrap(), b"old");
    let images = dir.path().join("images");
    let converted = dir.path().join("converted");
    fs::create_dir(&images).unwrap();
    fs::create_dir(&converted).unwrap();
    for n in 0..8 {
        fs::copy(&source, images.join(format!("{n}.png"))).unwrap();
    }
    let args = Args::parse_from([
        "webptune",
        "--batch",
        "-i",
        images.to_str().unwrap(),
        "-o",
        converted.to_str().unwrap(),
        "--jobs",
        "1",
    ]);
    assert!(BatchPlan::new_with_cancellation(&args, &cancel).is_err());
    let plan = BatchPlan::new(&args).unwrap();
    let cancel = Cancellation::default();
    let count = std::sync::atomic::AtomicUsize::new(0);
    let result = plan.execute_with_cancellation(&cancel, |task| {
        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        output::write(task, &trial)?;
        cancel.cancel();
        Ok(output::WriteOutcome::Written)
    });
    assert!(result.unwrap_err().is::<Cancelled>());
    assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(fs::read_dir(&converted).unwrap().count(), 1);
    assert_eq!(fs::read(converted.join("0.webp")).unwrap(), trial.bytes());
}

#[cfg(unix)]
#[test]
fn native_signals_cancel_blocked_stdin_and_search_with_cleanup() {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    use std::time::Duration;
    let dir = tempdir().unwrap();
    let scratch = dir.path().join("scratch");
    fs::create_dir(&scratch).unwrap();
    let source = dir.path().join("noise.png");
    let mut seed = 123u32;
    let pixels = image::RgbImage::from_fn(512, 512, |_, _| {
        let mut values = [0; 3];
        for v in &mut values {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = (seed >> 24) as u8;
        }
        image::Rgb(values)
    });
    pixels.save(&source).unwrap();
    for signal in ["INT", "TERM", "HUP"] {
        for stdin_input in [true, false] {
            let dest = dir.path().join("out.webp");
            fs::write(&dest, b"old destination").unwrap();
            let mut cmd = command(&scratch);
            cmd.arg("-i")
                .arg(if stdin_input { Path::new("-") } else { &source })
                .arg("-o")
                .arg(&dest)
                .args(["--search", "exhaustive", "--verbose"]);
            let mut child = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut stdin = child.stdin.take().unwrap();
            if stdin_input {
                stdin
                    .write_all(include_bytes!("fixtures/rgba.png"))
                    .unwrap();
            }
            let stderr = child.stderr.take().unwrap();
            let (tx, rx) = mpsc::channel();
            let reader = std::thread::spawn(move || {
                let mut text = String::new();
                for line in BufReader::new(stderr).lines() {
                    let line = line.unwrap();
                    let ready = line.contains(if stdin_input {
                        "Reading stdin"
                    } else {
                        "Searching WebP"
                    });
                    text.push_str(&line);
                    text.push('\n');
                    if ready {
                        let _ = tx.send(());
                    }
                }
                text
            });
            rx.recv_timeout(Duration::from_secs(30)).unwrap();
            if stdin_input {
                // Wait until the spool exists, keeping EOF withheld throughout.
                let start = std::time::Instant::now();
                while fs::read_dir(&scratch).unwrap().count() == 0 {
                    assert!(start.elapsed() < Duration::from_secs(10));
                    std::thread::sleep(Duration::from_millis(5));
                }
                assert!(child.try_wait().unwrap().is_none());
                assert_eq!(fs::read(&dest).unwrap(), b"old destination");
            }
            assert!(Command::new("kill")
                .args(["-s", signal, &child.id().to_string()])
                .status()
                .unwrap()
                .success());
            wait_bounded(&mut child);
            // Keep stdin open until exit to prove cancellation did not need EOF.
            drop(stdin);
            let out = child.wait_with_output().unwrap();
            assert_eq!(out.status.code(), Some(130));
            assert!(out.stdout.is_empty());
            let stderr = reader.join().unwrap();
            assert!(stderr.contains("operation cancelled"), "{stderr}");
            assert_eq!(fs::read(&dest).unwrap(), b"old destination");
            assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0);
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
        }
    }
}

#[test]
fn stdin_waits_for_eof_before_writing() {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    use std::time::Duration;
    let dir = tempdir().unwrap();
    let scratch = dir.path().join("scratch");
    fs::create_dir(&scratch).unwrap();
    let dest = dir.path().join("out.webp");
    fs::write(&dest, b"old destination").unwrap();
    let mut child = command(&scratch)
        .args(["-i", "-", "-o"])
        .arg(&dest)
        .arg("--verbose")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(include_bytes!("fixtures/rgba.png"))
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        for line in BufReader::new(stderr).lines() {
            let line = line.unwrap();
            if line.contains("Reading stdin") {
                let _ = tx.send(());
            }
            text.push_str(&line);
            text.push('\n');
        }
        text
    });
    rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(child.try_wait().unwrap().is_none());
    assert_eq!(fs::read(&dest).unwrap(), b"old destination");
    drop(stdin);
    wait_bounded(&mut child);
    let out = child.wait_with_output().unwrap();
    successful(&out);
    assert!(out.stdout.is_empty());
    assert!(reader.join().unwrap().contains("Wrote WebP"));
    assert!(image::load_from_memory(&fs::read(&dest).unwrap()).is_ok());
    assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0);
}
