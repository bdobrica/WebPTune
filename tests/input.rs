use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use image::{AnimationDecoder, ImageDecoder};
use tempfile::tempdir;
use webptune::input::{self, Pixels};
use webptune::limits::Resize;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn supported_rgb_formats_produce_packed_pixels() {
    let expected = input::load(&fixture("rgb.png"), 1.0).unwrap();
    assert_eq!(&expected.pixels().as_bytes()[..6], &[0, 0, 0, 13, 0, 7]);
    for name in ["rgb.png", "rgb.jpg", "rgb.bmp", "rgb.webp"] {
        let original = fs::read(fixture(name)).unwrap();
        let loaded = input::load(&fixture(name), 1.0).unwrap();
        assert_eq!(loaded.original_dimensions(), (17, 11));
        assert_eq!(loaded.pixels().dimensions(), (17, 11));
        assert!(matches!(loaded.pixels(), Pixels::Rgb(_)));
        assert_eq!(loaded.pixels().as_bytes().len(), 17 * 11 * 3);
        if name != "rgb.jpg" {
            assert_eq!(loaded.pixels().as_bytes(), expected.pixels().as_bytes());
        }
        assert_eq!(fs::read(fixture(name)).unwrap(), original);
    }
}

#[test]
fn format_detection_uses_contents_instead_of_extensions() {
    let directory = tempdir().unwrap();
    for name in [
        "wrong.jpg",
        "no-extension",
        "input.unsupported",
        "imagine-șț-日本語.png",
        "image with spaces.png",
    ] {
        let path = directory.path().join(name);
        fs::copy(fixture("rgba.png"), &path).unwrap();
        let image = input::load(&path, 1.0).unwrap();
        assert!(image.pixels().has_alpha());
        assert_eq!(image.pixels().dimensions(), (17, 11));
    }
    #[cfg(unix)]
    {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let path = directory
            .path()
            .join(OsString::from_vec(b"input-\xff.png".to_vec()));
        match fs::copy(fixture("rgb.png"), &path) {
            Ok(_) => {
                let image = input::load(&path, 1.0).unwrap();
                assert_eq!(image.pixels().dimensions(), (17, 11));
                assert!(!image.pixels().has_alpha());
            }
            // macOS filesystems can reject an invalid UTF-8 filename before
            // the loader runs (Darwin EILSEQ = 92). Only that fixture-creation
            // error is tolerated; loading errors and other I/O errors fail.
            #[cfg(target_os = "macos")]
            Err(error) if error.raw_os_error() == Some(92) => {
                eprintln!("raw-byte filename check unavailable: {error}");
            }
            Err(error) => panic!("cannot create raw-byte filename fixture: {error}"),
        }
    }
}

#[test]
fn alpha_and_opaque_alpha_sources_have_correct_layout() {
    for name in ["rgba.png", "rgba.webp"] {
        let loaded = input::load(&fixture(name), 1.0).unwrap();
        assert!(matches!(loaded.pixels(), Pixels::Rgba(_)));
        assert_eq!(loaded.pixels().as_bytes().len(), 17 * 11 * 4);
        let alphas: Vec<_> = loaded
            .pixels()
            .as_bytes()
            .chunks_exact(4)
            .map(|p| p[3])
            .collect();
        for row in alphas.chunks_exact(17) {
            assert_eq!(&row[..4], &[0, 64, 160, 255]);
        }
    }
    let rgba = input::load(&fixture("rgba.png"), 1.0).unwrap();
    // A no-resize pass preserves even the hidden RGB values exactly.
    assert_eq!(&rgba.pixels().as_bytes()[16..20], &[52, 0, 28, 0]);
    let opaque = input::load(&fixture("opaque-rgba.png"), 1.0).unwrap();
    let rgb = input::load(&fixture("rgb.png"), 1.0).unwrap();
    assert!(!opaque.pixels().has_alpha());
    assert_eq!(opaque.pixels().as_bytes(), rgb.pixels().as_bytes());
}

