//! 🏺 kintsugi — the hands.
//!
//! The terminal host: assembles the registry (body + every seam compiled
//! in), then plays, inspects, upscales, and translates repaired games.
//! No arguments prints the usage; start with `kintsugi demo`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kintsugi_core::asset::Image;
use kintsugi_core::codec::png::encode_png;
use kintsugi_core::detect::Confidence;
use kintsugi_core::error::{Error, Result};
use kintsugi_core::plugin::{EngineMount, Registry, WrittenFile};
use kintsugi_core::runtime::{Event, Host, Interpreter};
use kintsugi_core::script::{ChoiceOption, Command, Script};
use kintsugi_core::vfs::{MemorySource, Vfs, VirtualPath};
use kintsugi_translate::{
    Glossary, LlmTranslator, MockTranslator, Translator, extract_with_raw, write_jsonl,
};
use kintsugi_video::{
    BlendInterpolator, FrameInterpolator, UpscaleMethod, interpolate_sequence, upscale,
};

/// Why the host stopped.
///
/// A wrong command line and a broken game file are different failures, and
/// they deserve different exit codes: `2` for "you called me wrong", `1` for
/// "the engine refused" — the code a script can branch on.
enum Failure {
    /// The command line itself was wrong.
    Usage(String),
    /// A seam or the body refused the data.
    Engine(Error),
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Failure::Engine(e)
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Usage(message) => write!(f, "{message}"),
            Failure::Engine(e) => write!(f, "{e}"),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (code, message) = match run(&args) {
        Ok(()) => (0, None),
        Err(Failure::Usage(message)) => (2, Some(message)),
        Err(Failure::Engine(e)) => (1, Some(e.to_string())),
    };
    if let Some(message) = message {
        eprintln!("{} kintsugi: {message}", paint("🏺", true));
    }
    std::process::exit(code);
}

fn run(args: &[String]) -> std::result::Result<(), Failure> {
    // `kintsugi translate --help` asked for help, and answering "missing game
    // directory" is answering a question nobody asked. Asking for help wins
    // over doing the work, wherever in the line it appears.
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_usage();
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("kintsugi {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    match args.first().map(String::as_str) {
        Some("demo") => cmd_demo(&args[1..]),
        Some("detect") => cmd_detect(&args[1..]),
        Some("inspect") => cmd_inspect(&args[1..]),
        Some("play") => cmd_play(&args[1..]),
        Some("upscale") => cmd_upscale(&args[1..]),
        Some("interpolate") => cmd_interpolate(&args[1..]),
        Some("translate") => cmd_translate(&args[1..]),
        Some("install") => cmd_install(&args[1..]),
        Some("version") | Some("--version") | Some("-V") => {
            println!("kintsugi {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        // No arguments at all, `help`, `--help`, and `-h` are requests for
        // help, and help is not a failure. Anything else here is a command
        // that does not exist, and exiting 0 would tell a script that its
        // repair was done when nothing ran at all — so a typo is a usage
        // error, and the usage text goes to stderr where a failure's
        // explanation belongs.
        None | Some("help") | Some("--help") | Some("-h") => {
            print_usage();
            Ok(())
        }
        Some(other) => {
            eprint!("{}", usage_text());
            let commands: Vec<&str> = Args::ACCEPTED.iter().map(|(name, _, _)| *name).collect();
            Err(Failure::Usage(format!(
                "unknown command '{other}'; the commands are {}",
                commands.join(", ")
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// Registry: the assembled body + seams. This is the one place a new engine
// crate gets wired in — everything else is engine-agnostic.
// ---------------------------------------------------------------------------

fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register(Arc::new(kintsugi_bluegale::plugin()));
    // Engine two. Nothing else in this file knows what BSX is: the formats,
    // the file names and the pictures live behind these two lines, which is
    // the whole point of the seam.
    registry.register(Arc::new(kintsugi_bsx::plugin()));
    registry
}

// ---------------------------------------------------------------------------
// Argument parsing (hand-rolled: the body stays dependency-free)
// ---------------------------------------------------------------------------

struct Args {
    positional: Vec<String>,
    flags: HashMap<String, String>,
    bools: HashSet<String>,
}

impl Args {
    /// Flags that carry a value. Everything else is a boolean.
    ///
    /// Keeping this list explicit is what stops `translate --mock DIR` from
    /// swallowing `DIR` as the value of `--mock`.
    const VALUE_FLAGS: &'static [&'static str] = &[
        "asset",
        "dir",
        "script",
        "method",
        "factor",
        "output",
        "source",
        "target",
        "api-base",
        "api-key",
        "model",
        "glossary",
        "jsonl-dir",
        "write-script",
        "into",
        "as",
        "batch-size",
    ];

    /// Boolean flags, listed so a typo gets reported instead of ignored.
    const BOOL_FLAGS: &'static [&'static str] =
        &["auto", "mock", "no-play", "only-typed", "help", "version"];

    /// Which flags each command actually uses: `(command, values, booleans)`.
    ///
    /// A flag a command does not use is a mistake, not something to ignore.
    /// `translate --as omake.bdt` used to be accepted and silently dropped —
    /// the flag is `install`'s spelling — which is how a user translates one
    /// script and repairs another while both commands look happy. `--help` and
    /// `--version` are accepted everywhere.
    ///
    /// `help_text_matches_this_table` holds the hand-written help to this list,
    /// so the two cannot drift apart.
    const ACCEPTED: &'static [(
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
    )] = &[
        ("demo", &["dir"], &["auto", "no-play"]),
        ("detect", &[], &[]),
        ("inspect", &["script", "as"], &[]),
        ("play", &["as", "script"], &["auto"]),
        ("upscale", &["asset", "method", "factor", "output"], &[]),
        ("interpolate", &["asset", "factor", "output"], &[]),
        (
            "translate",
            &[
                "as",
                "script",
                "source",
                "target",
                "glossary",
                "jsonl-dir",
                "write-script",
                "api-base",
                "api-key",
                "model",
                "batch-size",
            ],
            &["mock", "only-typed", "no-play", "auto"],
        ),
        ("install", &["as", "script", "into"], &[]),
        ("version", &[], &[]),
        ("help", &[], &[]),
    ];

    /// Flags that name the thing this command works on, so a command can accept
    /// either spelling without accepting a flag it ignores.
    fn accepted(command: &str) -> Option<(&'static [&'static str], &'static [&'static str])> {
        Self::ACCEPTED
            .iter()
            .find(|(name, _, _)| *name == command)
            .map(|(_, values, bools)| (*values, *bools))
    }

    /// Expand the short spellings this CLI documents.
    fn canonical(name: &str) -> &str {
        match name {
            "o" => "output",
            "h" => "help",
            "V" => "version",
            other => other,
        }
    }

    fn parse(command: &str, args: &[String]) -> std::result::Result<Self, String> {
        let mut out = Self {
            positional: Vec::new(),
            flags: HashMap::new(),
            bools: HashSet::new(),
        };
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            // `--long`, or a single-letter `-x` that we actually document;
            // anything else (a path, a negative number) stays positional.
            let short = arg
                .strip_prefix('-')
                .filter(|rest| !rest.is_empty() && rest.len() == 1 && !rest.starts_with('-'));
            let name = match arg.strip_prefix("--") {
                Some(long) => Some(long),
                None => short,
            };
            let Some(name) = name else {
                out.positional.push(arg.clone());
                i += 1;
                continue;
            };
            let name = Self::canonical(name);
            let (values, bools) =
                Self::accepted(command).ok_or_else(|| format!("unknown command '{command}'"))?;
            let accepted_here = name == "help"
                || name == "version"
                || values.contains(&name)
                || bools.contains(&name);
            if Self::VALUE_FLAGS.contains(&name) {
                if !accepted_here {
                    return Err(format!(
                        "--{name} is not a flag of `kintsugi {command}` (try: kintsugi --help)"
                    ));
                }
                let Some(value) = args.get(i + 1) else {
                    return Err(format!("--{name} needs a value"));
                };
                out.flags.insert(name.to_string(), value.clone());
                i += 2;
            } else if Self::BOOL_FLAGS.contains(&name) {
                if !accepted_here {
                    return Err(format!(
                        "--{name} is not a flag of `kintsugi {command}` (try: kintsugi --help)"
                    ));
                }
                out.bools.insert(name.to_string());
                i += 1;
            } else {
                return Err(format!("unknown flag '--{name}' (try: kintsugi --help)"));
            }
        }
        Ok(out)
    }

    fn flag(&self, name: &str) -> Option<&str> {
        self.flags.get(name).map(String::as_str)
    }

    fn flag_or(&self, name: &str, default: &str) -> String {
        self.flag(name).unwrap_or(default).to_string()
    }

    fn on(&self, name: &str) -> bool {
        self.bools.contains(name)
    }

    fn position(&self, index: usize) -> Option<&str> {
        self.positional.get(index).map(String::as_str)
    }

    /// A required positional argument.
    ///
    /// Missing arguments are a *usage* error (exit 2), not an engine refusal
    /// (exit 1): nothing was discovered about the game, because the command to
    /// look at one was never finished. The exit-code table in
    /// `docs/PLATFORMS.md` says so, and a script that treats the two alike
    /// cannot tell "your command was wrong" from "your game file is broken".
    fn require_position(&self, index: usize, what: &str) -> std::result::Result<&str, Failure> {
        self.position(index).ok_or_else(|| {
            Failure::Usage(format!("missing {what} (see `kintsugi --help` for usage)"))
        })
    }
}

