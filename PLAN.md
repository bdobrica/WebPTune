# Implementation Plan — webptune

**Status:** Phases 0–6 implemented; Phase 7 local performance/robustness validated, macOS/Windows checks pending; Phase 8 documentation/CI implemented, hosted validation and tag pending; Phase 9 maximum-dimension resizing implemented · **Target:** v0.1.0 · **Language:** Rust (edition 2021)

## 1. Purpose and scope

Build `webptune`, a dependable CLI that converts a supported still image to WebP, optionally resizes it, and selects a small valid WebP output whose **decoded pixels** achieve at least a user-defined SSIMULACRA2 score compared with the processed reference image.

The objective is **minimize encoded bytes subject to a measured quality floor**, not merely find the lowest encoder quality. Search is performed in memory; only the chosen output is written. It is a *heuristic optimizer*: v0.1 does not promise a globally minimal WebP across every possible encoder configuration.

### Goals

1. Accept common still-image formats (JPEG, PNG, WebP, BMP) and convert to still WebP.
2. Support `--threshold` as a native SSIMULACRA2 score (default `80`, valid `0..=100`).
3. Support `--scale` (default `1.0`, valid `0 < scale <= 1.0`) with Lanczos3 resizing. Phase 9 adds maximum width/height bounds as alternatives.
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
webptune/
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
    scale_width: Option<u32>,
    scale_height: Option<u32>,
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
webptune \
  --input INPUT \
  --output OUTPUT|- \
  [--threshold 80] \
  [--scale 1.0 | --scale-width PIXELS [--scale-height PIXELS] | --scale-height PIXELS] \
  [--verbose]
```

Short flags: `-i`, `-o`, `-t`, `-s`, `-v`. Input and output are required for v0.1. `--help` and `--version` are provided by clap.

Validation:

- `threshold`: finite numeric value `0..=100`; reject NaN, infinities, and out-of-range values.
- `scale`: finite numeric value `(0,1]`; reject zero, negative, >1, NaN, infinity.
- `scale-width`/`scale-height`: positive `u32` maximum dimensions; either or both may be supplied. Reject combination with an explicit `--scale`, including `-s 1`. Fit to the tightest bound without upscaling, preserving aspect ratio with nearest-pixel rounding (minimum one pixel per axis). See section 21 for implementation details.
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
name = "webptune"
version = "0.1.0"
edition = "2021"
rust-version = "1.89"

[dependencies]
clap = { version = "~4.5", features = ["derive"] }
image = { version = "0.25", default-features = false, features = ["jpeg", "png", "webp", "bmp"] }
same-file = "1"
tempfile = "3"
webp = { version = "0.3", default-features = false }
fast-ssim2 = "0.9"
anyhow = "1"

[dev-dependencies]
png = "0.18"
```

Phase 0 uses `anyhow` for prototype and CLI errors; typed library errors can be introduced with the application pipeline. Clap stays in the 4.5 family. Disabling `webp`'s default `img` feature avoids enabling additional `image` codecs transitively; the spike uses raw RGB/RGBA buffers. The MSRV and pinned development toolchain are Rust 1.89. CI should eventually test that version and stable.

Phase 1 adds `same-file` for portable file-identity checks (including hard links)
and `tempfile` for isolated filesystem integration tests.
Phase 2 uses `png` as a development dependency to generate real APNG fixtures.
Phase 5 promotes the existing `tempfile` dependency to production for staged output.

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
check remain possible; Phase 5 revalidates and uses staged replacement.
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

Clap help/version exit 0; argument errors exit 2. Path errors exit 1, with empty
stdout and diagnostics on stderr. Phase 1 used an explicit pending-conversion
error for valid requests; phases 2–5 now supply the complete image pipeline.
Integration tests prove no output is created and existing files are preserved
for validation failures. Unix symlink/non-UTF-8
cases are conditional tests; macOS/Windows execution remains a Phase 7 task.

## 14. Phase 2 image preparation decisions

