//! Color/orientation oracles independent of application preparation and metric.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;

use fast_ssim2::{compute_ssimulacra2, PixelDescriptor, PixelSlice};
use image::{DynamicImage, ImageEncoder, ImageFormat, Rgba, Rgba32FImage};
use tempfile::tempdir;
use webptune::{input, limits::Resize};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn encode_srgb(linear: f64) -> f64 {
    if linear <= 0.0031308 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

fn decode_srgb(encoded: u8) -> f64 {
    let value = f64::from(encoded) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn quantize(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn close(actual: &[u8], expected: &[u8], tolerance: u8) {
    assert_eq!(actual.len(), expected.len());
    for (index, (&a, &e)) in actual.iter().zip(expected).enumerate() {
        assert!(a.abs_diff(e) <= tolerance, "sample {index}: {a} != {e}");
    }
}

#[test]
fn rgb_and_grayscale_icc_transform_to_srgb_without_changing_alpha() {
    for name in ["linear-icc.jpg", "linear-icc.webp"] {
        // image's ordinary decoder gives stored samples without an ICC transform.
        let raw = image::open(fixture(name)).unwrap().to_rgb8();
        let expected: Vec<_> = raw
            .as_raw()
            .iter()
            .map(|&v| quantize(encode_srgb(f64::from(v) / 255.0)))
            .collect();
        let prepared = input::load(&fixture(name), 1.0).unwrap();
        assert!(!prepared.pixels().has_alpha());
        close(prepared.pixels().as_bytes(), &expected, 1);
    }
    let gray = input::load(&fixture("gray-linear-icc.png"), 1.0).unwrap();
    assert!(gray.pixels().has_alpha());
    close(
        gray.pixels().as_bytes(),
        &[137, 137, 137, 64, 188, 188, 188, 160],
        1,
    );
    let plain = input::load(&fixture("rgba.png"), 1.0).unwrap();
    let srgb = input::load(&fixture("srgb-icc.png"), 1.0).unwrap();
    close(srgb.pixels().as_bytes(), plain.pixels().as_bytes(), 1);
    assert!(srgb
        .pixels()
        .as_bytes()
        .chunks_exact(4)
        .map(|p| p[3])
        .eq(plain.pixels().as_bytes().chunks_exact(4).map(|p| p[3])));
}

#[test]
fn display_p3_matches_an_independent_linear_matrix_and_clips_gamut() {
    let stored = image::open(fixture("display-p3.png")).unwrap().to_rgba8();
    let prepared = input::load(&fixture("display-p3.png"), 1.0).unwrap();
    let mut expected = Vec::new();
    for pixel in stored.pixels() {
        let [r, g, b] = [pixel[0], pixel[1], pixel[2]].map(decode_srgb);
        // D65 Display P3 -> linear sRGB. ICC's D50 chromatic adaptation and
        // fixed-point serialized colorants introduce small rounding differences.
        for value in [
            1.224745 * r - 0.224904 * g,
            -0.042058 * r + 1.042081 * g,
            -0.019642 * r - 0.078655 * g + 1.098537 * b,
        ] {
            expected.push(quantize(encode_srgb(value.max(0.0))));
        }
        expected.push(pixel[3]);
    }
    close(prepared.pixels().as_bytes(), &expected, 2);
    assert_ne!(prepared.pixels().as_bytes(), stored.as_raw());
    assert!(prepared
        .pixels()
        .as_bytes()
        .chunks_exact(4)
        .map(|p| p[3])
        .eq(stored.pixels().map(|p| p[3])));
}

#[test]
fn sixteen_bit_color_conversion_and_resize_precede_eight_bit_quantization() {
    let stored = image::open(fixture("linear16.png")).unwrap().to_rgba16();
    let expected = Rgba32FImage::from_fn(4, 2, |x, y| {
        let p = stored.get_pixel(x, y);
        Rgba([
            encode_srgb(f64::from(p[0]) / 65535.0) as f32,
            encode_srgb(f64::from(p[1]) / 65535.0) as f32,
            encode_srgb(f64::from(p[2]) / 65535.0) as f32,
            f32::from(p[3]) / 65535.0,
        ])
    });
    let full = input::load(&fixture("linear16.png"), 1.0).unwrap();
    close(
        full.pixels().as_bytes(),
        DynamicImage::ImageRgba32F(expected.clone())
            .to_rgba8()
            .as_raw(),
        1,
    );
    // Constant alpha makes straight and premultiplied filtering equivalent.
    let resized = image::imageops::resize(&expected, 2, 1, image::imageops::FilterType::Lanczos3);
    let small = input::load(&fixture("linear16.png"), 0.5).unwrap();
    close(
        small.pixels().as_bytes(),
        DynamicImage::ImageRgba32F(resized).to_rgba8().as_raw(),
        1,
    );

    // Values on either side of an 8-bit boundary would collapse differently if
    // the source were quantized before filtering. Search a tiny fixed family.
    let directory = tempdir().unwrap();
    let path = directory.path().join("precision.png");
    let mut distinguished = false;
    for seed in 0..16_u32 {
        let source = image::ImageBuffer::from_fn(7, 5, |x, y| {
            let value = (x * 15_731 + y * 28_753 + seed * 7919) as u16;
            image::Rgb([value; 3])
        });
        source.save(&path).unwrap();
        let float = DynamicImage::ImageRgb16(source.clone()).into_rgb32f();
        let expected = DynamicImage::ImageRgb32F(image::imageops::resize(
            &float,
            4,
            3,
            image::imageops::FilterType::Lanczos3,
        ))
        .to_rgb8();
        let early = image::imageops::resize(
            &DynamicImage::ImageRgb16(source).to_rgb8(),
            4,
            3,
            image::imageops::FilterType::Lanczos3,
        );
        let actual = input::load(&path, 0.5).unwrap();
        close(actual.pixels().as_bytes(), expected.as_raw(), 0);
        distinguished |= expected != early;
    }
    assert!(
        distinguished,
        "oracle must distinguish premature quantization"
    );
}

fn exif(orientation: u8) -> Vec<u8> {
    let mut exif = b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x01\0\0\0\0\0\0\0".to_vec();
    exif[18] = orientation;
    exif
}

#[test]
fn all_eight_exif_transforms_work_in_jpeg_png_and_webp() {
    let directory = tempdir().unwrap();
    for format in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::WebP] {
        let source = image::open(fixture("rgba.png")).unwrap();
        for orientation in 1..=8 {
            let mut bytes = Vec::new();
            let color = if format == ImageFormat::Jpeg {
                let rgb = source.to_rgb8();
                let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut bytes);
                encoder.set_exif_metadata(exif(orientation)).unwrap();
                encoder
                    .write_image(rgb.as_raw(), 17, 11, image::ExtendedColorType::Rgb8)
                    .unwrap();
                3
            } else {
                let rgba = source.to_rgba8();
                if format == ImageFormat::Png {
                    let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
                    encoder.set_exif_metadata(exif(orientation)).unwrap();
                    encoder
                        .write_image(rgba.as_raw(), 17, 11, image::ExtendedColorType::Rgba8)
                        .unwrap();
                } else {
                    let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut bytes);
                    encoder.set_exif_metadata(exif(orientation)).unwrap();
                    encoder
                        .write_image(rgba.as_raw(), 17, 11, image::ExtendedColorType::Rgba8)
                        .unwrap();
                }
                4
            };
            let raw = image::load_from_memory_with_format(&bytes, format).unwrap();
            let raw = if color == 3 {
                raw.to_rgb8().into_raw()
            } else {
                raw.to_rgba8().into_raw()
            };
            let path = directory.path().join("input.data");
            fs::write(&path, bytes).unwrap();
            let prepared = input::load(&path, 1.0).unwrap();
            let (width, height) = if orientation >= 5 { (11, 17) } else { (17, 11) };
            assert_eq!(prepared.pixels().dimensions(), (width, height));
            let mut expected = Vec::new();
            for y in 0..height {
                for x in 0..width {
                    let (sx, sy) = match orientation {
                        1 => (x, y),
                        2 => (16 - x, y),
                        3 => (16 - x, 10 - y),
                        4 => (x, 10 - y),
                        5 => (y, x),
                        6 => (y, 10 - x),
                        7 => (16 - y, 10 - x),
                        8 => (16 - y, x),
                        _ => unreachable!(),
                    };
                    let offset = (sy * 17 + sx) as usize * color;
                    expected.extend_from_slice(&raw[offset..offset + color]);
                }
            }
            assert_eq!(
                prepared.pixels().as_bytes(),
                expected,
                "{format:?} EXIF {orientation}"
            );
            let bounded = input::load_with_resize(
                &path,
                Resize::Bounds {
                    width: Some(6),
                    height: Some(8),
                },
            )
            .unwrap();
            assert_eq!(
                bounded.pixels().dimensions(),
                if orientation >= 5 { (5, 8) } else { (6, 4) }
            );
        }
    }
}

