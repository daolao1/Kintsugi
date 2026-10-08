//! SNN + INX: BlueGale's resource archive pair.
//!
//! The `.inx` file is the index: a `u32` entry count followed by `count`
//! records of `0x48` bytes — a CP932 name in a fixed `0x40`-byte field,
//! then a `u32` offset and a `u32` size into the sibling `.snn` blob. The
//! `.snn` file is raw entry data, nothing more.
//!
//! Layout verified against GARbro's `ArcFormats/BlueGale/ArcSNN.cs`
//! (morkt, MIT) — see `docs/RESEARCH-BlueGale.md`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use kintsugi_core::bytes::Reader;
use kintsugi_core::error::{Error, Result};
use kintsugi_core::vfs::{FileSource, VirtualPath};

/// Record size of one INX entry: `0x40` name + `u32` offset + `u32` size.
pub const INX_RECORD_SIZE: usize = 0x48;
/// Fixed name field size inside an INX record.
pub const INX_NAME_SIZE: usize = 0x40;
/// Upper bound on entries before we call the index corrupt.
pub const MAX_ENTRIES: usize = 1 << 20;

/// One parsed INX record.
///
/// The name is kept twice: decoded, because that is what a person and a lookup
/// use, and **as the exact bytes the record holds**, because those are what
/// writing the record back must emit. CP932 has some four hundred characters
/// with two or three valid spellings, so re-encoding a decoded name is a way to
/// change a file nobody asked to change (README, "The CP932 note").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InxEntry {
    /// Entry name as stored (usually with extension, e.g. `TITLE.ZBM`).
    pub name: String,
    /// The `0x40`-byte name field exactly as it appears in the index, NUL
    /// padding and all.
    pub name_raw: Vec<u8>,
    /// Byte offset into the SNN blob.
    pub offset: u32,
    /// Entry size in bytes.
    pub size: u32,
}

impl InxEntry {
    /// An entry with `name` encoded into the record's fixed name field.
    ///
    /// Only for building archives (fixtures, tests): a record that was *parsed*
    /// keeps the bytes it was parsed from instead of being re-encoded.
    pub fn new(name: &str, offset: u32, size: u32) -> Result<Self> {
        let encoded = crate::encode_cp932(name)?;
        let mut name_raw = vec![0u8; INX_NAME_SIZE];
        if encoded.len() > INX_NAME_SIZE {
            return Err(Error::corrupt(
                "SNN/INX archive",
                format!(
                    "entry name '{name}' needs {} bytes and the record's name field holds \
                     {INX_NAME_SIZE}",
                    encoded.len()
                ),
            ));
        }
        name_raw[..encoded.len()].copy_from_slice(&encoded);
        Ok(Self {
            name: name.to_string(),
            name_raw,
            offset,
            size,
        })
    }

    /// The record as it is written to an index: `0x40` name bytes, offset, size.
    fn to_record(&self) -> [u8; INX_RECORD_SIZE] {
        let mut record = [0u8; INX_RECORD_SIZE];
        let raw = &self.name_raw;
        let taken = raw.len().min(INX_NAME_SIZE);
        record[..taken].copy_from_slice(&raw[..taken]);
        record[INX_NAME_SIZE..INX_NAME_SIZE + 4].copy_from_slice(&self.offset.to_le_bytes());
        record[INX_NAME_SIZE + 4..INX_RECORD_SIZE].copy_from_slice(&self.size.to_le_bytes());
        record
    }
}

/// Parse a `.inx` index.
///
/// Fails with [`Error::Corrupt`] on a malformed index; callers use that
/// failure to *reject* detection, the same way the reference
/// implementation returns null.
pub fn parse_inx(data: &[u8]) -> Result<Vec<InxEntry>> {
    let mut reader = Reader::new(data);
    let count = reader.u32le()? as usize;
    if count == 0 || count > MAX_ENTRIES {
        return Err(Error::corrupt(
            "SNN/INX archive",
            format!("entry count {count} is out of bounds"),
        ));
    }
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let name_raw = reader.take(INX_NAME_SIZE)?;
        let offset = reader.u32le()?;
        let size = reader.u32le()?;
        let end = name_raw
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(INX_NAME_SIZE);
        let name = crate::decode_cp932(&name_raw[..end]).trim().to_string();
        if name.is_empty() {
            return Err(Error::corrupt(
                "SNN/INX archive",
                format!("entry {i} has an empty name"),
            ));
        }
        entries.push(InxEntry {
            name,
            name_raw: name_raw.to_vec(),
            offset,
            size,
        });
    }
    Ok(entries)
}

