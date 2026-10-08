//! The pen: finds a font that can write the game's language and draws text
//! into the framebuffer. A shell may pull dependencies (rule two), so this is
//! `ab_glyph` over a system font; the body stays dependency-free and never
//! sees a glyph.

use std::path::PathBuf;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};

use crate::render::{Frame, blend};

/// A loaded font plus the size text is drawn at.
pub struct Pen {
    font: FontVec,
    scale: PxScale,
    /// Where the baseline sits relative to the top of a line.
    ascent: f32,
    /// Line height: ascent + descent + the gap between lines.
    line_height: f32,
}

/// One candidate in the search: the file, and which face of a collection to
/// take (`.ttc` files hold several).
struct Candidate {
    path: &'static str,
    index: u32,
}

/// The search order, first hit wins. Every entry is a font that ships with
/// the operating system and covers Japanese — the games this engine repairs
/// are Japanese, and a shell that drew tofu boxes over a 2008 script would be
/// failing its one job. `KINTSUGI_FONT` overrides the whole list, which is
/// how tests (and readers with opinions) point at a specific file.
fn candidates() -> Vec<Candidate> {
    if let Ok(path) = std::env::var("KINTSUGI_FONT") {
        // The leaked string lives as long as the process, which is exactly
        // how long the candidate list does.
        let path: &'static str = Box::leak(path.into_boxed_str());
        return vec![Candidate { path, index: 0 }];
    }
    let mut list = Vec::new();
    if cfg!(target_os = "macos") {
        list.extend([
            Candidate {
                path: "/System/Library/Fonts/ヒラギノ角ゴシック W4.ttc",
                index: 0,
            },
            Candidate {
                path: "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
                index: 0,
            },
            Candidate {
                path: "/System/Library/Fonts/PingFang.ttc",
                index: 0,
            },
        ]);
    }
    if cfg!(target_os = "windows") {
        list.extend([
            Candidate {
                path: r"C:\Windows\Fonts\meiryo.ttc",
                index: 0,
            },
            Candidate {
                path: r"C:\Windows\Fonts\msgothic.ttc",
                index: 0,
            },
            Candidate {
                path: r"C:\Windows\Fonts\msyh.ttc",
                index: 0,
            },
        ]);
    }
    if cfg!(target_os = "linux") {
        list.extend([
            Candidate {
                path: "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                index: 2,
            },
            Candidate {
                path: "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
                index: 2,
            },
            Candidate {
                path: "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
                index: 0,
            },
        ]);
    }
    list
}

impl Pen {
    /// Load the first font from the search list that parses. `None` means no
    /// usable font exists on this machine; the hosts render the scene anyway
    /// and say so, because a missing font is a fact to report, not a reason
    /// to pretend.
    pub fn load(pixel_height: f32) -> Option<Self> {
        for candidate in candidates() {
            let path = PathBuf::from(candidate.path);
            let Ok(data) = std::fs::read(&path) else {
                continue;
            };
            if let Ok(font) = FontVec::try_from_vec_and_index(data, candidate.index) {
                let scale = PxScale::from(pixel_height);
                let scaled = font.as_scaled(scale);
                let ascent = scaled.ascent();
                let line_height = (scaled.ascent() - scaled.descent() + scaled.line_gap()).ceil();
                return Some(Self {
                    font,
                    scale,
                    ascent,
                    line_height,
                });
            }
        }
        None
    }

    /// Height of one line of text, for layout.
    pub fn line_height(&self) -> u32 {
        self.line_height.max(1.0) as u32
    }

    /// The width `text` would occupy if drawn.
    pub fn measure(&self, text: &str) -> u32 {
        let scaled = self.font.as_scaled(self.scale);
        let mut width = 0.0f32;
        let mut previous = None;
        for ch in text.chars() {
            let id = self.font.glyph_id(ch);
            if let Some(prev) = previous {
                width += scaled.kern(prev, id);
            }
            width += scaled.h_advance(id);
            previous = Some(id);
        }
        width.ceil() as u32
    }

