//! The plugin contract: how a golden seam attaches to the body.
//!
//! One engine = one crate = one [`EnginePlugin`]. The seam sees only the
//! [`Vfs`] the host mounts; it answers two questions — *is this my engine?*
//! ([`EnginePlugin::detect`]) and *give me a playable view of it*
//! ([`EnginePlugin::mount`]). Everything else (script IR, images, audio,
//! interpretation) is expressed in the body's types, so hosts and seams stay
//! strangers to each other.
//!
//! Seams are linked statically through a [`Registry`]: Rust has no stable
//! ABI, and a preservation engine must still build from source in twenty
//! years. The trait boundary below is deliberately dylib-shaped, so a
//! dynamic loader can be added later without rewriting any seam.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::asset::{Audio, Image};
use crate::detect::{Confidence, Detection};
use crate::error::{Error, Result};
use crate::script::Script;
use crate::vfs::{Vfs, VirtualPath};

/// What a seam calls itself.
pub struct PluginMetadata {
    /// Short engine id, e.g. `"bluegale"`.
    pub id: &'static str,
    /// Implementing crate, e.g. `"kintsugi-bluegale"`.
    pub crate_name: &'static str,
    /// Human-facing engine name, e.g. `"BlueGale (ブルーゲイル)"`.
    pub display_name: &'static str,
    /// Seam crate version.
    pub version: &'static str,
    /// File extensions this seam understands (lower-cased, no dot).
    pub file_extensions: &'static [&'static str],
}

impl PluginMetadata {
    /// One-line summary for reports.
    pub fn summary(&self) -> String {
        format!(
            "{} {} [{}] — {}",
            self.display_name,
            self.version,
            self.id,
            self.file_extensions.join(", ")
        )
    }
}

/// A mounted game: seams hand back this view, not raw handles.
#[derive(Debug)]
pub struct MountInfo {
    /// Engine id that mounted this game.
    pub engine: String,
    /// Every workaround, skip, and heuristic applied while mounting.
    /// Displayed by hosts — the gold must stay visible.
    pub notes: Vec<String>,
}

impl MountInfo {
    /// New mount info for `engine` with no notes yet.
    pub fn new(engine: impl Into<String>) -> Self {
        Self {
            engine: engine.into(),
            notes: Vec::new(),
        }
    }

    /// Record a visible work note.
    pub fn note(&mut self, text: impl Into<String>) {
        self.notes.push(text.into());
    }
}

/// The playable view a seam builds over a mounted game.
pub trait EngineMount: Send + Sync {
    /// Engine and visible work notes.
    fn info(&self) -> &MountInfo;

    /// The overlaid VFS (mounted archives shadowing loose files).
    fn vfs(&self) -> &Vfs;

    /// Decode an image asset.
    fn read_image(&self, path: &VirtualPath) -> Result<Image> {
        let _ = path;
        Err(Error::unsupported(
            self.info().engine.clone(),
            "this seam does not decode images yet",
        ))
    }

    /// Read an audio asset as a pass-through container.
    fn read_audio(&self, path: &VirtualPath) -> Result<Audio> {
        let _ = path;
        Err(Error::unsupported(
            self.info().engine.clone(),
            "this seam does not read audio yet",
        ))
    }

    /// Read a script into the body's IR.
    fn read_script(&self, path: &VirtualPath) -> Result<Script> {
        let _ = path;
        Err(Error::unsupported(
            self.info().engine.clone(),
            "this seam does not read scripts yet",
        ))
    }

    /// Which script is the game's, for a caller that did not name one.
    ///
    /// The host must not know that one engine's main script is called
    /// `story.bdt`, or that scripts have extensions at all, so the seam
    /// answers: this is the engine's own convention, and it belongs here with
    /// the rest of the engine's knowledge.
    ///
    /// Refusing is a supported, and often the honest, answer — a game with
    /// several scripts and no obvious main one is a question only the person
    /// holding the game can settle. A seam that does answer must name a script
    /// it can actually read out of a mount of its own game, which
    /// `kintsugi-testkit` checks.
    fn primary_script(&self) -> Result<VirtualPath> {
        Err(Error::unsupported(
            self.info().engine.clone(),
            "this seam does not name a main script",
        ))
    }