/// A whole-number `--factor` within `1..=max`, refused as a *usage* error when
/// it is not one.
///
/// Each command passes its own bound, because the two differ for real reasons.
/// The glazer's is `kintsugi_video::upscale::MAX_FACTOR`, read from there
/// rather than copied so one place still decides how far a picture may be blown
/// up; the interpolator has no fixed bound of its own (four frames at
/// `--factor 400` is a legitimate 1201 frames) and passes `u32::MAX`, leaving
/// the frame budget to refuse the impossible ones by name. What the command
/// line adds is the distinction the exit-code table promises: a user's mistake
/// is a usage error (exit 2), a game fault is an engine refusal (exit 1).
fn parse_factor(text: &str, max: u32) -> std::result::Result<u32, Failure> {
    let factor: u32 = text
        .parse()
        .map_err(|_| Failure::Usage(format!("--factor must be a whole number, not '{text}'")))?;
    if factor == 0 || factor > max {
        // An unbounded command should not answer "between 1 and 4294967295":
        // that is a number, not a bound.
        return Err(Failure::Usage(if max == u32::MAX {
            format!("--factor must be at least 1, not {factor}")
        } else {
            format!("--factor must be between 1 and {max}, not {factor}")
        }));
    }
    Ok(factor)
}

/// Install a repaired script into a copy of the game — never into the game.
///
/// This is the last step of the hero workflow (`translate` writes the patch,
/// `install` puts it somewhere playable), and it is the step where a tool can
/// do the most damage. So it does three things no hand-copy does:
///
/// * it **refuses a patch that does not line up**. The patch is parsed by the
///   same seam that will install it, line by line, and any id where the patch
///   has prose and the game has structure (or the other way round) stops the
///   install. Ids drift when a patch and a game come from different versions,
///   and a repair that lands on the wrong line is worse than no repair.
/// * it **rebuilds rather than overwrites**. The bytes written come from the
///   seam's writer applied to the *copy's own* original, so every line the
///   patch did not translate keeps its exact original bytes — the same
///   guarantee `translate` makes, held at install time.
/// * it **reads the result back**. The copy is mounted again, the script is
///   parsed again, and the number of changed lines is compared with the number
///   of replacements. If they disagree, the install failed and says so.
fn cmd_install(args: &[String]) -> std::result::Result<(), Failure> {
    let args = Args::parse("install", args).map_err(Failure::Usage)?;
    let dir = args.require_position(0, "game directory")?;
    let Some(patch_arg) = args.flag("script") else {
        return Err(Failure::Usage(
            "install needs --script FILE: the repaired script to install \
             (the file `translate --write-script` produced)"
                .into(),
        ));
    };
    let Some(into_arg) = args.flag("into") else {
        return Err(Failure::Usage(
            "install needs --into DIR: the copy to put the repair in".into(),
        ));
    };
    let patch_path = PathBuf::from(patch_arg);
    let into = PathBuf::from(into_arg);

    // The read-only rule, before anything is read: a copy inside the game
    // folder would be the original wearing a hat.
    ensure_outside_game(Path::new(dir), &into, "the repaired copy")?;

    let registry = registry();
    let original_vfs = open_game(dir)?;
    let (_, original_mount) = registry.mount_best(&original_vfs)?;
    // `--as` names the script inside the game; `--script` here is the patch,
    // which is a different thing with a confusingly similar name. Naming the
    // game's script is optional — the seam picks when it can.
    let target = pick_script(original_mount.as_ref(), args.flag("as"))?;

    // `install` copies a game *folder* and puts the repair inside the copy. A
    // disc image is not a folder, and the repair cannot go into the image — but
    // it does not have to: a folder holding the image and the repaired script
    // is a game whose loose script shadows the one in the image, which is how a
    // disc-image repair is mounted at all. Said here, with the path the seam
    // itself named, because the host is not supposed to know what a script in
    // this engine is called.
    if !Path::new(dir).is_dir() {
        return Err(Failure::Engine(Error::unsupported(
            "install",
            format!(
                "'{dir}' is a game image, not a folder: install copies a folder and writes the \
                 repair into the copy. Put the image in a folder of its own with the repaired \
                 script at '{target}' beside it — a loose file shadows the image's own copy, \
                 which is how a disc image is played with a repair — and install into a copy of \
                 that folder instead"
            ),
        )));
    }

    // The script may be a file in the game folder or an entry inside one of its
    // archives, and both are repairable: a seam that serves a script says which
    // files the repair changes. What must never happen is writing a loose copy
    // of a packed script — the archive shadows loose files, so the copy would
    // look repaired and play the original words.
    let original = original_mount.read_script(&target)?;
    let original_bytes = original_mount.vfs().read(&target).map_err(|e| {
        Error::Plugin(format!(
            "'{target}' was chosen as the script but cannot be read back: {e}"
        ))
    })?;
    let patch_bytes = std::fs::read(&patch_path)
        .map_err(|e| Error::Io(format!("reading the patch {}: {e}", patch_path.display())))?;

    // Let the *seam* read the patch: it is the only code that knows what a
    // script in this engine looks like. The overlay shadows the target name
    // with the patch bytes, so `read_script` parses the patch as this game's
    // script would be parsed.
    let mut overlay = open_game(dir)?;
    overlay.push_front(Arc::new(MemorySource::single(
        target.as_str(),
        patch_bytes.clone(),
    )));
    let (_, patch_mount) = registry.mount_best(&overlay)?;
    let patched = patch_mount.read_script(&target)?;

    // Line by line: what would change, and what must not.
    let mut replacements: BTreeMap<usize, String> = BTreeMap::new();
    let mut misfits: Vec<String> = Vec::new();
    for (id, before) in original.commands.iter().enumerate() {
        let Some(after) = patched.commands.get(id) else {
            continue;
        };
        match (command_text(before), command_text(after)) {
            (Some(before), Some(after)) => {
                if before != after {
                    replacements.insert(id, after.into_owned());
                }
            }
            (None, Some(_)) => misfits.push(format!(
                "line {id} is a {} in this game and the patch has prose for it",
                line_kind(before)
            )),
            (Some(_), None) => misfits.push(format!(
                "line {id} is prose in this game and the patch has a {} for it",
                line_kind(after)
            )),
            (None, None) => {}
        }
    }

    // A patch with nothing to change is not a repair, and installing it would
    // produce a copy that is byte-for-byte the original while reporting
    // success. Refuse instead: the user asked for a repair and there is none,
    // which usually means the patch was made from a different game or was
    // already installed in it.
    if replacements.is_empty() {
        return Err(Failure::Engine(Error::unsupported(
            "install",
            format!(
                "the patch has no line that differs from '{target}' in this game, so \
                 installing it would change nothing. Either the patch was made from a \
                 different game, or this game already has it"
            ),
        )));
    }
    if !misfits.is_empty() {
        let shown = misfits
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let more = if misfits.len() > 3 {
            format!(" (and {} more)", misfits.len() - 3)
        } else {
            String::new()
        };
        return Err(Failure::Engine(Error::unsupported(
            "install",
            format!(
                "the patch does not line up with this game: {shown}{more}. \
                 Ids drift when a patch and a game come from different versions, \
                 and a repair that lands on the wrong line is worse than no repair"
            ),
        )));
    }

    // What the original looked like, so that "the original is untouched" can be
    // a measurement rather than a sentence: the folder's contents, and the
    // script's bytes read back through the mount — which for a packed script
    // means re-parsing the archive, so this covers the container too.
    let originals_before = tree_names(Path::new(dir))?;

    // Everything that writes happens inside this closure, and every line of
    // the report is printed after it returns. Two guarantees fall out:
    //
    // * a destination Kintsugi created is **removed again** if the install does
    //   not finish — an error, or a panic from a bug like the one a game with a
    //   `bgm/` directory found — so the folder either holds a complete repair
    //   with a manifest, or it holds what it held before (nothing);
    // * no success is printed before it is true. The report is written from the
    //   finished state, so a half-installed copy can never be described as a
    //   repaired one.
    // The destination's right to be written over, decided before a single byte
    // is copied into it: absent or empty (ours to create), or a previous
    // install of ours that nobody has touched since.
    let previous = prepare_destination(&into)?;

    let installed = writing_into(&previous, &into, || {
        // A folder Kintsugi made and nobody has touched since may be replaced;
        // anything else was already refused by `prepare_destination`.
        if let Some(previous) = &previous {
            println!(
                "{}",
                dim(format!(
                    "replacing this folder's own previous install ({} file(s), every one \
                     unchanged since kintsugi wrote it, patch {})",
                    previous.files.len(),
                    previous.patch_name()
                ))
            );
        }
        let (files, bytes, skipped) = copy_tree(Path::new(dir), &into)?;
        println!(
            "{}",
            dim(format!(
                "copied {files} file(s), {bytes} byte(s), to {}",
                into.display()
            ))
        );
        for warning in &skipped {
            println!("{}", dim(format!("  · {warning}")));
        }

        // The bytes come from the *copy's* own original, through the seam's
        // writer.
        let copy_vfs = open_game(&into.to_string_lossy())?;
        let (_, copy_mount) = registry.mount_best(&copy_vfs)?;
        let written = copy_mount.write_script(&target, &replacements)?;
        // Every file the repair changed, which is the script itself for a loose
        // script and the archive plus its index for a packed one. The seam says
        // which; the host writes them and nothing else.
        write_written_files(&into, &written.files)?;

        // Read it back: the copy is mounted again and the script parsed again,
        // so the report is about the file on disk and not about the bytes in
        // hand.
        let read_back_vfs = open_game(&into.to_string_lossy())?;
        let (_, read_back_mount) = registry.mount_best(&read_back_vfs)?;
        let read_back = read_back_mount.read_script(&target)?;
        let changed = count_changed_lines(&original, &read_back);
        if changed != replacements.len() {
            return Err(Error::unsupported(
                "install",
                format!(
                    "the copy does not read back as the patch wrote it: {changed} line(s) \
                     differ, but {} replacement(s) were applied — the copy at {} is not \
                     what this command claimed to write",
                    replacements.len(),
                    into.display()
                ),
            ));
        }

        // The game it came from is byte-for-byte what it was. Nothing in this
        // command writes there, so this is a check on the tool itself.
        let originals_after = tree_names(Path::new(dir))?;
        let game_again = open_game(dir)?;
        let (_, game_again_mount) = registry.mount_best(&game_again)?;
        let bytes_after = game_again_mount.vfs().read(&target).ok();
        if originals_before != originals_after || Some(&original_bytes) != bytes_after.as_ref() {
            return Err(Error::unsupported(
                "install",
                format!(
                    "the game folder changed while installing a repair: {} was modified or \
                     gained a file. Nothing in this command writes there, so this is a bug \
                     — stop and report it rather than playing the game",
                    dir
                ),
            ));
        }

        // And the record of what was done goes in last, so that a manifest
        // that exists always describes a finished install.
        let manifest = InstallManifest {
            tool: env!("CARGO_PKG_VERSION").to_string(),
            engine: read_back_mount.info().engine.clone(),
            game: absolute(Path::new(dir)),
            script: target.as_str().to_string(),
            patch_size: patch_bytes.len() as u64,
            patch_checksum: fnv1a(&patch_bytes),
            patch_path: absolute(&patch_path),
            files: Vec::new(),
        };
        manifest.recording(&into, &previous)?.write(&into)?;

        Ok(Installed {
            engine: read_back_mount.info().engine.clone(),
            notes: read_back_mount.info().notes.clone(),
            files,
            bytes,
            script: target.as_str().to_string(),
            changed,
            total: original.commands.len(),
            unmatched: written.unmatched.clone(),
            originals: originals_after.len(),
            copy: into.clone(),
        })
    })?;

    installed.report();
    Ok(())
}

