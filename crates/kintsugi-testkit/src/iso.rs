//! A tiny ISO 9660 writer, so that a test can hold a disc without holding one.
//!
//! Games reach this project as folders more often than as the discs they were
//! pressed on, and one path only a disc can exercise is the folder that holds
//! an image: the repaired file lands loose beside the image and shadows the
//! same name inside it. That path needs a real volume — 2048-byte sectors, a
//! Primary Volume Descriptor, and directory records a reader walks — and it
//! cannot use anybody's disc, because the repository holds no game data. So the
//! fixture is written: every byte below is spelled out in code.
//!
//! Deliberately left out, because `kintsugi_core::iso` never reads them and a
//! fixture should not pretend to be a mastering tool:
//!
//! * the **path table** (ECMA-119 6.9). The reader starts at the root directory
//!   record in the volume descriptor and recurses through the directories
//!   themselves; nothing in the project reads a path table, so writing one
//!   would be bytes no test could hold this writer to.
//! * **Joliet and Rock Ridge**: no supplementary volume, no SUSP entries. The
//!   reader serves the plain ISO names when no other tree is present, and plain
//!   ISO names are exactly what a disc-image test of the shadowing path wants.
//! * **multi-extent files, extended attribute records, interleaving and
//!   multi-session volumes**. The reader refuses the first rather than
//!   half-reading a file, reads an extended attribute length but nothing here
//!   records attributes, never looks at interleaving, and takes one image for
//!   one session. Each of those is written as the zero that says "none".
//! * the **recording date** is a constant rather than the time of the build, so
//!   that the same files always produce the same image and a failure can be
//!   reproduced.

/// One logical sector. ECMA-119 lets a volume choose, and every disc image in
/// the wild — and the block size the reader's own fixtures declare — is 2048.
const SECTOR: usize = 2048;

/// Sector 16: where ECMA-119 puts the first volume descriptor, and where every
/// operating system on earth looks for it.
const DESCRIPTOR_SECTOR: usize = 16;

/// The first sector left for directories and files: the descriptor set here is
/// a Primary Volume Descriptor and its terminator, in sectors 16 and 17.
const FIRST_DATA_SECTOR: u32 = 18;

/// The volume identifier. A real disc carries the game's own title in this
/// field, in the disc's code page; a fixture has no title to carry, and nothing
/// the reader decides depends on it.
const VOLUME_ID: &str = "KINTSUGI";

/// The directory flag in a record's flag byte (ECMA-119 9.1.6).
const FLAG_DIRECTORY: u8 = 0x02;

/// A synthesized ISO 9660 disc image holding `files`, one sector at a time.
///
/// Paths may nest (`exe/bsx.dat`); directories are built for them. Every byte
/// comes from code: the repository holds no game data.
///
/// # Panics
///
/// On a path this writer cannot record honestly: an empty, `.` or `..`
/// component, a name whose characters are outside ECMA-119's alphabet, a name
/// too long for one directory record, or two entries of one directory sharing a
/// name. Each of those would produce a disc no honest reader could serve, so
/// the mistake is refused while the fixture is being built rather than read
/// back as a surprise later.
pub fn make_iso(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut directories = vec![Directory {
        // The root is its own parent, which is what its `..` record says.
        parent: 0,
        entries: Vec::new(),
        extent: 0,
        size: 0,
    }];
    for (path, bytes) in files {
        add(&mut directories, path, bytes);
    }

    // Sector numbers are assigned in one pass, parents before children and
    // files after every directory, so that a record can be written with the
    // extent of what it points at already known.
    let order = preorder(&directories);
    let mut next = FIRST_DATA_SECTOR;
    for index in &order {
        directories[*index].size = directory_size(&directories, *index);
        directories[*index].extent = next;
        next += directories[*index].size / SECTOR as u32;
    }
    for index in &order {
        for entry in &mut directories[*index].entries {
            if let Entry::File { bytes, extent, .. } = entry {
                *extent = next;
                next += sectors(bytes.len());
            }
        }
    }

    let mut image = vec![0u8; next as usize * SECTOR];
    write_descriptors(&mut image, next, &directories[0]);
    for index in 0..directories.len() {
        let bytes = directory_bytes(&directories, index);
        let at = directories[index].extent as usize * SECTOR;
        image[at..at + bytes.len()].copy_from_slice(&bytes);
    }
    for directory in &directories {
        for entry in &directory.entries {
            if let Entry::File { bytes, extent, .. } = entry {
                let at = *extent as usize * SECTOR;
                image[at..at + bytes.len()].copy_from_slice(bytes);
            }
        }
    }
    image
}