    /// Extensions this seam's [`EngineMount::read_image`] accepts, bare and
    /// lower-case, so a caller can list a game's pictures or pick one when the
    /// user did not.
    ///
    /// Empty means the seam does not decode images. This is discovery, not
    /// evidence: an extension still earns no trust, and a seam stays free to
    /// refuse a file that carries one of these.
    fn image_extensions(&self) -> &'static [&'static str] {
        &[]
    }

    /// Write a repaired script back out, when this seam knows how.
    ///
    /// `replacements` maps a command index — the same `id` a translation
    /// entry carries — to the text that should stand in its place. A seam
    /// that implements this is expected to change **only** those lines and
    /// leave every other byte of the original file alone, because a patch
    /// that quietly reformats a game is worse than no patch.
    ///
    /// The default refuses: a seam that cannot write must say so rather than
    /// emit something plausible.
    fn write_script(
        &self,
        path: &VirtualPath,
        replacements: &BTreeMap<usize, String>,
    ) -> Result<WrittenScript> {
        let _ = (path, replacements);
        Err(Error::unsupported(
            self.info().engine.clone(),
            "this seam cannot write scripts back yet",
        ))
    }
}

/// One file a repair changed, and the bytes to write into it.
///
/// It is a *list* of these rather than a single blob because a script is not
/// always a file. BlueGale keeps `.bdt` scripts both loose and inside its SNN
/// archives, and a script packed in an archive cannot be repaired by writing a
/// `.bdt`: the bytes live in the blob, and the INX index that points at them
/// has to be rewritten with its new offset and size. A seam that pretends
/// otherwise would either refuse every packed script or write a file the game
/// never reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrittenFile {
    /// Where to write, as a path inside the game folder — the script itself for
    /// a loose script, the archive and its index for a packed one.
    pub path: VirtualPath,
    /// The complete new contents of that file.
    pub data: Vec<u8>,
}

/// A repaired script: the script's own new bytes, the files a game folder needs
/// changed, and what happened on the way.
///
/// Two answers rather than one, because the two callers want different things
/// and conflating them is how a tool writes a file nobody asked for. `translate
/// --write-script` wants a *patch*: the translated script as a file the seam can
/// parse again, which is `script`. `install` wants the *repair*: whatever files
/// in the copy have to change for the game to play the translation, which is
/// `files` — one file for a loose script, and for a script packed in an archive
/// the blob and the index that points into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrittenScript {
    /// The script's own new bytes: exactly what a patch file holds, whether or
    /// not the script lives in a file of its own.
    pub script: Vec<u8>,
    /// Every file the repair changes, with its new bytes. Never empty: a repair
    /// that changed nothing is a refusal, not an empty success. The caller
    /// chooses where to put them; seams never write.
    pub files: Vec<WrittenFile>,
    /// How many of the requested replacements were applied.
    pub replaced: usize,
    /// Command indices the original file does not have — ids drift when the
    /// script and the translation come from different versions, and a silent
    /// mismatch here is how a patch lands on the wrong line.
    pub unmatched: Vec<usize>,
    /// Anything the seam had to decide the caller should repeat out loud —
    /// the same honesty rule as a mount's warnings, carried with the repair
    /// so an install can say it.
    pub notes: Vec<String>,
}

impl WrittenScript {
    /// Attach a note the caller should repeat when it reports the repair.
    pub fn noting(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// The common case: a script that is a file of its own, so the repair is
    /// one file and the script's bytes are that file's bytes.
    pub fn loose(
        path: VirtualPath,
        script: Vec<u8>,
        replaced: usize,
        unmatched: Vec<usize>,
    ) -> Self {
        Self {
            files: vec![WrittenFile {
                path,
                data: script.clone(),
            }],
            script,
            replaced,
            unmatched,
            notes: Vec::new(),
        }
    }

    /// A script packed inside a container: its own bytes, and the files the
    /// game folder needs changed to carry them.
    pub fn packed(
        script: Vec<u8>,
        files: Vec<WrittenFile>,
        replaced: usize,
        unmatched: Vec<usize>,
    ) -> Self {
        Self {
            script,
            files,
            replaced,
            unmatched,
            notes: Vec::new(),
        }
    }

    /// The paths this repair writes, for a report.
    pub fn paths(&self) -> impl Iterator<Item = &VirtualPath> {
        self.files.iter().map(|file| &file.path)
    }
}

/// One dead engine's golden seam.
pub trait EnginePlugin: Send + Sync {
    /// Stable metadata for reports.
    fn metadata(&self) -> &'static PluginMetadata;

    /// Decide whether the mounted files belong to this engine.
    ///
    /// Return one [`Detection`] per opinion (usually the strongest one);
    /// an empty `Vec` means "not mine". Detection must be read-only: no
    /// writing, no moving, no repairing the fan's files.
    fn detect(&self, vfs: &Vfs) -> Result<Vec<Detection>>;

