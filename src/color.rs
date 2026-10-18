//! Convert supported source color descriptions to an SDR sRGB reference.

use std::io::{BufRead, Seek, SeekFrom};

use anyhow::{ensure, Context, Result};
use image::{DynamicImage, Rgba32FImage};
use moxcms::{
    curve_from_gamma, Chromaticity, CicpProfile, ColorPrimaries, ColorProfile, DataColorSpace,
    Layout, MatrixCoefficients, ParsingOptions, RenderingIntent, TransformOptions,
};

const MAX_PROFILE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct SourceColor {
    profile: Option<ColorProfile>,
    pub(crate) hdr: bool,
}

impl SourceColor {
    pub(crate) fn icc(bytes: Option<Vec<u8>>) -> Result<Self> {
        let Some(bytes) = bytes else {
            return Ok(Self::default());
        };
        ensure!(
            bytes.len() <= MAX_PROFILE_BYTES,
            "ICC profile exceeds 4 MiB limit"
        );
        let profile = ColorProfile::new_from_slice_with_options(
            &bytes,
            ParsingOptions {
                max_profile_size: MAX_PROFILE_BYTES,
                max_allowed_clut_size: 1024 * 1024,
                max_allowed_trc_size: 65_536,
            },
        )
        .context("invalid or unsupported ICC profile")?;
        ensure!(
            matches!(profile.color_space, DataColorSpace::Rgb | DataColorSpace::Gray),
            "unsupported ICC color space {:?}; provide RGB or grayscale input with a matching profile",
            profile.color_space
        );
        if let Some(cicp) = profile.cicp {
            reject_hdr_transfer(cicp.transfer_characteristics as u8)?;
        }
        Ok(Self {
            profile: Some(profile),
            hdr: false,
        })
    }

    /// Inspect PNG metadata without decoding pixels, then rewind the same reader.
    /// cICP > iCCP > sRGB > gAMA/cHRM; absent descriptions assume sRGB.
    pub(crate) fn png<R: BufRead + Seek>(reader: &mut R) -> Result<Self> {
        let color = {
            let mut decoder = png::Decoder::new_with_limits(
                &mut *reader,
                png::Limits {
                    bytes: 16 * 1024 * 1024,
                },
            );
            decoder.set_ignore_text_chunk(true);
            let png = decoder.read_info().context("cannot read PNG header")?;
            let info = png.info();
            if let Some(cicp) = info.coding_independent_code_points {
                ensure!(
                    cicp.matrix_coefficients == 0 && cicp.is_video_full_range_image,
                    "unsupported PNG cICP matrix/range; provide full-range RGB"
                );
                reject_hdr_transfer(cicp.transfer_function)?;
                let mut profile = match cicp.color_primaries {
                    1 => ColorProfile::new_srgb(),
                    9 => ColorProfile::new_bt2020(),
                    11 => ColorProfile::new_dci_p3(),
                    12 => ColorProfile::new_display_p3(),
                    _ => anyhow::bail!(
                        "unsupported PNG cICP primaries; supported codes: 1, 9, 11, 12"
                    ),
                };
                let description = CicpProfile {
                    color_primaries: cicp
                        .color_primaries
                        .try_into()
                        .context("unsupported PNG cICP primaries")?,
                    transfer_characteristics: cicp
                        .transfer_function
                        .try_into()
                        .context("unsupported PNG cICP transfer")?,
                    matrix_coefficients: MatrixCoefficients::Identity,
                    full_range: true,
                };
                // Construct explicitly: moxcms 0.8.1's CICP update helper
                // returns false even after a successful update.
                let curve = moxcms::ToneReprCurve::try_from(description.transfer_characteristics)
                    .context("unsupported PNG cICP transfer curve")?;
                profile.red_trc = Some(curve.clone());
                profile.green_trc = Some(curve.clone());
                profile.blue_trc = Some(curve);
                profile.cicp = Some(description);
                Self {
                    profile: Some(profile),
                    hdr: false,
                }
            } else if let Some(icc) = &info.icc_profile {
                Self::icc(Some(icc.to_vec()))?
            } else if info.srgb.is_some() {
                Self::default()
            } else if info.gamma().is_some() || info.chromaticities().is_some() {
                let mut profile = ColorProfile::new_srgb();
                profile.cicp = None;
                if let Some(gamma) = info.gamma() {
                    let gamma = gamma.into_value();
                    ensure!(gamma.is_finite() && gamma > 0.0, "invalid PNG gAMA value");
                    let curve = curve_from_gamma(1.0 / gamma);
                    profile.red_trc = Some(curve.clone());
                    profile.green_trc = Some(curve.clone());
                    profile.blue_trc = Some(curve);
                }
                if let Some(chroma) = info.chromaticities() {
                    let xy = |pair: (png::ScaledFloat, png::ScaledFloat)| -> Result<Chromaticity> {
                        let (x, y) = (pair.0.into_value(), pair.1.into_value());
                        ensure!(
                            x > 0.0 && y > 0.0 && x + y <= 1.00001,
                            "invalid PNG cHRM coordinates"
                        );
                        Ok(Chromaticity::new(x, y))
                    };
                    profile.update_rgb_colorimetry(
                        xy(chroma.white)?.to_xyyb(),
                        ColorPrimaries {
                            red: xy(chroma.red)?,
                            green: xy(chroma.green)?,
                            blue: xy(chroma.blue)?,
                        },
                    );
                    ensure!(
                        profile
                            .colorant_matrix()
                            .v
                            .iter()
                            .flatten()
                            .all(|v| v.is_finite()),
                        "invalid PNG cHRM matrix"
                    );
                }
                Self {
                    profile: Some(profile),
                    hdr: false,
                }
            } else {
                Self::default()
            }
        };
        reader
            .seek(SeekFrom::Start(0))
            .context("cannot rewind PNG input")?;
        Ok(color)
    }

