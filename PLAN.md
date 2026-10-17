# Implementation Plan — WebP SSIM Optimizer

**Status:** Phases 0–4 implemented; phases 5–8 planned · **Target:** v0.1.0 · **Language:** Rust (edition 2021)

## 1. Purpose and scope

Build `webp-ssim-optimizer`, a dependable CLI that converts a supported still image to WebP, optionally resizes it, and selects a small valid WebP output whose **decoded pixels** achieve at least a user-defined SSIMULACRA2 score compared with the processed reference image.

The objective is **minimize encoded bytes subject to a measured quality floor**, not merely find the lowest encoder quality. Search is performed in memory; only the chosen output is written. It is a *heuristic optimizer*: v0.1 does not promise a globally minimal WebP across every possible encoder configuration.

### Goals

1. Accept common still-image formats (JPEG, PNG, WebP, BMP) and convert to still WebP.
2. Support `--threshold` as a native SSIMULACRA2 score (default `80`, valid `0..=100`).
3. Support `--scale` (default `1.0`, valid `0 < scale <= 1.0`) with Lanczos3 resizing.
4. Preserve alpha when present, with alpha-aware quality comparisons.
5. Search lossy quality configurations in memory, then consider lossless WebP.
6. Select the smallest **tested, feasible** encoded candidate.
7. Support `-o -` for binary stdout and reliable file output.
8. Provide useful reporting on stderr without contaminating stdout.
9. Exit with meaningful failure codes, never silently produce nonconforming output.
10. Ship tests, CI, repeatable build instructions, and reproducible fixtures.

### Non-goals for v0.1

Animated images, batch/directory processing, upscaling, metadata/ICC profile preservation, EXIF orientation preservation beyond explicitly tested decoder behavior, HDR/16-bit fidelity, arbitrary WebP encoder parameter sweeps, global-optimum guarantees, streaming input, and an image-comparison GUI. Unsupported cases must be documented or rejected, not quietly mishandled.

## 2. Correctness decisions

### 2.1 Score semantics

SSIMULACRA2 normally returns scores up to `100` (identical decoded imagery); heavily distorted output may score **below zero**. It is *not* a percentage. The CLI threshold is a native score: `--threshold 80` means `score >= 80.0`. Do **not** multiply by 100. Scores at 70/80/90 should be described as approximate perceptual guidance, not visibility guarantees.

The score always compares **the encoded-and-decoded candidate against the processed (resized) source**. Thus `--scale 0.5 --threshold 90` does not measure fidelity to the original full-size image.

### 2.2 Minimum-size objective

Encoder quality does not imply a strictly monotonic result in **file size** or SSIM score. Binary search finds a practical neighborhood quickly; it is not a proof. Record the `(mode, quality, size, score, bytes)` of each tested candidate. A candidate is *feasible* only if decoded successfully and its score is at least the threshold. Choose the smallest feasible candidate, with deterministic tie breaks (prefer lossy, then lower quality). Compare with lossless even if a lossy candidate already meets the threshold: lossless is sometimes smaller for icons and simple art.

For v0.1, test integer lossy qualities `0..=100`: quality 0 and 100 are valid libwebp inputs. Search for a feasible region via binary search, then probe a small set around its boundary (e.g. ±4 quality points), plus endpoints. Keep a configurable internal evaluation budget; deduplicate qualities. This does **not** guarantee the smallest feasible quality or global minimum. A future `--search exhaustive` mode can evaluate all 101 qualities.

### 2.3 Lossless and alpha handling

Use RGB only for fully opaque images; use RGBA when any pixel has transparency. SSIMULACRA2 can accept RGBA input and evaluates transparency against different backgrounds. Build reference and decoded-candidate pixel views using the *same* dimensions and correctly specified pixel descriptors; score the actual alpha-bearing output, not a silently flattened RGB version.

Lossless WebP preserves displayed pixels, but may normalize RGB values hidden under fully transparent pixels. Therefore **do not assert bytewise RGBA identity or automatically assume score 100**. Decode and score the lossless candidate just like every other candidate. For threshold 100, accept only candidates whose measured score satisfies the threshold (allow no undocumented epsilon). If no candidate is feasible, fail clearly and do not create an output. Test this edge case with transparent pixel fixtures.

### 2.4 Color and metadata

