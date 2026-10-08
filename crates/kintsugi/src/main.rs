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
use kintsugi_core::plugin::{EngineMount, Registry};
use kintsugi_core::runtime::{Event, Host, Interpreter};
use kintsugi_core::script::{ChoiceOption, Command};
use kintsugi_core::vfs::{Vfs, VirtualPath};
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
    match args.first().map(String::as_str) {
        Some("demo") => cmd_demo(&args[1..]),
        Some("detect") => cmd_detect(&args[1..]),
        Some("inspect") => cmd_inspect(&args[1..]),
        Some("play") => cmd_play(&args[1..]),
        Some("upscale") => cmd_upscale(&args[1..]),
        Some("interpolate") => cmd_interpolate(&args[1..]),
        Some("translate") => cmd_translate(&args[1..]),
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
            Err(Failure::Usage(format!(
                "unknown command '{other}'; the commands are demo, detect, inspect, \
                 play, upscale, interpolate, translate, version"
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
    ];

    /// Boolean flags, listed so a typo gets reported instead of ignored.
    const BOOL_FLAGS: &'static [&'static str] =
        &["auto", "mock", "no-play", "only-typed", "help", "version"];

    /// Expand the short spellings this CLI documents.
    fn canonical(name: &str) -> &str {
        match name {
            "o" => "output",
            "h" => "help",
            "V" => "version",
            other => other,
        }
    }

    fn parse(args: &[String]) -> std::result::Result<Self, String> {
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
            if Self::VALUE_FLAGS.contains(&name) {
                let Some(value) = args.get(i + 1) else {
                    return Err(format!("--{name} needs a value"));
                };
                out.flags.insert(name.to_string(), value.clone());
                i += 2;
            } else if Self::BOOL_FLAGS.contains(&name) {
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

fn open_game(dir: &str) -> Result<Vfs> {
    let root = PathBuf::from(dir);
    Vfs::from_directory(&root)
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
fn pick_script(mount: &dyn EngineMount, preferred: Option<&str>) -> Result<VirtualPath> {
    if let Some(name) = preferred {
        let path = VirtualPath::new(name);
        if mount.vfs().exists(&path) {
            return Ok(path);
        }
        return Err(Error::NotFound(format!(
            "script '{name}' not found in the game"
        )));
    }
    let candidates = mount.vfs().find_by_extension(&["bdt"]);
    if let Some(path) = candidates.iter().find(|p| p.as_str() == "story.bdt") {
        return Ok(path.clone());
    }
    candidates
        .first()
        .cloned()
        .ok_or_else(|| Error::unsupported("play", "no .bdt script found in this game"))
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

fn cmd_demo(args: &[String]) -> std::result::Result<(), Failure> {
    let args = Args::parse(args).map_err(Failure::Usage)?;
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
    let title = mount.read_image(&VirtualPath::new("title.bbm"))?;
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
    let args = Args::parse(args).map_err(Failure::Usage)?;
    let dir = args.require_position(0, "game directory")?;
    let vfs = open_game(dir)?;
    print_verdicts(&vfs)?;
    Ok(())
}

fn cmd_inspect(args: &[String]) -> std::result::Result<(), Failure> {
    let args = Args::parse(args).map_err(Failure::Usage)?;
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
    if let Ok(script_path) = pick_script(mount.as_ref(), args.flag("script")) {
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
    let args = Args::parse(args).map_err(Failure::Usage)?;
    let dir = args.require_position(0, "game directory")?;
    let vfs = open_game(dir)?;

    let registry = registry();
    let (_, mount) = registry.mount_best(&vfs)?;
    for note in &mount.info().notes {
        println!("{}", dim(format!("  [mount] {note}")));
    }
    let script_path = pick_script(mount.as_ref(), args.flag("script"))?;
    let script = mount.read_script(&script_path)?;
    for warning in &script.warnings {
        println!("{}", dim(format!("  [warn] {warning}")));
    }
    let mut host = TerminalHost::new(args.on("auto"));
    Interpreter::new(script).run(&mut host)?;
    Ok(())
}

fn cmd_upscale(args: &[String]) -> std::result::Result<(), Failure> {
    let args = Args::parse(args).map_err(Failure::Usage)?;
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
            println!("{}", gold("image assets in this game"));
            for path in mount.vfs().find_by_extension(&["zbm", "bbm", "bmp"]) {
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
    let args = Args::parse(args).map_err(Failure::Usage)?;
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
        candidates = mount
            .vfs()
            .find_by_extension(&["zbm", "bbm", "bmp"])
            .iter()
            .map(|path| path.to_string())
            .collect();
        candidates.sort();
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
    let args = Args::parse(args).map_err(Failure::Usage)?;
    let dir = args.require_position(0, "game directory")?;
    let source_lang = args.flag_or("source", "ja");
    let target_lang = args.flag_or("target", "en");
    let vfs = open_game(dir)?;

    let registry = registry();
    let (_, mount) = registry.mount_best(&vfs)?;
    let script_path = pick_script(mount.as_ref(), args.flag("script"))?;
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
        llm.translate_with_report(&entries, &source_lang, &target_lang)?
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
        std::fs::write(&path, &written.data)
            .map_err(|e| Error::Io(format!("writing {}: {e}", path.display())))?;
        println!(
            "{} {} ({} byte(s), {}/{} changed line(s))",
            gold("repaired script:"),
            path.display(),
            written.data.len(),
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
        "        bilinear | nearest. No ASSET lists candidates.",
    );
    line(&mut text, "");
    line(
        &mut text,
        "  kintsugi interpolate <DIR> [--factor N] [-o DIR]
        Fill the gaps between an image sequence's frames (插帧):
        blend | ... . Factor 2 turns N frames into 2N-1.

  kintsugi translate <DIR> [options]",
    );
    line(
        &mut text,
        "        Translate a script with an LLM (OpenAI-compatible API):",
    );
    line(
        &mut text,
        "          --script PATH        script to translate (default: story.bdt)",
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
