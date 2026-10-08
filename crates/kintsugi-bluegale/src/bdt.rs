//! BDT: BlueGale's script container.
//!
//! The newer BlueGale generation (『鬼父』 era onward) stores its scenarios
//! as `.bdt` files: **CP932 text, whole-file XOR-ed with `0xFF`**, lines
//! separated by CRLF. Label lines start with (tab-prefixed) `$` or `%`
//! followed by the label name; everything else is a line of the script.
//!
//! The community knows the *container* well — SExtractor ships
//! `extract_BlueGale_bdt.py`, which this module follows — but the semantics
//! of the non-label lines are **not yet reverse-engineered**: which are
//! dialogue, which are engine commands. Until someone fills that crack,
//! every non-label line is carried through as [`Command::RawLine`]: visible
//! in play mode, honest about being unclassified, ready to be retyped.
//! (The game engine's label index, `indexwww.dat`, is documented in
//! `docs/RESEARCH-BlueGale.md` but not yet parsed.)

use kintsugi_core::error::{Error, Result};
use kintsugi_core::script::{Command, Script};
use kintsugi_core::vfs::VirtualPath;

use std::collections::{BTreeMap, BTreeSet};

/// Whether `data` looks like a BDT script after de-XOR.
///
/// Heuristic, and says so in the note it produces: ≥ 60% of the bytes must
/// be CP932-printable (ASCII graphic, kana/kanji lead bytes, CR/LF/TAB),
/// and at least one `$`/`%` label line must appear. Detection confidence
/// must stay below "certain" because of this.
pub fn looks_like_bdt(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    let mut printable = 0usize;
    let mut labels = 0usize;
    let mut line_start = true;
    for &raw in data {
        let b = raw ^ 0xFF;
        let is_printable =
            matches!(b, 0x09 | 0x0A | 0x0D | 0x20..=0x7E | 0x81..=0x9F | 0xE0..=0xFC);
        if is_printable {
            printable += 1;
        }
        if line_start && (b == b'$' || b == b'%') {
            labels += 1;
        }
        line_start = b == b'\n';
    }
    printable * 10 >= data.len() * 6 && labels >= 1
}

/// De-XOR and decode a BDT file into text (CP932 → UTF-8).
pub fn decode_bdt_text(data: &[u8]) -> String {
    let plain: Vec<u8> = data.iter().map(|b| b ^ 0xFF).collect();
    crate::decode_cp932(&plain)
}

/// Parse a `.bdt` script into the body's IR.
///
/// `$name` / `%name` lines become labels; every other non-empty line becomes
/// a [`Command::RawLine`]. See the module docs for why the lines are not
/// (yet) classified as dialogue or commands.
pub fn parse_bdt(path: impl Into<VirtualPath>, data: &[u8]) -> Result<Script> {
    let path = path.into();
    let text = decode_bdt_text(data);
    let mut script = Script::new(path);

    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let after_tabs = line.trim_start_matches('\t');
        if let Some(rest) = after_tabs
            .strip_prefix('$')
            .or_else(|| after_tabs.strip_prefix('%'))
        {
            let name = rest.trim();
            if name.is_empty() {
                script
                    .warnings
                    .push(format!("empty label on line '{}'", line.trim_end()));
                continue;
            }
            script.commands.push(Command::Label(name.to_string()));
        } else {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                continue;
            }
            script.commands.push(Command::RawLine(trimmed.to_string()));
        }
    }

    if script.commands.is_empty() {
        return Err(Error::corrupt(
            "BDT script",
            format!("'{}' contains no commands after decoding", script.source),
        ));
    }
    Ok(script)
}

