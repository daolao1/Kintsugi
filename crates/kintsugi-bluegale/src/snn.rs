//! SNN + INX: BlueGale's resource archive pair.
//!
//! The `.inx` file is the index: a `u32` entry count followed by `count`
//! records of `0x48` bytes — a CP932 name in a fixed `0x40`-byte field,
//! then a `u32` offset and a `u32` size into the sibling `.snn` blob. The
//! `.snn` file is raw entry data, nothing more.
//!
//! Layout verified against GARbro's `ArcFormats/BlueGale/ArcSNN.cs`
//! (morkt, MIT) — see `docs/RESEARCH-BlueGale.md`.

use std::collections::HashMap;
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InxEntry {
    /// Entry name as stored (usually with extension, e.g. `TITLE.ZBM`).
    pub name: String,
    /// Byte offset into the SNN blob.
    pub offset: u32,
    /// Entry size in bytes.
    pub size: u32,
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
        entries.push(InxEntry { name, offset, size });
    }
    Ok(entries)
}

/// A mounted SNN archive: parsed index + the raw blob, joined.
#[derive(Debug)]
pub struct SnnArchive {
    /// Archive file name (e.g. `game.snn`), for reports.
    pub archive_name: String,
    /// The raw SNN data.
    pub data: Arc<Vec<u8>>,
    /// Parsed INX entries, in index order.
    pub entries: Vec<InxEntry>,
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
        let entries = parse_inx(inx_bytes)?;
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
            entries,
            lookup,
        })
    }

    /// Raw bytes of entry `index`.
    pub fn read_entry(&self, index: usize) -> Result<&[u8]> {
        let entry = self
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
