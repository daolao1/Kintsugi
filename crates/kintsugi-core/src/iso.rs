//! ISO 9660 (ECMA-119) disc images, read-only.
//!
//! Old games shipped on CD-ROM, and a data CD is an ISO 9660 filesystem. The
//! disc is usually the best copy of such a game anyone still has, so
//! [`IsoSource`] mounts an image as a [`FileSource`] straight off the file: no
//! unpacking step, no temporary copy, and the image stays the master. A seam
//! then reads through the VFS exactly as it would from a folder somebody
//! dumped.
//!
//! # Naming
//!
//! **Joliet wins over Rock Ridge.** A disc can carry two name trees side by
//! side: the ISO one (upper case, `8.3`, `;1` version suffixes) and a
//! supplementary one. Windows-era discs carry a Joliet tree holding the real
//! mixed-case names the game's own code asked for; discs cut on Linux carry
//! Rock Ridge `NM` entries instead. When both are present — a Linux-authored
//! disc for a Windows game has both — this reader serves the **Joliet** names,
//! because those are the ones the game looked up at run time and matching the
//! game's own requests is the whole job. Rock Ridge names are only decoded
//! when there is no Joliet tree, and [`IsoSource::naming`] says which tree
//! answered.
//!
//! # Refusals
//!
//! Anything this reader cannot serve honestly is an error rather than a guess:
//! a file recorded in more than one extent, a name that is not valid UCS-2 or
//! UTF-8, an extent that runs off the end of the image, a tree that points
//! back at itself. A half-read file is worse than a refusal, because the seam
//! above cannot tell the two apart. The one deliberate exception is the ISO
//! name tree itself: its names are bytes, not text, and are carried through
//! byte for byte (see [`iso_name`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

use crate::error::{Error, Result};
use crate::vfs::{FileSource, VirtualPath, checked_range};

/// Volume descriptors live in 2048-byte sectors whatever the medium: every
/// operating system on earth looks for the Primary Volume Descriptor at byte
/// 32768, so the descriptor area is read in CD-sized sectors even when the PVD
/// declares a smaller logical block size for the files themselves.
const DESCRIPTOR_SIZE: u64 = 2048;

/// Sector 16: where ECMA-119 puts the first volume descriptor.
const DESCRIPTOR_SECTOR: u64 = 16;

/// The descriptor set ends at a terminator descriptor. This bound only stops a
/// damaged (or booby-trapped) image from being walked to its end.
const DESCRIPTOR_LIMIT: u64 = 256;

/// ECMA-119 allows directories eight levels deep. This bound is far more
/// generous than that, because a writer that ignores the limit is still
/// readable; it exists so that a tree which points back at itself cannot
/// recurse until the stack runs out.
const MAX_DEPTH: usize = 32;

/// How many Rock Ridge continuation areas one record may be followed through,
/// for the same reason.
const MAX_CONTINUATIONS: usize = 4;

/// Directory record flag bits this reader acts on (ECMA-119 9.1.6). Note the
/// absence of bit 0 ("hidden"): a hidden file is a file, and games do ship
/// data files whose records are marked hidden.
const FLAG_DIRECTORY: u8 = 0x02;
const FLAG_ASSOCIATED: u8 = 0x04;
const FLAG_MULTI_EXTENT: u8 = 0x80;

/// Which set of names a disc image actually carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Naming {
    /// Plain ECMA-119 names: upper case, with the `;1` version suffix stripped.
    Iso,
    /// Joliet names from a Supplementary Volume Descriptor, decoded from UCS-2.
    Joliet,
    /// Plain ISO names, restored to the original mixed-case POSIX names by the
    /// Rock Ridge `NM` entries in each record's System Use area.
    RockRidge,
}

impl fmt::Display for Naming {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Iso => "ISO 9660",
            Self::Joliet => "Joliet",
            Self::RockRidge => "Rock Ridge",
        })
    }
}

/// Where one file's bytes live: the logical block its first byte sits in, and
/// how many bytes belong to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Extent {
    block: u32,
    size: u32,
}

/// A read-only [`FileSource`] over one ISO 9660 disc image.
///
/// The image is opened once, at construction: the volume descriptors are read,
/// the name tree is walked, and the resulting `(path, extent)` table lives in
/// memory from then on. Reads then seek straight to the recorded extent, so
/// mounting a 700 MB disc costs a few hundred kilobytes of memory and never a
/// copy of the disc.
pub struct IsoSource {
    /// The image, opened once and shared by every read. [`FileSource::read`]
    /// takes `&self`, so the handle needs interior mutability: a `Mutex`
    /// rather than the platform-split `FileExt::read_at` / `seek_read`, which
    /// would need a `cfg` and a trait import per platform for no gain here. The
    /// lock is held for one seek and one read, never across a directory walk.
    image: Mutex<File>,
    /// The image length as it was when the volume was parsed, kept so that
    /// extent checks do not need the lock.
    image_len: u64,
    block_size: u32,
    volume_id: String,
    naming: Naming,
    /// The path the caller gave, when there was one: only ever used to name the
    /// image in a message.
    origin: String,
    files: BTreeMap<VirtualPath, Extent>,
}

impl IsoSource {
    /// Open the disc image at `path`.
    ///
    /// The volume descriptors and the whole directory tree are read here, once.
    /// Afterwards the image is only touched by reads, which seek to the extent
    /// each file's record named.
    pub fn open(image: impl AsRef<Path>) -> Result<Self> {
        let path = image.as_ref();
        let file = File::open(path)?;
        Self::mount(file, path.display().to_string())
    }

    /// Mount an image whose handle the caller already holds.
    ///
    /// For hosts that reached the bytes some other way — an Android
    /// `content://` stream spooled to cache, a caller that just checked the
    /// file's size — and for [`IsoSource::open`] itself.
    pub fn from_file(file: File) -> Result<Self> {
        Self::mount(file, String::new())
    }

    fn mount(file: File, origin: String) -> Result<Self> {
        let image_len = file.metadata()?.len();
        let mut image = Image {
            file,
            len: image_len,
        };
        let layout = parse(&mut image, &origin)?;
        Ok(Self {
            image: Mutex::new(image.file),
            image_len,
            block_size: layout.block_size,
            volume_id: layout.volume_id,
            naming: layout.naming,
            origin,
            files: layout.files,
        })
    }

    /// The volume identifier the writer left in the Primary Volume Descriptor,
    /// trimmed of its padding (a Japanese game disc's is often just the title,
    /// in the disc's own code page).
    pub fn volume_id(&self) -> &str {
        &self.volume_id
    }

    /// The logical block size the PVD declares, in bytes.
    pub fn block_size(&self) -> u32 {
        self.block_size
    }

    /// Which name tree this image is served under.
    pub fn naming(&self) -> Naming {
        self.naming
    }