#[test]
fn radiance_hdr_is_reinhard_tone_mapped_then_encoded_as_srgb() {
    let prepared = input::load(&fixture("linear.hdr"), 1.0).unwrap();
    assert_eq!(prepared.pixels().dimensions(), (8, 2));
    assert!(!prepared.pixels().has_alpha());
    let mut expected = Vec::new();
    for _ in 0..2 {
        for value in [0.0, 0.25, 0.5, 1.0, 2.0, 4.0, 16.0, 128.0] {
            expected.extend_from_slice(&[quantize(encode_srgb(value / (1.0 + value))); 3]);
        }
    }
    close(prepared.pixels().as_bytes(), &expected, 0);
    assert_eq!(
        input::load(&fixture("linear.hdr"), 0.5)
            .unwrap()
            .pixels()
            .dimensions(),
        (4, 1)
    );
}

fn write_png(path: &Path, mutate: impl FnOnce(&mut png::Info<'static>)) {
    let mut info = png::Info::with_size(2, 1);
    info.color_type = png::ColorType::Rgb;
    info.bit_depth = png::BitDepth::Eight;
    mutate(&mut info);
    // png 0.18 reads cICP but does not emit this Info field automatically.
    let cicp = info.coding_independent_code_points;
    let mut writer = png::Encoder::with_info(File::create(path).unwrap(), info)
        .unwrap()
        .write_header()
        .unwrap();
    if let Some(cicp) = cicp {
        writer
            .write_chunk(
                png::chunk::cICP,
                &[
                    cicp.color_primaries,
                    cicp.transfer_function,
                    cicp.matrix_coefficients,
                    u8::from(cicp.is_video_full_range_image),
                ],
            )
            .unwrap();
    }
    writer
        .write_image_data(&[64, 128, 192, 128, 64, 32])
        .unwrap();
    writer.finish().unwrap();
}

#[test]
fn png_color_precedence_cicp_icc_srgb_gamma_and_chromaticities() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("input.png");
    let linear_profile = moxcms::ColorProfile::new_gray_with_gamma(1.0)
        .encode()
        .unwrap();
    // cICP overrides even an incompatible ICC; full-range sRGB is unchanged.
    write_png(&path, |info| {
        info.coding_independent_code_points = Some(png::CodingIndependentCodePoints {
            color_primaries: 1,
            transfer_function: 13,
            matrix_coefficients: 0,
            is_video_full_range_image: true,
        });
        info.icc_profile = Some(linear_profile.into());
        info.source_gamma = Some(png::ScaledFloat::new(1.0));
    });
    close(
        input::load(&path, 1.0).unwrap().pixels().as_bytes(),
        &[64, 128, 192, 128, 64, 32],
        1,
    );
    write_png(&path, |info| {
        info.srgb = Some(png::SrgbRenderingIntent::Perceptual);
        info.source_gamma = Some(png::ScaledFloat::new(1.0));
    });
    close(
        input::load(&path, 1.0).unwrap().pixels().as_bytes(),
        &[64, 128, 192, 128, 64, 32],
        0,
    );
    write_png(&path, |info| {
        info.source_gamma = Some(png::ScaledFloat::new(1.0));
    });
    close(
        input::load(&path, 1.0).unwrap().pixels().as_bytes(),
        &[137, 188, 225, 188, 137, 99],
        1,
    );
    write_png(&path, |info| {
        info.source_gamma = Some(png::ScaledFloat::new(0.45455));
        info.source_chromaticities = Some(png::SourceChromaticities::new(
            (0.3127, 0.3290),
            (0.64, 0.33),
            (0.3, 0.6),
            (0.15, 0.06),
        ));
    });
    let expected: Vec<_> = [64_u8, 128, 192, 128, 64, 32]
        .iter()
        .map(|&v| quantize(encode_srgb((f64::from(v) / 255.0).powf(1.0 / 0.45455))))
        .collect();
    close(
        input::load(&path, 1.0).unwrap().pixels().as_bytes(),
        &expected,
        1,
    );
    // ICC takes priority over a conflicting gAMA chunk.
    write_png(&path, |info| {
        info.icc_profile = Some(moxcms::ColorProfile::new_srgb().encode().unwrap().into());
        info.source_gamma = Some(png::ScaledFloat::new(1.0));
    });
    close(
        input::load(&path, 1.0).unwrap().pixels().as_bytes(),
        &[64, 128, 192, 128, 64, 32],
        1,
    );
    // Non-sRGB cICP primaries use their own matrix, even with sRGB transfer.
    write_png(&path, |info| {
        info.coding_independent_code_points = Some(png::CodingIndependentCodePoints {
            color_primaries: 12,
            transfer_function: 13,
            matrix_coefficients: 0,
            is_video_full_range_image: true,
        });
    });
    let from_cicp = input::load(&path, 1.0).unwrap();
    write_png(&path, |info| {
        info.icc_profile = Some(
            moxcms::ColorProfile::new_display_p3()
                .encode()
                .unwrap()
                .into(),
        )
    });
    close(
        from_cicp.pixels().as_bytes(),
        input::load(&path, 1.0).unwrap().pixels().as_bytes(),
        1,
    );
}

