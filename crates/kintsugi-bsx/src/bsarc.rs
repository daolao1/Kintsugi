//! The `BSArc` archive: BlueGale's and Bishop's shared resource container.
//!
//! # The format
//!
//! Read out of the real thing and cross-checked against every archive of a
//! 2008 BlueGale release (six of them, 4,405 entries), then against GARbro's
//! independent `ArcFormats/Bishop/ArcBSA.cs`:
//!
//! ```text
//! 0x00  "BSArc"            5 bytes
//! 0x05  00 00 00           padding
//! 0x08  u16 version        1..=3
//! 0x0A  u16 count          number of index records
//! 0x0C  u32 index_offset   where the records start
//!       ...                entry data
//! index_offset:
//!       count * 0x28       records
//! 0x20  name               NUL padded, CP932
//! 0x20  u32 offset         from the start of the file
//! 0x24  u32 size
//! ```
//!
//! A version 1 file uses a 0x28-byte record too; GARbro distinguishes v1 from
//! v2 by trying the v2 layout first, but the release this seam was written
//! against declares version **2** and stores the fixed layout above, so that is
//! what this reads. Version is checked, not trusted: the layout is validated
//! against the file size either way.
//!
//! # Why the index is at the end
//!
//! Nothing in the format says the records must sit after the data, and the
//! archives checked happen to put them at the very end of the file. That is
//! also why [`BsarcArchive`] never assumes it: the offset from the header is
//! what is used, and a file whose records overlap the data is refused rather
//! than guessed at.

use std::sync::Arc;

use kintsugi_core::error::{Error, Result};
use kintsugi_core::vfs::{FileSource, VirtualPath};

use crate::decode_cp932;

/// The five bytes every archive starts with.
pub const MAGIC: &[u8] = b"BSArc";

/// Bytes in one index record.
pub const RECORD_SIZE: usize = 0x28;

/// Bytes of the fixed name field in a record.
pub const NAME_SIZE: usize = 0x20;

/// Refuse an index longer than this. A record is 0x28 bytes, so this caps the
/// index at 40 MB — far past any real archive, and short of the size at which
/// a corrupt count would make the reader allocate wildly.
pub const MAX_ENTRIES: usize = 1 << 20;

/// One file inside an archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Path inside the archive, as the archive spells it.
    pub path: VirtualPath,
    /// Byte offset from the start of the archive.
    pub offset: u64,
    /// Byte length.
    pub size: u64,
}

/// A parsed archive index, tied to the bytes it describes.
///
/// The index is parsed once and kept; the bytes are read on demand through
/// [`BsarcArchive::read_range`], so mounting a release does not pull hundreds
/// of megabytes into memory.
#[derive(Debug)]
pub struct BsarcArchive {
    entries: Vec<Entry>,
    /// The virtual path of the archive itself, so a caller can read from it.
    archive_path: VirtualPath,
    /// Sizes and offsets are validated against this.
    file_size: u64,
    /// Directory markers (`>name` / `<`) are structural, not files, so they are
    /// kept to reconstruct paths but never listed.
    label: String,
}

impl BsarcArchive {
    /// Parse an archive whose whole contents are in hand.
    pub fn open(bytes: &[u8]) -> Result<Self> {
        Self::parse(bytes, VirtualPath::new(""), bytes.len() as u64)
    }

