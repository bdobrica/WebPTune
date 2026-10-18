//! Command-line parsing and read-only path validation.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use clap::{builder::TypedValueParser, Parser};

use crate::encoder::EncoderOptions;
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

fn parse_max_bytes(value: &str) -> std::result::Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|&bytes| bytes > 0)
        .ok_or_else(|| format!("byte limit must be an integer in [1, {}]", u64::MAX))
}

fn parse_jobs(value: &str) -> std::result::Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|jobs| (1..=64).contains(jobs))
        .ok_or_else(|| "jobs must be an integer in [1, 64]".into())
}

/// Convert a still image to WebP with a minimum SSIMULACRA2 score.
#[derive(Clone, Debug, Parser)]
#[command(
    version,
    after_help = "Examples:\n  webptune -i photo.png -o photo.webp -t 90 -s 0.5\n  webptune -i photo.png -o photo.webp --scale-width 1920 --scale-height 1080\n  webptune -i photo.png -o photo.webp --search exhaustive -t 90\n  webptune -i photo.png -o photo.webp --max-bytes 100000 --skip-if-larger --quiet\n  webptune --batch -i photos -o optimized --recursive --jobs 2\n  webptune -i photo.png -o - > photo.webp\n\nInput JPEG/PNG/WebP EXIF orientation is applied before bounds and resizing. Supported RGB/grayscale ICC and PNG color descriptions are converted to sRGB. Sixteen-bit sources keep float precision until final 8-bit quantization. Linear-sRGB Radiance HDR (.hdr) uses a fixed Reinhard tone map to SDR. The score compares this processed SDR reference; it does not measure source HDR/gamut/precision loss. PQ/HLG descriptions are rejected.\n\nDimension bounds preserve aspect ratio (rounded to whole pixels) without cropping, stretching or upscaling. Use either bound or both, as an alternative to --scale.\n\nExhaustive search evaluates all integer lossy qualities 0 through 100 plus lossless. It guarantees the smallest feasible result only within this fixed set at the current encoder settings, and takes more work than the default heuristic.\n\nEncoder tuning stays fixed throughout search. --method (0..=6) controls compression effort in both modes; --lossless-effort (0..=100) controls the lossless candidate. --alpha-quality (0..=100) and --alpha-filtering (none/fast/best) affect lossy alpha only; 100 keeps alpha exact. Reduced alpha quality is still measured by the decoded score. --sharp-yuv affects lossy color conversion; --exact preserves hidden RGB in the lossless candidate. Lossless keeps full alpha and remains a measured fallback. These options also apply to each batch image; exhaustive search does not sweep encoder settings.\n\n--max-bytes accepts a positive integer byte count and never relaxes the quality threshold. If no tested feasible result fits, it fails before writing. --skip-if-larger skips only results strictly larger than the source file, exits successfully and leaves the destination untouched (stdout is empty). Equal sizes are written. A failing byte cap takes priority over skipping. Quiet and verbose conflict; quiet keeps errors visible.\n\nBatch mode requires separate, existing input/output directories and supports .jpg/.jpeg/.png/.webp/.bmp/.hdr files (case insensitive). --recursive includes subdirectories and preserves relative paths, replacing extensions with .webp. Symlink entries and other extensions are skipped. Conflicting output names and overlapping directory trees fail before writing. --jobs (1..=64, default 1) bounds concurrent image pipelines; memory use grows with jobs. Each image uses the same quality/resize/size options. Processing continues after image errors; any failure exits 1 and retains successful outputs. Batch stdout output is unsupported.\n\nExisting destination files are replaced only after the complete WebP is ready. The input file cannot be the destination. Reports go to stderr; '-o -' writes only WebP bytes to stdout. Shell redirection may truncate its target even when output is skipped."
)]
pub struct Args {
    /// Source still image, or source directory with --batch; stdin is not supported
    #[arg(short, long, value_name = "PATH")]
    pub input: PathBuf,

    /// Destination WebP path, '-' for stdout, or existing directory with --batch
    #[arg(short, long, value_name = "PATH|-", value_parser = clap::builder::OsStringValueParser::new().try_map(|value: OsString| parse_output(&value)))]
    pub output: OutputTarget,