/// One directory while the image is being laid out.
struct Directory<'a> {
    /// The parent's index in the arena; the root is its own parent.
    parent: usize,
    /// The records after `.` and `..`, in the order the caller listed them.
    entries: Vec<Entry<'a>>,
    /// The sector this directory's own data starts in, and how long that data
    /// is. Both are filled in once the arena is complete.
    extent: u32,
    size: u32,
}

/// One record of a directory, other than `.` and `..`.
enum Entry<'a> {
    /// A subdirectory, by index into the arena.
    Directory { name: String, child: usize },
    /// A file: the identifier the disc records for it, its bytes, and the
    /// sector those bytes start in (filled in when the image is laid out).
    File {
        name: String,
        bytes: &'a [u8],
        extent: u32,
    },
}

impl Entry<'_> {
    /// The identifier this entry is recorded under.
    fn name(&self) -> &str {
        match self {
            Self::Directory { name, .. } | Self::File { name, .. } => name,
        }
    }
}

/// Add one file at `path`, building the directories its path names.
fn add<'a>(directories: &mut Vec<Directory<'a>>, path: &str, bytes: &'a [u8]) {
    assert!(!path.is_empty(), "a disc file needs a path");
    let parts: Vec<&str> = path.split('/').collect();
    let mut current = 0usize;
    for (position, part) in parts.iter().enumerate() {
        let is_file = position + 1 == parts.len();
        assert!(
            !part.is_empty() && *part != "." && *part != "..",
            "'{path}' is not a path a disc holds: one component is '{part}'"
        );
        assert!(
            !part.contains(['\\', '\0']),
            "'{path}' holds a path separator or a NUL byte inside a name, which a disc \
             record cannot say and the reader would refuse"
        );
        let name = identifier(part, is_file);
        // A directory already built for an earlier path is reused, so two files
        // in one directory share it; anything else of the same name is two
        // entries the virtual filesystem would fold into one.
        let existing = directories[current]
            .entries
            .iter()
            .find_map(|entry| match entry {
                Entry::Directory { name: held, child } if *held == name => Some(*child),
                _ => None,
            });
        if is_file {
            assert!(
                !directories[current]
                    .entries
                    .iter()
                    .any(|entry| entry.name() == name),
                "'{path}' would be a second entry called '{name}' in one directory; the \
                 virtual filesystem keeps one file per name, so the image would list \
                 one of them"
            );
            directories[current].entries.push(Entry::File {
                name,
                bytes,
                extent: 0,
            });
        } else if let Some(child) = existing {
            current = child;
        } else {
            assert!(
                !directories[current]
                    .entries
                    .iter()
                    .any(|entry| entry.name() == name),
                "'{path}' needs a directory called '{name}', and the disc already holds a \
                 file of that name"
            );
            let child = directories.len();
            directories.push(Directory {
                parent: current,
                entries: Vec::new(),
                extent: 0,
                size: 0,
            });
            directories[current]
                .entries
                .push(Entry::Directory { name, child });
            current = child;
        }
    }
}