`input::load(path, scale)` now returns an owned `PreparedImage`, preserving source
dimensions alongside typed packed `Pixels::Rgb` or `Pixels::Rgba` reference data.
The CLI invokes it after path validation. Phase 2 initially reported prepared
dimensions and layout in a pending-optimization error; phases 3–5 now supply
scoring, search and output. No destination is created or opened for writing
during preparation.

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
Phase 5 writes the selected candidate.

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

The CLI applies `--threshold` during search and prints numeric trial history
with `--verbose`. Phase 4 initially reported selection in a pending-output error;
Phase 5 replaces that error with writing and a success report on stderr.

Unit tests cover exact threshold equality, negative scores, lossless being
smaller, no feasible candidate, nonmonotonic curves, all 101 monotonic boundary
positions, clipped neighborhoods, deduplication, budgets 2–101, deterministic
ties, invalid options and evaluator errors. Real RGB/RGBA, tiny and transparent
fixtures at scales 1 and 0.5 and thresholds 0/80/90/100 verify selected bytes
through independent decoding and the one-shot metric. Re-evaluation verifies
every numeric summary; selection is compared against every tested feasible
trial, including ties. CLI tests verify verbose history, threshold application,
source preservation and stdout hygiene. No dependencies were added in Phase 4.

## 17. Phase 5 output and reporting

`output::write` accepts the selected `Candidate`, whose private fields and
immutable byte access preserve the evaluator's validation: nonempty still WebP,
successful pixel decoding, checked dimensions and a finite measured score.
It checks nonempty bytes and threshold feasibility at the output boundary.
No extra decode or metric comparison is needed; the exact measured bytes are
written, without re-encoding or metadata changes. Invalid or infeasible trials
cannot be constructed through the production public API and written silently.

For `-o -`, stdout is locked for `write_all` and an explicit flush. All progress,
success reports and errors use stderr. Broken pipe, short-write exhaustion and
flush errors propagate to the CLI's nonzero exit. Partial stdout writes cannot
be rolled back. Shell redirection opens/truncates its own target before the
program starts; use `-o path` for safe replacement of an existing destination.

For file output, the existing `tempfile` dependency is promoted from development
to production; no new package or lockfile resolution is required. A
`NamedTempFile` is created in the destination's existing parent directory, then
written with `write_all`, flushed and `sync_all`ed. Collision validation runs
before staging and again immediately before `persist`. Source and destination
are never opened with truncation. All failure paths drop the temporary file,
including extracting the I/O error from `PersistError` so its owned staging file
is cleaned up. Cleanup on drop is best effort if the filesystem itself refuses
removal; abrupt process termination can also leave a staging file.

**Overwrite policy:** replace an existing unrelated destination only once the
complete output is ready. `persist` performs same-directory rename/replace
(Unix `rename`; Windows `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` in the
locked dependency). No delete-first fallback is used. Input canonical/symlink/
hard-link aliases and dangling output symlinks are rejected. An unrelated
destination symlink is replaced as a directory entry, leaving its referent
untouched; a destination with other hard links likewise receives a new file
without changing the other links. Replacement creates a new file and does not
preserve the previous file's ownership, permissions or metadata. Unix temporary
file permissions normally result in mode `0600` for both new and replaced files.

Contents are synced before replacement, but the parent directory is not synced;
this is not a power-loss durability guarantee. Same-directory replacement relies
on OS/filesystem semantics; Linux tests pass, with Windows/macOS execution still
required in Phase 7. Filesystem manipulation after the final path check remains
a race; path validation is not a guarantee against adversarial concurrent changes.

The CLI exits zero only after the output operation succeeds. Its stderr success
summary includes selected mode, quality for lossy, decoded score, threshold,
original/processed dimensions, RGB/RGBA layout, bytes and measured trial count.
`--verbose` adds all trial summaries. Successful file output leaves stdout empty;
successful stdout output contains exactly the selected candidate's bytes.

Tests compare file and stdout output with real selected candidates, independently
decode and score the written payloads, and check all summary fields with and
without verbose mode. Coverage includes RGB/RGBA, resizing, tiny images and
thresholds 0/80/100. Focused I/O tests verify staged replacement, short/interrupted
writes, injected partial disk-full and deferred flush errors, cleanup on failed
rename, collision revalidation and a real Unix broken stdout connection.
An unrelated symlink test verifies entry replacement and referent preservation.