/// Rewrite a BDT with new text for the lines named in `replacements`.
///
/// The walk below is deliberately the *same* walk [`parse_bdt`] does, so a
/// command index means the same thing to the reader and the writer.
///
/// Only replaced lines are re-encoded. Every other byte is copied straight
/// from the de-XORed original into the patch, which is what makes the promise
/// below literal rather than aspirational: line endings, blank lines, `\t`
/// indentation, the `$`/`%` sigil on labels (we have not verified that the
/// engine's interpreter treats the two alike, so the only honest thing is to
/// never rewrite one), any trailing bytes, and whether the file ended with a
/// newline at all.
///
/// Copying rather than re-encoding matters more than it looks. CP932 has
/// hundreds of characters with more than one valid byte sequence — the NEC and
/// IBM duplicate rows, for instance `0x87 0x90` and `0x81 0xE0` are both `≒` —
/// so decoding the whole file and encoding it again would silently re-spell
/// lines nobody asked to touch, in a patch that reports `replaced` as it was.
/// A repair that edits bytes it did not understand is not a repair.
pub fn rewrite_bdt(original: &[u8], replacements: &BTreeMap<usize, String>) -> Result<Rewritten> {
    let plain: Vec<u8> = original.iter().map(|b| b ^ 0xFF).collect();
    let mut out: Vec<u8> = Vec::with_capacity(plain.len());
    let mut replaced = 0usize;
    let mut matched: BTreeSet<usize> = BTreeSet::new();
    let mut command = 0usize;
    let mut cursor = 0usize;

    while cursor < plain.len() {
        // The line's bytes, terminator included, and where its content stops.
        let (content_end, next) = match plain[cursor..].iter().position(|&b| b == b'\n') {
            Some(offset) => {
                let newline = cursor + offset;
                let content_end = if newline > cursor && plain[newline - 1] == b'\r' {
                    newline - 1
                } else {
                    newline
                };
                (content_end, newline + 1)
            }
            None => (plain.len(), plain.len()),
        };
        let content = &plain[cursor..content_end];

        // Structure is decided on the decoded text because that is exactly
        // what `parse_bdt` decides on; only the *output* is byte-level. The
        // two must agree command for command, or a translation would land on
        // the wrong line.
        let decoded = crate::decode_cp932(content);
        let after_tabs = decoded.trim_start_matches('\t');
        let is_label = after_tabs.starts_with('$') || after_tabs.starts_with('%');
        let is_blank = decoded.trim_end().is_empty();

        // Blank lines and empty labels produce no command (see `parse_bdt`),
        // so they must not consume an index here either.
        let label_has_name = is_label && !after_tabs[1..].trim().is_empty();
        let is_command = label_has_name || (!is_label && !is_blank);

        match replacements.get(&command).filter(|_| is_command) {
            Some(new_text) if !is_label => {
                check_replaceable(new_text, &decoded)?;
                // Indentation is copied, the words are encoded, the ending is
                // copied: the three parts of a line, each handled once.
                let indent = content.iter().take_while(|&&b| b == b'\t').count();
                out.extend_from_slice(&plain[cursor..cursor + indent]);
                out.extend_from_slice(&crate::encode_cp932(new_text.trim_end())?);
                out.extend_from_slice(&plain[content_end..next]);
                replaced += 1;
                matched.insert(command);
            }
            _ => out.extend_from_slice(&plain[cursor..next]),
        }
        if is_command {
            command += 1;
        }
        cursor = next;
    }

    let unmatched: Vec<usize> = replacements
        .keys()
        .copied()
        .filter(|id| !matched.contains(id))
        .collect();
    Ok(Rewritten {
        data: out.into_iter().map(|b| b ^ 0xFF).collect(),
        replaced,
        unmatched,
    })
}

/// A `.bdt` rewritten in memory: the whole file's new bytes, and what happened.
///
/// The bytes rather than a [`kintsugi_core::plugin::WrittenScript`], because a
/// script's bytes do not always end up in a file of their own: the same result
/// either becomes the new `story.bdt`, or goes back into the SNN blob it was
/// packed in. Deciding that is the mount's job, not the writer's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rewritten {
    /// The complete new contents of the script file.
    pub data: Vec<u8>,
    /// How many replacements were applied.
    pub replaced: usize,
    /// Ids that had no line to land on.
    pub unmatched: Vec<usize>,
}

