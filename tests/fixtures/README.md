# Input fixtures

All image pixels are deterministic synthetic data created for WebP SSIM
Optimizer. They contain no external assets and use the repository's MIT license.
Regenerate the corpus from the repository root with:

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

Encoding is deterministic for the locked dependency versions; binary bytes may
change when encoders are upgraded. Tests assert decoded behavior rather than
depending on exact compressed bytes.