    /// Convert supported files in a directory to a separate existing directory
    #[arg(long)]
    pub batch: bool,

    /// Include subdirectories in batch mode; preserve the relative directory tree
    #[arg(long, requires = "batch")]
    pub recursive: bool,

    /// Maximum concurrent batch conversions (1..=64); defaults to 1
    #[arg(long, value_name = "COUNT", value_parser = parse_jobs, requires = "batch", allow_hyphen_values = true)]
    pub jobs: Option<usize>,

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

    #[command(flatten)]
    pub encoder: EncoderOptions,

    /// Succeed without writing if the selected WebP is strictly larger than the input file
    #[arg(long)]
    pub skip_if_larger: bool,

    /// Maximum encoded WebP bytes; fail without writing if no tested feasible result fits
    #[arg(long, value_name = "BYTES", value_parser = parse_max_bytes, allow_hyphen_values = true)]
    pub max_bytes: Option<u64>,

    /// Suppress success/skip reports; errors remain on stderr
    #[arg(short, long, conflicts_with = "verbose")]
    pub quiet: bool,

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
        assert!(!args.batch);
        assert!(!args.recursive);
        assert_eq!(args.jobs, None);
        assert_eq!(args.threshold, 80.0);
        assert_eq!(args.scale, 1.0);
        assert_eq!(args.resize(), Resize::Scale(1.0));
        assert_eq!(args.scale_width, None);
        assert_eq!(args.scale_height, None);
        assert_eq!(args.search, SearchMode::Heuristic);
        assert!(!args.skip_if_larger);
        assert_eq!(args.max_bytes, None);
        assert!(!args.quiet);
        assert!(!args.verbose);
        let args = parse(&["-t", "90", "-s", "0.5", "-v"]).unwrap();
        assert_eq!(args.threshold, 90.0);
        assert_eq!(args.scale, 0.5);
        assert!(args.verbose);
    }

    #[test]
    fn batch_options_require_explicit_mode_and_bounded_jobs() {
        let args = parse(&["--batch", "--recursive", "--jobs", "64"]).unwrap();
        assert!(args.batch && args.recursive);
        assert_eq!(args.jobs, Some(64));
        assert_eq!(parse(&["--batch"]).unwrap().jobs, None);
        assert_eq!(parse(&["--batch", "--jobs=1"]).unwrap().jobs, Some(1));
        for flags in [vec!["--recursive"], vec!["--jobs", "2"]] {
            assert_eq!(
                parse(&flags).unwrap_err().kind(),
                clap::error::ErrorKind::MissingRequiredArgument
            );
        }
        for value in ["0", "65", "-1", "1.5", "inf", "9999999999999999999999", ""] {
            let error = parse(&["--batch", "--jobs", value]).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
            assert!(error
                .to_string()
                .contains("jobs must be an integer in [1, 64]"));
        }
        assert!(parse(&["--batch", "--jobs"]).is_err());
    }

    #[test]
    fn size_policy_and_quiet_options() {
        let args = parse(&["--skip-if-larger", "--max-bytes", "123", "-q"]).unwrap();
        assert!(args.skip_if_larger);
        assert_eq!(args.max_bytes, Some(123));
        assert!(args.quiet);
        assert!(parse(&["--quiet"]).unwrap().quiet);
        assert_eq!(parse(&["--max-bytes=1"]).unwrap().max_bytes, Some(1));
        assert_eq!(
            parse(&["--max-bytes", &u64::MAX.to_string()])
                .unwrap()
                .max_bytes,
            Some(u64::MAX)
        );
        for flags in [["-q", "-v"], ["--verbose", "--quiet"]] {
            assert_eq!(
                parse(&flags).unwrap_err().kind(),
                clap::error::ErrorKind::ArgumentConflict
            );
        }
    }

    #[test]
    fn byte_limit_rejects_zero_nonintegers_and_overflow() {
        for value in [
            "0",
            "-1",
            "1.5",
            "NaN",
            "inf",
            "1e3",
            "1KiB",
            "18446744073709551616",
            "",
        ] {
            let error = parse(&["--max-bytes", value]).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
            assert!(error.to_string().contains("byte limit must be an integer"));
        }
        assert!(parse(&["--max-bytes"]).is_err());
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
