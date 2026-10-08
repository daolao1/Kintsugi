//! Upscaling: from `nearest` to the Anime4K-inspired preset.
//!
//! All scalers are separable resamplers over RGBA (alpha carried through
//! bilinearly); the preset adds edge-adaptive sharpening on top. Scaling
//! factors are integers ≥ 1 (factor 1 = no-op copy for the plain methods,
//! and for the preset a pure sharpen pass).

use kintsugi_core::asset::Image;
use kintsugi_core::error::{Error, Result};

/// Available scalers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UpscaleMethod {
    /// Blocky nearest-neighbour — the "original pixels" reference.
    Nearest,
    /// Bilinear: smooth, soft, the honest baseline.
    Bilinear,
    /// Catmull-Rom bicubic: sharper than bilinear, no ringing to speak of.
    Bicubic,
    /// Lanczos-3: maximally sharp base, mild ringing on hard edges.
    Lanczos3,
    /// Anime4K-inspired preset (classic v0.9 recipe, no CNNs):
    /// Lanczos-3 base + edge-adaptive unsharp + luma-only sharpening, so
    /// flat cel areas stay clean and line-art gets crisp. See the crate
    /// docs for the honesty note on the name.
    ///
    /// Being Lanczos-based, it inherits that kernel's ringing halo: expect
    /// a faint overshoot a few pixels either side of a hard line (~1% of
    /// range from the sinc side-lobes). Uniform areas are reproduced
    /// *exactly*, so fades and flat cel fills never drift.
    Anime4K,
}

impl UpscaleMethod {
    /// All methods, in increasing order of fanciness.
    pub const ALL: &'static [UpscaleMethod] = &[
        UpscaleMethod::Nearest,
        UpscaleMethod::Bilinear,
        UpscaleMethod::Bicubic,
        UpscaleMethod::Lanczos3,
        UpscaleMethod::Anime4K,
    ];

    /// CLI-friendly name.
    pub fn as_str(&self) -> &'static str {
        match self {
            UpscaleMethod::Nearest => "nearest",
            UpscaleMethod::Bilinear => "bilinear",
            UpscaleMethod::Bicubic => "bicubic",
            UpscaleMethod::Lanczos3 => "lanczos3",
            UpscaleMethod::Anime4K => "anime4k",
        }
    }

    /// Parse a CLI name.
    pub fn from_name(name: &str) -> Result<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|m| m.as_str() == name.to_ascii_lowercase())
            .ok_or_else(|| {
                Error::unsupported(
                    "upscaler",
                    format!(
                        "unknown method '{name}' (known: {})",
                        UpscaleMethod::ALL
                            .iter()
                            .map(|m| m.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })
    }
}

/// Max upscale factor; 16× on a 4K frame is already absurd.
pub const MAX_FACTOR: u32 = 16;

/// Upscale `image` by integer `factor` with `method`.
pub fn upscale(image: &Image, factor: u32, method: UpscaleMethod) -> Result<Image> {
    if factor == 0 || factor > MAX_FACTOR {
        return Err(Error::unsupported(
            "upscaler",
            format!("factor {factor} outside 1..={MAX_FACTOR}"),
        ));
    }
    if factor == 1 && method != UpscaleMethod::Anime4K {
        return Ok(image.clone());
    }

    let base = match method {
        UpscaleMethod::Nearest => scale_nearest(image, factor),
        UpscaleMethod::Bilinear => resample(image, factor, Filter::Bilinear),
        UpscaleMethod::Bicubic => resample(image, factor, Filter::Bicubic),
        UpscaleMethod::Lanczos3 | UpscaleMethod::Anime4K => {
            resample(image, factor, Filter::Lanczos3)
        }
    };
    match method == UpscaleMethod::Anime4K {
        true => Ok(edge_adaptive_sharpen(&base)),
        false => Ok(base),
    }
}

fn scale_nearest(image: &Image, factor: u32) -> Image {
    let width = image.width * factor;
    let height = image.height * factor;
    let mut out = Image::new(width, height);
    let src_stride = image.width as usize * 4;
    let dst_stride = width as usize * 4;
    for y in 0..image.height as usize {
        let src_row = &image.data[y * src_stride..(y + 1) * src_stride];
        for x in 0..image.width as usize {
            let px = &src_row[x * 4..x * 4 + 4];
            for dy in 0..factor as usize {
                for dx in 0..factor as usize {
                    let dst =
                        (y * factor as usize + dy) * dst_stride + (x * factor as usize + dx) * 4;
                    out.data[dst..dst + 4].copy_from_slice(px);
                }
            }
        }
    }
    out
}

/// Separable resampling kernels.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Filter {
    Bilinear,
    Bicubic,
    Lanczos3,
}