    /// How many files the image holds. Directories are not files here, for the
    /// same reason [`FileSource::list`] does not list them.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// How this image is named in a message the user will read.
    fn label(&self) -> String {
        match self.origin.is_empty() {
            true => "this ISO 9660 image".to_string(),
            false => format!("ISO 9660 image '{}'", self.origin),
        }
    }

    /// The extent of a file this image serves.
    fn extent(&self, path: &VirtualPath) -> Result<Extent> {
        match self.files.get(path) {
            Some(extent) => Ok(*extent),
            None => Err(Error::NotFound(format!(
                "'{path}' is not a file in {} ({} file(s) listed)",
                self.label(),
                self.files.len()
            ))),
        }
    }

    /// Read `length` bytes starting `skip` bytes into a file's extent.
    fn read_inside(
        &self,
        path: &VirtualPath,
        extent: Extent,
        skip: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        let what = format!("'{path}' in {}", self.label());
        let start =
            check_extent(&what, extent, self.block_size, self.image_len, &self.origin)? + skip;
        // A poisoned lock means some other reader panicked mid-read; the data
        // is still fine because every read seeks first, so take the guard back
        // instead of panicking here too.
        let mut image = self
            .image
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        image.seek(SeekFrom::Start(start))?;
        let mut bytes = vec![0u8; length];
        image.read_exact(&mut bytes)?;
        Ok(bytes)
    }
}

impl FileSource for IsoSource {
    fn read(&self, path: &VirtualPath) -> Result<Vec<u8>> {
        let extent = self.extent(path)?;
        self.read_inside(path, extent, 0, extent.size as usize)
    }

    /// Seeking straight into the image, so a seam reading one entry out of a
    /// 400 MB archive on the disc never loads the whole archive.
    fn read_range(&self, path: &VirtualPath, offset: u64, len: usize) -> Result<Vec<u8>> {
        let extent = self.extent(path)?;
        let range = checked_range(path, offset, len, u64::from(extent.size))?;
        self.read_inside(path, extent, range.start as u64, len)
    }

    /// Every file (not directory) the image holds, with its recorded size.
    ///
    /// Two disc names that differ only in case fold to the same
    /// [`VirtualPath`]; the first record the walk reaches wins, because
    /// `VirtualPath` compares case-folded and one name can only serve one file.
    fn list(&self) -> Vec<(VirtualPath, u64)> {
        self.files
            .iter()
            .map(|(path, extent)| (path.clone(), u64::from(extent.size)))
            .collect()
    }
}

impl fmt::Debug for IsoSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IsoSource")
            .field("origin", &self.origin)
            .field("volume_id", &self.volume_id)
            .field("naming", &self.naming)
            .field("block_size", &self.block_size)
            .field("image_len", &self.image_len)
            .field("files", &self.files.len())
            .finish()
    }
}

/// The image while its volume is being read: an owned handle with exclusive
/// access, so the parser can seek where it likes. The very same `File` moves
/// into the [`Mutex`] when parsing is done.
struct Image {
    file: File,
    len: u64,
}

impl Image {
    /// Read exactly `length` bytes at `offset`.
    fn read_at(&mut self, offset: u64, length: usize) -> Result<Vec<u8>> {
        self.file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0u8; length];
        self.file.read_exact(&mut bytes)?;
        Ok(bytes)
    }
}

/// Everything a successful parse produces.
struct Layout {
    block_size: u32,
    volume_id: String,
    naming: Naming,
    files: BTreeMap<VirtualPath, Extent>,
}

/// The two volume descriptors this reader cares about, if the image has them.
#[derive(Default)]
struct Descriptors {
    primary: Option<Vec<u8>>,
    joliet: Option<Vec<u8>>,
}

/// Read the volume descriptors and walk the tree they point at.
fn parse(image: &mut Image, origin: &str) -> Result<Layout> {
    let descriptors = read_descriptors(image, origin)?;
    let primary = descriptors.primary.ok_or_else(|| {
        Error::corrupt(
            context(origin),
            "the volume descriptor set holds no Primary Volume Descriptor, so \
             there is no ISO 9660 volume here to read",
        )
    })?;
    let block_size = logical_block_size(&primary, origin)?;
    let volume_id = volume_id(&primary);
    // Joliet first: see the module docs for why that order is the right one.
    let (tree, mut naming) = match descriptors.joliet {
        Some(joliet) => (joliet, Naming::Joliet),
        None => (primary, Naming::Iso),
    };
    let root = root_extent(&tree, origin)?;
    let mut walker = Walker {
        image,
        origin,
        block_size,
        joliet: naming == Naming::Joliet,
        rock_ridge: false,
        files: BTreeMap::new(),
        visited: BTreeSet::new(),
    };
    // The root's own extent counts as visited, so a subdirectory that points
    // back at it is caught as a loop rather than walked twice.
    walker.visited.insert(root.block);
    walker.walk(root, "the root directory", "", 0)?;
    if walker.rock_ridge {
        naming = Naming::RockRidge;
    }
    Ok(Layout {
        block_size,
        volume_id,
        naming,
        files: walker.files,
    })
}

/// Walk the volume descriptor set, from sector 16 to its terminator.
fn read_descriptors(image: &mut Image, origin: &str) -> Result<Descriptors> {
    let first = DESCRIPTOR_SECTOR * DESCRIPTOR_SIZE;
    if image.len < first + DESCRIPTOR_SIZE {
        return Err(Error::corrupt(
            context(origin),
            format!(
                "the image is {} byte(s) long, too short to hold the volume \
                 descriptor at sector 16; this is not an ISO 9660 image",
                image.len
            ),
        ));
    }
    let mut found = Descriptors::default();
    for index in 0..DESCRIPTOR_LIMIT {
        let offset = first + index * DESCRIPTOR_SIZE;
        if offset + DESCRIPTOR_SIZE > image.len {
            break;
        }
        let descriptor = image.read_at(offset, DESCRIPTOR_SIZE as usize)?;
        if &descriptor[1..6] != b"CD001" {
            if index == 0 {
                let identifier = String::from_utf8_lossy(&descriptor[1..6]);
                return Err(Error::corrupt(
                    context(origin),
                    format!(
                        "sector 16 holds {identifier:?} where the \"CD001\" standard \
                         identifier belongs; this is not an ISO 9660 image"
                    ),
                ));
            }
            // Not a volume descriptor at all: on a UDF bridge disc the markers
            // (BEA01, NSR02, TEA01) follow the set, so this is the end of it.
            break;
        }
        match descriptor[0] {
            1 if found.primary.is_none() => found.primary = Some(descriptor),
            2 if found.joliet.is_none() && is_joliet(&descriptor) => {
                found.joliet = Some(descriptor);
            }
            255 => break,
            // Boot records and non-Joliet supplementary volumes are not our
            // business; they describe how to boot, not what the files are.
            _ => {}
        }
    }
    Ok(found)
}

