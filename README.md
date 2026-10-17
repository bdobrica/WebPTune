# WebP SSIM Optimizer

**Status: Phases 0–5 implemented, including file and binary stdout output.** The CLI now converts supported still images to measured WebP output. Broader integration coverage, performance work and release automation remain in phases 6–8. Implementation is tracked in [TODO.md](TODO.md) and designed in [PLAN.md](PLAN.md).

To build and validate the current implementation:

```bash
cargo check --all-targets --locked
cargo build --locked
cargo run --locked --example api_spike
cargo test --all-targets --locked
```

`rust-toolchain.toml` selects Rust 1.89.0 with rustfmt and Clippy. A C compiler and archiver are required: `libwebp-sys` builds its bundled libwebp sources. Input and output flags are required; supported still images are decoded, optionally resized, and normalized. The command searches lossy qualities and measures lossless using one cached metric reference, then writes the smallest tested candidate meeting `--threshold`. After writing, it reports the selected mode/quality, dimensions, byte count, decoded score and trial count on stderr. `--verbose` adds each trial's size and decoded score. See [image preparation](PLAN.md#14-phase-2-image-preparation-decisions), [evaluation](PLAN.md#15-phase-3-metric-and-trial-evaluation), [search](PLAN.md#16-phase-4-search-and-selection) and [output decisions](PLAN.md#17-phase-5-output-and-reporting).

Convert a supported still image into a compact WebP file while enforcing a minimum **SSIMULACRA2** perceptual similarity score. Rather than choosing a fixed WebP encoder quality in advance, the tool encodes candidates in memory, decodes and scores them, and writes the smallest **tested** output that meets your requested quality floor.

## Features

- JPEG, PNG, WebP and BMP still-image input.
- WebP output in either lossy or lossless mode.
- Perceptual quality target via native SSIMULACRA2 score (`0..100`, default `80`).
- Optional downscaling using Lanczos3.
- Transparency-aware evaluation and alpha preservation.
- Binary-safe stdout (`-o -`) for command-line pipelines.
- In-memory candidate evaluation with lossless comparison.
- Explicit errors when no candidate can be verified to meet the threshold.

The algorithm is a **fast heuristic**, not an exhaustive global size optimizer. It chooses the smallest *candidate it actually tested* that satisfies the quality constraint.

## Installation

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

Existing destination files are replaced after the complete payload is written and
synced to a temporary file in the same directory. The destination directory must
already exist. Input aliases are refused. Output `./-` writes a file named `-`;
only the exact value `-` selects stdout.

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

The current search tests lossy quality 0 and 100, runs up to seven integer binary
steps, and probes ±4 around the estimated boundary. Duplicate qualities are
cached. The default limit is 18 lossy evaluations plus one measured lossless
evaluation. Selection compares actual byte counts; equal sizes prefer lossy,
then lower quality. Only the best candidate's bytes and numeric trial summaries
are retained. An error means no **tested** candidate met the threshold, rather
than proving all untested qualities infeasible.

## Behavior and limitations

