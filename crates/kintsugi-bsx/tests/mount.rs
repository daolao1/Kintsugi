//! Mounting a BSX game, and reading pictures out of it.
//!
//! These tests go through the seam's public door — `plugin()`, `mount()`,
//! `read_image()` — rather than the decoders directly, because the interesting
//! failures are at the joints: an archive that parses but does not serve, a
//! prefix that hides a file, a picture that decodes to the wrong row order.

use kintsugi_bsx::fixtures::{
    demo_release, make_bsarc, make_bsg, make_bsx_dat, make_story_showing,
};
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

/// A line the code never shows is still a line of the story. The seam used to
/// hand over the whole table and say the order was the file's; now that it can
/// read the code, it must not lose the lines the code has nothing to say about.
#[test]
fn a_line_the_code_never_shows_is_kept_and_labelled() {
    // Four lines or more: a story as short as the fixture's three-name cast
    // list is refused as ambiguous, which is the rule this test must not break.
    let lines = [
        "shown first",
        "never shown",
        "shown last",
        "also never shown",
    ];
    let mut source = kintsugi_core::vfs::MemorySource::new();
    source.insert("exe/bsx.dat", make_story_showing(&lines, &[(0, 0), (1, 2)]));
    let mut vfs = kintsugi_core::vfs::Vfs::new();
    vfs.push(std::sync::Arc::new(source));
    let mount = kintsugi_bsx::plugin()
        .mount(&vfs)
        .expect("the story mounts");
    let script = mount
        .read_script(&VirtualPath::new("exe/bsx.dat"))
        .expect("the story reads");

    assert_eq!(script.commands.len(), 4);
    assert_eq!(
        script.commands[0],
        kintsugi_core::script::Command::Narration(String::from("shown first"))
    );
    assert_eq!(
        script.commands[1],
        kintsugi_core::script::Command::Dialogue {
            speaker: None,
            text: String::from("shown last"),
        }
    );
    assert_eq!(
        script.commands[2],
        kintsugi_core::script::Command::RawLine(String::from("never shown"))
    );
    assert!(
        script.warnings[0].contains("never shows"),
        "{:?}",
        script.warnings
    );
}

