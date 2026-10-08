//! Frame interpolation: fill the cracks between frames.
//!
//! 插帧 doubles (or ×N-s) a frame rate by synthesizing in-betweens. Motion
//! estimation is a hard, model-hungry problem, so the glaze stays honest
//! with a two-part design:
//!
//! * [`FrameInterpolator`] — the seam every backend plugs into: today's
//!   [`BlendInterpolator`] (linear crossfade, exactly what fades and
//!   slow pans need), tomorrow's motion-compensated or RIFE-style
//!   backends behind the same trait;
//! * [`interpolate_sequence`] — the timing math that turns
//!   `N` frames into `N × factor` frames, which is engine-agnostic.

use kintsugi_core::asset::Image;
use kintsugi_core::error::{Error, Result};

/// Synthesizes in-between frames.
pub trait FrameInterpolator: Send + Sync {
    /// Backend name for reports.
    fn name(&self) -> &'static str;

    /// The frame at time `t` between `a` (t = 0) and `b` (t = 1).
    ///
    /// Both frames must have identical dimensions; `t` is clamped to
    /// `0.0..=1.0`.
    fn in_between(&self, a: &Image, b: &Image, t: f32) -> Result<Image>;
}

/// Linear crossfade: every channel lerped between the two frames.
///
/// The honest default. On fades and dissolves it is *exactly* right; on
/// fast motion it produces the same double-image the original hardware
/// would show. Backends that estimate motion belong behind the same trait.
#[derive(Debug, Clone, Copy, Default)]
pub struct BlendInterpolator;

impl FrameInterpolator for BlendInterpolator {
    fn name(&self) -> &'static str {
        "blend"
    }

    fn in_between(&self, a: &Image, b: &Image, t: f32) -> Result<Image> {
        if a.width != b.width || a.height != b.height {
            return Err(Error::unsupported(
                "frame interpolation",
                format!(
                    "frame sizes differ: {}x{} vs {}x{}",
                    a.width, a.height, b.width, b.height
                ),
            ));
        }
        if a.data.len() != b.data.len() {
            return Err(Error::corrupt(
                "frame interpolation",
                "frame buffers have mismatched lengths",
            ));
        }
        let t = t.clamp(0.0, 1.0);
        let mut out = Image::new(a.width, a.height);
        for (dst, (x, y)) in out.data.iter_mut().zip(a.data.iter().zip(b.data.iter())) {
            *dst = (*x as f32 * (1.0 - t) + *y as f32 * t).round() as u8;
        }
        Ok(out)
    }
}

/// An image together with the name it was found under, so a report can
/// point at the file rather than at a position in a list.
pub type NamedFrame = (String, Image);

/// A name, and the human-readable reason it is not a frame of the sequence.
pub type NotAFrame = (String, String);

/// Split named images into the one sequence worth interpolating, plus the
/// images that are not frames of it.
///
/// A game directory holds images of many sizes, and only same-sized ones can
/// be frames of a single sequence: blending a 3x3 portrait into a 5x3
/// cutscene frame would invent motion that never existed. The longest
/// same-sized run wins (ties go to the size seen first), and every image left
/// out is returned with a human-readable reason so the host can name it. A
/// silently dropped frame is a silently wrong cutscene.
pub fn group_by_size(frames: &[NamedFrame]) -> (Vec<NamedFrame>, Vec<NotAFrame>) {
    use std::collections::BTreeMap;

    // An empty input has no sequence to choose and nothing to leave out. This
    // is also what stops the `expect` below from being a panic: `max_by_key`
    // has nothing to return, so the empty case is decided here, by contract
    // ("an empty input stays empty", the same rule `interpolate_sequence`
    // follows) rather than by an unwrap.
    if frames.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let mut sizes: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    let mut first_seen: Vec<(u32, u32)> = Vec::new();
    for (_, image) in frames {
        let size = (image.width, image.height);
        if !sizes.contains_key(&size) {
            first_seen.push(size);
        }
        *sizes.entry(size).or_insert(0) += 1;
    }
    // Longest run wins; a tie keeps the size that appeared first, so the
    // result depends on the file order rather than on a hash iteration.
    let chosen = first_seen
        .iter()
        .copied()
        .max_by_key(|size| {
            (
                sizes[size],
                std::cmp::Reverse(first_seen.iter().position(|s| s == size)),
            )
        })
        .expect("a chosen size implies at least one frame");

    let mut sequence = Vec::new();
    let mut skipped = Vec::new();
    for (name, image) in frames {
        if (image.width, image.height) == chosen {
            sequence.push((name.clone(), image.clone()));
        } else {
            skipped.push((
                name.clone(),
                format!(
                    "{}x{}, not {}x{} like the other frames",
                    image.width, image.height, chosen.0, chosen.1
                ),
            ));
        }
    }
    (sequence, skipped)
}