Initial implementation targets 8-bit, SDR, sRGB-like raster pixels. Decoding or converting wide-gamut images without explicit color management can change appearance. Document that v0.1 does not preserve EXIF, XMP, ICC, animation, or 16-bit source precision; do not advertise fully faithful conversion. Define EXIF orientation handling explicitly: either implement orientation before scaling and scoring, or reject/clearly document inputs where orientation is not applied. For initial simplicity, **do not promise automatic EXIF autorotation**; track it as a release limitation.

## 3. Suggested architecture

```text
webp-ssim-optimizer/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── PLAN.md
├── TODO.md
├── LICENSE                  # choose and add deliberately
├── .gitignore
├── .github/workflows/ci.yml
├── src/
│   ├── main.rs              # parse CLI, exit codes, stderr
│   ├── cli.rs               # clap input model and validation
│   ├── limits.rs            # pixel, memory and checked buffer arithmetic
│   ├── input.rs             # decode, limits, resize, RGBA/RGB
│   ├── metric.rs            # reusable reference and scoring
│   ├── encoder.rs           # lossy/lossless WebP candidates
│   ├── search.rs            # bounded search and selection
│   ├── output.rs            # stdout/file and atomic write
│   └── lib.rs               # testable orchestration/API
├── tests/
│   ├── cli.rs
│   ├── integration.rs
│   └── fixtures/            # generated images + provenance
└── benches/                 # later, optional
```

Keep modules reasonably small; merge files if the API is simpler than expected. Use typed, testable boundaries instead of putting logic in `main.rs`.

### Data types (conceptual)

```rust
struct Options {
    input: PathBuf,
    output: OutputTarget,
    threshold: f64,
    scale: f64,
    verbose: bool,
}

enum OutputTarget { Stdout, File(PathBuf) }
enum Mode { Lossy { quality: u8 }, Lossless }

struct Candidate {
    mode: Mode,
    score: f64,
    bytes: Vec<u8>,
}

struct Report {
    original_dimensions: (u32, u32),
    output_dimensions: (u32, u32),
    selected_mode: Mode,
    measured_score: f64,
    output_bytes: usize,
    evaluated_candidates: usize,
}
```

Do not retain every candidate's byte buffer indefinitely. Retain the current best and numeric trial summaries; discard inferior bytes after evaluation. Avoid duplicating full images when unnecessary.

## 4. CLI contract

```text
webp-ssim-optimizer \
  --input INPUT \
  --output OUTPUT|- \
  [--threshold 80] \
  [--scale 1.0] \
  [--verbose]
```

Short flags: `-i`, `-o`, `-t`, `-s`, `-v`. Input and output are required for v0.1. `--help` and `--version` are provided by clap.

Validation:

- `threshold`: finite numeric value `0..=100`; reject NaN, infinities, and out-of-range values.
- `scale`: finite numeric value `(0,1]`; reject zero, negative, >1, NaN, infinity.
- `input`: exists, readable, supported format, valid decoded dimensions.
- `output`: path or literal `-`. Refuse to overwrite the input file, including obvious canonical-path aliases; use a more robust same-file check if feasible.
- Reject inconsistent or dangerous dimensions. Compute scaled dimensions with checked arithmetic, `max(1, round(original * scale))` on each axis; validate results against encoder and application limits.
- Reject animated inputs in v0.1 rather than silently processing only frame 1 (where reliable animation detection is available).

Terminal behavior:

- File output: write report to stderr if verbose; brief success summary to stderr by default is acceptable.
- Stdout: **only WebP bytes** on stdout. Help and errors are never mixed with image payload.
- Exit `0` on success, nonzero on invalid arguments, unreadable input, failed encoder/decoder/metric, no feasible candidate, or write errors. Define stable symbolic error categories; exact numeric codes can be assigned in implementation.

## 5. Dependencies and toolchain

Current dependency configuration (exact resolutions are recorded in `Cargo.lock`):

```toml
[package]
name = "webp-ssim-optimizer"
version = "0.1.0"
edition = "2021"
rust-version = "1.89"

[dependencies]
clap = { version = "~4.5", features = ["derive"] }
image = { version = "0.25", default-features = false, features = ["jpeg", "png", "webp", "bmp"] }
same-file = "1"
webp = { version = "0.3", default-features = false }
fast-ssim2 = "0.9"
anyhow = "1"

[dev-dependencies]
png = "0.18"
tempfile = "3"
```

