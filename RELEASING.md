# Preparing v0.1.0

Release status: **native Linux/macOS/Windows CI passed; tag pending**.
[Hosted run 39074512319](https://github.com/bdobrica/WebPTune/actions/runs/39074512319) validated commit
`16cfe644518dd4b73181e1ae0e812e88a568ea16` on all three platforms, including
release builds, file replacement and independently scored release smoke checks.
The package version is 0.1.0, but no release tag has been created.
`publish = false` remains intentional: this is a source-build CLI, not a
crates.io publication workflow. Revalidate the exact proposed release commit
when it changes; an earlier passing run does not cover later code changes.

## Gates before tagging

1. Run the [CI workflow](.github/workflows/ci.yml) on the proposed commit and
   require every Linux/macOS/Windows job to pass. The matrix runs Rust 1.89.0
   formatting, Clippy with denied warnings, all-target tests, a locked release
   build, and smoke checks of the actual release binary. In particular, confirm
   successful file replacement and Windows replacement failure with a held file.
2. Confirm the independent written-output score checks, source preservation,
   binary stdout, dimension/alpha checks, failure cleanup and search-selection
   tests pass. Review the [acceptance checklist](TODO.md#release-acceptance-checklist).
3. Build from a clean checkout using `Cargo.lock` and an empty target directory.
   Exercise both README quick-start commands, then the release smoke checker.
4. Review [dependency/native requirements](DEPENDENCIES.md),
   [benchmark results](benchmarks/RESULTS.md), documented limitations, license
   and package version. If distributing binaries later, include all applicable
   third-party notices and test each artifact on its target OS.
5. Keep the successful hosted run/commit recorded in TODO.md and PLAN.md.
   Phase 7's platform checkbox is closed by the run above; the proposed release
   commit must also pass the full matrix before tagging.

Local validation commands from the checkout root:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked --bin webptune --example smoke
cargo run --release --locked --example smoke -- target/release/webptune
```

Append `.exe` to the final binary path on Windows. The smoke checker launches
the binary directly, avoiding shell text encoding of binary stdout. For each
RGB/RGBA fixture it checks replacement of an existing destination, byte equality
of file/stdout output, source preservation, decoded 9×6 dimensions, alpha,
independently recomputed SSIMULACRA2 >=90, and staging cleanup. It also verifies
the reported version against Cargo metadata. Full integration tests cover the
larger threshold/input/scale/failure matrix.

For a clean build, clone the intended commit into a fresh directory with no
`target` directory, enter it, and run the commands above. Cargo may reuse its
download cache; compiled artifacts must be fresh. `--locked` prevents lockfile
updates, and `--offline` can be added when all locked sources are already cached.
Verify `git status --porcelain` remains empty afterwards.

## Tag only after the gates pass

On the verified release commit with a clean working tree, create the local tag:

```bash
git tag -a v0.1.0 -m "webptune v0.1.0"
```

Confirm its commit and annotation with `git show v0.1.0`. Pushing a tag or
publishing a GitHub release is a separate action; this workflow never does it
automatically. The initial release notes should describe supported still-image
formats, scale/threshold semantics, the smallest-tested-candidate guarantee,
alpha/color/metadata limits, overwrite behavior, memory guards and validated
platforms, linking to the benchmark report. Do not label pending platforms as
validated.
