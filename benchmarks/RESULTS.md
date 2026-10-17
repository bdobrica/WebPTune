# Phase 7 baseline: Linux / WSL2

Measured on October 17–18, 2026 (Europe/Bucharest) with an Intel Core i5-9300H
at nominal 2.40 GHz, 8 logical CPUs and about 15.5 GiB available to WSL2.
Rust 1.89.0 / LLVM 20.1.7, Linux x86_64, locked dependencies, default release
profile. No benchmark jobs ran alongside compilation or other benchmark jobs.
CPU affinity/frequency and host background activity were not controlled.

The portable build used `cargo build --release`; the second build added only
`RUSTFLAGS="-C target-cpu=native"` in a separate target directory. Both built
bundled libwebp normally, without an added native C flag. Metric runtime SIMD
dispatch is available in the portable build too. See [reproduction commands
and measurement definitions](README.md).

Inputs were generated PNGs on the checkout's mounted Windows filesystem;
destinations were in `/tmp` (tmpfs). Repeated reads had a warm filesystem cache.
CLI times include startup, PNG preparation, reference setup, search and synced
replacement of an existing destination. File sync costs on other filesystems
can differ. These are local observations, not cross-machine performance claims.

## Portable versus native

Threshold 80, scale 1; one warm-up search, then three measured repetitions per
workload/build. Times are medians; RSS is the maximum observed portable CLI peak
across its three fresh processes. Small RSS values can reflect the launcher's
startup floor. Each build performed 24 measured CLI conversions (48 total).

| Workload | Trials | Portable CLI (s) | Native CLI (s) | Portable peak RSS (MiB) |
|---|---:|---:|---:|---:|
| Icon 256² | 15 | 0.185 | 0.193 | 22.6 |
| Icon 768² | 15 | 1.412 | 1.375 | 69.7 |
| Noisy photo 256² | 11 | 0.401 | 0.395 | 26.5 |
| Noisy photo 768² | 11 | 2.469 | 2.434 | 87.8 |
| Photo 256² | 15 | 0.345 | 0.341 | 22.6 |
| Photo 768² | 14 | 2.146 | 2.086 | 71.5 |
| Transparent art 256² | 14 | 0.680 | 0.567 | 22.6 |
| Transparent art 768² | 15 | 4.999 | 3.983 | 103.5 |

Native tuning was about 17–20% faster for transparent art in this run. Other
differences ranged from roughly 3% faster to 4% slower, with only three samples
and no claim of statistical significance. Keep normal release builds as the
portable default; native tuning is an optional local optimization.

Modes, reported scores, sizes and candidate counts matched between builds in every
workload. Photos selected lossy q82/q83 at 256/768 respectively; transparent art
256 selected q84. Other cases selected measured lossless. There were 11–15
candidates, within the 18-lossy-plus-one-lossless budget. Stage replay verified
each trial's score/size and exact winner bytes against the production search.

## Stage timings and reference caching

Portable medians, in milliseconds. Encode/decode/metric columns sum the replayed
trials of one search. Reference setup happens once per search. Rebuilt metric
time constructs a reference for every trial and compares the same decoded
pixels, checking the same scores. These replay totals are separate measurements
and do not necessarily sum to the measured search or CLI time.

| Workload | Reference setup | Encode | Decode | Cached comparisons | Rebuilt metric total |
|---|---:|---:|---:|---:|---:|
| Icon 256² | 2.5 | 38.8 | 12.0 | 98.0 | 154.9 |
| Icon 768² | 31.8 | 331.3 | 86.3 | 935.7 | 1414.2 |
| Noisy photo 256² | 2.4 | 237.9 | 35.6 | 54.0 | 82.2 |
| Noisy photo 768² | 29.6 | 1349.8 | 301.7 | 634.8 | 1046.2 |
| Photo 256² | 2.4 | 171.1 | 30.1 | 73.4 | 110.8 |
| Photo 768² | 30.0 | 976.8 | 198.4 | 818.5 | 1288.6 |
| Transparent art 256² | 13.6 | 307.4 | 21.6 | 259.7 | 447.9 |
| Transparent art 768² | 145.6 | 1555.6 | 208.9 | 2902.0 | 5091.7 |

