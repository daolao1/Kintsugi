//! 🏺 kintsugi-desktop — the window.
//!
//! The second shell: same registry, same mounts, same interpreter as the
//! terminal, different `Host`. Rule four is the whole design: nothing in
//! this crate knows what a BSX archive is.
//!
//! ```text
//! kintsugi-desktop <GAME> [--script NAME]
//! kintsugi-desktop <GAME> --dump-frames <DIR> [--max-frames N]
//! ```
//!
//! The dump writes the frames the window would show, one PNG per presented
//! line, so the shell can be verified on a machine without a screen (CI,
//! this one included when the seat is headless).

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use kintsugi_core::asset::{Image, ImageFormat};
use kintsugi_core::codec::png::encode_png;
use kintsugi_core::plugin::Registry;
use kintsugi_core::runtime::Interpreter;
use kintsugi_core::vfs::Vfs;
use kintsugi_desktop::font::Pen;
use kintsugi_desktop::render::{TEXT_MARGIN, render};
use kintsugi_desktop::state::{DumpHost, GameState, prepare_dump_dir, read_script};

mod window;

const USAGE: &str = "\
🏺 kintsugi-desktop — the window

  kintsugi-desktop <GAME> [--script NAME]
      Open the game in a window. Click, Space or Enter advances; 1–9 and the
      arrow keys answer choices; Esc closes the book.

  kintsugi-desktop <GAME> --dump-frames <DIR> [--max-frames N]
      No window: write the frames the window would show as PNGs into <DIR>
      (default 60 frames; --max-frames raises or lowers the cap). This is
      how the shell is verified where no screen exists.

  KINTSUGI_FONT names a font file to draw text with when the system's own
  search list finds nothing.

The game is never written to. Gold seams stay on screen while a seam only
partly understands its game.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") || args.is_empty() {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("🏺 kintsugi-desktop: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let mut game: Option<String> = None;
    let mut script_name: Option<String> = None;
    let mut dump_dir: Option<String> = None;
    let mut max_frames: usize = 60;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--script" => {
                script_name = Some(args.get(index + 1).ok_or("--script takes a name")?.clone());
                index += 1;
            }
            "--dump-frames" => {
                dump_dir = Some(
                    args.get(index + 1)
                        .ok_or("--dump-frames takes a directory")?
                        .clone(),
                );
                index += 1;
            }
            "--max-frames" => {
                let raw = args.get(index + 1).ok_or("--max-frames takes a count")?;
                max_frames = raw
                    .parse()
                    .map_err(|_| format!("--max-frames takes a number, not '{raw}'"))?;
                index += 1;
            }
            other if other.starts_with("--") => {
                return Err(format!(
                    "unknown flag '{other}' (try: kintsugi-desktop --help)"
                ));
            }
            positional => {
                if game.replace(positional.to_string()).is_some() {
                    return Err("one game at a time".to_string());
                }
            }
        }
        index += 1;
    }
    let game = game.ok_or("name the game's folder or disc image")?;

    let (vfs, notes) = Vfs::open_game(Path::new(&game)).map_err(|e| e.to_string())?;
    for note in &notes {
        println!("  · {note}");
    }

    // The same registry the terminal assembles: body plus every seam
    // compiled in. Nothing below this line knows what the seams know.
    let mut registry = Registry::new();
    registry.register(Arc::new(kintsugi_bluegale::plugin()));
    registry.register(Arc::new(kintsugi_bsx::plugin()));
    let (_, mount) = registry.mount_best(&vfs).map_err(|e| e.to_string())?;
    for note in &mount.info().notes {
        println!("  · {note}");
    }

    let script = read_script(mount.as_ref(), script_name.as_deref()).map_err(|e| e.to_string())?;
    for warning in &script.warnings {
        println!("  · [warn] {warning}");
    }

    match dump_dir {
        Some(dir) => dump_frames(
            mount.as_ref(),
            script,
            Path::new(&dir),
            Path::new(&game),
            max_frames,
        )
        .map_err(|e| e.to_string()),
        None => window::run(mount.as_ref(), script, notes).map_err(|e| e.to_string()),
    }
}

/// The headless twin: run the story with the dump host, rendering and
/// writing each presented frame until the cap. A dump that hit the cap says
/// so — a frame count that silently stopped early would be a lie of
/// completeness.
fn dump_frames(
    mount: &dyn kintsugi_core::plugin::EngineMount,
    script: kintsugi_core::script::Script,
    dir: &Path,
    game: &Path,
    max_frames: usize,
) -> kintsugi_core::Result<()> {
    prepare_dump_dir(dir, game)?;
    let pen = Pen::load(20.0);
    if pen.is_none() {
        println!("  · no usable system font found; frames will carry the seam note only");
    }

    let mut written = 0usize;
    let mut seen = 0usize;
    let result = {
        let mut host = DumpHost::new(mount, |state: &GameState| {
            seen += 1;
            if written >= max_frames {
                return;
            }
            let frame = render_state(state, pen.as_ref(), 960, 720);
            let png = match encode_png(&frame_to_image(&frame)) {
                Ok(png) => png,
                Err(error) => {
                    eprintln!("  · frame {seen}: could not encode: {error}");
                    return;
                }
            };
            let path = dir.join(format!("frame-{:04}.png", written + 1));
            match std::fs::write(&path, &png) {
                Ok(()) => written += 1,
                Err(error) => eprintln!(
                    "  · frame {seen}: could not write {}: {error}",
                    path.display()
                ),
            }
        });
        Interpreter::new(script).run(&mut host)
    };
    result?;
    println!(
        "🏺 {} frame(s) written to {} ({} line(s) presented{})",
        written,
        dir.display(),
        seen,
        if seen > written {
            format!("; capped at --max-frames {max_frames}")
        } else {
            String::new()
        }
    );
    Ok(())
}

/// Render one presented line the way the window would.
fn render_state(
    state: &GameState,
    pen: Option<&Pen>,
    width: u32,
    height: u32,
) -> kintsugi_desktop::render::Frame {
    let mut scene = kintsugi_desktop::render::Scene {
        background: state.background.as_deref(),
        characters: state
            .characters
            .iter()
            .map(|(_, image)| image.as_ref())
            .collect(),
        speaker: state.speaker.as_deref(),
        lines: Vec::new(),
        seam_note: state.seam_note.as_deref(),
    };
    match pen {
        Some(pen) => {
            // The dump frame is the design size: scale 1, the window at its
            // honest density does the glazing.

            let max_width = width.saturating_sub(TEXT_MARGIN * 2);
            scene.lines = pen.wrap(&state.text, max_width);
            render(
                &scene,
                width,
                height,
                pen.line_height(),
                1,
                |frame, x, y, text, colour| pen.draw(frame, x, y, text, colour),
            )
        }
        None => render(&scene, width, height, 26, 1, |_, _, _, _, _| {}),
    }
}

/// The PNG encoder speaks `Image`; the renderer speaks `Frame`. Convert.
fn frame_to_image(frame: &kintsugi_desktop::render::Frame) -> Image {
    let mut data = Vec::with_capacity((frame.width * frame.height * 4) as usize);
    for &px in &frame.pixels {
        data.push(((px >> 16) & 0xFF) as u8);
        data.push(((px >> 8) & 0xFF) as u8);
        data.push((px & 0xFF) as u8);
        data.push((px >> 24) as u8);
    }
    Image {
        width: frame.width,
        height: frame.height,
        format: ImageFormat::Rgba8,
        data,
    }
}
