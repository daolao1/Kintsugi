//! Virtual filesystem: a stack of sources that mounts archives over loose
//! files without ever touching the original discs.
//!
//! Preservation rule: the gold never melts the pottery. Seams expose
//! archives as [`FileSource`]s pushed *in front of* the directory a fan
//! dumped the game into; the untouched files stay byte-for-byte intact on
//! disk.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::error::{Error, Result};

/// A normalized, case-folded, forward-slashed path inside the [`Vfs`].
///
/// Real engines compare file names in all sorts of broken ways (case, code
/// page, trailing spaces). The VFS canonicalizes once, here, so seams can be
/// simple.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct VirtualPath(String);

impl VirtualPath {
    /// Normalize `raw` into a virtual path: trimmed, lower-cased,
    /// forward-slashed, with `./`, `/` and duplicate-slash prefixes removed.
    pub fn new(raw: &str) -> Self {
        let mut s = raw.trim().replace('\\', "/");
        while let Some(rest) = s.strip_prefix('/') {
            s = rest.to_string();
        }
        while let Some(rest) = s.strip_prefix("./") {
            s = rest.to_string();
        }
        while s.contains("//") {
            s = s.replace("//", "/");
        }
        s.make_ascii_lowercase();
        Self(s)
    }

    /// The normalized path string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the empty path (matches nothing).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The final path segment, without directory parts.
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// Lower-cased file extension, if any. `.hidden` files have none.
    pub fn extension(&self) -> Option<&str> {
        self.file_name()
            .rsplit_once('.')
            .filter(|(stem, _)| !stem.is_empty())
            .map(|(_, ext)| ext)
    }

    /// Path components; empty components are skipped.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|c| !c.is_empty())
    }
}

impl fmt::Display for VirtualPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'a> From<&'a str> for VirtualPath {
    fn from(s: &'a str) -> Self {
        Self::new(s)
    }
}

/// One mounted origin of bytes: a directory, an archive, a memory blob.
///
/// Sources are searched in stack order; the first source that has the path
/// serves it, so a seam can push a mounted archive in front of the loose
/// files and shadow them.
pub trait FileSource: Send + Sync {
    /// Read a whole virtual file into memory.
    fn read(&self, path: &VirtualPath) -> Result<Vec<u8>>;

    /// Read `len` bytes starting at `offset` inside a virtual file.
    ///
    /// The default implementation reads the whole file and slices it, which is
    /// correct for any source. Sources that can seek — a directory, a disc
    /// image — should override it, because archives use this to read one
    /// entry's extent out of a container that may be hundreds of megabytes, and
    /// loading the whole container to hand back a kilobyte is the difference
    /// between mounting a game and running out of memory.
    fn read_range(&self, path: &VirtualPath, offset: u64, len: usize) -> Result<Vec<u8>> {
        let bytes = self.read(path)?;
        let range = checked_range(path, offset, len, bytes.len() as u64)?;
        Ok(bytes[range].to_vec())
    }

    /// Every file this source provides, with byte sizes.
    fn list(&self) -> Vec<(VirtualPath, u64)>;
}

/// Check a byte range against the length of the file it points into.
///
/// Every source refuses a range past the end in the same words: a caller that
/// asked for bytes the file does not have has a corrupt idea of the file's
/// layout, and a short read would hide that from the seam above.
pub(crate) fn checked_range(
    path: &VirtualPath,
    offset: u64,
    len: usize,
    file_len: u64,
) -> Result<Range<usize>> {
    let start = usize::try_from(offset).map_err(|_| {
        Error::corrupt(
            path.to_string(),
            format!("offset {offset} does not fit in memory"),
        )
    })?;
    let end = start
        .checked_add(len)
        .ok_or_else(|| Error::corrupt(path.to_string(), "range overflows".to_string()))?;
    if end as u64 > file_len {
        return Err(Error::corrupt(
            path.to_string(),
            format!("asked for bytes {start}..{end} of a {file_len} byte file"),
        ));
    }
    Ok(start..end)
}

