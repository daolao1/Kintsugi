//! The BlueGale seam: detection + mounting.

use std::collections::BTreeMap;
use std::sync::Arc;

use kintsugi_core::asset::{Audio, Image};
use kintsugi_core::detect::{Confidence, Detection};
use kintsugi_core::error::{Error, Result};
use kintsugi_core::plugin::{
    EngineMount, EnginePlugin, MountInfo, PluginMetadata, WrittenFile, WrittenScript,
};
use kintsugi_core::script::Script;
use kintsugi_core::vfs::{Vfs, VirtualPath};

use crate::bbm;
use crate::bdt;
use crate::snn::{SnnArchive, SnnSource};
use crate::zbm;

/// Registry id of this seam.
pub const ENGINE_ID: &str = "bluegale";

static METADATA: PluginMetadata = PluginMetadata {
    id: ENGINE_ID,
    crate_name: "kintsugi-bluegale",
    display_name: "BlueGale (ブルーゲイル)",
    version: env!("CARGO_PKG_VERSION"),
    file_extensions: &["snn", "inx", "zbm", "bbm", "bdt", "amv"],
};

/// The seam's entry point.
pub fn plugin() -> BluegalePlugin {
    BluegalePlugin
}

/// Golden seam for the BlueGale engine.
pub struct BluegalePlugin;

