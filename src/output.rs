//! Binary stdout and same-directory staged file replacement.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use anyhow::{ensure, Context, Result};
use tempfile::NamedTempFile;

use crate::cli::{Args, OutputTarget};
use crate::search::Candidate;

/// A skip is successful but writes no bytes to either destination kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteOutcome {
    Written,
    Skipped { input_bytes: u64 },
}

/// Write a measured candidate without changing its bytes.
///
/// `Candidate` has private fields and immutable bytes: its production constructor
/// has already verified nonempty, valid still WebP, decoded dimensions and score.
/// Reuse that validation rather than decoding/scoring the winner a second time.
/// Check feasibility again here so library callers cannot write a trial that
/// fails the requested threshold. Enforce the inclusive byte ceiling before
/// considering a size-based skip. Neither policy relaxes the quality threshold
/// or changes the search; the smallest tested feasible candidate already fits
/// any cap that another tested feasible candidate could fit.
///
/// Existing destinations are replaced, including the directory entry of an
/// unrelated symlink (its referent is untouched). Input aliases are refused.
/// File contents are synced before replacement; directory crash durability and
/// protection against concurrent filesystem manipulation are not guaranteed.
/// Partial stdout writes cannot be rolled back.
pub fn write(args: &Args, candidate: &Candidate) -> Result<WriteOutcome> {
    ensure!(
        !candidate.bytes().is_empty(),
        "WebP candidate contains no bytes"
    );
    ensure!(
        args.threshold.is_finite()
            && (0.0..=100.0).contains(&args.threshold)
            && candidate.score().is_finite()
            && candidate.score() >= args.threshold,
        "candidate does not satisfy the requested SSIMULACRA2 threshold"
    );
    let candidate_bytes = u64::try_from(candidate.size()).context("WebP size exceeds u64")?;
    if let Some(limit) = args.max_bytes {
        ensure!(limit > 0, "byte limit must be greater than zero");
        ensure!(
            candidate_bytes <= limit,
            "smallest tested feasible WebP is {candidate_bytes} bytes, exceeding --max-bytes {limit}; no output written"
        );
    }
    if args.skip_if_larger {
        args.validate_paths()?;
        let input_bytes = std::fs::metadata(&args.input)
            .with_context(|| format!("cannot inspect input size '{}'", args.input.display()))?
            .len();
        if candidate_bytes > input_bytes {
            return Ok(WriteOutcome::Skipped { input_bytes });
        }
    }
    match &args.output {
        OutputTarget::Stdout => {
            args.validate_paths()?;
            write_payload(&mut io::stdout().lock(), candidate.bytes())
                .context("cannot write WebP to stdout")?;
        }
        OutputTarget::File(_) => {
            write_file_with(args, |file| write_payload(file, candidate.bytes()))?;
        }
    }
    Ok(WriteOutcome::Written)
}

fn write_payload(writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    writer.write_all(bytes)?;
    // stdout may buffer small payloads; report deferred write failures too.
    writer.flush()
}