#[test]
fn grayscale_and_sixteen_bit_sources_normalize_to_eight_bits() {
    let gray = input::load(&fixture("gray.png"), 1.0).unwrap();
    assert!(!gray.pixels().has_alpha());
    assert_eq!(&gray.pixels().as_bytes()[..6], &[0, 0, 0, 70, 70, 70]);
    let gray_alpha = input::load(&fixture("gray-alpha.png"), 1.0).unwrap();
    assert!(gray_alpha.pixels().has_alpha());
    assert_eq!(
        gray_alpha.pixels().as_bytes(),
        &[12, 12, 12, 64, 12, 12, 12, 64]
    );
    let sixteen = input::load(&fixture("rgba16.png"), 1.0).unwrap();
    assert!(sixteen.pixels().has_alpha());
    assert_eq!(
        sixteen.pixels().as_bytes(),
        &[0, 255, 128, 64, 0, 255, 128, 64]
    );
}

#[test]
fn scaling_rounds_axes_and_handles_tiny_and_unchanged_images() {
    for name in ["rgb.png", "rgba.png", "rgb.jpg", "rgb.bmp", "rgb.webp"] {
        for (scale, dimensions) in [
            (1.0, (17, 11)),
            (0.5, (9, 6)),
            (0.1, (2, 1)),
            (1e-300, (1, 1)),
        ] {
            let loaded = input::load(&fixture(name), scale).unwrap();
            assert_eq!(loaded.original_dimensions(), (17, 11));
            assert_eq!(loaded.pixels().dimensions(), dimensions);
        }
        let full = input::load(&fixture(name), 1.0).unwrap();
        let rounded = input::load(&fixture(name), 0.999).unwrap();
        assert_eq!(full.pixels().as_bytes(), rounded.pixels().as_bytes());
    }
    for (name, alpha, expected) in [
        ("tiny-rgb.png", false, &[80, 120, 160][..]),
        ("tiny-rgba.png", true, &[80, 120, 160, 64][..]),
    ] {
        for scale in [1.0, 0.5, 1e-300] {
            let loaded = input::load(&fixture(name), scale).unwrap();
            assert_eq!(loaded.pixels().dimensions(), (1, 1));
            assert_eq!(loaded.pixels().has_alpha(), alpha);
            assert_eq!(loaded.pixels().as_bytes(), expected);
        }
    }
}

#[test]
fn bounds_use_the_same_single_resize_and_alpha_path_as_scale() {
    for name in [
        "rgb.png",
        "rgba.png",
        "rgb.jpg",
        "rgb.bmp",
        "rgb.webp",
        "alpha-edge.png",
        "tiny-rgba.png",
    ] {
        let original = input::load(&fixture(name), 1.0).unwrap();
        let width = original.original_dimensions().0;
        let bounds = input::load_with_resize(
            &fixture(name),
            Resize::Bounds {
                width: Some(width.div_ceil(2)),
                height: None,
            },
        )
        .unwrap();
        let scale = input::load(
            &fixture(name),
            f64::from(width.div_ceil(2)) / f64::from(width),
        )
        .unwrap();
        assert_eq!(bounds.original_dimensions(), original.original_dimensions());
        assert_eq!(bounds.pixels().dimensions(), scale.pixels().dimensions());
        assert_eq!(bounds.pixels().as_bytes(), scale.pixels().as_bytes());
        assert_eq!(bounds.pixels().has_alpha(), scale.pixels().has_alpha());

        let unchanged = input::load_with_resize(
            &fixture(name),
            Resize::Bounds {
                width: Some(u32::MAX),
                height: Some(u32::MAX),
            },
        )
        .unwrap();
        assert_eq!(
            unchanged.pixels().dimensions(),
            original.pixels().dimensions()
        );
        assert_eq!(unchanged.pixels().as_bytes(), original.pixels().as_bytes());
    }
    let wide = input::load_with_resize(
        &fixture("wide.png"),
        Resize::Bounds {
            width: Some(10000),
            height: Some(1),
        },
    )
    .unwrap();
    assert_eq!(wide.pixels().dimensions(), (10000, 1));
}

#[test]
fn bounds_validate_arguments_and_header_limits_before_decoding() {
    for resize in [
        Resize::Bounds {
            width: None,
            height: None,
        },
        Resize::Bounds {
            width: Some(0),
            height: Some(1),
        },
    ] {
        let error = input::load_with_resize(Path::new("missing.png"), resize).unwrap_err();
        assert!(!error.to_string().contains("cannot read input"));
    }
    for (name, width, message) in [
        ("oversized.bmp", 1, "maximum is 100000000"),
        ("over-budget.bmp", u32::MAX, "estimated working memory"),
        ("wide.bmp", 20000, "exceeds WebP's maximum"),
    ] {
        let error = input::load_with_resize(
            &fixture(name),
            Resize::Bounds {
                width: Some(width),
                height: None,
            },
        )
        .unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains(message), "{error}");
        assert!(!error.contains("cannot decode image pixels"), "{error}");
    }
}