Phase 0 uses `anyhow` for prototype and CLI errors; typed library errors can be introduced with the application pipeline. Clap stays in the 4.5 family. Disabling `webp`'s default `img` feature avoids enabling additional `image` codecs transitively; the spike uses raw RGB/RGBA buffers. The MSRV and pinned development toolchain are Rust 1.89. CI should eventually test that version and stable.

Phase 1 adds `same-file` for portable file-identity checks (including hard links)
and `tempfile` for isolated filesystem integration tests.
Phase 2 uses `png` as a development dependency to generate real APNG fixtures.

API checkpoints:

- `webp::Encoder::from_rgb`, `from_rgba`, `encode_simple(false, quality)`, `encode_simple(true, quality)` / `encode_advanced` return a fallible result when chosen appropriately.
- `fast_ssim2::{PixelSlice, PixelDescriptor, Ssimulacra2Reference}`: create a reference once, then `reference.compare(&candidate)` with matching descriptors.
- `image` library: decode WebP candidates in memory and convert to exact width/height RGB or RGBA bytes. Check pixel buffers, no accidental implicit alpha drop.

Avoid calling `webp::Encoder::encode` or `encode_lossless` directly when their internals can panic; prefer checked/fallible APIs. Confirm configuration semantics in the installed version during implementation.

## 6. Processing pipeline

### Phase A — Parse and load

1. Parse CLI and validate finite ranges before doing I/O.
2. Resolve input/output collision risk; read header/dimensions and enforce the **100 million source pixels maximum** and **1 GiB estimated working-memory budget** established in section 13.
3. Decode exactly one supported still image. Report unsupported animation/color cases; reject malformed and truncated data.
4. Optionally resize once with Lanczos3. Handle small images: both resulting dimensions must be >=1.
5. Normalize to owned `RgbImage` if opaque, otherwise `RgbaImage`. Keep reference pixel memory alive.

### Phase B — Prepare reference

6. Construct a `PixelSlice` with accurate stride, 8-bit sRGB descriptor, width and height.
7. Precompute `Ssimulacra2Reference` **once**; reuse for all candidate comparisons.
8. For alpha sources, use the corresponding RGBA descriptor. Verify alpha scoring semantics against test fixtures.

### Phase C — Trial evaluation

9. Create one `webp::Encoder` over the normalized input buffer.
10. For each requested lossy quality `q`: encode into memory (fallible API); verify nonempty output, decode the WebP from memory, validate decoded dimensions, materialize matching pixel layout, and score with the precomputed reference. Reject non-finite scores or errors; do not treat failures as quality 0.
11. Record quality, decoded score and compressed size. If `score >= threshold`, consider for best candidate based primarily on byte size.
12. Deduplicate repeated `q` values to avoid wasted work.

### Phase D — Search and fallback

13. Always test lossy endpoints `0` and `100` (or test 100 before midpoint searches to bracket feasibility). Do not presume score monotonicity.
14. Run bounded integer binary search as a heuristic for the quality boundary. Keep every feasible evaluated candidate, independent of its position in the sequence.
15. Evaluate adjacent qualities within a limited ±4 window, and compare results by actual encoded bytes; optionally probe evenly spaced qualities if the metric is clearly nonmonotonic.
16. Always evaluate a lossless candidate and decode/score it. The lossless candidate may be smaller than any lossy candidate.
17. Choose the smallest feasible evaluated candidate using deterministic tie breaks. If none meet the threshold, return a `ThresholdUnachievable` error (without writing any image). The lossless candidate is *not* an unconditional guarantee under all pixel/alpha interpretations.

### Phase E — Output

18. Prepare final payload entirely before output.
19. For file output, write into a temporary file **in the destination directory**, flush/sync as appropriate and rename atomically where supported. Handle existing-file/Windows semantics deliberately; clean up temporary files on error.
20. For `-o -`, lock stdout and `write_all` encoded bytes; report errors to stderr and return nonzero. An interrupted/partial stdout write cannot be rolled back.
21. Report selected mode, quality if lossy, score, dimensions, bytes and evaluation count to stderr, especially when verbose.

## 7. Verification and test strategy

### Unit tests

- Threshold and scale validation: valid boundaries; reject NaN/infinity/outside range.
- Dimension scaling: odd numbers, tiny images, overflow and bounds.
- Candidate comparison: smallest bytes wins, score exactly at threshold passes, stable tie breaks.
- Candidate deduplication and bounded evaluation budget.
- No-candidate behavior; errors propagate without generating output.
- Output-target parser and stdout-safe formatting.