/// Whether a supplementary volume descriptor carries one of the Joliet escape
/// sequences: `%/@`, `%/C` or `%/E`, the UCS-2 levels 1, 2 and 3.
fn is_joliet(descriptor: &[u8]) -> bool {
    matches!(&descriptor[88..91], b"%/@" | b"%/C" | b"%/E")
}

/// The logical block size the PVD declares.
///
/// Refused rather than clamped: reading a 4096-byte-block image as if it were
/// 2048 would hand back every other block of every file, which looks like
/// success until the seam tries to parse it.
fn logical_block_size(primary: &[u8], origin: &str) -> Result<u32> {
    let size = u32::from(le_u16(primary, 128));
    if !size.is_power_of_two() || size < 512 {
        return Err(Error::corrupt(
            context(origin),
            format!(
                "the Primary Volume Descriptor declares a logical block size of {size}, which is not a power of two of at least 512"
            ),
        ));
    }
    if size > 4096 {
        return Err(Error::unsupported(
            format!("ISO 9660 with a {size}-byte logical block size"),
            "this reader handles logical block sizes from 512 to 4096 bytes",
        ));
    }
    Ok(size)
}

/// The volume identifier, trimmed of its space padding.
fn volume_id(primary: &[u8]) -> String {
    String::from_utf8_lossy(&primary[40..72]).trim().to_string()
}

/// The root directory's extent, from the directory record the volume
/// descriptor carries at byte 156.
fn root_extent(descriptor: &[u8], origin: &str) -> Result<Extent> {
    let record = directory_record(&descriptor[156..190], origin)?;
    if record.flags & FLAG_DIRECTORY == 0 {
        return Err(Error::corrupt(
            context(origin),
            "the volume descriptor's root directory record is not marked as a directory",
        ));
    }
    Ok(record.extent)
}

/// The walk itself: one directory at a time, recursing into subdirectories.
struct Walker<'a> {
    image: &'a mut Image,
    origin: &'a str,
    block_size: u32,
    /// Joliet names are UCS-2; ISO names are bytes. See [`iso_name`].
    joliet: bool,
    /// Set as soon as a Rock Ridge name is used, so the source can report it.
    rock_ridge: bool,
    files: BTreeMap<VirtualPath, Extent>,
    /// Extents already walked, so a directory tree cannot loop.
    visited: BTreeSet<u32>,
}

impl Walker<'_> {
    /// Read one directory extent and add everything under `prefix` to the
    /// table.
    fn walk(&mut self, directory: Extent, name: &str, prefix: &str, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(Error::corrupt(
                context(self.origin),
                format!("directory '{name}' nests more than {MAX_DEPTH} levels deep"),
            ));
        }
        let start = check_extent(
            name,
            directory,
            self.block_size,
            self.image.len,
            self.origin,
        )?;
        let data = self.image.read_at(start, directory.size as usize)?;
        let block = self.block_size as usize;
        let mut offset = 0usize;
        while offset < data.len() {
            let length = data[offset] as usize;
            if length == 0 {
                // A directory record never crosses a logical block, so the rest
                // of this block is padding; the next record starts at the next
                // block boundary.
                let next = (offset / block + 1) * block;
                if next <= offset {
                    break;
                }
                offset = next;
                continue;
            }
            if offset + length > data.len() {
                return Err(Error::corrupt(
                    context(self.origin),
                    format!(
                        "a directory record in '{name}' at byte {offset} claims \
                         {length} byte(s) but only {} remain in the directory",
                        data.len() - offset
                    ),
                ));
            }
            if offset % block + length > block {
                // ECMA-119 9.1: a record never crosses a logical block. One
                // that claims to would swallow the padding after it and read a
                // name out of the next block, so it is refused.
                return Err(Error::corrupt(
                    context(self.origin),
                    format!(
                        "a directory record in '{name}' at byte {offset} runs \
                         {length} byte(s) long, across the end of its {block} \
                         byte logical block"
                    ),
                ));
            }
            let record = directory_record(&data[offset..offset + length], self.origin)?;
            offset += length;
            // The single-byte `\0` (self) and `\1` (parent) records are not
            // files; they are how a directory points at itself.
            if record.name.len() == 1 && record.name[0] <= 1 {
                continue;
            }
            self.entry(&record, name, prefix, depth)?;
        }
        Ok(())
    }

    /// Add one directory record, recursing if it is a directory.
    fn entry(
        &mut self,
        record: &Record<'_>,
        directory: &str,
        prefix: &str,
        depth: usize,
    ) -> Result<()> {
        if record.flags & FLAG_ASSOCIATED != 0 {
            // An "associated" record is a second stream filed under the same
            // name (a Mac resource fork, a CD-XA companion). The VFS has one
            // name per file, and the associated record must not shadow the
            // real one, so it is skipped.
            return Ok(());
        }
        let (name, original) = if self.joliet {
            (joliet_name(record.name, directory, self.origin)?, false)
        } else {
            match self.rock_ridge_name(record, directory)? {
                Some(original) => (original, true),
                None => (iso_name(record.name), false),
            }
        };
        if original {
            self.rock_ridge = true;
        }
        // The version suffix is part of an ISO 9660 identifier, so it goes from
        // both the ISO and the Joliet tree. A Rock Ridge `NM` name is the
        // original POSIX name instead and is never touched; see
        // [`Walker::rock_ridge_name`].
        let name = match original {
            true => name,
            false => strip_version(&name).to_string(),
        };
        check_name(&name, directory, self.origin)?;
        let path = match prefix.is_empty() {
            true => name,
            false => format!("{prefix}/{name}"),
        };
        if record.flags & FLAG_MULTI_EXTENT != 0 {
            // Refused by name, after decoding, so the message is readable on a
            // Joliet disc as well as on a plain one.
            return Err(Error::unsupported(
                "ISO 9660 multi-extent file",
                format!(
                    "'{path}' in '{directory}' continues in a further directory \
                     record (flag bit 7); this reader serves one extent per file \
                     and will not hand back the first half as if it were whole"
                ),
            ));
        }
        if record.flags & FLAG_DIRECTORY != 0 {
            if !self.visited.insert(record.extent.block) {
                return Err(Error::corrupt(
                    context(self.origin),
                    format!(
                        "directory '{path}' reuses extent {}, which the walk has \
                         already visited; the directory tree contains a loop",
                        record.extent.block
                    ),
                ));
            }
            return self.walk(record.extent, &path, &path, depth + 1);
        }
        // First one wins: see [`IsoSource::list`].
        //
        // The extent is checked here, not only when the file is read, so that a
        // truncated image is refused at the door instead of listing a size it
        // could never serve.
        check_extent(
            &format!("'{path}'"),
            record.extent,
            self.block_size,
            self.image.len,
            self.origin,
        )?;
        self.files
            .entry(VirtualPath::new(&path))
            .or_insert(record.extent);
        Ok(())
    }

    /// The original POSIX name from the Rock Ridge `NM` entry in a record's
    /// System Use area, if it carries one.
    ///
    /// The name is taken exactly as the disc holds it, including a trailing
    /// `;1`: `NM` is the alternate *POSIX* name, and a POSIX file may really be
    /// called `chapter;1`, so the version-suffix rule that belongs to ISO
    /// identifiers must not touch it. The bytes must be UTF-8, because that is
    /// what every writer that emits `NM` puts there, and a wrong guess would
    /// hand a seam a path the game never asks for.
    fn rock_ridge_name(&mut self, record: &Record<'_>, directory: &str) -> Result<Option<String>> {
        let mut area = record.system_use.to_vec();
        let mut name: Option<Vec<u8>> = None;
        let mut complete = false;
        for _ in 0..MAX_CONTINUATIONS {
            let mut offset = 0usize;
            let mut continuation: Option<Vec<u8>> = None;
            while offset + 4 <= area.len() {
                let length = area[offset + 2] as usize;
                if length < 4 || offset + length > area.len() {
                    // Not a System Use entry: the chain is over, or was never
                    // one. Stop rather than read fields out of arbitrary bytes.
                    break;
                }
                let entry = &area[offset..offset + length];
                match &entry[0..2] {
                    // Flags 0x02 and 0x04 mark the `NM` entry that stands for
                    // "." and ".."; those carry no name field.
                    b"NM" if length >= 5 => {
                        let flags = entry[4];
                        if flags & 0x06 == 0 {
                            name.get_or_insert_with(Vec::new)
                                .extend_from_slice(&entry[5..]);
                            if flags & 0x01 == 0 {
                                complete = true;
                                break;
                            }
                        }
                    }
                    b"CE" => {
                        if length < 28 {
                            return Err(Error::corrupt(
                                context(self.origin),
                                format!(
                                    "a Rock Ridge continuation entry in '{directory}' is only {length} byte(s) long"
                                ),
                            ));
                        }
                        continuation = Some(self.read_continuation(entry, directory)?);
                        break;
                    }
                    // "ST" ends a System Use area; anything else (`RR`, `PX`,
                    // `TF`, `SP`, …) is not a name.
                    b"ST" => break,
                    _ => {}
                }
                offset += length;
            }
            match continuation {
                Some(next) => area = next,
                None => break,
            }
        }
        let Some(bytes) = name else {
            return Ok(None);
        };
        if !complete {
            return Err(Error::corrupt(
                context(self.origin),
                format!(
                    "the Rock Ridge name of a record in '{directory}' never ends (a continuation is missing)"
                ),
            ));
        }
        match String::from_utf8(bytes) {
            Ok(name) => Ok(Some(name)),
            Err(error) => Err(Error::corrupt(
                context(self.origin),
                format!(
                    "the Rock Ridge name of a record in '{directory}' is not \
                     UTF-8 (byte {} is the first that is not): {error}",
                    error.utf8_error().valid_up_to()
                ),
            )),
        }
    }

    /// Read the area a Rock Ridge `CE` (continuation) entry points at. Writers
    /// spill a long name out of the record's System Use area into one, so a
    /// name longer than the record still arrives whole.
    fn read_continuation(&mut self, entry: &[u8], directory: &str) -> Result<Vec<u8>> {
        let extent = Extent {
            block: le_u32(entry, 4),
            size: le_u32(entry, 12),
        };
        let skip = u64::from(le_u32(entry, 8));
        if extent.size == 0 {
            return Err(Error::corrupt(
                context(self.origin),
                format!(
                    "a Rock Ridge continuation area for a name in '{directory}' is zero bytes long"
                ),
            ));
        }
        let what = format!("a Rock Ridge continuation area for a name in '{directory}'");
        let start =
            check_extent(&what, extent, self.block_size, self.image.len, self.origin)? + skip;
        let end = start.saturating_add(u64::from(extent.size));
        if end > self.image.len {
            return Err(Error::corrupt(
                context(self.origin),
                format!(
                    "{what} at extent {} plus {skip} byte(s) with size {} ends at \
                     byte {end}, past the end of the {} byte image",
                    extent.block, extent.size, self.image.len
                ),
            ));
        }
        self.image.read_at(start, extent.size as usize)
    }
}

