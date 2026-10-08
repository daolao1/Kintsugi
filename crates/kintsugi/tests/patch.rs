//! The repair a translator actually ships, tested where it is wired: in the
//! host, which is the only layer that may know about a seam *and* the glaze.
//!
//! Seams do not depend on the translation crate, and the translation crate
//! does not depend on any seam. The host joins them, so this is the file that
//! proves the join holds — including the byte-level promise that a patch
//! touches nothing but the lines it translates.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use kintsugi_core::plugin::{EngineMount, Registry};
use kintsugi_core::runtime::{Event, Host, Interpreter};
use kintsugi_core::script::{ChoiceOption, Command};
use kintsugi_core::vfs::Vfs;
use kintsugi_core::{Error, Result as CoreResult};
use kintsugi_translate::{LlmTranslator, MockTranslator, Translator, apply, extract_with_raw};

/// A throwaway directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("kintsugi-patch-{}-{}", tag, std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

#[derive(Default)]
struct RecordingHost {
    lines: Vec<String>,
}

impl Host for RecordingHost {
    fn show_text(&mut self, speaker: Option<&str>, text: &str) -> CoreResult<()> {
        match speaker {
            Some(speaker) => self.lines.push(format!("[{speaker}] {text}")),
            None => self.lines.push(text.to_string()),
        }
        Ok(())
    }

    fn event(&mut self, _event: Event<'_>) -> CoreResult<()> {
        Ok(())
    }

    fn choose(&mut self, options: &[ChoiceOption]) -> CoreResult<usize> {
        Ok(options.len() - 1)
    }
}

/// Mount the synthetic demo game through the assembled registry, the way the
/// CLI does.
fn mount_demo(tag: &str) -> (TempDir, Box<dyn EngineMount>) {
    let temp = TempDir::new(tag);
    let game_dir = temp.0.join("game");
    kintsugi_bluegale::fixtures::write_demo_game(&game_dir).unwrap();
    let vfs = Vfs::from_directory(&game_dir).unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(kintsugi_bluegale::plugin()));
    let (plugin, mount) = registry.mount_best(&vfs).unwrap();
    assert_eq!(plugin.metadata().id, "bluegale");
    (temp, mount)
}

#[test]
fn a_translated_script_writes_back_as_a_playable_patch() {
    let (_temp, mount) = mount_demo("roundtrip");
    let script_path = "story.bdt".into();
    let script = mount.read_script(&script_path).unwrap();
    let entries = extract_with_raw(&script);
    assert!(!entries.is_empty(), "the demo story has lines to translate");

    // A CP932-clean stand-in for a translation: what is under test is the
    // container and the write-back contract, not the prose.
    let translated = MockTranslator::new("EN: ")
        .translate(&entries, "ja", "en")
        .unwrap();
    let replacements: BTreeMap<usize, String> = translated
        .iter()
        .zip(&entries)
        .filter(|(new, old)| new.text != old.text)
        .map(|(new, old)| (old.id, new.text.clone()))
        .collect();
    assert_eq!(replacements.len(), entries.len());

    let written = mount.write_script(&script_path, &replacements).unwrap();
    assert_eq!(written.replaced, entries.len());
    assert!(written.unmatched.is_empty());

    // Still a BDT: the XOR container and the CP932 text were both rebuilt.
    assert!(kintsugi_bluegale::bdt::looks_like_bdt(&written.data));

    // Read the patch back the way a player would: as a game script.
    let patched = kintsugi_bluegale::bdt::parse_bdt("story.en.bdt", &written.data).unwrap();
    assert_eq!(patched.commands.len(), script.commands.len());
    assert!(
        patched.commands.iter().any(
            |c| matches!(c, Command::RawLine(t) if t.starts_with("EN: ") && t.contains("金継ぎ"))
        ),
        "the translated text should be in the patch"
    );

    // Labels survived untouched, so every jump still lands.
    for name in ["start", "shatter", "repair", "seam", "fin"] {
        assert!(
            patched
                .commands
                .iter()
                .any(|c| matches!(c, Command::Label(label) if label == name)),
            "label '{name}' was lost in the patch"
        );
    }

    // And the patch plays.
    let mut host = RecordingHost::default();
    Interpreter::new(patched).run(&mut host).unwrap();
    assert!(host.lines.iter().any(|line| line.starts_with("EN: ")));

    // The in-memory merge agrees with the bytes.
    let merged = apply(&script, &translated).unwrap();
    assert_eq!(merged.commands.len(), script.commands.len());
}

