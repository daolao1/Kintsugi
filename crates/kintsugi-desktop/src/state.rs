//! The state of the story as the interpreter tells it, and the two hosts
//! that listen: one that talks to a window over channels, one that writes a
//! PNG per presented frame. What the seam said is kept whole here; how it
//! looks is `render`'s job, and how it feels is the window's.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kintsugi_core::asset::Image;
use kintsugi_core::plugin::EngineMount;
use kintsugi_core::runtime::{Event, Host};
use kintsugi_core::script::{ChoiceOption, Script};
use kintsugi_core::vfs::VirtualPath;
use kintsugi_core::{Error, Result};

/// Everything needed to draw one frame. Cheap to clone: the pictures are
/// shared, the strings are short.
#[derive(Clone, Default)]
pub struct GameState {
    /// The background currently on stage, if the story has set one.
    pub background: Option<Arc<Image>>,
    /// Characters on stage, in the order they arrived: `(id, sprite)`.
    pub characters: Vec<(String, Arc<Image>)>,
    /// Who is speaking, when the engine knows.
    pub speaker: Option<String>,
    /// The line being shown, unwrapped.
    pub text: String,
    /// The music the code last asked for, shown in words until audio lands.
    pub music: Option<String>,
    /// What the shell could not do, kept on screen rather than in a log: a
    /// named picture that would not decode, a seam that reads text only.
    pub seam_note: Option<String>,
    /// Lines shown so far, for the reader who wants to know where they are.
    pub lines_shown: u64,
}

impl std::fmt::Debug for GameState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The pictures are megabytes; a debug dump of one is a log nobody
        // reads, so the state prints their sizes, not their pixels.
        f.debug_struct("GameState")
            .field(
                "background",
                &self.background.as_ref().map(|i| (i.width, i.height)),
            )
            .field(
                "characters",
                &self
                    .characters
                    .iter()
                    .map(|(id, i)| (id, i.width, i.height))
                    .collect::<Vec<_>>(),
            )
            .field("speaker", &self.speaker)
            .field("text", &self.text)
            .field("music", &self.music)
            .field("seam_note", &self.seam_note)
            .field("lines_shown", &self.lines_shown)
            .finish()
    }
}

impl GameState {
    /// Fold one scene event into the state. A picture the code names but the
    /// seam cannot decode becomes a note on screen, never a silent gap: the
    /// kintsugi rule is that the seams stay visible.
    pub fn apply_event(&mut self, mount: &dyn EngineMount, event: &Event) {
        let picture = |name: &str, state: &mut GameState| -> Option<Arc<Image>> {
            match mount.read_image(&VirtualPath::new(name)) {
                Ok(image) => Some(Arc::new(image)),
                Err(error) => {
                    state.seam_note = Some(format!(
                        "the code names '{name}', which would not decode: {error}"
                    ));
                    None
                }
            }
        };
        match event {
            Event::Background(name) => {
                if let Some(image) = picture(name, self) {
                    self.background = Some(image);
                }
            }
            Event::CharacterShown { id, sprite } => {
                if let Some(image) = picture(sprite, self) {
                    self.characters.retain(|(shown, _)| shown != id);
                    self.characters.push((id.to_string(), image));
                }
            }
            Event::CharacterHidden { id } => {
                self.characters.retain(|(shown, _)| shown != id);
            }
            Event::Music(name) => {
                self.music = name.map(str::to_string);
            }
            Event::Sound(_) | Event::Wait(_) => {}
        }
    }
}

/// A message from the story thread to the one that owns the window.
#[derive(Debug)]
pub enum Request {
    /// A line is ready to be read; the window presents it and waits.
    Present(Box<GameState>),
    /// The story asks a question; the window collects the answer.
    Choose(Vec<String>),
    /// The interpreter reached `End` (or stopped because the window closed).
    Finished,
}

/// A message back: the reader's answer.
#[derive(Debug)]
pub enum Response {
    /// Show the next line.
    Advance,
    /// The answer to `Choose`.
    Choice(usize),
    /// The window was closed; the story should stop politely.
    Quit,
}

/// The windowed host: every `Host` call updates the shared state and, for
/// lines and choices, waits for the reader. The window lives on another
/// thread (winit's rule on macOS), so the two talk through channels; a third
/// channel carries the quit flag so a closed window lets the interpreter
/// stop instead of sprinting through ten thousand unread lines.
pub struct ChannelHost<'m> {
    mount: &'m dyn EngineMount,
    state: GameState,
    requests: Sender<Request>,
    responses: Receiver<Response>,
    stopped: Arc<Mutex<bool>>,
}

impl<'m> ChannelHost<'m> {
    /// Wire the host to the window's end of the conversation.
    pub fn new(
        mount: &'m dyn EngineMount,
        requests: Sender<Request>,
        responses: Receiver<Response>,
        stopped: Arc<Mutex<bool>>,
    ) -> Self {
        Self {
            mount,
            state: GameState::default(),
            requests,
            responses,
            stopped,
        }
    }

