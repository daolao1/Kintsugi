//! Mounting a BSX game, and reading pictures out of it.
//!
//! These tests go through the seam's public door — `plugin()`, `mount()`,
//! `read_image()` — rather than the decoders directly, because the interesting
//! failures are at the joints: an archive that parses but does not serve, a
//! prefix that hides a file, a picture that decodes to the wrong row order.

use kintsugi_bsx::fixtures::{demo_release, make_bsarc, make_bsg, make_bsx_dat};
use kintsugi_core::plugin::{EngineMount, EnginePlugin};
use kintsugi_core::vfs::VirtualPath;

fn mounted() -> Box<dyn EngineMount> {
    let (archive, loose, config) = demo_release();
    let mut source = kintsugi_core::vfs::MemorySource::new();
    source.insert("exe/bsx.ini", config);
    source.insert("exe/graphics.bsa", archive);
    source.insert("exe/title.bsg", loose);
    let mut vfs = kintsugi_core::vfs::Vfs::new();
    vfs.push(std::sync::Arc::new(source));
    kintsugi_bsx::plugin()
        .mount(&vfs)
        .expect("a fixture the seam recognizes mounts")
}

#[test]
fn a_picture_inside_an_archive_is_readable_under_the_archives_name() {
    let mount = mounted();
    let path = VirtualPath::new("graphics/room.bsg");
    assert!(
        mount.vfs().exists(&path),
        "the archive's entries should appear under the archive's own name"
    );
    let image = mount
        .read_image(&path)
        .expect("the archived picture decodes");
    assert_eq!((image.width, image.height), (2, 2));
    assert_eq!(
        image.rgba(),
        &[
            200, 40, 40, 255, 40, 200, 40, 255, 40, 40, 200, 255, 240, 240, 240, 255
        ]
    );
}

#[test]
fn a_loose_picture_beside_the_archive_is_still_visible() {
    let mount = mounted();
    let image = mount
        .read_image(&VirtualPath::new("exe/title.bsg"))
        .expect("the loose picture decodes");
    assert_eq!(image.rgba(), &[10, 20, 30, 255]);
}

#[test]
fn two_archives_holding_the_same_name_do_not_shadow_each_other() {
    let picture = make_bsg(1, 1, &[[1, 2, 3, 255]]);
    let other = make_bsg(1, 1, &[[4, 5, 6, 255]]);
    let mut source = kintsugi_core::vfs::MemorySource::new();
    source.insert("graphics.bsa", make_bsarc(&[("room.bsg", &picture)]));
    source.insert("extra.bsa", make_bsarc(&[("room.bsg", &other)]));
    let mut vfs = kintsugi_core::vfs::Vfs::new();
    vfs.push(std::sync::Arc::new(source));
    let mount = kintsugi_bsx::plugin()
        .mount(&vfs)
        .expect("two archives mount");
    assert_eq!(
        mount
            .read_image(&VirtualPath::new("graphics/room.bsg"))
            .expect("the first archive's picture")
            .rgba(),
        &[1, 2, 3, 255]
    );
    assert_eq!(
        mount
            .read_image(&VirtualPath::new("extra/room.bsg"))
            .expect("the second archive's picture")
            .rgba(),
        &[4, 5, 6, 255]
    );
}

#[test]
fn a_file_that_is_not_a_picture_is_refused_by_name() {
    let mut source = kintsugi_core::vfs::MemorySource::new();
    source.insert(
        "graphics.bsa",
        make_bsarc(&[("readme.txt", b"not a picture")]),
    );
    let mut vfs = kintsugi_core::vfs::Vfs::new();
    vfs.push(std::sync::Arc::new(source));
    let mount = kintsugi_bsx::plugin()
        .mount(&vfs)
        .expect("the archive mounts");
    let error = mount
        .read_image(&VirtualPath::new("graphics/readme.txt"))
        .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("readme.txt"),
        "the refusal should name the file: {message}"
    );
}

#[test]
fn a_folder_of_loose_pictures_is_mounted_without_an_archive() {
    // The picture format is this engine's, whether a `.bsa` holds it or the
    // release shipped it loose.
    let mut source = kintsugi_core::vfs::MemorySource::new();
    source.insert("title.bsg", make_bsg(1, 1, &[[1, 2, 3, 255]]));
    let mut vfs = kintsugi_core::vfs::Vfs::new();
    vfs.push(std::sync::Arc::new(source));
    let mount = kintsugi_bsx::plugin()
        .mount(&vfs)
        .expect("a loose picture is this engine's game");
    let image = mount
        .read_image(&VirtualPath::new("title.bsg"))
        .expect("the loose picture decodes");
    assert_eq!(image.width, 1);
}

#[test]
fn a_folder_of_strangers_is_not_mounted() {
    let mut source = kintsugi_core::vfs::MemorySource::new();
    source.insert("readme.txt", b"nothing here belongs to this engine");
    let mut vfs = kintsugi_core::vfs::Vfs::new();
    vfs.push(std::sync::Arc::new(source));
    let error = match kintsugi_bsx::plugin().mount(&vfs) {
        Ok(_) => panic!("a folder of strangers is not this engine's game"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(
        message.contains("BSArc"),
        "it should name what it looked for: {message}"
    );
    assert!(message.contains("BSScript"), "{message}");
}

#[test]
fn a_story_beside_no_archive_is_the_script_the_game_is_played_through() {
    let mut source = kintsugi_core::vfs::MemorySource::new();
    source.insert("exe/bsx.dat", make_bsx_dat());
    let mut vfs = kintsugi_core::vfs::Vfs::new();
    vfs.push(std::sync::Arc::new(source));
    let mount = kintsugi_bsx::plugin()
        .mount(&vfs)
        .expect("the story mounts");

    let path = mount
        .primary_script()
        .expect("one readable story names itself");
    assert_eq!(path.as_str(), "exe/bsx.dat");
    let script = mount.read_script(&path).expect("the story reads");
    assert_eq!(script.commands.len(), 6);
    assert_eq!(
        script.commands[5],
        kintsugi_core::script::Command::RawLine(String::from("見に行かない"))
    );
    // The seam says what it does not know, in the script it hands over.
    assert_eq!(script.warnings.len(), 1);
    assert!(
        script.warnings[0].contains("flat table"),
        "{:?}",
        script.warnings
    );

    // And a repair lands on the line it names and nowhere else.
    let mut replacements = std::collections::BTreeMap::new();
    replacements.insert(5usize, String::from("I will not go and look"));
    let written = mount
        .write_script(&path, &replacements)
        .expect("the story is a file of its own");
    assert_eq!(written.replaced, 1);
    assert!(written.files.is_empty() || written.files.len() == 1);
    let reread = kintsugi_bsx::Story::parse(&written.script).expect("a repair is a story");
    assert_eq!(reread.strings()[5], "I will not go and look");
    assert_eq!(reread.strings()[0], "■■■　真理奈ＥＮＤ　■■■");
}
