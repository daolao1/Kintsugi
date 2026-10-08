//! The BlueGale seam: detection + mounting.

use std::collections::BTreeMap;
use std::sync::Arc;

use kintsugi_core::asset::{Audio, Image};
use kintsugi_core::detect::{Confidence, Detection};
use kintsugi_core::error::{Error, Result};
use kintsugi_core::plugin::{EngineMount, EnginePlugin, MountInfo, PluginMetadata, WrittenScript};
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

        let mut inx_paths = vfs.find_by_extension(&["inx"]);
        inx_paths.sort();
        for inx_path in &inx_paths {
            let inx_bytes = vfs
                .read(inx_path)
                .map_err(|e| Error::Plugin(format!("reading {}: {e}", inx_path)))?;
            let entries = crate::snn::parse_inx(&inx_bytes)
                .map_err(|e| Error::Plugin(format!("parsing {}: {e}", inx_path)))?;
            let Some(snn_path) = sibling_with_extension(inx_path, "snn") else {
                info.note(format!(
                    "skipping '{}': no sibling .snn file",
                    inx_path.file_name()
                ));
                continue;
            };
            let snn_bytes = match vfs.read(&snn_path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    info.note(format!(
                        "skipping '{}': cannot read {snn_path}: {e}",
                        inx_path.file_name()
                    ));
                    continue;
                }
            };
            let archive = SnnArchive::open(&inx_bytes, snn_bytes, snn_path.as_str())
                .map_err(|e| Error::Plugin(format!("mounting {}: {e}", inx_path)))?;
            info.note(format!(
                "mounted '{}': {} entries",
                snn_path.file_name(),
                entries.len()
            ));
            mounted.push_front(Arc::new(SnnSource::new(Arc::new(archive))));
            mount_count += 1;
        }

        if mount_count == 0 && vfs.find_by_extension(&["bdt", "zbm", "bbm"]).is_empty() {
            return Err(Error::unsupported(
                "BlueGale",
                "no SNN/INX archives, BDT scripts, or ZBM/BBM images found",
            ));
        }

        // Archives shadow loose files; the original tree stays reachable
        // underneath, for scripts and anything not packed.
        for source in vfs.sources() {
            mounted.push(source.clone());
        }

        Ok(Box::new(BluegaleMount { info, vfs: mounted }))
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
pub struct BluegaleMount {
    info: MountInfo,
    vfs: Vfs,
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
        bdt::rewrite_bdt(&bytes, replacements)
    }
}