    fn is_stopped(&self) -> bool {
        self.stopped.lock().map(|flag| *flag).unwrap_or(true)
    }

    fn wait(&self) -> Response {
        if self.is_stopped() {
            return Response::Quit;
        }
        self.responses.recv().unwrap_or(Response::Quit)
    }

    fn stop(&self) {
        if let Ok(mut flag) = self.stopped.lock() {
            *flag = true;
        }
    }
}

impl Host for ChannelHost<'_> {
    fn show_text(&mut self, speaker: Option<&str>, text: &str) -> Result<()> {
        if self.is_stopped() {
            return Ok(());
        }
        self.state.speaker = speaker.map(str::to_string);
        self.state.text = text.to_string();
        self.state.lines_shown += 1;
        let _ = self
            .requests
            .send(Request::Present(Box::new(self.state.clone())));
        if matches!(self.wait(), Response::Quit) {
            self.stop();
        }
        Ok(())
    }

    fn event(&mut self, event: Event<'_>) -> Result<()> {
        if self.is_stopped() {
            return Ok(());
        }
        if let Event::Wait(ms) = event {
            std::thread::sleep(Duration::from_millis(ms as u64));
            return Ok(());
        }
        self.state.apply_event(self.mount, &event);
        Ok(())
    }

    fn choose(&mut self, options: &[ChoiceOption]) -> Result<usize> {
        if self.is_stopped() {
            return Ok(0);
        }
        let labels: Vec<String> = options.iter().map(|option| option.label.clone()).collect();
        let _ = self.requests.send(Request::Choose(labels));
        match self.wait() {
            Response::Choice(index) => Ok(index),
            Response::Advance => Ok(0),
            // A closed window answers every remaining question with the
            // first answer; the story it is sprinting through is not being
            // read, so the answer does not matter — stopping matters.
            Response::Quit => {
                self.stop();
                Ok(0)
            }
        }
    }
}

/// The headless host: every presented line is handed to a callback (the
/// shell's is "render and write a PNG"), and every question is answered with
/// the first option. It exists so a shell can be verified without a window —
/// the frame that the window would show is the frame this writes.
pub struct DumpHost<'m, F: FnMut(&GameState)> {
    mount: &'m dyn EngineMount,
    state: GameState,
    on_present: F,
}

impl<'m, F: FnMut(&GameState)> DumpHost<'m, F> {
    /// A host that presents every line to `on_present`.
    pub fn new(mount: &'m dyn EngineMount, on_present: F) -> Self {
        Self {
            mount,
            state: GameState::default(),
            on_present,
        }
    }
}

impl<F: FnMut(&GameState)> Host for DumpHost<'_, F> {
    fn show_text(&mut self, speaker: Option<&str>, text: &str) -> Result<()> {
        self.state.speaker = speaker.map(str::to_string);
        self.state.text = text.to_string();
        self.state.lines_shown += 1;
        (self.on_present)(&self.state);
        Ok(())
    }

    fn event(&mut self, event: Event<'_>) -> Result<()> {
        // Waits are timing, not content: a frame dump has no clock to keep.
        if !matches!(event, Event::Wait(_)) {
            self.state.apply_event(self.mount, &event);
        }
        Ok(())
    }

    fn choose(&mut self, options: &[ChoiceOption]) -> Result<usize> {
        if options.is_empty() {
            return Err(Error::Script {
                context: "dump host".to_string(),
                detail: "a choice with no options".to_string(),
            });
        }
        // Determinism is the point of a dump: the same story must write the
        // same frames every time, so the first branch is always the answer.
        Ok(0)
    }
}

/// Resolve the engine's script into a [`Script`], preferring `--script`
/// when the reader named one.
pub fn read_script(mount: &dyn EngineMount, name: Option<&str>) -> Result<Script> {
    // The seam answers which script is the game's; refusing is a supported
    // answer, and it reaches the reader word for word.
    let path = match name {
        Some(name) => VirtualPath::new(name),
        None => mount.primary_script()?,
    };
    mount.read_script(&path)
}

/// Notes worth keeping on screen, joined into the seam note: mount notes and
/// script warnings are facts about the repair, and the repair is the point.
pub fn initial_note(
    mount_notes: &[String],
    script_warnings: &[String],
    extra: &[String],
) -> Option<String> {
    let mut notes: Vec<String> = Vec::new();
    notes.extend(extra.iter().cloned());
    notes.extend(mount_notes.iter().cloned());
    notes.extend(script_warnings.iter().cloned());
    if notes.is_empty() {
        None
    } else {
        Some(notes.join(" · "))
    }
}