/// A file source that lives in memory.
///
/// Two jobs, one type. As a **stand-in for a game folder** it lets a test (or a
/// shell that already holds the bytes — a ZIP, a network fetch, an Android
/// `content://` handle) hand a seam a game without inventing a temporary
/// directory. As an **overlay** it can shadow a single path in a larger VFS,
/// which is how the host shows a seam a file the user named on the command line
/// without pretending the file is part of the game.
#[derive(Clone, Debug, Default)]
pub struct MemorySource {
    files: BTreeMap<VirtualPath, Vec<u8>>,
}

impl MemorySource {
    /// An empty source.
    pub fn new() -> Self {
        Self::default()
    }

    /// A source holding one file.
    pub fn single(path: &str, bytes: impl Into<Vec<u8>>) -> Self {
        let mut source = Self::new();
        source.insert(path, bytes);
        source
    }

    /// Add a file, replacing any file already under that virtual path.
    pub fn insert(&mut self, path: &str, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.files.insert(VirtualPath::new(path), bytes.into());
        self
    }

    /// The number of files held.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether no files are held.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The bytes under a path, or the refusal that says what this source does
    /// hold.
    fn bytes(&self, path: &VirtualPath) -> Result<&Vec<u8>> {
        match self.files.get(path) {
            Some(bytes) => Ok(bytes),
            None => Err(Error::NotFound(format!(
                "'{path}' is not in this in-memory source (it holds {} file(s))",
                self.files.len()
            ))),
        }
    }
}

impl FileSource for MemorySource {
    fn read(&self, path: &VirtualPath) -> Result<Vec<u8>> {
        Ok(self.bytes(path)?.clone())
    }

    fn read_range(&self, path: &VirtualPath, offset: u64, len: usize) -> Result<Vec<u8>> {
        let bytes = self.bytes(path)?;
        let range = checked_range(path, offset, len, bytes.len() as u64)?;
        Ok(bytes[range].to_vec())
    }

    fn list(&self) -> Vec<(VirtualPath, u64)> {
        self.files
            .iter()
            .map(|(path, bytes)| (path.clone(), bytes.len() as u64))
            .collect()
    }
}

/// Serves files from a real directory tree, read-only.
#[derive(Debug)]
pub struct DirectorySource {
    root: PathBuf,
    /// Every file in the tree, under the virtual path that names it.
    ///
    /// A virtual path is lower-cased, but a directory is not: a game extracted
    /// on Linux keeps whatever case the disc used, so `Graphics.bsa` on disk is
    /// `graphics.bsa` to the body, and building a real path by joining the two
    /// together finds nothing on a case-sensitive filesystem while happening to
    /// work on macOS. The index is what makes a lookup behave the same on all
    /// of them, and it is built once, on first use.
    index: OnceLock<HashMap<VirtualPath, PathBuf>>,
}

impl DirectorySource {
    /// Serve the tree rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            index: OnceLock::new(),
        }
    }

    /// The real directory backing this source.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The tree, as virtual path → real file.
    fn index(&self) -> &HashMap<VirtualPath, PathBuf> {
        self.index.get_or_init(|| {
            let mut files = Vec::new();
            Self::walk(&self.root, &self.root, &mut files);
            // Sorted first, so that when two names differ only by case — which
            // no virtual path can tell apart — the same one wins every run
            // instead of whichever the filesystem happened to list first.
            files.sort();
            let mut index = HashMap::with_capacity(files.len());
            for (path, real) in files {
                index.entry(path).or_insert(real);
            }
            index
        })
    }

    fn walk(dir: &Path, root: &Path, out: &mut Vec<(VirtualPath, PathBuf)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            if file_type.is_dir() {
                Self::walk(&entry.path(), root, out);
            } else if file_type.is_file() {
                let relative = match entry.path().strip_prefix(root) {
                    Ok(relative) => relative.to_string_lossy().into_owned(),
                    Err(_) => continue,
                };
                out.push((VirtualPath::new(&relative), entry.path()));
            }
        }
    }

    /// The real file a virtual path names.
    fn locate(&self, path: &VirtualPath) -> Result<PathBuf> {
        for component in path.components() {
            if component == ".." {
                // The body never lets a virtual path climb out of a source.
                return Err(Error::NotFound(format!(
                    "path traversal rejected: '{path}'"
                )));
            }
        }
        match self.index().get(path) {
            Some(real) => Ok(real.clone()),
            None => Err(Error::NotFound(format!(
                "'{path}' is not in {}",
                self.root.display()
            ))),
        }
    }
}