impl EnginePlugin for BluegalePlugin {
    fn metadata(&self) -> &'static PluginMetadata {
        &METADATA
    }

    fn detect(&self, vfs: &Vfs) -> Result<Vec<Detection>> {
        let mut best: Option<Detection> = None;
        let mut upgrade = |candidate: Detection| {
            if candidate.confidence > best.as_ref().map_or(Confidence::Unlikely, |d| d.confidence) {
                best = Some(candidate);
            }
        };

        // Strongest signal: a structurally valid INX + SNN pair.
        let mut archives = 0usize;
        let mut archive_entries = 0usize;
        for inx_path in vfs.find_by_extension(&["inx"]) {
            let Ok(inx_bytes) = vfs.read(&inx_path) else {
                continue;
            };
            let Ok(entries) = crate::snn::parse_inx(&inx_bytes) else {
                continue;
            };
            let Some(snn_path) = sibling_with_extension(&inx_path, "snn") else {
                continue;
            };
            let Ok(snn_bytes) = vfs.read(&snn_path) else {
                continue;
            };
            if SnnArchive::open(&inx_bytes, snn_bytes, snn_path.as_str()).is_ok() {
                archives += 1;
                archive_entries += entries.len();
            }
        }
        if archives > 0 {
            upgrade(Detection::new(
                ENGINE_ID,
                Confidence::Certain,
                format!(
                    "{archives} SNN archive(s) with valid INX indexes ({archive_entries} entries)"
                ),
            ));
        }

        // BDT scripts: strong, but the line semantics are heuristic.
        let mut scripts = 0usize;
        for bdt_path in vfs.find_by_extension(&["bdt"]) {
            let Ok(bytes) = vfs.read(&bdt_path) else {
                continue;
            };
            if bdt::looks_like_bdt(&bytes) {
                scripts += 1;
            }
        }
        if scripts > 0 {
            upgrade(Detection::new(
                ENGINE_ID,
                Confidence::Likely,
                format!(
                    "{scripts} BDT script(s) — XOR-0xFF CP932 text with $/% labels \
                     (line semantics not fully reverse-engineered)"
                ),
            ));
        }

        // Loose images: weak alone, corroborating with others.
        let mut images = 0usize;
        for path in vfs.find_by_extension(&["zbm", "bbm"]) {
            let Ok(bytes) = vfs.read(&path) else { continue };
            if zbm::is_zbm(&bytes) || bbm::is_bbm(&bytes) {
                images += 1;
            }
        }
        if images > 0 {
            upgrade(Detection::new(
                ENGINE_ID,
                Confidence::Possible,
                format!("{images} ZBM/BBM image(s) with valid signatures"),
            ));
        }

        Ok(best.into_iter().collect())
    }

    fn mount(&self, vfs: &Vfs) -> Result<Box<dyn EngineMount>> {
        let mut mounted = Vfs::new();
        let mut info = MountInfo::new(ENGINE_ID);
        let mut mount_count = 0usize;
        let mut archives: Vec<MountedArchive> = Vec::new();

        // The caller's source order is preserved, and each archive is inserted
        // **directly in front of the source it was found in** — an archive
        // shadows the loose files beside it, and nothing else.
        //
        // Pushing every archive to the very front instead is the bug this
        // replaced: `install` overlays the patch by putting a source in front of
        // the game, and a game whose script is packed would have had its own
        // archive shadow that overlay, so the patch was read back as the
        // *original* script, no line differed, and the install reported success
        // while changing nothing. A layer a caller mounted above the game has to
        // stay above it.
        for source in vfs.sources() {
            for (inx_path, _) in source.list() {
                if inx_path.extension() != Some("inx") {
                    continue;
                }
                let inx_bytes = match source.read(&inx_path) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        info.note(format!("skipping '{inx_path}': cannot read it: {e}"));
                        continue;
                    }
                };
                let entries = match crate::snn::parse_inx(&inx_bytes) {
                    Ok(entries) => entries,
                    Err(e) => {
                        info.note(format!("skipping '{inx_path}': {e}"));
                        continue;
                    }
                };
                let Some(snn_path) = sibling_with_extension(&inx_path, "snn") else {
                    info.note(format!(
                        "skipping '{}': no sibling .snn file",
                        inx_path.file_name()
                    ));
                    continue;
                };
                let snn_bytes = match source.read(&snn_path) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        info.note(format!(
                            "skipping '{}': cannot read {snn_path} beside it: {e}",
                            inx_path.file_name()
                        ));
                        continue;
                    }
                };
                let archive = SnnArchive::open(&inx_bytes, snn_bytes, snn_path.as_str())
                    .map_err(|e| Error::Plugin(format!("mounting {inx_path}: {e}")))?;
                info.note(format!(
                    "mounted '{}': {} entries",
                    snn_path.file_name(),
                    entries.len()
                ));
                let archive = Arc::new(archive);
                mounted.push(Arc::new(SnnSource::new(archive.clone())));
                archives.push(MountedArchive {
                    inx: inx_path.clone(),
                    snn: snn_path,
                    archive,
                });
                mount_count += 1;
            }
            // Behind its own archive, in front of the sources that came after.
            mounted.push(source.clone());
        }

        if mount_count == 0 && vfs.find_by_extension(&["bdt", "zbm", "bbm"]).is_empty() {
            return Err(Error::unsupported(
                "BlueGale",
                "no SNN/INX archives, BDT scripts, or ZBM/BBM images found",
            ));
        }

        Ok(Box::new(BluegaleMount {
            info,
            vfs: mounted,
            archives,
        }))
    }
}

fn sibling_with_extension(path: &VirtualPath, extension: &str) -> Option<VirtualPath> {
    let name = path.file_name();
    let stem = name.rsplit_once('.').map(|(stem, _)| stem)?;
    Some(VirtualPath::new(&format!(
        "{}/{}.{}",
        parent_of(path),
        stem,
        extension
    )))
}

fn parent_of(path: &VirtualPath) -> String {
    match path.as_str().rsplit_once('/') {
        Some((dir, _)) => dir.to_string(),
        None => String::new(),
    }
}

/// The playable view of a mounted BlueGale game.
/// One archive this mount serves, with the names of the two files it is.
///
/// Kept so that a script packed inside an archive can be written back where it
/// came from: the bytes go into the blob and the index that points at them is
/// rewritten with them, which is a repair of *two* files rather than one.
struct MountedArchive {
    inx: VirtualPath,
    snn: VirtualPath,
    archive: Arc<SnnArchive>,
}

