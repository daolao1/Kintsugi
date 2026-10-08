//! The BSX seam: detection, mounting and asset decoding.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use kintsugi_core::asset::{Audio, Image};
use kintsugi_core::detect::{Confidence, Detection};
use kintsugi_core::error::{Error, Result};
use kintsugi_core::plugin::{EngineMount, EnginePlugin, MountInfo, PluginMetadata, WrittenScript};
use kintsugi_core::script::{Command, Script};
use kintsugi_core::vfs::{Vfs, VirtualPath};

use crate::bsarc::BsarcArchive;
use crate::image;
use crate::script::{SCRIPT_MAGIC, Story};

/// Registry id of this seam.
///
/// The game names its own system: `bsx.ini` sits beside `bsx.dat`, and the
/// first line of that configuration is the game and the studio, `ONI@BLUEGALE`.
/// "BSX" is therefore what the engine calls itself, which is the name a repair
/// should carry.
pub const ENGINE_ID: &str = "bsx";

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
        // shared with anyone. The magic is the evidence here, not the file
        // name — a `.dat` proves nothing and a story named something else is
        // still a story — so every file's first bytes are read.
        let mut scripts = Vec::new();
        let mut lines = 0usize;
        for (path, size) in vfs.list() {
            if size < SCRIPT_MAGIC.len() as u64 {
                continue;
            }
            let Ok(head) = vfs.read_range(&path, 0, SCRIPT_MAGIC.len()) else {
                continue;
            };
            if head != SCRIPT_MAGIC {
                continue;
            }
            // Readable is not the same as understood: only a file whose table
            // of lines parses counts as a story, and only then does this seam
            // say it knows which file a game is played through.
            if let Ok(bytes) = vfs.read(&path) {
                if let Ok(story) = Story::parse(&bytes) {
                    lines += story.strings().len();
                    scripts.push(path);
                    continue;
                }
            }
            scripts.push(path);
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
            let story = if lines > 0 {
                format!("holding {lines} line(s) of story")
            } else {
                String::from("whose stories this seam cannot read")
            };
            verdicts.push(Detection::new(
                ENGINE_ID,
                Confidence::Certain,
                format!(
                    "{} BSArc archive(s) with valid indexes ({archive_entries} entries) and {} \
                     BSScript file(s) {story}",
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

        // The story the game is played through, found by its own magic rather
        // than by a file name, and only counted once it parses: `kintsugi
        // translate` must not be handed a `.dat` that this seam cannot read.
        let mut stories = Vec::new();
        let mut stories_seen = 0usize;
        for (path, size) in mounted.list() {
            if size < SCRIPT_MAGIC.len() as u64 {
                continue;
            }
            let Ok(head) = mounted.read_range(&path, 0, SCRIPT_MAGIC.len()) else {
                continue;
            };
            if head != SCRIPT_MAGIC {
                continue;
            }
            stories_seen += 1;
            match mounted
                .read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    Story::parse(&bytes)
                        .map(|story| story.strings().len())
                        .map_err(|e| e.to_string())
                }) {
                Ok(strings) => {
                    info.note(format!("story '{path}': {strings} line(s)"));
                    stories.push(path);
                }
                Err(why) => info.note(format!(
                    "'{path}' carries the BSScript magic and is not a story this seam can read, so \
                     it is mounted as a file and never handed over as a script: {why}"
                )),
            }
        }

        // A game of this engine may be archives, or loose files, or both: the
        // release this seam was written for keeps its story and its title
        // picture loose beside six archives. So the mount succeeds on any of
        // this engine's evidence and refuses a folder with none of it — an
        // empty folder is nobody's game, and neither is one that merely has
        // files in it.
        let loose_images = mounted
            .list()
            .into_iter()
            .filter(|(path, size)| {
                path.extension() == Some("bsg")
                    && *size >= image::GRAPHICS_MAGIC.len() as u64
                    && mounted
                        .read_range(path, 0, image::GRAPHICS_MAGIC.len())
                        .is_ok_and(|head| {
                            head == image::GRAPHICS_MAGIC || head == image::COMPOSITION_MAGIC
                        })
            })
            .count();
        if count == 0 && stories_seen == 0 && loose_images == 0 {
            return Err(Error::unsupported(
                "BSX",
                "no BSArc archive, no BSScript file and no loose BSG picture found, and this seam \
                 mounts a game through what its engine left in it",
            ));
        }
        info.note(format!(
            "{count} archive(s), {entries} file(s) visible in total"
        ));
        Ok(Box::new(BsxMount {
            info,
            vfs: mounted,
            stories,
        }))
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
    /// Every file that begins with the story magic **and** parses as a story,
    /// in the order the sources present them.
    stories: Vec<VirtualPath>,
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

    fn read_script(&self, path: &VirtualPath) -> Result<Script> {
        let bytes = self.vfs.read(path)?;
        let story = Story::parse(&bytes).map_err(|refused| Error::Script {
            context: path.to_string(),
            detail: refused.to_string(),
        })?;

        // The code decides what a line is shown as and in what order. Reading
        // the table in index order would be reading the dictionary rather than
        // the book, which is what this seam used to do and said so about.
        let plan = command_plan(&story);
        let shows = story.shows();
        let mut narration = 0usize;
        let mut dialogue = 0usize;
        let mut script = Script::new(path.clone());
        for (channel, line) in &plan {
            let text = story.strings()[*line].clone();
            match channel {
                Some(0) => {
                    narration += 1;
                    script.commands.push(Command::Narration(text));
                }
                Some(_) => {
                    dialogue += 1;
                    script.commands.push(Command::Dialogue {
                        speaker: None,
                        text,
                    });
                }
                // Shown by nothing the code says: the words are real, the kind
                // is not known, and the body has a variant for exactly that.
                None => script.commands.push(Command::RawLine(text)),
            }
        }

        let runs = shows
            .windows(2)
            .filter(|pair| pair[1].line != pair[0].line + 1)
            .count()
            + usize::from(!shows.is_empty());
        script.warnings.push(format!(
            "{} line(s) of story in the order the code shows them: {narration} on the narration \
             channel and {dialogue} on a dialogue channel, from {} show instruction(s) in {runs} \
             run(s) of consecutive lines. The channel says which text box the game draws, not who \
             is speaking, so no line is given a speaker. A branch is not decoded: the runs are \
             walked one after the other, so this is every line in the order the story can reach \
             it, not one playthrough.{}",
            story.strings().len(),
            shows.len(),
            match command_plan(&story).len() - shows.len() {
                0 => String::new(),
                kept => format!(
                    " {kept} line(s) the code never shows are kept at the end as raw lines rather \
                     than dropped."
                ),
            }
        ));
        Ok(script)
    }

    fn primary_script(&self) -> Result<VirtualPath> {
        // The magic is the evidence, so a folder with one readable story names
        // it without a guess. Two of them is a question for the person holding
        // the game: this seam will not play one and repair the other.
        match self.stories.as_slice() {
            [only] => Ok(only.clone()),
            [] => Err(Error::unsupported(
                "BSX",
                "no BSScript story parses in this game, so this seam cannot name a main script",
            )),
            several => Err(Error::unsupported(
                "BSX",
                format!(
                    "{} BSScript stories parse in this game ({}); name the one to read or repair \
                     with --script, because this seam will not choose between them",
                    several.len(),
                    several
                        .iter()
                        .map(|path| format!("'{path}'"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
        }
    }

    fn write_script(
        &self,
        path: &VirtualPath,
        replacements: &BTreeMap<usize, String>,
    ) -> Result<WrittenScript> {
        let bytes = self.vfs.read(path)?;
        let story = Story::parse(&bytes).map_err(|refused| Error::Script {
            context: path.to_string(),
            detail: refused.to_string(),
        })?;

        // The ids a translation carries are the positions of the commands
        // `read_script` handed over, so the same walk of the code turns an id
        // back into the line a repair can move. A story is read the same way
        // twice, which is what makes a repair land on the words it names.
        let plan = command_plan(&story);
        let mut wanted: BTreeMap<usize, String> = BTreeMap::new();
        let mut unmatched = Vec::new();
        let mut clashes = Vec::new();
        for (id, text) in replacements {
            let Some((_, line)) = plan.get(*id).copied() else {
                unmatched.push(*id);
                continue;
            };
            match wanted.get(&line) {
                Some(existing) if existing != text => clashes.push((*id, line)),
                _ => {
                    wanted.insert(line, text.clone());
                }
            }
        }
        if !clashes.is_empty() {
            // One line shown in two scenes is one string in the file. A
            // translation that gives it two different readings cannot be
            // honoured in both places, and picking one would silently lose the
            // other.
            return Err(Error::Script {
                context: format!("'{path}'"),
                detail: format!(
                    "the same line is shown more than once and the translation gives it different \
                     words at different places ({}). This seam repairs the line itself, so both \
                     places would change together; it refuses rather than pick one of them",
                    clashes
                        .iter()
                        .map(|(id, line)| format!("command {id} at line {line}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
        let repair = story.rebuild(&wanted).map_err(|refused| Error::Script {
            context: format!("'{path}'"),
            detail: refused.to_string(),
        })?;

        // A story is a file of its own here, so the repair is one file: the
        // index rewritten in place and the text block after it, with every
        // other byte of the game's script untouched.
        // Ids the command list does not have, and lines the story does not
        // have, are the same complaint to whoever wrote the translation.
        let mut missing = unmatched;
        missing.extend(repair.unmatched.iter().copied());
        missing.sort_unstable();
        missing.dedup();
        Ok(WrittenScript::loose(
            path.clone(),
            repair.bytes,
            repair.replaced,
            missing,
        ))
    }
}

/// The commands [`BsxPlugin::read_script`] hands over, in order: every line the
/// code shows, then the lines it never shows.
///
/// This is the one place that decides what a command id means. Reading and
/// repairing both go through it, so an id cannot drift between the translation
/// that names it and the repair that has to land on it.
fn command_plan(story: &Story) -> Vec<(Option<u8>, usize)> {
    let shows = story.shows();
    let shown: BTreeSet<usize> = shows.iter().map(|show| show.line).collect();
    let mut plan: Vec<(Option<u8>, usize)> = shows
        .iter()
        .map(|show| (Some(show.channel), show.line))
        .collect();
    for line in 0..story.strings().len() {
        if !shown.contains(&line) {
            plan.push((None, line));
        }
    }
    plan
}
