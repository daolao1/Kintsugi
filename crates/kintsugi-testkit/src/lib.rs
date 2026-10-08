//! 🏺 kintsugi-testkit — the seam contract, as a function a new engine calls.
//!
//! The body promises that a new engine is a new crate and nothing else. That
//! promise is only as good as the contract being *checkable*: an engine author
//! should not have to infer the rules from `ARCHITECTURE.md` and hope. So the
//! rules that are true of every seam, whatever its file format, live here and
//! run in the seam's own test suite:
//!
//! 1. **A name is never evidence.** A seam may reach `Possible` from an
//!    extension alone, but `Likely` and `Certain` must come from structure
//!    ([`assert_seam_contract`] feeds the seam a junk file with each of its own
//!    claimed extensions and insists it does not get excited).
//! 2. **`Certain` means mountable.** A verdict at `Certain` is a promise that
//!    the seam can mount those files, and the promise is checked, not assumed.
//! 3. **A seam names itself.** Every verdict and every mount carries the id
//!    from `metadata()`. A copy-pasted seam that reports another engine's id
//!    would mislabel a repair, which is the one thing this project cannot do.
//! 4. **An empty folder is nobody's game.** No seam claims `Possible` or
//!    better on nothing at all.
//! 5. **Changing nothing changes nothing.** `write_script` with no replacements
//!    either refuses (the default, honest) or returns the original file
//!    byte-for-byte. A writer that reformats a file nobody asked it to touch is
//!    the failure mode `ARCHITECTURE.md` §2 exists to prevent.
//!
//! None of this knows anything about any engine's format, which is the point:
//! it is the part of "add an engine" that is the same every time. See
//! `docs/ADDING-AN-ENGINE.md` for the walkthrough, and
//! `crates/kintsugi-bluegale/tests/conformance.rs` for a seam passing it.

use std::sync::Arc;

use kintsugi_core::detect::Confidence;
use kintsugi_core::plugin::EnginePlugin;
use kintsugi_core::vfs::{MemorySource, Vfs};

// The disc-image fixture: a game handed over as an `.iso` needs a real volume
// to mount, and the repository holds no game data, so it is written in code.
mod iso;
pub use iso::make_iso;

/// One example of the format this seam exists for, built in memory.
///
/// The fixture is deliberately not a directory: it travels with the seam's
/// tests, needs no cleanup, and cannot accidentally be someone's installed game.
pub struct SeamFixture {
    /// What to call this example in a failure message.
    pub name: &'static str,
    files: Vec<(&'static str, Vec<u8>)>,
    confidence: Confidence,
    script: Option<&'static str>,
}

impl SeamFixture {
    /// A fixture that the seam must recognize at `confidence`.
    pub fn new(name: &'static str, confidence: Confidence) -> Self {
        Self {
            name,
            files: Vec::new(),
            confidence,
            script: None,
        }
    }

    /// Add a file to the fixture, at a virtual path inside it.
    pub fn file(mut self, path: &'static str, bytes: impl Into<Vec<u8>>) -> Self {
        self.files.push((path, bytes.into()));
        self
    }

    /// Declare that this fixture contains a script the seam must be able to
    /// read into the body's IR.
    pub fn script(mut self, path: &'static str) -> Self {
        self.script = Some(path);
        self
    }

