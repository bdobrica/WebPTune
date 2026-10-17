use clap::Parser;

/// WebP SSIM Optimizer (Phase 0: dependency and API validation).
#[derive(Parser)]
#[command(version)]
struct Args {}

fn main() -> anyhow::Result<()> {
    Args::parse();
    anyhow::bail!(
        "image optimization is not implemented yet; run `cargo run --example api_spike` to validate codec and metric APIs"
    );
}