/// An index as it was parsed: the records, and whatever followed them.
///
/// The tail matters. A parser that only understands the records would drop any
/// byte after them when writing the index back — a "repair" that quietly
/// shortens a file. What the tail *is* we do not know (padding, a second
/// section, a tool's signature), and not knowing is a reason to carry it over
/// untouched rather than to decide it is not needed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InxArchive {
    /// The records, in index order.
    pub entries: Vec<InxEntry>,
    /// Every byte after the last record.
    pub tail: Vec<u8>,
}

impl InxArchive {
    /// Parse an index and keep its tail.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let entries = parse_inx(data)?;
        let records_end = 4 + entries.len() * INX_RECORD_SIZE;
        let tail = data.get(records_end..).unwrap_or_default().to_vec();
        Ok(Self { entries, tail })
    }

    /// Write the index back: count, records, tail.
    ///
    /// With no entry changed this reproduces the parsed file byte for byte,
    /// including the raw spellings of the names, which is the property the
    /// whole write path rests on.
    pub fn write(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(4 + self.entries.len() * INX_RECORD_SIZE + self.tail.len());
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for entry in &self.entries {
            out.extend_from_slice(&entry.to_record());
        }
        out.extend_from_slice(&self.tail);
        out
    }
}

/// A mounted SNN archive: parsed index + the raw blob, joined.
#[derive(Debug)]
pub struct SnnArchive {
    /// Archive file name (e.g. `game.snn`), for reports.
    pub archive_name: String,
    /// The raw SNN data.
    pub data: Arc<Vec<u8>>,
    /// The parsed index: records in order, plus whatever followed them.
    pub index: InxArchive,
    /// `name.lowercased() → entry index`.
    lookup: HashMap<String, usize>,
}

impl SnnArchive {
    /// Join an INX index with its SNN blob, validating every placement.
    pub fn open(
        inx_bytes: &[u8],
        snn_bytes: Vec<u8>,
        archive_name: impl Into<String>,
    ) -> Result<Self> {
        let index = InxArchive::parse(inx_bytes)?;
        let entries = index.entries.clone();
        let data = Arc::new(snn_bytes);
        for entry in &entries {
            let end = entry.offset as u64 + entry.size as u64;
            if end > data.len() as u64 {
                return Err(Error::corrupt(
                    "SNN/INX archive",
                    format!(
                        "entry '{}' spans [{}, {}) but '{}' holds {} bytes",
                        entry.name,
                        entry.offset,
                        end,
                        archive_name_shown(&archive_name.into().to_string()),
                        data.len()
                    ),
                ));
            }
        }
        let archive_name = archive_name.into();
        let lookup = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.name.to_ascii_lowercase(), i))
            .collect();
        Ok(Self {
            archive_name,
            data,
            index,
            lookup,
        })
    }

    /// Raw bytes of entry `index`.
    pub fn read_entry(&self, index: usize) -> Result<&[u8]> {
        let entry = self
            .index
            .entries
            .get(index)
            .ok_or_else(|| Error::NotFound(format!("SNN entry #{index}")))?;
        let start = entry.offset as usize;
        let end = start + entry.size as usize;
        Ok(&self.data[start..end])
    }

    /// Entry index for a (case-insensitive) name.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.lookup.get(&name.to_ascii_lowercase()).copied()
    }

    /// The archive rebuilt with some entries replaced, as the two files it
    /// really is: `(snn bytes, inx bytes)`.
    ///
    /// A replacement that fits in the space its entry already has is written
    /// there, so every other byte in the blob keeps its offset. One that does
    /// not fit goes at the end of the blob with the index pointing at it,
    /// leaving the entry's old bytes behind as dead space. The alternative —
    /// repacking the archive so everything stays contiguous — would move
    /// resources that have nothing to do with the repair, and a resource that
    /// does not move is a resource this tool cannot have broken. It is also why
    /// the index is rewritten from the parsed records: same names, same order,
    /// same raw spellings, only the changed entry's offset and size differ.
    ///
    /// With no replacements this reproduces both files byte for byte, which is
    /// the property the whole write path rests on and the seam contract checks.
    pub fn rebuilt(&self, replacements: &BTreeMap<usize, Vec<u8>>) -> Result<(Vec<u8>, Vec<u8>)> {
        if let Some((first, second)) = self.overlapping_entries() {
            return Err(Error::unsupported(
                "SNN/INX archive",
                format!(
                    "entries '{first}' and '{second}' in '{}' share bytes, so replacing one \
                     would change the other. This tool will not guess which resource those \
                     bytes belong to",
                    self.archive_name
                ),
            ));
        }
        let mut blob = (*self.data).clone();
        let mut entries = self.index.entries.clone();
        for (index, new_bytes) in replacements {
            let total = entries.len();
            let entry = entries.get_mut(*index).ok_or_else(|| {
                Error::NotFound(format!(
                    "SNN entry #{index} in '{}' (it holds {total} entries)",
                    self.archive_name
                ))
            })?;
            let fits = new_bytes.len() <= entry.size as usize;
            let offset = if fits {
                entry.offset as usize
            } else {
                let at = blob.len();
                blob.extend_from_slice(new_bytes);
                at
            };
            if fits {
                blob[offset..offset + new_bytes.len()].copy_from_slice(new_bytes);
            }
            entry.offset = offset as u32;
            entry.size = new_bytes.len() as u32;
        }
        let index = InxArchive {
            entries,
            tail: self.index.tail.clone(),
        };
        Ok((blob, index.write()))
    }

    /// Two entries whose byte ranges intersect, if the archive has any.
    ///
    /// Deduplicated resources are a real thing in archives, and a rewrite that
    /// silently changed a second resource is exactly the kind of damage a
    /// repair tool must not do.
    fn overlapping_entries(&self) -> Option<(String, String)> {
        let mut sorted: Vec<&InxEntry> = self
            .index
            .entries
            .iter()
            .filter(|entry| entry.size > 0)
            .collect();
        sorted.sort_by_key(|entry| entry.offset);
        sorted.windows(2).find_map(|pair| {
            let (first, second) = (pair[0], pair[1]);
            let first_end = first.offset as u64 + first.size as u64;
            (first_end > second.offset as u64).then(|| (first.name.clone(), second.name.clone()))
        })
    }
}

