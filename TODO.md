# Implementation Checklist — WebP SSIM Optimizer

This list implements [PLAN.md](PLAN.md). Work roughly top to bottom; check a task only after its acceptance criteria are met. **MVP boundary:** complete phases 0–8. Phase 9 is optional follow-up.

## Phase 0 — Setup and API validation (M0)

- [x] Create Rust binary package (`edition = "2021"`) and version-control repository.
- [x] Establish MSRV (initially Rust 1.89) and add `rust-toolchain.toml` if desired.
- [x] Add initial dependencies: clap 4.5, image 0.25 with explicit features, webp 0.3, fast-ssim2 0.9, error handling crate(s).
- [x] Commit `Cargo.lock` and confirm `cargo check` on target toolchain.
- [x] Implement a disposable API spike: encode RGB and RGBA with `webp::Encoder` using fallible methods; decode via `image`.
- [x] API spike: create `fast_ssim2::PixelSlice`, `PixelDescriptor`, `Ssimulacra2Reference`; compare original to identical/distorted images.
- [x] Verify available alpha-aware pixel descriptor and expected behavior of transparent colors.
- [x] Investigate WebP dimension constraints and animated-input detection; document findings.
- [x] Add `.gitignore`, choose `LICENSE`, basic `src/lib.rs`/`src/main.rs` layout.

**Gate:** clean build and tested encode/decode/score prototype, including transparency.