#[test]
fn lanczos_resampling_preserves_constant_color_and_transparent_edges() {
    let edge = input::load(&fixture("alpha-edge.png"), 0.5).unwrap();
    assert_eq!(edge.pixels().dimensions(), (2, 1));
    assert!(edge.pixels().has_alpha());
    for pixel in edge.pixels().as_bytes().chunks_exact(4) {
        assert_eq!(&pixel[..3], &[255, 0, 0]);
        assert!(pixel[3] > 0 && pixel[3] < 255);
    }
    let invisible = input::load(&fixture("transparent.png"), 0.5).unwrap();
    assert!(invisible.pixels().has_alpha());
    assert!(invisible
        .pixels()
        .as_bytes()
        .chunks_exact(4)
        .all(|p| p[3] == 0));
    let constant = input::load(&fixture("wide.png"), 0.5).unwrap();
    assert_eq!(constant.pixels().dimensions(), (10_000, 1));
    assert!(constant
        .pixels()
        .as_bytes()
        .chunks_exact(3)
        .all(|p| p == [80, 120, 160]));
}

#[test]
fn animated_fixtures_have_two_frames_and_are_rejected() {
    let bytes = fs::read(fixture("animated.png")).unwrap();
    let png = image::codecs::png::PngDecoder::new(Cursor::new(bytes)).unwrap();
    assert!(png.is_apng().unwrap());
    assert_eq!(
        png.apng()
            .unwrap()
            .into_frames()
            .collect_frames()
            .unwrap()
            .len(),
        2
    );
    let bytes = fs::read(fixture("animated.webp")).unwrap();
    let webp = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).unwrap();
    assert!(webp.has_animation());
    assert_eq!(webp.into_frames().collect_frames().unwrap().len(), 2);
    for (name, message) in [
        ("animated.png", "animated PNG"),
        ("animated.webp", "animated WebP"),
    ] {
        let error = input::load(&fixture(name), 0.5).unwrap_err();
        assert!(format!("{error:#}").contains(message), "{error:#}");
    }
}

#[test]
fn unsupported_corrupt_and_truncated_inputs_fail() {
    for (name, message) in [
        ("unsupported.gif", "unsupported input format Gif"),
        ("corrupt.png", "cannot read PNG header"),
    ] {
        let error = input::load(&fixture(name), 1.0).unwrap_err();
        assert!(format!("{error:#}").contains(message), "{error:#}");
    }
    let directory = tempdir().unwrap();
    let path = directory.path().join("bad.png");
    for bytes in [&b""[..], &b"not an image"[..]] {
        fs::write(&path, bytes).unwrap();
        assert!(input::load(&path, 1.0).is_err());
    }
    for name in ["rgb.png", "rgb.jpg", "rgb.bmp", "rgb.webp"] {
        let bytes = fs::read(fixture(name)).unwrap();
        fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(input::load(&path, 1.0).is_err(), "truncated {name} decoded");
    }
    assert!(input::load(&directory.path().join("missing.png"), 1.0).is_err());
    for scale in [f64::NAN, f64::INFINITY, 0.0, -1.0, 2.0] {
        let error = input::load(&fixture("rgb.png"), scale).unwrap_err();
        assert!(error.to_string().contains("scale must be"));
    }
}

#[test]
fn header_limits_fail_before_missing_pixel_payload_is_read() {
    for (name, scale, message) in [
        ("oversized.bmp", 1e-3, "maximum is 100000000"),
        ("over-budget.bmp", 1.0, "estimated working memory"),
        ("wide.bmp", 1.0, "exceeds WebP's maximum"),
    ] {
        let error = input::load(&fixture(name), scale).unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains(message), "{error}");
        assert!(!error.contains("cannot decode image pixels"), "{error}");
    }
    // This header becomes acceptable after scaling, so it reaches the absent
    // payload and fails decoding. A real wide PNG succeeds in the resize test.
    let error = input::load(&fixture("wide.bmp"), 0.5).unwrap_err();
    assert!(format!("{error:#}").contains("cannot decode image pixels"));
}