impl Filter {
    fn radius(&self) -> f32 {
        match self {
            Filter::Bilinear => 1.0,
            Filter::Bicubic => 2.0,
            Filter::Lanczos3 => 3.0,
        }
    }

    /// Weight of source sample at distance `d` (≥ 0) from the target.
    fn weight(&self, d: f32) -> f32 {
        let d = d.abs();
        match self {
            Filter::Bilinear => {
                if d < 1.0 {
                    1.0 - d
                } else {
                    0.0
                }
            }
            Filter::Bicubic => catmull_rom(d),
            Filter::Lanczos3 => {
                if d < 1e-8 {
                    1.0
                } else if d < 3.0 {
                    let a = d * std::f32::consts::PI / 3.0;
                    let s = d * std::f32::consts::PI;
                    3.0 * a.sin() * s.sin() / (s * s)
                } else {
                    0.0
                }
            }
        }
    }
}

/// Catmull-Rom cubic kernel (a = −0.5).
fn catmull_rom(d: f32) -> f32 {
    if d < 1.0 {
        1.5 * d * d * d - 2.5 * d * d + 1.0
    } else if d < 2.0 {
        -0.5 * d * d * d + 2.5 * d * d - 4.0 * d + 2.0
    } else {
        0.0
    }
}

/// Horizontal pass, then vertical pass, both with clamped edges.
fn resample(image: &Image, factor: u32, filter: Filter) -> Image {
    let width = image.width as usize * factor as usize;
    let height = image.height as usize * factor as usize;
    let src_width = image.width as usize;
    let src_height = image.height as usize;
    let radius = filter.radius() as usize;
    let src_stride = src_width * 4;

    // --- Horizontal: src (w,h) → tmp (width, h)
    let mut tmp = vec![0f32; width * src_height * 4];
    let center_step = 1.0 / factor as f32;
    for sx_target in 0..width {
        let center = (sx_target as f32 + 0.5) * center_step - 0.5;
        let base = center.floor() as isize;
        let frac = center - center.floor();
        let mut weights: Vec<f32> = Vec::with_capacity(radius * 2 + 2);
        let mut indices: Vec<usize> = Vec::with_capacity(radius * 2 + 2);
        let mut total = 0f32;
        for k in -(radius as isize)..=(radius as isize + 1) {
            let source = base + k;
            let clamped = source.clamp(0, src_width as isize - 1) as usize;
            let weight = filter.weight((k as f32 - frac).abs());
            if weight != 0.0 {
                weights.push(weight);
                indices.push(clamped);
                total += weight;
            }
        }
        for y in 0..src_height {
            let row = &image.data[y * src_stride..(y + 1) * src_stride];
            let dst = (y * width + sx_target) * 4;
            for c in 0..4 {
                let mut acc = 0f32;
                for (w, idx) in weights.iter().zip(indices.iter()) {
                    acc += w * row[idx * 4 + c] as f32;
                }
                tmp[dst + c] = acc / total;
            }
        }
    }

    // --- Vertical: tmp (width, h) → out (width, height)
    let mut out = Image::new(width as u32, height as u32);
    let dst_stride = width * 4;
    for sy_target in 0..height {
        let center = (sy_target as f32 + 0.5) * center_step - 0.5;
        let base = center.floor() as isize;
        let frac = center - center.floor();
        let mut weights: Vec<f32> = Vec::with_capacity(radius * 2 + 2);
        let mut indices: Vec<usize> = Vec::with_capacity(radius * 2 + 2);
        let mut total = 0f32;
        for k in -(radius as isize)..=(radius as isize + 1) {
            let source = base + k;
            let clamped = source.clamp(0, src_height as isize - 1) as usize;
            let weight = filter.weight((k as f32 - frac).abs());
            if weight != 0.0 {
                weights.push(weight);
                indices.push(clamped);
                total += weight;
            }
        }
        let dst_row = sy_target * dst_stride;
        for x in 0..width {
            for c in 0..4 {
                let mut acc = 0f32;
                for (w, idx) in weights.iter().zip(indices.iter()) {
                    acc += w * tmp[(idx * width + x) * 4 + c];
                }
                out.data[dst_row + x * 4 + c] = (acc / total).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// Total edge-adaptive sharpening amount.
const SHARPEN_AMOUNT: f32 = 0.9;
/// Luma detail below which a pixel counts as "flat" (no sharpening).
const FLAT_THRESHOLD: f32 = 0.04;

/// The Anime4K-inspired finish: variance-adaptive unsharp on luma.
///
/// 1. Blur luma with a 3×3 gaussian (σ = 1) to find the low-frequency base.
/// 2. `detail = luma − blurred`; where |detail| is large (an edge, a line),
///    push the luma *away* from the blur by `SHARPEN_AMOUNT`;
///    where it is tiny (flat cel shading), leave it alone.
/// 3. Apply the luma delta to RGB equally, so hues never shift.
fn edge_adaptive_sharpen(image: &Image) -> Image {
    let width = image.width as usize;
    let height = image.height as usize;
    let stride = width * 4;

    let luma = |data: &[u8], x: usize, y: usize| -> f32 {
        let p = y * stride + x * 4;
        (data[p] as f32 * 0.299 + data[p + 1] as f32 * 0.587 + data[p + 2] as f32 * 0.114) / 255.0
    };

    let mut out = Image::new(image.width, image.height);
    out.data.copy_from_slice(&image.data);
    for y in 0..height {
        for x in 0..width {
            let center = luma(&image.data, x, y);
            let mut sum = 0f32;
            let mut weight_total = 0f32;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let yy = (y as i32 + dy).clamp(0, height as i32 - 1) as usize;
                    let xx = (x as i32 + dx).clamp(0, width as i32 - 1) as usize;
                    let w = if dx == 0 && dy == 0 {
                        4.0
                    } else if dx == 0 || dy == 0 {
                        2.0
                    } else {
                        1.0
                    };
                    sum += w * luma(&image.data, xx, yy);
                    weight_total += w;
                }
            }
            let blurred = sum / weight_total;
            let detail = center - blurred;
            // Adaptive strength: flats (|detail| ≈ 0) get none; lines get full.
            let strength = (detail.abs() / FLAT_THRESHOLD).min(1.0) * SHARPEN_AMOUNT;
            let delta = detail * strength;

            let p = y * stride + x * 4;
            for c in 0..3 {
                let channel = image.data[p + c] as f32 + delta * 255.0;
                out.data[p + c] = channel.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kintsugi_core::asset::ImageFormat;

    fn gradient(width: u32, height: u32) -> Image {
        let mut image = Image::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let p = ((y * width + x) * 4) as usize;
                image.data[p] = (x * 255 / (width - 1).max(1)) as u8;
                image.data[p + 1] = (y * 255 / (height - 1).max(1)) as u8;
                image.data[p + 2] = 128;
                image.data[p + 3] = 255;
            }
        }
        image
    }

    #[test]
    fn all_methods_produce_exact_dimensions() {
        let image = gradient(5, 4);
        for method in UpscaleMethod::ALL {
            let out = upscale(&image, 3, *method).unwrap();
            assert_eq!((out.width, out.height), (15, 12), "{method:?}");
            assert_eq!(out.format, ImageFormat::Rgba8);
        }
    }

    #[test]
    fn factor_bounds_are_enforced() {
        let image = gradient(2, 2);
        assert!(upscale(&image, 0, UpscaleMethod::Bilinear).is_err());
        assert!(upscale(&image, 17, UpscaleMethod::Bilinear).is_err());
    }

    #[test]
    fn bilinear_on_gradient_is_smooth() {
        // A pure horizontal gradient upsampled 4× must stay monotonic.
        let mut image = Image::new(3, 1);
        for (i, chunk) in image.data.chunks_exact_mut(4).enumerate() {
            let v = [i as u8 * 127, 0, 0, 255];
            chunk.copy_from_slice(&v);
        }
        let out = upscale(&image, 4, UpscaleMethod::Bilinear).unwrap();
        let mut last = -1i32;
        for x in 0..out.width as usize {
            let v = out.data[x * 4] as i32;
            assert!(v >= last, "gradient must not go backwards at {x}");
            last = v;
        }
    }

    #[test]
    fn anime4k_sharpens_lineart_more_than_bilinear() {
        // 8×8 white field with a one-pixel black line down column 3.
        let mut image = Image::new(8, 8);
        for chunk in image.data.chunks_exact_mut(4) {
            chunk.copy_from_slice(&[255, 255, 255, 255]);
        }
        for y in 0..8 {
            let p = (y * 8 + 3) * 4;
            image.data[p..p + 4].copy_from_slice(&[0, 0, 0, 255]);
        }

        // Strongest edge anywhere in the frame, summed over rows. Sampling a
        // fixed column would be a guess about where the 2x kernel lands;
        // line-art sharpening is really a claim about edge steepness.
        let contrast = |img: &Image| -> u32 {
            let width = img.width as usize;
            let mut total = 0u32;
            for y in 0..img.height as usize {
                let row = y * width * 4;
                let mut strongest = 0u32;
                for x in 1..width {
                    let left = img.data[row + (x - 1) * 4];
                    let right = img.data[row + x * 4];
                    strongest = strongest.max(left.abs_diff(right) as u32);
                }
                total += strongest;
            }
            total
        };

        let bilinear = upscale(&image, 2, UpscaleMethod::Bilinear).unwrap();
        let anime4k = upscale(&image, 2, UpscaleMethod::Anime4K).unwrap();
        assert!(
            contrast(&anime4k) > contrast(&bilinear),
            "the preset must out-contrast bilinear on line-art \
             ({} vs {})",
            contrast(&anime4k),
            contrast(&bilinear)
        );
        // The halo is faint and bounded: the sinc side-lobes of a Lanczos
        // base may dim the field by a few levels, never smear it. This is
        // the documented ringing, and this assertion is what keeps the
        // documentation honest.
        let row = 2 * 16 * 4;
        for x in 0..16 {
            if (5..=8).contains(&x) {
                continue; // the line body and its immediate edge
            }
            let v = anime4k.data[row + x * 4];
            assert!(
                v >= 240,
                "ringing at x={x} dipped to {v}; the preset promises a faint halo"
            );
        }
    }

    #[test]
    fn every_method_reproduces_a_constant_field_exactly() {
        // Partition-of-unity check: a resampler that loses weight at the
        // borders (or normalises wrongly) darkens or brightens the edges.
        for method in UpscaleMethod::ALL {
            for level in [0u8, 17, 128, 254, 255] {
                let mut flat = Image::new(5, 4);
                for chunk in flat.data.chunks_exact_mut(4) {
                    chunk.copy_from_slice(&[level, level, level, 255]);
                }
                let out = upscale(&flat, 3, *method).unwrap();
                for (i, px) in out.data.chunks_exact(4).enumerate() {
                    assert_eq!(
                        &px[0..3],
                        &[level, level, level],
                        "{} drifted at pixel {i} for level {level}",
                        method.as_str()
                    );
                    assert_eq!(px[3], 255, "{} touched alpha", method.as_str());
                }
            }
        }
    }

    #[test]
    fn anime4k_leaves_flat_areas_flat() {
        // A perfectly flat image must come through the preset untouched:
        // no edges, no sharpening, no drift.
        let mut image = Image::new(6, 6);
        for chunk in image.data.chunks_exact_mut(4) {
            chunk.copy_from_slice(&[120, 60, 200, 255]);
        }
        let out = upscale(&image, 1, UpscaleMethod::Anime4K).unwrap();
        assert_eq!(out.data, image.data);
    }

    #[test]
    fn alpha_is_carried_through() {
        let mut image = Image::new(2, 2);
        image.data[3] = 128;
        image.data[7] = 128;
        image.data[11] = 128;
        image.data[15] = 128;
        let out = upscale(&image, 3, UpscaleMethod::Lanczos3).unwrap();
        assert!(out.data.chunks_exact(4).all(|px| px[3] == 128));
    }

    #[test]
    fn method_names_roundtrip() {
        for method in UpscaleMethod::ALL {
            assert_eq!(UpscaleMethod::from_name(method.as_str()).unwrap(), *method);
        }
        assert!(UpscaleMethod::from_name("waifu2x-cpu").is_err());
    }
}