### Integration fixtures

Create deterministic small fixtures: photographic JPEG, RGB PNG, RGBA PNG (including fully transparent colored pixels and semitransparency), flat icon/line art, 1×1 image, small dimensions, BMP, corrupted file, oversized-header fixture, and optional WebP input. Keep fixture licenses/provenance.

For each successful encode:

- WebP magic/format is valid and decodable.
- Output dimensions follow scaling contract.
- Recomputing the SSIMULACRA2 score from the processed reference meets the specified threshold.
- Alpha is present when needed and visibility behavior is maintained.
- Selected file size is <= each **other tested feasible** candidate's size.
- Original input remains unchanged.
- stdout output equals file output bytes for same options; stdout contains no log text.

Failure tests: unsupported input, malformed image, unachievable threshold, unwritable destination, same source/destination, invalid paths, broken pipe where practical, and restricted-memory/oversized image handling.

### Property/regression tests

Randomly generate bounded 8-bit RGB/RGBA images; assert nonpanicking behavior, valid WebP decoding, score constraints, and candidate best-selection invariants. Keep a regression corpus for pixel-format and alpha problems. Avoid brittle assertions that a particular image always yields a specific quality value across codec versions.

### Benchmarking

Measure total wall time, peak RSS, number of trial encodes, encoding time, decoding time, score time, output bytes and quality, across photographic and graphic samples and multiple dimensions. Compare release build with and without `RUSTFLAGS="-C target-cpu=native"`; native CPU tuning is **optional for machine-specific deployments** and should not be required for distributable binaries. `fast-ssim2` has runtime SIMD dispatch.