- **Not globally optimal:** WebP quality, SSIM scores, and output byte sizes are not strictly monotonic. The bounded binary-search heuristic plus nearby probes finds a useful candidate, but can miss a smaller feasible one.
- **Threshold cannot always be guaranteed:** even the native lossless path is decoded and scored. Alpha normalization and other conversion subtleties can affect the comparison. If nothing satisfies the threshold, the command fails rather than silently lowering the target.
- **Still images only:** input format is detected from contents, regardless of extension. Animated PNG (APNG) and WebP are rejected before decoding pixels. GIF and other formats are unsupported. JPEG/BMP decoders expose no animation detection; JPEG multi-picture extensions and concatenated images are not enumerated or guaranteed to be rejected.
- **Initial color scope:** decoded samples are treated as SDR sRGB-like values. ICC/profile transforms and linear-light resizing are not implemented. Supported 16-bit PNGs are reduced to 8 bits after resizing, losing precision; HDR fidelity is unsupported.
- **EXIF orientation:** orientation is deliberately not applied. Pre-orient images when orientation matters; stored raster dimensions determine scaling.
- **Image metadata:** EXIF, XMP and ICC profiles are not promised to carry over into the WebP output.
- **Resizing:** each axis uses `max(1, round(dimension * scale))`. Lanczos3 runs once when dimensions change and is bypassed at scale 1 or when rounding keeps the same size. Alpha is premultiplied during filtering and returned as straight alpha; hidden RGB does not tint visible edges. Fully opaque processed images use RGB8; others use RGBA8.
- **Source safety and overwrite policy:** canonical, symlink and hard-link aliases of the input are refused, with another check immediately before replacement. Existing destinations are replaced via a same-directory temporary file and rename; failures before replacement preserve the old destination and clean up staging files. An unrelated destination symlink is replaced as a directory entry, leaving its referent untouched. Dangling destination symlinks are rejected. Concurrent filesystem changes after validation cannot be fully prevented.
- **File permissions and durability:** the replacement is a new temporary file, so existing destination permissions/ownership/metadata are not preserved (on Unix, new outputs normally have mode `0600`). Contents are synced before replacement, but the containing directory is not synced; power-loss durability is not guaranteed. Abrupt process termination may leave a temporary file. Windows/macOS replacement behavior still needs platform testing in Phase 7.
- **Output failures:** write, flush, sync and rename errors return nonzero. Broken stdout pipes fail too; a partial stdout payload cannot be rolled back. Shell redirection creates/truncates its target before the program runs, so file preservation applies to `-o path`, not `-o - > path`. Keep the redirection target separate from the source.
- **Safety limits:** headers are checked before pixel decoding: 100 million source pixels, 16,383 pixels per output axis after scaling, and a 1 GiB estimated working-memory budget including metric padding and resize buffers. This conservative estimate permits 2000×2000 at full scale, while 2048×2048 requires downscaling. Decoder limits are configured, but they are best effort. Headers/metadata can allocate earlier (the JPEG decoder buffers compressed input), and the estimate is not a measured peak-memory guarantee.
- **Corrupt input:** header/pixel decoder errors propagate as failures. Detection follows the underlying decoders; it is not an exhaustive container-integrity check, and permissive decoders can accept recoverable damage or trailing data.

## Exit behavior

A successful conversion exits `0` after writing the selected, decoded-and-scored
WebP. Help/version also exit `0`. Invalid arguments exit `2`; path/input errors,
encoding/decoding failures, unmet thresholds and output errors exit `1` with
details on stderr. Successful file output leaves stdout empty; successful stdout
output contains only WebP bytes. Input `-` is unsupported.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release --locked
```

The suite covers argument and allocation limits, CLI process behavior, all four
input formats, rounded/tiny resizing, RGB/RGBA normalization, grayscale/16-bit
conversion, transparent edges, animation rejection, corrupt/oversized inputs,
EXIF behavior, source preservation and empty stdout on failure. Candidate tests
cover every integer lossy quality, lossless, alpha semantics, dimension/layout
validation and decode errors. Trial scores are verified through an independent
decode and one-shot metric path. Search tests cover nonmonotonic scores/sizes,
exact thresholds, lossless comparison, deduplication, budgets, ties and errors;
real-image selections are checked against every tested feasible candidate and
independently scored. Output tests compare file/stdout payloads with selected
bytes, independently decode/score written images, check stderr summaries and
verbose history, and cover staged replacement, cleanup, late collisions, partial
writes, injected disk-full/flush errors and a real broken stdout connection.
The suite also retains the Phase 0 API tests.

The synthetic fixtures are committed under `tests/fixtures`, with provenance in
[their README](tests/fixtures/README.md). Regenerate them with:

```bash
cargo run --locked --example generate_fixtures
```

See [PLAN.md](PLAN.md) for architecture, edge cases, algorithm decisions, release gates and deferred features. See [TODO.md](TODO.md) for the task-by-task checklist.

## License

Licensed under the [MIT License](LICENSE). This project is not yet released or ready for production.

## References

- [SSIMULACRA2 metric](https://github.com/cloudinary/ssimulacra2)
- [fast-ssim2](https://docs.rs/fast-ssim2/latest/fast_ssim2/)
- [webp encoder](https://docs.rs/webp/latest/webp/struct.Encoder.html)
- [Rust image crate](https://docs.rs/image/latest/image/)