Dividing rebuilt time by **setup + cached comparisons** gives approximately
1.46–1.67× across these workloads. Caching meaningfully reduces metric work, so
retain the existing single-reference design. This is a component comparison,
not a measured whole-CLI speedup against an uncached application. The original
cached evaluator remains alive during the rebuilding experiment, affecting
allocator/cache behavior. Encoding dominates the noisy-photo case, whereas
metric work is substantial for icons and transparent art; optimizing a single
stage cannot promise the same overall gain for every image.

## Memory checks and policy

The additional memory-only runs use fresh portable CLI processes, threshold 80,
one sample per case, without stage replay or extra metric references. Full-size
2000² inputs approach the current estimated processing budget. The separate
4000² sources at scale 0.25 exercise Lanczos scratch buffers and, for art,
premultiplied floating-point alpha buffers. All conversions succeeded, preserved
their source, replaced the existing destination and left stdout empty.

| Workload | Source → output | CLI (s) | Peak RSS (MiB) |
|---|---|---:|---:|
| Icon | 2000² → 2000² | 12.273 | 443.6 |
| Noisy photo | 2000² → 2000² | 16.987 | 452.1 |
| Photo | 2000² → 2000² | 15.602 | 448.3 |
| Transparent art | 2000² → 2000² | 34.721 | 669.0 |
| Icon | 4000² → 1000² | 3.730 | 115.0 |
| Noisy photo | 4000² → 1000² | 6.165 | 116.7 |
| Photo | 4000² → 1000² | 5.147 | 116.1 |
| Transparent art | 4000² → 1000² | 10.399 | 323.8 |

The largest observed process peak was about 669 MiB for 2000² alpha art, versus
the policy's approximately 1007 MiB estimate for that full-scale size. The 4000²
alpha source at scale 0.25 peaked around 324 MiB, versus approximately 671 MiB
estimated including resize scratch. Both remain within the current 1 GiB policy.

The existing policy remains: 100 million source pixels, 16,383 per output axis,
1 GiB estimated working memory including source, metric padding and resize
buffers. Full-scale 2000² is accepted and 2048² is rejected before pixel decoding.
Peak RSS is measured process residency, not the policy's allocation estimate.
These samples do not justify tighter limits. The estimate remains a conservative
guard rather than a guaranteed process memory cap.

Unmeasured risks remain: different codecs and platform allocators, large metadata
or compressed inputs buffered before header validation, pathological dimensions,
other thresholds/scale ratios, corrupted files, and systems with less available RAM or many
concurrent conversions. The measurements do not establish an upper bound for
every permitted input. Decoder limits remain best effort, and allocation failure
or upstream codec panics are not universally recoverable.

## Validation and platform status

Linux: all 76 tests, `cargo fmt --all -- --check`,
`cargo clippy --all-targets --locked --offline -- -D warnings`,
`cargo check --all-targets --locked --offline`, and both release builds passed.
The tests include actual file replacement, preserving other hard links to an
old destination, staged-write/rename failures, stdout failures and timed/ordinary
evaluator equivalence. The integration matrix independently decodes and scores
the fixture outputs; the benchmark complements those correctness tests.

The benchmark measurements above were collected on Linux only. Subsequent
[hosted CI on commit `16cfe64`](https://github.com/bdobrica/WebPTune/actions/runs/39074512319)
passed native Linux, macOS and Windows tests, release builds and smoke checks,
including file replacement and the Windows failure test for a destination
opened without delete sharing. This closes Phase 7's cross-platform checkbox;
it does not add macOS/Windows performance measurements. Windows ACL-specific
denial cases remain outside the current suite. See
[the platform validation record](../PLAN.md#24-hosted-platform-validation).

Raw [portable samples](results/2026-10-18-linux-wsl2/portable.csv),
[native samples](results/2026-10-18-linux-wsl2/native.csv) and
[metadata](results/2026-10-18-linux-wsl2/metadata.json) retain all repetitions,
input hashes, binary hashes and toolchain details. The photographic source is
512²; larger photo workloads are upsampled. The noise and geometric/alpha art
are synthetic. See [corpus provenance and limitations](README.md).

Memory-only [full-size samples](results/2026-10-18-linux-wsl2/large.csv) and
[resize samples](results/2026-10-18-linux-wsl2/resize.csv) have separate
[full-size metadata](results/2026-10-18-linux-wsl2/large-metadata.json) and
[resize metadata](results/2026-10-18-linux-wsl2/resize-metadata.json).
[Source hashes](results/2026-10-18-linux-wsl2/sources.json) identify the measured
evaluator, benchmark example and dependency lockfile.