fn write_file_with(args: &Args, stage: impl FnOnce(&mut File) -> io::Result<()>) -> Result<()> {
    args.validate_paths()?;
    let OutputTarget::File(destination) = &args.output else {
        anyhow::bail!("file output requires a destination path");
    };
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = NamedTempFile::new_in(parent)
        .with_context(|| format!("cannot create temporary output in '{}'", parent.display()))?;
    stage(temporary.as_file_mut()).with_context(|| {
        format!(
            "cannot write temporary output for '{}'",
            destination.display()
        )
    })?;
    temporary.as_file().sync_all().with_context(|| {
        format!(
            "cannot sync temporary output for '{}'",
            destination.display()
        )
    })?;
    // Search and staging can take time: repeat collision checks immediately
    // before replacement. Never open/truncate the final destination for writing.
    args.validate_paths()?;
    temporary
        .persist(destination)
        // PersistError owns the temporary file. Extract the I/O error and drop
        // that file here so failed renames also trigger automatic cleanup.
        .map_err(|error| error.error)
        .with_context(|| format!("cannot replace output '{}'", destination.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::Mode;
    use tempfile::{tempdir, TempDir};

    fn fixture() -> (TempDir, Args) {
        let directory = tempdir().unwrap();
        let input = directory.path().join("input.png");
        std::fs::write(&input, include_bytes!("../tests/fixtures/rgb.png")).unwrap();
        let args = Args {
            input,
            output: OutputTarget::File(directory.path().join("output.webp")),
            batch: false,
            recursive: false,
            jobs: None,
            threshold: 80.0,
            scale: 1.0,
            scale_width: None,
            scale_height: None,
            search: crate::search::SearchMode::Heuristic,
            skip_if_larger: false,
            max_bytes: None,
            quiet: false,
            verbose: false,
        };
        (directory, args)
    }

    #[test]
    fn empty_or_infeasible_candidates_fail_before_staging() {
        let (directory, mut args) = fixture();
        for (score, size) in [(100.0, 0), (79.999, 10), (f64::NAN, 10)] {
            let candidate = Candidate::synthetic(Mode::Lossless, score, size);
            assert!(write(&args, &candidate).is_err());
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        }
        let candidate = Candidate::synthetic(Mode::Lossless, 100.0, 10);
        for threshold in [f64::NAN, -1.0, 101.0] {
            args.threshold = threshold;
            assert!(write(&args, &candidate).is_err());
        }
    }

    #[test]
    fn byte_limit_is_inclusive_and_checked_before_staging() {
        for limit in [0, 9, 10, 11, u64::MAX] {
            let (directory, mut args) = fixture();
            args.max_bytes = Some(limit);
            let OutputTarget::File(destination) = &args.output else {
                unreachable!()
            };
            std::fs::write(destination, b"old destination").unwrap();
            let candidate = Candidate::synthetic(Mode::Lossless, 100.0, 10);
            let result = write(&args, &candidate);
            if limit < 10 {
                assert!(result.is_err());
                assert_eq!(std::fs::read(destination).unwrap(), b"old destination");
            } else {
                assert_eq!(result.unwrap(), WriteOutcome::Written);
                assert_eq!(std::fs::read(destination).unwrap(), candidate.bytes());
            }
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
        }
    }

    #[test]
    fn skip_only_strictly_larger_results_and_preserve_existing_destination() {
        for existing in [false, true] {
            for difference in [-1, 0, 1] {
                let (directory, mut args) = fixture();
                args.skip_if_larger = true;
                let input_bytes = std::fs::metadata(&args.input).unwrap().len();
                let size = (input_bytes as i64 + difference) as usize;
                let candidate = Candidate::synthetic(Mode::Lossless, 100.0, size);
                let OutputTarget::File(destination) = &args.output else {
                    unreachable!()
                };
                if existing {
                    std::fs::write(destination, b"old destination").unwrap();
                }
                let result = write(&args, &candidate).unwrap();
                if difference > 0 {
                    assert_eq!(result, WriteOutcome::Skipped { input_bytes });
                    if existing {
                        assert_eq!(std::fs::read(destination).unwrap(), b"old destination");
                    } else {
                        assert!(!destination.exists());
                    }
                } else {
                    assert_eq!(result, WriteOutcome::Written);
                    assert_eq!(std::fs::read(destination).unwrap(), candidate.bytes());
                }
                assert_eq!(
                    std::fs::read(&args.input).unwrap(),
                    include_bytes!("../tests/fixtures/rgb.png")
                );
                assert_eq!(
                    std::fs::read_dir(directory.path()).unwrap().count(),
                    if difference > 0 && !existing { 1 } else { 2 }
                );
            }
        }
    }

    #[test]
    fn skip_does_not_hide_quality_cap_or_path_errors() {
        let (directory, mut args) = fixture();
        args.skip_if_larger = true;
        let input_bytes = std::fs::metadata(&args.input).unwrap().len();
        let size = input_bytes as usize + 1;
        let candidate = Candidate::synthetic(Mode::Lossless, 100.0, size);
        args.max_bytes = Some(input_bytes);
        assert!(write(&args, &candidate)
            .unwrap_err()
            .to_string()
            .contains("--max-bytes"));
        args.max_bytes = None;
        let infeasible = Candidate::synthetic(Mode::Lossless, 79.0, size);
        assert!(write(&args, &infeasible)
            .unwrap_err()
            .to_string()
            .contains("threshold"));
        args.output = OutputTarget::File(args.input.clone());
        assert!(write(&args, &candidate)
            .unwrap_err()
            .to_string()
            .contains("refers to the input"));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn destination_survives_staging_until_complete_replacement() {
        let (directory, args) = fixture();
        let OutputTarget::File(destination) = &args.output else {
            unreachable!()
        };
        std::fs::write(destination, b"old destination").unwrap();
        write_file_with(&args, |file| {
            assert_eq!(std::fs::read(destination).unwrap(), b"old destination");
            // The only staging file is beside input and destination.
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 3);
            file.write_all(b"complete new payload")?;
            assert_eq!(std::fs::read(destination).unwrap(), b"old destination");
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read(destination).unwrap(), b"complete new payload");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn disk_full_after_partial_staging_cleans_up_and_preserves_destination() {
        for existing in [false, true] {
            let (directory, args) = fixture();
            let OutputTarget::File(destination) = &args.output else {
                unreachable!()
            };
            if existing {
                std::fs::write(destination, b"old destination").unwrap();
            }
            let original = std::fs::read(&args.input).unwrap();
            let error = write_file_with(&args, |file| {
                file.write_all(b"partial payload")?;
                Err(io::Error::from(io::ErrorKind::StorageFull))
            })
            .unwrap_err();
            assert_eq!(
                error.downcast_ref::<io::Error>().unwrap().kind(),
                io::ErrorKind::StorageFull
            );
            assert!(error.to_string().contains("cannot write temporary output"));
            if existing {
                assert_eq!(std::fs::read(destination).unwrap(), b"old destination");
            } else {
                assert!(!destination.exists());
            }
            assert_eq!(std::fs::read(&args.input).unwrap(), original);
            assert_eq!(
                std::fs::read_dir(directory.path()).unwrap().count(),
                if existing { 2 } else { 1 }
            );
        }
    }

    #[test]
    fn collision_introduced_during_staging_is_rechecked() {
        let (directory, args) = fixture();
        let OutputTarget::File(destination) = &args.output else {
            unreachable!()
        };
        let original = std::fs::read(&args.input).unwrap();
        let error = write_file_with(&args, |file| {
            file.write_all(b"new payload")?;
            std::fs::hard_link(&args.input, destination)
        })
        .unwrap_err();
        assert!(error.to_string().contains("refers to the input file"));
        assert_eq!(std::fs::read(&args.input).unwrap(), original);
        assert_eq!(std::fs::read(destination).unwrap(), original);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn failed_persist_cleans_up_and_preserves_existing_destination() {
        let (directory, args) = fixture();
        let OutputTarget::File(destination) = &args.output else {
            unreachable!()
        };
        std::fs::write(destination, b"old destination").unwrap();
        let error = write_file_with(&args, |file| {
            file.write_all(b"new payload")?;
            // Removing an open staging file forces rename failure on Unix.
            let staging = std::fs::read_dir(directory.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path != &args.input && path != destination)
                .unwrap();
            std::fs::remove_file(staging)
        })
        .unwrap_err();
        assert!(error.to_string().contains("cannot replace output"));
        assert_eq!(std::fs::read(destination).unwrap(), b"old destination");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    struct ShortWriter {
        bytes: Vec<u8>,
        interrupt_once: bool,
        flush_error: Option<io::ErrorKind>,
    }

    impl Write for ShortWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.interrupt_once {
                self.interrupt_once = false;
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            let count = bytes.len().min(2);
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            match self.flush_error {
                Some(kind) => Err(io::Error::from(kind)),
                None => Ok(()),
            }
        }
    }

    #[test]
    fn payload_retries_short_and_interrupted_writes_and_propagates_flush_failures() {
        for flush_error in [
            None,
            Some(io::ErrorKind::BrokenPipe),
            Some(io::ErrorKind::StorageFull),
        ] {
            let mut writer = ShortWriter {
                bytes: Vec::new(),
                interrupt_once: true,
                flush_error,
            };
            let result = write_payload(&mut writer, b"complete payload");
            assert_eq!(writer.bytes, b"complete payload");
            match flush_error {
                Some(kind) => assert_eq!(result.unwrap_err().kind(), kind),
                None => result.unwrap(),
            }
        }
    }
}