/// Every directory, parents before children.
fn preorder(directories: &[Directory<'_>]) -> Vec<usize> {
    let mut order = Vec::with_capacity(directories.len());
    let mut pending = vec![0usize];
    while let Some(index) = pending.pop() {
        order.push(index);
        for entry in directories[index].entries.iter().rev() {
            if let Entry::Directory { child, .. } = entry {
                pending.push(*child);
            }
        }
    }
    order
}

/// The identifier a disc records for one path component.
///
/// ECMA-119 spells file identifiers in upper case and ends them with a version
/// number, `;1`; a directory identifier carries no version. A component outside
/// the format's alphabet is refused rather than recorded, because a name this
/// writer cannot spell is a name the reader would hand back as something the
/// caller never asked for.
fn identifier(component: &str, is_file: bool) -> String {
    let upper = component.to_ascii_uppercase();
    assert!(
        upper.bytes().all(|byte| byte.is_ascii_uppercase()
            || byte.is_ascii_digit()
            || byte == b'_'
            || byte == b'.'),
        "'{component}' is outside the alphabet ECMA-119 spells identifiers in (A-Z, 0-9, \
         _ and the dot). This writer builds the plain ISO tree only — Joliet, with its \
         real names, is deliberately left out — so it refuses rather than record a name \
         it cannot spell"
    );
    let name = if is_file { format!("{upper};1") } else { upper };
    assert!(
        record_length(name.len()) <= 255,
        "'{component}' does not fit one directory record, whose length is one byte"
    );
    name
}

/// How many bytes a directory's own data takes, padded to whole sectors.
///
/// A directory's recorded length is a multiple of the logical block size, even
/// when its last block is mostly zeroes: that is how much a reader is told to
/// read.
fn directory_size(directories: &[Directory<'_>], index: usize) -> u32 {
    let (_, used) = record_layout(&name_lengths(directories, index));
    (used.div_ceil(SECTOR) * SECTOR) as u32
}

/// A directory's data: every record where the layout below placed it, and
/// zeroes everywhere else.
fn directory_bytes(directories: &[Directory<'_>], index: usize) -> Vec<u8> {
    let directory = &directories[index];
    let parent = &directories[directory.parent];
    let (offsets, used) = record_layout(&name_lengths(directories, index));
    let mut data = vec![0u8; used.div_ceil(SECTOR) * SECTOR];
    let mut put = |at: usize, bytes: Vec<u8>| {
        data[at..at + bytes.len()].copy_from_slice(&bytes);
    };
    // The two records every directory opens with: itself, and its parent. The
    // root records itself for both.
    put(
        offsets[0],
        record(b"\0", directory.extent, directory.size, FLAG_DIRECTORY),
    );
    put(
        offsets[1],
        record(b"\x01", parent.extent, parent.size, FLAG_DIRECTORY),
    );
    for (position, entry) in directory.entries.iter().enumerate() {
        let bytes = match entry {
            Entry::Directory { name, child } => {
                let child = &directories[*child];
                record(name.as_bytes(), child.extent, child.size, FLAG_DIRECTORY)
            }
            Entry::File {
                name,
                bytes,
                extent,
            } => record(name.as_bytes(), *extent, bytes.len() as u32, 0),
        };
        put(offsets[position + 2], bytes);
    }
    data
}

/// The name lengths of a directory's records, in order: `.`, `..`, then its
/// entries.
fn name_lengths(directories: &[Directory<'_>], index: usize) -> Vec<usize> {
    let mut lengths = vec![1, 1];
    lengths.extend(
        directories[index]
            .entries
            .iter()
            .map(|entry| entry.name().len()),
    );
    lengths
}

/// Where every record of a directory begins, and how many bytes they use.
///
/// A record never crosses a logical block (ECMA-119 9.1), so one that would is
/// moved to the start of the next block and the bytes it skipped stay zero.
/// The directory's size and its bytes both come from this one layout, so the
/// two cannot disagree about where a record is.
fn record_layout(name_lengths: &[usize]) -> (Vec<usize>, usize) {
    let mut offsets = Vec::with_capacity(name_lengths.len());
    let mut at = 0usize;
    for name_len in name_lengths {
        let length = record_length(*name_len);
        if at % SECTOR + length > SECTOR {
            at += SECTOR - at % SECTOR;
        }
        offsets.push(at);
        at += length;
    }
    (offsets, at)
}

/// A record's length: the 33 bytes every record starts with, the identifier,
/// and the padding byte an even-length identifier gets so that the System Use
/// area starts at an even offset (ECMA-119 9.1.12).
fn record_length(name_len: usize) -> usize {
    33 + name_len + usize::from(name_len % 2 == 0)
}

/// The volume descriptor set: a Primary Volume Descriptor in sector 16, and the
/// terminator that closes the set in the sector after it.
///
/// The fields filled in are the ones ECMA-119 8.4 defines for the volume as a
/// whole. The rest — the publisher, the application identifier, the
/// bibliographic fields — are left at the zeroes the format uses, and the path
/// table fields say plainly that there is none (see the module docs).
fn write_descriptors(image: &mut [u8], volume_sectors: u32, root: &Directory<'_>) {
    let at = DESCRIPTOR_SECTOR * SECTOR;
    let primary = &mut image[at..at + SECTOR];
    primary[0] = 1;
    primary[1..6].copy_from_slice(b"CD001");
    primary[6] = 1;
    padded(&mut primary[8..40], b"KINTSUGI");
    padded(&mut primary[40..72], VOLUME_ID.as_bytes());
    // The volume's size in logical blocks, recorded both ways round like every
    // number in this format.
    primary[80..88].copy_from_slice(&both_u32(volume_sectors));
    primary[120..124].copy_from_slice(&both_u16(1)); // volume set size
    primary[124..128].copy_from_slice(&both_u16(1)); // volume sequence number
    primary[128..132].copy_from_slice(&both_u16(SECTOR as u16));
    primary[132..140].copy_from_slice(&both_u32(0)); // path table size: none
    // Bytes 140 to 152 are the two path table locations and the optional copy's,
    // zero like the size above rather than pointing at sectors that hold
    // directory records instead.
    primary[156..190].copy_from_slice(&record(b"\0", root.extent, root.size, FLAG_DIRECTORY));

    let terminator = &mut image[at + SECTOR..at + 2 * SECTOR];
    terminator[0] = 255;
    terminator[1..6].copy_from_slice(b"CD001");
    terminator[6] = 1;
}

/// One directory record, exactly as it goes on the disc (ECMA-119 9.1).
fn record(name: &[u8], extent: u32, size: u32, flags: u8) -> Vec<u8> {
    let mut record = vec![0u8; 33];
    // Byte 1 is the Extended Attribute Record length, in blocks: this writer
    // records no attributes, so a file's data starts at its own extent.
    record[2..10].copy_from_slice(&both_u32(extent));
    record[10..18].copy_from_slice(&both_u32(size));
    // The recording date and time: years since 1900, month, day, hour, minute,
    // second, and the offset from GMT in quarter hours. A constant, so that two
    // runs of a test produce the same bytes.
    record[18..25].copy_from_slice(&[126, 1, 1, 0, 0, 0, 0]);
    record[25] = flags;
    // Bytes 26 and 27 are the file unit size and the interleave gap, both zero:
    // no file here is interleaved.
    record[28..32].copy_from_slice(&both_u16(1)); // volume sequence number
    record[32] = name.len() as u8;
    record.extend_from_slice(name);
    if name.len() % 2 == 0 {
        record.push(0);
    }
    record[0] = record.len() as u8;
    record
}

/// Write `text` into one of a volume descriptor's character fields, which are
/// padded with spaces to the end of the field (ECMA-119 8.4).
fn padded(field: &mut [u8], text: &[u8]) {
    field[..text.len()].copy_from_slice(text);
    field[text.len()..].fill(b' ');
}

/// How many sectors a file of `length` bytes occupies.
///
/// A file of no bytes occupies none: its extent is wherever the next file
/// starts, which is the honest recording of "there is nothing here".
fn sectors(length: usize) -> u32 {
    length.div_ceil(SECTOR) as u32
}

/// A number ECMA-119 records twice: the little-endian half first, then the
/// big-endian one. The reader reads the little-endian copy, and a writer that
/// filled in only one half would be producing an image the format calls
/// malformed.
fn both_u32(value: u32) -> [u8; 8] {
    let mut field = [0u8; 8];
    field[..4].copy_from_slice(&value.to_le_bytes());
    field[4..].copy_from_slice(&value.to_be_bytes());
    field
}

/// The 16-bit twin of [`both_u32`].
fn both_u16(value: u16) -> [u8; 4] {
    let mut field = [0u8; 4];
    field[..2].copy_from_slice(&value.to_le_bytes());
    field[2..].copy_from_slice(&value.to_be_bytes());
    field
}

#[cfg(test)]
mod tests {
    use super::*;
    use kintsugi_core::iso::{IsoSource, Naming};
    use kintsugi_core::vfs::{Vfs, VirtualPath};

    /// Write a synthesized image to a scratch file, named for the test that
    /// asked for it: tests run in parallel inside one process.
    fn written(label: &str, image: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "kintsugi-testkit-iso-{}-{label}.iso",
            std::process::id()
        ));
        std::fs::write(&path, image).expect("the test writes its own fixture");
        path
    }

    #[test]
    fn a_synthesized_disc_mounts_as_a_plain_iso_and_gives_the_bytes_back() {
        let story = b"BSScript\0\0\0\0a story, not a game's".to_vec();
        // Longer than one sector, and carrying every byte value: a writer that
        // lost a length, rounded an extent or went through text would give
        // something else back.
        let archive: Vec<u8> = (0..5000u32).map(|i| (i % 256) as u8).collect();
        let image = make_iso(&[
            ("exe/bsx.dat", &story),
            ("exe/Graphics.bsa", &archive),
            ("README.TXT", b"one loose file"),
        ]);
        let path = written("plain", &image);

        let source = IsoSource::open(&path).expect("the reader accepts what this writer writes");
        assert_eq!(source.naming(), Naming::Iso);
        assert_eq!(source.file_count(), 3);
        assert_eq!(source.block_size(), SECTOR as u32);

        // And the same bytes through the VFS, which is how a seam sees a disc.
        let vfs = Vfs::from_iso(&path).expect("the image mounts as a game");
        assert_eq!(vfs.read(&VirtualPath::new("exe/bsx.dat")).unwrap(), story);
        assert_eq!(
            vfs.read(&VirtualPath::new("exe/Graphics.bsa")).unwrap(),
            archive
        );
        // The `;1` version is not part of the name, and the plain ISO tree is
        // upper case: a lookup in the case the caller wrote finds it anyway.
        assert_eq!(
            vfs.read(&VirtualPath::new("readme.txt")).unwrap(),
            b"one loose file"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_directory_whose_records_do_not_fit_one_sector_still_lists_every_file() {
        // Enough entries that the records cross a logical block, which is where
        // a hand-written ISO writer is most often wrong: the block is padded
        // and the next record starts after it, or the files after the boundary
        // are lost.
        let payloads: Vec<Vec<u8>> = (0..70u8).map(|index| vec![index; 3]).collect();
        let names: Vec<String> = (0..70)
            .map(|index| format!("pack/file{index:03}.dat"))
            .collect();
        let files: Vec<(&str, &[u8])> = names
            .iter()
            .zip(&payloads)
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect();
        let image = make_iso(&files);
        let path = written("multi-block", &image);

        let source = IsoSource::open(&path).expect("the reader accepts what this writer writes");
        assert_eq!(source.file_count(), 70);
        let vfs = Vfs::from_iso(&path).unwrap();
        for (index, payload) in payloads.iter().enumerate() {
            let looked_up = format!("pack/file{index:03}.dat");
            assert_eq!(
                vfs.read(&VirtualPath::new(&looked_up)).unwrap(),
                *payload,
                "'{looked_up}' did not come back"
            );
        }
        std::fs::remove_file(&path).ok();
    }
}
