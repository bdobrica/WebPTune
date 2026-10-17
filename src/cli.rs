//! Command-line parsing and read-only path validation.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use clap::{builder::TypedValueParser, Parser};

use crate::limits::Resize;
use crate::search::SearchMode;

/// A literal `-` selects binary stdout; every other value is a file path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputTarget {
    Stdout,
    File(PathBuf),
}

fn parse_output(value: &OsStr) -> std::result::Result<OutputTarget, String> {
    if value.is_empty() {
        return Err("output must be a file path or '-' for stdout".into());
    }
    Ok(if value == "-" {
        OutputTarget::Stdout
    } else {
        OutputTarget::File(value.into())
    })
}

fn parse_threshold(value: &str) -> std::result::Result<f64, String> {
    let threshold = value
        .parse::<f64>()
        .map_err(|_| "threshold must be a finite number in [0, 100]".to_owned())?;
    if !threshold.is_finite() || !(0.0..=100.0).contains(&threshold) {
        return Err("threshold must be a finite number in [0, 100]".into());
    }
    Ok(threshold)
}

fn parse_scale(value: &str) -> std::result::Result<f64, String> {
    let scale = value
        .parse::<f64>()
        .map_err(|_| "scale must be a finite number in (0, 1]".to_owned())?;
    if !(scale.is_finite() && 0.0 < scale && scale <= 1.0) {
        return Err("scale must be a finite number in (0, 1]".into());
    }
    Ok(scale)
}

fn parse_dimension(value: &str) -> std::result::Result<u32, String> {
    value
        .parse::<u32>()
        .ok()
        .filter(|&dimension| dimension > 0)
        .ok_or_else(|| format!("dimension must be an integer in [1, {}]", u32::MAX))
}

/// Convert a still image to WebP with a minimum SSIMULACRA2 score.
#[derive(Clone, Debug, Parser)]
#[command(
    version,
    after_help = "Examples:\n  webptune -i photo.png -o photo.webp -t 90 -s 0.5\n  webptune -i photo.png -o photo.webp --scale-width 1920 --scale-height 1080\n  webptune -i photo.png -o photo.webp --search exhaustive -t 90\n  webptune -i photo.png -o - > photo.webp\n\nDimension bounds preserve aspect ratio (rounded to whole pixels) without cropping, stretching or upscaling. Use either bound or both, as an alternative to --scale.\n\nExhaustive search evaluates all integer lossy qualities 0 through 100 plus lossless. It guarantees the smallest feasible result only within this fixed set at the current encoder settings, and takes more work than the default heuristic.\n\nExisting destination files are replaced only after the complete WebP is ready. The input file cannot be the destination. Reports go to stderr; '-o -' writes only WebP bytes to stdout."
)]
pub struct Args {
    /// Source still image (JPEG, PNG, WebP or BMP); stdin is not supported
    #[arg(short, long, value_name = "PATH")]
    pub input: PathBuf,

    /// Destination WebP path, or '-' for binary stdout
    #[arg(short, long, value_name = "PATH|-", value_parser = clap::builder::OsStringValueParser::new().try_map(|value: OsString| parse_output(&value)))]
    pub output: OutputTarget,

    /// Minimum native SSIMULACRA2 score in [0, 100] (not a percentage)
    #[arg(short, long, default_value = "80", value_name = "SCORE", value_parser = parse_threshold, allow_hyphen_values = true)]
    pub threshold: f64,

    /// Width/height multiplier in (0, 1]; score is measured after resizing
    #[arg(short, long, default_value = "1.0", value_name = "FACTOR", value_parser = parse_scale, allow_hyphen_values = true, conflicts_with_all = ["scale_width", "scale_height"])]
    pub scale: f64,

    /// Maximum output width in pixels; preserve aspect ratio, never upscale
    #[arg(long, value_name = "PIXELS", value_parser = parse_dimension, allow_hyphen_values = true)]
    pub scale_width: Option<u32>,

    /// Maximum output height in pixels; preserve aspect ratio, never upscale
    #[arg(long, value_name = "PIXELS", value_parser = parse_dimension, allow_hyphen_values = true)]
    pub scale_height: Option<u32>,

    /// Search strategy; exhaustive finds the smallest feasible of 102 fixed candidates
    #[arg(long, value_enum, default_value = "heuristic", value_name = "MODE")]
    pub search: SearchMode,

    /// Print detailed progress and trial diagnostics to stderr
    #[arg(short, long)]
    pub verbose: bool,
}

