# WebPTune

Convert JPEG, PNG, WebP, BMP or Radiance HDR still images to WebP with a minimum measured
**SSIMULACRA2** score. The tool encodes candidates, decodes and scores them, then
writes the smallest **tested** output meeting the threshold. It can resize first
and preserves transparency.

**Status:** [v0.1.0 is tagged](https://github.com/bdobrica/WebPTune/tree/v0.1.0)
at commit `faf6ae1`. The native Linux, macOS and Windows
[CI matrix passed](https://github.com/bdobrica/WebPTune/actions/runs/39074733215)
on that commit, including tests, release builds and release smoke checks.
Install from source using the instructions below. Progress is tracked in
[TODO.md](TODO.md); release validation is recorded in [RELEASING.md](RELEASING.md).

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

- JPEG, PNG, WebP, BMP and Radiance HDR still-image input.
- Automatic EXIF orientation and RGB/grayscale ICC conversion to sRGB.
- Float processing for 16-bit sources and HDR-to-SDR tone mapping.
- WebP output in either lossy or lossless mode.
- Configurable compression method, lossless effort and lossy alpha quality.
- Optional byte ceiling and skipping of results larger than the source file.
- Directory batches with optional recursion and a configurable worker limit.
- Perceptual quality target via native SSIMULACRA2 score (`0..100`, default `80`).
- Optional Lanczos3 downscaling by a multiplier or maximum width/height.
- Transparency-aware evaluation and exact alpha by default.
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
from source in the [GitHub repository](https://github.com/bdobrica/WebPTune). The commands in
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
| `-i, --input <PATH>` | Input still image, or directory with `--batch` | Required |
| `-o, --output <PATH\|->` | Destination WebP file, `-` for stdout, or existing directory with `--batch` | Required |
| `--batch` | Convert a directory of supported images to a separate directory | Off |
| `--recursive` | Include subdirectories and preserve their relative paths; requires `--batch` | Off |
| `--jobs <COUNT>` | Concurrent image pipelines, integer `1..=64`; requires `--batch` | `1` |
| `-t, --threshold <SCORE>` | Minimum native SSIMULACRA2 score (`0..=100`) | `80` |
| `-s, --scale <FACTOR>` | Output width/height multiplier (`0 < scale <= 1`) | `1.0` |
| `--scale-width <PIXELS>` | Maximum output width, preserving aspect ratio; alternative to `--scale` | Unbounded |
| `--scale-height <PIXELS>` | Maximum output height, preserving aspect ratio; alternative to `--scale` | Unbounded |
| `--search <MODE>` | `heuristic` (bounded search) or `exhaustive` (all 102 fixed candidates) | `heuristic` |
| `--method <0..6>` | Compression method for both modes; 0 fastest, 6 slowest | `4` |
| `--lossless-effort <0..100>` | Compression effort of the lossless candidate; 0 fastest, 100 slowest | `75` |
| `--alpha-quality <0..100>` | Alpha quality of lossy candidates; 100 preserves alpha exactly | `100` |
| `--alpha-filtering <MODE>` | Lossy alpha predictive filtering: `none`, `fast`, `best` | `fast` |
| `--sharp-yuv` | Slower, sharper RGB-to-YUV conversion in lossy candidates | Off |
| `--exact` | Preserve hidden RGB under fully transparent pixels in the lossless candidate | Off |
| `--skip-if-larger` | Skip writing when the selected WebP is strictly larger than the input file | Off |
| `--max-bytes <BYTES>` | Inclusive positive integer byte ceiling; fail if the smallest tested feasible result exceeds it | Unlimited |
| `-q, --quiet` | Suppress success/skip reports; retain errors; conflicts with verbose | Off |
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

Example: evaluate every integer lossy quality and lossless:

```bash
webptune -i photo.png -o photo.webp --search exhaustive --threshold 90
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

Example: require at most 100,000 bytes without lowering the quality target:

```bash
webptune -i photo.png -o photo.webp --threshold 90 --max-bytes 100000
```

`--max-bytes` accepts a positive integer through `18446744073709551615`, in
bytes (no units or fractions). The limit is inclusive: an exactly equal result
is written. If the smallest tested feasible candidate exceeds it, the command
exits `1` without creating/replacing a file or writing stdout. The threshold is
never lowered, and search performs its usual trials. Heuristic failure concerns
only tested candidates; `--search exhaustive` checks the complete fixed set.

Example: keep the destination untouched when conversion would increase file size:

```bash
webptune -i photo.jpg -o photo.webp --skip-if-larger
webptune -i photo.jpg -o photo.webp --skip-if-larger --quiet
```

`--skip-if-larger` compares the selected payload against the source file's byte
length at the output decision, including any source metadata, even when resizing.
A strictly larger result exits `0` with a `Skipped WebP` report on stderr and no
output. Equal sizes are written. Existing destinations remain untouched; absent
destinations stay absent. With `-o -`, a skip produces **empty stdout**, not a
WebP image or a copy of the source. Shell redirection can still create/truncate
its own target; use `-o path` when preservation matters. Validation, quality and
byte-limit failures take priority over skipping and still exit nonzero.

`-q`/`--quiet` suppresses success and skip reports, keeping error diagnostics on
stderr and the same exit codes and output bytes. It conflicts with `--verbose`.
It does not suppress explicit help/version output. JSON reporting remains a
possible future enhancement.

### Encoder tuning

These controls are available on the source branch after v0.1.0. For example:

```bash
webptune -i photo.png -o photo.webp --method 6 --sharp-yuv --lossless-effort 100
webptune -i artwork.png -o artwork.webp --alpha-quality 80 --alpha-filtering best -t 90
webptune -i artwork.png -o exact.webp --exact --threshold 100
```

`--method` controls the compression speed/size tradeoff for both lossy and
lossless trials. `--lossless-effort` sets the lossless encoder's quality parameter
as compression effort; it does not reduce pixel fidelity. Higher effort can
take longer and does not guarantee a smaller selected output. The lossy RGB
quality parameter remains selected by search.

`--alpha-quality 100` preserves alpha exactly. Lower values permit quantized
alpha **only in lossy trials**. The actual decoded alpha participates in the
quality score, so lowering this setting never bypasses the threshold. It may
cause search to select lossless instead. `--alpha-filtering` selects alpha
prediction during compression and does not itself lower alpha quality. These
alpha options have no effect on fully opaque processed inputs.

`--sharp-yuv` changes lossy color conversion and has no effect on lossless.
`--exact` affects only the lossless candidate, preserving RGB even where alpha
is zero. Without it, the encoder can discard invisible RGB for compression.
The metric ignores RGB hidden under alpha zero, so threshold 100 alone does not
promise preservation of those samples. `--exact` does not force lossless selection;
its guarantee applies when the selected mode is lossless.

The lossless candidate always keeps full alpha, disables near-lossless
preprocessing, and is decoded and scored. All settings stay fixed throughout
heuristic or exhaustive search and apply to each batch image. Exhaustive search
still checks 102 candidates at these settings; it does not search combinations
of methods, alpha qualities or other controls. Verbose stderr includes the
active settings. With no tuning flags, encoded bytes and defaults are unchanged.
The parameter meanings follow libwebp's
[encoder controls](https://developers.google.com/speed/webp/docs/cwebp).

### Color, orientation and HDR

These preparation improvements are available on the current source branch after
v0.1.0. They apply automatically in single-file and batch mode:

```bash
webptune -i camera.jpg -o oriented.webp --scale-width 1920
webptune -i wide-gamut-16bit.png -o srgb.webp --threshold 90
webptune -i scene.hdr -o preview.webp --scale-width 1920
```

JPEG/PNG/WebP EXIF orientation is applied before calculating output dimensions,
so maximum width/height refer to the displayed orientation. All eight EXIF
rotations/reflections are supported. Missing or invalid orientation tags follow
the decoder's default of no transform; metadata read errors fail the conversion.
The summary's source dimensions describe the stored raster.

RGB and grayscale ICC profiles exposed by the JPEG/PNG/WebP decoders are
converted to sRGB with relative colorimetric intent; alpha is untouched by the
color transform. Unsupported or invalid profiles fail explicitly. PNG color
descriptions take precedence in this order: `cICP`, `iCCP`, `sRGB`, `gAMA`/`cHRM`.
Supported PNG cICP primaries are BT.709, BT.2020, DCI P3 and Display P3 (codes
1/9/11/12), with supported SDR transfer curves, identity matrix and full range.
Untagged inputs assume sRGB. BMP profile extraction is not supported: convert
profiled BMPs to a tagged PNG first. Out-of-sRGB colors are clipped; there is no
perceptual gamut mapping or black-point compensation.

Color-managed and 16-bit PNG sources use normalized float samples through color
conversion and the single resize, then quantize to RGB8/RGBA8 for WebP and the
metric. This avoids early 8-bit reduction; the CMS itself uses finite-precision
curves/LUTs (its float transform is approximately 14-bit). Filtering still uses
encoded sRGB samples with premultiplied alpha, rather than linear-light resizing.

Radiance `.hdr` input is interpreted as **linear sRGB**. Stored samples are used
directly, including any exposure/color correction already baked into them;
`EXPOSURE`/`COLORCORR` headers are not reapplied or undone. A fixed-exposure,
luminance-preserving Reinhard map divides each linear RGB channel by `1 + Y`,
where `Y = 0.2126 R + 0.7152 G + 0.0722 B`, clips to `[0,1]`, then applies the
sRGB transfer function. Neutral linear `1` becomes approximately `188/255`.
Tone mapping precedes resizing. Custom `PRIMARIES`, nonsquare pixels, XYZE,
PQ/HLG descriptions and other HDR formats are unsupported; preconvert them to
linear-sRGB RGBE without a `PRIMARIES` header before using this path.

Output is always **8-bit SDR sRGB WebP**, without source ICC/EXIF/XMP metadata.
The threshold compares the decoded WebP with the **oriented, color-converted,
tone-mapped, resized and quantized reference**. It does not measure the loss of
HDR range, wide gamut, source precision or source resolution.

### Directory batches

Create a separate output directory, then convert the top-level images or include
the entire directory tree:

```bash
mkdir -p optimized
webptune --batch -i photos -o optimized
webptune --batch -i photos -o optimized --recursive --jobs 2 --threshold 90
```

Batch mode is available on the current source branch after v0.1.0. Both roots
must be existing directories; their canonical trees cannot overlap. Nested
output directories are created as needed. For example, `photos/trip/image.jpg`
becomes `optimized/trip/image.webp`. Existing destination files use the same
staged replacement as single-image output.

Discovery selects regular files with `.jpg`, `.jpeg`, `.png`, `.webp`, `.bmp` or `.hdr`
extensions, case insensitively. Other extensions, extensionless files, symlink
entries and special files are ignored. Selected files are still decoded by
contents; corrupt, animated or unsupported contents fail for that image. An
empty selection is an error. `--recursive` includes subdirectories without
following symlink entries. Batch mode requires directory output and rejects
`-o -`.

The complete mapping is checked before any output is written. Conflicts such as
`image.jpg` and `image.png` mapping to `image.webp`, file/directory conflicts,
and existing destinations that alias any selected input are errors. Name checks
conservatively fold case, even on case-sensitive filesystems. Existing symlinks
below the output root are refused, including destination symlinks. Root paths
are resolved to canonical directories. Filesystem changes during conversion
remain subject to the existing race limitations.

`--jobs` limits the number of complete decode/search/write pipelines in flight.
The default is **one worker**; choose additional workers according to available
memory. The existing per-image estimate is not a total batch memory limit:
several large images can multiply memory use. Discovery retains paths and
options; only active workers hold image/metric/candidate buffers.

Resize, threshold, search strategy, byte ceiling and size-based skip apply to
each image independently. Every selected image is attempted after individual
failures. Per-image reports go to stderr with source/destination paths, followed
by written/skipped/failed counts; verbose history is grouped per image. Parallel
success reports follow completion order; failures are listed in sorted source
order after processing. Stdout stays empty. Quiet suppresses success, skip and
count reports while keeping errors visible.

The batch exits `0` if every image was written or skipped, and `1` if any image
failed. Successful outputs remain in place; the batch has no rollback. A failed
or skipped image preserves its destination, but created subdirectories may
remain. Invalid arguments still exit `2`.

## How quality scoring works

1. Decode the source, apply EXIF orientation, convert supported color descriptions to sRGB, and tone map HDR when applicable.
2. Resize once if the scale multiplier or dimension bounds change the dimensions.
3. Treat **that processed image** as the quality reference.
4. Try several lossy WebP encoder qualities, decoding each trial from memory.
5. Score each decoded trial using SSIMULACRA2 against the processed reference.
6. Test a lossless WebP candidate as well.
7. Apply any byte ceiling or size-based skip, then write the smallest tested candidate whose score is at least the threshold.

The native SSIMULACRA2 scale is **not a percentage** and may yield negative scores for severe distortion. A score of `100` indicates identical imagery under the metric's comparison semantics; `90` is commonly considered visually lossless, around `80` very high quality, and `70` high quality. These are guidance, not guarantees that everyone will perceive images identically. See [SSIMULACRA2 reference](https://github.com/cloudinary/ssimulacra2).

**Important:** when downscaling, the threshold measures **encoding quality after resizing**, not the reduction in quality compared to the original full-size image. For example, `--scale 0.5 --threshold 90` can score 90 even though the image contains fewer pixels.

The optimization may choose lossless because it yields a smaller file (especially for low-complexity graphics) or because no tested lossy setting meets the requested quality.

The default `--search heuristic` tests lossy quality 0 and 100, runs up to seven
integer binary steps, and probes ±4 around the estimated boundary. Duplicate qualities are
cached. The default limit is 18 lossy evaluations plus one measured lossless
evaluation. It can miss feasible qualities or a smaller output outside its probes.

`--search exhaustive` evaluates every integer lossy quality from 0 through 100,
then lossless, for exactly 102 trials on a completed search. It guarantees the
smallest feasible output **within this fixed candidate set**, using the current
encoder settings and processed reference. Fractional qualities and other WebP
configurations are outside that guarantee. It performs more work than the
default heuristic; it does not stop early when a candidate passes.

Both modes compare actual byte counts; equal sizes prefer lossy, then lower
quality. Only the best candidate's bytes and numeric trial summaries are
retained. If no tested candidate meets the threshold, the command fails without
writing output. Exhaustive failure covers the complete fixed set; heuristic
failure leaves untested qualities unresolved. Encoding, decoding or scoring
errors abort either mode, without returning a partial winner.

## Behavior and limitations

- **Optimization scope:** WebP quality, SSIM scores, and output byte sizes are not strictly monotonic. The default heuristic can miss a smaller feasible candidate. Exhaustive mode guarantees the smallest feasible result only among the 101 integer lossy qualities and lossless with the current encoder settings, rather than across all possible WebP configurations.
- **Threshold cannot always be guaranteed:** even the native lossless path is decoded and scored. Alpha normalization and other conversion subtleties can affect the comparison. If nothing satisfies the threshold, the command fails rather than silently lowering the target.
- **Still images only:** input format is detected from contents, regardless of extension. Animated PNG (APNG) and WebP are rejected before decoding pixels. GIF and other formats are unsupported. JPEG/BMP decoders expose no animation detection; JPEG multi-picture extensions and concatenated images are not enumerated or guaranteed to be rejected.
- **Color and precision:** supported ICC/PNG descriptions become sRGB, with float preparation for color-managed/16-bit sources. Linear-sRGB Radiance HDR is tone mapped to SDR. Output remains 8-bit; wide gamut/HDR fidelity and linear-light resizing are unsupported. See [color preparation](#color-orientation-and-hdr) for precedence and format limits.
- **EXIF orientation:** supported JPEG/PNG/WebP orientation is applied before dimension bounds, resizing and scoring.
- **Image metadata:** source EXIF, XMP and ICC profiles are discarded after preparation; output pixels are sRGB.
- **Resizing:** `--scale` uses `max(1, round(dimension * scale))` on each axis. Dimension bounds use the smaller of 1, maximum width/source width and maximum height/source height (omitting unspecified bounds), with the same nearest-pixel rounding and one-pixel minimum. Both axes share one ratio; whole-pixel rounding can slightly change the exact aspect ratio, especially for tiny images. Output stays within every supplied bound. Lanczos3 runs once when dimensions change and is bypassed otherwise. Alpha is premultiplied during filtering and returned as straight alpha; hidden RGB does not tint visible edges. Fully opaque processed images use RGB8; others use RGBA8.
- **Source safety and overwrite policy:** canonical, symlink and hard-link aliases of the input are refused, with another check immediately before replacement. Existing destinations are replaced via a same-directory temporary file and rename; failures before replacement preserve the old destination and clean up staging files. An unrelated destination symlink is replaced as a directory entry, leaving its referent untouched. Dangling destination symlinks are rejected. Concurrent filesystem changes after validation cannot be fully prevented.
- **File permissions and durability:** the replacement is a new temporary file, so existing destination permissions/ownership/metadata are not preserved (on Unix, new outputs normally have mode `0600`). Contents are synced before replacement, but the containing directory is not synced; power-loss durability is not guaranteed. Abrupt process termination may leave a temporary file. Native Linux/macOS/Windows replacement tests and release smoke checks [passed in hosted CI](https://github.com/bdobrica/WebPTune/actions/runs/39074512319); this validates the tested runner filesystems.
- **Output failures:** write, flush, sync and rename errors return nonzero. Broken stdout pipes fail too; a partial stdout payload cannot be rolled back. Shell redirection creates/truncates its target before the program runs, so file preservation applies to `-o path`, not `-o - > path`. Keep the redirection target separate from the source.
- **Safety limits:** headers are checked before pixel decoding: 100 million source pixels, 16,383 pixels per output axis after scaling, and a 1 GiB estimated working-memory budget including metric padding, orientation/color and resize buffers. Untagged 8-bit 2000×2000 remains permitted at full scale; color-managed/16-bit/HDR sources can require tighter bounds. ICC parsing caps profile size at 4 MiB, CLUT size at 1 MiB and TRC length at 65,536; PNG metadata inspection has a 16 MiB decoder budget. Decoder limits are best effort. Headers/metadata can allocate earlier (the JPEG decoder buffers compressed input), and the estimate is not a measured peak-memory guarantee.
- **Corrupt input:** header/pixel decoder errors propagate as failures. Detection follows the underlying decoders; it is not an exhaustive container-integrity check, and permissive decoders can accept recoverable damage or trailing data.

## Exit behavior

A successful conversion exits `0` after writing the selected, decoded-and-scored
WebP, or after a requested size-based skip without writing. Help/version also
exit `0`. Invalid arguments exit `2`; path/input errors,
encoding/decoding failures, unmet thresholds, exceeded byte caps and output errors exit `1` with
details on stderr. Successful file output leaves stdout empty; successful stdout
output contains only WebP bytes unless skipped (empty stdout). Quiet mode keeps
errors visible. Input `-` is unsupported.

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
scaled dimensions, alpha and source preservation. It also exercises every tuning
flag with exact RGBA equality at threshold 100 and alpha quality 0, verifying the
lossless fallback and file/stdout equality.

[CI](.github/workflows/ci.yml) runs the commands above on push, pull request and
manual dispatch on all three target platforms. Each job uses a fresh checkout
and the repository toolchain, including native replacement tests. It validates
builds without publishing packages, tags or binaries. The full matrix
[passed on commit `16cfe64`](https://github.com/bdobrica/WebPTune/actions/runs/39074512319).

The suite covers argument and allocation limits, CLI process behavior, all five
input formats, rounded/tiny resizing, width/height bounds and option conflicts,
RGB/RGBA normalization, grayscale/16-bit
conversion, transparent edges, animation rejection, corrupt/oversized inputs,
all eight EXIF orientations in JPEG/PNG/WebP, ICC conversion and PNG color
precedence, independent Display P3/gamma/tone-map oracles, late 16-bit
quantization, source preservation and empty stdout on failure. Candidate tests
cover every integer lossy quality, lossless, alpha semantics, dimension/layout
validation and decode errors. Trial scores are verified through an independent
decode and one-shot metric path. Search tests cover nonmonotonic scores/sizes,
exact thresholds, lossless comparison, deduplication, budgets, ties and errors;
real-image selections are checked against every tested feasible candidate and
independently scored. Exhaustive tests include every isolated feasible quality,
nonmonotonic sizes/scores, ties, complete-set failure and evaluation errors. A
full-set oracle independently decodes/scores all 102 candidates for RGB, RGBA,
tiny and transparent inputs, checking selections at thresholds 0/80/100 and
12 RGB/RGBA CLI conversions through file/stdout, including dimension bounds.
Output tests compare file/stdout payloads with selected bytes, independently
decode/score written images, check stderr summaries and
verbose history, and cover staged replacement, cleanup, late collisions, partial
writes, injected disk-full/flush errors and a real broken stdout connection.
Size-policy tests cover inclusive caps, strict larger-only skips, failure
precedence, source/destination preservation, empty stdout on skips/failures,
both search modes, resized alpha, and quiet reporting with visible errors.
Batch tests check recursive and top-level discovery, all supported formats,
Unicode names, independent decoded quality/alpha, byte equality with single
conversions, collision preflight, failure continuation, per-image size policies,
source preservation and actual worker concurrency bounds. Release smoke also
checks recursive two-worker output against the verified single-image bytes.
Encoder tuning tests compare defaults with the original configuration and
nondefault trials with direct libwebp encoding, verify measured alpha loss and
exact hidden RGB, reject invalid CLI/library options, and check both search
modes, file/stdout equality, byte caps and two-worker batch results. The suite
also retains the Phase 0 API tests.

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