#[test]
fn exif_orientation_is_applied_before_resizing() {
    let bytes = fs::read(fixture("oriented.jpg")).unwrap();
    let mut decoder = image::codecs::jpeg::JpegDecoder::new(Cursor::new(bytes)).unwrap();
    assert_eq!(
        decoder.orientation().unwrap(),
        image::metadata::Orientation::Rotate90
    );
    let oriented = input::load(&fixture("oriented.jpg"), 1.0).unwrap();
    let plain = input::load(&fixture("rgb.jpg"), 1.0).unwrap();
    assert_eq!(oriented.original_dimensions(), (17, 11));
    assert_eq!(oriented.pixels().dimensions(), (11, 17));
    let Pixels::Rgb(plain) = plain.pixels() else {
        panic!("expected RGB")
    };
    assert_eq!(
        oriented.pixels().as_bytes(),
        image::imageops::rotate90(plain).as_raw()
    );
    let bounded = input::load_with_resize(
        &fixture("oriented.jpg"),
        Resize::Bounds {
            width: Some(6),
            height: Some(8),
        },
    )
    .unwrap();
    assert_eq!(bounded.pixels().dimensions(), (5, 8));
}

#[test]
fn cli_reports_prepared_dimensions_writes_webp_and_preserves_source() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("input.png");
    let output = directory.path().join("output.webp");
    fs::copy(fixture("rgba.png"), &input).unwrap();
    let original = fs::read(&input).unwrap();
    fs::write(&output, b"existing output").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_webptune"))
        .arg("-i")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .args(["-s", "0.5"])
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("17x11 -> 9x6 RGBA8"));
    assert_eq!(fs::read(&input).unwrap(), original);
    let written =
        image::load_from_memory_with_format(&fs::read(&output).unwrap(), image::ImageFormat::WebP)
            .unwrap();
    assert_eq!((written.width(), written.height()), (9, 6));
    assert!(written.color().has_alpha());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn cli_ingestion_errors_leave_stdout_and_destination_untouched() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("input.data");
    let destination = directory.path().join("output.webp");
    let mut cases: Vec<_> = [
        ("animated.png", "animated PNG"),
        ("animated.webp", "animated WebP"),
        ("corrupt.png", "cannot read PNG header"),
        ("unsupported.gif", "unsupported input format"),
        ("oversized.bmp", "maximum is 100000000"),
        ("over-budget.bmp", "estimated working memory"),
        ("wide.bmp", "exceeds WebP's maximum"),
        ("wide.png", "exceeds WebP's maximum"),
    ]
    .into_iter()
    .map(|(name, message)| (name, fs::read(fixture(name)).unwrap(), message))
    .collect();
    cases.extend([
        (
            "empty",
            Vec::new(),
            "unsupported or unrecognized input format",
        ),
        (
            "unknown",
            b"not an image".to_vec(),
            "unsupported or unrecognized input format",
        ),
    ]);
    for name in ["rgb.png", "rgb.jpg", "rgb.bmp", "rgb.webp"] {
        let mut bytes = fs::read(fixture(name)).unwrap();
        bytes.truncate(bytes.len() / 2);
        cases.push((name, bytes, "cannot"));
    }
    for (name, source, message) in cases {
        fs::write(&input, &source).unwrap();
        for output in [destination.as_path(), Path::new("-")] {
            let result = Command::new(env!("CARGO_BIN_EXE_webptune"))
                .arg("-i")
                .arg(&input)
                .arg("-o")
                .arg(output)
                .output()
                .unwrap();
            assert_eq!(result.status.code(), Some(1));
            assert!(result.stdout.is_empty());
            assert!(
                String::from_utf8_lossy(&result.stderr).contains(message),
                "{name}: {result:?}"
            );
            assert!(!destination.exists());
            assert_eq!(fs::read(&input).unwrap(), source);
            assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        }
        fs::write(&destination, b"existing output").unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_webptune"))
            .arg("-i")
            .arg(&input)
            .arg("-o")
            .arg(&destination)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        assert_eq!(fs::read(&destination).unwrap(), b"existing output");
        assert_eq!(fs::read(&input).unwrap(), source);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
        fs::remove_file(&destination).unwrap();
    }
}
