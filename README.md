# WebP SSIM Optimizer

**Status: Phases 0–1 implemented (API prototype, CLI parsing and validation); image conversion is still pending.** Implementation is tracked in [TODO.md](TODO.md) and designed in [PLAN.md](PLAN.md). The flags below work for validation; conversion examples will produce images after the processing and output phases are implemented.

To validate the current prototype (it generates synthetic images in memory):

```bash
cargo check --all-targets --locked
cargo run --locked --example api_spike
cargo test --all-targets --locked
```

`rust-toolchain.toml` selects Rust 1.89.0 with rustfmt and Clippy. A C compiler and archiver are required: `libwebp-sys` builds its bundled libwebp sources. `cargo run -- --help` and `--version` work. Input and output flags are required; valid arguments and paths currently report that conversion is pending and exit nonzero without writing output. See [Phase 0 findings](PLAN.md#12-phase-0-api-validation-findings) for codec APIs and [Phase 1 decisions](PLAN.md#13-phase-1-cli-and-validation-decisions) for validation details.

Convert a supported still image into a compact WebP file while enforcing a minimum **SSIMULACRA2** perceptual similarity score. Rather than choosing a fixed WebP encoder quality in advance, the tool encodes candidates in memory, decodes and scores them, and writes the smallest **tested** output that meets your requested quality floor.

## Features (planned for v0.1)

- JPEG, PNG, WebP and BMP still-image input.
- WebP output in either lossy or lossless mode.
- Perceptual quality target via native SSIMULACRA2 score (`0..100`, default `80`).
- Optional downscaling using Lanczos3.
- Transparency-aware evaluation and alpha preservation.
- Binary-safe stdout (`-o -`) for command-line pipelines.
- In-memory candidate evaluation with lossless comparison.
- Explicit errors when no candidate can be verified to meet the threshold.

The algorithm is a **fast heuristic**, not an exhaustive global size optimizer. It chooses the smallest *candidate it actually tested* that satisfies the quality constraint.

## Installation (after implementation)

Prerequisites: a Rust toolchain with Rust **1.89 or newer** (initial target MSRV), Cargo, and a platform toolchain capable of building `webp`/its native dependencies.

```bash
git clone <REPOSITORY_URL>
cd webp-ssim-optimizer
cargo build --release --locked
```

The binary will be located at `target/release/webp-ssim-optimizer` (or `.exe` on Windows). The repository URL will be filled in when the project is published.

For a machine-specific optimized build, optionally use:

```bash
RUSTFLAGS="-C target-cpu=native" cargo build --release --locked
```

Native CPU tuning can produce a binary unsuitable for other machines. Normal release builds are the portable default. No benchmark-backed performance claims are made before implementation.

## Usage

```text
webp-ssim-optimizer --input INPUT --output OUTPUT [OPTIONS]
```

| Flag | Meaning | Default |
|---|---|---|
| `-i, --input <PATH>` | Input still image | Required |
| `-o, --output <PATH\|->` | Destination WebP file, or `-` for stdout | Required |
| `-t, --threshold <SCORE>` | Minimum native SSIMULACRA2 score (`0..=100`) | `80` |
| `-s, --scale <FACTOR>` | Output width/height multiplier (`0 < scale <= 1`) | `1.0` |
| `-v, --verbose` | Detailed progress and trial diagnostics on stderr | Off |
| `-h, --help` | Print help | — |
| `-V, --version` | Print version | — |

Example: encode a PNG using the default perceptual target:

```bash
webp-ssim-optimizer -i photo.png -o photo.webp
```

Example: preserve a higher perceptual target:

```bash
webp-ssim-optimizer -i photo.jpg -o photo.webp --threshold 90
```

Example: scale an image to half its width and height before encoding:

```bash
webp-ssim-optimizer -i large.png -o small.webp --scale 0.5 --threshold 85
```

Example: write binary WebP to stdout for redirection:

```bash
webp-ssim-optimizer -i image.png -o - > optimized.webp
```

Only image bytes go to stdout in that mode; logs and errors go to stderr. This matters when piping into another process.

## How quality scoring works

1. Decode the source still image.
2. Resize if `--scale` is different from 1.
3. Treat **that processed image** as the quality reference.
4. Try several lossy WebP encoder qualities, decoding each trial from memory.
5. Score each decoded trial using SSIMULACRA2 against the processed reference.
6. Test a lossless WebP candidate as well.
7. Write the smallest tested candidate whose score is at least the threshold.

The native SSIMULACRA2 scale is **not a percentage** and may yield negative scores for severe distortion. A score of `100` indicates identical imagery under the metric's comparison semantics; `90` is commonly considered visually lossless, around `80` very high quality, and `70` high quality. These are guidance, not guarantees that everyone will perceive images identically. See [SSIMULACRA2 reference](https://github.com/cloudinary/ssimulacra2).

**Important:** when downscaling, the threshold measures **encoding quality after resizing**, not the reduction in quality compared to the original full-size image. For example, `--scale 0.5 --threshold 90` can score 90 even though the image contains fewer pixels.

The optimization may choose lossless because it yields a smaller file (especially for low-complexity graphics) or because no tested lossy setting meets the requested quality.

## Behavior and limitations

- **Not globally optimal:** WebP quality, SSIM scores, and output byte sizes are not strictly monotonic. The bounded binary-search heuristic plus nearby probes finds a useful candidate, but can miss a smaller feasible one.
- **Threshold cannot always be guaranteed:** even the native lossless path is decoded and scored. Alpha normalization and other conversion subtleties can affect the comparison. If nothing satisfies the threshold, the command fails rather than silently lowering the target.
- **Still images only:** animated inputs are not a supported workflow in v0.1.
- **Initial color scope:** 8-bit SDR sRGB-like decoded pixels; ICC color management, HDR, 16-bit precision, and metadata preservation are out of scope.
- **EXIF orientation:** automatic application is not guaranteed in v0.1; pre-orient images when orientation matters.
- **Image metadata:** EXIF, XMP and ICC profiles are not promised to carry over into the WebP output.
- **Resizing:** ratios above 1 are rejected; resulting dimensions are at least 1×1.
- **Source safety:** overwrite of the input path is refused. File destinations are written safely through a temporary file when possible.
- **Safety limits established in Phase 1:** 100 million source pixels, 16,383 pixels per output axis, and a 1 GiB estimated working-memory budget including metric padding. This conservative estimate permits 2000×2000 at full scale, while 2048×2048 requires downscaling. These reusable guards will be applied to image headers and processing in Phase 2; they are not a measured peak-memory guarantee.

## Exit behavior

In the completed optimizer, a successful command will exit with code `0` after
verifying the decoded output against the requested score. Unsupported or
malformed images, encoding/decoding failures, impossible thresholds and output
errors will exit nonzero with details on stderr.

Currently, help/version exit `0`, invalid arguments exit `2`, and path errors or
the pending-conversion diagnostic exit `1`. Parsing and path checks preserve the
input and any existing destination, including rejecting canonical-path, symlink
and hard-link collisions. Missing output paths require an existing parent
directory. Input `-` is unsupported; output `-` selects stdout.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release --locked
```

The current suite includes nine argument/limit unit tests, ten CLI process tests,
and five API spike tests on Linux. It covers numeric boundaries, defaults, help,
required flags, native paths, collisions, file preservation, empty stdout on
failure, checked dimension arithmetic, memory policy, RGB/RGBA round trips,
transparency and animation detection APIs. Image ingestion, resizing and full
optimizer behavior remain for later phases.

See [PLAN.md](PLAN.md) for architecture, edge cases, algorithm decisions, release gates and deferred features. See [TODO.md](TODO.md) for the task-by-task checklist.

## License

Licensed under the [MIT License](LICENSE). This project is not yet released or ready for production.

## References

- [SSIMULACRA2 metric](https://github.com/cloudinary/ssimulacra2)
- [fast-ssim2](https://docs.rs/fast-ssim2/latest/fast_ssim2/)
- [webp encoder](https://docs.rs/webp/latest/webp/struct.Encoder.html)
- [Rust image crate](https://docs.rs/image/latest/image/)