#[test]
fn orientation_and_float_preparation_have_predecode_memory_guards() {
    use webptune::limits::{validate_preparation_dimensions, validate_resize_dimensions};
    assert!(
        validate_preparation_dimensions((1, 1), (1, 1), (1, 1), false, usize::MAX, true, true)
            .is_err()
    );
    // Both are safe under the old decode/resize estimate. Extra orientation
    // copies or float buffers must be rejected without allocating these images.
    assert!(validate_resize_dimensions((10_000, 7_000), (1, 1), false).is_ok());
    let error = validate_preparation_dimensions(
        (10_000, 7_000),
        (7_000, 10_000),
        (1, 1),
        false,
        8,
        true,
        false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("orientation/color buffers"));
    assert!(validate_resize_dimensions((7_000, 4_000), (1, 1), true).is_ok());
    let error = validate_preparation_dimensions(
        (7_000, 4_000),
        (7_000, 4_000),
        (1, 1),
        true,
        8,
        false,
        true,
    )
    .unwrap_err();
    assert!(error.to_string().contains("orientation/color buffers"));
    assert!(validate_preparation_dimensions(
        (2_000, 2_000),
        (2_000, 2_000),
        (2_000, 2_000),
        true,
        8,
        false,
        true
    )
    .is_err());
    assert!(validate_preparation_dimensions(
        (2_000, 2_000),
        (2_000, 2_000),
        (1_000, 1_000),
        true,
        8,
        true,
        true
    )
    .is_ok());
}

