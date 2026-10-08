//! 🏺 Golden seam №1: the **BlueGale** engine (ブルーゲイル).
//!
//! BlueGale shipped visual novels from the late 1990s into the 2010s
//! (『えろすまっしゅ！』, the 『鬼父』 series, 『パパラブ』…), and the studio's
//! formats changed with the hardware. This seam covers what the community
//! has reverse-engineered so far — and says so when it doesn't:
//!
//! * **SNN + INX** — resource archives: a `.inx` index of `0x48`-byte
//!   records beside a raw `.snn` data blob ([`snn`]);
//! * **ZBM** — `amp_`-signed, LZ-packed bitmaps, optionally with the first
//!   100 bytes XOR-obfuscated ([`zbm`]);
//! * **BBM** — bitmaps with the first 100 bytes XOR-obfuscated ([`bbm`]);
//! * **BDT** — scripts: CP932 text, whole-file XOR `0xFF`, with `$`/`%`
//!   label lines ([`bdt`]);
//! * **AMV** — video; *not implemented*, tracked in `docs/RESEARCH-BlueGale.md`.
//!
//! Everything below is translated into the body's vocabulary
//! ([`kintsugi_core`]) so the runtime never learns a BlueGale detail.

pub mod bbm;
pub mod bdt;
pub mod fixtures;
pub mod plugin;
pub mod snn;
pub mod zbm;

pub use plugin::{BluegaleMount, BluegalePlugin, plugin};

use kintsugi_core::error::{Error, Result};

/// CP932 (Windows-31J) — the code page BlueGale's tools wrote.
///
/// `encoding_rs::SHIFT_JIS` *is* Windows-31J, i.e. CP932 with the vendor
/// extensions, which is exactly what these files need.
pub fn decode_cp932(data: &[u8]) -> String {
    let (text, _, _) = encoding_rs::SHIFT_JIS.decode(data);
    text.into_owned()
}

/// Encode text back to CP932.
///
/// Strict by design. `encoding_rs` substitutes HTML numeric character
/// references for characters the code page cannot hold — an em dash becomes
/// the eight literal characters `&#8212;`, which the game engine would then
/// draw on screen. A repair tool must refuse instead: the caller can report
/// exactly which characters the original code page cannot express (a
/// translated line may need rewording, or a fan patch needs a font hack).
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
    fn cp932_round_trips_the_words_this_engine_actually_stores() {
        let text = "深夜の工房。金継ぎ――金繕いだった。「直せるものなら、直したい……」";
        let bytes = encode_cp932(text).unwrap();
        assert_eq!(decode_cp932(&bytes), text);
        // CP932 stores these kanji in two bytes each where UTF-8 needs
        // three, so this can only pass if the code page really was used.
        assert!(
            bytes.len() < text.len(),
            "expected CP932 ({} bytes) to be tighter than UTF-8 ({} bytes)",
            bytes.len(),
            text.len()
        );
    }

    #[test]
    fn cp932_refuses_characters_it_cannot_hold() {
        // The honest failure: `encoding_rs` alone would write the literal
        // ASCII text "&#8212;" into the script and call it success.
        let err = encode_cp932("金継ぎ—金缮").unwrap_err();
        assert_eq!(
            err,
            Error::Encoding {
                encoding: "CP932".into(),
                characters: "—缮".into(),
            }
        );
        let message = err.to_string();
        assert!(message.contains("CP932"), "{message}");
        assert!(message.contains('—') && message.contains('缮'), "{message}");
    }

    #[test]
    fn cp932_keeps_half_width_katakana_and_ascii() {
        // Free text from a translation may be pure ASCII; it must still work.
        let text = "Hello, world! ｱｲｳ 123";
        let bytes = encode_cp932(text).unwrap();
        assert_eq!(decode_cp932(&bytes), text);
    }
}