## 18. Phase 6 functional and integration coverage

The executable matrix in `tests/evaluation.rs` covers 11 fixtures: JPEG, RGB
PNG/BMP/WebP, RGBA PNG/WebP, opaque RGBA PNG, 1x1 RGB/RGBA, fully transparent
pixels and transparent color edges. Every fixture runs at scales 1 and 0.5,
thresholds 0/80/90/100, and both file and stdout output: 88 conversions, each
written through both output routes (176 process invocations). Thresholds 90/100
also exercise verbose reporting. Tests check initial creation and replacement,
dimensions including odd-axis rounding and tiny-axis clamping, exact alpha
preservation, source preservation, absence of leftover temporary files, stderr
reports and empty stdout for file output.

Written bytes must match the selected in-memory candidate exactly, making
file/stdout equality explicit across the matrix. A separate image decoder and
the one-shot `fast_ssim2` API independently score each payload against the
prepared reference; no application candidate decoder or cached metric is used
for that verification. The reference still uses application image preparation,
whose pixel normalization and premultiplied-alpha resizing have separate tests.
Scores must satisfy the exact threshold without rounding or an epsilon.

The existing search unit tests cover impossible thresholds using synthetic
scores, including measured lossless below 100 and all scores below zero. The
CLI rejects thresholds outside [0,100]. The real fixture corpus reaches 100
through measured lossless, so it does not provide a natural unachievable
threshold case; production codec behavior is not overridden for a CLI test.
Existing metric tests check that hidden RGB under alpha zero has no visible
effect, while partial transparency and alpha changes affect the score.

Ingestion failure tests now copy their sources into an isolated directory and
cover empty/unknown data, truncated PNG/JPEG/BMP/WebP, corrupt PNG, unsupported
GIF, animation and source/working-memory/WebP-axis limits. Each failure must
return nonzero with empty stdout, preserve input and any destination, and leave
no staging files. Alias/hard-link/symlink rejection remains covered by CLI tests.

Unix permission tests use the OS temporary directory, rather than the checkout
filesystem. They remove read permission from the source or write permission
from the output directory, verify the OS actually returns `PermissionDenied`,
then invoke the real CLI with absent and existing destinations. They restore
permissions even during unwinding. These two tests explicitly report a skip
when the user (such as root) or filesystem bypasses mode restrictions; Windows
ACLs and macOS/Windows execution remain Phase 7 work. Both permission checks ran
without skipping during Linux validation for this phase.

The fixture generator now has a regression test that generates into two
temporary directories, checks the complete filename sets, and compares every
image byte with both runs and the committed corpus under the locked dependency
versions. No external assets or new dependencies are needed; fixture provenance
and the repository Apache 2.0 license are recorded in `tests/fixtures/README.md`.

## 19. Phase 7 performance and robustness

`examples/benchmark.rs` generates photographic, noisy photographic, flat-color
icon and alpha-art workloads and measures the production search. Optional
`TrialEvaluator::evaluate_timed` shares the encoder, validated decoder and metric
implementation with ordinary evaluation. Its const-generic timing switch avoids
clock reads in ordinary evaluations. Regression tests compare timed and untimed
bytes/scores, including RGB, RGBA, tiny and fully transparent inputs and invalid
quality rejection.

Stage measurements replay precisely the real search's tested qualities,
including lossless. Scores/sizes must match the search and winner bytes must
match exactly. A second comparison rebuilds the metric reference for each trial
and verifies the same score. This measures reference caching without changing
the production search policy or pretending to benchmark an exhaustive search.

`scripts/benchmark.py` runs prebuilt portable/native binaries against the same
hashed PNG inputs. It records CSV repetitions and platform/toolchain/executable
metadata. A fresh CLI child measures complete conversion, staged replacement and
peak RSS separately from the stage replay and cache experiment. Unix `wait4`
provides per-child resource usage; the runner normalizes macOS RSS bytes to KiB
and leaves RSS blank where unsupported. The developer harness needs Python,
but the application has no new dependency.