/// What a finished install did, so that the report can be printed from the
/// finished state rather than assembled while writing.
struct Installed {
    engine: String,
    notes: Vec<String>,
    files: usize,
    bytes: u64,
    script: String,
    changed: usize,
    total: usize,
    unmatched: Vec<usize>,
    originals: usize,
    copy: PathBuf,
}

impl Installed {
    fn report(&self) {
        let _ = (self.files, self.bytes);
        println!(
            "{}",
            dim(format!("the copy mounts as {}", gold(&self.engine)))
        );
        for note in &self.notes {
            println!("{}", dim(format!("  [mount] {note}")));
        }
        println!(
            "installed {} into the copy: {} of {} line(s) differ from the original",
            gold(self.script.as_str()),
            self.changed,
            self.total
        );
        if !self.unmatched.is_empty() {
            let ids: Vec<String> = self
                .unmatched
                .iter()
                .take(8)
                .map(usize::to_string)
                .collect();
            let more = if self.unmatched.len() > 8 {
                format!(" and {} more", self.unmatched.len() - 8)
            } else {
                String::new()
            };
            println!(
                "{}",
                dim(format!(
                    "  · {} line(s) of the patch have no line in this game to land on \
                     (ids: {}{more})",
                    self.unmatched.len(),
                    ids.join(", ")
                ))
            );
        }
        println!(
            "{}",
            dim(format!(
                "the original game folder is untouched ({} file(s), '{}' byte-identical)",
                self.originals, self.script
            ))
        );
        println!(
            "→ play the repaired copy: {}",
            dim(format!("kintsugi play {} --auto", self.copy.display()))
        );
        println!(
            "{}",
            dim(format!(
                "wrote {}: what this copy is, and every file kintsugi put in it",
                INSTALL_MANIFEST
            ))
        );
    }
}

/// Run the writing half of an install, and undo a destination this command
/// created if it does not finish.
///
/// Catches panics as well as errors on purpose: the writing half is where a bug
/// would otherwise leave a folder that looks like a copy of someone's game and
/// is not a repair — and the next install would then refuse it by name, asking
/// the user to delete a folder they did not create. A destination that already
/// held a previous install is left as it was; its manifest still describes it,
/// and the refusal says so.
fn writing_into<T>(
    previous: &Option<InstallManifest>,
    into: &Path,
    work: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let created_here = previous.is_none();
    let undo = |into: &Path| {
        if created_here {
            // Say it out loud. A folder that was there a moment ago and is gone
            // now is exactly the kind of silence this project does not do — and
            // the user is about to re-run the command, so they should know the
            // destination is free again.
            match std::fs::remove_dir_all(into) {
                Ok(()) => println!(
                    "{}",
                    dim(format!(
                        "removed the half-made copy at {}: an install either finishes or \
                         leaves nothing",
                        into.display()
                    ))
                ),
                Err(e) => println!(
                    "{}",
                    dim(format!(
                        "could not remove the half-made copy at {}: {e} — delete it before \
                         installing again",
                        into.display()
                    ))
                ),
            }
        }
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => {
            undo(into);
            Err(e)
        }
        Err(payload) => {
            undo(into);
            std::panic::resume_unwind(payload)
        }
    }
}

/// The words of a line — the part a translation may replace — or `None` when
/// the line is structure and prose has no business standing there.
fn line_text(command: &Command) -> Option<&str> {
    match command {
        Command::Narration(text) => Some(text),
        Command::Dialogue { text, .. } => Some(text),
        Command::RawLine(text) => Some(text),
        _ => None,
    }
}

/// The translatable text of any command: prose as itself, a choice as its
/// labels joined with newlines — the shape `translate` extracts and
/// `write-script` splits back, one label per line, in order.
fn command_text(command: &Command) -> Option<std::borrow::Cow<'_, str>> {
    match command {
        Command::Choice(options) if !options.is_empty() => Some(std::borrow::Cow::Owned(
            options
                .iter()
                .map(|option| option.label.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )),
        _ => line_text(command).map(std::borrow::Cow::Borrowed),
    }
}

/// What a line is, for a refusal a human can act on.
fn line_kind(command: &Command) -> &'static str {
    match command {
        Command::Narration(_) => "narration line",
        Command::Dialogue { .. } => "dialogue line",
        Command::RawLine(_) => "raw line",
        Command::Label(_) => "label",
        Command::Jump(_) => "jump",
        Command::Choice(_) => "choice",
        Command::SetBackground(_) => "background change",
        Command::ShowCharacter { .. } => "character sprite",
        _ => "engine command",
    }
}

fn count_changed_lines(before: &Script, after: &Script) -> usize {
    before
        .commands
        .iter()
        .enumerate()
        .filter(|(id, command)| match after.commands.get(*id) {
            Some(after) => command_text(command) != command_text(after),
            None => command_text(command).is_some(),
        })
        .count()
}

/// Every file and directory under `root`, as sorted relative paths.
///
/// Used to prove the game folder did not change, so it must not skip anything
/// that a write could plausibly create.
fn tree_names(root: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| Error::Io(format!("reading {}: {e}", dir.display())))?;
        for entry in entries {
            let entry = entry.map_err(|e| Error::Io(format!("reading {}: {e}", dir.display())))?;
            let path = entry.path();
            // Forward slashes, always: a `.kintsugi-install` written on
            // Windows and verified after copying the folder to a Mac must
            // describe the same names, and this string is written into that
            // file. Case is left alone on purpose — on a case-sensitive
            // filesystem `README.TXT` and `readme.txt` are two files.
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            names.push(relative);
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                stack.push(path);
            }
        }
    }
    names.sort();
    Ok(names)
}

/// The file an install leaves in the copy: what this copy is, and every file
/// Kintsugi put in it.
///
/// This is the seam list in the artifact — a user asking "what did the repair
/// tool touch?" should not have to read the tool's source, and a second
/// install should not have to ask a human to delete a folder by hand. Plain
/// text, one record per line, tab-separated, **path last** so that paths with
/// spaces (the common case) parse. The host has no dependencies and this file
/// needs none.
const INSTALL_MANIFEST: &str = ".kintsugi-install";

/// Which manifest format this is. A copy carrying a newer one is not
/// something this binary is allowed to overwrite.
const INSTALL_MANIFEST_VERSION: &str = "1";

/// What an install recorded about itself.
#[derive(Debug, PartialEq, Eq)]
struct InstallManifest {
    tool: String,
    engine: String,
    game: String,
    script: String,
    patch_size: u64,
    patch_checksum: u64,
    patch_path: String,
    /// Files this install wrote: name, size, checksum.
    files: Vec<(String, u64, u64)>,
}

impl InstallManifest {
    fn patch_name(&self) -> String {
        Path::new(&self.patch_path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.patch_path.clone())
    }

    /// Walk the copy, check it against `previous` (if this is a re-install),
    /// and record every file that is now in it.
    ///
    /// Files the previous install wrote that the game folder no longer has are
    /// removed from the copy: they verified byte-for-byte against the manifest
    /// first, so they are Kintsugi's own artifact and not a user's file — and
    /// leaving them would make the next install refuse the folder it just
    /// wrote. The removal is printed, never silent.
    fn recording(mut self, into: &Path, previous: &Option<Self>) -> Result<Self> {
        let mut names = tree_names(into)?;
        names.retain(|name| name != INSTALL_MANIFEST);
        let mut files = Vec::new();
        for name in &names {
            let path = into.join(name);
            if path.is_dir() {
                // The manifest lists *files*. A directory is structure, the
                // game folder's shape rather than something kintsugi wrote, and
                // `fs::read` on one is an error on every platform — which is
                // how a game with a `bgm/` in it found this.
                continue;
            }
            let bytes = std::fs::read(&path)
                .map_err(|e| Error::Io(format!("reading {}: {e}", path.display())))?;
            files.push((name.clone(), bytes.len() as u64, fnv1a(&bytes)));
        }
        if let Some(previous) = previous {
            let now: HashSet<&str> = files.iter().map(|(name, _, _)| name.as_str()).collect();
            for (stale, _, _) in &previous.files {
                if now.contains(stale.as_str()) {
                    continue;
                }
                let path = into.join(stale);
                std::fs::remove_file(&path)
                    .map_err(|e| Error::Io(format!("removing {}: {e}", path.display())))?;
                println!(
                    "{}",
                    dim(format!(
                        "  · removed '{stale}': the previous install wrote it, this game \
                         folder no longer has it, and it verified byte-for-byte before \
                         being removed"
                    ))
                );
            }
        }
        self.files = files;
        Ok(self)
    }