    /// Build a playable view of the game.
    ///
    /// Called only after the host chose this seam (usually via
    /// [`Registry::detect_all`] + [`Registry::mount_best`]). Mounting may
    /// read everything but must still modify nothing on disk.
    fn mount(&self, vfs: &Vfs) -> Result<Box<dyn EngineMount>>;
}

/// The set of seams compiled into a host.
#[derive(Default)]
pub struct Registry {
    plugins: Vec<Arc<dyn EnginePlugin>>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one seam.
    pub fn register(&mut self, plugin: Arc<dyn EnginePlugin>) -> &mut Self {
        self.plugins.push(plugin);
        self
    }

    /// All registered seams, in registration order.
    pub fn plugins(&self) -> &[Arc<dyn EnginePlugin>] {
        &self.plugins
    }

    /// Number of registered seams.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Whether no seam is registered.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Find a seam by engine id.
    pub fn find(&self, id: &str) -> Option<Arc<dyn EnginePlugin>> {
        self.plugins.iter().find(|p| p.metadata().id == id).cloned()
    }

    /// Ask every seam what it thinks, strongest opinion first.
    pub fn detect_all(&self, vfs: &Vfs) -> Vec<(usize, Detection)> {
        let mut verdicts: Vec<(usize, Detection)> = Vec::new();
        for (index, plugin) in self.plugins.iter().enumerate() {
            if let Ok(detections) = plugin.detect(vfs) {
                for detection in detections {
                    verdicts.push((index, detection));
                }
            }
        }
        verdicts.sort_by(|a, b| {
            b.1.confidence
                .cmp(&a.1.confidence)
                .then_with(|| a.1.engine.cmp(b.1.engine))
        });
        verdicts
    }

    /// Detect, pick the most confident seam, and mount through it.
    ///
    /// Fails when nothing scores at least [`Confidence::Possible`].
    pub fn mount_best(&self, vfs: &Vfs) -> Result<(Arc<dyn EnginePlugin>, Box<dyn EngineMount>)> {
        let verdicts = self.detect_all(vfs);
        let Some((index, best)) = verdicts.first() else {
            return Err(Error::unsupported(
                "auto-detect",
                "no registered engine recognizes these files",
            ));
        };
        if best.confidence < Confidence::Possible {
            return Err(Error::unsupported(
                "auto-detect",
                "best verdict is only 'unlikely'; refusing to mount",
            ));
        }
        let plugin = self.plugins[*index].clone();
        let mount = plugin.mount(vfs)?;
        Ok((plugin, mount))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubPlugin {
        metadata: &'static PluginMetadata,
        confidence: Confidence,
    }

    impl EnginePlugin for StubPlugin {
        fn metadata(&self) -> &'static PluginMetadata {
            self.metadata
        }

        fn detect(&self, _vfs: &Vfs) -> Result<Vec<Detection>> {
            Ok(vec![Detection::new(
                self.metadata.id,
                self.confidence,
                "stub",
            )])
        }

        fn mount(&self, _vfs: &Vfs) -> Result<Box<dyn EngineMount>> {
            Err(Error::Plugin("stub does not mount".into()))
        }
    }

    fn meta(id: &'static str) -> &'static PluginMetadata {
        static A: std::sync::OnceLock<PluginMetadata> = std::sync::OnceLock::new();
        static B: std::sync::OnceLock<PluginMetadata> = std::sync::OnceLock::new();
        match id {
            "aaa" => A.get_or_init(|| PluginMetadata {
                id: "aaa",
                crate_name: "stub-aaa",
                display_name: "Stub A",
                version: "0",
                file_extensions: &["aaa"],
            }),
            _ => B.get_or_init(|| PluginMetadata {
                id: "bbb",
                crate_name: "stub-bbb",
                display_name: "Stub B",
                version: "0",
                file_extensions: &["bbb"],
            }),
        }
    }

    #[test]
    fn registry_sorts_by_confidence() {
        let mut registry = Registry::new();
        registry.register(Arc::new(StubPlugin {
            metadata: meta("aaa"),
            confidence: Confidence::Certain,
        }));
        registry.register(Arc::new(StubPlugin {
            metadata: meta("bbb"),
            confidence: Confidence::Likely,
        }));

        let vfs = Vfs::new();
        let verdicts = registry.detect_all(&vfs);
        assert_eq!(verdicts[0].1.engine, "aaa");
        assert_eq!(verdicts[1].1.engine, "bbb");
        assert_eq!(registry.find("bbb").map(|p| p.metadata().id), Some("bbb"));
        assert!(registry.find("zzz").is_none());
    }
}