/// Refuse a replacement that would change the file's *structure* rather than
/// its words.
fn check_replaceable(new_text: &str, original_line: &str) -> Result<()> {
    if new_text.contains('\n') || new_text.contains('\r') {
        return Err(Error::unsupported(
            "BDT writer",
            "a translated line may not contain a line break: it would split one \
             line into two and shift every line after it",
        ));
    }
    let after_tabs = new_text.trim_start_matches('\t');
    if after_tabs.starts_with('$') || after_tabs.starts_with('%') {
        return Err(Error::unsupported(
            "BDT writer",
            format!(
                "a translated line may not start with '$' or '%': it would turn \
                 the line into a label (was: '{}')",
                original_line.trim_end()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_cp932, encode_cp932};
    use kintsugi_core::script::Command;

    fn make_bdt(text: &str) -> Vec<u8> {
        encode_cp932(text)
            .expect("test text must be CP932-clean")
            .into_iter()
            .map(|b| b ^ 0xFF)
            .collect()
    }

    #[test]
    fn parses_labels_and_raw_lines() {
        let source = "$start\r\nこんにちは。\r\n\t%scene2\r\n金継ぎの夜。\r\n";
        let script = parse_bdt("story.bdt", &make_bdt(source)).unwrap();
        assert_eq!(
            script.commands,
            vec![
                Command::Label("start".into()),
                Command::RawLine("こんにちは。".into()),
                Command::Label("scene2".into()),
                Command::RawLine("金継ぎの夜。".into()),
            ]
        );
    }

    #[test]
    fn skips_blank_lines_and_warns_on_empty_labels() {
        let source = "$\r\n\r\n一行だけ。\r\n";
        let script = parse_bdt("s.bdt", &make_bdt(source)).unwrap();
        assert_eq!(script.commands, vec![Command::RawLine("一行だけ。".into())]);
        assert_eq!(script.warnings.len(), 1);
    }

    #[test]
    fn rejects_empty_scripts() {
        assert!(parse_bdt("s.bdt", &make_bdt("\r\n\r\n")).is_err());
    }

    #[test]
    fn detection_heuristic() {
        assert!(looks_like_bdt(&make_bdt("$start\r\n台詞。\r\n")));
        // Random bytes (de-XORed still random) must fail.
        let junk: Vec<u8> = (0..256u32).map(|i| (i * 7 % 256) as u8).collect();
        assert!(!looks_like_bdt(&junk));
        // Plain BMP data must not look like a script.
        assert!(!looks_like_bdt(
            b"BM\x36\x00\x00\x00\x00\x00\x00\x00\x36\x00\x00\x00"
        ));
    }

    /// Indices follow `parse_bdt` exactly, so a translation entry's `id`
    /// means the same line to both halves of the tool.
    #[test]
    fn command_indices_line_up_with_the_parser() {
        let source = "$start\r\n\r\nこんにちは。\r\n\t%scene2\r\n\r\n金継ぎの夜。\r\n";
        let original = make_bdt(source);
        let script = parse_bdt("story.bdt", &original).unwrap();
        assert_eq!(
            script.commands,
            vec![
                Command::Label("start".into()),
                Command::RawLine("こんにちは。".into()),
                Command::Label("scene2".into()),
                Command::RawLine("金継ぎの夜。".into()),
            ]
        );

        let replacements = BTreeMap::from([(1, "Hello.".to_string())]);
        let written = rewrite_bdt(&original, &replacements).unwrap();
        assert_eq!(written.replaced, 1);
        assert!(written.unmatched.is_empty());

        // The blank lines are still blank, the label still carries its sigil
        // and its tab, and only line 1 changed.
        assert_eq!(
            decode_bdt_text(&written.data),
            "$start\r\n\r\nHello.\r\n\t%scene2\r\n\r\n金継ぎの夜。\r\n"
        );
        // …and the result is still a BDT this seam can read back.
        let reparsed = parse_bdt("story.bdt", &written.data).unwrap();
        assert_eq!(
            reparsed.commands,
            vec![
                Command::Label("start".into()),
                Command::RawLine("Hello.".into()),
                Command::Label("scene2".into()),
                Command::RawLine("金継ぎの夜。".into()),
            ]
        );
    }

    #[test]
    fn every_command_can_be_replaced_at_once() {
        let original = make_bdt("$a\r\none\r\ntwo\r\n$b\r\nthree\r\n");
        let replacements = BTreeMap::from([
            (1, "ONE".to_string()),
            (2, "TWO".to_string()),
            (4, "THREE".to_string()),
        ]);
        let written = rewrite_bdt(&original, &replacements).unwrap();
        assert_eq!(written.replaced, 3);
        assert_eq!(
            decode_bdt_text(&written.data),
            "$a\r\nONE\r\nTWO\r\n$b\r\nTHREE\r\n"
        );
    }

    #[test]
    fn a_missing_trailing_newline_stays_missing() {
        let original = make_bdt("$a\r\nlast line");
        let written = rewrite_bdt(&original, &BTreeMap::from([(1, "tail".to_string())])).unwrap();
        assert_eq!(decode_bdt_text(&written.data), "$a\r\ntail");
    }

    #[test]
    fn lf_only_files_are_not_converted_to_crlf() {
        let original = make_bdt("$a\none\ntwo\n");
        let written = rewrite_bdt(&original, &BTreeMap::from([(1, "1".to_string())])).unwrap();
        assert_eq!(decode_bdt_text(&written.data), "$a\n1\ntwo\n");
    }

    #[test]
    fn unmatched_ids_are_reported_not_ignored() {
        let original = make_bdt("$a\r\none\r\n");
        let written = rewrite_bdt(&original, &BTreeMap::from([(9, "x".to_string())])).unwrap();
        assert_eq!(written.replaced, 0);
        assert_eq!(written.unmatched, vec![9]);
        // Nothing changed at all.
        assert_eq!(written.data, original);
    }

    #[test]
    fn labels_are_never_rewritten_even_if_asked() {
        // Command 0 is the label. Replacing it would break every jump that
        // targets it, so the writer leaves it alone and reports the miss.
        let original = make_bdt("$start\r\none\r\n");
        let written = rewrite_bdt(&original, &BTreeMap::from([(0, "$other".to_string())])).unwrap();
        assert_eq!(written.replaced, 0);
        assert_eq!(written.unmatched, vec![0]);
        assert_eq!(decode_bdt_text(&written.data), "$start\r\none\r\n");
    }

    #[test]
    fn refuses_a_translation_that_would_change_the_file_structure() {
        let original = make_bdt("$a\r\none\r\n");

        // A line break would split one line into two.
        let err = rewrite_bdt(&original, &BTreeMap::from([(1, "a\nb".to_string())])).unwrap_err();
        assert!(err.to_string().contains("line break"), "{err}");

        // A leading sigil would turn prose into a label.
        let err = rewrite_bdt(&original, &BTreeMap::from([(1, "$oops".to_string())])).unwrap_err();
        assert!(err.to_string().contains("label"), "{err}");
        let err = rewrite_bdt(&original, &BTreeMap::from([(1, "%oops".to_string())])).unwrap_err();
        assert!(err.to_string().contains("label"), "{err}");
    }

    /// The CP932 refusal, through the writer a real patch would use.
    #[test]
    fn refuses_text_the_code_page_cannot_hold() {
        let original = make_bdt("$a\r\none\r\n");
        // An em dash and a simplified-Chinese character: neither is CP932.
        let err =
            rewrite_bdt(&original, &BTreeMap::from([(1, "金継ぎ—金缮".to_string())])).unwrap_err();
        match err {
            Error::Encoding { characters, .. } => assert_eq!(characters, "—缮"),
            other => panic!("expected an encoding refusal, got {other:?}"),
        }
    }

    #[test]
    fn accepts_a_cp932_clean_patch() {
        let original = make_bdt("$a\r\none\r\n");
        let written = rewrite_bdt(
            &original,
            &BTreeMap::from([(1, "金繕いの夜。".to_string())]),
        )
        .unwrap();
        assert_eq!(written.replaced, 1);
        assert_eq!(decode_bdt_text(&written.data), "$a\r\n金繕いの夜。\r\n");
    }

    /// A BDT whose text uses a CP932 spelling that does not survive a
    /// decode/encode round trip: `87 90` is the NEC row-13 `≒`, whose canonical
    /// spelling is `81 E0`. Nothing else in these tests can hold such a byte
    /// sequence, because the fixtures are built from Rust strings.
    fn bdt_with_a_duplicate_spelling() -> (Vec<u8>, Vec<u8>) {
        let mut plain = Vec::new();
        plain.extend_from_slice(b"$start\r\n");
        plain.extend_from_slice(b"\x87\x90\x82\xcc\x93\xfa\r\n"); // ≒の日, NEC spelling
        plain.extend_from_slice(b"\x8b\xe2\x82\xc5\x82\xb7\r\n"); // 金継ぎです
        (plain.iter().map(|b| b ^ 0xFF).collect(), plain)
    }

    #[test]
    fn an_untouched_file_comes_back_byte_for_byte() {
        let (original, plain) = bdt_with_a_duplicate_spelling();

        // The premise, checked rather than assumed: if CP932 round-tripped,
        // this test would pass even with a re-encoding writer.
        let respelled = encode_cp932(&decode_cp932(&plain)).unwrap();
        assert_ne!(
            respelled, plain,
            "this test is vacuous unless decode/encode really is lossy here"
        );

        let written = rewrite_bdt(&original, &BTreeMap::new()).unwrap();
        assert_eq!(written.replaced, 0);
        assert!(
            written.unmatched.is_empty(),
            "an empty request cannot leave anything unmatched"
        );
        assert_eq!(
            written.data, original,
            "an empty replacement map must not change one byte of the file"
        );
    }

    #[test]
    fn a_duplicate_spelling_survives_a_translation_elsewhere() {
        let (original, _) = bdt_with_a_duplicate_spelling();
        // Command 1 is the NEC-spelled line; command 2 is the one below it.
        let written = rewrite_bdt(
            &original,
            &BTreeMap::from([(2, "gold runs through the crack".to_string())]),
        )
        .unwrap();

        assert_eq!(written.replaced, 1);
        let plain_out: Vec<u8> = written.data.iter().map(|b| b ^ 0xFF).collect();
        assert!(
            plain_out.windows(2).any(|pair| pair == [0x87, 0x90]),
            "the NEC-spelled line was re-encoded while translating another line"
        );
        assert!(
            !plain_out.windows(2).any(|pair| pair == [0x81, 0xE0]),
            "the line was rewritten to its canonical spelling"
        );
        assert_eq!(
            decode_bdt_text(&written.data),
            "$start\r\n≒の日\r\ngold runs through the crack\r\n"
        );
    }

    #[test]
    fn an_untouched_line_keeps_its_own_line_ending() {
        // One LF line between CRLF lines, indentation, and no final newline:
        // all of it is copied, none of it is normalized.
        let mut plain = Vec::new();
        plain.extend_from_slice(b"$start\r\n");
        plain.extend_from_slice(b"\tone\n");
        plain.extend_from_slice(b"two");
        let original: Vec<u8> = plain.iter().map(|b| b ^ 0xFF).collect();

        let written = rewrite_bdt(&original, &BTreeMap::from([(3, "two!".to_string())])).unwrap();
        assert_eq!(
            written.data, original,
            "replacing the last line must leave the rest, endings included, alone"
        );
    }
}