Corpus provenance, reproducible commands, measurement definitions, representative
results and limitations are recorded in [benchmarks/README.md](benchmarks/README.md).
The included NASA photograph is public domain, distinct from the project-authored
Apache 2.0 test fixtures. Above 512×512 it is upsampled; noise is added after resizing.

Replacement coverage now includes preserving another hard link to an unrelated
old destination on every platform. A Windows-specific test holds the destination
open with read sharing but without delete sharing and requires replacement to
fail without changing either file or leaving a staging file. That test is gated
to Windows; Linux execution cannot validate it.

Linux validation runs under WSL2. Windows PowerShell is callable, but no Windows
Rust/C toolchain is available on PATH; no macOS runner is available. Windows and
macOS execution, including their filesystem replacement semantics and Windows
ACL behavior, remains explicitly unverified. Phase 7's cross-platform checkbox
must stay open until those platforms actually run; Phase 8's CI matrix can
provide those runs. Cross-compilation alone would not close this requirement.

The [reviewed baseline](benchmarks/RESULTS.md) records 48 fresh portable/native
CLI conversions across eight 256²/768² workloads, plus eight memory-only runs:
four at 2000² and four 4000² sources scaled to 1000². It retains raw repetitions,
input/source/executable hashes and toolchain metadata. Reference caching reduced
metric work by about 1.46–1.67× including setup. Native CPU tuning improved
transparent-art CLI medians by about 17–20%; other differences were only a few
percent and are not treated as statistically established gains.

The largest measured process RSS was about 669 MiB at full-scale 2000² RGBA;
4000² RGBA scaled to 1000² reached about 324 MiB. These samples do not warrant
tightening the existing 1 GiB estimate, which includes padded metric and resize
buffers. The policy remains a guard, not a strict RSS cap or a guarantee for
different inputs/platforms. Format, Clippy with denied warnings, all-target
check, all 76 tests and portable/native release builds passed on Linux with
locked/offline dependencies. The local performance/documentation gate is met;
cross-platform execution remains open as described above.

## 20. Phase 8 documentation, CI and release preparation

The README now starts with runnable file/stdout quick starts using committed
fixtures, including Windows commands that preserve binary stdout. Installation
uses a local checkout or `cargo install --path . --locked`; there is no invented
remote URL or crates.io package. The toolchain is pinned to Rust 1.89.0. Flags,
score meaning, resized-reference semantics, heuristic selection, error codes,
source safety, overwrite behavior, color/metadata scope and memory limits remain
explicit. Package metadata now matches the Apache-2.0 license chosen by the
repository owner, with README, categories and keywords added. Version 0.1.0 and
`publish = false` are retained.

`.github/workflows/ci.yml` validates fresh checkouts on `ubuntu-24.04` x86_64,
`macos-15` Apple Silicon and `windows-2022` MSVC, on pushes, pull requests or
manual dispatch. It installs the checked-in Rust toolchain via rustup, then
runs formatting, denied-warning Clippy, all-target tests (including native
replacement behavior), a locked portable release build and a release smoke
checker. Jobs have a time limit, read-only contents permission, independent
matrix failures, no retained checkout credentials, and a pinned checkout commit.
No publishing or tag creation is automated.

`examples/smoke.rs` launches the specified executable directly. It checks its
version, RGB/RGBA fixture conversion at scale 0.5/threshold 90, replacement of an
old destination, empty stdout for file output, exact binary stdout/file equality,
source preservation and temporary-file cleanup. Written bytes are independently
decoded and scored through the one-shot metric API, bypassing the application's
cached metric/view helper. Dimensions must be 9×6 and reference alpha must be
preserved. This complements the full integration matrix without adding a
runtime or CI Python dependency.

[DEPENDENCIES.md](DEPENDENCIES.md) records locked direct versions, licenses,
feature choices, transitive decoder choices, bundled libwebp/SharpYUV C build
requirements and platform limits. `webp` defaults are disabled, but its
`libwebp-sys` dependency's own default features remain enabled. No system codec,
CMake, pkg-config or bindgen is used. A specific existing libwebp advisory was
checked against the patched crate range; this is not a full security audit.
No dependency versions changed. Redistributable binary archives would require
the full set of third-party license notices; this phase prepares source builds.

