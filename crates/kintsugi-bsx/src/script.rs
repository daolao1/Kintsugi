//! `BSScript`: the compiled story, and the one part of it a repair needs.
//!
//! A `.dat` file in this engine is a header, a block of bytecode, and a list of
//! records described by `(offset, size)` pairs at `0x2C`. The records hold the
//! variable names, the cast list, and the story itself: an index of
//! block-relative offsets immediately followed by a block of NUL-terminated
//! CP932 text, one string per line the game can show.
//!
//! Only the strings are read here — the bytecode is not — and that is enough to
//! repair a translation, because the release refers to its story **by index**
//! into that table and never by address. Measured on the 1,062,484-byte story
//! of the release this seam was written for: of 200 sampled lines, all 200 are
//! named by index somewhere in the file before the text block (785 references),
//! and four have a value that could be read as an offset (7 hits, fewer than
//! chance would produce across 386 KB of data). Rebuilding the table therefore
//! changes no address anywhere: the index is rewritten in place at the same
//! size, the text block — which is the last thing in the file — grows or
//! shrinks, and the one directory pair that measures it is corrected.
//!
//! `docs/RESEARCH-BSX.md` carries the rest: the header fields, the bytecode's
//! shape, and what is still unknown.

use std::collections::BTreeMap;

use kintsugi_core::{Error, Result};

use crate::{decode_cp932, encode_cp932};

/// What a compiled story starts with.
pub const SCRIPT_MAGIC: &[u8] = b"BSScript";

/// Where the record list begins: after the magic and the header's numbers.
pub(crate) const DIRECTORY_START: usize = 0x2C;

/// The most strings a table may claim, as a guard against a corrupt file
/// describing a table the size of a disc.
const MAX_STRINGS: usize = 1 << 22;

/// One `(offset, size)` pair of the record list.
#[derive(Clone, Copy, Debug)]
struct Record {
    at: usize,
    len: usize,
}

/// A string table: an index of block-relative offsets, then the block.
#[derive(Clone, Copy, Debug)]
struct Table {
    index_at: usize,
    /// Which record describes the block, so a repair can correct its size when
    /// the text changes length.
    block_record: usize,
    block_at: usize,
    block_len: usize,
    strings: usize,
}

impl Table {
    fn block_end(&self) -> usize {
        self.block_at + self.block_len
    }
}

/// What a repair did, and what it could not.
#[derive(Clone, Debug)]
pub struct Repair {
    /// The whole file, rebuilt.
    pub bytes: Vec<u8>,
    /// How many lines the repair changed.
    pub replaced: usize,
    /// Line indices the story does not have: ids drift when a translation and
    /// a story come from different versions of a game.
    pub unmatched: Vec<usize>,
}

/// A compiled story, read out of a `.dat` file.
#[derive(Clone, Debug)]
pub struct Story {
    bytes: Vec<u8>,
    table: Table,
    strings: Vec<String>,
}

