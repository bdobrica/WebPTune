//! Validate a built CLI, including its actual file/stdout bytes, on each CI OS.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use anyhow::{ensure, Context, Result};
use fast_ssim2::{compute_ssimulacra2, PixelDescriptor, PixelSlice};
use image::ImageFormat;
use tempfile::tempdir;
use webptune::input;

fn successful(output: &Output) -> Result<()> {
    ensure!(
        output.status.success(),
        "CLI failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn main() -> Result<()> {
    let binary = std::env::args_os()
        .nth(1)
        .context("usage: cargo run --release --locked --example smoke -- PATH_TO_CLI")?;
    let binary = Path::new(&binary).canonicalize().context("locate CLI")?;
    let version = Command::new(&binary).arg("--version").output()?;
    successful(&version)?;
    ensure!(
        String::from_utf8_lossy(&version.stdout).trim()
            == concat!("webptune ", env!("CARGO_PKG_VERSION")),
        "unexpected CLI version"
    );

    let directory = tempdir()?;
    let mut verified = Vec::new();
    for (name, (width, height)) in [
        ("rgb.png", (9, 6)),
        ("rgba.png", (9, 6)),
        ("oriented.jpg", (6, 9)),
        ("display-p3.png", (9, 6)),
        ("linear16.png", (2, 1)),
        ("linear.hdr", (4, 1)),
    ] {
        let source = directory.path().join(name);
        let original = fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )?;
        fs::write(&source, &original)?;
        let destination = directory.path().join("output.webp");
        fs::write(&destination, b"existing destination must be replaced")?;
        let run = |target: &Path| {
            Command::new(&binary)
                .arg("-i")
                .arg(&source)
                .arg("-o")
                .arg(target)
                .args(["--threshold", "90", "--scale", "0.5", "--verbose"])
                .output()
        };
        let file = run(&destination)?;
        successful(&file)?;
        ensure!(file.stdout.is_empty(), "file output leaked text to stdout");
        let stdout = run(Path::new("-"))?;
        successful(&stdout)?;
        let bytes = fs::read(&destination)?;
        ensure!(stdout.stdout == bytes, "file and stdout payloads differ");
        for result in [&file, &stdout] {
            let stderr = String::from_utf8_lossy(&result.stderr);
            ensure!(
                stderr.contains("Wrote WebP:") && stderr.contains("Trial "),
                "missing reports"
            );
        }
        ensure!(fs::read(&source)? == original, "source was modified");
        let reference = input::load(&source, 0.5)?;
        let pixels = reference.pixels();
        let decoded = image::load_from_memory_with_format(&bytes, ImageFormat::WebP)?;
        ensure!(
            decoded.width() == width && decoded.height() == height,
            "incorrect scaled dimensions"
        );
        let (candidate, channels, descriptor) = if pixels.has_alpha() {
            let rgba = decoded.to_rgba8();
            ensure!(
                pixels
                    .as_bytes()
                    .chunks_exact(4)
                    .map(|p| p[3])
                    .eq(rgba.pixels().map(|p| p[3])),
                "alpha changed"
            );
            (rgba.into_raw(), 4, PixelDescriptor::RGBA8_SRGB)
        } else {
            (decoded.to_rgb8().into_raw(), 3, PixelDescriptor::RGB8_SRGB)
        };
        // Bypass the application's cached metric and pixel-view helper.
        let source_view = PixelSlice::new(
            pixels.as_bytes(),
            width,
            height,
            width as usize * channels,
            descriptor,
        )?;
        let output_view = PixelSlice::new(
            &candidate,
            width,
            height,
            width as usize * channels,
            descriptor,
        )?;
        let score = compute_ssimulacra2(&source_view, &output_view)?;
        ensure!(
            score.is_finite() && score >= 90.0,
            "written score {score} is below 90"
        );
        println!(
            "{name}: file/stdout match; {width}x{height}; independently measured score {score:.6}"
        );
        verified.push((name, original, bytes));
    }
    ensure!(
        fs::read_dir(directory.path())?.count() == verified.len() + 1,
        "unexpected staging files"
    );

    let batch_input = directory.path().join("images");
    let batch_output = directory.path().join("converted");
    fs::create_dir_all(batch_input.join("nested"))?;
    fs::create_dir_all(batch_output.join("nested"))?;
    for (name, original, _) in &verified {
        let relative = if *name == "rgba.png" {
            Path::new("nested").join(name)
        } else {
            name.into()
        };
        fs::write(batch_input.join(&relative), original)?;
        fs::write(
            batch_output.join(relative.with_extension("webp")),
            b"replace this",
        )?;
    }
    let batch = Command::new(&binary)
        .arg("--batch")
        .arg("-i")
        .arg(&batch_input)
        .arg("-o")
        .arg(&batch_output)
        .args([
            "--recursive",
            "--jobs",
            "2",
            "--threshold",
            "90",
            "--scale",
            "0.5",
            "--quiet",
        ])
        .output()?;
    successful(&batch)?;
    ensure!(
        batch.stdout.is_empty() && batch.stderr.is_empty(),
        "quiet batch leaked output"
    );
    for (name, original, bytes) in &verified {
        let relative = if *name == "rgba.png" {
            Path::new("nested").join(name)
        } else {
            name.into()
        };
        ensure!(
            fs::read(batch_input.join(&relative))? == *original,
            "batch modified source"
        );
        ensure!(
            fs::read(batch_output.join(relative.with_extension("webp")))? == *bytes,
            "batch output differs from independently verified single-image output"
        );
    }
    ensure!(
        fs::read_dir(&batch_output)?.count() == verified.len()
            && fs::read_dir(batch_output.join("nested"))?.count() == 1,
        "unexpected batch staging files"
    );
    println!("batch: recursive two-worker RGB/RGBA, EXIF, ICC, 16-bit and HDR output matches independently verified single-image bytes");

    // Nondefault settings must reach the built executable too. Reduced alpha
    // quality must not prevent an exact, threshold-100 lossless fallback.
    let source = directory.path().join("rgba.png");
    let destination = directory.path().join("tuned.webp");
    let tuned = |target: &Path| {
        Command::new(&binary)
            .arg("-i")
            .arg(&source)
            .arg("-o")
            .arg(target)
            .args([
                "--threshold",
                "100",
                "--method",
                "6",
                "--lossless-effort",
                "100",
                "--alpha-quality",
                "0",
                "--alpha-filtering",
                "best",
                "--sharp-yuv",
                "--exact",
                "--verbose",
            ])
            .output()
    };
    let file = tuned(&destination)?;
    successful(&file)?;
    let stdout = tuned(Path::new("-"))?;
    successful(&stdout)?;
    let bytes = fs::read(&destination)?;
    ensure!(
        file.stdout.is_empty() && stdout.stdout == bytes,
        "tuned output differs"
    );
    let reference = input::load(&source, 1.0)?;
    let decoded = image::load_from_memory_with_format(&bytes, ImageFormat::WebP)?.to_rgba8();
    ensure!(
        decoded.as_raw() == reference.pixels().as_bytes(),
        "exact lossless fallback changed pixels"
    );
    ensure!(
        String::from_utf8_lossy(&file.stderr).contains("Encoder settings:")
            && String::from_utf8_lossy(&file.stderr).contains("selected Lossless"),
        "tuned settings or lossless fallback missing from report"
    );
    ensure!(
        fs::read(&source)? == verified[1].1,
        "tuned conversion modified source"
    );
    ensure!(
        fs::read_dir(directory.path())?.count() == verified.len() + 4,
        "tuned conversion left staging files"
    );
    println!("encoder tuning: file/stdout match with full RGBA equality at threshold 100 despite alpha quality 0");
    let stdin_scratch = tempdir()?;
    let stdin_destination = directory.path().join("stdin.webp");
    for (name, original, expected) in &verified {
        for target in [stdin_destination.as_path(), Path::new("-")] {
            let mut child = Command::new(&binary)
                .args(["-i", "-", "-o"])
                .arg(target)
                .args(["--threshold", "90", "--scale", "0.5", "--quiet"])
                .env("TMPDIR", stdin_scratch.path())
                .env("TMP", stdin_scratch.path())
                .env("TEMP", stdin_scratch.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            child
                .stdin
                .take()
                .context("missing stdin pipe")?
                .write_all(original)?;
            let result = child.wait_with_output()?;
            successful(&result)?;
            ensure!(result.stderr.is_empty(), "quiet stdin leaked diagnostics");
            if target == Path::new("-") {
                ensure!(
                    result.stdout == *expected,
                    "stdin/stdout mismatch for {name}"
                );
            } else {
                ensure!(
                    result.stdout.is_empty() && fs::read(target)? == *expected,
                    "stdin/file mismatch for {name}"
                );
            }
            ensure!(
                fs::read_dir(stdin_scratch.path())?.count() == 0,
                "stdin spool leaked"
            );
        }
    }
    ensure!(
        fs::read_dir(directory.path())?.count() == verified.len() + 5,
        "stdin output left staging files"
    );
    println!("stdin: all six preparation cases match independently verified file/stdout output; spools cleaned");
    Ok(())
}