pub struct BluegaleMount {
    info: MountInfo,
    vfs: Vfs,
    /// In mount order; the last one is the front-most source, so a lookup walks
    /// this backwards to find the archive the mounted view would read from.
    archives: Vec<MountedArchive>,
}

impl BluegaleMount {
    /// The archive that serves `path`, if the mounted view gets it from one.
    ///
    /// Walks backwards because archives are pushed to the front of the mounted
    /// view as they are opened: the last one opened is the one a read of a name
    /// they share actually reaches.
    fn archive_holding(&self, path: &VirtualPath) -> Option<&MountedArchive> {
        self.archives
            .iter()
            .rev()
            .find(|host| host.archive.find(path.as_str()).is_some())
    }
}

impl EngineMount for BluegaleMount {
    fn info(&self) -> &MountInfo {
        &self.info
    }

    fn vfs(&self) -> &Vfs {
        &self.vfs
    }

    fn read_image(&self, path: &VirtualPath) -> Result<Image> {
        let bytes = self.vfs.read(path)?;
        if zbm::is_zbm(&bytes) {
            zbm::decode_zbm(&bytes)
        } else if bbm::is_bbm(&bytes) {
            bbm::decode_bbm(&bytes)
        } else if bytes.starts_with(b"BM") {
            // Loose, unobfuscated bitmaps travel with some releases.
            kintsugi_core::codec::bmp::decode_bmp(&bytes)
        } else {
            Err(Error::unsupported(
                "BlueGale image",
                format!("'{path}' matches no known image signature (ZBM 'amp_', BBM, plain BMP)"),
            ))
        }
    }

    fn read_audio(&self, path: &VirtualPath) -> Result<Audio> {
        Ok(Audio::new(self.vfs.read(path)?))
    }

    fn read_script(&self, path: &VirtualPath) -> Result<Script> {
        let bytes = self.vfs.read(path)?;
        if path.extension() == Some("bdt") {
            return bdt::parse_bdt(path.clone(), &bytes);
        }
        Err(Error::unsupported(
            "BlueGale script",
            format!("'{path}' is not a .bdt script"),
        ))
    }

    /// Write a repaired BDT: same container, only the translated lines changed.
    ///
    /// This seam never touches the disk: it returns bytes, and the host
    /// decides where they go. A seam that could write would be a seam that
    /// could break the fan's original.
    fn write_script(
        &self,
        path: &VirtualPath,
        replacements: &BTreeMap<usize, String>,
    ) -> Result<WrittenScript> {
        let bytes = self.vfs.read(path)?;
        let rewritten = bdt::rewrite_bdt(&bytes, replacements)?;

        // The script may be a file of its own, or an entry inside an archive.
        // Which one decides what gets written: a loose script is one file, a
        // packed one is the blob plus the index that points into it.
        if let Some(host) = self.archive_holding(path) {
            let index = host.archive.find(path.as_str()).ok_or_else(|| {
                Error::Plugin(format!(
                    "'{path}' is served by '{}' but the archive cannot find it by name",
                    host.snn
                ))
            })?;
            let mut replacement = BTreeMap::new();
            let script = rewritten.data;
            replacement.insert(index, script.clone());
            let (snn, inx) = host.archive.rebuilt(&replacement)?;
            return Ok(WrittenScript::packed(
                script,
                vec![
                    WrittenFile {
                        path: host.snn.clone(),
                        data: snn,
                    },
                    WrittenFile {
                        path: host.inx.clone(),
                        data: inx,
                    },
                ],
                rewritten.replaced,
                rewritten.unmatched,
            ));
        }

        Ok(WrittenScript::loose(
            path.clone(),
            rewritten.data,
            rewritten.replaced,
            rewritten.unmatched,
        ))
    }
}