impl FileSource for DirectorySource {
    fn read(&self, path: &VirtualPath) -> Result<Vec<u8>> {
        Ok(fs::read(self.locate(path)?)?)
    }

    fn read_range(&self, path: &VirtualPath, offset: u64, len: usize) -> Result<Vec<u8>> {
        let real = self.locate(path)?;
        let range = checked_range(path, offset, len, fs::metadata(&real)?.len())?;
        let mut file = fs::File::open(&real)?;
        file.seek(SeekFrom::Start(range.start as u64))?;
        let mut bytes = vec![0u8; len];
        file.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    fn list(&self) -> Vec<(VirtualPath, u64)> {
        let mut out: Vec<(VirtualPath, u64)> = self
            .index()
            .iter()
            .map(|(path, real)| {
                let size = fs::metadata(real).map(|m| m.len()).unwrap_or(0);
                (path.clone(), size)
            })
            .collect();
        out.sort();
        out
    }
}

/// A stack of [`FileSource`]s: the mounted view of one game.
#[derive(Clone, Default)]
pub struct Vfs {
    sources: Vec<Arc<dyn FileSource>>,
}

impl Vfs {
    /// An empty VFS; add sources with [`Vfs::push_front`] / [`Vfs::push`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a VFS serving one directory tree.
    pub fn from_directory(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        if !root.is_dir() {
            return Err(Error::NotFound(format!(
                "game root '{}' is not a directory",
                root.display()
            )));
        }
        let mut vfs = Self::new();
        vfs.push(Arc::new(DirectorySource::new(root)));
        Ok(vfs)
    }

    /// Build a VFS serving one ISO 9660 disc image.
    ///
    /// The disc-image twin of [`Vfs::from_directory`], for the common case of a
    /// game that only ever existed on a CD: the image is the whole game, and
    /// every read comes straight out of it.
    pub fn from_iso(image: impl Into<PathBuf>) -> Result<Self> {
        let mut vfs = Self::new();
        vfs.push(Arc::new(crate::iso::IsoSource::open(image.into())?));
        Ok(vfs)
    }

    /// Number of mounted sources.
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// The mounted sources, topmost (searched first) first.
    ///
    /// Seams use this to re-stack sources when they mount: archive sources
    /// pushed in front, the original tree kept underneath.
    pub fn sources(&self) -> &[Arc<dyn FileSource>] {
        &self.sources
    }

    /// Add a source at the *bottom* of the stack (searched last).
    pub fn push(&mut self, source: Arc<dyn FileSource>) {
        self.sources.push(source);
    }

    /// Add a source at the *top* of the stack (searched first).
    ///
    /// Seams call this for every archive they mount, so archive contents
    /// shadow same-named loose files, exactly like the original engines'
    /// search order.
    pub fn push_front(&mut self, source: Arc<dyn FileSource>) {
        self.sources.insert(0, source);
    }