    fn render(&self) -> String {
        let mut text = format!("kintsugi-install\t{INSTALL_MANIFEST_VERSION}\n");
        text.push_str(&format!("tool\t{}\n", self.tool));
        text.push_str(&format!("engine\t{}\n", self.engine));
        text.push_str(&format!("game\t{}\n", self.game));
        text.push_str(&format!("script\t{}\n", self.script));
        text.push_str(&format!(
            "patch\t{}\t{:016x}\t{}\n",
            self.patch_size, self.patch_checksum, self.patch_path
        ));
        for (name, size, checksum) in &self.files {
            text.push_str(&format!("file\t{size}\t{checksum:016x}\t{name}\n"));
        }
        text
    }

    fn parse(text: &str) -> Result<Self> {
        let mut lines = text.lines();
        let header = lines.next().unwrap_or_default();
        let mut header_fields = header.split('\t');
        let (Some("kintsugi-install"), Some(version)) =
            (header_fields.next(), header_fields.next())
        else {
            return Err(Error::unsupported(
                "install",
                format!("'{INSTALL_MANIFEST}' is not a kintsugi install manifest"),
            ));
        };
        if version != INSTALL_MANIFEST_VERSION {
            return Err(Error::unsupported(
                "install",
                format!(
                    "'{INSTALL_MANIFEST}' is version {version} and this kintsugi writes \
                     version {INSTALL_MANIFEST_VERSION}; refusing to write over a copy \
                     made by another version"
                ),
            ));
        }
        let mut manifest = Self {
            tool: String::new(),
            engine: String::new(),
            game: String::new(),
            script: String::new(),
            patch_size: 0,
            patch_checksum: 0,
            patch_path: String::new(),
            files: Vec::new(),
        };
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            match (fields.first().copied(), fields.len()) {
                (Some("tool"), 2) => manifest.tool = fields[1].to_string(),
                (Some("engine"), 2) => manifest.engine = fields[1].to_string(),
                (Some("game"), 2) => manifest.game = fields[1].to_string(),
                (Some("script"), 2) => manifest.script = fields[1].to_string(),
                (Some("patch"), 4) => {
                    manifest.patch_size = parse_field(fields[1], "patch size")?;
                    manifest.patch_checksum = parse_checksum(fields[2])?;
                    manifest.patch_path = fields[3].to_string();
                }
                (Some("file"), 4) => {
                    let size = parse_field(fields[1], "file size")?;
                    let checksum = parse_checksum(fields[2])?;
                    manifest.files.push((fields[3].to_string(), size, checksum));
                }
                (Some(key), _) => {
                    return Err(Error::unsupported(
                        "install",
                        format!(
                            "'{INSTALL_MANIFEST}' has a '{key}' line this kintsugi does not \
                             understand; leaving the folder alone"
                        ),
                    ));
                }
                (None, _) => {}
            }
        }
        Ok(manifest)
    }

    fn write(&self, into: &Path) -> Result<()> {
        let path = into.join(INSTALL_MANIFEST);
        std::fs::write(&path, self.render())
            .map_err(|e| Error::Io(format!("writing {}: {e}", path.display())))
    }
}

fn parse_field(text: &str, what: &str) -> Result<u64> {
    text.parse::<u64>().map_err(|_| {
        Error::unsupported(
            "install",
            format!("'{INSTALL_MANIFEST}' has a {what} that is not a number: '{text}'"),
        )
    })
}

fn parse_checksum(text: &str) -> Result<u64> {
    u64::from_str_radix(text, 16).map_err(|_| {
        Error::unsupported(
            "install",
            format!("'{INSTALL_MANIFEST}' has a checksum that is not hexadecimal: '{text}'"),
        )
    })
}

/// FNV-1a, 64-bit: enough to notice that a file is not the one Kintsugi wrote,
/// which is the whole job. It **identifies, it does not authenticate** — an
/// adversary who wants a changed file to look unchanged can arrange that, and
/// a manifest is not a signed document.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// An absolute path for the record, without asking the filesystem to resolve
/// anything: a manifest written into a copy must still describe the game if
/// the game folder is later moved away.
fn absolute(path: &Path) -> String {
    match std::env::current_dir() {
        Ok(cwd) if path.is_relative() => cwd.join(path).to_string_lossy().into_owned(),
        _ => path.to_string_lossy().into_owned(),
    }
}

/// Decide whether `into` may be written to, and report what is already there.
///
/// Three answers, and the middle one is the point:
///
/// * nothing there → `Ok(None)`, a fresh copy;
/// * a folder Kintsugi wrote, still exactly as it was written (every file it
///   listed is present with the recorded size and checksum, and there is
///   nothing in there it did not write) → `Ok(Some(previous))`, a re-install;
/// * anything else → a refusal that names what it found, because a tool that
///   writes over a folder it did not create cannot tell a stale copy from
///   someone's installation — or from a copy that has save games in it.
fn prepare_destination(into: &Path) -> Result<Option<InstallManifest>> {
    if into.exists() && !into.is_dir() {
        return Err(Error::unsupported(
            "install",
            format!(
                "--into {} is a file, not a folder: the copy is a folder the game can be \
                 played from",
                into.display()
            ),
        ));
    }
    let existing = if into.exists() {
        tree_names(into)?
    } else {
        Vec::new()
    };
    if existing.is_empty() {
        return Ok(None);
    }
    let manifest_path = into.join(INSTALL_MANIFEST);
    if !manifest_path.is_file() {
        return Err(occupied(existing.iter().map(String::as_str)));
    }
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| Error::Io(format!("reading {}: {e}", manifest_path.display())))?;
    let previous = InstallManifest::parse(&text)?;

    // Verify the folder *against* the manifest rather than trusting where the
    // manifest came from: if every file matches what it recorded, then
    // overwriting this folder is exactly the operation it describes.
    let recorded: HashSet<&str> = previous.files.iter().map(|(n, _, _)| n.as_str()).collect();
    let mut foreign: Vec<String> = existing
        .iter()
        .filter(|name| {
            name.as_str() != INSTALL_MANIFEST
                && !recorded.contains(name.as_str())
                // Directories are not foreign. They are implied by the files
                // inside them, and writing over this folder neither removes nor
                // changes one, so an extra empty directory costs the user
                // nothing. A directory holding anything they added is caught
                // through that file, by name.
                && !into.join(name.as_str()).is_dir()
        })
        .cloned()
        .collect();
    let mut changed: Vec<String> = Vec::new();
    for (name, size, checksum) in &previous.files {
        let path = into.join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            changed.push(format!("{name} (missing)"));
            continue;
        };
        if bytes.len() as u64 != *size || fnv1a(&bytes) != *checksum {
            changed.push(format!("{name} (modified since kintsugi wrote it)"));
        }
    }
    foreign.sort();
    changed.sort();
    if !foreign.is_empty() || !changed.is_empty() {
        let mut found = foreign.iter().map(|name| name.as_str()).collect::<Vec<_>>();
        found.extend(changed.iter().map(String::as_str));
        return Err(occupied(found.into_iter()));
    }
    Ok(Some(previous))
}

/// The refusal both "someone else's folder" and "a copy that has been used"
/// produce. The list is the useful part, so it is always shown.
fn occupied<'a>(names: impl Iterator<Item = &'a str>) -> Error {
    let names: Vec<&str> = names.collect();
    let shown: Vec<&str> = names.iter().take(4).copied().collect();
    let more = if names.len() > shown.len() {
        format!(" and {} more", names.len() - shown.len())
    } else {
        String::new()
    };
    Error::unsupported(
        "install",
        format!(
            "the destination already holds {} thing(s) kintsugi did not write or that \
             changed after it did ({}{more}); refusing to write a repaired copy over \
             them — point --into at a new folder, or delete that folder yourself if \
             those files do not matter. If kintsugi itself was interrupted partway \
             through a previous install, that folder is a partial copy and deleting \
             it is the whole repair",
            names.len(),
            shown.join(", ")
        ),
    )
}

/// Copy a directory tree into `to`, creating it.
///
/// The destination's right to be written over is [`prepare_destination`]'s
/// decision, made before this runs. Returns the file count, the bytes copied,
/// and one human-readable line per thing it deliberately left out.
/// Write the files a repair produced into the copy, making the folders they
/// need on the way.
///
/// A repaired script can sit at a path that exists *only* inside a disc image:
/// `exe/bsx.dat` beside the image shadows the copy inside it, which is how a
/// game on a CD is played with a repair at all. The copy of the folder has no
/// `exe/` to write into until this makes one, and an install that failed here
/// would fail on exactly the games that most need repairing.
fn write_written_files(into: &Path, files: &[WrittenFile]) -> Result<()> {
    for file in files {
        let destination = into.join(file.path.as_str());
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Io(format!("creating {}: {e}", parent.display())))?;
        }
        std::fs::write(&destination, &file.data)
            .map_err(|e| Error::Io(format!("writing {}: {e}", destination.display())))?;
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(usize, u64, Vec<String>)> {
    std::fs::create_dir_all(to)
        .map_err(|e| Error::Io(format!("creating {}: {e}", to.display())))?;

    let mut files = 0usize;
    let mut bytes = 0u64;
    let mut skipped = Vec::new();
    let mut stack = vec![(from.to_path_buf(), to.to_path_buf())];
    while let Some((source, destination)) = stack.pop() {
        let entries = std::fs::read_dir(&source)
            .map_err(|e| Error::Io(format!("reading {}: {e}", source.display())))?;
        let mut entries: Vec<_> = entries
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| Error::Io(format!("reading {}: {e}", source.display())))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let target = destination.join(entry.file_name());
            let kind = entry
                .file_type()
                .map_err(|e| Error::Io(format!("reading {}: {e}", path.display())))?;
            if kind.is_symlink() {
                // Following it could copy something outside the game, and
                // recreating it would make the copy depend on a path elsewhere.
                skipped.push(format!(
                    "left out the symlink '{}' (a repair copy holds real files)",
                    path.display()
                ));
            } else if kind.is_dir() {
                std::fs::create_dir_all(&target)
                    .map_err(|e| Error::Io(format!("creating {}: {e}", target.display())))?;
                stack.push((path, target));
            } else {
                let copied = std::fs::copy(&path, &target).map_err(|e| {
                    Error::Io(format!(
                        "copying {} to {}: {e}",
                        path.display(),
                        target.display()
                    ))
                })?;
                files += 1;
                bytes += copied;
            }
        }
    }
    Ok((files, bytes, skipped))
}