    /// Parse the index of an archive that lives at `path` and is `file_size`
    /// bytes long, given the whole file.
    pub fn parse(bytes: &[u8], path: VirtualPath, file_size: u64) -> Result<Self> {
        let label = path.to_string();
        let label = || {
            if label.is_empty() {
                "the archive".to_string()
            } else {
                format!("'{label}'")
            }
        };
        if bytes.len() < 16 {
            return Err(Error::corrupt(
                label(),
                format!(
                    "an archive header is 16 bytes, this file has {}",
                    bytes.len()
                ),
            ));
        }
        if &bytes[..MAGIC.len()] != MAGIC {
            return Err(Error::unsupported(
                "BSArc",
                format!("{} does not start with 'BSArc'", label()),
            ));
        }
        let version = u16::from_le_bytes([bytes[8], bytes[9]]);
        if version == 0 || version > 3 {
            return Err(Error::corrupt(
                label(),
                format!("version {version} is not one this seam knows (1 to 3)"),
            ));
        }
        let count = u16::from_le_bytes([bytes[10], bytes[11]]) as usize;
        if count > MAX_ENTRIES {
            return Err(Error::corrupt(
                label(),
                format!("{count} index records is not a plausible count"),
            ));
        }
        let index_offset = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as u64;
        let index_len = (count as u64) * (RECORD_SIZE as u64);
        let index_end = index_offset
            .checked_add(index_len)
            .ok_or_else(|| Error::corrupt(label(), "the index length overflows".to_string()))?;
        if index_end > file_size {
            return Err(Error::corrupt(
                label(),
                format!(
                    "{count} records at {index_offset} would end at {index_end}, \
                     past the end of a {file_size} byte file"
                ),
            ));
        }
        let index = bytes
            .get(index_offset as usize..index_end as usize)
            .ok_or_else(|| {
                Error::corrupt(
                    label(),
                    format!("the index at {index_offset} is past the bytes this reader was given"),
                )
            })?;
        Self::from_parts(bytes, index, path, file_size)
    }

    /// Parse an index that was read separately from the file it describes.
    ///
    /// `header` is the archive's first sixteen bytes and `records` its index
    /// records, which is what a caller holding a 190 MB archive on a disc image
    /// actually wants to read: nothing else is touched until a file is asked
    /// for.
    pub fn from_parts(
        header: &[u8],
        records: &[u8],
        path: VirtualPath,
        file_size: u64,
    ) -> Result<Self> {
        let label = path.to_string();
        let label = || {
            if label.is_empty() {
                "the archive".to_string()
            } else {
                format!("'{label}'")
            }
        };
        if header.len() < 16 {
            return Err(Error::corrupt(
                label(),
                format!(
                    "an archive header is 16 bytes, only {} were given",
                    header.len()
                ),
            ));
        }
        if &header[..MAGIC.len()] != MAGIC {
            return Err(Error::unsupported(
                "BSArc",
                format!("{} does not start with 'BSArc'", label()),
            ));
        }
        let version = u16::from_le_bytes([header[8], header[9]]);
        if version == 0 || version > 3 {
            return Err(Error::corrupt(
                label(),
                format!("version {version} is not one this seam knows (1 to 3)"),
            ));
        }
        let count = u16::from_le_bytes([header[10], header[11]]) as usize;
        if count > MAX_ENTRIES {
            return Err(Error::corrupt(
                label(),
                format!("{count} index records is not a plausible count"),
            ));
        }
        let index_offset =
            u32::from_le_bytes([header[12], header[13], header[14], header[15]]) as u64;
        let index_end = index_offset + (count as u64) * (RECORD_SIZE as u64);
        if index_end > file_size {
            return Err(Error::corrupt(
                label(),
                format!(
                    "{count} records at {index_offset} would end at {index_end}, \
                     past the end of a {file_size} byte file"
                ),
            ));
        }
        if records.len() < count * RECORD_SIZE {
            return Err(Error::corrupt(
                label(),
                format!(
                    "the index needs {} bytes for {count} records, {} were given",
                    count * RECORD_SIZE,
                    records.len()
                ),
            ));
        }

        let mut entries = Vec::with_capacity(count);
        let mut dirs: Vec<String> = Vec::new();
        for i in 0..count {
            let record = &records[i * RECORD_SIZE..(i + 1) * RECORD_SIZE];
            let raw_name = &record[..NAME_SIZE];
            let name_bytes = match raw_name.iter().position(|b| *b == 0) {
                Some(end) => &raw_name[..end],
                None => raw_name,
            };
            let name = decode_cp932(name_bytes).0;
            if name.is_empty() {
                return Err(Error::corrupt(
                    label(),
                    format!("record {i} has an empty name"),
                ));
            }
            let offset =
                u32::from_le_bytes([record[0x20], record[0x21], record[0x22], record[0x23]]) as u64;
            let size =
                u32::from_le_bytes([record[0x24], record[0x25], record[0x26], record[0x27]]) as u64;

            // The archive spells its own hierarchy: `>dir` descends, `<` comes
            // back up, anything else is a file in the current directory. This
            // is prior art from GARbro's reader, and it is the archive's own
            // convention rather than a guess.
            match name.as_bytes()[0] {
                b'>' => {
                    dirs.push(name[1..].to_string());
                    continue;
                }
                b'<' => {
                    dirs.pop();
                    continue;
                }
                _ => {}
            }

            let mut joined = String::new();
            for dir in &dirs {
                joined.push_str(dir);
                joined.push('/');
            }
            joined.push_str(&name);
            let end = offset.checked_add(size).ok_or_else(|| {
                Error::corrupt(label(), format!("'{name}' has an overflowing size"))
            })?;
            if end > file_size {
                return Err(Error::corrupt(
                    label(),
                    format!("'{joined}' runs to {end} past the end of a {file_size} byte file"),
                ));
            }
            entries.push(Entry {
                path: VirtualPath::new(&joined),
                offset,
                size,
            });
        }

        Ok(Self {
            entries,
            archive_path: path,
            file_size,
            label: label(),
        })
    }

