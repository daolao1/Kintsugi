//! Virtual filesystem: a stack of sources that mounts archives over loose
//! files without ever touching the original discs.
//!
//! Preservation rule: the gold never melts the pottery. Seams expose
//! archives as [`FileSource`]s pushed *in front of* the directory a fan
//! dumped the game into; the untouched files stay byte-for-byte intact on
//! disk.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

    /// Every file this source provides, with byte sizes.
    fn list(&self) -> Vec<(VirtualPath, u64)>;
}

/// Serves files from a real directory tree, read-only.
#[derive(Debug, Clone)]
pub struct DirectorySource {
    root: PathBuf,
}

impl DirectorySource {
    /// Serve the tree rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The real directory backing this source.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn resolve(&self, path: &VirtualPath) -> Result<PathBuf> {
        for component in path.components() {
            if component == ".." {
                // The body never lets a virtual path climb out of a source.
                return Err(Error::NotFound(format!(
                    "path traversal rejected: '{path}'"
                )));
            }
        }
        let mut real = self.root.clone();
        for component in path.components() {
            real.push(component);
        }
        Ok(real)
    }

    fn walk(dir: &Path, root: &Path, out: &mut Vec<(VirtualPath, u64)>) {
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
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                let relative = match entry.path().strip_prefix(root) {
                    Ok(relative) => relative.to_string_lossy().into_owned(),
                    Err(_) => continue,
                };
                out.push((VirtualPath::new(&relative), size));
            }
        }
    }
}

impl FileSource for DirectorySource {
    fn read(&self, path: &VirtualPath) -> Result<Vec<u8>> {
        let real = self.resolve(path)?;
        match real.is_file() {
            true => Ok(fs::read(&real)?),
            false => Err(Error::NotFound(format!(
                "'{path}' (in {}) ",
                self.root.display()
            ))),
        }
    }

    fn list(&self) -> Vec<(VirtualPath, u64)> {
        let mut out = Vec::new();
        Self::walk(&self.root, &self.root, &mut out);
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
}