// ---------------------------------------------------------------------------
// Terminal dressing
// ---------------------------------------------------------------------------

fn colors_enabled() -> bool {
    std::env::var("NO_COLOR").is_err() && std::env::var("TERM").as_deref() != Ok("dumb")
}

fn paint(text: impl AsRef<str>, gold: bool) -> String {
    let text = text.as_ref();
    if !colors_enabled() {
        return text.to_string();
    }
    if gold {
        format!("\x1b[38;5;178m{text}\x1b[0m")
    } else {
        format!("\x1b[2m{text}\x1b[0m")
    }
}

fn gold(text: impl AsRef<str>) -> String {
    paint(text, true)
}

fn dim(text: impl AsRef<str>) -> String {
    paint(text, false)
}

/// The host that prints a game into a terminal: text as text, scene events
/// as visible bracketed annotations — the repair seams stay on display.
struct TerminalHost {
    auto: bool,
    quiet_events: bool,
}

impl TerminalHost {
    fn new(auto: bool) -> Self {
        Self {
            auto,
            quiet_events: false,
        }
    }

    fn annotation(&self, text: &str) {
        if !self.quiet_events {
            println!("{}", dim(format!("  [{text}]")));
        }
    }
}

impl Host for TerminalHost {
    fn show_text(&mut self, speaker: Option<&str>, text: &str) -> Result<()> {
        match speaker {
            Some(s) => println!("{} {}", gold(format!("【{s}】")), text),
            None => println!("{text}"),
        }
        Ok(())
    }

    fn event(&mut self, event: Event<'_>) -> Result<()> {
        match event {
            Event::Background(name) => self.annotation(&format!("背景 bg: {name}")),
            Event::CharacterShown { id, sprite } => {
                self.annotation(&format!("立绘 show: {id} / {sprite}"))
            }
            Event::CharacterHidden { id } => self.annotation(&format!("立绘 hide: {id}")),
            Event::Music(Some(name)) => self.annotation(&format!("♪ music: {name}")),
            Event::Music(None) => self.annotation("♪ music: stop"),
            Event::Sound(name) => self.annotation(&format!("♪ sfx: {name}")),
            Event::Wait(ms) => {
                self.annotation(&format!("wait {} ms", ms.min(2000)));
                std::thread::sleep(Duration::from_millis(ms.min(2000) as u64));
            }
        }
        Ok(())
    }