impl Story {
    /// Read a compiled story.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < SCRIPT_MAGIC.len() {
            return Err(Error::corrupt(
                label(),
                format!(
                    "a compiled story starts with {:?} and is at least {} bytes, this file is {}",
                    String::from_utf8_lossy(SCRIPT_MAGIC),
                    DIRECTORY_START + 8,
                    bytes.len()
                ),
            ));
        }
        let head = &bytes[..SCRIPT_MAGIC.len()];
        if head != SCRIPT_MAGIC {
            return Err(Error::corrupt(
                label(),
                format!(
                    "this file starts with {:?}, not {:?}",
                    String::from_utf8_lossy(head),
                    String::from_utf8_lossy(SCRIPT_MAGIC)
                ),
            ));
        }
        if bytes.len() < DIRECTORY_START + 8 {
            return Err(Error::corrupt(
                label(),
                format!(
                    "a compiled story is at least {} bytes, this file is {}",
                    DIRECTORY_START + 8,
                    bytes.len()
                ),
            ));
        }

        let records = read_records(bytes)?;
        let table = find_story(bytes, &records)?;
        let strings = read_strings(bytes, &table)?;
        Ok(Self {
            bytes: bytes.to_vec(),
            table,
            strings,
        })
    }

    /// The story's lines, in the order the game indexes them.
    pub fn strings(&self) -> &[String] {
        &self.strings
    }

    /// The bytes this story was read from.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The file with `replacements` standing in for the lines they name.
    ///
    /// A line the map does not name, or names with its own text, is left
    /// exactly as it was. The rebuilt file is read back as a story and compared
    /// with what this call meant to write before it is handed over: a repair
    /// that does not read back as itself is a bug, not a patch.
    pub fn rebuild(&self, replacements: &BTreeMap<usize, String>) -> Result<Repair> {
        let mut wanted = Vec::with_capacity(self.strings.len());
        let mut replaced = 0usize;
        let mut unmatched = Vec::new();
        for (index, original) in self.strings.iter().enumerate() {
            match replacements.get(&index) {
                Some(text) if text != original => {
                    replaced += 1;
                    wanted.push(text.clone());
                }
                Some(_) => wanted.push(original.clone()),
                None => wanted.push(original.clone()),
            }
        }
        for id in replacements.keys() {
            if *id >= self.strings.len() {
                unmatched.push(*id);
            }
        }

        let mut index = Vec::with_capacity(self.table.strings * 4);
        let mut block = Vec::new();
        for (line, text) in wanted.iter().enumerate() {
            let encoded = encode_cp932(text).map_err(|refused| Error::Script {
                context: format!("line {line} of the story"),
                detail: format!(
                    "the repair cannot be written: {refused}. A line this seam cannot encode is \
                     refused rather than approximated, because the game would render the \
                     approximation as garbage"
                ),
            })?;
            index.extend_from_slice(&(block.len() as u32).to_le_bytes());
            block.extend_from_slice(&encoded);
            block.push(0);
        }

        // Nothing may live after the text: this seam moves the text by
        // rewriting it, and a record that began out there would be left
        // pointing at whatever ends up in its place.
        let was_after = self.table.block_end();
        for (position, record) in read_records(&self.bytes)?.iter().enumerate() {
            if record.at >= was_after {
                return Err(Error::unsupported(
                    label(),
                    format!(
                        "record {position} of this file ({} byte(s) at 0x{:x}) begins after the \
                         story's text ends at 0x{was_after:x}, and this seam does not know how to \
                         move it out of the repair's way",
                        record.len, record.at
                    ),
                ));
            }
        }

        let mut bytes = self.bytes.clone();
        bytes[self.table.index_at..self.table.index_at + index.len()].copy_from_slice(&index);
        let pair = DIRECTORY_START + self.table.block_record * 8;
        bytes[pair + 4..pair + 8].copy_from_slice(&(block.len() as u32).to_le_bytes());
        // Rebuilt in place: everything before the text, the new text, and
        // everything that followed the old text — which the release keeps
        // nothing of, and a padded file keeps its padding.
        let mut rebuilt = Vec::with_capacity(bytes.len() - self.table.block_len + block.len());
        rebuilt.extend_from_slice(&bytes[..self.table.block_at]);
        rebuilt.extend_from_slice(&block);
        rebuilt.extend_from_slice(&bytes[was_after..]);
        let bytes = rebuilt;

        let reread = Story::parse(&bytes)?;
        if reread.strings != wanted {
            return Err(Error::corrupt(
                label(),
                "the rebuilt story does not read back as the lines that were written into it"
                    .to_string(),
            ));
        }

        Ok(Repair {
            bytes,
            replaced,
            unmatched,
        })
    }
}

/// The pair list at [`DIRECTORY_START`], up to the first pair that is not a
/// record inside the file.
///
/// The release this seam was written for describes thirteen records and then
/// four pairs that point past the end of the file — that is what says the list
/// has ended, and the same rule is used here rather than one that guesses at a
/// count from a header field whose meaning is not established.
fn read_records(bytes: &[u8]) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    let mut at = DIRECTORY_START;
    while at + 8 <= bytes.len() {
        let offset =
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        let size = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
            as usize;
        if size == 0 || offset < DIRECTORY_START || offset + size > bytes.len() {
            break;
        }
        records.push(Record {
            at: offset,
            len: size,
        });
        at += 8;
    }
    if records.is_empty() {
        return Err(Error::corrupt(
            label(),
            format!("no record is described at 0x{DIRECTORY_START:x}"),
        ));
    }
    Ok(records)
}

