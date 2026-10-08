//! 🏺 Kintsugi on Android (and anywhere else with a C ABI).
//!
//! The body, the golden seams, and the glaze are the same crates the desktop
//! host uses: nothing here knows about Android beyond how a string crosses
//! the boundary. What changes per platform is the [`Host`] — the desktop host
//! prints to a terminal, this one records a transcript for the UI to draw,
//! and a future iOS or web host would do something else again.
//!
//! Two surfaces are exported:
//!
//! * a **plain C ABI** ([`kintsugi_detect`], [`kintsugi_play`],
//!   [`kintsugi_upscale`], [`kintsugi_free`]) that any language can call —
//!   verified on the host with `tests/c_abi.c`;
//! * a **JNI bridge** behind the `jni-bridge` feature, which is what the
//!   Kotlin app in `platforms/android/` actually talks to.
//!
//! # Rules this crate keeps
//!
//! * Every returned string is owned by the *caller* and released with
//!   [`kintsugi_free`]: Rust allocated it, Rust must free it, and the same
//!   allocator has to be the one that does it.
//! * No panic may cross the boundary. Every entry point catches unwinds and
//!   turns them into an error string, because unwinding into a JVM or a C
//!   caller is undefined behaviour.
//! * Failures are reported as text, not as silent `NULL`: the UI can show a
//!   repair that failed, which is more honest than a blank screen.

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;

use kintsugi_core::codec::png::encode_png;
use kintsugi_core::error::{Error, Result};
use kintsugi_core::plugin::{EngineMount, Registry};
use kintsugi_core::runtime::{Event, Host, Interpreter};
use kintsugi_core::script::ChoiceOption;
use kintsugi_core::vfs::{Vfs, VirtualPath};
use kintsugi_video::upscale::{UpscaleMethod, upscale};

/// The registry of golden seams this build carries.
///
/// Same wiring as the CLI host: adding an engine crate is a one-line change
/// here, and Android gains that engine with no Kotlin involved.
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register(Arc::new(kintsugi_bluegale::plugin()));
    registry
}

/// A [`Host`] that records the game's requests instead of drawing them.
///
/// Choices are answered with the first option and the transcript says so:
/// the app shows which branch it took rather than pretending the player
/// chose. An interactive host is a matter of sending a question back over
/// the bridge, not of changing the engine.
#[derive(Debug, Default)]
pub struct TranscriptHost {
    lines: Vec<String>,
    /// Set when a choice was auto-answered, so the UI can show the seam.
    auto_answered: usize,
}

impl TranscriptHost {
    /// A host that answers choices with the first option.
    pub fn new() -> Self {
        Self::default()
    }

    /// The recorded transcript, one line per event.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// How many choices were answered automatically.
    pub fn auto_answered(&self) -> usize {
        self.auto_answered
    }

    fn push(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }
}

impl Host for TranscriptHost {
    fn show_text(&mut self, speaker: Option<&str>, text: &str) -> Result<()> {
        match speaker {
            Some(speaker) => self.push(format!("【{speaker}】{text}")),
            None => self.push(text.to_string()),
        }
        Ok(())
    }

    fn event(&mut self, event: Event<'_>) -> Result<()> {
        match event {
            Event::Background(name) => self.push(format!("[背景] {name}")),
            Event::CharacterShown { id, sprite } => {
                self.push(format!("[出场] {id} ({sprite})"));
            }
            Event::CharacterHidden { id } => self.push(format!("[退场] {id}")),
            Event::Music(Some(track)) => self.push(format!("[音乐] {track}")),
            Event::Music(None) => self.push("[音乐] 停止".to_string()),
            Event::Sound(name) => self.push(format!("[音效] {name}")),
            Event::Wait(ms) => self.push(format!("[等待] {ms}ms")),
        }
        Ok(())
    }

    fn choose(&mut self, options: &[ChoiceOption]) -> Result<usize> {
        self.auto_answered += 1;
        let labels: Vec<&str> = options.iter().map(|option| option.label.as_str()).collect();
        self.push(format!("[选择] {} ⇒ {}", labels.join(" / "), labels[0]));
        Ok(0)
    }
}

/// Mount the best-matching seam for a game directory.
fn mount_game(dir: &str) -> Result<(Registry, Box<dyn EngineMount>, Vfs)> {
    let vfs = Vfs::from_directory(Path::new(dir))?;
    let registry = registry();
    let (_, mount) = registry.mount_best(&vfs)?;
    Ok((registry, mount, vfs))
}

/// Detection verdicts for a game directory, one line per seam.
pub fn detect(dir: &str) -> Result<String> {
    let vfs = Vfs::from_directory(Path::new(dir))?;
    let registry = registry();
    let detections = registry.detect_all(&vfs);
    if detections.is_empty() {
        return Ok("no seam recognized this directory".to_string());
    }
    let mut out = String::new();
    // `detect_all` yields `(rank, verdict)` in the registry's own order.
    for (_, detection) in detections {
        out.push_str(&format!(
            "{} {} — {}\n",
            detection.confidence.label(),
            detection.engine,
            detection.note
        ));
    }
    Ok(out)
}