    fn choose(&mut self, options: &[ChoiceOption]) -> Result<usize> {
        println!("{}", gold("―― 選択 / choose ――"));
        for (i, option) in options.iter().enumerate() {
            println!("  {}) {}", i + 1, option.label);
        }
        if self.auto {
            println!(
                "{}",
                dim(format!("--auto: picking 1) {}", options[0].label))
            );
            return Ok(0);
        }
        loop {
            print!("{}", dim("> "));
            io::stdout().flush().ok();
            let mut line = String::new();
            io::stdin()
                .lock()
                .read_line(&mut line)
                .map_err(Error::from)?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return Ok(0);
            }
            if let Ok(n) = trimmed.parse::<usize>() {
                if (1..=options.len()).contains(&n) {
                    return Ok(n - 1);
                }
            }
            println!("{}", dim("enter a number from the list (empty = first)"));
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Open whatever the user pointed at: a game folder, or a disc image holding one.
///
/// Old games are handed over as disc images far more often than as folders — the
/// release this engine was measured against only exists as an `.iso` inside an
/// archive — and asking someone to mount a disc before they can run `detect`
/// would be asking them to do the tool's job. A mounted disc still works, since a
/// mount point is a folder.
/// Whether this image's note has still to be printed, and mark it said.
fn noted_note(note: &str) -> bool {
    static SAID: std::sync::OnceLock<std::sync::Mutex<HashSet<String>>> =
        std::sync::OnceLock::new();
    let said = SAID.get_or_init(|| std::sync::Mutex::new(HashSet::new()));
    match said.lock() {
        Ok(mut said) => said.insert(note.to_string()),
        // A poisoned lock means another thread panicked while holding it; the
        // note is worth printing again rather than swallowing.
        Err(_) => true,
    }
}

fn open_game(dir: &str) -> Result<Vfs> {
    let (vfs, notes) = Vfs::open_game(Path::new(dir))?;
    // Said once per note: a command opens the same game several times over
    // (once to read it, once to write into a copy, once to read the copy
    // back), and a note repeated four times is a note nobody reads.
    for note in notes {
        if noted_note(&note) {
            println!("{}", dim(&note));
        }
    }
    Ok(vfs)
}

fn print_verdicts(vfs: &Vfs) -> Result<Vec<(usize, kintsugi_core::detect::Detection)>> {
    let registry = registry();
    let verdicts = registry.detect_all(vfs);
    if verdicts.is_empty() {
        println!("{}", dim("no engine recognizes these files"));
    }
    for (_, detection) in &verdicts {
        let mark = match detection.confidence {
            Confidence::Certain => gold("●●●"),
            Confidence::Likely => gold("●●"),
            Confidence::Possible => paint(" ● ", false),
            Confidence::Unlikely => paint("·  ", false),
        };
        println!(
            "{mark} {} — {}",
            gold(detection.engine),
            dim(&detection.note)
        );
    }
    Ok(verdicts)
}

/// Pick a script to play: `preferred` if given, else `story.bdt`, else the
/// first script-ish file the mounted game exposes.
/// The script a command works on: the one the user named, or the one the seam
/// calls the game's.
///
/// Naming one is checked against the mounted view, so a typo is a `not found`
/// that quotes the name back instead of a repair aimed at nothing. Otherwise
/// the *seam* answers, because which file is a game's script is engine
/// knowledge: the host does not know that BlueGale's is called `story.bdt`, or
/// that scripts have an extension at all. A seam that will not choose (a game
/// with several scripts and no obvious main one) refuses, and the user names
/// one — the alternative is picking one silently and reporting which *after*
/// translating it.
fn pick_script(mount: &dyn EngineMount, named: Option<&str>) -> Result<VirtualPath> {
    match named {
        Some(name) => {
            let path = VirtualPath::new(name);
            if mount.vfs().exists(&path) {
                Ok(path)
            } else {
                Err(Error::NotFound(format!(
                    "script '{name}' not found in the game"
                )))
            }
        }
        None => mount.primary_script(),
    }
}

/// The images a command works on when the user did not name one: every file in
/// the game carrying an extension the *seam* says it can decode, in name order
/// so a frame sequence comes out in the order it was drawn.
fn discover_images(mount: &dyn EngineMount) -> Vec<String> {
    let extensions = mount.image_extensions();
    if extensions.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<String> = mount
        .vfs()
        .find_by_extension(extensions)
        .iter()
        .map(|path| path.to_string())
        .collect();
    found.sort();
    found
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

fn cmd_demo(args: &[String]) -> std::result::Result<(), Failure> {
    let args = Args::parse("demo", args).map_err(Failure::Usage)?;
    let dir = args.flag_or("dir", "demo-game");

    println!("{}", gold("🏺 Kintsugi — 金缮引擎 · demo"));
    println!();

    // 1. Synthesize a tiny BlueGale-style release (no copyrighted bytes).
    let target = Path::new(&dir);
    let written = kintsugi_bluegale::fixtures::write_demo_game(target)?;
    println!(
        "{} synthetic demo game into {}:",
        gold("wrote"),
        dim(target.display().to_string())
    );
    for path in &written {
        println!("  {} {}", dim("·"), path.display());
    }
    println!();

    // 2. Detect.
    println!("{}", gold("detecting engine…"));
    let vfs = open_game(&dir)?;
    print_verdicts(&vfs)?;
    println!();

    // 3. Play.
    if !args.on("no-play") {
        println!("{}", gold("playing story.bdt"));
        println!("{}", dim("  ————————————————————————"));
        let registry = registry();
        let (_, mount) = registry.mount_best(&vfs)?;
        let script_path = pick_script(mount.as_ref(), None)?;
        let script = mount.read_script(&script_path)?;
        let mut host = TerminalHost::new(args.on("auto"));
        Interpreter::new(script).run(&mut host)?;
        println!("{}", dim("  ————————————————————————"));
        println!();
    }

    // 4. Glaze: upscale the title screen with the Anime4K-style preset.
    println!("{}", gold("glazing (upscaling) the title screen…"));
    let registry = registry();
    let (_, mount) = registry.mount_best(&vfs)?;
    let title_path = VirtualPath::new(
        discover_images(mount.as_ref())
            .first()
            .map(String::as_str)
            .unwrap_or("title.bbm"),
    );
    let title = mount.read_image(&title_path)?;
    let upscaled = upscale(&title, 4, UpscaleMethod::Anime4K)?;
    let png = encode_png(&upscaled)?;
    let out = target.join("title-x4-anime4k.png");
    std::fs::write(&out, &png).map_err(|e| Error::Io(format!("writing {}: {e}", out.display())))?;
    println!(
        "  {} {}×{} → {} {}",
        dim("·"),
        title.width,
        title.height,
        gold(format!("{}×{}", upscaled.width, upscaled.height)),
        dim(out.display().to_string())
    );
    println!();
    println!(
        "{}",
        gold("修复完成 — the pot plays again, seams on display.")
    );
    Ok(())
}

fn cmd_detect(args: &[String]) -> std::result::Result<(), Failure> {
    let args = Args::parse("detect", args).map_err(Failure::Usage)?;
    let dir = args.require_position(0, "game directory")?;
    let vfs = open_game(dir)?;
    print_verdicts(&vfs)?;
    Ok(())
}

fn cmd_inspect(args: &[String]) -> std::result::Result<(), Failure> {
    let args = Args::parse("inspect", args).map_err(Failure::Usage)?;
    let dir = args.require_position(0, "game directory")?;
    let vfs = open_game(dir)?;

    println!("{}", gold("detection"));
    print_verdicts(&vfs)?;
    println!();

    let registry = registry();
    let (plugin, mount) = registry.mount_best(&vfs)?;
    println!("{}", gold("mount"));
    println!("  {} {}", dim("engine:"), plugin.metadata().display_name);
    for note in &mount.info().notes {
        println!("  {} {}", dim("note:"), note);
    }
    println!();

    println!("{}", gold("files"));
    let files = mount.vfs().list();
    for (path, size) in &files {
        println!("  {:>10}  {}", dim(size.to_string()), path);
    }
    println!(
        "  {} {} file(s)",
        dim("total:"),
        gold(files.len().to_string())
    );
    println!();

    println!("{}", gold("script preview"));
    if let Ok(script_path) = pick_script(mount.as_ref(), args.flag("script").or(args.flag("as"))) {
        let script = mount.read_script(&script_path)?;
        println!("  {} {}", dim("source:"), script.source);
        if !script.warnings.is_empty() {
            println!("  {}", dim("warnings:"));
            for warning in &script.warnings {
                println!("    {} {}", dim("·"), warning);
            }
        }
        for command in script.commands.iter().take(12) {
            println!("  {} {}", dim("·"), preview(command));
        }
        if script.commands.len() > 12 {
            println!("  {} … {} more", dim("·"), script.commands.len() - 12);
        }
    } else {
        println!("  {}", dim("no script found"));
    }
    Ok(())
}

fn preview(command: &Command) -> String {
    match command {
        Command::Label(name) => format!("label  ${name}"),
        Command::Narration(text) => format!("text    {text}"),
        Command::Dialogue { speaker, text } => match speaker {
            Some(s) => format!("say     [{s}] {text}"),
            None => format!("say     {text}"),
        },
        Command::RawLine(line) => format!("raw     {line}"),
        Command::Jump(target) => format!("jump    →{target}"),
        Command::Choice(options) => {
            let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
            format!("choice  {}", labels.join(" / "))
        }
        other => format!("{other:?}"),
    }
}

fn cmd_play(args: &[String]) -> std::result::Result<(), Failure> {
    let args = Args::parse("play", args).map_err(Failure::Usage)?;
    let dir = args.require_position(0, "game directory")?;
    let vfs = open_game(dir)?;

    let registry = registry();
    let (_, mount) = registry.mount_best(&vfs)?;
    for note in &mount.info().notes {
        println!("{}", dim(format!("  [mount] {note}")));
    }
    let script_path = pick_script(mount.as_ref(), args.flag("script").or(args.flag("as")))?;
    let script = mount.read_script(&script_path)?;
    for warning in &script.warnings {
        println!("{}", dim(format!("  [warn] {warning}")));
    }
    let mut host = TerminalHost::new(args.on("auto"));
    Interpreter::new(script).run(&mut host)?;
    Ok(())
}

fn cmd_upscale(args: &[String]) -> std::result::Result<(), Failure> {
    let args = Args::parse("upscale", args).map_err(Failure::Usage)?;
    let dir = args.require_position(0, "game directory")?;
    let vfs = open_game(dir)?;
    // Both of these are the command line being wrong, not the game being
    // broken: exit 2, like `--faktur 4`, so a script can tell them apart.
    let method_name = args.flag_or("method", "anime4k");
    let method = UpscaleMethod::from_name(&method_name).map_err(|_| {
        Failure::Usage(format!(
            "unknown method '{method_name}'; the methods are anime4k, lanczos3, \
             bicubic, bilinear, nearest"
        ))
    })?;
    // The glazer's bound comes from the glazer, so there is still one place
    // that decides how far a picture may be blown up.
    let factor = parse_factor(
        &args.flag_or("factor", "2"),
        kintsugi_video::upscale::MAX_FACTOR,
    )?;

    let registry = registry();
    let (_, mount) = registry.mount_best(&vfs)?;

    // The asset can be named either way: `upscale DIR title.bbm` or
    // `upscale DIR --asset title.bbm`. With neither, list the candidates.
    let asset = match args.flag("asset").or_else(|| args.position(1)) {
        Some(asset) => asset.to_string(),
        None => {
            let found = discover_images(mount.as_ref());
            if found.is_empty() {
                println!(
                    "{}",
                    dim("this game has no image this seam claims to decode")
                );
                return Ok(());
            }
            println!("{}", gold("image assets in this game"));
            for path in &found {
                println!("  {} {}", dim("·"), path);
            }
            return Ok(());
        }
    };

    let image = mount.read_image(&VirtualPath::new(&asset))?;
    let output = PathBuf::from(args.flag_or("output", "kintsugi-upscaled.png"));
    ensure_outside_game(Path::new(dir), &output, "the glazed image")?;
    let upscaled = upscale(&image, factor, method)?;
    let png = encode_png(&upscaled)?;
    std::fs::write(&output, &png)
        .map_err(|e| Error::Io(format!("writing {}: {e}", output.display())))?;
    println!(
        "{} {} {}×{} → {}×{} ({})",
        gold("glazed:"),
        asset,
        image.width,
        image.height,
        upscaled.width,
        upscaled.height,
        method.as_str()
    );
    println!("  {} {}", dim("→"), output.display());
    Ok(())
}

fn cmd_interpolate(args: &[String]) -> std::result::Result<(), Failure> {
    let args = Args::parse("interpolate", args).map_err(Failure::Usage)?;
    let dir = args.require_position(0, "game directory")?;
    let vfs = open_game(dir)?;
    // Only zero is wrong on its face here. The interpolator has no fixed
    // bound — the frame budget inside the crate refuses the impossible ones by
    // name, with the sequence length in the message, which this command line
    // cannot know before mounting.
    let factor = parse_factor(&args.flag_or("factor", "2"), u32::MAX)?;

    let registry = registry();
    let (_, mount) = registry.mount_best(&vfs)?;

    // Frames come from the engine as images in name order. BlueGale's AMV is
    // a container we do not demux (see docs/RESEARCH-BlueGale.md), so this
    // operates on what the seam can actually hand over: a sequence of image
    // assets, which is exactly what an extracted cutscene is.
    let mut candidates: Vec<String> = Vec::new();
    if let Some(pattern) = args.flag("asset") {
        candidates.push(pattern.to_string());
    } else {
        candidates = discover_images(mount.as_ref());
    }
    if candidates.is_empty() {
        return Err(Failure::Engine(Error::unsupported(
            "interpolate",
            "no image assets to use as frames",
        )));
    }

    let mut frames = Vec::with_capacity(candidates.len());
    let mut used = Vec::with_capacity(candidates.len());
    let mut skipped: Vec<(String, String)> = Vec::new();
    for name in &candidates {
        // A frame that cannot be read is named and skipped, not faked: the
        // alternative is a video with a hole nobody can see.
        match mount.read_image(&VirtualPath::new(name)) {
            Ok(image) => {
                frames.push(image);
                used.push(name.clone());
            }
            Err(error) => skipped.push((name.clone(), error.to_string())),
        }
    }
    if frames.is_empty() {
        return Err(Failure::Engine(Error::unsupported(
            "interpolate",
            "none of the image assets could be read as frames",
        )));
    }

    // A game directory holds images of many sizes, and only same-sized ones
    // can be frames of one sequence; the glaze decides which run that is and
    // names everything it left out (see `kintsugi_video::group_by_size`).
    let named: Vec<(String, Image)> = used.into_iter().zip(frames).collect();
    let (sequence, skipped) = kintsugi_video::group_by_size(&named);
    if sequence.len() < 2 {
        return Err(Failure::Engine(Error::unsupported(
            "interpolate",
            format!(
                "only {} readable image(s) of one size ({}x{}); interpolating needs \
                 at least two, and mixing sizes would invent motion that is not there",
                sequence.len(),
                sequence.first().map(|(_, i)| i.width).unwrap_or(0),
                sequence.first().map(|(_, i)| i.height).unwrap_or(0),
            ),
        )));
    }
    let (group_names, group): (Vec<String>, Vec<Image>) = sequence.into_iter().unzip();

    let interpolator = BlendInterpolator;
    let out_frames = interpolate_sequence(&group, factor, &interpolator)?;
    let output_dir = PathBuf::from(args.flag_or("output", "kintsugi-frames"));
    ensure_outside_game(Path::new(dir), &output_dir, "the interpolated frames")?;
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| Error::Io(format!("creating {}: {e}", output_dir.display())))?;
    for (index, frame) in out_frames.iter().enumerate() {
        let path = output_dir.join(format!("frame_{index:05}.png"));
        let png = encode_png(frame)?;
        std::fs::write(&path, &png)
            .map_err(|e| Error::Io(format!("writing {}: {e}", path.display())))?;
    }
    println!(
        "{} {} frame(s) → {} frame(s) at ×{} ({})",
        gold("interpolated:"),
        group.len(),
        out_frames.len(),
        factor,
        interpolator.name()
    );
    println!(
        "  {} {} ({}x{})",
        dim("sequence:"),
        group_names.join(", "),
        group.first().map(|i| i.width).unwrap_or(0),
        group.first().map(|i| i.height).unwrap_or(0)
    );
    println!("  {} {}", dim("→"), output_dir.display());
    for (name, reason) in &skipped {
        println!("{} {} ({reason})", dim("· not a frame:"), name);
    }
    println!(
        "{}",
        dim("  originals are frames 0, ×, 2×, …; only the gaps between them were filled")
    );
    Ok(())
}

fn cmd_translate(args: &[String]) -> std::result::Result<(), Failure> {
    let args = Args::parse("translate", args).map_err(Failure::Usage)?;
    let dir = args.require_position(0, "game directory")?;
    let source_lang = args.flag_or("source", "ja");
    let target_lang = args.flag_or("target", "en");
    let vfs = open_game(dir)?;

    let registry = registry();
    let (_, mount) = registry.mount_best(&vfs)?;
    let script_path = pick_script(mount.as_ref(), args.flag("script").or(args.flag("as")))?;
    let script = mount.read_script(&script_path)?;

    let entries = if args.on("only-typed") {
        kintsugi_translate::extract(&script)
    } else {
        extract_with_raw(&script)
    };
    if entries.is_empty() {
        return Err(Failure::Engine(Error::unsupported(
            "translate",
            "no translatable lines in this script",
        )));
    }
    if !args.on("only-typed") {
        println!(
            "{}",
            dim(format!(
                "extracted {} line(s), including unclassified raw lines \
                 (ASCII command-like lines are passed through automatically)",
                entries.len()
            ))
        );
    }

    // Optional JSONL of the source lines, for manual translation workflows.
    if let Some(jsonl_dir) = args.flag("jsonl-dir") {
        let dir = PathBuf::from(jsonl_dir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| Error::Io(format!("creating {}: {e}", dir.display())))?;
        let source_path = dir.join(format!("source.{source_lang}.jsonl"));
        write_jsonl(&source_path, &entries)?;
        println!("{} {}", gold("source batch:"), source_path.display());
    }

    // Resume: lines an interrupted run already wrote into the JSONL are not
    // sent again. Each batch is appended as it lands (below), so a run that
    // dies at line 8,000 is a paused run, not a lost one. Ids the current
    // script no longer has are dropped here — a stale file must not leak
    // lines into a different repair.
    let mut resumed: BTreeMap<usize, String> = BTreeMap::new();
    let mut remaining = entries.clone();
    if !args.on("mock")
        && let Some(jsonl_dir) = args.flag("jsonl-dir")
    {
        let progress_path =
            PathBuf::from(jsonl_dir).join(format!("translated.{target_lang}.jsonl"));
        if progress_path.is_file() {
            let current: HashSet<usize> = entries.iter().map(|e| e.id).collect();
            for entry in kintsugi_translate::read_jsonl(&progress_path)? {
                if current.contains(&entry.id) && !resumed.contains_key(&entry.id) {
                    resumed.insert(entry.id, entry.text);
                }
            }
            remaining.retain(|e| !resumed.contains_key(&e.id));
            if !resumed.is_empty() {
                println!(
                    "{}",
                    dim(format!(
                        "resuming: {} line(s) already translated, {} to go",
                        resumed.len(),
                        remaining.len()
                    ))
                );
            }
        }
    }

    let (translated, reports) = if args.on("mock") {
        // The default marker is deliberately CP932-clean: `--mock
        // --write-script` must produce a real, playable patch offline, and a
        // marker the code page cannot hold would make the offline path fail
        // for the wrong reason. KINTSUGI_MOCK_MARKER exists so the refusal
        // path can be demonstrated on purpose (see the README).
        let marker = std::env::var("KINTSUGI_MOCK_MARKER").unwrap_or_else(|_| "mock: ".to_string());
        let mock = MockTranslator::new(marker);
        println!(
            "{} {} → {} via {}",
            gold("translating"),
            source_lang,
            target_lang,
            mock.name()
        );
        let out = mock.translate(&entries, &source_lang, &target_lang)?;
        (out, Vec::new())
    } else {
        let api_base = args
            .flag("api-base")
            .map(str::to_string)
            .or_else(|| std::env::var("KINTSUGI_API_BASE").ok())
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let api_key = args
            .flag("api-key")
            .map(str::to_string)
            .or_else(|| std::env::var("KINTSUGI_API_KEY").ok())
            .or_else(|| std::env::var("OPENAI_API_KEY").ok())
            .unwrap_or_default();
        let model = args
            .flag("model")
            .map(str::to_string)
            .or_else(|| std::env::var("KINTSUGI_MODEL").ok())
            .unwrap_or_else(|| "gpt-4o-mini".to_string());
        let mut llm = LlmTranslator::new(api_base, api_key, model);
        if let Some(size) = args.flag("batch-size") {
            let size: usize = size.parse().map_err(|_| {
                Failure::Usage(format!(
                    "--batch-size takes a number of lines, not '{size}'"
                ))
            })?;
            if size == 0 {
                return Err(Failure::Usage(
                    "--batch-size 0 would send nothing; the smallest batch is 1 line".to_string(),
                ));
            }
            llm = llm.with_batch_size(size);
        }
        if let Some(path) = args.flag("glossary") {
            llm = llm.with_glossary(load_glossary(Path::new(path))?);
        }
        println!(
            "{} {} → {} via {} ({})",
            gold("translating"),
            source_lang,
            target_lang,
            llm.name(),
            llm.model
        );
        let jsonl_dir = args.flag("jsonl-dir").map(PathBuf::from);
        let total = entries.len();
        let done = std::cell::Cell::new(resumed.len());
        let mut on_batch = |chunk: &[kintsugi_translate::TranslationEntry]| -> Result<()> {
            done.set(done.get() + chunk.len());
            println!("{}", dim(format!("  · {}/{total} line(s)", done.get())));
            if let Some(dir) = &jsonl_dir {
                let path = dir.join(format!("translated.{target_lang}.jsonl"));
                kintsugi_translate::append_jsonl(&path, chunk)?;
            }
            Ok(())
        };
        let (new_translated, reports) =
            llm.translate_with_progress(&remaining, &source_lang, &target_lang, &mut on_batch)?;
        // The full list, in the script's order: resumed lines and fresh ones,
        // with any line neither produced kept as its original text.
        let mut by_id = resumed;
        for entry in new_translated {
            by_id.insert(entry.id, entry.text);
        }
        let translated: Vec<kintsugi_translate::TranslationEntry> = entries
            .iter()
            .map(|e| kintsugi_translate::TranslationEntry {
                id: e.id,
                speaker: e.speaker.clone(),
                text: by_id.remove(&e.id).unwrap_or_else(|| e.text.clone()),
            })
            .collect();
        (translated, reports)
    };

    let changed = translated
        .iter()
        .zip(&entries)
        .filter(|(t, e)| t.text != e.text)
        .count();
    for report in &reports {
        println!(
            "  {} batch {}: {} sent, {} translated, {} unchanged{}",
            dim("·"),
            report.batch,
            report.requested,
            report.translated,
            report.unchanged,
            if report.extra_ids > 0 {
                format!(", {} hallucinated id(s) dropped", report.extra_ids)
            } else {
                String::new()
            }
        );
    }
    println!(
        "  {} {changed}/{} line(s) changed",
        dim("total:"),
        entries.len()
    );

    if let Some(jsonl_dir) = args.flag("jsonl-dir") {
        let dir = PathBuf::from(jsonl_dir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| Error::Io(format!("creating {}: {e}", dir.display())))?;
        let out_path = dir.join(format!("translated.{target_lang}.jsonl"));
        write_jsonl(&out_path, &translated)?;
        println!("{} {}", gold("translated batch:"), out_path.display());
    }

    let merged = kintsugi_translate::apply(&script, &translated)?;
    for warning in &merged.warnings {
        println!("{}", dim(format!("  [warn] {warning}")));
    }

    // Write a patch only when asked, and never over the original: the seam
    // returns the bytes, the host decides where they land.
    if let Some(out_path) = args.flag("write-script") {
        let replacements: BTreeMap<usize, String> = translated
            .iter()
            .zip(&entries)
            .filter(|(new, old)| new.text != old.text)
            .map(|(new, old)| (old.id, new.text.clone()))
            .collect();
        let written = mount.write_script(&script_path, &replacements)?;
        let path = PathBuf::from(out_path);
        ensure_outside_game(Path::new(dir), &path, "the repaired script")?;
        // `written.script` rather than the files a game folder needs: this is a
        // patch, and a patch is a script the seam can parse again — even when
        // the script it came from is packed inside an archive, whose repair is
        // two different files (`install` writes those).
        std::fs::write(&path, &written.script)
            .map_err(|e| Error::Io(format!("writing {}: {e}", path.display())))?;
        println!(
            "{} {} ({} byte(s), {}/{} changed line(s))",
            gold("repaired script:"),
            path.display(),
            written.script.len(),
            written.replaced,
            replacements.len()
        );
        if !written.unmatched.is_empty() {
            println!(
                "{}",
                dim(format!(
                    "  [warn] {} translated line(s) had no home in the original \
                     file (ids {:?}); they were NOT written",
                    written.unmatched.len(),
                    written.unmatched
                ))
            );
        }
    }

    if args.on("no-play") {
        return Ok(());
    }
    println!();
    println!("{}", gold("playing the translated script"));
    println!("{}", dim("  ————————————————————————"));
    let mut host = TerminalHost::new(args.on("auto"));
    Interpreter::new(merged).run(&mut host)?;
    println!("{}", dim("  ————————————————————————"));
    Ok(())
}

/// Refuse to write anywhere inside the game directory.
///
/// README.md says "nothing in the original game folder is ever modified —
/// repairs are written beside it". This is the check that makes that sentence
/// true rather than a promise about the user's care. It stops the obvious
/// mistake (`--write-script game/story.bdt`, the very file being read) and the
/// quieter one (`--write-script game/game.snn` — not the script, but still an
/// original), and it keeps every artifact this tool produces — patch, PNG,
/// frame directory — outside the folder the game is installed in.
///
/// What it does not see: a hard link to an original, made outside the folder
/// (`ln game/story.bdt /tmp/copy.bdt` is a second name for the same bytes).
/// Catching that needs file identity — device + inode, or volume + file index
/// — and portable std has no call for it, so a fix would be `#[cfg]` platform
/// code in the shell. Rather than a check that half-works, the hole is written
/// down here and in `docs/PLATFORMS.md`.
fn ensure_outside_game(
    game_dir: &Path,
    out: &Path,
    what: &str,
) -> std::result::Result<(), Failure> {
    let Some(root) = resolve(game_dir) else {
        return Ok(()); // A game directory that cannot be resolved already failed to open.
    };
    let Some(target) = resolve(out) else {
        return Ok(()); // An unusable path will be reported by the write itself.
    };
    if target.starts_with(&root) {
        let suggestion = match root.parent() {
            Some(parent) => format!(
                " Write it outside the folder instead, for example {}.",
                parent
                    .join(out.file_name().unwrap_or(root.as_os_str()))
                    .display()
            ),
            None => String::new(),
        };
        return Err(Failure::Usage(format!(
            "refusing to write {what} inside the game folder: {}\n         \
             The game folder is read-only to this tool, because a repair that \
             edits the game it was hired to save is not a repair.{suggestion}",
            target.display()
        )));
    }
    Ok(())
}

/// The absolute, symlink-free form of a path, whether or not it exists yet.
///
/// `canonicalize` alone is not enough: the patch, the PNG, and the frame folder
/// usually do not exist when they are checked. So the deepest existing ancestor
/// is canonicalized and the rest is appended, which also collapses `a/./b` and
/// resolves a symlinked game folder.
fn resolve(path: &Path) -> Option<PathBuf> {
    let absolute = std::path::absolute(path).ok()?;
    let mut suffix = PathBuf::new();
    let mut probe = absolute.as_path();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(probe) {
            return Some(canonical.join(suffix));
        }
        let name = probe.file_name()?;
        suffix = Path::new(name).join(&suffix);
        probe = probe.parent()?;
    }
}

/// Glossary files: one `source = target` pair per line, `#` comments.
fn load_glossary(path: &Path) -> Result<Glossary> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| Error::Io(format!("reading {}: {e}", path.display())))?;
    let mut glossary = Glossary::new();
    for (number, line) in body.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((source, target)) = line.split_once('=') else {
            return Err(Error::corrupt(
                "glossary",
                format!(
                    "{}: line {}: expected 'source = target'",
                    path.display(),
                    number + 1
                ),
            ));
        };
        glossary = glossary.term(source.trim(), target.trim());
    }
    Ok(glossary)
}

