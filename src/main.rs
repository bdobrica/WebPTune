use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::Parser;
use webptune::batch::BatchPlan;
use webptune::cancel::Cancellation;
use webptune::cli::{Args, OutputTarget};
use webptune::encoder::TrialEvaluator;
use webptune::{input, output, search};

fn main() -> ExitCode {
    let args = Args::parse();
    let cancel = Cancellation::default();
    let handler = cancel.clone();
    let result = ctrlc::set_handler(move || handler.cancel())
        .context("cannot install cancellation handler")
        .and_then(|()| run(&args, &cancel));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error:#}");
            if cancel.is_cancelled() {
                ExitCode::from(130)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn run(args: &Args, cancel: &Cancellation) -> Result<()> {
    if args.batch {
        let summary = BatchPlan::new_with_cancellation(args, cancel)?
            .execute_with_cancellation(cancel, |task| convert(task, cancel))?;
        for failure in &summary.failures {
            eprintln!("Failed '{}': {:#}", failure.input.display(), failure.error);
        }
        if !args.quiet {
            eprintln!(
                "Batch complete: {} written, {} skipped, {} failed",
                summary.written,
                summary.skipped,
                summary.failures.len()
            );
        }
        if !summary.failures.is_empty() {
            bail!(
                "batch failed for {} image(s); successful outputs were retained",
                summary.failures.len()
            );
        }
    } else {
        convert(args, cancel)?;
    }
    cancel.check()
}

fn convert(args: &Args, cancel: &Cancellation) -> Result<output::WriteOutcome> {
    cancel.check()?;
    args.validate_paths()?;
    if args.verbose && args.is_stdin() {
        eprintln!("Reading stdin until EOF (maximum 256 MiB)...");
    }
    let stdin = if args.is_stdin() {
        Some(input::read_stdin(cancel)?)
    } else {
        None
    };
    let reference = if let Some(stdin) = &stdin {
        stdin
            .prepare(args.resize(), cancel)
            .context("cannot prepare stdin image")?
    } else {
        input::load_with_cancellation(&args.input, args.resize(), cancel)?
    };
    let evaluator = TrialEvaluator::with_cancellation(reference.pixels(), args.encoder, cancel)?;
    if args.verbose {
        eprintln!("Searching WebP candidates...");
    }
    let result = search::search_with_mode(&evaluator, args.threshold, args.search)?;
    cancel.check()?;
    let mut report = String::new();
    if args.batch && !args.quiet {
        let OutputTarget::File(destination) = &args.output else {
            unreachable!()
        };
        writeln!(
            report,
            "File '{}' -> '{}':",
            args.input.display(),
            destination.display()
        )?;
    }
    if args.verbose {
        writeln!(report, "Encoder settings: {:?}", args.encoder)?;
        for trial in result.trials() {
            writeln!(
                report,
                "Trial {:?}: {} bytes, decoded SSIMULACRA2 score {:.6}",
                trial.mode, trial.size, trial.score
            )?;
        }
    }
    let selected = result.candidate();
    let outcome = match output::write_with_cancellation(
        args,
        selected,
        stdin.as_ref().map(input::SpooledInput::bytes),
        cancel,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            // Preserve verbose trial diagnostics on an output-policy/write error.
            io::stderr().lock().write_all(report.as_bytes())?;
            return Err(error);
        }
    };
    if args.quiet {
        return Ok(outcome);
    }
    if let output::WriteOutcome::Skipped { input_bytes } = outcome {
        writeln!(report,
            "Skipped WebP: selected {:?}, {} bytes > input {} bytes; no output written; decoded SSIMULACRA2 score {:.6} >= threshold {}; {} measured trials",
            selected.mode(), selected.size(), input_bytes, selected.score(), args.threshold,
            result.trials().len(),
        )?;
    } else {
        writeln!(report,
        "Wrote WebP: {}x{} -> {}x{} {}; selected {:?}, {} bytes, decoded SSIMULACRA2 score {:.6} >= threshold {}; {} measured trials",
        reference.original_dimensions().0,
        reference.original_dimensions().1,
        reference.pixels().dimensions().0,
        reference.pixels().dimensions().1,
        if reference.pixels().has_alpha() { "RGBA8" } else { "RGB8" },
        selected.mode(),
        selected.size(),
        selected.score(),
        args.threshold,
        result.trials().len(),
    )?;
    }
    // One lock per image keeps its summary and verbose history together when
    // other workers finish concurrently. Completion order is intentionally free.
    io::stderr().lock().write_all(report.as_bytes())?;
    Ok(outcome)
}
