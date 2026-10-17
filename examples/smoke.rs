//! Validate a built CLI, including its actual file/stdout bytes, on each CI OS.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{ensure, Context, Result};
use fast_ssim2::{compute_ssimulacra2, PixelDescriptor, PixelSlice};
use image::ImageFormat;
use tempfile::tempdir;
use webp_ssim_optimizer::input;

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
            == concat!("webp-ssim-optimizer ", env!("CARGO_PKG_VERSION")),
        "unexpected CLI version"
    );

    let directory = tempdir()?;
    for name in ["rgb.png", "rgba.png"] {
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
            decoded.width() == 9 && decoded.height() == 6,
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
        let source_view = PixelSlice::new(pixels.as_bytes(), 9, 6, 9 * channels, descriptor)?;
        let output_view = PixelSlice::new(&candidate, 9, 6, 9 * channels, descriptor)?;
        let score = compute_ssimulacra2(&source_view, &output_view)?;
        ensure!(
            score.is_finite() && score >= 90.0,
            "written score {score} is below 90"
        );
        println!("{name}: file/stdout match; 9x6; independently measured score {score:.6}");
    }
    ensure!(
        fs::read_dir(directory.path())?.count() == 3,
        "unexpected staging files"
    );
    Ok(())
}