/// The whole help text, in one place, so the same words reach a user who asked
/// for help and a user who mistyped a command.
fn usage_text() -> String {
    let mut text = String::new();
    let line = |text: &mut String, s: &str| {
        text.push_str(s);
        text.push('\n');
    };
    line(
        &mut text,
        &format!(
            "🏺 kintsugi {version} — 金缮引擎 · Repairing old games with gold.",
            version = env!("CARGO_PKG_VERSION")
        ),
    );
    line(&mut text, "");
    line(&mut text, "Usage:");
    line(
        &mut text,
        "  kintsugi demo [--dir DIR] [--no-play] [--auto]",
    );
    line(
        &mut text,
        "        Synthesize a tiny BlueGale-style game, detect, play, and",
    );
    line(
        &mut text,
        "        upscale its title screen (Anime4K-style preset).",
    );
    line(&mut text, "");
    line(&mut text, "  kintsugi detect <DIR>");
    line(
        &mut text,
        "        Ask every registered seam whose engine this is.",
    );
    line(&mut text, "");
    line(&mut text, "  kintsugi inspect <DIR> [--script PATH]");
    line(
        &mut text,
        "        --script, or --as, picks the script to preview.",
    );
    line(
        &mut text,
        "        Detection verdicts, mounted archives, files, script preview.",
    );
    line(&mut text, "");
    line(&mut text, "  kintsugi play <DIR> [--script PATH] [--auto]");
    line(
        &mut text,
        "        Mount the best-matching engine and play a script in the",
    );
    line(
        &mut text,
        "        terminal. --auto answers choices with the first option.",
    );
    line(
        &mut text,
        "        --script and --as name the same thing. With neither, the",
    );
    line(
        &mut text,
        "        engine's main script is used; a game with several scripts",
    );
    line(
        &mut text,
        "        and no obvious main one is a question only you can settle.",
    );
    line(&mut text, "");
    line(
        &mut text,
        "  kintsugi upscale <DIR> [ASSET] [--method M] [--factor N] [-o PNG]",
    );
    line(
        &mut text,
        "        Glaze an image asset: anime4k | lanczos3 | bicubic |",
    );
    line(
        &mut text,
        "        bilinear | nearest. ASSET and --asset NAME are the same",
    );
    line(
        &mut text,
        "        thing; with neither, the candidates are listed. -o and",
    );
    line(&mut text, "        --output name the same file.");
    line(&mut text, "");
    line(
        &mut text,
        "  kintsugi interpolate <DIR> [--asset NAME] [--factor N] [-o DIR]
        Fill the gaps between an image sequence's frames (插帧):
        blend | ... . Factor 2 turns N frames into 2N-1. Frames are
        every image the engine decodes, in name order, unless
        --asset names one.

  kintsugi translate <DIR> [options] [--batch-size N]",
    );
    line(
        &mut text,
        "        Translate a script with an LLM (OpenAI-compatible API):",
    );
    line(
        &mut text,
        "          --script PATH        script to translate, or --as PATH (the same
                               thing). Default: the engine's main script",
    );
    line(&mut text, "          --source ja --target en");
    line(
        &mut text,
        "          --api-base URL       default https://api.openai.com/v1",
    );
    line(
        &mut text,
        "          --api-key KEY        or KINTSUGI_API_KEY / OPENAI_API_KEY",
    );
    line(
        &mut text,
        "          --model M            default gpt-4o-mini",
    );
    line(
        &mut text,
        "          --batch-size N       lines per request, default 40. Smaller
                               batches survive a slow or strict gateway",
    );
    line(
        &mut text,
        "          --glossary FILE      'source = target' lines, # comments",
    );
    line(
        &mut text,
        "          --jsonl-dir DIR      also write source/translated JSONL",
    );
    line(
        &mut text,
        "          --write-script PATH  write the repaired script outside <DIR>",
    );
    line(
        &mut text,
        "          --only-typed         skip unclassified raw lines",
    );
    line(
        &mut text,
        "          --mock               offline dry run, no network",
    );
    line(
        &mut text,
        "                               (KINTSUGI_MOCK_MARKER sets its prefix)",
    );
    line(
        &mut text,
        "          --no-play            don't play the result",
    );
    line(&mut text, "");
    line(
        &mut text,
        "  kintsugi install <DIR> --script FILE --into COPY [--as NAME]",
    );
    line(
        &mut text,
        "        Put a repaired script into a copy of the game (never into",
    );
    line(
        &mut text,
        "        the game), then mount the copy and read the script back to",
    );
    line(
        &mut text,
        "        prove it landed. Refuses a patch whose lines do not line up",
    );
    line(&mut text, "        with this game's lines.");
    line(&mut text, "");
    line(&mut text, "  kintsugi version");
    line(&mut text, "");
    line(
        &mut text,
        &dim(
            "<DIR> is read-only: the game folder is never written to — not by
--write-script, not by upscale -o, not by interpolate -o. Every
artifact lands outside it, and installing a patch is your decision.
The originals are never modified. All repairs stay visible. 以金缮之艺，续老游戏之命。",
        ),
    );
    text
}

