//! The BSX seam: detection, mounting and asset decoding.

use std::sync::Arc;

use kintsugi_core::asset::{Audio, Image};
use kintsugi_core::detect::{Confidence, Detection};
use kintsugi_core::error::{Error, Result};
use kintsugi_core::plugin::{EngineMount, EnginePlugin, MountInfo, PluginMetadata};
use kintsugi_core::vfs::{Vfs, VirtualPath};

use crate::bsarc::BsarcArchive;
use crate::image;

/// Registry id of this seam.
///
/// The game names its own system: `bsx.ini` sits beside `bsx.dat`, and the
/// first line of that configuration is the game and the studio, `ONI@BLUEGALE`.
/// "BSX" is therefore what the engine calls itself, which is the name a repair
/// should carry.
pub const ENGINE_ID: &str = "bsx";

/// The script format's magic, used as evidence of the engine rather than of a
/// file name.
pub const SCRIPT_MAGIC: &[u8] = b"BSScript";

static METADATA: PluginMetadata = PluginMetadata {
    id: ENGINE_ID,
    crate_name: "kintsugi-bsx",
    display_name: "BlueGale BSX (BSArc · BSScript)",
    version: env!("CARGO_PKG_VERSION"),
    file_extensions: &["bsa", "bsg", "bmp", "dat", "ogg"],
};

/// The seam's entry point.
pub fn plugin() -> BsxPlugin {
    BsxPlugin
}

/// Golden seam for BlueGale's later BSX engine.
pub struct BsxPlugin;

impl EnginePlugin for BsxPlugin {
    fn metadata(&self) -> &'static PluginMetadata {
        &METADATA
    }

    fn detect(&self, vfs: &Vfs) -> Result<Vec<Detection>> {
        let mut archives = 0usize;
        let mut archive_entries = 0usize;
        for (path, size) in vfs.list() {
            if path.extension() != Some("bsa") {
                continue;
            }
            let Ok(header) = vfs.read_range(&path, 0, 16) else {
                continue;
            };
            let Ok(records) = index_bytes(|p, o, l| vfs.read_range(p, o, l), &path, &header, size)
            else {
                continue;
            };
            if let Ok(archive) = BsarcArchive::from_parts(&header, &records, path.clone(), size) {
                archives += 1;
                archive_entries += archive.len();
            }
        }

        // The engine's own script format: the strongest statement a folder can
        // make about which system it belongs to, because the format is not
        // shared with anyone.
        let mut scripts = Vec::new();
        for (path, size) in vfs.list() {
            if size < SCRIPT_MAGIC.len() as u64 {
                continue;
            }
            if path.extension() != Some("dat") {
                continue;
            }
            if let Ok(head) = vfs.read_range(&path, 0, SCRIPT_MAGIC.len()) {
                if head == SCRIPT_MAGIC {
                    scripts.push(path);
                }
            }
        }

        // Loose pictures: real evidence of the image format, weaker evidence of
        // the engine, since Bishop's games carry the same pictures.
        let mut loose_images = 0usize;
        for (path, size) in vfs.list() {
            if path.extension() != Some("bsg") || size < image::GRAPHICS_MAGIC.len() as u64 {
                continue;
            }
            if let Ok(head) = vfs.read_range(&path, 0, image::GRAPHICS_MAGIC.len()) {
                if head == image::GRAPHICS_MAGIC || head == image::COMPOSITION_MAGIC {
                    loose_images += 1;
                }
            }
        }

        let mut verdicts = Vec::new();
        if archives > 0 && !scripts.is_empty() {
            verdicts.push(Detection::new(
                ENGINE_ID,
                Confidence::Certain,
                format!(
                    "{} BSArc archive(s) with valid indexes ({archive_entries} entries) and {} \
                     BSScript file(s)",
                    archives,
                    scripts.len()
                ),
            ));
        } else if archives > 0 {
            verdicts.push(Detection::new(
                ENGINE_ID,
                Confidence::Likely,
                format!(
                    "{archives} BSArc archive(s) with valid indexes ({archive_entries} entries), \
                     no BSScript file: BlueGale and Bishop share this container, so the engine is \
                     not pinned down"
                ),
            ));
        }
        if !scripts.is_empty() && archives == 0 {
            verdicts.push(Detection::new(
                ENGINE_ID,
                Confidence::Likely,
                format!("{} BSScript file(s) but no BSArc archive", scripts.len()),
            ));
        }
        if archives == 0 && scripts.is_empty() && loose_images > 0 {
            verdicts.push(Detection::new(
                ENGINE_ID,
                Confidence::Possible,
                format!("{loose_images} loose BSG image(s): the picture format without its engine"),
            ));
        }
        Ok(verdicts)
    }

    fn mount(&self, vfs: &Vfs) -> Result<Box<dyn EngineMount>> {
        let mut mounted = Vfs::new();
        let mut info = MountInfo::new(ENGINE_ID);
        let mut count = 0usize;
        let mut entries = 0usize;

        // Source order is preserved and each archive is inserted directly in
        // front of the source it came from, so an archive shadows the loose
        // files beside it and nothing else. Pushing every archive to the front
        // would put a game's own archive above an overlay a caller mounted on
        // top of the game — which is how `install` reads its patch back.
        for source in vfs.sources() {
            let files = source.list();
            for (path, size) in files {
                if path.extension() != Some("bsa") {
                    continue;
                }
                let header = source.read_range(&path, 0, 16)?;
                let records =
                    match index_bytes(|p, o, l| source.read_range(p, o, l), &path, &header, size) {
                        Ok(records) => records,
                        Err(e) => {
                            info.note(format!("skipping '{path}': {e}"));
                            continue;
                        }
                    };
                let archive = match BsarcArchive::from_parts(&header, &records, path.clone(), size)
                {
                    Ok(archive) => archive,
                    Err(e) => {
                        info.note(format!("skipping '{path}': {e}"));
                        continue;
                    }
                };
                // The archive's own name, without its extension: `Graphics.bsa`
                // serves as `graphics/`, which keeps two archives that both hold
                // `b01a.bsg` from shadowing each other.
                let prefix = match path.file_name().rsplit_once('.') {
                    Some((stem, _)) if !stem.is_empty() => stem.to_ascii_lowercase(),
                    _ => String::from("archive"),
                };
                info.note(format!(
                    "mounted '{path}': {} entries, {} bytes of data, under '{prefix}/'",
                    archive.len(),
                    archive.data_bytes()
                ));
                entries += archive.len();
                count += 1;
                mounted.push(Arc::new(crate::bsarc::BsarcSource::new(
                    Arc::new(archive),
                    source.clone(),
                    Some(prefix),
                )));
            }
            mounted.push(source.clone());
        }

        if count == 0 {
            return Err(Error::unsupported(
                "BSX",
                "no BSArc archives found, and this seam mounts its games through their archives",
            ));
        }
        info.note(format!(
            "{count} archive(s), {entries} file(s) visible in total"
        ));
        Ok(Box::new(BsxMount { info, vfs: mounted }))
    }
}