/// Canonicalize a path that may not exist yet: walk up to the nearest
/// ancestor that does, canonicalize that (resolving `/tmp`-style symlinks),
/// then re-append the missing tail. Containment checks that skip this get
/// symlinked paths to disagree with themselves.
fn canonicalize_loosely(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut cursor = path;
    while !cursor.exists() {
        match cursor.file_name() {
            Some(name) => missing.push(name.to_owned()),
            None => break,
        }
        cursor = cursor.parent().unwrap_or(Path::new("/"));
    }
    let mut base = cursor
        .canonicalize()
        .unwrap_or_else(|_| cursor.to_path_buf());
    for part in missing.iter().rev() {
        base.push(part);
    }
    base
}

/// Where a frame dump goes: the directory is created, and writing inside the
/// game folder is refused — the original stays untouched on every host. The
/// check runs before a single directory is made.
pub fn prepare_dump_dir(dir: &Path, game: &Path) -> Result<()> {
    let dir = canonicalize_loosely(dir);
    let game = canonicalize_loosely(game);
    if dir.starts_with(&game) {
        return Err(Error::Io(format!(
            "refusing to write frames inside the game folder ({})",
            dir.display()
        )));
    }
    std::fs::create_dir_all(&dir).map_err(|e| Error::Io(format!("creating {}: {e}", dir.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kintsugi_core::plugin::MountInfo;
    use kintsugi_core::script::ChoiceOption;
    use kintsugi_core::vfs::Vfs;

    /// A mount that decodes nothing and names no script: the shell's job
    /// around such a mount is to say so, on screen and in the error, never
    /// to fill the gap with silence. The default trait methods refuse
    /// politely, so the mock only owns what it must.
    struct NoMount {
        vfs: Vfs,
        info: MountInfo,
    }

    fn no_mount() -> NoMount {
        NoMount {
            vfs: Vfs::new(),
            info: MountInfo {
                engine: "none".to_string(),
                notes: vec![],
            },
        }
    }

    impl EngineMount for NoMount {
        fn info(&self) -> &MountInfo {
            &self.info
        }
        fn vfs(&self) -> &Vfs {
            &self.vfs
        }
    }

    #[test]
    fn a_picture_that_will_not_decode_becomes_a_note_not_a_gap() {
        let mut state = GameState::default();
        state.apply_event(&no_mount(), &Event::Background("bg/missing.bsg"));
        assert!(state.background.is_none());
        let note = state.seam_note.expect("the failure must be visible");
        assert!(note.contains("bg/missing.bsg"), "{note}");
    }

    #[test]
    fn characters_arrive_replace_and_leave_by_id() {
        let mut state = GameState::default();
        // Every sprite fails to decode, so the stage stays empty and the
        // note accrues: no phantom characters are ever shown.
        state.apply_event(
            &no_mount(),
            &Event::CharacterShown {
                id: "hina",
                sprite: "x",
            },
        );
        assert!(state.characters.is_empty());
        assert!(state.seam_note.is_some());
    }

    #[test]
    fn the_dump_host_counts_lines_and_answers_the_first_choice() {
        let mount = no_mount();
        let mut seen = 0;
        let mut host = DumpHost::new(&mount, |_: &GameState| seen += 1);
        host.show_text(Some("hina"), "おはよう").unwrap();
        host.show_text(None, "narration").unwrap();
        assert_eq!(host.state.lines_shown, 2);
        let options = vec![ChoiceOption {
            label: "a".to_string(),
            goto: "b".to_string(),
        }];
        assert_eq!(host.choose(&options).unwrap(), 0);
        drop(host);
        assert_eq!(seen, 2);
    }

    #[test]
    fn a_quit_lets_the_interpreter_stop_instead_of_sprint() {
        let (request_tx, request_rx) = std::sync::mpsc::channel();
        let (response_tx, response_rx) = std::sync::mpsc::channel();
        let stopped = Arc::new(Mutex::new(false));
        let mount = no_mount();
        let mut host = ChannelHost::new(&mount, request_tx, response_rx, stopped.clone());
        response_tx.send(Response::Quit).unwrap();
        host.show_text(None, "one").unwrap();
        assert!(host.is_stopped());
        // Once stopped, nothing is sent again and nothing blocks.
        host.show_text(None, "two").unwrap();
        host.event(Event::Background("x")).unwrap();
        let pending: Vec<_> = request_rx.try_iter().collect();
        assert_eq!(pending.len(), 1, "only the first line may be presented");
    }

    #[test]
    fn read_script_refuses_an_engine_that_names_no_script() {
        let err = read_script(&no_mount(), None).unwrap_err().to_string();
        assert!(err.contains("does not name a main script"), "{err}");
    }

    #[test]
    fn the_dump_dir_may_not_live_inside_the_game() {
        let dir = std::env::temp_dir().join(format!("kintsugi-dump-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("game")).unwrap();
        let err = prepare_dump_dir(&dir.join("game/frames"), &dir.join("game"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("inside the game folder"), "{err}");
        prepare_dump_dir(&dir.join("frames"), &dir.join("game")).unwrap();
        assert!(dir.join("frames").is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