fn print_usage() {
    print!("{}", usage_text());
}

#[cfg(test)]
mod tests {
    /// A repair can name a path that exists only inside a disc image. Writing
    /// it into a copy of the folder used to fail with `No such file or
    /// directory`, and the install cleaned up after itself — so the games that
    /// most need a repair were the ones it refused.
    #[test]
    fn a_repair_writes_into_a_folder_the_copy_does_not_have_yet() {
        let into =
            std::env::temp_dir().join(format!("kintsugi-install-parents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&into);
        std::fs::create_dir_all(&into).unwrap();
        let files = vec![
            WrittenFile {
                path: kintsugi_core::vfs::VirtualPath::new("exe/bsx.dat"),
                data: b"a story where the image keeps its own".to_vec(),
            },
            WrittenFile {
                path: kintsugi_core::vfs::VirtualPath::new("story.bdt"),
                data: b"a loose script in the folder itself".to_vec(),
            },
        ];
        write_written_files(&into, &files).expect("a repair makes the folders it needs");
        assert_eq!(
            std::fs::read(into.join("exe/bsx.dat")).unwrap(),
            b"a story where the image keeps its own"
        );
        assert_eq!(
            std::fs::read(into.join("story.bdt")).unwrap(),
            b"a loose script in the folder itself"
        );
        let _ = std::fs::remove_dir_all(&into);
    }

    use super::*;

    /// Every flag the help text offers must be a flag of at least one command,
    /// and every flag a command accepts must be in the help.
    ///
    /// The table and the prose are two halves of one promise to the user, and
    /// the failure they guard against is the quiet kind: a flag the help
    /// documents and nothing accepts, or a command that takes a flag the help
    /// never mentions. `--as` was in neither list for `translate` and silently
    /// ignored, which is exactly what this test rules out.
    #[test]
    fn help_text_matches_this_table() {
        let help = usage_text();
        let universal = ["help", "version"];
        for (command, values, bools) in Args::ACCEPTED {
            for flag in values.iter().chain(bools.iter()) {
                assert!(
                    help.contains(&format!("--{flag}")),
                    "`kintsugi {command}` accepts --{flag}, which the help text never mentions"
                );
            }
        }
        // Every `--flag` the help names, in both the usage lines and the prose.
        for token in help.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')') {
            let Some(flag) = token.strip_prefix("--") else {
                continue;
            };
            let flag = flag.trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
            if flag.is_empty() || flag.contains('=') {
                continue;
            }
            let known = Args::VALUE_FLAGS.contains(&flag)
                || Args::BOOL_FLAGS.contains(&flag)
                || universal.contains(&flag);
            assert!(
                known,
                "the help text offers --{flag}, which no command accepts: either add it to \
                 Args::ACCEPTED or stop documenting it"
            );
        }
    }

    /// Every command in the dispatch table parses its own flags, and a command
    /// cannot be dispatched without one.
    #[test]
    fn every_command_is_in_the_flag_table() {
        for command in [
            "demo",
            "detect",
            "inspect",
            "play",
            "upscale",
            "interpolate",
            "translate",
            "install",
            "version",
            "help",
        ] {
            assert!(
                Args::accepted(command).is_some(),
                "`kintsugi {command}` is dispatched but has no entry in Args::ACCEPTED"
            );
        }
    }
}