    /// Wrap `text` to `max_width` pixels, breaking between characters —
    /// Japanese has no spaces to break on, and breaking mid-word in English
    /// is the same operation there. `\n` starts a new line unconditionally.
    pub fn wrap(&self, text: &str, max_width: u32) -> Vec<String> {
        let mut lines = Vec::new();
        for raw in text.split('\n') {
            let mut line = String::new();
            for ch in raw.chars() {
                let candidate = format!("{line}{ch}");
                if !line.is_empty() && self.measure(&candidate) > max_width {
                    lines.push(std::mem::take(&mut line));
                }
                line.push(ch);
            }
            lines.push(line);
        }
        if lines.is_empty() {
            lines.push(String::new());
        }
        lines
    }

    /// Draw `text` with its top-left at (`x`, `y`), blending by coverage.
    pub fn draw(&self, frame: &mut Frame, x: i64, y: i64, text: &str, argb: u32) {
        let scaled = self.font.as_scaled(self.scale);
        let mut pen_x = x as f32;
        let baseline = y as f32 + self.ascent;
        let mut previous = None;
        for ch in text.chars() {
            let id = self.font.glyph_id(ch);
            if let Some(prev) = previous {
                pen_x += scaled.kern(prev, id);
            }
            let glyph = id.with_scale_and_position(self.scale, ab_glyph::point(pen_x, baseline));
            pen_x += scaled.h_advance(id);
            previous = Some(id);
            let Some(outlined) = self.font.outline_glyph(glyph) else {
                continue; // whitespace and control glyphs have no outline
            };
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                if coverage == 0.0 {
                    return;
                }
                let px = bounds.min.x as i64 + gx as i64;
                let py = bounds.min.y as i64 + gy as i64;
                let alpha = ((argb >> 24) & 0xFF) as f32 * coverage;
                let src = ((alpha as u32) << 24) | (argb & 0x00FF_FFFF);
                let under = frame.at(px, py);
                frame.set(px, py, blend(src, under));
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::BACKGROUND;

    /// A font is present on macOS (Hiragino always ships) and may be absent
    /// on a headless CI box. The behavioural assertions only run when there
    /// is a pen to run them with; the search itself is asserted to terminate
    /// cleanly either way.
    fn pen() -> Option<Pen> {
        Pen::load(24.0)
    }

    #[test]
    fn the_search_either_finds_a_font_or_says_so() {
        // The contract is the Option, not the machine this test runs on.
        let _ = pen();
    }

    #[test]
    fn drawn_text_leaves_ink_in_the_frame() {
        let Some(pen) = pen() else {
            eprintln!(
                "no system font here; the behavioural half of this test is macOS/Windows work"
            );
            return;
        };
        let mut frame = Frame::new(200, 60);
        pen.draw(&mut frame, 4, 4, "緒 A", crate::render::INK);
        let inked = frame.pixels.iter().filter(|&&px| px != BACKGROUND).count();
        assert!(inked > 50, "two glyphs should ink more than {inked} pixels");
    }

    #[test]
    fn wrapping_respects_the_width_and_keeps_every_character() {
        let Some(pen) = pen() else {
            eprintln!("no system font here; wrap behaviour verified where a font ships");
            return;
        };
        let text = "鬼の父は、娘を想うあまりに道を外れた。The father lost his way.";
        let lines = pen.wrap(text, 160);
        assert!(lines.len() > 1, "a long line must wrap");
        let joined: String = lines.concat();
        assert_eq!(joined, text, "wrapping may not drop or add a character");
        for line in &lines {
            assert!(
                pen.measure(line) <= 160,
                "'{line}' measures {}px over the 160px limit",
                pen.measure(line)
            );
        }
    }

    #[test]
    fn explicit_newlines_always_start_a_line() {
        let Some(pen) = pen() else { return };
        let lines = pen.wrap("one\ntwo", 10_000);
        assert_eq!(lines, vec!["one".to_string(), "two".to_string()]);
    }
}