/// Mount a game and play one script, returning the transcript.
///
/// Failing to *mount* is an error the caller sees; failing to *interpret* is
/// not, because a game that stops halfway is still worth showing. Such a stop
/// is appended to the transcript as a `stopped:` line — the seam stays
/// visible instead of the screen silently going blank.
pub fn play(dir: &str, script: Option<&str>) -> Result<String> {
    let (_, mount, _) = mount_game(dir)?;
    let mut out = String::new();
    out.push_str(&format!("engine: {}\n", mount.info().engine));
    for note in &mount.info().notes {
        out.push_str(&format!("mount: {note}\n"));
    }

    let script_path = match script {
        Some(path) => VirtualPath::new(path),
        None => pick_script(&*mount)?,
    };
    let script = mount.read_script(&script_path)?;
    for warning in &script.warnings {
        out.push_str(&format!("warning: {warning}\n"));
    }

    let mut host = TranscriptHost::new();
    let stopped = Interpreter::new(script).run(&mut host).err();
    for line in host.lines() {
        out.push_str(line);
        out.push('\n');
    }
    if let Some(error) = stopped {
        out.push_str(&format!("stopped: {error}\n"));
    }
    if host.auto_answered() > 0 {
        out.push_str(&format!(
            "({} choice(s) auto-answered with the first option)\n",
            host.auto_answered()
        ));
    }
    Ok(out)
}

/// Choose a script when the caller did not name one.
fn pick_script(mount: &dyn EngineMount) -> Result<VirtualPath> {
    let candidates = mount.vfs().find_by_extension(&["bdt", "txt", "ks"]);
    candidates
        .into_iter()
        .next()
        .ok_or_else(|| Error::NotFound("no script file in this game".to_string()))
}

/// Synthesize the demo game, then detect, play, and glaze it.
///
/// The Android app's "try it" button: a real end-to-end run with no game
/// files of the user's own, and no copyrighted bytes anywhere.
pub fn demo(dir: &str) -> Result<String> {
    kintsugi_bluegale::fixtures::write_demo_game(Path::new(dir))?;
    let mut out = String::new();
    out.push_str(&format!("demo game written to {dir}\n\n"));
    out.push_str("detection\n");
    out.push_str(&detect(dir)?);
    out.push('\n');
    out.push_str("playing story.bdt\n");
    out.push_str(&play(dir, Some("story.bdt"))?);
    out.push('\n');
    let glazed = upscale_asset(
        dir,
        "title.bbm",
        4,
        "anime4k",
        &format!("{dir}/title-x4.png"),
    )?;
    out.push_str(&glazed);
    Ok(out)
}

/// Glaze (upscale) one image asset and write it as a PNG.
pub fn upscale_asset(
    dir: &str,
    asset: &str,
    factor: u32,
    method: &str,
    out_path: &str,
) -> Result<String> {
    let (_, mount, _) = mount_game(dir)?;
    let method = UpscaleMethod::from_name(method)?;
    let image = mount.read_image(&VirtualPath::new(asset))?;
    let upscaled = upscale(&image, factor, method)?;
    let png = encode_png(&upscaled)?;
    std::fs::write(out_path, &png).map_err(|e| Error::Io(format!("writing {out_path}: {e}")))?;
    Ok(format!(
        "glazed {asset}: {}×{} → {}×{} ({}) → {out_path}\n",
        image.width,
        image.height,
        upscaled.width,
        upscaled.height,
        method.as_str()
    ))
}

// ---------------------------------------------------------------------------
// The C ABI. Every entry point is total: it either returns a NUL-terminated
// string the caller owns, or NULL when even the error could not be built.
// ---------------------------------------------------------------------------

/// Run `body` with unwinding turned into an error string.
///
/// Returns an owned pointer, or NULL if the error message itself could not be
/// allocated. A panic that escapes into a JVM is undefined behaviour, so it
/// stops here.
fn guard(body: impl FnOnce() -> Result<String>) -> *mut c_char {
    let outcome = catch_unwind(AssertUnwindSafe(body));
    let message = match outcome {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => format!("error: {e}"),
        Err(_) => "error: the engine panicked; this is a bug, and the seam \
                   that caused it should be reported"
            .to_string(),
    };
    match CString::new(message) {
        Ok(text) => text.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Borrow a C string as `&str`.
///
/// # Safety
/// `text` must be NULL or a valid NUL-terminated string.
unsafe fn borrow<'a>(text: *const c_char) -> Result<&'a str> {
    if text.is_null() {
        return Err(Error::Plugin("null string argument".to_string()));
    }
    // SAFETY: the caller promises a NUL-terminated string.
    let raw = unsafe { CStr::from_ptr(text) };
    raw.to_str()
        .map_err(|e| Error::Plugin(format!("argument is not UTF-8: {e}")))
}

/// The engine version, as a string the caller frees.
///
/// # Safety
/// The returned pointer must be released with [`kintsugi_free`].
#[unsafe(no_mangle)]
pub extern "C" fn kintsugi_version() -> *mut c_char {
    guard(|| Ok(format!("kintsugi {}", env!("CARGO_PKG_VERSION"))))
}

/// Detection verdicts for a game directory.
///
/// # Safety
/// `dir` must be a valid NUL-terminated UTF-8 path; the result must be freed
/// with [`kintsugi_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kintsugi_detect(dir: *const c_char) -> *mut c_char {
    let dir = unsafe { borrow(dir) };
    guard(move || detect(dir?))
}

/// Play a script (or `script = NULL` for the first one found) and return the
/// transcript.
///
/// # Safety
/// Both arguments must be NULL or valid NUL-terminated UTF-8 strings; the
/// result must be freed with [`kintsugi_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kintsugi_play(dir: *const c_char, script: *const c_char) -> *mut c_char {
    let dir = unsafe { borrow(dir) };
    let script = if script.is_null() {
        Ok(None)
    } else {
        unsafe { borrow(script) }.map(Some)
    };
    guard(move || play(dir?, script?))
}

