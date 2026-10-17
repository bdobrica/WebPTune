use clap::Parser;
use webp_ssim_optimizer::cli::Args;

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    args.validate_paths()?;
    anyhow::bail!(
        "CLI validation passed; image optimization is not implemented yet (Phases 2–5). Run `cargo run --example api_spike` to validate codec and metric APIs"
    );
}