    fn vfs(&self) -> Vfs {
        let mut source = MemorySource::new();
        for (path, bytes) in &self.files {
            source.insert(path, bytes.clone());
        }
        let mut vfs = Vfs::new();
        vfs.push_front(Arc::new(source));
        vfs
    }
}

/// Check every rule in this module's list against `plugin`.
///
/// Panics with a message that says which rule broke and what to do about it —
/// this is meant to be read by whoever is writing the next seam.
pub fn assert_seam_contract(plugin: &dyn EnginePlugin, fixtures: &[SeamFixture]) {
    assert_metadata_is_usable(plugin);
    assert_empty_folder_is_nobodys_game(plugin);
    assert_a_name_is_never_evidence(plugin);
    for fixture in fixtures {
        assert_fixture_is_recognized(plugin, fixture);
    }
}

fn assert_metadata_is_usable(plugin: &dyn EnginePlugin) {
    let metadata = plugin.metadata();
    assert!(
        !metadata.id.is_empty()
            && metadata
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '-' || c == '_' || c.is_ascii_digit()),
        "seam id '{}' must be a lowercase identifier: it is what a user sees attached to a \
         repair, and what `--script` refuses to guess at",
        metadata.id
    );
    assert!(
        !metadata.display_name.is_empty(),
        "seam '{}' has no display name",
        metadata.id
    );
    assert!(
        !metadata.version.is_empty(),
        "seam '{}' reports no version; a repair has to be able to say what made it",
        metadata.id
    );
    assert!(
        !metadata.file_extensions.is_empty(),
        "seam '{}' claims no file extensions, so nothing will ever be offered to it",
        metadata.id
    );
    for extension in metadata.file_extensions {
        assert!(
            !extension.starts_with('.') && extension == &extension.to_ascii_lowercase(),
            "seam '{}' lists extension '{extension}': list bare, lower-case extensions \
             (\"bdt\", not \".BDT\"), because that is what `VirtualPath::extension` returns",
            metadata.id
        );
    }
}

/// Rule 3, in one place: a verdict carries the id from `metadata()`.
///
/// Checked everywhere a verdict appears, because the id is what the host prints
/// next to a repair — a seam reporting another engine's id would attach the
/// wrong name to someone's fixed game.
fn assert_verdict_names_itself(plugin_id: &str, verdict: &kintsugi_core::detect::Detection) {
    assert_eq!(
        verdict.engine, plugin_id,
        "seam '{plugin_id}' returned a verdict naming '{}'",
        verdict.engine
    );
}

fn assert_empty_folder_is_nobodys_game(plugin: &dyn EnginePlugin) {
    let verdicts = plugin
        .detect(&Vfs::new())
        .expect("a seam must be able to look at an empty folder without failing");
    for verdict in &verdicts {
        assert!(
            verdict.confidence < Confidence::Possible,
            "seam '{}' claims {:?} for an empty folder; the ladder in ARCHITECTURE.md starts \
             at a file name and a shape, and an empty folder has neither",
            plugin.metadata().id,
            verdict.confidence
        );
        assert_verdict_names_itself(plugin.metadata().id, verdict);
    }
}

fn assert_a_name_is_never_evidence(plugin: &dyn EnginePlugin) {
    for extension in plugin.metadata().file_extensions {
        let path = format!("sample.{extension}");
        let mut source = MemorySource::new();
        // Junk that is not empty and not a header: the shape a seam must refuse
        // to read as structure.
        source.insert(
            &path,
            b"this is not a game file, it is a text file".to_vec(),
        );
        let mut vfs = Vfs::new();
        vfs.push_front(Arc::new(source));
        let verdicts = plugin
            .detect(&vfs)
            .unwrap_or_else(|e| panic!("detect failed on a file named '{path}': {e}"));
        for verdict in &verdicts {
            assert_verdict_names_itself(plugin.metadata().id, verdict);
            assert!(
                verdict.confidence < Confidence::Likely,
                "seam '{}' claims {:?} for '{path}', whose contents are text: an extension is \
                 never evidence for Likely or Certain (ARCHITECTURE.md, \"Detection: an \
                 extension is never evidence\")",
                plugin.metadata().id,
                verdict.confidence
            );
        }
    }
}