/// One directory record, as far as this reader cares about it.
struct Record<'a> {
    extent: Extent,
    flags: u8,
    name: &'a [u8],
    /// The System Use area, where SUSP entries (Rock Ridge names among them)
    /// live. Empty when the record has none.
    system_use: &'a [u8],
}

/// Decode one directory record (ECMA-119 9.1).
///
/// The extent is what the data length says, shifted by the Extended Attribute
/// Record length when the record declares one: the attributes are recorded in
/// the logical blocks immediately before the file's own data, so a record that
/// has them starts that many blocks later than its extent number suggests.
fn directory_record<'a>(bytes: &'a [u8], origin: &str) -> Result<Record<'a>> {
    let length = match bytes.first() {
        Some(length) => usize::from(*length),
        None => 0,
    };
    if length < 34 || length > bytes.len() {
        return Err(Error::corrupt(
            context(origin),
            format!(
                "a directory record is {length} byte(s) long, which is not a \
                 whole record of 34 to 255 bytes"
            ),
        ));
    }
    let bytes = &bytes[..length];
    let attributes = u32::from(bytes[1]);
    let block = le_u32(bytes, 2).checked_add(attributes).ok_or_else(|| {
        Error::corrupt(
            context(origin),
            "a directory record's extent plus its extended attribute record overflows".to_string(),
        )
    })?;
    let name_len = bytes[32] as usize;
    if 33 + name_len > length {
        return Err(Error::corrupt(
            context(origin),
            format!(
                "a directory record claims a {name_len} byte name but is only {length} byte(s) long"
            ),
        ));
    }
    let name = &bytes[33..33 + name_len];
    // The System Use area starts at an even offset from the start of the
    // record; one padding byte follows an even-length identifier.
    let system_use_start = (33 + name_len + 1) & !1;
    let system_use = match system_use_start < length {
        true => &bytes[system_use_start..],
        false => &[],
    };
    Ok(Record {
        extent: Extent {
            block,
            size: le_u32(bytes, 10),
        },
        flags: bytes[25],
        name,
        system_use,
    })
}

/// The plain ISO 9660 file identifier as a name.
///
/// This is the one name in the format that is bytes rather than text: ECMA-119
/// gives the writer an alphabet of upper-case letters, digits and underscores,
/// and real Japanese discs ignore it and put Shift-JIS bytes in the field
/// anyway. Each byte therefore becomes one `char` — the mapping round-trips
/// exactly, so no byte is ever invented or lost — and a disc that carries
/// Joliet or Rock Ridge names is served under those instead.
fn iso_name(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| char::from(*byte)).collect()
}