Validation: Rust 1.89.0 `cargo check --all-targets --locked`,
`cargo fmt --all -- --check`, `cargo clippy --all-targets --locked -- -D warnings`,
and `cargo test --all-targets --locked` (five prototype tests).
Run `cargo run --locked --example api_spike` to inspect the prototype's results.
API findings and remaining input-detection limitations are recorded in
[PLAN.md, section 12](PLAN.md#12-phase-0-api-validation-findings).

## Phase 1 — CLI and validation (M1)

- [x] Define clap `Args`: `-i/--input`, `-o/--output`, `-t/--threshold` default 80, `-s/--scale` default 1.0, `-v/--verbose`.
- [x] Use a typed `OutputTarget::Stdout | File(PathBuf)` for `-o -`.
- [x] Reject non-finite thresholds, thresholds outside [0,100], scales outside (0,1].
- [x] Provide actionable CLI help and parse errors.
- [x] Add source/destination collision validation, including canonical path checks where possible.
- [x] Establish application pixel/memory safety limits and checked dimension arithmetic.
- [x] Add CLI argument unit tests and parsing integration tests.

**Gate:** invalid input exits nonzero without writing output.

Validation: Rust 1.89.0 `cargo check --all-targets --locked`,
`cargo fmt --all -- --check`, `cargo clippy --all-targets --locked -- -D warnings`,
and `cargo test --all-targets --locked` (nine unit tests, ten CLI integration
tests, and five Phase 0 prototype tests on Linux).
Phase 1 established argument/path validation and a pending-pipeline error without
output. Phase 2 now connects the reusable header and processing limits to
decoding/resizing; optimization and output remain pending.
See [PLAN.md, section 13](PLAN.md#13-phase-1-cli-and-validation-decisions).

## Phase 2 — Decode, normalize and resize (M1)

- [x] Decode supported still images (JPEG, PNG, WebP, BMP) and report unsupported formats.
- [x] Reject malformed/corrupt input and excessive image dimensions before large allocations where possible.
- [x] Detect and reject animation where supported; explicitly document remaining limitations.
- [x] Scale dimensions with `max(1, round(dimension * scale))`, checked for overflow and codec limits.
- [x] Resize with Lanczos3 at most once; bypass resize when scale is 1.
- [x] Normalize opaque source to RGB8 and alpha-bearing source to RGBA8.
- [x] Document lack of full ICC/profile, EXIF rotation and 16-bit fidelity in v0.1.
- [x] Add fixtures for RGB PNG, JPEG, BMP, RGBA and tiny images.

**Gate:** dimensions and alpha representation are correct before encoding.

Validation: Rust 1.89.0 `cargo check --all-targets --locked`,
`cargo fmt --all -- --check`, `cargo clippy --all-targets --locked -- -D warnings`,
and `cargo test --all-targets --locked` (eleven unit tests, twenty-two integration
tests and five Phase 0 prototype tests on Linux).

Phase 2 connected reference preparation to the CLI; Phase 3 now evaluates trials
before reporting that search and output remain pending. It creates no output
and preserves any existing destination. Input tests cover supported formats, alpha/opaque layouts,
rounded and tiny scales, premultiplied-alpha filtering, animation rejection,
header limits, malformed data, grayscale/16-bit conversion and explicit EXIF
behavior. The committed synthetic fixtures have a reproducible generator and
provenance. See [PLAN.md, section 14](PLAN.md#14-phase-2-image-preparation-decisions)
for resize memory accounting and decoder/animation limitations.

## Phase 3 — Metric and candidate evaluation (M2)

- [x] Build reference `PixelSlice` with correct pixel width, height, stride and descriptor.
- [x] Construct `Ssimulacra2Reference` only once per image.
- [x] Implement `score(decoded_candidate)` for RGB and RGBA output; verify dimensions first.
- [x] Implement a trial evaluator: encode WebP in memory using fallible API, decode WebP, score and return size/mode/score/bytes.
- [x] Ensure quality `0..=100` and lossless are both supported.
- [x] Handle all encoder, decoder and metric errors; reject non-finite computed scores.
- [x] Add tests proving identical reference gives 100 in opaque cases; verify alpha case based on actual library behavior.
- [x] Verify score of output against reference using an independent integration-test scoring path.

**Gate:** each trial has measured decoded score, with no disk writes.

Validation: Rust 1.89.0 `cargo check --all-targets --locked`,
`cargo fmt --all -- --check`, `cargo clippy --all-targets --locked -- -D warnings`,
and `cargo test --all-targets --locked` (twelve unit tests, thirty integration
tests and five Phase 0 prototype tests on Linux).

`MetricReference` caches the reference once; `TrialEvaluator` returns only
successfully encoded, decoded and scored trial bytes. Tests cover every integer
lossy quality, lossless, alpha semantics, malformed/animated candidates,
dimension/layout errors and non-finite scores. An independent decoder and the
one-shot metric API verify trial scores for all supported formats, scales and
tiny/transparent images. The CLI evaluates quality 75 and lossless as pipeline
checks, then exits nonzero without output until phases 4–5 implement selection
and writing. See [PLAN.md, section 15](PLAN.md#15-phase-3-metric-and-trial-evaluation).

## Phase 4 — Search and selection (M3)

- [ ] Define `Candidate`, feasibility (`score >= threshold`), size-first ordering and stable tie breaks.
- [ ] Add tested-quality cache / deduplication.
- [ ] Evaluate quality endpoints `0` and `100`.
- [ ] Implement integer binary-search heuristic with an explicit evaluation budget.
- [ ] Probe the boundary neighborhood (suggested ±4 qualities) and retain smallest feasible tested bytes.
- [ ] Ensure no monotonicity assumption is treated as a correctness guarantee.
- [ ] Always evaluate a lossless candidate, decode and score it.
- [ ] Select smallest **tested feasible** lossy/lossless output; document heuristic scope.
- [ ] Return explicit failure when no candidate satisfies threshold.
- [ ] Unit test score exactly equal threshold, lossless smaller, no feasible quality, nonmonotonic synthetic score curve, duplicate quality and tie breaking.

**Gate:** algorithm chooses smallest feasible *evaluated* candidate, not simply minimum quality.

## Phase 5 — Output and reporting (M4)

- [ ] Verify candidate WebP bytes are nonempty and valid before writing.
- [ ] For `-o -`, `write_all` raw bytes to locked stdout; all human messages go to stderr.
- [ ] For file output, use a same-directory temporary file followed by safe rename/replace; implement cleanup on errors.
- [ ] Do not truncate source or existing destination prematurely; explicitly decide overwrite behavior.
- [ ] Handle broken pipe and insufficient disk space as nonzero failures.
- [ ] Print mode, quality (if lossy), score, dimensions, byte count and trial count to stderr (`--verbose` for detailed trials).
- [ ] Add tests that stdout bytes are WebP and no diagnostics leak into stdout.

**Gate:** output path and stdout both produce the selected, independently verifiable bytes.

## Phase 6 — Functional/integration tests (M5)

- [ ] Test JPEG, RGB PNG, RGBA PNG, BMP and WebP inputs.
- [ ] Test scale `1`, `0.5`, tiny dimensions and odd dimensions.
- [ ] Test thresholds `0`, `80`, `90`, `100`, and impossible/corner scenarios.
- [ ] Test RGB under transparent alpha, partially transparent pixels, and fully transparent images.
- [ ] Test all supported outputs decode and satisfy the measured score constraint.
- [ ] Test same-file rejection, corrupted input, unreadable files, unwritable outputs, and oversized images.
- [ ] Compare byte-identical result for file vs stdout with identical options (if codec deterministic).
- [ ] Confirm original file remains unchanged on success and failure.
- [ ] Record fixture licenses/provenance and add deterministic synthetic fixture generator.

**Gate:** integration suite covers correctness, failure, resizing, alpha and CLI hygiene.

## Phase 7 — Performance and robustness (M5)

- [ ] Run benchmarks on photo, noisy photo, flat-color icon and transparent art at multiple sizes.
- [ ] Measure encode/decode/metric timing separately and total candidate count.
- [ ] Measure peak memory and add tighter limits if large images are hazardous.
- [ ] Compare `cargo build --release` with `RUSTFLAGS="-C target-cpu=native" cargo build --release` on local hardware.
- [ ] Assess whether cached metric reference meaningfully improves run time.
- [ ] Run `cargo fmt`, `cargo clippy -D warnings`, all unit/integration tests.
- [ ] Test Linux, macOS and Windows, including file replacement behavior.

**Gate:** document at least one representative benchmark run and all known limits.

## Phase 8 — Documentation, CI and release (M5)

- [ ] Complete `README.md`: quick start, install, flags, examples, score meaning, limitations.
- [ ] Keep `PLAN.md` aligned with decisions and `TODO.md` checked off as completed.
- [ ] Add GitHub Actions for format, lint, tests, release build and smoke test.
- [ ] Add `LICENSE`, repository description, and release version metadata.
- [ ] Validate building with `--locked` from a clean checkout.
- [ ] Review dependencies and native libwebp build requirements for release targets.
- [ ] Tag v0.1.0 after all acceptance gates pass.

**Gate:** new users can build and successfully run both `-o file.webp` and `-o -` from README instructions.

## Phase 9 — Deferred enhancements (NOT required for MVP)

- [ ] Add `--search exhaustive` (all 101 integer lossy qualities + lossless), with guarantee limited to this set.
- [ ] Add `--skip-if-larger`, `--max-bytes`, JSON reporting or quiet mode.
- [ ] Add batch/directory mode with bounded parallelism.
- [ ] Add color profile support, EXIF orientation and 16-bit/HDR pathway.
- [ ] Add richer WebP encoder tuning options and alpha-quality controls.
- [ ] Add stdin input and interrupted-operation cancellation.

## Release acceptance checklist

- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --all-targets -- -D warnings` passes.
- [ ] `cargo test --all-targets` passes.
- [ ] `cargo build --release --locked` passes.
- [ ] Output is valid WebP at correct dimensions.
- [ ] Measured output score meets the threshold on every successful fixture.
- [ ] Smallest **tested feasible** candidate is selected.
- [ ] Raw WebP on stdout has no text contamination.
- [ ] All failure paths return nonzero, preserve input and avoid partial final files.
- [ ] Limitations, non-optimality guarantee and scope are clearly documented.
