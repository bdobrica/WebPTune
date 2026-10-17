use clap::Parser;
use webp_ssim_optimizer::cli::Args;
use webp_ssim_optimizer::encoder::{Mode, TrialEvaluator};
use webp_ssim_optimizer::input;

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    args.validate_paths()?;
    let reference = input::load(&args.input, args.scale)?;
    let evaluator = TrialEvaluator::new(reference.pixels())?;
    // Exercise the completed evaluator while search and output are pending.
    // These are pipeline checks, not threshold-based candidate selection.
    for mode in [Mode::Lossy { quality: 75 }, Mode::Lossless] {
        let trial = evaluator.evaluate(mode)?;
        if args.verbose {
            eprintln!(
                "Trial {:?}: {} bytes, decoded SSIMULACRA2 score {:.6}",
                trial.mode(),
                trial.size(),
                trial.score()
            );
        }
    }
    anyhow::bail!(
        "Candidate evaluation passed: {}x{} -> {}x{} {}; 2 measured trials; search and output are not implemented yet (Phases 4–5)",
        reference.original_dimensions().0,
        reference.original_dimensions().1,
        reference.pixels().dimensions().0,
        reference.pixels().dimensions().1,
        if reference.pixels().has_alpha() { "RGBA8" } else { "RGB8" },
    );
}
