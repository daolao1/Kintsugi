//! End-to-end tests for the BlueGale seam: synthetic files in, playable
//! game out. No real game data is used anywhere in this crate.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use kintsugi_core::asset::AudioCodec;
use kintsugi_core::detect::Confidence;
use kintsugi_core::plugin::{EnginePlugin, Registry};
use kintsugi_core::runtime::{Event, Host};
use kintsugi_core::vfs::Vfs;

use kintsugi_bluegale::fixtures;
use kintsugi_bluegale::{plugin, zbm};

/// A throwaway directory, removed on drop.
struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("kintsugi-bluegale-{}-{}", tag, std::process::id()));
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
    events: Vec<String>,
}

impl Host for RecordingHost {
    fn show_text(&mut self, speaker: Option<&str>, text: &str) -> kintsugi_core::Result<()> {
        match speaker {
            Some(s) => self.lines.push(format!("[{s}] {text}")),
            None => self.lines.push(text.to_string()),
        }
        Ok(())
    }

    fn event(&mut self, event: Event<'_>) -> kintsugi_core::Result<()> {
        self.events.push(format!("{event:?}"));
        Ok(())
    }

    fn choose(
        &mut self,
        options: &[kintsugi_core::script::ChoiceOption],
    ) -> kintsugi_core::Result<usize> {
        Ok(options.len() - 1)
    }
}

fn demo_vfs(tag: &str) -> (TempDir, Vfs) {
    let temp = TempDir::new(tag);
    let game_dir = temp.0.join("game");
    fixtures::write_demo_game(&game_dir).unwrap();
    let vfs = Vfs::from_directory(&game_dir).unwrap();
    (temp, vfs)
}

#[test]
fn zbm_roundtrip_both_obfuscation_variants() {
    let bmp = fixtures::bmp_bytes(4, 3, &[], &[128u8; 4 * 3 * 3]);
    for obfuscate in [false, true] {
        let zbm_bytes = fixtures::make_zbm(&bmp, obfuscate);
        assert!(zbm::is_zbm(&zbm_bytes));
        let image = zbm::decode_zbm(&zbm_bytes).unwrap();
        assert_eq!((image.width, image.height), (4, 3));
        // Gray 128 stays gray: the luma survives pack, XOR, and decode.
        assert_eq!(&image.rgba()[0..4], &[128, 128, 128, 255]);
    }
}

#[test]
fn zbm_rejects_garbage_payload() {
    let mut data = b"amp_".to_vec();
    data.extend_from_slice(&1i16.to_le_bytes());
    data.extend_from_slice(&0x100u32.to_le_bytes()); // promises 256 bytes
    data.extend_from_slice(&14u32.to_le_bytes());
    data.extend_from_slice(&[0u8; 4]); // but packs almost nothing
    assert!(zbm::decode_zbm(&data).is_err());
}

#[test]
fn plugin_detects_demo_game_as_certain() {
    let (_temp, vfs) = demo_vfs("detect");
    let detections = plugin().detect(&vfs).unwrap();
    assert_eq!(detections.len(), 1);
    assert_eq!(detections[0].confidence, Confidence::Certain);
    assert!(detections[0].note.contains("SNN archive"));
}

#[test]
fn plugin_detects_rejects_empty_dir() {
    let temp = TempDir::new("empty");
    fs::write(temp.0.join("readme.txt"), "nothing here").unwrap();
    let vfs = Vfs::from_directory(&temp.0).unwrap();
    let detections = plugin().detect(&vfs).unwrap();
    assert!(detections.is_empty());
}

#[test]
fn mount_reads_archived_images_audio_and_script() {
    let (_temp, vfs) = demo_vfs("mount");
    let mount = plugin().mount(&vfs).unwrap();

    // Mounted archive shadows nothing here, but its entries must be visible.
    let title = mount.read_image(&"title.bbm".into()).unwrap();
    assert_eq!((title.width, title.height), (4, 3));
    let gold = &title.rgba()[16..20]; // row 1 (the gold seam)
    assert_eq!(gold, &[212, 175, 55, 255]);

    let room = mount.read_image(&"room.zbm".into()).unwrap();
    assert_eq!((room.width, room.height), (4, 4));

    let face = mount.read_image(&"face.zbm".into()).unwrap();
    assert_eq!((face.width, face.height), (3, 3));
    // Palette index 1 (gold) sits at the centre of the 3x3 portrait, and it
    // survived CP932-free packing: XOR, LZ, and palette lookup all agree.
    assert_eq!(&face.rgba()[16..20], &[212, 175, 55, 255]); // (1,1)
    assert_eq!(&face.rgba()[4..8], &[200, 160, 90, 255]); // (1,0) = index 4

    let audio = mount.read_audio(&"opening.ogg".into()).unwrap();
    assert_eq!(audio.codec, AudioCodec::Ogg);

    let script = mount.read_script(&"story.bdt".into()).unwrap();
    assert!(script.commands.len() >= 10);
    assert!(
        script
            .commands
            .iter()
            .any(|c| matches!(c, kintsugi_core::script::Command::Label(l) if l == "repair"))
    );
    assert!(
        script.commands.iter().any(
            |c| matches!(c, kintsugi_core::script::Command::RawLine(t) if t.contains("金継ぎ"))
        )
    );
}

#[test]
fn script_interprets_end_to_end() {
    let (_temp, vfs) = demo_vfs("play");
    let mount = plugin().mount(&vfs).unwrap();
    let script = mount.read_script(&"story.bdt".into()).unwrap();
    let interpreter = kintsugi_core::runtime::Interpreter::new(script);
    let mut host = RecordingHost::default();
    interpreter.run(&mut host).unwrap();

    assert!(host.lines.iter().any(|l| l.contains("金継ぎ")));
    assert!(host.lines.iter().any(|l| l.contains("茶碗")));
    // The story ends at the %fin label and stops.
    assert_eq!(
        host.lines.last().unwrap(),
        "欠けた時間ごと、黄金で結ばれて。"
    );
}

#[test]
fn registry_mounts_demo_game_end_to_end() {
    let (_temp, vfs) = demo_vfs("registry");
    let mut registry = Registry::new();
    registry.register(Arc::new(plugin()));
    let (found_plugin, mount) = registry.mount_best(&vfs).unwrap();
    assert_eq!(found_plugin.metadata().id, "bluegale");
    assert!(!mount.info().notes.is_empty());
    assert!(mount.vfs().exists(&"story.bdt".into()));
}

#[test]
fn loose_bdt_alone_mounts_without_archives() {
    let temp = TempDir::new("bdt-only");
    let bdt = fixtures::make_bdt("$a\r\nこんにちは。\r\n");
    fs::write(temp.0.join("scene.bdt"), bdt).unwrap();
    let vfs = Vfs::from_directory(&temp.0).unwrap();

    let detections = plugin().detect(&vfs).unwrap();
    assert_eq!(detections[0].confidence, Confidence::Likely);

    let mount = plugin().mount(&vfs).unwrap();
    let script = mount.read_script(&"scene.bdt".into()).unwrap();
    assert_eq!(script.commands.len(), 2);
}