    /// Read a virtual file from the first source that has it.
    pub fn read(&self, path: &VirtualPath) -> Result<Vec<u8>> {
        for source in &self.sources {
            match source.read(path) {
                Ok(data) => return Ok(data),
                Err(Error::NotFound(_)) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(Error::NotFound(format!(
            "'{path}' not found in {} mounted source(s)",
            self.sources.len()
        )))
    }

    /// Read `len` bytes starting at `offset` inside a virtual file.
    ///
    /// The first source that has the path serves it, exactly as [`Vfs::read`]
    /// does, but a source that can seek only reads that far.
    pub fn read_range(&self, path: &VirtualPath, offset: u64, len: usize) -> Result<Vec<u8>> {
        for source in self.sources() {
            match source.read_range(path, offset, len) {
                Ok(bytes) => return Ok(bytes),
                Err(Error::NotFound(_)) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(Error::NotFound(format!(
            "'{path}' not found in {} mounted source(s)",
            self.sources.len()
        )))
    }

    /// Whether any source provides this path.
    pub fn exists(&self, path: &VirtualPath) -> bool {
        self.sources.iter().any(|s| s.read(path).is_ok())
    }

    /// All visible virtual files, deduplicated (topmost source wins),
    /// sorted by path.
    pub fn list(&self) -> Vec<(VirtualPath, u64)> {
        let mut seen: BTreeMap<VirtualPath, u64> = BTreeMap::new();
        for source in &self.sources {
            for (path, size) in source.list() {
                seen.entry(path).or_insert(size);
            }
        }
        seen.into_iter().collect()
    }

    /// All visible paths whose extension is in `extensions`, sorted.
    pub fn find_by_extension(&self, extensions: &[&str]) -> Vec<VirtualPath> {
        self.list()
            .into_iter()
            .map(|(path, _)| path)
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| extensions.contains(&ext))
            })
            .collect()
    }
}

impl fmt::Debug for Vfs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vfs")
            .field("sources", &self.sources.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_normalization() {
        assert_eq!(
            VirtualPath::new("DATA\\Title.ZBM").as_str(),
            "data/title.zbm"
        );
        assert_eq!(
            VirtualPath::new("./Game/\\snn//x.snn").as_str(),
            "game/snn/x.snn"
        );
        assert_eq!(VirtualPath::new("Archive.INX").extension(), Some("inx"));
        assert_eq!(VirtualPath::new(".hidden").extension(), None);
        assert_eq!(VirtualPath::new("dir/noext").extension(), None);
        assert_eq!(VirtualPath::new("a/b/c.png").file_name(), "c.png");
    }

    #[test]
    fn traversal_is_rejected() {
        let dir = std::env::temp_dir().join("kintsugi-vfs-test-traversal");
        std::fs::create_dir_all(&dir).unwrap();
        let source = DirectorySource::new(&dir);
        let evil = VirtualPath::new("../escape");
        assert!(matches!(
            source.read(&evil),
            Err(Error::NotFound(msg)) if msg.contains("traversal")
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_range_read_answers_from_the_file_on_disk() {
        let dir = std::env::temp_dir().join(format!("kintsugi-vfs-range-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("scene.dat"), b"0123456789").unwrap();
        let source = DirectorySource::new(&dir);
        let path = VirtualPath::new("scene.dat");

        assert_eq!(source.read_range(&path, 2, 3).unwrap(), b"234");
        assert_eq!(source.read_range(&path, 0, 10).unwrap(), b"0123456789");
        assert_eq!(source.read_range(&path, 10, 0).unwrap(), b"");
        // Past the end is refused, not shortened: a caller that asked for bytes
        // the file does not have has the file's layout wrong.
        let past_end = source.read_range(&path, 8, 4).unwrap_err();
        assert!(matches!(past_end, Error::Corrupt { .. }), "{past_end}");
        assert!(
            format!("{past_end}").contains("asked for bytes 8..12 of a 10 byte file"),
            "{past_end}"
        );
        // The directory itself is not a file, and neither is a missing path.
        assert!(matches!(
            source.read_range(&VirtualPath::new("."), 0, 1),
            Err(Error::NotFound(_))
        ));
        assert!(matches!(
            source.read_range(&VirtualPath::new("missing.dat"), 0, 1),
            Err(Error::NotFound(_))
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_capitalised_file_is_found_through_its_lower_cased_name() {
        // A virtual path is lower-cased; a directory is not. Joining the two
        // together happens to work on macOS, where the filesystem folds case,
        // and fails on Linux, where it does not — so the same release was
        // detected on one machine and invisible on the other. This test is the
        // regression: it only *fails* on a case-sensitive filesystem, which is
        // why Linux CI is the job that enforces it.
        let dir = std::env::temp_dir().join(format!("kintsugi-vfs-case-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("exe")).unwrap();
        std::fs::write(dir.join("exe").join("Graphics.BSA"), b"BSArc\0\0\0\0").unwrap();
        let vfs = Vfs::from_directory(&dir).unwrap();

        let listed: Vec<String> = vfs
            .list()
            .into_iter()
            .map(|(path, size)| format!("{path} ({size})"))
            .collect();
        assert_eq!(listed, vec!["exe/graphics.bsa (9)".to_string()]);
        assert_eq!(
            vfs.read_range(&VirtualPath::new("exe/graphics.bsa"), 0, 5)
                .unwrap(),
            b"BSArc"
        );
        // And the same file, asked for in the case it was written in.
        assert_eq!(
            vfs.read(&VirtualPath::new("EXE/Graphics.BSA")).unwrap(),
            b"BSArc\0\0\0\0"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_range_read_falls_through_to_the_source_that_has_the_file() {
        let dir = std::env::temp_dir().join(format!("kintsugi-vfs-stack-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("archive.dat"), b"the loose copy").unwrap();

        let mut vfs = Vfs::new();
        // The topmost source does not have the path at all, so the range has to
        // come out of the directory underneath.
        vfs.push_front(Arc::new(MemorySource::single(
            "other.dat",
            "something else",
        )));
        vfs.push(Arc::new(DirectorySource::new(&dir)));
        assert_eq!(
            vfs.read_range(&VirtualPath::new("archive.dat"), 4, 5)
                .unwrap(),
            b"loose"
        );
        let missing = vfs
            .read_range(&VirtualPath::new("absent.dat"), 0, 1)
            .unwrap_err();
        assert!(
            format!("{missing}").contains("not found in 2 mounted source(s)"),
            "{missing}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod memory_source_tests {
    use super::*;

    #[test]
    fn holds_what_was_put_in_it_and_says_what_it_does_not() {
        let mut source = MemorySource::new();
        assert!(source.is_empty());
        source.insert("Story.BDT", b"hello".to_vec());
        assert_eq!(source.len(), 1);
        // Paths canonicalize on the way in, so a seam's `VirtualPath::new` and
        // a caller's string meet in the same place.
        assert_eq!(
            source.read(&VirtualPath::new("story.bdt")).unwrap(),
            b"hello"
        );
        assert_eq!(source.list(), vec![(VirtualPath::new("story.bdt"), 5u64)]);
        let missing = source.read(&VirtualPath::new("other.bdt")).unwrap_err();
        assert!(
            format!("{missing}").contains("holds 1 file(s)"),
            "the refusal should say what the source does have: {missing}"
        );
    }

    #[test]
    fn a_range_read_takes_exactly_the_bytes_asked_for() {
        let source = MemorySource::single("story.bdt", "hello world");
        let path = VirtualPath::new("story.bdt");
        assert_eq!(source.read_range(&path, 6, 5).unwrap(), b"world");
        assert_eq!(source.read_range(&path, 0, 0).unwrap(), b"");
        assert_eq!(source.read_range(&path, 11, 0).unwrap(), b"");
        let past_end = source.read_range(&path, 6, 100).unwrap_err();
        assert!(matches!(past_end, Error::Corrupt { .. }), "{past_end}");
        assert!(
            format!("{past_end}").contains("asked for bytes 6..106 of a 11 byte file"),
            "{past_end}"
        );
        let missing = source
            .read_range(&VirtualPath::new("other.bdt"), 0, 1)
            .unwrap_err();
        assert!(matches!(missing, Error::NotFound(_)), "{missing}");
    }

    #[test]
    fn shadows_one_path_in_a_larger_vfs_and_leaves_the_rest_alone() {
        let dir =
            std::env::temp_dir().join(format!("kintsugi-memory-overlay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("story.bdt"), b"the original").unwrap();
        std::fs::write(dir.join("other.bdt"), b"another original").unwrap();

        let mut vfs = Vfs::from_directory(&dir).unwrap();
        vfs.push_front(Arc::new(MemorySource::single("story.bdt", "the patch")));
        assert_eq!(
            vfs.read(&VirtualPath::new("story.bdt")).unwrap(),
            b"the patch"
        );
        assert_eq!(
            vfs.read(&VirtualPath::new("other.bdt")).unwrap(),
            b"another original",
            "the overlay must shadow only the path it holds"
        );
        // The shadowed file keeps its place in the listing, with the size of
        // the bytes that will actually be read.
        let listed = vfs.list();
        assert_eq!(
            listed
                .iter()
                .find(|(path, _)| path.as_str() == "story.bdt")
                .map(|(_, size)| *size),
            Some(9),
            "listing: {listed:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
