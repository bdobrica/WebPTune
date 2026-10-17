use clap::Parser;
use webptune::cli::Args;
use webptune::encoder::TrialEvaluator;
use webptune::{input, output, search};

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    args.validate_paths()?;
    let reference = input::load(&args.input, args.scale)?;
    let evaluator = TrialEvaluator::new(reference.pixels())?;
    let result = search::search(&evaluator, args.threshold)?;
    if args.verbose {
        for trial in result.trials() {
            eprintln!(
                "Trial {:?}: {} bytes, decoded SSIMULACRA2 score {:.6}",
                trial.mode, trial.size, trial.score
            );
        }
    }
    let selected = result.candidate();
    output::write(&args, selected)?;
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
