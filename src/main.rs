use clap::Parser;
use webp_ssim_optimizer::cli::Args;
use webp_ssim_optimizer::input;

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    args.validate_paths()?;
    let reference = input::load(&args.input, args.scale)?;
    anyhow::bail!(
        "Image preparation passed: {}x{} -> {}x{} {}; image optimization is not implemented yet (Phases 3–5)",
        reference.original_dimensions().0,
        reference.original_dimensions().1,
        reference.pixels().dimensions().0,
        reference.pixels().dimensions().1,
        if reference.pixels().has_alpha() { "RGBA8" } else { "RGB8" },
    );
}