impl Args {
    /// Resolve the dimension bounds or the default/explicit scale multiplier.
    pub fn resize(&self) -> Resize {
        if self.scale_width.is_some() || self.scale_height.is_some() {
            Resize::Bounds {
                width: self.scale_width,
                height: self.scale_height,
            }
        } else {
            Resize::Scale(self.scale)
        }
    }

    /// Check input readability and reject source/destination aliases without writing.
    ///
    /// Canonical paths catch relative and symlink aliases; file handles also catch
    /// hard links on supported platforms. Recheck before writing in the output
    /// stage: these checks cannot prevent concurrent filesystem changes.
    pub fn validate_paths(&self) -> Result<()> {
        ensure!(self.input.as_os_str() != OsStr::new("-"), "stdin input is not supported; pass --input a file path (use './-' for a file named '-')");
        let input_metadata = fs::metadata(&self.input)
            .with_context(|| format!("cannot inspect input '{}'", self.input.display()))?;
        ensure!(
            input_metadata.is_file(),
            "input '{}' must be a regular file",
            self.input.display()
        );
        let input = File::open(&self.input)
            .with_context(|| format!("cannot read input '{}'", self.input.display()))?;
        let input_path = self
            .input
            .canonicalize()
            .with_context(|| format!("cannot resolve input '{}'", self.input.display()))?;

        let OutputTarget::File(output) = &self.output else {
            return Ok(());
        };
        let output_metadata = match fs::symlink_metadata(output) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Resolve the existing parent without creating the destination.
                let parent = output
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                let parent = parent.canonicalize().with_context(|| {
                    format!(
                        "cannot resolve output directory '{}'; choose an existing directory",
                        parent.display()
                    )
                })?;
                ensure!(
                    parent.is_dir(),
                    "output parent '{}' must be a directory",
                    parent.display()
                );
                return Ok(());
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("cannot inspect output '{}'", output.display()))
            }
        };

        // Resolve symlinks explicitly, including reporting dangling symlinks.
        let output_path = output
            .canonicalize()
            .with_context(|| format!("cannot resolve output '{}'", output.display()))?;
        if input_path == output_path {
            bail!("output '{}' refers to the input file; choose a different destination or '-' for stdout", output.display());
        }
        ensure!(
            output_metadata.is_file() || output_metadata.file_type().is_symlink(),
            "output '{}' must be a regular file or '-' for stdout",
            output.display()
        );
        ensure!(
            fs::metadata(&output_path)
                .with_context(|| format!("cannot inspect resolved output '{}'", output.display()))?
                .is_file(),
            "output '{}' must resolve to a regular file",
            output.display()
        );

        let input_handle = same_file::Handle::from_file(input)
            .context("cannot identify input file for collision validation")?;
        let output_handle = same_file::Handle::from_path(output).with_context(|| {
            format!(
                "cannot identify output '{}' for collision validation",
                output.display()
            )
        })?;
        ensure!(input_handle != output_handle, "output '{}' refers to the input file (hard link); choose a different destination or '-' for stdout", output.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(extra: &[&str]) -> std::result::Result<Args, clap::Error> {
        Args::try_parse_from(
            ["optimizer", "-i", "in.png", "-o", "out.webp"]
                .into_iter()
                .chain(extra.iter().copied()),
        )
    }

    #[test]
    fn defaults_and_short_flags() {
        let args = parse(&[]).unwrap();
        assert_eq!(args.input, PathBuf::from("in.png"));
        assert_eq!(args.output, OutputTarget::File("out.webp".into()));
        assert_eq!(args.threshold, 80.0);
        assert_eq!(args.scale, 1.0);
        assert_eq!(args.resize(), Resize::Scale(1.0));
        assert_eq!(args.scale_width, None);
        assert_eq!(args.scale_height, None);
        assert_eq!(args.search, SearchMode::Heuristic);
        assert!(!args.verbose);
        let args = parse(&["-t", "90", "-s", "0.5", "-v"]).unwrap();
        assert_eq!(args.threshold, 90.0);
        assert_eq!(args.scale, 0.5);
        assert!(args.verbose);
    }

    #[test]
    fn search_accepts_only_supported_strategies() {
        for (name, mode) in [
            ("heuristic", SearchMode::Heuristic),
            ("exhaustive", SearchMode::Exhaustive),
        ] {
            assert_eq!(parse(&["--search", name]).unwrap().search, mode);
            assert_eq!(parse(&[&format!("--search={name}")]).unwrap().search, mode);
        }
        for value in ["fast", "thorough", "EXHAUSTIVE", "101", ""] {
            let error = parse(&["--search", value]).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::InvalidValue);
            assert!(error.to_string().contains("heuristic, exhaustive"));
        }
        assert_eq!(
            parse(&["--search"]).unwrap_err().kind(),
            clap::error::ErrorKind::InvalidValue
        );
    }

    #[test]
    fn long_flags_and_stdout() {
        let args = Args::try_parse_from([
            "optimizer",
            "--input",
            "in.png",
            "--output",
            "-",
            "--threshold",
            "0",
            "--scale",
            "1",
            "--verbose",
        ])
        .unwrap();
        assert_eq!(args.output, OutputTarget::Stdout);
        assert_eq!(args.threshold, 0.0);
        assert_eq!(parse(&["--threshold=100"]).unwrap().threshold, 100.0);
        assert!(parse(&["--scale=1e-300"]).is_ok());
        assert_eq!(
            parse_output(OsStr::new("./-")).unwrap(),
            OutputTarget::File("./-".into())
        );
        assert!(parse_output(OsStr::new("")).is_err());
    }

    #[test]
    fn dimension_bounds_are_alternatives_to_explicit_scale() {
        for (flags, width, height) in [
            (vec!["--scale-width", "1920"], Some(1920), None),
            (vec!["--scale-height=1080"], None, Some(1080)),
            (
                vec!["--scale-width", "1920", "--scale-height", "1080"],
                Some(1920),
                Some(1080),
            ),
            (vec!["--scale-width", "4294967295"], Some(u32::MAX), None),
        ] {
            let args = parse(&flags).unwrap();
            assert_eq!(args.resize(), Resize::Bounds { width, height });
            for scale in ["-s", "--scale"] {
                for value in ["0.5", "1.0"] {
                    let mut conflicting = flags.clone();
                    conflicting.extend([scale, value]);
                    assert_eq!(
                        parse(&conflicting).unwrap_err().kind(),
                        clap::error::ErrorKind::ArgumentConflict
                    );
                    conflicting.rotate_right(2);
                    assert_eq!(
                        parse(&conflicting).unwrap_err().kind(),
                        clap::error::ErrorKind::ArgumentConflict
                    );
                }
            }
        }
        for flag in ["--scale-width", "--scale-height"] {
            for value in [
                "0",
                "-1",
                "0.5",
                "1e3",
                "NaN",
                "inf",
                "abc",
                "",
                "4294967296",
            ] {
                let error = parse(&[flag, value]).unwrap_err().to_string();
                assert!(
                    error.contains("dimension must be an integer in [1, 4294967295]"),
                    "{error}"
                );
            }
        }
    }

    #[test]
    fn invalid_numeric_values_are_actionable() {
        for value in [
            "NaN", "inf", "+inf", "-inf", "1e999", "-0.1", "100.1", "abc", "",
        ] {
            let error = parse(&["--threshold", value]).unwrap_err().to_string();
            assert!(
                error.contains("threshold must be a finite number in [0, 100]"),
                "{error}"
            );
        }
        for value in [
            "NaN", "inf", "+inf", "-inf", "1e999", "0", "-0", "-1", "1.01", "abc", "",
        ] {
            let error = parse(&["--scale", value]).unwrap_err().to_string();
            assert!(
                error.contains("scale must be a finite number in (0, 1]"),
                "{error}"
            );
        }
    }

    #[test]
    fn required_paths_and_unknown_flags() {
        for values in [
            vec!["optimizer"],
            vec!["optimizer", "-i", "in.png"],
            vec!["optimizer", "-o", "out.webp"],
        ] {
            assert_eq!(
                Args::try_parse_from(values).unwrap_err().kind(),
                clap::error::ErrorKind::MissingRequiredArgument
            );
        }
        assert_eq!(
            parse(&["--unknown"]).unwrap_err().kind(),
            clap::error::ErrorKind::UnknownArgument
        );
    }

    #[cfg(unix)]
    #[test]
    fn paths_preserve_non_utf8_names() {
        use std::os::unix::ffi::OsStringExt;
        let path = OsString::from_vec(b"image-\xff.webp".to_vec());
        let args = Args::try_parse_from([
            OsString::from("optimizer"),
            "-i".into(),
            path.clone(),
            "-o".into(),
            path.clone(),
        ])
        .unwrap();
        assert_eq!(args.input, PathBuf::from(&path));
        assert_eq!(args.output, OutputTarget::File(path.into()));
    }
}