/// Fill the timeline between consecutive `frames`.
///
/// For `n` frames this returns `(n - 1) * factor + 1` frames: every gap
/// gets `factor - 1` evenly spaced in-betweens, and the originals stay in
/// order. Filling gaps (rather than inventing frames past the end) is the
/// honest contract — the host holds the final frame for its duration, and
/// nothing is extrapolated that was never on the disc.
///
/// An empty input stays empty; a single frame is returned unchanged
/// (there is no gap to fill).
pub fn interpolate_sequence(
    frames: &[Image],
    factor: u32,
    interpolator: &dyn FrameInterpolator,
) -> Result<Vec<Image>> {
    if factor == 0 {
        return Err(Error::unsupported(
            "frame interpolation",
            "factor must be at least 1",
        ));
    }
    if frames.is_empty() {
        return Ok(Vec::new());
    }
    if factor == 1 {
        return Ok(frames.to_vec());
    }
    let mut out = Vec::with_capacity(frames.len() * factor as usize);
    for (index, frame) in frames.iter().enumerate() {
        out.push(frame.clone());
        if index + 1 == frames.len() {
            continue;
        }
        let next = &frames[index + 1];
        for step in 1..factor {
            let t = step as f32 / factor as f32;
            out.push(interpolator.in_between(frame, next, t)?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, value: u8) -> Image {
        let mut image = Image::new(width, height);
        image.data.fill(value);
        image
    }

    #[test]
    fn blend_midpoint_of_extremes_is_half() {
        let a = solid(2, 2, 0);
        let b = solid(2, 2, 255);
        let mid = BlendInterpolator.in_between(&a, &b, 0.5).unwrap();
        assert!(mid.data.iter().all(|&v| v == 127 || v == 128));
    }

    #[test]
    fn blend_endpoints_reproduce_inputs() {
        let a = solid(3, 2, 10);
        let b = solid(3, 2, 200);
        let at_zero = BlendInterpolator.in_between(&a, &b, 0.0).unwrap();
        let at_one = BlendInterpolator.in_between(&a, &b, 1.0).unwrap();
        assert_eq!(at_zero.data, a.data);
        assert_eq!(at_one.data, b.data);
    }

    #[test]
    fn mismatched_sizes_are_rejected() {
        let a = solid(2, 2, 0);
        let b = solid(3, 2, 255);
        assert!(BlendInterpolator.in_between(&a, &b, 0.5).is_err());
    }

    #[test]
    fn sequence_doubling_fills_every_gap() {
        let black = solid(2, 1, 0);
        let white = solid(2, 1, 255);
        let frames = vec![black, white];
        // (2 - 1) * 2 + 1 = 3: the original pair plus one true midpoint.
        let out = interpolate_sequence(&frames, 2, &BlendInterpolator).unwrap();
        assert_eq!(out.len(), 3);
        assert!(out[0].data.iter().all(|&v| v == 0));
        assert!(out[2].data.iter().all(|&v| v == 255));
        assert!(out[1].data.iter().all(|&v| v == 127 || v == 128));

        // Three frames at 3x: (3 - 1) * 3 + 1 = 7 frames, gaps evenly spaced.
        let three = vec![solid(1, 1, 0), solid(1, 1, 90), solid(1, 1, 180)];
        let out = interpolate_sequence(&three, 3, &BlendInterpolator).unwrap();
        assert_eq!(out.len(), 7);
        assert_eq!(out[0].data[0], 0);
        assert_eq!(out[3].data[0], 90); // the second original
        assert_eq!(out[6].data[0], 180); // the third original
        assert_eq!(out[1].data[0], 30); // 1/3 of the first gap
        assert_eq!(out[2].data[0], 60); // 2/3 of the first gap
    }

    #[test]
    fn sequence_edge_cases() {
        assert!(
            interpolate_sequence(&[], 4, &BlendInterpolator)
                .unwrap()
                .is_empty()
        );
        // A lone frame has no gap: it comes back as itself, not multiplied.
        let one = vec![solid(1, 1, 9)];
        let out = interpolate_sequence(&one, 3, &BlendInterpolator).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].data, vec![9, 9, 9, 9]);
        assert!(interpolate_sequence(&one, 0, &BlendInterpolator).is_err());
        // factor 1 is the identity, whatever the length.
        let two = vec![solid(1, 1, 1), solid(1, 1, 2)];
        let same = interpolate_sequence(&two, 1, &BlendInterpolator).unwrap();
        assert_eq!(same.len(), 2);
        assert_eq!(same[1].data, vec![2, 2, 2, 2]);
    }

    fn named(name: &str, image: Image) -> (String, Image) {
        (name.to_string(), image)
    }

    #[test]
    fn grouping_picks_the_longest_run_and_names_the_rest() {
        // A real directory: three sizes, only the cutscene has several frames.
        let frames = vec![
            named("title.bbm", solid(4, 3, 1)),
            named("room.zbm", solid(4, 4, 2)),
            named("face.zbm", solid(3, 3, 3)),
            named("cut01.zbm", solid(5, 3, 10)),
            named("cut02.zbm", solid(5, 3, 20)),
            named("cut03.zbm", solid(5, 3, 30)),
        ];
        let (sequence, skipped) = group_by_size(&frames);

        let names: Vec<&str> = sequence.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["cut01.zbm", "cut02.zbm", "cut03.zbm"]);
        // Order inside the sequence is the caller's file order, untouched.
        assert_eq!(sequence[1].1.data[0], 20);

        assert_eq!(skipped.len(), 3);
        assert_eq!(skipped[0].0, "title.bbm");
        assert!(skipped[0].1.contains("4x3"), "{}", skipped[0].1);
        assert!(skipped[0].1.contains("5x3"), "{}", skipped[0].1);
        // The reason says why, so a host can print it verbatim.
        assert!(skipped[0].1.contains("not 5x3 like the other frames"));
    }

    #[test]
    fn grouping_keeps_a_uniform_directory_whole() {
        let frames = vec![
            named("a.zbm", solid(2, 2, 1)),
            named("b.zbm", solid(2, 2, 2)),
        ];
        let (sequence, skipped) = group_by_size(&frames);
        assert_eq!(sequence.len(), 2);
        assert!(skipped.is_empty());
    }

    #[test]
    fn grouping_an_empty_directory_is_empty_rather_than_a_panic() {
        // `interpolate_sequence` documents "an empty input stays empty"; the
        // grouping step has to agree, or a host that groups before it checks
        // for frames gets a panic instead of an empty answer.
        let (sequence, skipped) = group_by_size(&[]);
        assert!(sequence.is_empty());
        assert!(skipped.is_empty());
    }

    #[test]
    fn grouping_ties_go_to_the_size_seen_first() {
        // Two sizes with two frames each: file order decides, not a hash.
        let frames = vec![
            named("wide01.zbm", solid(4, 2, 1)),
            named("tall01.zbm", solid(2, 4, 2)),
            named("wide02.zbm", solid(4, 2, 3)),
            named("tall02.zbm", solid(2, 4, 4)),
        ];
        let (sequence, skipped) = group_by_size(&frames);
        let names: Vec<&str> = sequence.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["wide01.zbm", "wide02.zbm"]);
        assert_eq!(skipped.len(), 2);
    }

    #[test]
    fn grouping_a_single_image_leaves_one_frame_for_the_caller_to_refuse() {
        let frames = vec![named("only.zbm", solid(3, 3, 7))];
        let (sequence, skipped) = group_by_size(&frames);
        assert_eq!(sequence.len(), 1);
        assert!(skipped.is_empty());
        // One frame is not a sequence: the gap-filling contract returns it
        // unchanged rather than inventing a neighbour.
        let out = interpolate_sequence(
            &sequence.iter().map(|(_, i)| i.clone()).collect::<Vec<_>>(),
            4,
            &BlendInterpolator,
        )
        .unwrap();
        assert_eq!(out.len(), 1);
    }
}
