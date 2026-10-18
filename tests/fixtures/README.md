# Input fixtures

All image pixels are deterministic synthetic data created for WebP SSIM
Optimizer. They contain no external assets and use the repository's Apache 2.0
license. Regenerate the corpus from the repository root with:

```bash
cargo run --locked --example generate_fixtures
```

`examples/generate_fixtures.rs` documents the pixel patterns and container
construction. The corpus includes 17x11 RGB PNG/JPEG/BMP/WebP, transparent and
fully opaque RGBA, 1x1 RGB/RGBA, grayscale and grayscale-alpha, 16-bit RGBA,
transparent color edges, fully transparent color, two-frame APNG and WebP,
an EXIF-oriented JPEG, and a valid 20000x1 PNG. Corrupt PNG, unsupported GIF
and header-only BMP files exercise failure and preallocation limit checks.
The modified BMP headers intentionally have no matching pixel payload.

Color preparation fixtures add `srgb-icc.png` and `display-p3.png` (the same
17x11 RGBA samples tagged with generated standard profiles), `linear-icc.jpg`
and `linear-icc.webp` (RGB with linear-sRGB ICC), `gray-linear-icc.png` (two
grayscale-alpha pixels with a linear gray ICC), `linear16.png` (4x2 RGBA16 with
gAMA=1) and `linear.hdr` (8x2 neutral linear-sRGB RGBE samples from 0 through
128). Profiles are serialized from moxcms's standard mathematical definitions;
no external ICC files are copied. These fixtures share the repository's
synthetic-data provenance. Tests also synthesize EXIF orientations 1–8 in
JPEG/PNG/WebP and invalid/unsupported color metadata in temporary directories.

Encoding is deterministic for the locked dependency versions; binary bytes may
change when encoders are upgraded. Image-processing tests assert decoded
behavior; output-route comparisons use bytes from the same encoder run/version.

`cargo test --all-targets --locked` also tests the generator itself: it generates
the complete corpus twice in temporary directories and compares both results
with the committed image bytes. Update fixtures deliberately when upgrading
encoders. The functional CLI matrix uses these fixtures to check all supported
formats, odd/tiny dimensions, opaque/partial/zero alpha, scales 1 and 0.5,
thresholds 0/80/90/100, and both output routes.