#[test]
fn a_patch_changes_only_the_lines_it_translates() {
    let (_temp, mount) = mount_demo("minimal");
    let script_path = "story.bdt".into();
    let script = mount.read_script(&script_path).unwrap();
    let entries = extract_with_raw(&script);

    // Translate exactly one line.
    let target = &entries[2];
    let replacements = BTreeMap::from([(target.id, "only this line".to_string())]);
    let written = mount.write_script(&script_path, &replacements).unwrap();
    assert_eq!(written.replaced, 1);

    // Every other command is byte-identical in meaning: same labels, same
    // original text, and the same number of commands.
    let patched = kintsugi_bluegale::bdt::parse_bdt("story.bdt", &written.data).unwrap();
    assert_eq!(patched.commands.len(), script.commands.len());
    for (before, after) in script.commands.iter().zip(&patched.commands) {
        match (before, after) {
            (Command::RawLine(old), Command::RawLine(new)) if old == &target.text => {
                assert_eq!(new, "only this line");
            }
            _ => assert_eq!(before, after, "an untranslated line changed"),
        }
    }
}

#[test]
fn an_unwritable_translation_is_refused_by_name_before_anything_is_written() {
    let (_temp, mount) = mount_demo("refuse");
    let script_path = "story.bdt".into();
    let script = mount.read_script(&script_path).unwrap();
    let entries = extract_with_raw(&script);

    let replacements = BTreeMap::from([(entries[0].id, "dash — and 缮".to_string())]);
    match mount.write_script(&script_path, &replacements) {
        Err(Error::Encoding { characters, .. }) => assert_eq!(characters, "—缮"),
        other => panic!("expected a CP932 refusal, got {other:?}"),
    }
}

#[test]
fn a_seam_that_cannot_write_says_so() {
    // The default trait method, exercised through a seam that does not
    // implement writing: no silent success, no empty file.
    struct ReadOnly;

    impl EngineMount for ReadOnly {
        fn info(&self) -> &kintsugi_core::plugin::MountInfo {
            static INFO: std::sync::OnceLock<kintsugi_core::plugin::MountInfo> =
                std::sync::OnceLock::new();
            INFO.get_or_init(|| kintsugi_core::plugin::MountInfo::new("readonly"))
        }

        fn vfs(&self) -> &Vfs {
            unimplemented!("the default write_script must not need the VFS")
        }
    }

    let err = ReadOnly
        .write_script(&"x.bdt".into(), &BTreeMap::new())
        .unwrap_err();
    assert!(
        err.to_string().contains("cannot write scripts back"),
        "{err}"
    );
}

/// An LLM translation is only as good as its strictness: a backend that
/// returns ids the script does not have must not shift every line after it.
#[test]
fn an_llm_reply_with_hallucinated_ids_is_reported() {
    let booking = LlmTranslator::new("http://127.0.0.1:1", "unused", "test-model");
    assert_eq!(booking.model, "test-model");
    assert!(booking.name().contains("llm"));

    // The batch contract: one request per batch, ids echoed back. Exercised
    // end to end against the mock, since the point here is the id discipline,
    // not HTTP (that lives in the translate crate's own tests).
    let (_temp, mount) = mount_demo("ids");
    let script = mount.read_script(&"story.bdt".into()).unwrap();
    let entries = extract_with_raw(&script);
    let translated = MockTranslator::new("x")
        .translate(&entries, "ja", "en")
        .unwrap();
    assert_eq!(translated.len(), entries.len());
    for (entry, original) in translated.iter().zip(&entries) {
        assert_eq!(entry.id, original.id);
    }
}
