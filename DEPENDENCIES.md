# Dependencies and native builds

Review date: 2026-10-18. Versions below come from `Cargo.lock` and the resolved
Cargo feature graph. No dependency versions changed during Phase 8. Phase 9
color preparation promotes existing transitive moxcms and development png to
runtime dependencies and enables image's HDR codec; locked versions stay fixed.

| Direct dependency | Locked version | Purpose / enabled scope | Declared license |
|---|---|---|---|
| anyhow | 1.0.104 | Contextual errors | MIT OR Apache-2.0 |
| clap | 4.5.61 | CLI, derive; constrained to the 4.5 series | MIT OR Apache-2.0 |
| fast-ssim2 | 0.9.0 | Cached RGB/RGBA SSIMULACRA2; empty default feature set | BSD-2-Clause |
| image | 0.25.10 | JPEG, PNG, WebP, BMP and Radiance HDR; defaults disabled | MIT OR Apache-2.0 |
| moxcms | 0.8.1 | Pure-Rust ICC transforms and standard profile definitions | BSD-3-Clause OR Apache-2.0 |
| png | 0.18.1 | PNG color metadata inspection and deterministic fixtures | MIT OR Apache-2.0 |
| same-file | 1.0.6 | File identity, including hard links | Unlicense/MIT |
| tempfile | 3.27.0 | Same-directory staging and replacement | MIT OR Apache-2.0 |
| webp | 0.3.1 | Fallible native encoding; defaults disabled | MIT OR Apache-2.0 |

`fast-ssim2` requires Rust 1.89.0 and `image` requires 1.88.0; the repository
pins 1.89.0 with rustfmt and Clippy. Runtime SIMD dispatch is provided by the
metric implementation. Its optional Rayon feature is not enabled. Disabling
`webp` defaults avoids its optional `image` integration, but its dependency
`libwebp-sys` still has its own default `std`, `parallel` and `neon` features.
Do not infer that all native codec parallel/SIMD features are disabled.

`moxcms` 0.8.1 requires Rust 1.85 and was already used by `image`; adding it
directly adds no package or version change. Its default matrix/shaper/LUT and
platform SIMD features remain enabled. PNG inspection uses the existing png
0.18.1 implementation, with an explicit metadata allocation budget. HDR uses
image's pure-Rust RGBE decoder; there is no new native color-management library
or system dependency. Float transforms have finite LUT precision (documented
by moxcms as approximately 14-bit), so high-precision preparation does not imply
lossless 16-bit or HDR output. The source-color policy and parser limits are in
[README](README.md#color-orientation-and-hdr) and [PLAN section 27](PLAN.md#27-phase-9--color-orientation-and-high-precision-preparation).

## Native codec

`webp` resolves to `libwebp-sys` **0.9.6**, which builds bundled C sources with
`cc` **1.8.0** and `glob`. Its build script compiles libwebp and SharpYUV into
static libraries. A C compiler, linker and archiver must be available. The
locked build does not invoke CMake, pkg-config, bindgen or a system libwebp.
Installing a system libwebp does not replace the bundled sources.

| Release target | C toolchain | Validation |
|---|---|---|
| x86_64-unknown-linux-gnu | GCC/Clang and binutils; Debian/Ubuntu `build-essential` | Local WSL2 and hosted `ubuntu-24.04` tests/build/smoke [passed](https://github.com/bdobrica/WebPTune/actions/runs/39074512319) |
| aarch64-apple-darwin | Apple Clang and SDK from Xcode Command Line Tools | Hosted `macos-15` tests/build/smoke [passed](https://github.com/bdobrica/WebPTune/actions/runs/39074512319) |
| x86_64-pc-windows-msvc | MSVC C++ build tools and Windows SDK | Hosted `windows-2022` tests/build/smoke [passed](https://github.com/bdobrica/WebPTune/actions/runs/39074512319), including held-file replacement failure |

The hosted results above validate commit `16cfe64`; revalidate after changes.
These are the initial validation targets, not claims about other architectures,
musl, MinGW or older OS versions. Hosted runner labels and installed software
are described in [GitHub's runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
Rustup must use the native host on each runner. The `cc` crate discovers MSVC
through the installed Visual Studio toolchain; a Unix C compiler is not required
on Windows. GitHub-hosted jobs supply their native development tools; local
users must install them as described in [README.md](README.md#installation).

The vendored `configure.ac`/`NEWS` identify a **1.3.1 base**, with subsequent
patches in the crate sources. Do not equate that label with an unpatched upstream
tarball. In particular, [RustSec RUSTSEC-2023-0061](https://rustsec.org/advisories/RUSTSEC-2023-0061.html)
lists `libwebp-sys >=0.9.3` as patched; locked 0.9.6 satisfies that range, and its
Huffman table code contains the segmented allocation implementation. This is a
specific advisory check, not a complete or continuously refreshed security audit.

Production WebP **decoding** goes through `image` / pure-Rust `image-webp` 0.2.4,
not the C decoder exported by `libwebp-sys`. JPEG uses `zune-jpeg` 0.5.15 and PNG
uses `png` 0.18.1. Native libwebp is used for encoding. Decoder allocation limits
and the application's memory estimate remain best-effort safeguards; see
[README limitations](README.md#behavior-and-limitations).

Portable release builds use the default target CPU. Optional
`RUSTFLAGS="-C target-cpu=native"` tunes Rust code for the build machine and
does not explicitly tune the C compiler; those binaries must not be assumed
portable. Do not set native CPU flags in distributable builds.

## Review and redistribution

Inspect the exact locked graph before changing versions or release targets:

```bash
cargo metadata --locked --format-version 1
cargo tree --locked -e features
cargo tree --locked -e features -i libwebp-sys
```

The application is Apache-2.0. Dependencies retain their own licenses; in
particular the bundled libwebp `vendor/COPYING` uses BSD terms independently
of the Rust wrapper's declared MIT license. Binary distributions must carry
the applicable dependency license and attribution texts, including that native
codec notice. This phase prepares source builds and CI, and does not publish
binary archives or crates. Before adding such artifacts, collect the complete
resolved graph's notices rather than copying only this direct-dependency table.
The NASA benchmark photograph has separate public-domain
[provenance](benchmarks/README.md#workloads-and-provenance).

On dependency upgrades, rerun the full native matrix, fixture-regeneration test,
independent decoded-score checks and release smoke check; reassess memory and
benchmarks if behavior changes. A current advisory scan (for example, with
`cargo audit` and a refreshed database) is useful before distribution. No full
advisory scan was run as part of this build/feature/license review.
