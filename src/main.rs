use clap::Parser;
use webptune::cli::Args;
use webptune::encoder::TrialEvaluator;
use webptune::{input, output, search};

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    args.validate_paths()?;
    let reference = input::load_with_resize(&args.input, args.resize())?;
    let evaluator = TrialEvaluator::new(reference.pixels())?;
    let result = search::search_with_mode(&evaluator, args.threshold, args.search)?;
    if args.verbose {
        for trial in result.trials() {
            eprintln!(
                "Trial {:?}: {} bytes, decoded SSIMULACRA2 score {:.6}",
                trial.mode, trial.size, trial.score
            );
        }
    }
    let selected = result.candidate();
    let outcome = output::write(&args, selected)?;
    if args.quiet {
        return Ok(());
    }
    if let output::WriteOutcome::Skipped { input_bytes } = outcome {
        eprintln!(
            "Skipped WebP: selected {:?}, {} bytes > input {} bytes; no output written; decoded SSIMULACRA2 score {:.6} >= threshold {}; {} measured trials",
            selected.mode(), selected.size(), input_bytes, selected.score(), args.threshold,
            result.trials().len(),
        );
        return Ok(());
    }
    eprintln!(
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
    );
    Ok(())
}
