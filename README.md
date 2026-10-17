# WebPTune

Convert JPEG, PNG, WebP or BMP still images to WebP with a minimum measured
**SSIMULACRA2** score. The tool encodes candidates, decodes and scores them, then
writes the smallest **tested** output meeting the threshold. It can resize first
and preserves transparency.

**Status:** v0.1.0 is in release preparation. Linux validation passes; the
[CI matrix](.github/workflows/ci.yml) is configured for Linux, macOS and Windows,
but hosted runs and macOS/Windows validation are still pending. There is no
release tag yet. Progress is tracked in [TODO.md](TODO.md); release gates and
commands are in [RELEASING.md](RELEASING.md).

## Quick start

From the repository root, with the [build prerequisites](#installation) installed:

```bash
cargo build --release --locked
target/release/webptune -i tests/fixtures/rgb.png -o target/quickstart.webp
target/release/webptune -i tests/fixtures/rgb.png -o - > target/quickstart-stdout.webp
```

Both outputs are WebP images; the conversion summary goes to stderr. Replace the
fixture path with your image. The binary stays in `target/release` unless you
install it on your PATH.

On Windows, build with the same Cargo command, then run these from PowerShell:

```powershell
.\target\release\webptune.exe -i tests/fixtures/rgb.png -o target/quickstart.webp
cmd /d /c "target\release\webptune.exe -i tests/fixtures/rgb.png -o - > target\quickstart-stdout.webp"
```

The stdout example uses `cmd` for byte-preserving redirection, including when
launched from Windows PowerShell 5.1. Keep a redirection target separate from the
source: the shell opens it before the application starts.

## Features

- JPEG, PNG, WebP and BMP still-image input.
- WebP output in either lossy or lossless mode.
- Perceptual quality target via native SSIMULACRA2 score (`0..100`, default `80`).
- Optional Lanczos3 downscaling by a multiplier or maximum width/height.
- Transparency-aware evaluation and alpha preservation.
- Binary-safe stdout (`-o -`) for command-line pipelines.
- In-memory candidate evaluation with lossless comparison.
- Explicit errors when no candidate can be verified to meet the threshold.

The algorithm is a **fast heuristic**, not an exhaustive global size optimizer. It chooses the smallest *candidate it actually tested* that satisfies the quality constraint.

## Installation

Install [Rust through rustup](https://rustup.rs/) and a native C build toolchain.
The checked-in `rust-toolchain.toml` selects **Rust 1.89.0**, rustfmt and Clippy;
1.89 is the minimum supported Rust version for the locked dependencies.

| Platform | Native build tools | CI target |
|---|---|---|
| Linux | GCC or Clang, linker and archiver; on Debian/Ubuntu: `sudo apt-get install build-essential` | x86_64 GNU/Linux |
| macOS | Xcode Command Line Tools: `xcode-select --install` | Apple Silicon (aarch64) |
| Windows | Visual Studio 2022 Build Tools, **Desktop development with C++**, MSVC and Windows SDK; use Rust's MSVC host | x86_64 MSVC |

`libwebp-sys` compiles bundled libwebp and SharpYUV. No system libwebp package,
CMake, pkg-config or separate codec executable is required by this build. See
[the dependency and native-build review](DEPENDENCIES.md) for the locked versions,
features and platform validation limits.

Open a terminal in your downloaded or cloned checkout and build:

```bash
cargo build --release --locked
```

The executable is `target/release/webptune` (`.exe` on Windows).
To install it on your Cargo binary PATH from this checkout:

```bash
cargo install --path . --locked
webptune --version
```

The package is not published to crates.io (`publish = false`); installation is
from source. No remote URL is configured in this checkout yet. The commands in
[Quick start](#quick-start) need no PATH change and use the included fixtures.

For a machine-specific optimized build, optionally use:

```bash
RUSTFLAGS="-C target-cpu=native" cargo build --release --locked
```

Native CPU tuning can produce a binary unsuitable for other machines. Normal release builds are the portable default. See the [local release/native measurements](benchmarks/RESULTS.md) for observed performance and their limits.

## Usage

```text
webptune --input INPUT --output OUTPUT [OPTIONS]
```

| Flag | Meaning | Default |
|---|---|---|
| `-i, --input <PATH>` | Input still image | Required |
| `-o, --output <PATH\|->` | Destination WebP file, or `-` for stdout | Required |
| `-t, --threshold <SCORE>` | Minimum native SSIMULACRA2 score (`0..=100`) | `80` |
| `-s, --scale <FACTOR>` | Output width/height multiplier (`0 < scale <= 1`) | `1.0` |
| `--scale-width <PIXELS>` | Maximum output width, preserving aspect ratio; alternative to `--scale` | Unbounded |
| `--scale-height <PIXELS>` | Maximum output height, preserving aspect ratio; alternative to `--scale` | Unbounded |
| `-v, --verbose` | Detailed progress and trial diagnostics on stderr | Off |
| `-h, --help` | Print help | — |
| `-V, --version` | Print version | — |

Example: encode a PNG using the default perceptual target:

```bash
webptune -i photo.png -o photo.webp
```

Example: preserve a higher perceptual target:

```bash
webptune -i photo.jpg -o photo.webp --threshold 90
```

Example: scale an image to half its width and height before encoding:

```bash
webptune -i large.png -o small.webp --scale 0.5 --threshold 85
```

Example: fit within 1920×1080, preserving aspect ratio without cropping or stretching:

```bash
webptune -i large.png -o fitted.webp --scale-width 1920 --scale-height 1080
```

Either bound can be used alone, for example `--scale-width 800`. Bounds must be
positive integers up to `4294967295`; actual output must still pass the codec
and memory limits below. Images already within the bounds keep their size;
images are never upscaled. An explicitly supplied `--scale` (including `-s 1`)
cannot be combined with either dimension bound. With no resizing options, the
default remains `--scale 1.0`.

Example: write binary WebP to stdout for redirection:

```bash
webptune -i image.png -o - > optimized.webp
```

Only image bytes go to stdout in that mode; logs and errors go to stderr. This matters when piping into another process.

Existing destination files are replaced after the complete payload is written and
synced to a temporary file in the same directory. The destination directory must
already exist. Input aliases are refused. Output `./-` writes a file named `-`;
only the exact value `-` selects stdout.

## How quality scoring works

1. Decode the source still image.
2. Resize once if the scale multiplier or dimension bounds change the dimensions.
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
- **Resizing:** `--scale` uses `max(1, round(dimension * scale))` on each axis. Dimension bounds use the smaller of 1, maximum width/source width and maximum height/source height (omitting unspecified bounds), with the same nearest-pixel rounding and one-pixel minimum. Both axes share one ratio; whole-pixel rounding can slightly change the exact aspect ratio, especially for tiny images. Output stays within every supplied bound. Lanczos3 runs once when dimensions change and is bypassed otherwise. Alpha is premultiplied during filtering and returned as straight alpha; hidden RGB does not tint visible edges. Fully opaque processed images use RGB8; others use RGBA8.
- **Source safety and overwrite policy:** canonical, symlink and hard-link aliases of the input are refused, with another check immediately before replacement. Existing destinations are replaced via a same-directory temporary file and rename; failures before replacement preserve the old destination and clean up staging files. An unrelated destination symlink is replaced as a directory entry, leaving its referent untouched. Dangling destination symlinks are rejected. Concurrent filesystem changes after validation cannot be fully prevented.
- **File permissions and durability:** the replacement is a new temporary file, so existing destination permissions/ownership/metadata are not preserved (on Unix, new outputs normally have mode `0600`). Contents are synced before replacement, but the containing directory is not synced; power-loss durability is not guaranteed. Abrupt process termination may leave a temporary file. Windows/macOS replacement behavior still needs platform execution; the Phase 7 platform check remains open.
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
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked --bin webptune --example smoke
cargo run --release --locked --example smoke -- target/release/webptune
```

On Windows append `.exe` to the smoke command's final path. The smoke checker
executes the release binary for RGB/RGBA inputs, replaces an existing file,
compares file/stdout bytes, independently decodes/scores them, and verifies
scaled dimensions, alpha and source preservation.

[CI](.github/workflows/ci.yml) runs the commands above on push, pull request and
manual dispatch on all three target platforms. Each job uses a fresh checkout
and the repository toolchain, including native replacement tests. It validates
builds without publishing packages, tags or binaries. Hosted execution is pending.

The suite covers argument and allocation limits, CLI process behavior, all four
input formats, rounded/tiny resizing, width/height bounds and option conflicts,
RGB/RGBA normalization, grayscale/16-bit
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

The executable matrix crosses 11 fixtures with scales 1/0.5, thresholds
0/80/90/100, and file/stdout output (176 CLI invocations). Every payload is
independently decoded and scored, with byte-identical file/stdout results and
checks for dimensions, alpha, diagnostics and source preservation. Unix tests
also exercise unreadable sources and unwritable output directories; these
explicitly report a skip if the user/filesystem bypasses Unix permissions.
Impossible valid thresholds use synthetic search tests because measured
lossless reaches 100 on the real corpus. See
[Phase 6 coverage](PLAN.md#18-phase-6-functional-and-integration-coverage).

The synthetic fixtures are committed under `tests/fixtures`, with provenance in
[their README](tests/fixtures/README.md). Regenerate them with:

```bash
cargo run --locked --example generate_fixtures
```

The generator's test rebuilds the corpus twice in temporary directories and
compares every fixture byte with the committed files using locked dependencies.

The [benchmark harness](benchmarks/README.md) measures real search trial counts,
encoding, decoding, metric work, reference caching, complete CLI conversions and
per-process peak memory. It compares separate portable/native release builds on
identical hashed photo, noisy-photo, icon and transparent-art workloads. Reviewed
[Linux results and memory-limit assessment](benchmarks/RESULTS.md) include the raw
repetitions and metadata. The application has no additional runtime dependency;
the developer runner uses Python's standard library.

See [PLAN.md](PLAN.md) for architecture, edge cases, algorithm decisions, release gates and deferred features. See [TODO.md](TODO.md) for the task-by-task checklist.

## License

Licensed under the [Apache License 2.0](LICENSE). The NASA benchmark photograph
is public domain; its [provenance](benchmarks/README.md#workloads-and-provenance)
is recorded separately. Dependencies retain their own licenses; see
[DEPENDENCIES.md](DEPENDENCIES.md).

## References

- [SSIMULACRA2 metric](https://github.com/cloudinary/ssimulacra2)
- [fast-ssim2](https://docs.rs/fast-ssim2/latest/fast_ssim2/)
- [webp encoder](https://docs.rs/webp/latest/webp/struct.Encoder.html)
- [Rust image crate](https://docs.rs/image/latest/image/)