/// A Joliet file identifier: UCS-2, big-endian, one unit per character.
///
/// Supplementary-plane characters arrive as a UTF-16 surrogate pair, which
/// `decode_utf16` joins; a lone surrogate is a name this reader cannot decode,
/// and is refused rather than replaced with a look-alike.
fn joliet_name(bytes: &[u8], directory: &str, origin: &str) -> Result<String> {
    if bytes.len() % 2 != 0 {
        return Err(Error::corrupt(
            context(origin),
            format!(
                "the Joliet name of a record in '{directory}' has an odd length ({} byte(s)) and cannot be UCS-2",
                bytes.len()
            ),
        ));
    }
    let units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]));
    let mut name = String::with_capacity(units.len());
    for decoded in char::decode_utf16(units) {
        match decoded {
            Ok(character) => name.push(character),
            Err(_) => {
                return Err(Error::corrupt(
                    context(origin),
                    format!(
                        "the Joliet name of a record in '{directory}' holds an unpaired UTF-16 surrogate and cannot be decoded"
                    ),
                ));
            }
        }
    }
    Ok(name)
}

/// Strip the `;1` version suffix from an ISO 9660 identifier.
///
/// A name that merely contains a semicolon keeps it: only `;` followed by one
/// to five digits at the very end is a version, and only the version goes.
fn strip_version(name: &str) -> &str {
    match name.rsplit_once(';') {
        Some((stem, version))
            if !stem.is_empty()
                && !version.is_empty()
                && version.len() <= 5
                && version.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            stem
        }
        _ => name,
    }
}

/// Refuse a name the VFS could not address honestly.
///
/// A separator inside a name would silently invent a directory level, and `.` /
/// `..` would address something other than the file the record describes.
fn check_name(name: &str, directory: &str, origin: &str) -> Result<()> {
    let complaint = match name.is_empty() {
        true => "is empty".to_string(),
        false if name.contains('/') || name.contains('\\') => {
            format!("contains a path separator: {name:?}")
        }
        false if name == "." || name == ".." => format!("is {name:?}, which names a directory"),
        false if name.contains('\0') => format!("contains a NUL byte: {name:?}"),
        false => return Ok(()),
    };
    Err(Error::corrupt(
        context(origin),
        format!("a record in '{directory}' has a name that {complaint}"),
    ))
}

/// The byte range an extent covers, refused when it leaves the image.
///
/// Named rather than silent: "file X is 900 MB into a 700 MB image" is a
/// diagnosis, and zeroes or a short read are not.
fn check_extent(
    what: &str,
    extent: Extent,
    block_size: u32,
    image_len: u64,
    origin: &str,
) -> Result<u64> {
    let start = u64::from(extent.block) * u64::from(block_size);
    let end = start.saturating_add(u64::from(extent.size));
    if end > image_len {
        return Err(Error::corrupt(
            context(origin),
            format!(
                "{what}: extent {} (byte {start}) with size {} ends at byte \
                 {end}, past the end of the {image_len} byte image",
                extent.block, extent.size
            ),
        ));
    }
    Ok(start)
}

/// The error context for everything this module refuses: the format, and the
/// file the user named when there was one.
fn context(origin: &str) -> String {
    match origin.is_empty() {
        true => "ISO 9660 image".to_string(),
        false => format!("ISO 9660 image '{origin}'"),
    }
}