[RELEASING.md](RELEASING.md) defines clean-checkout, hosted-matrix and output
acceptance gates before tagging. Local Linux checks do not validate macOS or
Windows. This checkout has no remote, so the workflow cannot be executed on
GitHub here. The Phase 7 platform task remains open and v0.1.0 must remain
untagged until native platform runs pass. The CI configuration has been checked
with actionlint 1.7.12; that is workflow validation, not a hosted run.


Local Phase 8 validation on Linux (2026-10-18, Rust 1.89.0) passed format,
Clippy with denied warnings, all 76 tests, the portable release build and the
independent release smoke check. Both README quick starts produced identical
WebP bytes, and local-path installation into an isolated prefix reported
version 0.1.0. For clean-build validation, a fresh local Git clone received the
same Phase 8 changes as a temporary validation commit. With no target directory,
`cargo build --release --locked --offline --bin webptune --example smoke`
rebuilt all dependencies from cached downloads; release smoke and README file/
stdout quick starts passed, and Git status stayed clean before and after.
The manifest, lockfile and Rust sources matched the working checkout. This
closes the local documentation/clean-build gate; hosted CI and the release tag
remain pending.

## 21. Phase 9 — Maximum output dimensions

The first deferred enhancement adds `--scale-width PIXELS` and
`--scale-height PIXELS` as maximum output dimensions. Either may be supplied
alone, or both together. Values must be positive integers fitting `u32`;
zero, negative, fractional, nonnumeric and overflowing values fail argument
parsing. An explicit `--scale` or `-s` conflicts with either bound, including a
factor of 1. Clap's default scale does not count as an explicit argument, so
bound-only commands work without changing the default no-resize behavior.

`limits::Resize` represents a multiplier or dimension bounds. For bounds it
chooses `min(1, width/source_width, height/source_height)`, omitting missing
bounds, with exact `u64` rational comparisons. Each output axis is rounded to
the nearest pixel (halves upward) with a minimum of one. Output never exceeds
a supplied bound or the source dimensions. This fits without cropping,
stretching or upscaling; unavoidable pixel rounding can slightly change the
exact aspect ratio, especially for tiny sources. Bounds do not bypass source,
codec or working-memory limits, and do not silently shrink further to satisfy
those limits.

`input::load_with_resize(path, Resize)` shares the existing guarded decoder,
single Lanczos3 pass, premultiplied-alpha filtering and RGB/RGBA normalization.
The original `input::load(path, scale)` API remains available as a wrapper.
The CLI resolves its sizing options once before preparation. Scoring still
compares candidates with the processed reference; quality search and safe
file/stdout output use the same pipeline.

Coverage includes landscape/portrait and single/combined bounds, limiting-axis
selection, whole-pixel rounding, one-pixel results, no upscaling, `u32` boundaries,
invalid options and scale conflicts in both orders. A grid checks that rounded
dimensions stay within both source and bound dimensions. Input tests compare
processed pixels with equivalent multiplier resizes, including transparent
edges, unchanged images, and sources exceeding WebP's output axis limit.
Header-only fixtures verify that bounds preserve pre-decode resource checks.
The executable matrix adds 56 RGB/RGBA file/stdout conversions with width-only,
height-only, combined and oversized bounds at thresholds 90/100. Their outputs
are independently decoded and scored against equivalent multiplier references,
with checks for dimensions, alpha, identical file/stdout bytes, source
preservation and staging cleanup. CI's existing all-target test step includes
these tests on each configured platform; native hosted runs remain pending.

Local Linux validation (Rust 1.89.0) passed `cargo fmt --all -- --check`,
`cargo clippy --all-targets --locked --offline -- -D warnings`,
`cargo test --all-targets --locked --offline` (84 tests), and the portable
locked/offline release build for the binary and smoke checker. The independent
release smoke checks passed. An additional release CLI check fitted the RGBA
fixture from 17×11 to 8×5 within 9×5 bounds at threshold 100, producing identical
106-byte WebP payloads through file and stdout output.