#[test]
fn invalid_profiles_and_unsupported_hdr_descriptions_fail_without_writing() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("input.data");
    let destination = directory.path().join("output.webp");
    let mut bad = Vec::new();
    for profile in [
        b"not an ICC profile".to_vec(),
        moxcms::ColorProfile::new_gray_with_gamma(1.0)
            .encode()
            .unwrap(),
    ] {
        write_png(&path, |info| info.icc_profile = Some(profile.into()));
        bad.push((fs::read(&path).unwrap(), "ICC"));
    }
    for transfer in [16, 18, 255] {
        write_png(&path, |info| {
            info.coding_independent_code_points = Some(png::CodingIndependentCodePoints {
                color_primaries: 9,
                transfer_function: transfer,
                matrix_coefficients: 0,
                is_video_full_range_image: true,
            })
        });
        bad.push((
            fs::read(&path).unwrap(),
            if transfer == 255 { "cICP" } else { "PQ/HLG" },
        ));
    }
    // Valid but unsupported/mismatched profiles and bounded parser resources.
    let mut cmyk = moxcms::ColorProfile::new_srgb().encode().unwrap();
    cmyk[16..20].copy_from_slice(b"CMYK");
    write_png(&path, |info| info.icc_profile = Some(cmyk.into()));
    bad.push((fs::read(&path).unwrap(), "ICC"));
    write_png(&path, |info| {
        info.icc_profile = Some(vec![0_u8; 4 * 1024 * 1024 + 1].into())
    });
    bad.push((fs::read(&path).unwrap(), "ICC profile exceeds"));
    for (primaries, matrix, full) in [(255, 0, true), (1, 1, true), (1, 0, false)] {
        write_png(&path, |info| {
            info.coding_independent_code_points = Some(png::CodingIndependentCodePoints {
                color_primaries: primaries,
                transfer_function: 13,
                matrix_coefficients: matrix,
                is_video_full_range_image: full,
            })
        });
        bad.push((
            fs::read(&path).unwrap(),
            if matrix != 0 {
                "cannot read PNG header"
            } else {
                "cICP"
            },
        ));
    }
    bad.push((b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\nPRIMARIES=0.64 0.33 0.30 0.60 0.15 0.06 0.3127 0.3290\n\n-Y 2 +X 8\n".to_vec(), "PRIMARIES"));
    bad.push((
        b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 10000 +X 10001\n".to_vec(),
        "maximum is 100000000",
    ));
    for (bytes, message) in bad {
        fs::write(&path, &bytes).unwrap();
        for output in [destination.as_path(), Path::new("-")] {
            fs::write(&destination, b"old output").unwrap();
            let result = Command::new(env!("CARGO_BIN_EXE_webptune"))
                .arg("-i")
                .arg(&path)
                .arg("-o")
                .arg(output)
                .arg("--quiet")
                .output()
                .unwrap();
            assert_eq!(result.status.code(), Some(1), "{result:?}");
            assert!(result.stdout.is_empty());
            assert!(
                String::from_utf8_lossy(&result.stderr).contains(message),
                "{result:?}"
            );
            assert_eq!(fs::read(&destination).unwrap(), b"old output");
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
        }
    }
}

#[test]
fn processed_references_are_independently_scored_in_single_and_batch_output() {
    let directory = tempdir().unwrap();
    let sources = directory.path().join("sources");
    let outputs = directory.path().join("outputs");
    fs::create_dir(&sources).unwrap();
    fs::create_dir(&outputs).unwrap();
    for name in [
        "oriented.jpg",
        "display-p3.png",
        "linear16.png",
        "linear.hdr",
    ] {
        let source = sources.join(name);
        let original = fs::read(fixture(name)).unwrap();
        fs::write(&source, &original).unwrap();
        let reference = input::load_with_resize(
            &source,
            Resize::Bounds {
                width: Some(8),
                height: Some(6),
            },
        )
        .unwrap();
        let single = Command::new(env!("CARGO_BIN_EXE_webptune"))
            .arg("-i")
            .arg(&source)
            .args([
                "-o",
                "-",
                "-t",
                "100",
                "--scale-width",
                "8",
                "--scale-height",
                "6",
                "--quiet",
            ])
            .output()
            .unwrap();
        assert!(single.status.success(), "{single:?}");
        assert!(single.stderr.is_empty());
        let decoded =
            image::load_from_memory_with_format(&single.stdout, ImageFormat::WebP).unwrap();
        let mut metadata =
            image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(&single.stdout)).unwrap();
        use image::ImageDecoder;
        assert!(metadata.icc_profile().unwrap().is_none());
        assert_eq!(
            metadata.orientation().unwrap(),
            image::metadata::Orientation::NoTransforms
        );
        let pixels = reference.pixels();
        let (width, height) = pixels.dimensions();
        assert_eq!((decoded.width(), decoded.height()), (width, height));
        let (candidate, channels, descriptor) = if pixels.has_alpha() {
            (
                decoded.to_rgba8().into_raw(),
                4,
                PixelDescriptor::RGBA8_SRGB,
            )
        } else {
            (decoded.to_rgb8().into_raw(), 3, PixelDescriptor::RGB8_SRGB)
        };
        let a = PixelSlice::new(
            pixels.as_bytes(),
            width,
            height,
            width as usize * channels,
            descriptor,
        )
        .unwrap();
        let b = PixelSlice::new(
            &candidate,
            width,
            height,
            width as usize * channels,
            descriptor,
        )
        .unwrap();
        let score = compute_ssimulacra2(&a, &b).unwrap();
        assert!(score.is_finite() && score >= 100.0, "{name}: {score}");
        fs::write(
            directory
                .path()
                .join(Path::new(name).with_extension("webp")),
            &single.stdout,
        )
        .unwrap();
        assert_eq!(fs::read(&source).unwrap(), original);
    }
    // linear16.png and linear.hdr have distinct stems and no output collisions.
    let batch = Command::new(env!("CARGO_BIN_EXE_webptune"))
        .arg("--batch")
        .arg("-i")
        .arg(&sources)
        .arg("-o")
        .arg(&outputs)
        .args([
            "--jobs",
            "2",
            "-t",
            "100",
            "--scale-width",
            "8",
            "--scale-height",
            "6",
            "--quiet",
        ])
        .output()
        .unwrap();
    assert!(batch.status.success(), "{batch:?}");
    assert!(batch.stdout.is_empty() && batch.stderr.is_empty());
    for name in [
        "oriented.webp",
        "display-p3.webp",
        "linear16.webp",
        "linear.webp",
    ] {
        assert_eq!(
            fs::read(outputs.join(name)).unwrap(),
            fs::read(directory.path().join(name)).unwrap()
        );
    }
}