/// One string in the file can be shown in two scenes. A translation that gives
/// those two places different words cannot be honoured in both, and a seam that
/// silently picked one would lose the other.
#[test]
fn one_line_shown_twice_cannot_be_translated_two_ways() {
    let lines = ["the same words", "other words", "third", "fourth"];
    let mut source = kintsugi_core::vfs::MemorySource::new();
    source.insert(
        "exe/bsx.dat",
        make_story_showing(&lines, &[(0, 0), (1, 1), (0, 0)]),
    );
    let mut vfs = kintsugi_core::vfs::Vfs::new();
    vfs.push(std::sync::Arc::new(source));
    let mount = kintsugi_bsx::plugin()
        .mount(&vfs)
        .expect("the story mounts");
    let path = VirtualPath::new("exe/bsx.dat");

    // The same words at both places is a repair that can be honoured.
    let agreed = std::collections::BTreeMap::from([
        (0usize, String::from("同じ言葉")),
        (2usize, String::from("同じ言葉")),
    ]);
    let written = mount
        .write_script(&path, &agreed)
        .expect("one line, one text, one repair");
    let reread = kintsugi_bsx::Story::parse(&written.script).expect("a repair is a story");
    assert_eq!(reread.strings()[0], "同じ言葉");
    assert_eq!(written.replaced, 1);

    // Two readings of the same string is a question, not a repair.
    let clashing = std::collections::BTreeMap::from([
        (0usize, String::from("一つ目")),
        (2usize, String::from("二つ目")),
    ]);
    let error = match mount.write_script(&path, &clashing) {
        Ok(_) => panic!("two readings of one line cannot both be written"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(message.contains("shown more than once"), "{message}");
    assert!(message.contains("line 0"), "{message}");
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
    // The code says what each line is shown as. The fixture cycles its three
    // channels over its six lines, so the first is narration and the sixth is
    // dialogue — typed text, where this seam used to hand over six raw lines.
    assert_eq!(
        script.commands[0],
        kintsugi_core::script::Command::Narration(String::from("■■■　真理奈ＥＮＤ　■■■"))
    );
    assert_eq!(
        script.commands[5],
        kintsugi_core::script::Command::Dialogue {
            speaker: None,
            text: String::from("見に行かない"),
        }
    );
    // The seam says what it does not know, in the script it hands over: the
    // channel is not a character, and a branch is not decoded.
    assert_eq!(script.warnings.len(), 1);
    assert!(
        script.warnings[0].contains("order the code shows them"),
        "{:?}",
        script.warnings
    );
    assert!(
        script.warnings[0].contains("not one playthrough"),
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

/// A story with a program table mounts into a walk with scenery and choices:
/// the stage walk reads what the code says, and a repair still lands on the
/// line it names because the same walk numbers the commands.
#[test]
fn a_staged_story_plays_with_its_scenery_and_repairs_by_id() {
    use kintsugi_core::script::Command;

    let mut source = kintsugi_core::vfs::MemorySource::new();
    source.insert("exe/bsx.dat", kintsugi_bsx::fixtures::make_staged_story());
    source.insert("graphics/bg01.bsg", b"a picture");
    source.insert("bgm/bgm01.ogg", b"a tune");
    source.insert("voice/10100001.ogg", b"a voice");
    let mut vfs = kintsugi_core::vfs::Vfs::new();
    vfs.push(std::sync::Arc::new(source));
    let mount = kintsugi_bsx::plugin()
        .mount(&vfs)
        .expect("the staged story mounts");
    let path = VirtualPath::new("exe/bsx.dat");
    let script = mount.read_script(&path).expect("the staged story reads");

    // The walk names the programs, sets the scene, offers the choice, and
    // brings the branches back to the story.
    assert!(
        matches!(&script.commands[0], Command::Label(label) if label == "bsx:0:opening"),
        "{:?}",
        script.commands[0]
    );
    assert!(
        script
            .commands
            .iter()
            .any(|c| matches!(c, Command::SetBackground(p) if p == "graphics/bg01.bsg")),
        "the background resolves through the archives: {:?}",
        script.commands
    );
    assert!(
        script
            .commands
            .iter()
            .any(|c| matches!(c, Command::PlayMusic(Some(p)) if p == "bgm/bgm01.ogg")),
        "the music resolves"
    );
    assert!(
        script
            .commands
            .iter()
            .any(|c| matches!(c, Command::PlaySound(p) if p == "voice/10100001.ogg")),
        "the voice resolves"
    );
    let choice = script
        .commands
        .iter()
        .find_map(|c| match c {
            Command::Choice(options) => Some(options),
            _ => None,
        })
        .expect("the choice is offered");
    assert_eq!(choice[0].label, "pick the first branch");
    assert_eq!(choice[0].goto, "bsx:1:branch_a");
    assert!(
        script
            .commands
            .iter()
            .any(|c| matches!(c, Command::Jump(label) if label == "bsx:3:merge")),
        "a branch returns to the story"
    );
    assert!(
        script.warnings[0].contains("program table"),
        "{:?}",
        script.warnings
    );

    // A repair lands by command id on the line that command shows, through
    // the same walk the reading took.
    let id = script
        .commands
        .iter()
        .position(|c| matches!(c, Command::Narration(text) if text == "line zero"))
        .expect("the line is a command");
    let replacements = std::collections::BTreeMap::from([(id, String::from("第零行"))]);
    let written = mount
        .write_script(&path, &replacements)
        .expect("the repair writes");
    assert_eq!(written.replaced, 1);
    let reread = kintsugi_bsx::Story::parse(&written.script).expect("a repair is a story");
    assert_eq!(reread.strings()[0], "第零行");
    assert_eq!(reread.strings()[3], "line three", "no other line moves");
}