/// Write the synthetic demo game into `dir` and run it end to end.
///
/// # Safety
/// `dir` must be a valid NUL-terminated UTF-8 path the process may write to;
/// the result must be freed with [`kintsugi_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kintsugi_demo(dir: *const c_char) -> *mut c_char {
    let dir = unsafe { borrow(dir) };
    guard(move || demo(dir?))
}

/// Glaze one image asset into a PNG file.
///
/// # Safety
/// All pointers must be NULL or valid NUL-terminated UTF-8 strings; the
/// result must be freed with [`kintsugi_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kintsugi_upscale(
    dir: *const c_char,
    asset: *const c_char,
    factor: u32,
    method: *const c_char,
    out_path: *const c_char,
) -> *mut c_char {
    let dir = unsafe { borrow(dir) };
    let asset = unsafe { borrow(asset) };
    let method = unsafe { borrow(method) };
    let out_path = unsafe { borrow(out_path) };
    guard(move || upscale_asset(dir?, asset?, factor, method?, out_path?))
}

/// Release a string this library returned.
///
/// # Safety
/// `text` must be a pointer returned by this library and not yet freed, or
/// NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kintsugi_free(text: *mut c_char) {
    if text.is_null() {
        return;
    }
    // SAFETY: the pointer came from `CString::into_raw` in this library, so
    // taking it back is the matching operation.
    drop(unsafe { CString::from_raw(text) });
}

#[cfg(feature = "jni-bridge")]
mod jni_bridge;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A scratch directory that cleans itself up.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("kintsugi-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &str {
            self.0.to_str().unwrap()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn transcript_host_records_every_event() {
        let mut host = TranscriptHost::new();
        host.show_text(None, "a line").unwrap();
        host.show_text(Some("藍子"), "a spoken line").unwrap();
        host.event(Event::Background("room.zbm")).unwrap();
        host.event(Event::CharacterShown {
            id: "aiko",
            sprite: "face.zbm",
        })
        .unwrap();
        host.event(Event::Music(Some("opening.ogg"))).unwrap();
        host.event(Event::Sound("seam.ogg")).unwrap();
        host.event(Event::Wait(500)).unwrap();
        host.event(Event::CharacterHidden { id: "aiko" }).unwrap();
        host.event(Event::Music(None)).unwrap();
        let picked = host
            .choose(&[
                ChoiceOption {
                    label: "repair".into(),
                    goto: "repair".into(),
                },
                ChoiceOption {
                    label: "hide".into(),
                    goto: "hide".into(),
                },
            ])
            .unwrap();
        assert_eq!(picked, 0);
        assert_eq!(host.auto_answered(), 1);
        assert_eq!(
            host.lines(),
            [
                "a line",
                "【藍子】a spoken line",
                "[背景] room.zbm",
                "[出场] aiko (face.zbm)",
                "[音乐] opening.ogg",
                "[音效] seam.ogg",
                "[等待] 500ms",
                "[退场] aiko",
                "[音乐] 停止",
                "[选择] repair / hide ⇒ repair",
            ]
        );
    }

    #[test]
    fn demo_runs_end_to_end_through_the_bridge() {
        let scratch = Scratch::new("demo");
        let out = demo(scratch.path()).unwrap();
        assert!(out.contains("detection"), "{out}");
        assert!(out.contains("bluegale"), "{out}");
        assert!(out.contains("金継ぎ"), "{out}");
        assert!(out.contains("glazed title.bbm"), "{out}");
        assert!(
            scratch.0.join("title-x4.png").is_file(),
            "the glazed PNG should exist"
        );
        // A PNG signature, because "it wrote a file" is not "it wrote an image".
        let png = std::fs::read(scratch.0.join("title-x4.png")).unwrap();
        assert_eq!(
            &png[0..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
        );
    }

    #[test]
    fn missing_game_reports_rather_than_panics() {
        let out = detect("/definitely/not/a/game").unwrap_err();
        assert!(matches!(out, Error::NotFound(_)), "{out:?}");
    }
}