/// The little-endian half of a both-endian field.
///
/// ECMA-119 records every number twice, little-endian first; the little-endian
/// copy is the one every writer fills in and every reader reads. Callers
/// length-check the record before calling, so the field is always there.
fn le_u32(bytes: &[u8], offset: usize) -> u32 {
    debug_assert!(bytes.len() >= offset + 4, "field at {offset} is present");
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

/// The little-endian half of a both-endian 16-bit field.
fn le_u16(bytes: &[u8], offset: usize) -> u16 {
    debug_assert!(bytes.len() >= offset + 2, "field at {offset} is present");
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::Vfs;

    /// One logical block. Every fixture in these tests is built from these,
    /// byte by byte: the repository holds no game data, and a fixture written
    /// in code says exactly which field of which record is under test.
    const BLOCK: usize = 2048;

    /// A synthesized disc image, assembled one sector at a time.
    struct Disc {
        bytes: Vec<u8>,
    }

    impl Disc {
        fn new(sectors: usize) -> Self {
            Self {
                bytes: vec![0; sectors * BLOCK],
            }
        }

        fn put(&mut self, sector: u64, offset: usize, bytes: &[u8]) {
            let start = sector as usize * BLOCK + offset;
            self.bytes[start..start + bytes.len()].copy_from_slice(bytes);
        }

        fn bytes(&self) -> Vec<u8> {
            self.bytes.clone()
        }
    }

    /// Where a test's scratch image lives. The name carries the test's own
    /// label, because tests run in parallel inside one process.
    fn scratch_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("kintsugi-iso-{}-{label}.iso", std::process::id()))
    }

    /// Write a synthesized image to a scratch file and mount it. The file is
    /// removed on the way out; the mount keeps its own handle to it.
    fn mounted(label: &str, bytes: &[u8]) -> Result<IsoSource> {
        let path = scratch_path(label);
        std::fs::write(&path, bytes).expect("the test writes its own fixture");
        let source = IsoSource::open(&path);
        std::fs::remove_file(&path).ok();
        source
    }

    fn both_u32(value: u32) -> [u8; 8] {
        let mut field = [0u8; 8];
        field[..4].copy_from_slice(&value.to_le_bytes());
        field[4..].copy_from_slice(&value.to_be_bytes());
        field
    }

    fn both_u16(value: u16) -> [u8; 4] {
        let mut field = [0u8; 4];
        field[..2].copy_from_slice(&value.to_le_bytes());
        field[2..].copy_from_slice(&value.to_be_bytes());
        field
    }

    fn padded(field: &mut [u8], text: &[u8]) {
        field[..text.len()].copy_from_slice(text);
        for byte in &mut field[text.len()..] {
            *byte = b' ';
        }
    }

    /// A volume descriptor, PVD or SVD.
    fn volume_descriptor(
        kind: u8,
        escape: &[u8],
        volume_id: &[u8],
        root_block: u32,
        root_size: u32,
        sectors: u32,
    ) -> Vec<u8> {
        let mut descriptor = vec![0u8; BLOCK];
        descriptor[0] = kind;
        descriptor[1..6].copy_from_slice(b"CD001");
        descriptor[6] = 1;
        padded(&mut descriptor[8..40], b"KINTSUGI");
        padded(&mut descriptor[40..72], volume_id);
        descriptor[80..88].copy_from_slice(&both_u32(sectors));
        descriptor[88..88 + escape.len()].copy_from_slice(escape);
        descriptor[120..124].copy_from_slice(&both_u16(1));
        descriptor[124..128].copy_from_slice(&both_u16(1));
        descriptor[128..132].copy_from_slice(&both_u16(BLOCK as u16));
        descriptor[132..140].copy_from_slice(&both_u32(10));
        descriptor[140..144].copy_from_slice(&10u32.to_le_bytes());
        descriptor[148..152].copy_from_slice(&10u32.to_be_bytes());
        let root = record(b"\0", root_block, root_size, FLAG_DIRECTORY, b"");
        descriptor[156..190].copy_from_slice(&root);
        descriptor
    }

    fn terminator() -> Vec<u8> {
        let mut descriptor = vec![0u8; BLOCK];
        descriptor[0] = 255;
        descriptor[1..6].copy_from_slice(b"CD001");
        descriptor[6] = 1;
        descriptor
    }

    /// A directory record, exactly as it goes on the disc.
    fn record(name: &[u8], block: u32, size: u32, flags: u8, system_use: &[u8]) -> Vec<u8> {
        let mut record = vec![0u8; 33];
        record[1] = 0; // extended attribute record length, in blocks
        record[2..10].copy_from_slice(&both_u32(block));
        record[10..18].copy_from_slice(&both_u32(size));
        record[18..25].copy_from_slice(&[108, 9, 30, 12, 0, 0, 0]); // recording date and time
        record[25] = flags;
        record[26] = 0; // file unit size
        record[27] = 0; // interleave gap size
        record[28..32].copy_from_slice(&both_u16(1));
        record[32] = name.len() as u8;
        record.extend_from_slice(name);
        if name.len() % 2 == 0 {
            record.push(0); // padding, so the System Use area starts even
        }
        record.extend_from_slice(system_use);
        record[0] = record.len() as u8;
        record
    }

    /// A directory holding the two records every directory starts with.
    fn directory(block: u32, parent: u32, entries: &[Vec<u8>]) -> Vec<u8> {
        let mut data = record(b"\0", block, BLOCK as u32, FLAG_DIRECTORY, b"");
        data.extend_from_slice(&record(b"\x01", parent, BLOCK as u32, FLAG_DIRECTORY, b""));
        for entry in entries {
            data.extend_from_slice(entry);
        }
        data
    }

    /// UCS-2, big-endian, as Joliet writes it.
    fn ucs2(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_be_bytes).collect()
    }

    /// A Rock Ridge `NM` name part, with the "continues" flag when asked.
    fn nm(part: &[u8], continues: bool) -> Vec<u8> {
        let mut entry = vec![b'N', b'M', 0, 1, u8::from(continues)];
        entry.extend_from_slice(part);
        entry[2] = entry.len() as u8;
        entry
    }

    /// The SUSP marker a Linux writer puts in the root's self record. This
    /// reader does not require it (see the module docs), but a fixture that
    /// omitted it would not look like the discs the reader is for.
    fn sp() -> Vec<u8> {
        vec![b'S', b'P', 7, 1, 0xBE, 0, 0]
    }

    /// The disc most tests use: a PVD, a root holding two files and one
    /// subdirectory, and one file inside that subdirectory.
    fn plain_disc() -> Vec<u8> {
        let mut disc = Disc::new(24);
        disc.put(
            16,
            0,
            &volume_descriptor(1, b"", b"KINTSUGI", 18, BLOCK as u32, 24),
        );
        disc.put(17, 0, &terminator());
        disc.put(
            18,
            0,
            &directory(
                18,
                18,
                &[
                    record(b"MAIN.TXT;1", 20, 11, 0, b""),
                    record(b"NOEXT;1", 21, 3, 0, b""),
                    record(b"SUB", 19, BLOCK as u32, FLAG_DIRECTORY, b""),
                ],
            ),
        );
        disc.put(
            19,
            0,
            &directory(19, 18, &[record(b"NESTED.BIN;1", 22, 5, 0, b"")]),
        );
        disc.put(20, 0, b"hello world");
        disc.put(21, 0, b"abc");
        disc.put(22, 0, &[0, 1, 2, 3, 4]);
        disc.bytes()
    }

    #[test]
    fn a_plain_disc_lists_and_reads_back_the_bytes_it_was_built_with() {
        let source =
            mounted("plain", &plain_disc()).expect("the synthesized image is a valid disc");
        assert_eq!(source.naming(), Naming::Iso);
        assert_eq!(source.volume_id(), "KINTSUGI");
        assert_eq!(source.block_size(), 2048);
        assert_eq!(source.file_count(), 3);

        // Directories are not files, and `;1` is a version, not part of a name.
        let listed: Vec<(String, u64)> = source
            .list()
            .into_iter()
            .map(|(path, size)| (path.to_string(), size))
            .collect();
        assert_eq!(
            listed,
            vec![
                ("main.txt".to_string(), 11),
                ("noext".to_string(), 3),
                ("sub/nested.bin".to_string(), 5),
            ]
        );

        assert_eq!(
            source.read(&VirtualPath::new("MAIN.TXT")).unwrap(),
            b"hello world"
        );
        assert_eq!(
            source.read(&VirtualPath::new("sub/NESTED.BIN")).unwrap(),
            vec![0, 1, 2, 3, 4]
        );
        assert_eq!(source.read(&VirtualPath::new("noext")).unwrap(), b"abc");
    }

    #[test]
    fn a_directory_and_a_file_that_is_not_there_are_not_found() {
        let source = mounted("not-found", &plain_disc()).unwrap();
        assert!(matches!(
            source.read(&VirtualPath::new("sub")),
            Err(Error::NotFound(message)) if message.contains("is not a file")
        ));
        assert!(matches!(
            source.read(&VirtualPath::new("missing.txt")),
            Err(Error::NotFound(_))
        ));
        // The version suffix is stripped on the way in, so the un-stripped
        // spelling is not a second name for the same file.
        assert!(matches!(
            source.read(&VirtualPath::new("main.txt;1")),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn a_range_read_starts_where_it_is_told_and_stops_where_it_is_told() {
        let source = mounted("range", &plain_disc()).unwrap();
        let path = VirtualPath::new("main.txt");
        assert_eq!(source.read_range(&path, 6, 5).unwrap(), b"world");
        assert_eq!(source.read_range(&path, 0, 11).unwrap(), b"hello world");
        assert_eq!(source.read_range(&path, 0, 0).unwrap(), b"");
        assert_eq!(source.read_range(&path, 11, 0).unwrap(), b"");
        let past_end = source.read_range(&path, 6, 100).unwrap_err();
        assert!(matches!(past_end, Error::Corrupt { .. }));
        assert!(
            format!("{past_end}").contains("asked for bytes 6..106 of a 11 byte file"),
            "{past_end}"
        );
        assert!(
            source
                .read_range(&VirtualPath::new("missing.txt"), 0, 1)
                .is_err()
        );
    }

    #[test]
    fn records_continue_in_the_next_logical_block_of_a_directory() {
        // A directory larger than one block ends its used part with a zero
        // length byte and continues at the next block boundary; a reader that
        // stopped at the zero would silently lose every file after it.
        let mut disc = Disc::new(24);
        disc.put(16, 0, &volume_descriptor(1, b"", b"KINTSUGI", 18, 4096, 24));
        disc.put(17, 0, &terminator());
        let mut first = Vec::new();
        first.extend_from_slice(&record(b"\x00", 18, BLOCK as u32, FLAG_DIRECTORY, b""));
        first.extend_from_slice(&record(b"\x01", 18, BLOCK as u32, FLAG_DIRECTORY, b""));
        first.extend_from_slice(&record(b"FIRST.TXT;1", 21, 3, 0, b""));
        // Everything after the last record in the block is zero padding.
        first.resize(BLOCK, 0);
        let mut second = Vec::new();
        second.extend_from_slice(&record(b"SECOND.TXT;1", 22, 4, 0, b""));
        disc.put(18, 0, &first);
        disc.put(18, BLOCK, &second);
        disc.put(21, 0, b"one");
        disc.put(22, 0, b"two!");

        let source = mounted("multi-block", &disc.bytes()).unwrap();
        let listed: Vec<(String, u64)> = source
            .list()
            .into_iter()
            .map(|(path, size)| (path.to_string(), size))
            .collect();
        assert_eq!(
            listed,
            vec![("first.txt".to_string(), 3), ("second.txt".to_string(), 4),]
        );
        assert_eq!(
            source.read(&VirtualPath::new("second.txt")).unwrap(),
            b"two!"
        );
    }

    #[test]
    fn an_extended_attribute_record_shifts_the_file_data_by_its_length() {
        let mut disc = Disc::new(24);
        disc.put(
            16,
            0,
            &volume_descriptor(1, b"", b"KINTSUGI", 18, BLOCK as u32, 24),
        );
        disc.put(17, 0, &terminator());
        // The attributes live in the block right before the data, so a record
        // with one block of them at extent 20 reads its bytes at extent 21.
        let mut shifted = record(b"SHIFT.TXT;1", 20, 5, 0, b"");
        shifted[1] = 1;
        disc.put(18, 0, &directory(18, 18, &[shifted]));
        disc.put(21, 0, b"after");
        let source = mounted("xar", &disc.bytes()).unwrap();
        assert_eq!(
            source.read(&VirtualPath::new("shift.txt")).unwrap(),
            b"after"
        );
        assert_eq!(source.list(), vec![(VirtualPath::new("shift.txt"), 5u64)]);
    }

    #[test]
    fn a_joliet_volume_supplies_the_names_the_game_asked_for() {
        let mut disc = Disc::new(24);
        disc.put(
            16,
            0,
            &volume_descriptor(1, b"", b"KINTSUGI", 18, BLOCK as u32, 24),
        );
        disc.put(
            17,
            0,
            &volume_descriptor(2, b"%/E", &ucs2("KINTSUGI"), 19, BLOCK as u32, 24),
        );
        disc.put(18, 0, &terminator());
        // The ISO tree, with a Rock Ridge name that must lose to Joliet.
        disc.put(
            18,
            0,
            &directory(
                18,
                18,
                &[record(
                    b"PLAIN.TXT;1",
                    20,
                    6,
                    0,
                    &nm(b"Ugly Old Name.txt", false),
                )],
            ),
        );
        disc.put(
            19,
            0,
            &directory(
                19,
                19,
                &[
                    record(&ucs2("Plain.txt"), 21, 7, 0, b""),
                    record(&ucs2("鬼父.txt"), 22, 5, 0, b""),
                    // A supplementary-plane character arrives as a surrogate
                    // pair, which the decoder has to join back together.
                    record(&ucs2("🎮.dat"), 23, 4, 0, b""),
                ],
            ),
        );
        disc.put(20, 0, b"iso!!!");
        disc.put(21, 0, b"joliet!");
        disc.put(22, 0, b"oni!!");
        disc.put(23, 0, b"game");

        let source = mounted("joliet", &disc.bytes()).unwrap();
        assert_eq!(source.naming(), Naming::Joliet);
        let listed: Vec<(String, u64)> = source
            .list()
            .into_iter()
            .map(|(path, size)| (path.to_string(), size))
            .collect();
        assert_eq!(
            listed,
            vec![
                ("plain.txt".to_string(), 7),
                // Paths sort by their UTF-8 bytes, not by Unicode code point.
                ("鬼父.txt".to_string(), 5),
                ("🎮.dat".to_string(), 4),
            ]
        );
        // The bytes come from Joliet's extent, not from the ISO record that
        // shares the name — proof that the winning tree is the one read.
        assert_eq!(
            source.read(&VirtualPath::new("Plain.txt")).unwrap(),
            b"joliet!"
        );
        assert_eq!(
            source.read(&VirtualPath::new("鬼父.txt")).unwrap(),
            b"oni!!"
        );
        assert!(matches!(
            source.read(&VirtualPath::new("ugly old name.txt")),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn a_joliet_name_that_is_not_ucs2_is_refused() {
        let mut disc = Disc::new(24);
        disc.put(
            16,
            0,
            &volume_descriptor(1, b"", b"KINTSUGI", 18, BLOCK as u32, 24),
        );
        disc.put(
            17,
            0,
            &volume_descriptor(2, b"%/E", &ucs2("KINTSUGI"), 19, BLOCK as u32, 24),
        );
        disc.put(18, 0, &terminator());
        disc.put(18, 0, &directory(18, 18, &[]));
        // A high surrogate with no low surrogate after it is not a name.
        disc.put(
            19,
            0,
            &directory(19, 19, &[record(&[0xD8, 0x00], 20, 4, 0, b"")]),
        );
        disc.put(20, 0, b"data");

        let error = mounted("joliet-bad", &disc.bytes()).unwrap_err();
        assert!(matches!(error, Error::Corrupt { .. }), "{error}");
        assert!(
            format!("{error}").contains("unpaired UTF-16 surrogate"),
            "{error}"
        );
    }

    #[test]
    fn rock_ridge_nm_entries_restore_the_original_mixed_case_names() {
        let mut disc = Disc::new(24);
        disc.put(
            16,
            0,
            &volume_descriptor(1, b"", b"KINTSUGI", 18, BLOCK as u32, 24),
        );
        disc.put(17, 0, &terminator());
        let mut root = record(b"\0", 18, BLOCK as u32, FLAG_DIRECTORY, &sp());
        root.extend_from_slice(&record(b"\x01", 18, BLOCK as u32, FLAG_DIRECTORY, b""));
        root.extend_from_slice(&record(
            b"README.TXT;1",
            20,
            7,
            0,
            &nm(b"ReadMe.txt", false),
        ));
        // A name too long for one entry is spread over two "continues" parts.
        let mut split = nm(b"A Very Long ", true);
        split.extend_from_slice(&nm(b"Original Name.txt", false));
        root.extend_from_slice(&record(b"LONGNA~1.TXT;1", 21, 4, 0, &split));
        // `;1` here is part of a POSIX name, not a version suffix.
        root.extend_from_slice(&record(b"CHAPTE~1.;1", 22, 3, 0, &nm(b"chapter;1", false)));
        disc.put(18, 0, &root);
        disc.put(20, 0, b"readme\n");
        disc.put(21, 0, b"long");
        disc.put(22, 0, b"ch1");

        let source = mounted("rock-ridge", &disc.bytes()).unwrap();
        assert_eq!(source.naming(), Naming::RockRidge);
        let listed: Vec<(String, u64)> = source
            .list()
            .into_iter()
            .map(|(path, size)| (path.to_string(), size))
            .collect();
        assert_eq!(
            listed,
            vec![
                ("a very long original name.txt".to_string(), 4),
                ("chapter;1".to_string(), 3),
                ("readme.txt".to_string(), 7),
            ]
        );
        assert_eq!(
            source.read(&VirtualPath::new("ReadMe.txt")).unwrap(),
            b"readme\n"
        );
        assert!(matches!(
            source.read(&VirtualPath::new("readme.txt;1")),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn a_multi_extent_file_is_refused_rather_than_half_read() {
        let mut disc = Disc::new(24);
        disc.put(
            16,
            0,
            &volume_descriptor(1, b"", b"KINTSUGI", 18, BLOCK as u32, 24),
        );
        disc.put(17, 0, &terminator());
        disc.put(
            18,
            0,
            &directory(
                18,
                18,
                &[record(b"BIG.BIN;1", 20, 11, FLAG_MULTI_EXTENT, b"")],
            ),
        );
        disc.put(20, 0, b"hello world");

        let error = mounted("multi-extent", &disc.bytes()).unwrap_err();
        assert!(matches!(error, Error::Unsupported { .. }), "{error}");
        assert!(format!("{error}").contains("multi-extent"), "{error}");
    }

    #[test]
    fn a_file_whose_extent_runs_past_the_end_is_refused_with_its_numbers() {
        let mut disc = Disc::new(24);
        disc.put(
            16,
            0,
            &volume_descriptor(1, b"", b"KINTSUGI", 18, BLOCK as u32, 24),
        );
        disc.put(17, 0, &terminator());
        disc.put(
            18,
            0,
            &directory(18, 18, &[record(b"GHOST.BIN;1", 23, 4096, 0, b"")]),
        );
        // 24 sectors is 49152 bytes; the record asks for bytes up to 51200.
        let error = mounted("past-end", &disc.bytes()).unwrap_err();
        let message = format!("{error}");
        assert!(matches!(error, Error::Corrupt { .. }), "{message}");
        for expected in ["GHOST.BIN", "extent 23", "4096", "49152"] {
            assert!(
                message.contains(expected),
                "{expected} is missing from: {message}"
            );
        }
    }

    #[test]
    fn a_directory_that_points_back_at_an_ancestor_is_refused() {
        let mut disc = Disc::new(24);
        disc.put(
            16,
            0,
            &volume_descriptor(1, b"", b"KINTSUGI", 18, BLOCK as u32, 24),
        );
        disc.put(17, 0, &terminator());
        // The "subdirectory" is the root's own extent.
        disc.put(
            18,
            0,
            &directory(
                18,
                18,
                &[record(b"LOOP", 18, BLOCK as u32, FLAG_DIRECTORY, b"")],
            ),
        );
        let error = mounted("loop", &disc.bytes()).unwrap_err();
        assert!(matches!(error, Error::Corrupt { .. }), "{error}");
        assert!(format!("{error}").contains("loop"), "{error}");
    }

    #[test]
    fn an_image_whose_sector_16_is_not_a_volume_descriptor_is_refused() {
        let mut not_a_disc = plain_disc();
        not_a_disc[16 * BLOCK..17 * BLOCK].fill(0);
        let error = mounted("not-iso", &not_a_disc).unwrap_err();
        assert!(matches!(error, Error::Corrupt { .. }), "{error}");
        assert!(
            format!("{error}").contains("this is not an ISO 9660 image"),
            "{error}"
        );

        // Something that really is another format, caught in the same place.
        let mut archive = vec![0u8; 24 * BLOCK];
        archive[16 * BLOCK..16 * BLOCK + 4].copy_from_slice(b"PK\x03\x04");
        assert!(mounted("zip", &archive).is_err());
    }

    #[test]
    fn an_image_too_short_to_hold_a_volume_descriptor_is_refused() {
        let error = mounted("short", &[0u8; 1000]).unwrap_err();
        assert!(matches!(error, Error::Corrupt { .. }), "{error}");
        assert!(
            format!("{error}").contains("this is not an ISO 9660 image"),
            "{error}"
        );
    }

    #[test]
    fn a_logical_block_size_that_is_not_a_power_of_two_is_refused() {
        let mut disc = plain_disc();
        let start = 16 * BLOCK;
        disc[start + 128..start + 130].copy_from_slice(&1000u16.to_le_bytes());
        disc[start + 130..start + 132].copy_from_slice(&1000u16.to_be_bytes());
        let error = mounted("block-size", &disc).unwrap_err();
        assert!(matches!(error, Error::Corrupt { .. }), "{error}");
        assert!(format!("{error}").contains("1000"), "{error}");

        // 8192 is a legal block size in ECMA-119, just not one this reads.
        let mut disc = plain_disc();
        disc[start + 128..start + 130].copy_from_slice(&8192u16.to_le_bytes());
        disc[start + 130..start + 132].copy_from_slice(&8192u16.to_be_bytes());
        let error = mounted("block-size-big", &disc).unwrap_err();
        assert!(matches!(error, Error::Unsupported { .. }), "{error}");
        assert!(format!("{error}").contains("8192"), "{error}");
    }

    #[test]
    fn an_image_that_is_not_there_is_not_found() {
        let missing = scratch_path("missing");
        assert!(matches!(IsoSource::open(&missing), Err(Error::NotFound(_))));
    }

    #[test]
    fn a_mounted_image_can_be_pushed_in_front_of_loose_files() {
        let path = scratch_path("vfs");
        std::fs::write(&path, plain_disc()).expect("the test writes its own fixture");
        let vfs = Vfs::from_iso(&path).unwrap();
        assert_eq!(
            vfs.read(&VirtualPath::new("MAIN.TXT")).unwrap(),
            b"hello world"
        );
        assert_eq!(
            vfs.read_range(&VirtualPath::new("main.txt"), 6, 5).unwrap(),
            b"world"
        );
        assert_eq!(vfs.list().len(), 3);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn the_source_is_shareable_across_threads() {
        fn assert_send_and_sync<T: Send + Sync>() {}
        assert_send_and_sync::<IsoSource>();
    }
}
