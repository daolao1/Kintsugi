//! Golden seam for BlueGale's later **BSX** engine: `BSArc` archives and
//! `BSG` pictures, the formats of a 2008 release whose story is carried by a
//! compiled `BSScript`.
//!
//! # Where the facts come from
//!
//! Every layout in this crate was read out of a real release and checked
//! against a second source before it was written down:
//!
//! * `BSArc` against all six archives of the release (4,405 entries) and
//!   against GARbro's `ArcFormats/Bishop/ArcBSA.cs`, which documents the same
//!   container — BlueGale and Bishop shipped the same middleware.
//! * `BSG` against all 765 images of that release and against GARbro's
//!   `ArcFormats/Bishop/ImageBSG.cs`.
//! * The `BSScript` story (`bsx.dat`) against the file itself and against a
//!   parser written independently of this crate: its thirteen records, its four
//!   string tables, and the measurement that says the engine refers to its
//!   lines by index rather than by address — which is what makes a repair that
//!   changes line lengths possible at all.
//!
//! `docs/RESEARCH-BSX.md` records the numbers, the sources and — just as
//! importantly — what is **not** implemented.
//!
//! # What this seam does today
//!
//! Detection, mounting (loose files plus every archive, read through byte
//! ranges), image decoding, and the story: `bsx.dat` is read into the body's IR
//! in the order the code shows it, translated, and written back with only the
//! lines a translation names changed. Verified on a real 2008 release: 11,986
//! `show` instructions turned into 11,864 narration and dialogue commands,
//! mock-translated and repaired in about a second, with the code, the header and
//! the name tables byte-identical afterwards.
//!
//! The classification comes from the instructions themselves: `1a <channel>
//! <line:u32>`, where the channel is the text box the game draws. On the
//! release this was written against the channels are three, they cover 99.8% of
//! the lines, and their channel 0 text is narration while 1 and 2 are spoken —
//! that is what the lines say, not what the numbers were assumed to mean.
//!
//! What it does **not** do is read the branch instructions, so
//! [`kintsugi_core::script::Command::Choice`] is never produced: the runs of
//! consecutive lines are walked one after the other, which is every line the
//! story can reach rather than one playthrough, and the seam says so in a
//! warning on every script it hands over. A line the code never shows is kept as
//! [`kintsugi_core::script::Command::RawLine`] rather than dropped.

pub mod bsarc;
pub mod fixtures;
pub mod image;
pub mod plugin;
pub mod script;

pub use plugin::{ENGINE_ID, plugin};
pub use script::{SCRIPT_MAGIC, Show, Story};

use kintsugi_core::error::{Error, Result};

/// Decode CP932 bytes, reporting whether anything had to be replaced.
///
/// Names inside these files are the game's own bytes, and the engine's text is
/// CP932. Decoding is deliberately lossy-but-reported rather than
/// all-or-nothing: one odd byte in one name should not hide a whole game, but
/// the caller gets told so it can put the crack in a mount note.
pub fn decode_cp932(bytes: &[u8]) -> (String, bool) {
    let (text, _, had_errors) = encoding_rs::SHIFT_JIS.decode(bytes);
    (text.into_owned(), had_errors)
}

/// Encode text as CP932, refusing characters the encoding cannot carry.
///
/// The refusal is the point: a translation that writes a character CP932 has
/// no code for would produce a script the engine reads as mojibake, and a quiet
/// mojibake is worse than a named character.
pub fn encode_cp932(text: &str) -> Result<Vec<u8>> {
    let (bytes, _, unmappable) = encoding_rs::SHIFT_JIS.encode(text);
    if !unmappable {
        return Ok(bytes.into_owned());
    }
    let mut characters = String::new();
    for ch in text.chars() {
        let mut buf = [0u8; 4];
        let (_, _, failed) = encoding_rs::SHIFT_JIS.encode(ch.encode_utf8(&mut buf));
        if failed && !characters.contains(ch) {
            characters.push(ch);
        }
    }
    Err(Error::encoding("CP932", characters))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp932_round_trips_engine_text() {
        let text = "深夜の工房。";
        let bytes = encode_cp932(text).expect("CP932 can carry this");
        assert_eq!(bytes.len(), 12, "six double byte characters");
        let (back, lossy) = decode_cp932(&bytes);
        assert_eq!(back, text);
        assert!(!lossy);
    }

    #[test]
    fn a_character_cp932_cannot_carry_is_named() {
        let error = encode_cp932("a 𝒜").unwrap_err();
        let message = error.to_string();
        assert!(message.contains('𝒜'), "{message}");
    }

    #[test]
    fn undecodable_bytes_are_reported_not_hidden() {
        let (_, lossy) = decode_cp932(&[0x81, 0x20]);
        assert!(lossy, "an invalid CP932 pair must be reported");
    }
}