    pub(crate) fn needs_float(&self, high_precision: bool) -> bool {
        self.profile.is_some() || self.hdr || high_precision
    }

    pub(crate) fn convert(&self, source: DynamicImage) -> Result<DynamicImage> {
        let grayscale = matches!(
            source.color(),
            image::ColorType::L8
                | image::ColorType::La8
                | image::ColorType::L16
                | image::ColorType::La16
        );
        let mut rgba = source.into_rgba32f();
        if let Some(profile) = &self.profile {
            ensure!(
                profile.color_space != DataColorSpace::Gray || grayscale,
                "grayscale ICC profile does not match RGB input pixels"
            );
            let gray = profile.color_space == DataColorSpace::Gray;
            let transform = profile
                .create_transform_f32(
                    if gray { Layout::Gray } else { Layout::Rgb },
                    &ColorProfile::new_srgb(),
                    Layout::Rgb,
                    TransformOptions {
                        rendering_intent: RenderingIntent::RelativeColorimetric,
                        prefer_fixed_point: false,
                        ..Default::default()
                    },
                )
                .context("cannot build ICC/color transform to sRGB")?;
            // Bounded row scratch; alpha is never fed through the color transform.
            let mut src = vec![0.0; rgba.width() as usize * if gray { 1 } else { 3 }];
            let mut dst = vec![0.0; rgba.width() as usize * 3];
            let row_len = rgba.width() as usize * 4;
            for row in rgba.as_mut().chunks_exact_mut(row_len) {
                for (pixel, channels) in
                    row.chunks_exact(4)
                        .zip(src.chunks_exact_mut(if gray { 1 } else { 3 }))
                {
                    channels.copy_from_slice(&pixel[..channels.len()]);
                }
                transform
                    .transform(&src, &mut dst)
                    .context("cannot transform source colors to sRGB")?;
                for (pixel, channels) in row.chunks_exact_mut(4).zip(dst.chunks_exact(3)) {
                    pixel[..3].copy_from_slice(channels);
                }
            }
        }
        if self.hdr {
            tone_map(&mut rgba)?;
        }
        for pixel in rgba.pixels_mut() {
            ensure!(
                pixel.0.iter().all(|v| v.is_finite()),
                "color conversion produced non-finite samples"
            );
            for channel in &mut pixel.0 {
                *channel = channel.clamp(0.0, 1.0);
            }
        }
        Ok(DynamicImage::ImageRgba32F(rgba))
    }
}

fn reject_hdr_transfer(transfer: u8) -> Result<()> {
    ensure!(!matches!(transfer, 16 | 18),
        "PQ/HLG color descriptions are not supported; convert to linear sRGB Radiance HDR (.hdr) for tone mapping");
    Ok(())
}

/// Fixed-exposure, luminance-preserving Reinhard in linear sRGB, then sRGB OETF.
/// Stored Radiance samples are used directly: no automatic exposure adaptation.
fn tone_map(image: &mut Rgba32FImage) -> Result<()> {
    for pixel in image.pixels_mut() {
        ensure!(
            pixel.0.iter().all(|v| v.is_finite() && *v >= 0.0),
            "HDR input contains negative or non-finite samples"
        );
        let luminance = 0.2126_f64 * f64::from(pixel[0])
            + 0.7152_f64 * f64::from(pixel[1])
            + 0.0722_f64 * f64::from(pixel[2]);
        for channel in &mut pixel.0[..3] {
            let linear = (f64::from(*channel) / (1.0 + luminance)).clamp(0.0, 1.0);
            *channel = if linear <= 0.0031308 {
                12.92 * linear
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            } as f32;
        }
    }
    Ok(())
}