    /// Every file in the archive, in index order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// How many files the archive holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the archive holds no files at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The path this archive was opened under, if it was opened with one.
    pub fn path(&self) -> &VirtualPath {
        &self.archive_path
    }

    /// Total bytes of file data the index describes.
    pub fn data_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.size).sum()
    }

    /// The entry at `path`, if the archive has one.
    pub fn entry(&self, path: &VirtualPath) -> Option<&Entry> {
        self.entries.iter().find(|entry| &entry.path == path)
    }

    /// Read one entry out of `source`, which must serve this archive's bytes.
    pub fn read_from(&self, source: &dyn FileSource, path: &VirtualPath) -> Result<Vec<u8>> {
        let entry = self.entry(path).ok_or_else(|| {
            Error::NotFound(format!(
                "'{path}' is not in {} ({} has {})",
                self.label,
                self.label,
                if self.entries.is_empty() {
                    "no files".to_string()
                } else {
                    format!("{} files", self.entries.len())
                }
            ))
        })?;
        let bytes = source.read_range(&self.archive_path, entry.offset, entry.size as usize)?;
        if bytes.len() as u64 != entry.size {
            return Err(Error::corrupt(
                self.label.clone(),
                format!(
                    "'{path}' was indexed as {} bytes but {} came back",
                    entry.size,
                    bytes.len()
                ),
            ));
        }
        Ok(bytes)
    }

    /// One line for a report: what the index holds.
    pub fn summary(&self) -> String {
        format!(
            "{} file(s), {} bytes",
            self.entries.len(),
            self.data_bytes()
        )
    }

    /// The file size the index was validated against.
    pub fn file_size(&self) -> u64 {
        self.file_size
    }
}

/// A [`FileSource`] that serves one archive's entries.
///
/// Reads go straight to `source` with a byte range, so a 190 MB archive costs
/// the size of the one file asked for.
pub struct BsarcSource {
    archive: Arc<BsarcArchive>,
    source: Arc<dyn FileSource>,
    prefix: Option<String>,
}

impl BsarcSource {
    /// Serve `archive`'s entries out of `source`.
    ///
    /// `prefix`, when given, is put in front of every path — the usual case is
    /// naming the archive, so that `b01a.bsg` from `Graphics.bsa` appears as
    /// `graphics/b01a.bsg` and cannot collide with a same-named file in
    /// another archive.
    pub fn new(
        archive: Arc<BsarcArchive>,
        source: Arc<dyn FileSource>,
        prefix: Option<String>,
    ) -> Self {
        Self {
            archive,
            source,
            prefix,
        }
    }

    /// The archive being served.
    pub fn archive(&self) -> &Arc<BsarcArchive> {
        &self.archive
    }

    fn visible(&self, entry: &Entry) -> VirtualPath {
        match &self.prefix {
            Some(prefix) => VirtualPath::new(&format!("{prefix}/{}", entry.path)),
            None => entry.path.clone(),
        }
    }
}

