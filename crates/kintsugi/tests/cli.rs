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

/// The exit-code table in `docs/PLATFORMS.md` is a contract with scripts, and
/// nothing was testing it. `--faktur 4` used to print the usage text and exit
/// **0**: a typo reported success to whatever was watching the exit status, so
/// a CI job or a batch script would carry on as if a repair had been made.
#[test]
fn a_mistyped_command_is_a_usage_error_and_not_a_success() {
    let output = run(&["--faktur", "4"]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "stdout: {}\nstderr: {}",
        stdout(&output),
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("unknown command '--faktur'"),
        "stderr: {}",
        stderr(&output)
    );
    // The usage text goes to stderr with the error, where a failure's
    // explanation belongs; stdout stays clean for anyone piping it.
    assert!(
        stderr(&output).contains("Usage:"),
        "stderr: {}",
        stderr(&output)
    );
    assert!(stdout(&output).is_empty(), "stdout: {}", stdout(&output));
}

/// Helping is not failing: asking for help, with or without words, is exit 0.
#[test]
fn asking_for_help_is_not_an_error() {
    for args in [vec![], vec!["help"], vec!["--help"], vec!["-h"]] {
        let output = run(&args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "`kintsugi {}` should be help, not a failure; stderr: {}",
            args.join(" "),
            stderr(&output)
        );
        assert!(
            stdout(&output).contains("Usage:"),
            "`kintsugi {}` should print the usage text on stdout",
            args.join(" ")
        );
    }
}

/// A missing argument is a command line that was never finished, so it is exit
/// 2 — not exit 1, which would blame the game for the user's typo.
#[test]
fn a_missing_argument_is_a_usage_error() {
    for command in [
        "detect",
        "inspect",
        "play",
        "upscale",
        "interpolate",
        "translate",
    ] {
        let output = run(&[command]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "`kintsugi {command}` with no directory should be a usage error; stderr: {}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains("missing game directory"),
            "stderr: {}",
            stderr(&output)
        );
    }
}

/// Flag *values* are the same story, and the two commands have honestly
/// different bounds: the glazer's factor is bounded, the interpolator's is not.
#[test]
fn a_bad_flag_value_is_a_usage_error_with_the_right_bound() {
    let temp = TempDir::new("bad-flags");
    let game = temp.0.join("game");
    kintsugi_bluegale::fixtures::write_demo_game(&game).unwrap();
    let dir = game.to_str().unwrap();

    let cases: [(&[&str], &str); 5] = [
        (
            &["upscale", dir, "--factor", "abc"],
            "--factor must be a whole number",
        ),
        (
            &["upscale", dir, "--factor", "0"],
            "--factor must be between 1 and 16",
        ),
        (&["upscale", dir, "--method", "wat"], "unknown method 'wat'"),
        (
            &["interpolate", dir, "--factor", "0"],
            "--factor must be at least 1",
        ),
        (
            &["upscale", dir, "--faktur", "4"],
            "unknown flag '--faktur'",
        ),
    ];

    for (args, expected) in cases {
        let output = run(args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "`kintsugi {}` should be a usage error; stderr: {}",
            args.join(" "),
            stderr(&output)
        );
        assert!(
            stderr(&output).contains(expected),
            "`kintsugi {}` should say {expected:?}; stderr: {}",
            args.join(" "),
            stderr(&output)
        );
    }
}

/// ...but a factor the interpolator genuinely cannot honour is an engine
/// refusal (exit 1) that names the input, not a complaint about the command
/// line: four frames at `--factor 400` is real work (1201 frames), and the
/// budget refuses what no machine could hold without pretending the user
/// mistyped.
#[test]
fn an_impossible_frame_count_is_refused_by_the_engine_not_the_parser() {
    let temp = TempDir::new("frame-budget");
    let game = temp.0.join("game");
    kintsugi_bluegale::fixtures::write_demo_game(&game).unwrap();

    let output = run(&[
        "interpolate",
        game.to_str().unwrap(),
        "--factor",
        "4294967295",
        "-o",
        temp.0.join("frames").to_str().unwrap(),
    ]);

    assert_eq!(
        output.status.code(),
        Some(1),
        "stdout: {}\nstderr: {}",
        stdout(&output),
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("100000-frame limit"),
        "stderr: {}",
        stderr(&output)
    );
    assert!(
        !temp.0.join("frames").exists(),
        "a refused interpolation must not leave an output folder behind"
    );
}

