# Implementation Checklist — webptune

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
output. Phase 2 connects the reusable header and processing limits to
decoding/resizing; phases 3–4 supply scoring and selection, and Phase 5 writes output.
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

Phase 2 connected reference preparation to the CLI; phases 3–4 evaluate and
select candidates and Phase 5 writes the winner. Preparation itself creates no
output. Input tests cover supported formats, alpha/opaque layouts,
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
tiny/transparent images. Phase 3 exercised quality 75 and lossless as CLI pipeline
checks; Phase 4 replaces these with threshold-based search and selection, and
Phase 5 writes the winner. See [PLAN.md, section 15](PLAN.md#15-phase-3-metric-and-trial-evaluation).

## Phase 4 — Search and selection (M3)

- [x] Define `Candidate`, feasibility (`score >= threshold`), size-first ordering and stable tie breaks.
- [x] Add tested-quality cache / deduplication.
- [x] Evaluate quality endpoints `0` and `100`.
- [x] Implement integer binary-search heuristic with an explicit evaluation budget.
- [x] Probe the boundary neighborhood (suggested ±4 qualities) and retain smallest feasible tested bytes.
- [x] Ensure no monotonicity assumption is treated as a correctness guarantee.
- [x] Always evaluate a lossless candidate, decode and score it.
- [x] Select smallest **tested feasible** lossy/lossless output; document heuristic scope.
- [x] Return explicit failure when no candidate satisfies threshold.
- [x] Unit test score exactly equal threshold, lossless smaller, no feasible quality, nonmonotonic synthetic score curve, duplicate quality and tie breaking.

**Gate:** algorithm chooses smallest feasible *evaluated* candidate, not simply minimum quality.

Validation: Rust 1.89.0 `cargo check --all-targets --locked`,
`cargo fmt --all -- --check`, `cargo clippy --all-targets --locked -- -D warnings`,
and `cargo test --all-targets --locked` (twenty-four unit tests, thirty-two
integration tests and five Phase 0 prototype tests on Linux).

The default budget permits up to 18 distinct lossy trials plus measured lossless.
Only the best feasible bytes and numeric trial summaries are retained. Real-image
tests independently score selected bytes and compare them with every tested
feasible trial. Synthetic tests cover all 101 monotonic boundary positions,
nonmonotonic scores/sizes, missed untested qualities, exact thresholds, ties,
deduplication, budget limits and evaluator failures. The CLI now enforces the
threshold during selection; Phase 5 adds successful output writing.
See [PLAN.md, section 16](PLAN.md#16-phase-4-search-and-selection).

## Phase 5 — Output and reporting (M4)

- [x] Verify candidate WebP bytes are nonempty and valid before writing.
- [x] For `-o -`, `write_all` raw bytes to locked stdout; all human messages go to stderr.
- [x] For file output, use a same-directory temporary file followed by safe rename/replace; implement cleanup on errors.
- [x] Do not truncate source or existing destination prematurely; explicitly decide overwrite behavior.
- [x] Handle broken pipe and insufficient disk space as nonzero failures.
- [x] Print mode, quality (if lossy), score, dimensions, byte count and trial count to stderr (`--verbose` for detailed trials).
- [x] Add tests that stdout bytes are WebP and no diagnostics leak into stdout.

**Gate:** output path and stdout both produce the selected, independently verifiable bytes.

Validation: Rust 1.89.0 `cargo check --all-targets --locked --offline`,
`cargo fmt --all -- --check`,
`cargo clippy --all-targets --locked --offline -- -D warnings`, and
`cargo test --all-targets --locked --offline` (thirty unit tests, thirty-six
integration tests and five Phase 0 prototype tests on Linux; 71 total).

Output accepts only immutable, decoded-and-scored candidate bytes and checks
nonempty payload/threshold feasibility before writing. File output stages beside
the destination, flushes/syncs, revalidates path collisions, then replaces the
destination using `tempfile::NamedTempFile::persist`. Existing destinations are
overwritten only at replacement; unrelated symlink entries are replaced without
changing their referents. Staging files are dropped on failure. Stdout is locked,
written with `write_all`, and flushed; diagnostics stay on stderr.

Tests independently decode and score file/stdout bytes, compare them exactly
with the selected candidate, and check summaries and verbose history. Failure
coverage includes injected partial disk-full/flush errors, late hard-link
collisions, failed rename cleanup and a real broken stdout connection on Unix.
See [PLAN.md, section 17](PLAN.md#17-phase-5-output-and-reporting) for overwrite,
permissions, durability and platform limits.

## Phase 6 — Functional/integration tests (M5)

- [x] Test JPEG, RGB PNG, RGBA PNG, BMP and WebP inputs.
- [x] Test scale `1`, `0.5`, tiny dimensions and odd dimensions.
- [x] Test thresholds `0`, `80`, `90`, `100`, and impossible/corner scenarios.
- [x] Test RGB under transparent alpha, partially transparent pixels, and fully transparent images.
- [x] Test all supported outputs decode and satisfy the measured score constraint.
- [x] Test same-file rejection, corrupted input, unreadable files, unwritable outputs, and oversized images.
- [x] Compare byte-identical result for file vs stdout with identical options (if codec deterministic).
- [x] Confirm original file remains unchanged on success and failure.
- [x] Record fixture licenses/provenance and add deterministic synthetic fixture generator.

**Gate:** integration suite covers correctness, failure, resizing, alpha and CLI hygiene.

Validation: Rust 1.89.0 `cargo check --all-targets --locked --offline`,
`cargo fmt --all -- --check`,
`cargo clippy --all-targets --locked --offline -- -D warnings`, and
`cargo test --all-targets --locked --offline` (thirty unit tests, thirty-eight
integration tests, five API prototype tests and one fixture-generator test;
74 total on Linux).

The CLI matrix checks 11 fixtures × 2 scales × 4 thresholds × 2 output routes
(176 invocations). Written bytes match the selected candidate, independently
decode/score at or above the threshold, preserve alpha/dimensions, and leave
the source unchanged. Failures cover aliases, truncated/corrupt/empty/unknown
inputs, animation and allocation/codec limits, preserving existing destinations
and leaving no staging files. Unix unreadable-source and unwritable-directory
tests ran with real permission denial; they report a skip on systems that bypass
mode restrictions. Impossible valid thresholds use the existing synthetic search
tests; real fixtures reach 100 through measured lossless. The project-authored synthetic
fixture generator is now tested for byte-identical regeneration twice against
the committed corpus. See [PLAN.md, section 18](PLAN.md#18-phase-6-functional-and-integration-coverage)
for coverage boundaries; cross-platform execution remains Phase 7 work.

## Phase 7 — Performance and robustness (M5)

- [x] Run benchmarks on photo, noisy photo, flat-color icon and transparent art at multiple sizes.
- [x] Measure encode/decode/metric timing separately and total candidate count.
- [x] Measure peak memory and add tighter limits if large images are hazardous.
- [x] Compare `cargo build --release` with `RUSTFLAGS="-C target-cpu=native" cargo build --release` on local hardware.
- [x] Assess whether cached metric reference meaningfully improves run time.
- [x] Run `cargo fmt`, `cargo clippy -D warnings`, all unit/integration tests.
- [ ] Test Linux, macOS and Windows, including file replacement behavior.

**Gate:** document at least one representative benchmark run and all known limits.

Local gate met: the [reproducible harness](benchmarks/README.md) and
[reviewed Linux baseline](benchmarks/RESULTS.md) include separate stage timings,
actual candidate counts, 48 portable/native CLI measurements at 256²/768²,
and eight fresh memory-only conversions at 2000² or 4000² resized to 1000².
Raw repetitions, source/input/binary hashes and toolchain metadata are committed.
Cached metric work was about 1.46–1.67× faster including setup; native tuning
was workload dependent. Peak RSS reached about 669 MiB, so the existing 1 GiB
estimate/resize guards are retained; this is not a hard process memory cap.

Validation: Rust 1.89.0 format, Clippy (`-D warnings`), all-target check, all
76 tests and both release builds passed with locked/offline dependencies on Linux.
Timed/ordinary evaluator equivalence and preserving other hard links to an old
destination are tested. A Windows-specific sharing/rename failure test is ready
but unexecuted. No macOS runner or Windows Rust/C toolchain is available here;
the platform checkbox deliberately remains open until real runs pass, including
replacement behavior. See [PLAN.md, section 19](PLAN.md#19-phase-7-performance-and-robustness).

## Phase 8 — Documentation, CI and release (M5)

- [x] Complete `README.md`: quick start, install, flags, examples, score meaning, limitations.
- [x] Keep `PLAN.md` aligned with decisions and `TODO.md` checked off as completed.
- [x] Add GitHub Actions for format, lint, tests, release build and smoke test.
- [x] Add `LICENSE`, repository description, and release version metadata.
- [x] Validate building with `--locked` from a clean checkout.
- [x] Review dependencies and native libwebp build requirements for release targets.
- [ ] Tag v0.1.0 after all acceptance gates pass.

**Gate:** new users can build and successfully run both `-o file.webp` and `-o -` from README instructions.

Phase 8 implementation includes executable quick-start examples, local-path
installation, platform C-toolchain prerequisites, the complete flags/score/limits
guide, [dependency review](DEPENDENCIES.md) and [release procedure](RELEASING.md).
Package metadata declares v0.1.0, Apache-2.0, README, description, categories and
keywords. The Apache license decision is reflected in all current licensing
descriptions, including fixture and benchmark provenance.

The [CI workflow](.github/workflows/ci.yml) runs format, Clippy, all-target tests,
a locked release build and release smoke checks on Ubuntu 24.04 x86_64, macOS 15
Apple Silicon and Windows 2022 MSVC. It uses the pinned repository toolchain,
read-only permissions and a commit-pinned checkout action. `examples/smoke.rs`
checks the actual binary's version, RGB/RGBA conversions, existing-file
replacement, file/stdout byte equality, independent decoded score >=90, resized
dimensions, alpha, source preservation and cleanup.

Linux validation (Rust 1.89.0, 2026-10-18): format, Clippy with denied warnings,
all 76 tests, portable release build, independent release smoke checks, both
README quick starts with byte-identical output, and local-path installation
passed. An isolated local Git clone with the same Phase 8 changes committed as
a validation snapshot built with `--release --locked --offline` from an empty
target directory; smoke and quick starts passed there too, with clean Git status
before and after. Cargo reused only downloaded sources, not compiled artifacts.
The source manifest, lockfile and Rust sources matched the working checkout.

Hosted execution remains pending: this checkout has no Git remote and there is
no local macOS/Windows Rust/C toolchain. Workflow configuration is validated with
`actionlint`; it does not count as passing hosted CI. Phase 7's platform check
and the v0.1.0 tag remain open until actual native runs pass.

## Phase 9 — Deferred enhancements (NOT required for MVP)

- [ ] Add `--scale-width` and `--scale-height` arguments as alternatives to `--scale`, allowing users to specify maximum output dimensions. Resize the image to fit within the specified bounds while preserving its original aspect ratio, without cropping or stretching.
- [ ] Add `--search exhaustive` (all 101 integer lossy qualities + lossless), with guarantee limited to this set.
- [ ] Add `--skip-if-larger`, `--max-bytes`, JSON reporting or quiet mode.
- [ ] Add batch/directory mode with bounded parallelism.
- [ ] Add color profile support, EXIF orientation and 16-bit/HDR pathway.
- [ ] Add richer WebP encoder tuning options and alpha-quality controls.
- [ ] Add stdin input and interrupted-operation cancellation.

## Release acceptance checklist

Checked items below are validated on local Linux; the final native CI gate is
still required before tagging. Failure coverage follows the documented limits
for shell redirection, partial stdout, filesystem races and abrupt termination.

- [x] `cargo fmt --all -- --check` passes.
- [x] `cargo clippy --all-targets -- -D warnings` passes.
- [x] `cargo test --all-targets` passes.
- [x] `cargo build --release --locked` passes.
- [x] Output is valid WebP at correct dimensions.
- [x] Measured output score meets the threshold on every successful fixture.
- [x] Smallest **tested feasible** candidate is selected.
- [x] Raw WebP on stdout has no text contamination.
- [x] All failure paths return nonzero, preserve input and avoid partial final files.
- [x] Limitations, non-optimality guarantee and scope are clearly documented.
- [ ] Native Linux/macOS/Windows CI jobs pass on the proposed release commit.