impl FileSource for BsarcSource {
    fn read(&self, path: &VirtualPath) -> Result<Vec<u8>> {
        let inner = match &self.prefix {
            Some(prefix) => {
                let prefix = format!("{prefix}/");
                match path.as_str().strip_prefix(&prefix) {
                    Some(rest) => VirtualPath::new(rest),
                    None => {
                        return Err(Error::NotFound(format!(
                            "'{path}' is not in the archive mounted as '{prefix}'"
                        )));
                    }
                }
            }
            None => path.clone(),
        };
        self.archive.read_from(self.source.as_ref(), &inner)
    }

    fn read_range(&self, path: &VirtualPath, offset: u64, len: usize) -> Result<Vec<u8>> {
        let inner = match &self.prefix {
            Some(prefix) => {
                let prefix = format!("{prefix}/");
                match path.as_str().strip_prefix(&prefix) {
                    Some(rest) => VirtualPath::new(rest),
                    None => {
                        return Err(Error::NotFound(format!(
                            "'{path}' is not in the archive mounted as '{prefix}'"
                        )));
                    }
                }
            }
            None => path.clone(),
        };
        let entry = self
            .archive
            .entry(&inner)
            .ok_or_else(|| Error::NotFound(format!("'{path}' is not in this archive")))?;
        let end = offset
            .checked_add(len as u64)
            .ok_or_else(|| Error::corrupt(path.to_string(), "the range overflows".to_string()))?;
        if end > entry.size {
            return Err(Error::corrupt(
                path.to_string(),
                format!(
                    "asked for bytes {offset}..{end} of a {} byte entry",
                    entry.size
                ),
            ));
        }
        self.source
            .read_range(&self.archive_path_for_read(), entry.offset + offset, len)
    }

    fn list(&self) -> Vec<(VirtualPath, u64)> {
        self.archive
            .entries()
            .iter()
            .map(|entry| (self.visible(entry), entry.size))
            .collect()
    }
}

impl BsarcSource {
    fn archive_path_for_read(&self) -> VirtualPath {
        self.archive.path().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal archive: a header, one data byte range, and one record.
    ///
    /// Written through the fixture builder rather than by hand, so that the
    /// reader and the writer are checked against each other: a layout only one
    /// of them believes in fails here.
    fn one_file(name: &str, data: &[u8]) -> Vec<u8> {
        crate::fixtures::make_bsarc(&[(name, data)])
    }

    #[test]
    fn an_archive_index_is_read_back() {
        let bytes = one_file("hello.bsg", b"payload!");
        let archive = BsarcArchive::open(&bytes).expect("a well formed archive");
        assert_eq!(archive.len(), 1);
        let entry = &archive.entries()[0];
        assert_eq!(entry.path.as_str(), "hello.bsg");
        assert_eq!((entry.offset, entry.size), (16, 8));
        assert_eq!(archive.data_bytes(), 8);
    }

    #[test]
    fn something_that_is_not_an_archive_is_refused() {
        let error = BsarcArchive::open(b"not an archive at all").unwrap_err();
        assert!(
            error.to_string().contains("BSArc"),
            "the refusal should name the magic: {error}"
        );
    }

    #[test]
    fn an_index_that_runs_past_the_file_is_refused() {
        let mut bytes = one_file("a.bsg", b"x");
        bytes[8..10].copy_from_slice(&1u16.to_le_bytes());
        // Claim 500 records in a file that holds one.
        bytes[10..12].copy_from_slice(&500u16.to_le_bytes());
        let error = BsarcArchive::open(&bytes).unwrap_err();
        assert!(
            error.to_string().contains("past the end"),
            "the refusal should say what did not line up: {error}"
        );
    }

    #[test]
    fn an_unknown_version_is_refused() {
        let mut bytes = one_file("a.bsg", b"x");
        bytes[8..10].copy_from_slice(&9u16.to_le_bytes());
        let error = BsarcArchive::open(&bytes).unwrap_err();
        assert!(error.to_string().contains("version 9"), "{error}");
    }

    #[test]
    fn directory_markers_build_paths_and_are_not_files() {
        let bytes = crate::fixtures::make_bsarc(&[(">cg", b""), ("a.bsg", b"0123"), ("<", b"")]);
        let archive = BsarcArchive::open(&bytes).expect("a well formed archive");
        assert_eq!(archive.len(), 1, "markers are structure, not files");
        assert_eq!(archive.entries()[0].path.as_str(), "cg/a.bsg");
    }
}