/// `install` is the step that puts a repair somewhere playable, so these tests
/// are about what it refuses as much as what it writes. Everything here runs
/// the real binary, because the promises are about files on disk.
mod install {
    use super::*;

    /// Make a demo game and the patch `translate` would produce for it.
    fn game_and_patch(temp: &TempDir, tag: &str) -> (PathBuf, PathBuf) {
        let game = temp.0.join(format!("game-{tag}"));
        kintsugi_bluegale::fixtures::write_demo_game(&game).unwrap();
        let patch = temp.0.join(format!("patch-{tag}.bdt"));
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
            "the patch this test needs was not produced; stderr: {}",
            stderr(&output)
        );
        (game, patch)
    }

    #[test]
    fn the_repair_lands_in_the_copy_and_the_original_is_untouched() {
        let temp = TempDir::new("install-ok");
        let (game, patch) = game_and_patch(&temp, "ok");
        let original_before = fs::read(game.join("story.bdt")).unwrap();
        let files_before = listing(&game);
        let copy = temp.0.join("repaired");

        let output = run(&[
            "install",
            game.to_str().unwrap(),
            "--script",
            patch.to_str().unwrap(),
            "--into",
            copy.to_str().unwrap(),
        ]);

        assert_eq!(
            output.status.code(),
            Some(0),
            "stdout: {}\nstderr: {}",
            stdout(&output),
            stderr(&output)
        );
        // The installed script is byte-for-byte what `translate` wrote: the
        // install goes through the seam's writer again, from the copy's own
        // original bytes, and round-trips exactly.
        assert_eq!(
            fs::read(copy.join("story.bdt")).unwrap(),
            fs::read(&patch).unwrap(),
            "the installed script differs from the patch it was made from"
        );
        // ...and the game it came from is exactly as it was.
        assert_eq!(listing(&game), files_before, "the game folder changed");
        assert_eq!(
            fs::read(game.join("story.bdt")).unwrap(),
            original_before,
            "install modified the original script"
        );
        // The copy is a playable game, not just a folder of files.
        let played = run(&["play", copy.to_str().unwrap(), "--auto"]);
        assert_eq!(
            played.status.code(),
            Some(0),
            "the copy does not play; stderr: {}",
            stderr(&played)
        );
        assert!(
            stdout(&played).contains("mock: "),
            "the copy plays the original words instead of the repair: {}",
            stdout(&played)
        );
    }

    /// A copy inside the game folder is the original wearing a hat.
    #[test]
    fn a_copy_inside_the_game_folder_is_refused() {
        let temp = TempDir::new("install-inside");
        let (game, patch) = game_and_patch(&temp, "inside");
        let copy = game.join("repaired");

        let output = run(&[
            "install",
            game.to_str().unwrap(),
            "--script",
            patch.to_str().unwrap(),
            "--into",
            copy.to_str().unwrap(),
        ]);

        assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
        assert!(
            stderr(&output).contains("refusing to write the repaired copy inside the game folder"),
            "stderr: {}",
            stderr(&output)
        );
        assert!(!copy.exists(), "a refused install created the copy anyway");
    }

    /// A folder that already holds something is not a copy we may write over.
    #[test]
    fn a_folder_that_already_holds_something_is_refused() {
        let temp = TempDir::new("install-occupied");
        let (game, patch) = game_and_patch(&temp, "occupied");
        let copy = temp.0.join("repaired");
        fs::create_dir_all(&copy).unwrap();
        fs::write(copy.join("someone-elses-save.dat"), b"do not lose me").unwrap();

        let output = run(&[
            "install",
            game.to_str().unwrap(),
            "--script",
            patch.to_str().unwrap(),
            "--into",
            copy.to_str().unwrap(),
        ]);

        assert_eq!(output.status.code(), Some(1), "stderr: {}", stderr(&output));
        assert!(
            stderr(&output).contains("refusing to write a repaired copy over them"),
            "stderr: {}",
            stderr(&output)
        );
        assert_eq!(
            fs::read(copy.join("someone-elses-save.dat")).unwrap(),
            b"do not lose me",
            "a refused install touched a file that was already there"
        );
        assert_eq!(listing(&copy), vec!["someone-elses-save.dat".to_string()]);
    }

    /// The dangerous case: a patch whose lines do not sit where the game's
    /// lines sit. Ids drift when a patch and a game come from different
    /// versions, and a repair that lands on the wrong line is worse than none.
    #[test]
    fn a_patch_that_does_not_line_up_is_refused_before_anything_is_copied() {
        let temp = TempDir::new("install-misaligned");
        let (game, _) = game_and_patch(&temp, "misaligned");
        // One extra label at the top shifts every id by one.
        let original = fs::read(game.join("story.bdt")).unwrap();
        let shifted = kintsugi_bluegale::fixtures::make_bdt(&format!(
            "$extra\r\n{}",
            kintsugi_bluegale::bdt::decode_bdt_text(&original)
        ));
        let patch = temp.0.join("shifted.bdt");
        fs::write(&patch, &shifted).unwrap();
        let copy = temp.0.join("repaired");

        let output = run(&[
            "install",
            game.to_str().unwrap(),
            "--script",
            patch.to_str().unwrap(),
            "--into",
            copy.to_str().unwrap(),
        ]);

        assert_eq!(output.status.code(), Some(1), "stderr: {}", stderr(&output));
        assert!(
            stderr(&output).contains("does not line up with this game"),
            "stderr: {}",
            stderr(&output)
        );
        assert!(
            !copy.exists(),
            "the refusal must come before the copy is made, or a half-repaired \
             folder is left behind"
        );
    }

    /// A script served from an archive cannot be repaired by writing a file
    /// beside it: the archive shadows loose files, so the copy would look
    /// repaired and play the original words.
    #[test]
    fn a_script_that_lives_in_an_archive_is_refused() {
        let temp = TempDir::new("install-archived");
        let game = temp.0.join("game");
        fs::create_dir_all(&game).unwrap();
        // A game whose only script is inside the archive: `game.inx` + `game.snn`
        // hold STORY.BDT, and there is no `story.bdt` file on disk.
        let script = kintsugi_bluegale::fixtures::make_bdt("$start\r\nこんにちは。\r\n%fin\r\n");
        let (snn, placements) = kintsugi_bluegale::fixtures::make_snn(&[&script]);
        let (offset, size) = placements[0];
        let inx = kintsugi_bluegale::fixtures::make_inx(&[("STORY.BDT", offset, size)]);
        fs::write(game.join("game.inx"), &inx).unwrap();
        fs::write(game.join("game.snn"), &snn).unwrap();
        let patch = temp.0.join("patch.bdt");
        fs::write(&patch, &script).unwrap();

        let output = run(&[
            "install",
            game.to_str().unwrap(),
            "--script",
            patch.to_str().unwrap(),
            "--into",
            temp.0.join("repaired").to_str().unwrap(),
        ]);

        assert_eq!(
            output.status.code(),
            Some(1),
            "stdout: {}\nstderr: {}",
            stdout(&output),
            stderr(&output)
        );
        assert!(
            stderr(&output).contains("served from an archive"),
            "stderr: {}",
            stderr(&output)
        );
    }

    /// Both flags are required, and saying which one is missing is the whole
    /// point of a usage error.
    #[test]
    fn install_without_its_flags_is_a_usage_error() {
        let temp = TempDir::new("install-flags");
        let (game, patch) = game_and_patch(&temp, "flags");

        let missing_script = run(&[
            "install",
            game.to_str().unwrap(),
            "--into",
            temp.0.join("copy-a").to_str().unwrap(),
        ]);
        assert_eq!(
            missing_script.status.code(),
            Some(2),
            "stderr: {}",
            stderr(&missing_script)
        );
        assert!(
            stderr(&missing_script).contains("install needs --script FILE"),
            "stderr: {}",
            stderr(&missing_script)
        );

        let missing_into = run(&[
            "install",
            game.to_str().unwrap(),
            "--script",
            patch.to_str().unwrap(),
        ]);
        assert_eq!(
            missing_into.status.code(),
            Some(2),
            "stderr: {}",
            stderr(&missing_into)
        );
        assert!(
            stderr(&missing_into).contains("install needs --into DIR"),
            "stderr: {}",
            stderr(&missing_into)
        );
        assert!(
            !temp.0.join("copy-a").exists(),
            "a usage error must not leave a folder behind"
        );
    }
}