/// The story: the **last** record in the file, and larger than every other
/// string table in it.
///
/// A compiler writes the string heap last because it is the only part whose
/// size depends on the text; the release puts its 11,864-line story there, and
/// the four `@` variables, the twelve `#` variables and the twenty cast names
/// in the tables before it. Requiring both — that the last record is a string
/// table, and that it holds more lines than any other — is what stops a repair
/// aimed at a story whose block was damaged from quietly landing on the cast
/// list instead: with the story unreadable, no candidate is left that satisfies
/// both, and this seam refuses rather than rewriting a game's variable names
/// into a translation.
fn find_story(bytes: &[u8], records: &[Record]) -> Result<Table> {
    let mut candidates = Vec::new();
    for (position, pair) in records.windows(2).enumerate() {
        let (index, block) = (pair[0], pair[1]);
        if index.at + index.len != block.at || index.len % 4 != 0 {
            continue;
        }
        let strings = index.len / 4;
        if strings == 0 || strings > MAX_STRINGS {
            continue;
        }
        let table = Table {
            index_at: index.at,
            block_record: position + 1,
            block_at: block.at,
            block_len: block.len,
            strings,
        };
        if fits_a_string_block(bytes, &table) {
            candidates.push(table);
        }
    }

    let last = records.len() - 1;
    let Some(story) = candidates
        .iter()
        .copied()
        .find(|table| table.block_record == last)
    else {
        return Err(Error::unsupported(
            label(),
            format!(
                "the last of the {} record(s) in this file is not an index followed by a block of \
                 text, and this seam does not look for a story anywhere else in it",
                records.len()
            ),
        ));
    };

    let mut larger: Vec<&Table> = candidates
        .iter()
        .filter(|table| table.block_record != last && table.strings >= story.strings)
        .collect();
    if !larger.is_empty() {
        larger.sort_by_key(|table| table.block_at);
        return Err(Error::unsupported(
            label(),
            format!(
                "the last record holds {} line(s) and so does the table at {}, so this seam will \
                 not guess which of them is the story",
                story.strings,
                larger
                    .iter()
                    .map(|table| format!("0x{:x}", table.block_at))
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
        ));
    }
    Ok(story)
}

/// Whether `table`'s index describes exactly the block after it: offsets that
/// start at zero, never go backwards, and put a NUL at every boundary.
fn fits_a_string_block(bytes: &[u8], table: &Table) -> bool {
    let end = table.block_end();
    if end > bytes.len() || table.block_len == 0 {
        return false;
    }
    let mut previous: Option<usize> = None;
    for slot in 0..table.strings {
        let at = table.index_at + slot * 4;
        let offset =
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        if offset >= table.block_len {
            return false;
        }
        match previous {
            // The first line starts the block.
            None if offset == 0 => {}
            None => return false,
            // The line before this offset ends in the NUL immediately before
            // it, and holds none of its own; only then does the index describe
            // the whole block rather than a sample of it.
            Some(last) if offset > last => {
                let starts = table.block_at + last;
                let terminates = table.block_at + offset - 1;
                if bytes[terminates] != 0 || bytes[starts..terminates].contains(&0) {
                    return false;
                }
            }
            Some(_) => return false,
        }
        previous = Some(offset);
    }
    // And the last line ends at the end of the block, with nothing after it.
    match previous {
        Some(last) => {
            let starts = table.block_at + last;
            bytes[end - 1] == 0 && !bytes[starts..end - 1].contains(&0)
        }
        None => false,
    }
}

/// Decode the table's strings, refusing text this seam cannot hand over intact.
fn read_strings(bytes: &[u8], table: &Table) -> Result<Vec<String>> {
    let mut strings = Vec::with_capacity(table.strings);
    for slot in 0..table.strings {
        let at = table.index_at + slot * 4;
        let start =
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        let end = if slot + 1 < table.strings {
            let next = table.index_at + (slot + 1) * 4;
            u32::from_le_bytes([
                bytes[next],
                bytes[next + 1],
                bytes[next + 2],
                bytes[next + 3],
            ]) as usize
                - 1
        } else {
            table.block_len - 1
        };
        let raw = &bytes[table.block_at + start..table.block_at + end];
        let (text, lossy) = decode_cp932(raw);
        if lossy {
            return Err(Error::Script {
                context: format!("line {slot} of the story"),
                detail: format!(
                    "{} byte(s) at 0x{:x} are not CP932 text. A seam that cannot read a line must \
                     not offer to rewrite the block around it, so the file is refused whole",
                    raw.len(),
                    table.block_at + start
                ),
            });
        }
        strings.push(text);
    }
    Ok(strings)
}

/// What to call this file in an error message.
fn label() -> String {
    String::from("BSScript")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{make_bsx_dat, make_story};

    fn replacements(pairs: &[(usize, &str)]) -> BTreeMap<usize, String> {
        pairs
            .iter()
            .map(|(id, text)| (*id, (*text).to_string()))
            .collect()
    }

    #[test]
    fn a_story_reads_back_the_lines_it_was_built_from() {
        let story = Story::parse(&make_bsx_dat()).expect("the fixture is a story");
        assert_eq!(story.strings().len(), 6);
        assert_eq!(story.strings()[0], "■■■　真理奈ＥＮＤ　■■■");
        assert_eq!(story.strings()[5], "見に行かない");
        // The name lists beside it are not the story, and are not returned.
        assert!(!story.strings().iter().any(|line| line == "@harem"));
        assert!(!story.strings().iter().any(|line| line == "孝三"));
    }

    #[test]
    fn a_name_list_is_never_mistaken_for_the_story() {
        // Four lines is the smallest story the fixture's three name lists
        // cannot tie with, and the reader has to pick it, not them.
        let bytes = make_story(&["一", "二", "三", "四"]);
        let story = Story::parse(&bytes).expect("four lines beat three");
        assert_eq!(story.strings(), ["一", "二", "三", "四"]);
    }

    #[test]
    fn two_equally_long_tables_are_refused_rather_than_guessed_between() {
        // Three lines ties with the cast list, and the file says so.
        let refused = Story::parse(&make_story(&["一", "二", "三"]))
            .expect_err("a tie is not a story this seam picks");
        let message = format!("{refused}");
        assert!(
            message.contains("the last record holds 3 line(s)"),
            "{message}"
        );
        assert!(message.contains("will not guess"), "{message}");
    }

    #[test]
    fn a_replacement_changes_one_line_and_leaves_the_others_alone() {
        let original = make_bsx_dat();
        let story = Story::parse(&original).expect("the fixture is a story");
        let repaired = story
            .rebuild(&replacements(&[(2, "What is that difference!?")]))
            .expect("the line encodes");

        assert_eq!(repaired.replaced, 1);
        assert!(repaired.unmatched.is_empty());
        let reread = Story::parse(&repaired.bytes).expect("a repair is a story");
        assert_eq!(reread.strings()[2], "What is that difference!?");
        for index in [0, 1, 3, 4, 5] {
            assert_eq!(reread.strings()[index], story.strings()[index]);
        }
        // Everything before the index table is untouched — the header, the
        // directory and the name lists — except the one number that measures
        // the text block, which is a repair's business to correct.
        let reread = Story::parse(&repaired.bytes).expect("a repair is a story");
        let mut expected = original[..story.table.index_at].to_vec();
        let pair = DIRECTORY_START + reread.table.block_record * 8;
        expected[pair + 4..pair + 8].copy_from_slice(&repaired.bytes[pair + 4..pair + 8]);
        assert_eq!(repaired.bytes[..story.table.index_at], expected[..]);
    }

    #[test]
    fn a_longer_line_grows_the_block_and_corrects_the_record_that_measures_it() {
        let story = Story::parse(&make_bsx_dat()).expect("the fixture is a story");
        let long = "a line that is much longer than the six bytes it replaces, twice over";
        let repaired = story
            .rebuild(&replacements(&[(5, long)]))
            .expect("the line encodes");

        assert!(repaired.bytes.len() > story.bytes.len());
        let reread = Story::parse(&repaired.bytes).expect("a repair is a story");
        assert_eq!(reread.strings()[5], long);
        // The directory pair that measures the block was corrected, and the
        // file ends exactly where the block does.
        let pair = DIRECTORY_START + reread.table.block_record * 8;
        let size = u32::from_le_bytes([
            repaired.bytes[pair + 4],
            repaired.bytes[pair + 5],
            repaired.bytes[pair + 6],
            repaired.bytes[pair + 7],
        ]) as usize;
        assert_eq!(size, reread.table.block_len);
        // And the bytes the fixture keeps after its text are still there.
        assert_eq!(
            repaired.bytes[reread.table.block_end()..],
            story.bytes()[story.table.block_end()..]
        );
    }

    #[test]
    fn a_shorter_line_shrinks_the_file() {
        let story = Story::parse(&make_bsx_dat()).expect("the fixture is a story");
        let repaired = story
            .rebuild(&replacements(&[(1, "a")]))
            .expect("the line encodes");
        assert!(repaired.bytes.len() < story.bytes.len());
        assert_eq!(
            Story::parse(&repaired.bytes)
                .expect("a repair is a story")
                .strings()[1],
            "a"
        );
    }

    #[test]
    fn a_line_named_with_its_own_text_is_not_a_repair() {
        let story = Story::parse(&make_bsx_dat()).expect("the fixture is a story");
        let same = story.strings()[2].clone();
        let repaired = story
            .rebuild(&replacements(&[(2, &same)]))
            .expect("nothing to do is not an error");
        assert_eq!(repaired.replaced, 0);
        // Every byte back, including the padding a fixture keeps after its
        // text: a repair that changes nothing changes nothing.
        assert_eq!(repaired.bytes, story.bytes());
    }

    #[test]
    fn a_line_the_story_does_not_have_is_reported_rather_than_dropped() {
        let story = Story::parse(&make_bsx_dat()).expect("the fixture is a story");
        let repaired = story
            .rebuild(&replacements(&[(2, "kept"), (99, "nowhere")]))
            .expect("the line that exists still encodes");
        assert_eq!(repaired.replaced, 1);
        assert_eq!(repaired.unmatched, vec![99]);
    }

    #[test]
    fn text_that_cannot_be_written_is_refused_and_names_the_line() {
        let story = Story::parse(&make_bsx_dat()).expect("the fixture is a story");
        let refused = story
            .rebuild(&replacements(&[(
                3,
                "an emoji is not in this character set: 😀",
            )]))
            .expect_err("CP932 has no emoji");
        assert!(matches!(refused, Error::Script { .. }), "{refused}");
        let message = format!("{refused}");
        assert!(message.contains("line 3 of the story"), "{message}");
        assert!(message.contains('😀'), "{message}");
    }

    #[test]
    fn a_file_that_is_not_a_story_is_refused_with_its_magic() {
        let mut archive = b"BSArc\0\0\0".to_vec();
        archive.resize(DIRECTORY_START + 8, 0);
        let refused = Story::parse(&archive).expect_err("an archive is not a story");
        let message = format!("{refused}");
        assert!(message.contains("BSArc"), "{message}");
        assert!(message.contains("BSScript"), "{message}");
    }

    #[test]
    fn a_file_too_short_to_hold_a_record_list_is_refused_with_its_length() {
        let refused = Story::parse(b"BSScript").expect_err("nine bytes are not a story");
        let message = format!("{refused}");
        assert!(message.contains("this file is 8"), "{message}");
    }

    #[test]
    fn a_story_that_has_bytes_after_its_text_keeps_them() {
        // The release's text block is the last thing in its file; a padded
        // fixture's is not, and a repair must not quietly trim the difference.
        let mut padded = make_bsx_dat();
        padded.extend_from_slice(b"trailing bytes that belong to nobody");
        let story = Story::parse(&padded).expect("trailing bytes are not a record");
        let repaired = story
            .rebuild(&replacements(&[(0, "replaced")]))
            .expect("the line encodes");
        assert_eq!(repaired.replaced, 1);
        let reread = Story::parse(&repaired.bytes).expect("a repair is a story");
        assert_eq!(reread.strings()[0], "replaced");
        assert_eq!(
            &repaired.bytes[reread.table.block_end()..],
            &padded[story.table.block_end()..],
            "the padding and the trailing bytes both come through the repair"
        );
        assert!(
            repaired
                .bytes
                .ends_with(b"trailing bytes that belong to nobody")
        );
    }

    #[test]
    fn a_story_whose_text_block_was_damaged_is_refused() {
        let mut bytes = make_bsx_dat();
        // A byte of the text block turned into a NUL splits a line in two and
        // stops the index from describing the block.
        let story = Story::parse(&bytes).expect("the fixture is a story");
        bytes[story.table.block_at + 4] = 0;
        let refused = Story::parse(&bytes).expect_err("the index no longer fits the block");
        let message = format!("{refused}");
        // The name lists are still intact and still parse; what stops them from
        // being taken for the story is that they are not the last record.
        assert!(
            message.contains("does not look for a story anywhere else"),
            "{message}"
        );
    }

    #[test]
    fn a_truncated_file_is_refused_rather_than_read_past_its_end() {
        let bytes = make_bsx_dat();
        let story = Story::parse(&bytes).expect("the fixture is a story");
        let cut = &bytes[..story.table.block_end() - 3];
        assert!(Story::parse(cut).is_err());
    }
}
