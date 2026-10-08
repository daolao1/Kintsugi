//! The shell's own promises, tested through the binary a user actually runs.
//!
//! `patch.rs` tests the write-back contract where it is wired (library calls);
//! this file tests the shell around it. The important one is the rule that the
//! game folder is read-only — README.md promises that in those words, and the
//! check is a private function of the binary (`ensure_outside_game` in
//! `src/main.rs`), so running the binary is the only honest way to test it. A
//! promise about data loss that nothing exercises is a sentence, not a check.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A throwaway directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("kintsugi-cli-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Run the CLI the way a user does and capture everything it said.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kintsugi"))
        .args(args)
        .output()
        .expect("the kintsugi binary is built for this test")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The names in a directory, sorted, so "nothing was written here" is testable.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_patch_written_anywhere_in_the_game_folder_is_refused_and_changes_nothing() {
    let temp = TempDir::new("over-original");
    let game = temp.0.join("game");
    kintsugi_bluegale::fixtures::write_demo_game(&game).unwrap();
    let original = game.join("story.bdt");
    let before = fs::read(&original).unwrap();
    let files_before = listing(&game);

    let output = run(&[
        "translate",
        game.to_str().unwrap(),
        "--mock",
        "--no-play",
        "--write-script",
        original.to_str().unwrap(),
    ]);

    // Exit 2 is the documented code: the request was wrong, the game is fine.
    assert_eq!(
        output.status.code(),
        Some(2),
        "stdout: {}\nstderr: {}",
        stdout(&output),
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("refusing to write the repaired script inside the game folder"),
        "stderr: {}",
        stderr(&output)
    );
    assert_eq!(
        fs::read(&original).unwrap(),
        before,
        "the original was modified by a write that was supposed to be refused"
    );
    assert_eq!(
        listing(&game),
        files_before,
        "a refused write left a file behind in the game folder"
    );
}

/// The rule is about the folder, not just about the file being read: another
/// original in the same folder is just as protected.
#[test]
fn a_patch_written_over_a_different_original_is_refused_too() {
    let temp = TempDir::new("over-other-original");
    let game = temp.0.join("game");
    kintsugi_bluegale::fixtures::write_demo_game(&game).unwrap();
    let archive = game.join("game.snn");
    let before = fs::read(&archive).unwrap();

    let output = run(&[
        "translate",
        game.to_str().unwrap(),
        "--mock",
        "--no-play",
        "--write-script",
        archive.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert_eq!(
        fs::read(&archive).unwrap(),
        before,
        "--write-script is not allowed to overwrite an original that happens not to be the script"
    );
}

/// ...and it covers the glaze's own output, so no artifact lands in the folder
/// a game is installed in.
#[test]
fn a_glazed_png_inside_the_game_folder_is_refused() {
    let temp = TempDir::new("upscale-inside");
    let game = temp.0.join("game");
    kintsugi_bluegale::fixtures::write_demo_game(&game).unwrap();
    let before = listing(&game);

    let output = run(&[
        "upscale",
        game.to_str().unwrap(),
        "--asset",
        "title.bbm",
        "--factor",
        "2",
        "-o",
        game.join("title-x2.png").to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains("refusing to write the glazed image inside the game folder"),
        "stderr: {}",
        stderr(&output)
    );
    assert_eq!(listing(&game), before, "nothing new in the game folder");
}

#[test]
fn a_patch_written_beside_the_folder_leaves_every_original_byte_alone() {
    let temp = TempDir::new("beside-original");
    let game = temp.0.join("game");
    kintsugi_bluegale::fixtures::write_demo_game(&game).unwrap();
    let original = game.join("story.bdt");
    let before = fs::read(&original).unwrap();
    let files_before = listing(&game);
    // Beside the folder, not inside it: this is what the README recommends.
    let patch = temp.0.join("story.en.bdt");

    let output = run(&[
        "translate",
        game.to_str().unwrap(),
        "--mock",
        "--no-play",
        "--write-script",
        patch.to_str().unwrap(),
    ]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        stdout(&output),
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("repaired script:"),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        listing(&game),
        files_before,
        "the patch belongs outside the game folder, and nothing else may appear in it"
    );
    assert!(patch.exists(), "the patch itself must have been written");
    assert_eq!(
        fs::read(&original).unwrap(),
        before,
        "writing a patch beside the original changed the original"
    );

    // The patch is a real script, and it carries the mock translation.
    let bytes = fs::read(&patch).unwrap();
    assert!(kintsugi_bluegale::bdt::looks_like_bdt(&bytes));
    let patched = kintsugi_bluegale::bdt::parse_bdt("story.en.bdt", &bytes).unwrap();
    assert!(
        patched.commands.iter().any(|command| matches!(
            command,
            kintsugi_core::script::Command::RawLine(text) if text.starts_with("mock: ")
        )),
        "the patch should contain the translated text"
    );
}
