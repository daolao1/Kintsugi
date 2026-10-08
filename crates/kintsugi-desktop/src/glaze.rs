//! The desktop shell's glaze: firing the art at the frame's density.
//!
//! A 2008 background is 800×600 of carefully drawn pixels; a modern window
//! is a framebuffer two or three times denser. Shown at their own size the
//! scenes sit small in the middle of the glass. The glaze fires them to the
//! largest integer scale the frame fits — integers, because a fractional
//! resample of pixel-drawn line-art is a blur the original artist never
//! painted — with the video crate's edge-adaptive preset, so flat colour
//! stays flat and the lines get their crispness back.
//!
//! The glaze caches what it fires: a scene changes its background far less
//! often than a window repaints, and the unsharp mask is priced per pixel.

use std::collections::HashMap;
use std::sync::Arc;

use kintsugi_core::asset::Image;
use kintsugi_video::upscale::{UpscaleMethod, upscale};

/// The preset the desktop glaze fires with.
const METHOD: UpscaleMethod = UpscaleMethod::Anime4K;

/// The largest integer factor at which `image` still fits the frame, in
/// `1..=4`. Past four the seams would outnumber the picture.
pub fn factor_for(image: &Image, frame_width: u32, frame_height: u32) -> u32 {
    let by_width = frame_width / image.width.max(1);
    let by_height = frame_height / image.height.max(1);
    by_width.min(by_height).clamp(1, 4)
}

/// A firing kiln with a memory: the same art at the same scale comes out
/// the same, so it comes out of the cache.
#[derive(Default)]
pub struct Glaze {
    // Keyed by the image's allocation and the factor — GameState holds art
    // in Arc, so the pointer is the identity the cache needs.
    fired: HashMap<(usize, u32), Arc<Image>>,
}

impl Glaze {
    /// The art at the frame's density. Anything the kiln cannot do — an
    /// image it will not upscale — comes back as the art itself: showing
    /// the original beats showing nothing.
    pub fn fire(&mut self, art: &Arc<Image>, frame_width: u32, frame_height: u32) -> Arc<Image> {
        let factor = factor_for(art, frame_width, frame_height);
        if factor == 1 {
            return art.clone();
        }
        let key = (Arc::as_ptr(art) as usize, factor);
        if let Some(fired) = self.fired.get(&key) {
            return fired.clone();
        }
        let fired = match upscale(art, factor, METHOD) {
            Ok(image) => Arc::new(image),
            Err(_) => return art.clone(),
        };
        self.fired.insert(key, fired.clone());
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kintsugi_core::asset::ImageFormat;

    fn art(width: u32, height: u32) -> Arc<Image> {
        Arc::new(Image {
            width,
            height,
            format: ImageFormat::Rgba8,
            data: vec![128; (width * height * 4) as usize],
        })
    }

    #[test]
    fn the_scale_is_the_largest_integer_that_fits() {
        let image = art(800, 600);
        assert_eq!(factor_for(&image, 960, 720), 1);
        assert_eq!(factor_for(&image, 1920, 1440), 2);
        assert_eq!(factor_for(&image, 2000, 1200), 2);
        assert_eq!(factor_for(&image, 3840, 2160), 3);
        assert_eq!(factor_for(&image, 7680, 4320), 4, "capped past 4");
    }

    #[test]
    fn the_kiln_fires_once_and_remembers() {
        let image = art(40, 30);
        let mut glaze = Glaze::default();
        let first = glaze.fire(&image, 160, 120);
        assert_eq!((first.width, first.height), (160, 120));
        let second = glaze.fire(&image, 160, 120);
        assert!(
            Arc::ptr_eq(&first, &second),
            "the same art at the same scale is the same firing"
        );
        // A frame the art already fills gets the art itself.
        let as_is = glaze.fire(&image, 40, 30);
        assert!(Arc::ptr_eq(&as_is, &image));
    }
}
