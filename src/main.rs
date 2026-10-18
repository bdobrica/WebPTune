use std::fmt::Write as _;
use std::io::{self, Write as _};

use anyhow::{bail, Result};
use clap::Parser;
use webptune::batch::BatchPlan;
use webptune::cli::{Args, OutputTarget};
use webptune::encoder::TrialEvaluator;
use webptune::{input, output, search};

fn main() -> Result<()> {
    let args = Args::parse();
    if args.batch {
        let summary = BatchPlan::new(&args)?.execute(convert)?;
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
        convert(&args)?;
    }
    Ok(())
}

fn convert(args: &Args) -> Result<output::WriteOutcome> {
    args.validate_paths()?;
    let reference = input::load_with_resize(&args.input, args.resize())?;
    let evaluator = TrialEvaluator::with_options(reference.pixels(), args.encoder)?;
    let result = search::search_with_mode(&evaluator, args.threshold, args.search)?;
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
    let outcome = match output::write(args, selected) {
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
