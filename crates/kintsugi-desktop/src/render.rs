//! The framebuffer: turns the current scene into pixels, with no window and
//! no engine knowledge. Everything here is a pure function over its inputs,
//! because the part of a shell that can be tested headless is the part that
//! gets tested in CI — the window itself is a thin coat over this.

use kintsugi_core::asset::Image;

/// A frame of `0xAARRGGBB` pixels, row-major, top-down. softbuffer presents
/// the same layout with the alpha byte ignored, so nothing is converted on
/// the way out.
#[derive(Clone, Debug)]
pub struct Frame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height` pixels.
    pub pixels: Vec<u32>,
}

impl Frame {
    /// A frame filled with the lacquer dark the engine dresses in.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![BACKGROUND; (width * height) as usize],
        }
    }

    /// Set a pixel, doing nothing outside the frame. Out-of-bounds writes are
    /// a fact of blitting clipped sprites, not an error.
    pub fn set(&mut self, x: i64, y: i64, argb: u32) {
        if x >= 0 && y >= 0 && x < self.width as i64 && y < self.height as i64 {
            self.pixels[(y as u32 * self.width + x as u32) as usize] = argb;
        }
    }

    /// Read a pixel; `BACKGROUND` outside the frame.
    pub fn at(&self, x: i64, y: i64) -> u32 {
        if x >= 0 && y >= 0 && x < self.width as i64 && y < self.height as i64 {
            self.pixels[(y as u32 * self.width + x as u32) as usize]
        } else {
            BACKGROUND
        }
    }

    /// Fill a rectangle with an alpha-blended colour.
    pub fn fill_rect(&mut self, x: i64, y: i64, w: u32, h: u32, argb: u32) {
        for row in y..y + h as i64 {
            for col in x..x + w as i64 {
                let under = self.at(col, row);
                self.set(col, row, blend(argb, under));
            }
        }
    }

    /// Blit an image with its top-left corner at (`x`, `y`), honouring alpha.
    pub fn blit(&mut self, image: &Image, x: i64, y: i64) {
        for row in 0..image.height as i64 {
            for col in 0..image.width as i64 {
                let base = ((row as u32 * image.width + col as u32) * 4) as usize;
                let (r, g, b, a) = (
                    image.data[base] as u32,
                    image.data[base + 1] as u32,
                    image.data[base + 2] as u32,
                    image.data[base + 3] as u32,
                );
                if a == 0 {
                    continue;
                }
                let src = (a << 24) | (r << 16) | (g << 8) | b;
                let under = self.at(x + col, y + row);
                self.set(x + col, y + row, blend(src, under));
            }
        }
    }
}

/// The dark lacquer the empty parts of the frame wear.
pub const BACKGROUND: u32 = 0xFF0D_0C0B;
/// The gold the seams wear.
pub const GOLD: u32 = 0xFFD8_B45A;
/// The ink text is written in.
pub const INK: u32 = 0xFFF2_EDDE;

/// Alpha-blend `src` over `dst`, both `0xAARRGGBB`. The result is always
/// fully opaque: the frame is the bottom of the stack.
pub fn blend(src: u32, dst: u32) -> u32 {
    let a = (src >> 24) & 0xFF;
    if a == 0xFF {
        return src | 0xFF00_0000;
    }
    if a == 0 {
        return dst | 0xFF00_0000;
    }
    let mix = |s: u32, d: u32| ((s * a + d * (255 - a)) / 255) & 0xFF;
    0xFF00_0000
        | (mix((src >> 16) & 0xFF, (dst >> 16) & 0xFF) << 16)
        | (mix((src >> 8) & 0xFF, (dst >> 8) & 0xFF) << 8)
        | mix(src & 0xFF, dst & 0xFF)
}

/// What the renderer is asked to draw: the scene as the seam has described it
/// so far, plus the text the host is currently showing.
#[derive(Default)]
pub struct Scene<'a> {
    /// The current background, if any instruction has set one.
    pub background: Option<&'a Image>,
    /// Character sprites currently on stage, in the order they appeared.
    pub characters: Vec<&'a Image>,
    /// The speaking name shown above the box, when the engine knows one.
    pub speaker: Option<&'a str>,
    /// The lines of text inside the box, already wrapped to its width.
    pub lines: Vec<String>,
    /// The seam's own words, kept on screen while it shows a story it only
    /// partly understands. `None` when there is nothing to confess.
    pub seam_note: Option<&'a str>,
}

/// Where the text box sits and how tall it is, as a fraction of the frame.
/// A visual novel's box is a band across the bottom; nothing here is shy
/// about that.
pub const TEXT_BOX_TOP_FRACTION: f32 = 0.72;

/// The margin around the text inside the box, in pixels at scale 1.
pub const TEXT_MARGIN: u32 = 18;

/// Draw the scene into a fresh frame. `scale` is the framebuffer's density
/// against the design size — a Retina frame is 2 — and the chrome (margins,
/// rules, gaps) scales with it; the art's resampling is the glazer's job, not
/// the renderer's. `draw_text` is the font half, kept as a parameter so the
/// renderer's tests can pass a fake pen: a renderer that needed a system font
/// to be tested would not be tested on a CI box.
pub fn render(
    scene: &Scene,
    width: u32,
    height: u32,
    line_height: u32,
    scale: u32,
    mut draw_text: impl FnMut(&mut Frame, i64, i64, &str, u32),
) -> Frame {
    let scale = scale.max(1);
    let margin = (TEXT_MARGIN * scale) as i64;
    let gap = 24 * scale;
    let mut frame = Frame::new(width, height);

    if let Some(background) = scene.background {
        // Centred, at its own size: the glazer owns resampling, not the
        // player. A 2008 background is 800×600; a frame of another size
        // letterboxes it rather than pretending it was drawn larger.
        let x = (width as i64 - background.width as i64) / 2;
        let y = (height as i64 - background.height as i64) / 2;
        frame.blit(background, x, y);
    }

    // Characters stand on the bottom edge of the background area, spread
    // across the middle. Layering order is show order: the engine told us
    // who arrived first, so they stand furthest back.
    let total_width: i64 = scene
        .characters
        .iter()
        .map(|image| image.width as i64)
        .sum::<i64>()
        + gap as i64 * scene.characters.len().saturating_sub(1) as i64;
    let mut x = (width as i64 - total_width) / 2;
    for image in &scene.characters {
        frame.blit(image, x, height as i64 - image.height as i64);
        x += image.width as i64 + gap as i64;
    }

    // The text box: a translucent band, a gold rule above it, and the text.
    let box_top = (height as f32 * TEXT_BOX_TOP_FRACTION) as i64;
    frame.fill_rect(0, box_top, width, height - box_top as u32, 0xC026_221C);
    frame.fill_rect(0, box_top, width, 2 * scale, GOLD);

    let text_top = box_top + margin;
    if let Some(speaker) = scene.speaker {
        draw_text(&mut frame, margin, text_top, speaker, GOLD);
    }
    let mut y = text_top
        + if scene.speaker.is_some() {
            line_height as i64
        } else {
            0
        };
    for line in &scene.lines {
        draw_text(&mut frame, margin, y, line, INK);
        y += line_height as i64;
    }

    // The seam's note sits in the top-left, small and dim, where a reader who
    // wants the truth finds it and a reader who does not is not interrupted.
    if let Some(note) = scene.seam_note {
        draw_text(&mut frame, margin, margin, note, 0xFF8A_8378);
    }

    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, r: u8, g: u8, b: u8, a: u8) -> Image {
        let mut data = vec![0u8; (width * height * 4) as usize];
        for px in data.chunks_exact_mut(4) {
            px.copy_from_slice(&[r, g, b, a]);
        }
        Image {
            width,
            height,
            format: kintsugi_core::asset::ImageFormat::Rgba8,
            data,
        }
    }

    fn no_text(_: &mut Frame, _: i64, _: i64, _: &str, _: u32) {}

    #[test]
    fn an_empty_scene_is_lacquer_with_a_text_box() {
        let frame = render(&Scene::default(), 100, 100, 26, 1, no_text);
        assert_eq!(frame.at(50, 10), BACKGROUND, "above the box: lacquer");
        let box_top = (100.0 * TEXT_BOX_TOP_FRACTION) as i64;
        let in_box = frame.at(50, box_top + 10);
        assert_eq!(in_box >> 24, 0xFF, "the frame is always opaque");
        assert_ne!(in_box, BACKGROUND, "the box tints what it covers");
        assert_eq!(frame.at(50, box_top), GOLD, "the gold rule tops the box");
    }

    #[test]
    fn a_background_is_blit_centred_and_at_its_own_size() {
        let bg = solid(40, 20, 200, 0, 0, 255);
        let scene = Scene {
            background: Some(&bg),
            ..Scene::default()
        };
        let frame = render(&scene, 100, 100, 26, 1, no_text);
        assert_eq!(
            frame.at(50, 45) & 0xFF0000,
            200 << 16,
            "the red reached the frame"
        );
        assert_eq!(frame.at(29, 45), BACKGROUND, "letterboxed, not stretched");
        assert_eq!(
            frame.at(50, 59) & 0xFF0000,
            200 << 16,
            "the background's last row"
        );
        assert_eq!(
            frame.at(50, 65),
            BACKGROUND,
            "letterboxed below the background"
        );
    }

    #[test]
    fn a_half_transparent_sprite_mixes_with_what_is_under_it() {
        let mut frame = Frame::new(4, 4);
        let sprite = solid(1, 1, 255, 255, 255, 128);
        frame.blit(&sprite, 1, 1);
        let px = frame.at(1, 1);
        let r = (px >> 16) & 0xFF;
        // half of 255 over ~13 (the lacquer) is about 134
        assert!((125..=140).contains(&r), "got {r}");
    }

    #[test]
    fn blend_is_opaque_at_full_alpha_and_identity_at_zero() {
        let red = 0xFFAA_0000;
        assert_eq!(blend(red, BACKGROUND), red);
        assert_eq!(blend(0x00FF_FFFF, BACKGROUND), BACKGROUND);
    }

    #[test]
    fn the_pen_is_called_for_speaker_and_each_line() {
        let scene = Scene {
            speaker: Some("hina"),
            lines: vec!["one".to_string(), "two".to_string()],
            ..Scene::default()
        };
        let mut calls = Vec::new();
        let _ = render(&scene, 100, 100, 26, 1, |_, _, _, text, colour| {
            calls.push((text.to_string(), colour));
        });
        assert_eq!(
            calls,
            vec![
                ("hina".to_string(), GOLD),
                ("one".to_string(), INK),
                ("two".to_string(), INK)
            ]
        );
    }

    #[test]
    fn the_seams_note_is_drawn_when_there_is_one() {
        let scene = Scene {
            seam_note: Some("this seam reads text only"),
            ..Scene::default()
        };
        let mut seen = Vec::new();
        let _ = render(&scene, 100, 100, 26, 1, |_, _, _, text, _| {
            seen.push(text.to_string())
        });
        assert_eq!(seen, vec!["this seam reads text only".to_string()]);
    }
}