### CI quality gates

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --all-targets`
- `cargo build --release`
- Smoke test both output file and binary stdout.
- Linux, macOS, Windows (CI matrix, subject to available runners).
- Security/dependency review and optional `cargo audit`.

## 8. Milestones and acceptance criteria

| Milestone | Deliverable | Acceptance criterion |
|---|---|---|
| M0 | Dependency/API spike | Current crate APIs compile and round-trip RGB/RGBA images |
| M1 | CLI + ingestion | Valid arguments, supported input, resize, safeguards |
| M2 | Metric + encoder | Every evaluated candidate is decoded/scored correctly |
| M3 | Optimizer | Bound-aware search, local probes, lossless comparison, smallest tested feasible bytes |
| M4 | Reliable output | Stdout clean, file outputs safe, failures don't clobber source |
| M5 | Release readiness | Fixtures, CI, benchmark notes, README, license, reproducible build |

**Definition of done:** all integration requirements pass; quality constraint verified on the *written* bytes; any impossible threshold exits nonzero; `README.md` accurately reflects supported formats and limitations; CI passes release-mode smoke tests.

## 9. Risks and mitigation

| Risk | Impact | Mitigation |
|---|---|---|
| Nonmonotonic quality/size | Binary search skips superior candidates | Treat search as heuristic, local probes, smallest tested bytes, optional future exhaustive mode |
| Transparent-pixel differences | Lossless fallback doesn't satisfy score | Alpha-aware metric, decoded lossless validation, clear error |
| Color profile / orientation loss | Visual discrepancy vs viewer | Scope SDR 8-bit, document behavior, add dedicated support later |
| Excessive allocations | Out-of-memory crash | Header limits, checked arithmetic, bounded buffers and workloads |
| Encoder panic or invalid data | CLI crash | Checked encoding API, error handling, malformed-file tests |
| Decoder and encoder version drift | Build failure or changed output | Cargo.lock, API spike, CI, stable test tolerances |
| Cross-platform file replacement | Data loss / unexpected behavior | Temp file in same directory, safe overwrite policy, Windows tests |
| Tool creates larger output | Surprising users | Report original/output size; optional future `--skip-if-larger` (not in v0.1) |

## 10. Post-v0.1 roadmap (optional)

1. `--search fast|thorough|exhaustive`; quality/timing tradeoff with explicit global minimum across the *fixed discrete candidate set* for exhaustive search.
2. `--max-bytes`, `--skip-if-larger`, quiet/JSON reports, and stdin input.
3. Batch processing and parallel files, with explicit memory/concurrency caps.
4. Preserve ICC/EXIF orientation and add color-managed / 16-bit paths.
5. Advanced WebP parameters and alpha-quality tuning; support animation as a separate workflow.
6. Standalone performance benchmark dataset and published platform-specific binaries.

## 11. References

- `fast-ssim2` current API and alpha behavior: https://docs.rs/fast-ssim2/latest/fast_ssim2/
- `webp` Encoder API: https://docs.rs/webp/latest/webp/struct.Encoder.html
- Original SSIMULACRA2 interpretation: https://github.com/cloudinary/ssimulacra2
- Image crate: https://docs.rs/image/latest/image/

These docs must be rechecked before any dependency upgrade or API change.

## 12. Phase 0 API validation findings

The disposable probe is `examples/api_spike.rs`; run it with
`cargo run --locked --example api_spike`. Its checks also run under
`cargo test --all-targets --locked`. All fixtures are deterministic synthetic
pixels generated by the example; no external images or fixture licenses are needed.

### Encoding and decoding

- The locked versions are Clap 4.5.61, image 0.25.10, webp 0.3.1 and
  fast-ssim2 0.9.0. The package uses edition 2021 and Rust 1.89.0.
- `Encoder::from_rgb` and `from_rgba` borrow packed bytes.
  `encode_simple(lossless, quality)` returns `Result<WebPMemory, WebPEncodingError>`.
  The error enum requires debug formatting to wrap it in `anyhow`.
  The spike decodes each candidate using
  `image::load_from_memory_with_format(..., ImageFormat::WebP)`.
- The spike exercises lossy qualities 0, 75 and 100, and lossless at quality 75
  (quality controls compression effort in lossless mode). It verifies dimensions,
  nonempty output, preserved alpha samples and finite measured scores.
- Avoid `encode` and `encode_lossless`: they unwrap the fallible result. Even
  the fallible API is not a universal panic guarantee: constructors panic for
  undersized pixel buffers and the wrapper unwraps native initialization. Validate
  dimensions and buffer lengths before constructing encoders in the later pipeline.
- `webp`'s default `img` feature is disabled. `cargo tree -e features -i image`
  confirms only `jpeg`, `png`, `webp` and `bmp` are enabled. `libwebp-sys` compiles
  bundled C sources with `cc`; a C compiler and archiver are required, rather than
  a separately installed shared libwebp.

See the [webp encoder API](https://docs.rs/webp/0.3.1/webp/struct.Encoder.html)
and [libwebp API](https://developers.google.com/speed/webp/docs/api).

### Metric and alpha

- `PixelSlice::new(bytes, width, height, stride_bytes, descriptor)` uses a
  **byte stride per row**: packed RGB8 is `width * 3`, RGBA8 is `width * 4`.
  Descriptors are `PixelDescriptor::RGB8_SRGB` and `RGBA8_SRGB` (straight alpha).
- Build `Ssimulacra2Reference::new(&source)` once and reuse `compare(&candidate)`.
  Identical RGB and RGBA fixtures score exactly 100. Visible RGB distortion,
  partially transparent RGB changes and alpha changes lower the score. RGB
  changes confined to alpha=0 score 100, including fully transparent images.
- Alpha comparisons composite over sRGB backgrounds 0.1 and 0.9 and take the
  lower score. Lossless WebP may change hidden RGB because `exact` defaults to 0;
  inspect the decoded score instead of requiring bytewise RGBA identity.
- Tiny images below 8×8 are reflect-padded by the metric. Its pixel limit is
  268,435,456 including padding; allocations still require a smaller application
  memory budget in Phase 1.

See [fast-ssim2 input semantics and limits](https://docs.rs/fast-ssim2/0.9.0/fast_ssim2/).

### Dimensions and animation

- libwebp's encoder header defines `WEBP_MAX_DIMENSION = 16383`. The probe
  verifies 16383×1 and 1×16383 succeed in both modes, and 16384×1 and 1×16384
  return `VP8_ENC_ERROR_BAD_DIMENSION`. It also round-trips and scores 1×1
  RGB and partially transparent RGBA. This codec limit is separate from the
  application's future pixel/memory limits.
- `image::codecs::webp::WebPDecoder::has_animation()` detects animation before
  decoding pixels. The probe checks a generated two-frame WebP and still outputs.
- `image::codecs::png::PngDecoder::is_apng()` returns `ImageResult<bool>` based on
  the PNG animation-control chunk. The API is compiled and checked against a
  generated still PNG; an animated PNG fixture remains for Phase 2.
- The later input pipeline must invoke these checks explicitly: decoding a
  `DynamicImage` alone does not establish a still-image-only policy. JPEG and BMP
  have no animation API in these decoders. GIF is outside the enabled formats.
  Malformed containers must propagate decoder errors rather than be treated as
  still images. Phase 0 provides probes, not application input rejection yet.

See the [WebP decoder](https://docs.rs/image/0.25.10/image/codecs/webp/struct.WebPDecoder.html)
and [PNG decoder](https://docs.rs/image/0.25.10/image/codecs/png/struct.PngDecoder.html).

## 13. Phase 1 CLI and validation decisions

`src/cli.rs` defines the testable Clap `Args` and typed `OutputTarget`.
Input/output are required; only the exact output value `-` selects stdout.
Paths retain native OS strings, including non-UTF-8 names on Unix. Threshold
and scale are parsed as `f64`, checked for finiteness, and constrained to
`[0,100]` and `(0,1]` respectively. Using `f64` avoids prematurely rounding tiny
positive scales to zero; dimension rounding remains Phase 2 work.

Path validation opens the existing regular input read-only and checks canonical
paths and `same-file` handle identity against existing regular destinations.
This catches relative, symlink and hard-link aliases. Dangling output symlinks
are rejected; a missing destination requires an existing parent directory. No
output or parent is created or truncated. Existing output identity must be
readable for the conservative collision check. Filesystem changes after this
check remain possible; Phase 5 must revalidate and use safe replacement.
Literal stdin input `-` is rejected (use `./-` for a file named `-`).

`src/limits.rs` establishes reusable, allocation-free checks:

- Nonzero dimensions and at most **100,000,000 source pixels**, even when scaling.
- Checked row-stride and buffer-length arithmetic, including `usize` conversion
  and the platform's `isize::MAX` allocation boundary.
- At most **16,383 pixels per output axis**, applied after scaling so larger
  sources can be downscaled.
- At most **1,073,741,824 estimated bytes** (1 GiB), using
  `8 * source_pixels + 256 * max(output_width, 8) * max(output_height, 8)`.
  Eight bytes per source pixel allows for 16-bit RGBA decode. The processing
  allowance reserves space for normalized/candidate pixels, both alpha
  backgrounds, cached metric pyramids, scratch and encoder buffers. Metric
  reflection padding is included. For example, 2000×2000 without scaling fits;
  2048×2048 requires a smaller scale under this policy.

The memory estimate is an initial conservative policy, **not a measured peak-RSS
guarantee** or an OS allocation cap. Phase 2 must invoke the source guard before
large decode allocations and the combined guard before resize/metric allocation,
and configure decoder allocation limits. Phase 7 will measure and refine it.
The guard functions are tested now; Phase 1 does not decode image headers.

Clap help/version exit 0; argument errors exit 2. Path errors and the explicit
pending-conversion error exit 1, with empty stdout and diagnostics on stderr.
Valid requests intentionally fail until Phases 2–5 supply the image pipeline.
Integration tests prove no output is created and existing files are preserved
for validation failures and pending-pipeline requests. Unix symlink/non-UTF-8
cases are conditional tests; macOS/Windows execution remains a Phase 7 task.

## 14. Phase 2 image preparation decisions

`input::load(path, scale)` now returns an owned `PreparedImage`, preserving source
dimensions alongside typed packed `Pixels::Rgb` or `Pixels::Rgba` reference data.
The CLI invokes it after path validation. It then reports prepared dimensions and
layout in a pending-optimization error; phases 3–5 still supply scoring, search
and output. No destination is created or opened for writing during preparation.

### Decode and validation

Format detection uses file contents, with no extension fallback. JPEG, PNG,
WebP and BMP have explicit decoder branches. PNG's `is_apng` and WebP's
`has_animation` reject animation before pixel decode. The corpus includes real
two-frame APNG/WebP files, and tests independently enumerate both frames. GIF and
other formats are rejected. JPEG/BMP have no animation API in these decoders;
multi-picture JPEG extensions and concatenated rasters are not enumerated, so
their rejection is not guaranteed.

Header dimensions are checked against the source cap, rounded output codec
limits and combined memory budget before `DynamicImage::from_decoder` allocates
pixels. Checked buffer sizes are compared with the decoder's advertised byte
count. PNG receives allocation/dimension limits at construction, and all decoders
receive limits before pixel decode. Header and pixel errors propagate with
context. These are decoder-based checks, not exhaustive container validation:
permissive decoders may accept recoverable corruption or trailing bytes. Header
parsing/metadata can allocate before application dimension checks; notably the
JPEG decoder reads compressed input into memory in its constructor. Decoder
allocation limits are best effort and do not impose a process memory cap.

### Scale, alpha and color

Each axis uses `max(1, round(dimension * scale))`, computed in `f64` with checked
conversion to `u32`. This gives 17x11 -> 9x6 at scale 0.5, accepts arbitrarily
small positive representable scales and never produces a zero axis. Codec limits
apply after scaling; the 20000x1 PNG fixture can be downscaled to 10000x1.

Lanczos3 runs at most once using exact rounded dimensions. Scale 1 and scales
whose rounding leaves dimensions unchanged bypass it. Alpha-bearing buffers are
converted to float RGBA and premultiplied before filtering, then unpremultiplied
back to straight alpha. RGB hidden under alpha=0 therefore cannot tint visible
edges. Filtering uses existing sRGB-like samples, not a linear-light transform.
After resizing, normalization produces 8-bit RGB for fully opaque processed
pixels and 8-bit RGBA when any processed alpha is below 255. Grayscale expands
to RGB/RGBA; 16-bit PNG is reduced to 8 bits. Without resizing, 8-bit RGBA retains
hidden colors. Resizing may normalize invisible colors.

ICC profiles, EXIF orientation and metadata are not applied or preserved.
The EXIF orientation-6 fixture proves stored raster orientation is retained.
There is no color-managed, wide-gamut, HDR or 16-bit-fidelity pathway in v0.1.

### Resize memory refinement and fixtures

`validate_resize_dimensions` extends section 13's base estimate whenever
dimensions change. It adds `16 * source_width * output_height` for the image
crate's float intermediate, `8 * max(source_width, source_height)` for coefficient
vectors including growth, and `16 * source_pixels` when alpha needs a float
source. All large buffer arithmetic is checked before decoding. This closes the
case where a very wide source has a small output but a large intermediate; some
images passing the base guard now require a smaller source or scale. The 1 GiB
budget remains an initial conservative estimate pending Phase 7 measurements.

`examples/generate_fixtures.rs` reproducibly creates the committed synthetic
corpus. `tests/fixtures/README.md` records provenance and regeneration. Fixtures
cover supported still formats, transparency/opaque alpha, tiny images, grayscale,
16-bit RGBA, animated inputs, orientation, corrupt/unsupported input and oversized
headers. Header-only BMP tests demonstrate limits fail before absent pixels are
read. Tests cover source/destination preservation and empty stdout at the CLI
boundary. Cross-platform execution remains Phase 7 work.

## 15. Phase 3 metric and trial evaluation

`metric::MetricReference::new(&Pixels)` validates processing dimensions and
packed buffer length, builds an RGB8_SRGB or straight-alpha RGBA8_SRGB
`PixelSlice` with a checked **byte stride per row**, then constructs one
`Ssimulacra2Reference`. Its `score` method checks candidate dimensions and layout
before comparison. Native scores are neither scaled nor clamped; NaN and
infinity are errors. Repeated comparisons reuse the cached reference.

`encoder::TrialEvaluator` borrows the immutable processed pixels and owns one
metric reference. `evaluate(Mode)` accepts every integer lossy quality 0–100 or
lossless (default effort 75). It initializes `WebPConfig` fallibly and uses
`encode_advanced`, with the same lossless/alpha-compression settings as
`encode_simple`. Dimensions and exact buffer lengths are checked before entering
the encoder. Returned encoder, decoder and metric errors propagate with context;
they are never converted to a score. The upstream wrapper still unwraps native
picture initialization internally, as identified in Phase 0; this is not a
guarantee against allocation failure or every upstream panic.

Every trial, including lossless, is decoded and measured. `score_webp` rejects
empty bytes, unreadable headers, animation and wrong dimensions before pixel
allocation; it applies decoder limits and propagates pixel decode failures.
Decoded pixels use the reference layout. An RGB candidate compared with an RGBA
reference receives its actual opaque alpha, never the reference's alpha. An
RGBA candidate for an RGB reference must be fully opaque before RGB conversion;
transparency cannot be silently discarded. Lossless hidden RGB normalization
is permitted, with its actual score retained.

`Trial` exposes mode, measured score, size (derived from bytes) and owned encoded
bytes. It makes no threshold or ordering decision. The evaluator writes no files
and retains no collection of past trials. Phase 3 exercised lossy quality 75 and
lossless as two CLI pipeline checks. Phase 4 replaces those checks with bounded
search and selection, retaining only the best bytes and numeric summaries.
Output writing remains Phase 5.

Integration tests independently decode trial bytes with the image convenience
API, construct separate pixel views and call the one-shot
`fast_ssim2::compute_ssimulacra2`, bypassing application scoring/caching helpers.
Scores match exactly for JPEG/PNG/BMP/WebP sources, RGB/RGBA, scale 1 and 0.5,
tiny and fully transparent images, qualities 0/75/100 and lossless. All 101 lossy
qualities are exercised on RGB and RGBA. Identity scores are exactly 100 for the
tested opaque and alpha fixtures; hidden RGB changes score 100, while partial
RGB and alpha changes lower the score. Tests also cover invalid quality/layout,
dimension mismatch, non-finite scores, empty/corrupt/truncated/animated candidate
bytes, source/destination preservation and CLI stdout hygiene. No new
dependencies are required.

## 16. Phase 4 search and selection

`search::Candidate` reuses the evaluator's measured `Trial` type. Its private
fields ensure public callers cannot manufacture unmeasured candidate bytes.
`TrialSummary` records mode, measured score and actual byte count without keeping
the encoded buffer. Feasibility is exactly `score >= threshold`, with no epsilon.
Among feasible trials the ordering is byte count first, then lossy before
lossless, then lower integer quality; a higher score does not override that
ordering. `SearchResult` owns the best candidate and summaries in evaluation
order. Inferior bytes are discarded immediately.

`search::search` uses one existing `TrialEvaluator` and its cached metric
reference. It evaluates lossy 0 and 100 first, then runs at most seven integer
binary-search steps within 1–99 when quality 0 fails. Quality 100 failing does
not skip this search: lower qualities can still pass on a nonmonotonic curve.
The lowest passing binary trial anchors a ±4 neighborhood; if none passed, the
anchor is 100. If quality 0 passed, the anchor is 0. Neighborhoods are clipped
to 0–100. A fixed 101-entry cache prevents duplicate encoding or scoring,
including when endpoints or binary trials overlap the probes.

`SearchOptions::max_lossy_trials` defaults to 18: two endpoints, up to seven
binary steps and up to nine neighborhood points before deduplication. The
library's `search_with_options` accepts budgets 2–101; smaller budgets can
truncate binary steps or probes. A larger budget does not turn the heuristic
into exhaustive search. Lossless receives a separate final evaluation even if
the lossy budget is exhausted or a lossy trial passed. It is decoded and scored
by the same evaluator and competes using the same ordering, never assumed to
score 100. Default total evaluations are at most 19.

Every feasible trial competes for selection regardless of its position in the
search. Binary decisions only choose where to look next; they do not prove
anything about untested qualities or sizes. A test explicitly demonstrates a
feasible quality outside the probes being missed. If no tested trial passes,
the typed `ThresholdUnachievable` error includes threshold, best measured score
and trial count, and states that untested feasible qualities may exist. Invalid
thresholds or budgets fail before evaluation. Encoder/decoder/metric errors
abort with mode context and their original cause, rather than becoming failed
quality scores; an earlier evaluation error also aborts before later trials.

The CLI now applies `--threshold` during search, prints numeric trial history
with `--verbose`, and reports the selected mode, size, measured score, prepared
dimensions and trial count on stderr in a pending-output error. It still exits
nonzero and writes nothing until Phase 5 implements output. Input and existing
destinations are preserved for both file and stdout targets.

Unit tests cover exact threshold equality, negative scores, lossless being
smaller, no feasible candidate, nonmonotonic curves, all 101 monotonic boundary
positions, clipped neighborhoods, deduplication, budgets 2–101, deterministic
ties, invalid options and evaluator errors. Real RGB/RGBA, tiny and transparent
fixtures at scales 1 and 0.5 and thresholds 0/80/90/100 verify selected bytes
through independent decoding and the one-shot metric. Re-evaluation verifies
every numeric summary; selection is compared against every tested feasible
trial, including ties. CLI tests verify verbose history, threshold application,
empty stdout and unchanged source/destination files. No dependencies were added.