fn assert_fixture_is_recognized(plugin: &dyn EnginePlugin, fixture: &SeamFixture) {
    let id = plugin.metadata().id;
    let vfs = fixture.vfs();
    let verdicts = plugin.detect(&vfs).unwrap_or_else(|e| {
        panic!(
            "seam '{id}' failed to detect fixture '{}': {e}",
            fixture.name
        )
    });
    for verdict in &verdicts {
        assert_verdict_names_itself(id, verdict);
    }
    let verdict = verdicts
        .iter()
        .find(|verdict| verdict.engine == id)
        .unwrap_or_else(|| {
            panic!(
                "seam '{id}' does not recognize its own fixture '{}'; a seam that cannot \
             recognize the format it was written for will not recognize a real game either",
                fixture.name
            )
        });
    assert_eq!(
        verdict.confidence, fixture.confidence,
        "seam '{id}' reports {:?} for its own fixture '{}', expected {:?}",
        verdict.confidence, fixture.name, fixture.confidence
    );

    // Rule 2: Certain is a promise that the files can be mounted.
    let mount = plugin.mount(&vfs).unwrap_or_else(|e| {
        panic!(
            "seam '{id}' reports {:?} for '{}' and then cannot mount it: {e}",
            fixture.confidence, fixture.name
        )
    });
    assert_eq!(
        mount.info().engine,
        id,
        "seam '{id}' mounted '{}' as '{}'",
        fixture.name,
        mount.info().engine
    );
    assert!(
        !mount.vfs().list().is_empty(),
        "seam '{id}' mounted '{}' into an empty view",
        fixture.name
    );

    for extension in mount.image_extensions() {
        assert!(
            !extension.starts_with('.') && extension == &extension.to_ascii_lowercase(),
            "seam '{id}' lists image extension '{extension}': list bare, lower-case \
             extensions, because that is the form a path is compared in"
        );
    }

    // Rule 5: a seam that names the game's script must be able to hand it over.
    // Naming one the mount does not have, or naming a different file than the
    // one script a fixture declares, is how a command repairs the wrong script
    // and reports the name it chose afterwards.
    if let Ok(named) = mount.primary_script() {
        assert!(
            mount.vfs().exists(&named),
            "seam '{id}' names '{named}' as the game's script, but the mount of its own \
             fixture '{}' has no such file: a name the seam cannot follow through on \
             sends `translate` and `install` at nothing",
            fixture.name
        );
        mount.read_script(&named).unwrap_or_else(|e| {
            panic!(
                "seam '{id}' names '{named}' as the game's script and cannot read it out \
                 of its own mount: {e}"
            )
        });
        if let Some(script_path) = fixture.script {
            let declared = kintsugi_core::vfs::VirtualPath::new(script_path);
            assert_eq!(
                named, declared,
                "seam '{id}' names '{named}' as the game's script, but '{}' is the only \
                 script in this fixture: a game with one script has one main script, and \
                 choosing another one is a repair aimed at the wrong file",
                fixture.name
            );
        }
    }
    // Refusing is a supported answer — see `EngineMount::primary_script`.

    if let Some(script_path) = fixture.script {
        let path = kintsugi_core::vfs::VirtualPath::new(script_path);
        let script = mount.read_script(&path).unwrap_or_else(|e| {
            panic!(
                "seam '{id}' cannot read the script '{script_path}' of its own fixture '{}': {e}",
                fixture.name
            )
        });
        assert!(
            !script.commands.is_empty(),
            "seam '{id}' read '{script_path}' into an empty script",
        );

        // Rule 6: changing nothing must change nothing.
        let original_script_bytes = mount.vfs().read(&path).unwrap_or_else(|e| {
            panic!("seam '{id}' read '{script_path}' but the mount has no such file: {e}")
        });
        let untouched = std::collections::BTreeMap::new();
        match mount.write_script(&path, &untouched) {
            Ok(written) => {
                // Every file the seam says it changed has to come back
                // unchanged. Compare against what the *mount* reads, not against
                // the raw fixture: a seam is allowed to shadow the source it was
                // handed (an archive in front of a loose file — BlueGale does
                // exactly that), and a writer is only ever given the mounted
                // view. A script packed in an archive exercises this properly:
                // its repair touches the blob *and* the index, and neither may
                // differ when nothing was translated.
                assert_eq!(
                    written.script, original_script_bytes,
                    "seam '{id}' rewrote '{script_path}' with no replacements to make: the \
                     script's own bytes must come back unchanged (this is what a patch file \
                     holds)"
                );
                assert!(
                    !written.files.is_empty(),
                    "seam '{id}' wrote '{script_path}' with no replacements and reported no \
                     files: a repair that changed nothing is a refusal, not an empty success"
                );
                for file in &written.files {
                    let original = mount.vfs().read(&file.path).unwrap_or_else(|e| {
                        panic!(
                            "seam '{id}' says it wrote '{}' for '{script_path}', but the mount \
                             has no such file: {e}",
                            file.path
                        )
                    });
                    assert_eq!(
                        file.data, original,
                        "seam '{id}' rewrote '{}' with no replacements to make: `write_script` \
                         must copy every byte it was not asked to change (ARCHITECTURE.md §2). \
                         The bytes differ even though nothing was translated.",
                        file.path
                    );
                }
                assert_eq!(
                    written.replaced, 0,
                    "seam '{id}' reports replacements for an empty replacement map"
                );
            }
            Err(e) => {
                // Honest refusal: this seam does not write scripts yet, which is
                // a supported answer and must be the *only* other one.
                let message = format!("{e}");
                assert!(
                    message.contains("write") || message.contains("script"),
                    "seam '{id}' refused to write '{script_path}' with a message that does \
                     not say what it cannot do: {message}"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kintsugi_core::detect::Detection;
    use kintsugi_core::error::{Error, Result};
    use kintsugi_core::plugin::{EngineMount, MountInfo, PluginMetadata, WrittenScript};
    use kintsugi_core::script::{Command, Script};
    use kintsugi_core::vfs::VirtualPath;
    use std::collections::BTreeMap;

    /// A seam for a toy format: `*.toy` files that start with `TOY1`.
    ///
    /// Every flaw below is a rule from this module's list, broken on purpose —
    /// because a contract checker that cannot fail is decoration. Each one has
    /// a `#[should_panic]` test that names the message it must produce.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Flaw {
        None,
        /// `Certain` from the file name alone.
        EvidenceFromName,
        /// Excited about an empty folder.
        PossibleOnNothing,
        /// Reports another engine's id.
        ForeignVerdict,
        /// Says `Certain` and then cannot mount.
        CertainButUnmountable,
        /// Lists `.toy` instead of `toy`.
        DottedExtension,
        /// Rewrites a file it was asked to leave alone.
        ReformatsWhenUntouched,
        /// Does not recognize its own format.
        UnrecognizedFixture,
        /// Names a script its own game does not have.
        PrimaryScriptMissing,
        /// Names some script other than the game's only one.
        PrimaryScriptIsAnother,
    }

    struct ToyPlugin(Flaw);

    fn metadata_for(flaw: Flaw) -> &'static PluginMetadata {
        let extension = if flaw == Flaw::DottedExtension {
            ".toy"
        } else {
            "toy"
        };
        Box::leak(Box::new(PluginMetadata {
            id: "toy",
            crate_name: "toy-seam",
            display_name: "Toy",
            version: "1",
            file_extensions: Box::leak(vec![extension].into_boxed_slice()),
        }))
    }

    struct ToyMount {
        info: MountInfo,
        vfs: Vfs,
        flaw: Flaw,
    }

    impl EngineMount for ToyMount {
        fn info(&self) -> &MountInfo {
            &self.info
        }

        fn vfs(&self) -> &Vfs {
            &self.vfs
        }

        fn primary_script(&self) -> Result<VirtualPath> {
            match self.flaw {
                Flaw::PrimaryScriptMissing => Ok(VirtualPath::new("nowhere.toy")),
                Flaw::PrimaryScriptIsAnother => Ok(VirtualPath::new("extra.toy")),
                _ => Ok(VirtualPath::new("story.toy")),
            }
        }

        fn read_script(&self, path: &VirtualPath) -> Result<Script> {
            let bytes = self.vfs.read(path)?;
            let text = String::from_utf8_lossy(&bytes[4..]).into_owned();
            let mut script = Script::new(path.clone());
            script.commands = text
                .lines()
                .map(|line| Command::RawLine(line.to_string()))
                .collect();
            Ok(script)
        }

        fn write_script(
            &self,
            path: &VirtualPath,
            replacements: &BTreeMap<usize, String>,
        ) -> Result<WrittenScript> {
            let bytes = self.vfs.read(path)?;
            if self.flaw == Flaw::ReformatsWhenUntouched {
                // The failure mode §2 exists to prevent: the writer decodes and
                // re-encodes, so a file nobody asked to change comes back
                // different (here: without its trailing newline).
                let text = String::from_utf8_lossy(&bytes[4..]).into_owned();
                return Ok(WrittenScript::loose(
                    path.clone(),
                    format!("TOY1{}", text.trim_end_matches('\n')).into_bytes(),
                    0,
                    Vec::new(),
                ));
            }
            let text = String::from_utf8_lossy(&bytes[4..]).into_owned();
            let mut out = String::from("TOY1");
            let mut replaced = 0;
            for (index, line) in text.split_inclusive('\n').enumerate() {
                let body = line.trim_end_matches('\n');
                match replacements.get(&index) {
                    Some(new) if new != body => {
                        replaced += 1;
                        out.push_str(new);
                    }
                    _ => out.push_str(body),
                }
                if line.ends_with('\n') {
                    out.push('\n');
                }
            }
            Ok(WrittenScript::loose(
                path.clone(),
                out.into_bytes(),
                replaced,
                Vec::new(),
            ))
        }
    }

    impl EnginePlugin for ToyPlugin {
        fn metadata(&self) -> &'static PluginMetadata {
            metadata_for(self.0)
        }

        fn detect(&self, vfs: &Vfs) -> Result<Vec<Detection>> {
            if self.0 == Flaw::PossibleOnNothing {
                return Ok(vec![Detection::new(
                    "toy",
                    Confidence::Possible,
                    "looking at nothing",
                )]);
            }
            let mut verdicts = Vec::new();
            for path in vfs.find_by_extension(self.metadata().file_extensions) {
                let bytes = vfs.read(&path).unwrap_or_default();
                let signed = bytes.starts_with(b"TOY1");
                if self.0 == Flaw::UnrecognizedFixture {
                    continue;
                }
                let confidence = if self.0 == Flaw::EvidenceFromName || signed {
                    Confidence::Certain
                } else {
                    Confidence::Possible
                };
                let engine = if self.0 == Flaw::ForeignVerdict {
                    "someone-else"
                } else {
                    "toy"
                };
                verdicts.push(Detection::new(engine, confidence, "toy signature"));
            }
            Ok(verdicts)
        }

        fn mount(&self, vfs: &Vfs) -> Result<Box<dyn EngineMount>> {
            if self.0 == Flaw::CertainButUnmountable {
                return Err(Error::unsupported("toy", "cannot mount after all"));
            }
            Ok(Box::new(ToyMount {
                info: MountInfo::new("toy"),
                vfs: vfs.clone(),
                flaw: self.0,
            }))
        }
    }

    fn toy_fixture() -> SeamFixture {
        SeamFixture::new("a tiny toy game", Confidence::Certain)
            .file("story.toy", b"TOY1hello\nworld\n".to_vec())
            .script("story.toy")
    }

    #[test]
    fn a_conforming_seam_passes() {
        assert_seam_contract(&ToyPlugin(Flaw::None), &[toy_fixture()]);
    }

    #[test]
    fn a_seam_that_names_a_script_it_does_not_have_is_caught() {
        let failure = std::panic::catch_unwind(|| {
            assert_seam_contract(&ToyPlugin(Flaw::PrimaryScriptMissing), &[toy_fixture()]);
        })
        .unwrap_err();
        let message = panic_message(&failure);
        assert!(
            message.contains("nowhere.toy") && message.contains("has no such file"),
            "{message}"
        );
    }

    #[test]
    fn a_seam_that_names_another_script_is_caught() {
        // Two scripts on disk, one of them declared: naming the other one is a
        // repair aimed at the wrong file.
        let fixture = toy_fixture().file("extra.toy", b"TOY1not the one\n".to_vec());
        let failure = std::panic::catch_unwind(|| {
            assert_seam_contract(&ToyPlugin(Flaw::PrimaryScriptIsAnother), &[fixture]);
        })
        .unwrap_err();
        let message = panic_message(&failure);
        assert!(
            message.contains("extra.toy") && message.contains("one main script"),
            "{message}"
        );
    }

    #[test]
    fn a_seam_that_claims_certain_from_a_name_is_caught() {
        let failure = std::panic::catch_unwind(|| {
            assert_seam_contract(&ToyPlugin(Flaw::EvidenceFromName), &[toy_fixture()]);
        })
        .unwrap_err();
        assert!(
            panic_message(&failure).contains("an extension is never evidence"),
            "{}",
            panic_message(&failure)
        );
    }

    #[test]
    fn a_seam_that_claims_a_folder_of_nothing_is_caught() {
        let failure = std::panic::catch_unwind(|| {
            assert_seam_contract(&ToyPlugin(Flaw::PossibleOnNothing), &[toy_fixture()]);
        })
        .unwrap_err();
        assert!(
            panic_message(&failure).contains("for an empty folder"),
            "{}",
            panic_message(&failure)
        );
    }

    #[test]
    fn a_seam_that_names_another_engine_is_caught() {
        let failure = std::panic::catch_unwind(|| {
            assert_seam_contract(&ToyPlugin(Flaw::ForeignVerdict), &[toy_fixture()]);
        })
        .unwrap_err();
        assert!(
            panic_message(&failure).contains("returned a verdict naming 'someone-else'"),
            "{}",
            panic_message(&failure)
        );
    }

    #[test]
    fn a_seam_that_cannot_mount_what_it_was_certain_about_is_caught() {
        let failure = std::panic::catch_unwind(|| {
            assert_seam_contract(&ToyPlugin(Flaw::CertainButUnmountable), &[toy_fixture()]);
        })
        .unwrap_err();
        assert!(
            panic_message(&failure).contains("cannot mount it"),
            "{}",
            panic_message(&failure)
        );
    }

    #[test]
    fn a_seam_that_lists_a_dotted_extension_is_caught() {
        let failure = std::panic::catch_unwind(|| {
            assert_seam_contract(&ToyPlugin(Flaw::DottedExtension), &[toy_fixture()]);
        })
        .unwrap_err();
        assert!(
            panic_message(&failure).contains("list bare, lower-case extensions"),
            "{}",
            panic_message(&failure)
        );
    }

    #[test]
    fn a_writer_that_reformats_an_untouched_file_is_caught() {
        let failure = std::panic::catch_unwind(|| {
            assert_seam_contract(&ToyPlugin(Flaw::ReformatsWhenUntouched), &[toy_fixture()]);
        })
        .unwrap_err();
        // Either message is the same defect caught at either end of the repair:
        // the script's own bytes, or a file that carries them.
        let message = panic_message(&failure);
        assert!(
            message.contains("the script's own bytes must come back unchanged")
                || message.contains("must copy every byte it was not asked to change"),
            "{message}"
        );
    }

    #[test]
    fn a_seam_that_does_not_recognize_its_own_format_is_caught() {
        let failure = std::panic::catch_unwind(|| {
            assert_seam_contract(&ToyPlugin(Flaw::UnrecognizedFixture), &[toy_fixture()]);
        })
        .unwrap_err();
        assert!(
            panic_message(&failure).contains("does not recognize its own fixture"),
            "{}",
            panic_message(&failure)
        );
    }

    fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
        if let Some(text) = payload.downcast_ref::<String>() {
            return text.clone();
        }
        if let Some(text) = payload.downcast_ref::<&str>() {
            return (*text).to_string();
        }
        String::from("<panic without a message>")
    }
}
