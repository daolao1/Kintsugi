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
    run_with_env(args, &[])
}

/// The same, with environment variables set for the child.
///
/// Needed because `--mock` is deterministic: asking for two *different* patches
/// out of one game means asking the mock translator to mark its output, which
/// it does through `KINTSUGI_MOCK_MARKER`.
fn run_with_env(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kintsugi"));
    command.args(args);
    // Colour is for eyes, not for assertions. The binary honours `NO_COLOR`,
    // and a test that matched gold-coloured text would pass only in a shell
    // that happened to set it — which is exactly how one did, on one of three
    // CI platforms.
    command.env("NO_COLOR", "1");
    for (key, value) in env {
        command.env(key, value);
    }
    command
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

    /// A game whose script lives inside its archive: the repair goes back into
    /// the archive, which means two files change — the blob and the index that
    /// points into it — and the copy plays the translation.
    #[test]
    fn a_script_inside_an_archive_is_repaired_in_the_archive() {
        let temp = TempDir::new("install-archived");
        let game = temp.0.join("game");
        fs::create_dir_all(&game).unwrap();
        // No `story.bdt` on disk: STORY.BDT exists only inside game.snn.
        let script = kintsugi_bluegale::fixtures::make_bdt("$start\r\nこんにちは。\r\n%fin\r\n");
        let (snn, placements) = kintsugi_bluegale::fixtures::make_snn(&[&script]);
        let (offset, size) = placements[0];
        let inx = kintsugi_bluegale::fixtures::make_inx(&[("STORY.BDT", offset, size)]);
        fs::write(game.join("game.inx"), &inx).unwrap();
        fs::write(game.join("game.snn"), &snn).unwrap();
        // A patch with one line translated, which is what `translate
        // --write-script` produces: the script's own bytes, one line changed.
        let parsed = kintsugi_bluegale::bdt::parse_bdt("story.bdt", &script).unwrap();
        let id = parsed
            .commands
            .iter()
            .position(
                |c| matches!(c, kintsugi_core::script::Command::RawLine(t) if t == "こんにちは。"),
            )
            .expect("the fixture has a narration line");
        let patch_bytes = kintsugi_bluegale::bdt::rewrite_bdt(
            &script,
            &std::collections::BTreeMap::from([(id, "mock: hello".to_string())]),
        )
        .unwrap()
        .data;
        let patch = temp.0.join("patch.bdt");
        fs::write(&patch, &patch_bytes).unwrap();
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
        assert!(
            stdout(&output).contains("installed story.bdt into the copy"),
            "stdout: {}",
            stdout(&output)
        );

        // The archive changed, the loose file was not invented, and the original
        // folder is byte-for-byte what it was.
        assert!(!copy.join("story.bdt").exists());
        assert_ne!(fs::read(copy.join("game.snn")).unwrap(), snn);
        assert_ne!(fs::read(copy.join("game.inx")).unwrap(), inx);
        assert_eq!(fs::read(game.join("game.snn")).unwrap(), snn);
        assert_eq!(fs::read(game.join("game.inx")).unwrap(), inx);

        // And the copy plays the translated script, read back through the seam:
        // the archive had to round-trip through its own parser for that to work.
        let mut vfs = kintsugi_core::vfs::Vfs::new();
        vfs.push(std::sync::Arc::new(
            kintsugi_core::vfs::DirectorySource::new(&copy),
        ));
        let mount = kintsugi_core::plugin::EnginePlugin::mount(&kintsugi_bluegale::plugin(), &vfs)
            .expect("the repaired copy must mount");
        let script_path = kintsugi_core::vfs::VirtualPath::new("story.bdt");
        let repaired = mount
            .read_script(&script_path)
            .expect("the repaired script must be readable");
        assert!(
            repaired.commands.iter().any(
                |c| matches!(c, kintsugi_core::script::Command::RawLine(t) if t == "mock: hello")
            ),
            "the copy does not play the translation: {:?}",
            repaired.commands
        );
    }

    /// Re-running the pipeline after fixing a translation must not mean
    /// deleting a folder by hand — that is how a real game folder gets deleted
    /// by mistake. Kintsugi recognises its own work instead.
    #[test]
    fn a_second_install_replaces_the_folders_own_previous_work() {
        let temp = TempDir::new("install-again");
        let (game, first) = game_and_patch(&temp, "again");
        let copy = temp.0.join("repaired");
        let install = |patch: &Path| {
            run(&[
                "install",
                game.to_str().unwrap(),
                "--script",
                patch.to_str().unwrap(),
                "--into",
                copy.to_str().unwrap(),
            ])
        };
        assert_eq!(install(&first).status.code(), Some(0));

        // A different patch: the second run must land it, not refuse the folder.
        let second = temp.0.join("patch-again-2.bdt");
        let output = run_with_env(
            &[
                "translate",
                game.to_str().unwrap(),
                "--mock",
                "--no-play",
                "--write-script",
                second.to_str().unwrap(),
            ],
            &[("KINTSUGI_MOCK_MARKER", "[second pass] ")],
        );
        assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
        assert_ne!(
            fs::read(&first).unwrap(),
            fs::read(&second).unwrap(),
            "this test needs two different patches to be meaningful"
        );

        let again = install(&second);
        assert_eq!(
            again.status.code(),
            Some(0),
            "stdout: {}\nstderr: {}",
            stdout(&again),
            stderr(&again)
        );
        assert!(
            stdout(&again).contains("replacing this folder's own previous install"),
            "stdout: {}",
            stdout(&again)
        );
        assert_eq!(
            fs::read(copy.join("story.bdt")).unwrap(),
            fs::read(&second).unwrap(),
            "the second install did not land the second patch"
        );
        // Nothing was left behind from the first install.
        let manifest = fs::read_to_string(copy.join(".kintsugi-install")).unwrap();
        assert!(
            manifest.contains(&format!("patch\t{}\t", fs::read(&second).unwrap().len())),
            "the manifest still describes the first patch:\n{manifest}"
        );
    }

    /// A copy that has been played in is not a copy this tool may overwrite:
    /// the save game in it is the player's, not ours.
    #[test]
    fn a_copy_that_has_been_used_is_refused_by_name() {
        let temp = TempDir::new("install-used");
        let (game, patch) = game_and_patch(&temp, "used");
        let copy = temp.0.join("repaired");
        let install = || {
            run(&[
                "install",
                game.to_str().unwrap(),
                "--script",
                patch.to_str().unwrap(),
                "--into",
                copy.to_str().unwrap(),
            ])
        };
        assert_eq!(install().status.code(), Some(0));

        fs::write(copy.join("save01.dat"), b"someone played this").unwrap();
        let used = install();
        assert_eq!(used.status.code(), Some(1), "stderr: {}", stderr(&used));
        assert!(
            stderr(&used).contains("save01.dat"),
            "the refusal must name the file that stopped it: {}",
            stderr(&used)
        );
        assert_eq!(
            fs::read(copy.join("save01.dat")).unwrap(),
            b"someone played this"
        );

        // And a copy whose installed script was hand-edited afterwards is the
        // same case, reported with the reason rather than the bare name.
        fs::remove_file(copy.join("save01.dat")).unwrap();
        let mut edited = fs::read(copy.join("story.bdt")).unwrap();
        edited.push(b'x');
        fs::write(copy.join("story.bdt"), &edited).unwrap();
        let changed = install();
        assert_eq!(
            changed.status.code(),
            Some(1),
            "stderr: {}",
            stderr(&changed)
        );
        assert!(
            stderr(&changed).contains("modified since kintsugi wrote it"),
            "stderr: {}",
            stderr(&changed)
        );
        assert_eq!(
            fs::read(copy.join("story.bdt")).unwrap(),
            edited,
            "a refused install rewrote the file anyway"
        );
    }

    /// A patch that changes nothing is refused rather than installed: a copy
    /// that is byte-for-byte the original, reported as a repaired one, is the
    /// most expensive kind of quiet lie.
    #[test]
    fn a_patch_that_changes_nothing_is_refused() {
        let temp = TempDir::new("install-noop");
        let (game, _translated) = game_and_patch(&temp, "noop");
        // The patch is the game's own script, so every line "matches".
        let same = temp.0.join("same.bdt");
        fs::copy(game.join("story.bdt"), &same).unwrap();
        let copy = temp.0.join("repaired");

        let output = run(&[
            "install",
            game.to_str().unwrap(),
            "--script",
            same.to_str().unwrap(),
            "--into",
            copy.to_str().unwrap(),
        ]);
        assert_eq!(
            output.status.code(),
            Some(1),
            "stdout: {}\nstderr: {}",
            stdout(&output),
            stderr(&output)
        );
        assert!(
            stderr(&output).contains("would change nothing"),
            "stderr: {}",
            stderr(&output)
        );
        assert!(
            !copy.exists(),
            "a refused install must not leave a copy: {}",
            copy.display()
        );
    }

    /// The guarantee that makes a mistake cheap: a destination kintsugi made is
    /// removed again if the install does not finish, so the folder is either a
    /// complete repair with a manifest, or it is nothing. Nothing here is about
    /// the user being careful.
    #[test]
    fn a_failed_install_leaves_nothing_behind() {
        let temp = TempDir::new("install-rollback");
        let (game, patch) = game_and_patch(&temp, "rollback");
        let script = game.join("story.bdt");
        // A read-only script: `fs::copy` carries the permission bits into the
        // copy, so writing the repair fails *after* the destination exists —
        // which is the only moment this guarantee is about.
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&script, permissions).unwrap();
        let copy = temp.0.join("repaired");

        let output = run(&[
            "install",
            game.to_str().unwrap(),
            "--script",
            patch.to_str().unwrap(),
            "--into",
            copy.to_str().unwrap(),
        ]);
        if output.status.code() == Some(0) {
            // Running as root, where a read-only file is still writable and the
            // failure this test provokes cannot happen. CI is not root.
            return;
        }
        assert_eq!(
            output.status.code(),
            Some(1),
            "stdout: {}\nstderr: {}",
            stdout(&output),
            stderr(&output)
        );
        assert!(
            !copy.exists(),
            "the failed install left a half-made copy at {}",
            copy.display()
        );
        assert!(
            stdout(&output).contains("removed the half-made copy"),
            "the removal must be said out loud, not done quietly: {}",
            stdout(&output)
        );
        assert!(
            stderr(&output).contains("story.bdt"),
            "the error must name the file it could not write: {}",
            stderr(&output)
        );
        // The game folder itself is untouched, read-only script and all.
        assert!(script.is_file(), "the original script was removed");
    }

    /// A game folder with a subdirectory in it: the copy must be a copy, and
    /// the manifest must describe the nested file the way every platform
    /// spells it (forward slashes), because that string is verified later.
    #[test]
    fn a_nested_folder_is_copied_and_recorded() {
        let temp = TempDir::new("install-nested");
        let (game, patch) = game_and_patch(&temp, "nested");
        fs::create_dir_all(game.join("bgm")).unwrap();
        fs::write(game.join("bgm").join("theme.ogg"), b"not really an ogg").unwrap();
        let copy = temp.0.join("repaired");
        let nested = copy.join("bgm").join("theme.ogg");

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
        assert_eq!(
            fs::read(&nested).unwrap(),
            b"not really an ogg",
            "the nested file was not copied"
        );

        let manifest = fs::read_to_string(copy.join(".kintsugi-install")).unwrap();
        assert!(
            manifest.contains("\tbgm/theme.ogg\n"),
            "the manifest must record nested files with forward slashes, on every \
             platform:\n{manifest}"
        );
        // And an unchanged install still verifies, nested file and all: the
        // check would reject the folder it just wrote if the two spellings
        // disagreed.
        let again = run(&[
            "install",
            game.to_str().unwrap(),
            "--script",
            patch.to_str().unwrap(),
            "--into",
            copy.to_str().unwrap(),
        ]);
        assert_eq!(
            again.status.code(),
            Some(0),
            "stdout: {}\nstderr: {}",
            stdout(&again),
            stderr(&again)
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

    /// The script a command works on: the one the engine names, or the one the
    /// user names — the host knows neither an engine's file names nor its
    /// extensions.
    #[test]
    fn the_script_is_the_engines_choice_or_the_users() {
        let temp = TempDir::new("script-choice");
        let (game, _) = game_and_patch(&temp, "choice");
        // A second script, much shorter, so which one was translated shows up
        // in the patch rather than in the wording of a log line.
        let omake = kintsugi_bluegale::fixtures::make_bdt("$start\r\nおまけです。\r\n%fin\r\n");
        fs::write(game.join("omake.bdt"), &omake).unwrap();
        let out = |name: &str| temp.0.join(name).to_string_lossy().into_owned();

        // Named with `--as`, which is `install`'s spelling of the same thing.
        let named = run(&[
            "translate",
            game.to_str().unwrap(),
            "--mock",
            "--no-play",
            "--as",
            "omake.bdt",
            "--write-script",
            &out("as.bdt"),
        ]);
        assert_eq!(
            named.status.code(),
            Some(0),
            "stdout: {}\nstderr: {}",
            stdout(&named),
            stderr(&named)
        );
        let via_as = kintsugi_bluegale::bdt::parse_bdt("as.bdt", &fs::read(out("as.bdt")).unwrap())
            .expect("the patch must be a script");
        assert_eq!(
            via_as.commands.len(),
            3,
            "`--as omake.bdt` did not translate omake.bdt"
        );

        // `--script` is the same flag here, and must produce the same bytes.
        let via_script = run(&[
            "translate",
            game.to_str().unwrap(),
            "--mock",
            "--no-play",
            "--script",
            "omake.bdt",
            "--write-script",
            &out("script.bdt"),
        ]);
        assert_eq!(via_script.status.code(), Some(0));
        assert_eq!(
            fs::read(out("as.bdt")).unwrap(),
            fs::read(out("script.bdt")).unwrap(),
            "--as and --script disagree about which script to translate"
        );

        // With neither, the seam names the game's script: story.bdt, which is
        // the long one.
        let default = run(&[
            "translate",
            game.to_str().unwrap(),
            "--mock",
            "--no-play",
            "--write-script",
            &out("default.bdt"),
        ]);
        assert_eq!(default.status.code(), Some(0));
        let chosen = kintsugi_bluegale::bdt::parse_bdt(
            "default.bdt",
            &fs::read(out("default.bdt")).unwrap(),
        )
        .unwrap();
        assert!(
            chosen.commands.len() > 3,
            "the seam's own choice was not the game's main script: {} commands",
            chosen.commands.len()
        );
    }

    /// Several scripts and no main one: a question only the person holding the
    /// game can settle, so the answer is a refusal that names them — not a
    /// heuristic that picks one and reports which *after* translating it.
    #[test]
    fn several_scripts_and_no_main_one_is_refused_with_the_list() {
        let temp = TempDir::new("script-ambiguous");
        let (game, _) = game_and_patch(&temp, "ambiguous");
        fs::rename(game.join("story.bdt"), game.join("main_scenario.bdt")).unwrap();
        fs::write(
            game.join("omake.bdt"),
            kintsugi_bluegale::fixtures::make_bdt("$start\r\nおまけです。\r\n%fin\r\n"),
        )
        .unwrap();

        let output = run(&["translate", game.to_str().unwrap(), "--mock", "--no-play"]);
        assert_eq!(
            output.status.code(),
            Some(1),
            "stdout: {}\nstderr: {}",
            stdout(&output),
            stderr(&output)
        );
        let message = stderr(&output);
        assert!(
            message.contains("main_scenario.bdt")
                && message.contains("omake.bdt")
                && message.contains("none of them is story.bdt"),
            "the refusal must list what it found: {message}"
        );

        // And naming one gets on with it.
        let named = run(&[
            "translate",
            game.to_str().unwrap(),
            "--mock",
            "--no-play",
            "--script",
            "omake.bdt",
        ]);
        assert_eq!(named.status.code(), Some(0), "stderr: {}", stderr(&named));
    }

    /// Asking for help gets help, wherever in the line it appears: a user who
    /// typed `--help` asked a question, and answering "missing game directory"
    /// is answering a different one.
    #[test]
    fn asking_for_help_gets_help() {
        for args in [
            vec!["--help"],
            vec!["help"],
            vec!["translate", "--help"],
            vec!["install", "some-game", "-h"],
            vec!["upscale", "some-game", "--help"],
        ] {
            let output = run(&args);
            assert_eq!(
                output.status.code(),
                Some(0),
                "`{}` exited {}; stderr: {}",
                args.join(" "),
                output.status.code().unwrap_or(-1),
                stderr(&output)
            );
            assert!(
                stdout(&output).contains("Usage:"),
                "`{}` did not print usage: {}",
                args.join(" "),
                stdout(&output)
            );
        }
        // `--version` is the same kind of question, and takes no game folder.
        let version = run(&["install", "--version"]);
        assert_eq!(version.status.code(), Some(0));
        assert!(stdout(&version).starts_with("kintsugi "));
    }

    /// A flag a command does not use is a mistake, not something to ignore:
    /// `translate --as` used to be accepted and silently dropped, which is how
    /// a user translates one script and repairs another.
    #[test]
    fn a_flag_the_command_does_not_use_is_a_usage_error() {
        let temp = TempDir::new("flag-scope");
        let (game, _) = game_and_patch(&temp, "scope");

        for (args, flag) in [
            (
                vec!["detect", game.to_str().unwrap(), "--factor", "4"],
                "--factor",
            ),
            (
                vec!["detect", game.to_str().unwrap(), "--as", "story.bdt"],
                "--as",
            ),
            (
                vec![
                    "translate",
                    game.to_str().unwrap(),
                    "--into",
                    "/tmp/nowhere",
                ],
                "--into",
            ),
            (
                vec!["play", game.to_str().unwrap(), "--method", "anime4k"],
                "--method",
            ),
        ] {
            let output = run(&args);
            assert_eq!(
                output.status.code(),
                Some(2),
                "`{}` exited {} instead of 2; stderr: {}",
                args.join(" "),
                output.status.code().unwrap_or(-1),
                stderr(&output)
            );
            let message = stderr(&output);
            assert!(
                message.contains(&format!("{flag} is not a flag of")) && message.contains(args[0]),
                "the refusal must name the command and the flag: {message}"
            );
        }

        // The same flags still work where they mean something.
        let ok = run(&[
            "upscale",
            game.to_str().unwrap(),
            "--method",
            "nearest",
            "--factor",
            "2",
            "-o",
            temp.0.join("up.png").to_str().unwrap(),
        ]);
        assert_eq!(
            ok.status.code(),
            Some(0),
            "stdout: {}\nstderr: {}",
            stdout(&ok),
            stderr(&ok)
        );
    }
}

#[test]
fn the_shell_works_on_a_second_engine_it_was_never_told_about() {
    // The body is supposed to be engine-agnostic: adding an engine should mean
    // adding a crate and a registration line, and nothing else. This test is
    // the evidence — the shell is handed a format family it has no code for,
    // and `detect`, `inspect` and `upscale` have to work anyway.
    let temp = TempDir::new("second-engine");
    let game = temp.0.join("game");
    kintsugi_bsx::fixtures::write_demo_game(&game).unwrap();
    let game_arg = game.to_str().unwrap();

    let detected = run(&["detect", game_arg]);
    assert!(
        detected.status.success(),
        "detect failed: {}",
        stderr(&detected)
    );
    let text = stdout(&detected);
    assert!(text.contains("bsx"), "detect should name the seam: {text}");
    assert!(
        text.contains("●●●"),
        "a release with its own configuration and story file is a certainty: {text}"
    );

    let inspected = run(&["inspect", game_arg]);
    assert!(
        inspected.status.success(),
        "inspect failed: {}",
        stderr(&inspected)
    );
    let text = stdout(&inspected);
    assert!(
        text.contains("BlueGale BSX"),
        "inspect should name the engine: {text}"
    );
    assert!(
        text.contains("graphics/room.bsg"),
        "the archive's entries should be visible under the archive's name: {text}"
    );
    assert!(
        text.contains("no script found"),
        "this seam cannot read BSScript yet and must say so rather than guess: {text}"
    );

    // The point of the whole exercise: a picture out of a 2008 container,
    // through the body's upscaler, at twice the size.
    let output = temp.0.join("room.png");
    let upscaled = run(&[
        "upscale",
        game_arg,
        "graphics/room.bsg",
        "--output",
        output.to_str().unwrap(),
        "--factor",
        "2",
    ]);
    assert!(
        upscaled.status.success(),
        "upscale failed: {}",
        stderr(&upscaled)
    );
    let png = fs::read(&output).expect("upscale should write a picture");
    assert_eq!(
        &png[..8],
        b"\x89PNG\r\n\x1a\n",
        "the output should be a PNG"
    );
    let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    assert_eq!((width, height), (8, 8), "a 4x4 picture at factor 2 is 8x8");
}

#[test]
fn a_script_the_seam_cannot_read_leaves_nothing_behind() {
    let temp = TempDir::new("second-engine-script");
    let game = temp.0.join("game");
    kintsugi_bsx::fixtures::write_demo_game(&game).unwrap();
    let before = listing(&game.join("exe"));
    let out = temp.0.join("translated.bdt");

    let output = run(&[
        "translate",
        game.to_str().unwrap(),
        "--mock",
        "--write-script",
        out.to_str().unwrap(),
    ]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "a seam that cannot read the script is a refusal, not a crash: {}",
        stderr(&output)
    );
    assert!(
        !out.exists(),
        "a refused translation must not leave a file behind"
    );
    assert_eq!(
        listing(&game.join("exe")),
        before,
        "a refused translation must not touch the game"
    );
}