/// The bytes of an archive's index, given its header.
fn index_bytes<F>(read: F, path: &VirtualPath, header: &[u8], size: u64) -> Result<Vec<u8>>
where
    F: Fn(&VirtualPath, u64, usize) -> Result<Vec<u8>>,
{
    if header.len() < 16 {
        return Err(Error::corrupt(
            path.to_string(),
            format!(
                "an archive header is 16 bytes, only {} came back",
                header.len()
            ),
        ));
    }
    let count = u16::from_le_bytes([header[10], header[11]]) as u64;
    let offset = u32::from_le_bytes([header[12], header[13], header[14], header[15]]) as u64;
    let length = count * (crate::bsarc::RECORD_SIZE as u64);
    if length == 0 {
        return Ok(Vec::new());
    }
    let end = offset.checked_add(length).ok_or_else(|| {
        Error::corrupt(path.to_string(), "the index length overflows".to_string())
    })?;
    if end > size {
        return Err(Error::corrupt(
            path.to_string(),
            format!(
                "{count} records at {offset} would end at {end}, past the end of a {size} byte file"
            ),
        ));
    }
    read(path, offset, length as usize)
}

/// A mounted BSX game.
pub struct BsxMount {
    info: MountInfo,
    vfs: Vfs,
}

impl EngineMount for BsxMount {
    fn info(&self) -> &MountInfo {
        &self.info
    }

    fn vfs(&self) -> &Vfs {
        &self.vfs
    }

    fn read_image(&self, path: &VirtualPath) -> Result<Image> {
        let bytes = self.vfs.read(path)?;
        if image::is_bsg(&bytes) {
            return image::decode_bsg(&bytes, path.as_str());
        }
        if bytes.starts_with(b"BM") {
            return kintsugi_core::codec::bmp::decode_bmp(&bytes)
                .map_err(|e| Error::corrupt(path.to_string(), format!("BMP: {e}")));
        }
        Err(Error::unsupported(
            "BSX",
            format!(
                "'{path}' is neither a BSG image ({} bytes) nor a BMP",
                bytes.len()
            ),
        ))
    }

    fn read_audio(&self, path: &VirtualPath) -> Result<Audio> {
        let bytes = self.vfs.read(path)?;
        let audio = Audio::new(bytes);
        if audio.codec == kintsugi_core::asset::AudioCodec::Unknown {
            return Err(Error::unsupported(
                "BSX",
                format!("'{path}' is not an Ogg or WAVE container this seam recognizes"),
            ));
        }
        Ok(audio)
    }

    fn image_extensions(&self) -> &'static [&'static str] {
        &["bsg", "bmp"]
    }
}
