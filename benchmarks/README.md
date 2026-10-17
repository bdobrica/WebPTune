# Performance and robustness measurements

The benchmark uses the production search and evaluator. It is a manual developer
tool, not a timing assertion in the test suite. No benchmark dependencies are
added to the application.

See [RESULTS.md](RESULTS.md) for the reviewed Linux baseline, memory assessment
and platform validation status.

The archived Phase 7 metadata under `results/` predates the `webptune` rename.
It retains the original executable paths and hashes as recorded; the commands
below and new benchmark runs use `webptune`.

## Reproduce

Run from the repository root with Rust 1.89.0, a C compiler and Python 3.9+:

```bash
cargo build --release --locked --bin webptune --example benchmark
RUSTFLAGS="-C target-cpu=native" CARGO_TARGET_DIR=target/native cargo build --release --locked --bin webptune --example benchmark
target/release/examples/benchmark generate --sizes 256,768
python3 scripts/benchmark.py --build portable=target/release --build native=target/native/release
```

Use separate target directories to retain both binaries. Native tuning affects
Rust code; this comparison does not pass architecture flags to the bundled C
libwebp build. Its SIMD and the metric's runtime SIMD dispatch can already use
CPU features in a normal release build. Native binaries may be unsuitable for
other machines. Generation and measurement are separate so generated PNGs are
identical for both builds. On Windows, use the `.exe` suffix when invoking the
example and PowerShell environment assignment syntax for the build variables.

The runner records per-repeat CSVs and metadata containing platform/toolchain,
threshold, repetitions, input hashes and executable hashes. The default is one
warm-up search followed by three measured repetitions per workload/build.
`--inputs`, `--output`, `--repeats`, `--threshold` and `--scale` select another
run. Results go under ignored `target/`; only reviewed baseline results are committed.

For a near-limit memory check, generate into a separate directory:

```bash
target/release/examples/benchmark generate --directory target/benchmark-large --sizes 2000
python3 scripts/benchmark.py --inputs target/benchmark-large --output target/benchmark-large-results --repeats 1 --memory-only --build portable=target/release
```

`--memory-only` skips stage replay and warm-up and measures fresh CLI processes
only. To include resize buffers, generate 4000-pixel sources into another
directory and run this mode with `--scale 0.25`. Generation applies source limits;
the measured application applies its processing/resize limits before decoding.

### Workloads and provenance

- **Photo:** `astronaut.png`, a 512×512 NASA photograph of Eileen Collins,
  obtained unchanged from [scikit-image v0.25.2](https://raw.githubusercontent.com/scikit-image/scikit-image/v0.25.2/skimage/data/astronaut.png).
  SHA-256: `88431cd9653ccd539741b555fb0a46b61558b301d4110412b5bc28b5e3ea6cb5`.
  Scikit-image documents it as [public domain, with no known copyright
  restrictions](https://scikit-image.org/docs/0.25.x/api/skimage.data.html#skimage.data.astronaut).
  That provenance applies to this photograph; it is not a project-authored test fixture.
- **Noisy photo:** the same resized photograph with deterministic independent
  uniform noise in [-32, 32] added to each channel using xorshift32.
- **Icon:** a generated flat-color geometric icon with hard edges.
- **Transparent art:** generated gradients/checkers and radial straight alpha,
  including partial alpha and nonzero hidden RGB.

Generated workloads are square PNGs, RGB8 for the first three and RGBA8 for art.
The generator is in [examples/benchmark.rs](../examples/benchmark.rs); its code
and generated icon/art are under the project's Apache 2.0 license. Lanczos3
resizing of the photo happens during corpus generation. Sizes above 512 are upsampled
workloads, not new high-resolution photographic detail. Noise is added after
resizing. The stage/CPU comparison uses scale 1 and threshold 80 unless
overridden; separate memory-only runs probe large-source resizing. These
workloads do not benchmark every supported decoder.

### Measurement definitions

| CSV field | Meaning |
|---|---|
| `load_ms` | One initial PNG preparation, outside warm-up/repeated measurements |
| `reference_ms` | Construct the production cached metric reference once |
| `search_ms` | Real bounded search, including encode/decode/score and candidate bookkeeping; excludes reference setup |
| `trials` | Actual search candidates, including lossless |
| `mode`, `score`, `bytes` | The selected measured candidate |
| `encode_ms`, `decode_ms`, `metric_ms` | Totals from replaying exactly the modes tested by that search, using the same production evaluator |
| `uncached_metric_ms` | Sum of rebuilding a reference and scoring for every replayed candidate; decoding excluded |
| `cli_ms` | Separate fresh CLI process: startup, loading, reference, search, staged write/sync/replacement and exit |
| `cli_peak_kib` | That fresh CLI process's peak resident set, obtained with `wait4`; KiB on Linux, converted from bytes on macOS; blank on unsupported platforms |

Stage replay checks scores and sizes against the original search and exact bytes
for the winner. Rebuilt-reference scores must equal cached scores. To assess
caching, compare `reference_ms + metric_ms` with `uncached_metric_ms`.
Stage totals are independent measurements and need not sum to `search_ms`.
The cached evaluator remains alive during the rebuilt-reference comparison;
allocator/cache effects may differ from a complete uncached CLI implementation.
Encode timing includes config/setup; decode includes validation/layout
normalization; metric timing includes finite-score checks. Final compressed-byte
copying and drops are outside the stage timers. The ordinary evaluator does not
read the clock.

Peak memory belongs to a separate CLI process, excluding the harness, stage
replays and extra references used to compare caching. Each CLI run replaces an
existing destination and verifies its size/WebP signature, empty stdout, a
successful stderr summary and unchanged source bytes. The integration suite
performs the independent pixel/score and replacement correctness checks.

Results vary with CPU load, thermals, filesystem and allocator. These small
samples are local observations, not portable performance guarantees. Peak RSS
is not an allocation upper bound; source metadata, malformed files, resizing,
codec scratch, platform allocators and concurrent jobs can change memory use.
Resource measurements include process startup; small workloads can reflect the
Python launcher's inherited memory floor before exec. Inputs have a warm OS
filesystem cache after generation and repeated reads. Outputs use the OS
temporary directory, so CLI timings also depend on that filesystem's sync and
replacement costs.