fn archive_name_shown(name: &str) -> &str {
    if name.is_empty() { "snn blob" } else { name }
}

/// Serves one mounted SNN archive as a [`FileSource`].
pub struct SnnSource {
    archive: Arc<SnnArchive>,
}

impl SnnSource {
    /// Serve `archive` into a [`Vfs`].
    pub fn new(archive: Arc<SnnArchive>) -> Self {
        Self { archive }
    }
}

impl FileSource for SnnSource {
    fn read(&self, path: &VirtualPath) -> Result<Vec<u8>> {
        match self.archive.find(path.as_str()) {
            Some(index) => Ok(self.archive.read_entry(index)?.to_vec()),
            None => Err(Error::NotFound(format!(
                "'{path}' in {}",
                self.archive.archive_name
            ))),
        }
    }

    fn list(&self) -> Vec<(VirtualPath, u64)> {
        self.archive
            .index
            .entries
            .iter()
            .map(|e| (VirtualPath::new(&e.name), e.size as u64))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_inx(entries: &[(&str, u32, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (name, offset, size) in entries {
            let mut field = [0u8; INX_NAME_SIZE];
            field[..name.len()].copy_from_slice(name.as_bytes());
            out.extend_from_slice(&field);
            out.extend_from_slice(&offset.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
        }
        out
    }

    #[test]
    fn parses_index_records() {
        let inx = build_inx(&[("TITLE.ZBM", 0, 3), ("ROOM.ZBM", 3, 2)]);
        let entries = parse_inx(&inx).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "TITLE.ZBM");
        assert_eq!(entries[0].offset, 0);
        assert_eq!(entries[1].size, 2);
    }

    /// The property everything else rests on: an archive rewritten with nothing
    /// to change is the archive it was, byte for byte — both files, including
    /// gaps between entries, bytes after the last entry, and the raw CP932
    /// spellings of the names.
    #[test]
    fn rebuilding_with_no_replacements_reproduces_the_archive() {
        // Deliberately untidy: a two-byte gap between entries, dead bytes after
        // the last one, and something after the index records that this parser
        // does not understand.
        let mut blob = Vec::new();
        blob.extend_from_slice(b"AAAA");
        blob.extend_from_slice(&[0xEE, 0xEE]); // gap
        blob.extend_from_slice(b"BB");
        blob.extend_from_slice(&[0x00, 0x11, 0x22]); // trailing bytes
        let mut inx = build_inx(&[("TITLE.ZBM", 0, 4), ("ROOM.ZBM", 6, 2)]);
        inx.extend_from_slice(b"trailer");

        let archive = SnnArchive::open(&inx, blob.clone(), "game.snn").unwrap();
        let (new_blob, new_inx) = archive.rebuilt(&BTreeMap::new()).unwrap();
        assert_eq!(new_blob, blob, "the blob must come back untouched");
        assert_eq!(new_inx, inx, "the index must come back untouched");
    }

    /// A smaller replacement is written where the entry already is, so nothing
    /// that has nothing to do with the repair moves.
    #[test]
    fn a_smaller_replacement_stays_where_it_was() {
        let (blob, placements) = crate::fixtures::make_snn(&[b"AAAA", b"BBBB"]);
        let (offset_a, size_a) = placements[0];
        let (offset_b, size_b) = placements[1];
        let inx = build_inx(&[("A", offset_a, size_a), ("B", offset_b, size_b)]);
        let archive = SnnArchive::open(&inx, blob, "game.snn").unwrap();

        let (new_blob, new_inx) = archive
            .rebuilt(&BTreeMap::from([(0usize, b"x".to_vec())]))
            .unwrap();
        let entries = parse_inx(&new_inx).unwrap();
        assert_eq!(&new_blob[0..1], b"x");
        assert_eq!(&new_blob[1..4], b"AAA", "the old bytes stay as dead space");
        assert_eq!(&new_blob[4..8], b"BBBB", "the other entry did not move");
        assert_eq!((entries[0].offset, entries[0].size), (0, 1));
        assert_eq!((entries[1].offset, entries[1].size), (4, 4));
    }

    /// A larger replacement goes at the end of the blob, and the index points
    /// at it: the entry that grew moves rather than everything after it.
    #[test]
    fn a_larger_replacement_is_appended_and_indexed() {
        let (blob, placements) = crate::fixtures::make_snn(&[b"AAAA", b"BBBB"]);
        let (offset_a, size_a) = placements[0];
        let (offset_b, size_b) = placements[1];
        let inx = build_inx(&[("A", offset_a, size_a), ("B", offset_b, size_b)]);
        let archive = SnnArchive::open(&inx, blob, "game.snn").unwrap();

        let (new_blob, new_inx) = archive
            .rebuilt(&BTreeMap::from([(0usize, b"longer than before".to_vec())]))
            .unwrap();
        let entries = parse_inx(&new_inx).unwrap();
        assert_eq!(new_blob.len(), 8 + 18);
        assert_eq!(&new_blob[8..], b"longer than before");
        assert_eq!((entries[0].offset, entries[0].size), (8, 18));
        assert_eq!(
            (entries[1].offset, entries[1].size),
            (4, 4),
            "the entry after it did not move"
        );
        assert_eq!(&new_blob[0..4], b"AAAA", "the old bytes are still there");
    }

    /// Two entries pointing at the same bytes cannot be rewritten one at a
    /// time: changing one changes the other, and guessing which resource the
    /// bytes belong to is not this tool's job.
    #[test]
    fn entries_that_share_bytes_are_refused() {
        let inx = build_inx(&[("A", 0, 4), ("B", 2, 2)]);
        let archive = SnnArchive::open(&inx, b"ABCD".to_vec(), "game.snn").unwrap();
        let error = archive
            .rebuilt(&BTreeMap::from([(0usize, b"x".to_vec())]))
            .unwrap_err();
        let message = format!("{error}");
        assert!(
            message.contains("share bytes") && message.contains('A') && message.contains('B'),
            "the refusal must name both entries: {message}"
        );
    }

    #[test]
    fn rejects_absurd_counts() {
        assert!(parse_inx(&0u32.to_le_bytes()).is_err());
        let mut inx = (u32::MAX).to_le_bytes().to_vec();
        inx.extend_from_slice(&[0u8; 8]);
        assert!(parse_inx(&inx).is_err());
    }

    #[test]
    fn archive_validates_placement() {
        let inx = build_inx(&[("A", 0, 4)]);
        assert!(SnnArchive::open(&inx, b"1234".to_vec(), "a.snn").is_ok());
        assert!(SnnArchive::open(&inx, b"123".to_vec(), "a.snn").is_err());
    }

    #[test]
    fn source_reads_and_lists() {
        let inx = build_inx(&[("TITLE.ZBM", 0, 3), ("ROOM.ZBM", 3, 2)]);
        let archive = SnnArchive::open(&inx, b"abcdef".to_vec(), "game.snn").unwrap();
        let source = SnnSource::new(Arc::new(archive));
        let data = source.read(&VirtualPath::new("title.zbm")).unwrap();
        assert_eq!(data, b"abc");
        assert!(source.read(&VirtualPath::new("nope.zbm")).is_err());
        let listed = source.list();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].1, 3);
    }
}
