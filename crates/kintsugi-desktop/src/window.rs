//! The window: winit owns the OS side, softbuffer owns the presentation,
//! and this file owns nothing else. The story runs on its own thread (the
//! interpreter blocks on the reader, and the reader's thread is the one the
//! OS insists on owning), so the two talk through the channels from `state`:
//! requests arrive as user events, answers go back as plain messages.

use std::num::NonZeroU32;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use kintsugi_core::plugin::EngineMount;
use kintsugi_core::runtime::Interpreter;
use kintsugi_core::script::Script;
use kintsugi_core::{Error, Result};
use kintsugi_desktop::font::Pen;
use kintsugi_desktop::render::{Frame, Scene, TEXT_BOX_TOP_FRACTION, TEXT_MARGIN, render};
use kintsugi_desktop::state::{ChannelHost, GameState, Request, Response, initial_note};

/// The user event that carries a story request into the loop. winit's loop
/// sleeps until an event arrives; a line produced on the story thread is not
/// an OS event, so a forwarder thread taps the loop on the shoulder.
enum UiEvent {
    Story(Request),
}

/// Everything the window knows. The story's state arrives whole (cheap:
/// pictures are shared); the choice under discussion and whether the reader
/// is expected to answer are the window's own business.
struct App {
    window: Option<Arc<Window>>,
    context: Option<Context<Arc<Window>>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    state: GameState,
    /// Labels of the choice on the table, plus which one is highlighted.
    choice: Option<(Vec<String>, usize)>,
    /// A Present is unanswered: input advances the story. Otherwise input is
    /// ignored — a click that lands between lines must not queue up a skip.
    waiting: bool,
    /// The interpreter reached `End`; the last frame stays up until closed.
    finished: bool,
    pen: Option<Pen>,
    responses: Sender<Response>,
    stopped: Arc<Mutex<bool>>,
}

impl App {
    /// What the reader sees right now, as a frame.
    fn draw(&self) -> Frame {
        let size = self
            .window
            .as_ref()
            .map(|window| window.inner_size())
            .unwrap_or_else(|| winit::dpi::PhysicalSize::new(960, 720));
        let (width, height) = (size.width.max(320), size.height.max(240));

        let mut scene = Scene {
            background: self.state.background.as_deref(),
            characters: self
                .state
                .characters
                .iter()
                .map(|(_, image)| image.as_ref())
                .collect(),
            speaker: self.state.speaker.as_deref(),
            lines: Vec::new(),
            seam_note: self.state.seam_note.as_deref(),
        };

        let Some(pen) = &self.pen else {
            // No font on this machine: the scene still renders and the seam
            // note says why the box is empty. A shell that drew nothing and
            // said nothing would be lying by omission.
            return render(&scene, width, height, 26, |_, _, _, _, _| {});
        };

        let max_width = width.saturating_sub(TEXT_MARGIN * 2);
        let box_height = (height as f32 * (1.0 - TEXT_BOX_TOP_FRACTION)) as u32;
        let max_lines = box_height.saturating_sub(TEXT_MARGIN * 2) / pen.line_height().max(1);
        let mut lines = if let Some((labels, selected)) = &self.choice {
            // The question on the table, with the gold mark on the answer the
            // reader is about to give.
            labels
                .iter()
                .enumerate()
                .map(|(index, label)| {
                    if index == *selected {
                        format!("▶ {label}")
                    } else {
                        format!("　 {label}")
                    }
                })
                .collect::<Vec<_>>()
        } else {
            pen.wrap(&self.state.text, max_width)
        };
        if max_lines > 0 && lines.len() > max_lines as usize {
            lines.truncate(max_lines as usize);
            // The mark that text was cut: a reader must see that the box ran
            // out of room, never have to guess it.
            if let Some(last) = lines.last_mut() {
                last.push('…');
            }
        }
        scene.lines = lines;
        render(
            &scene,
            width,
            height,
            pen.line_height(),
            |frame, x, y, text, colour| pen.draw(frame, x, y, text, colour),
        )
    }

    /// Answer the outstanding question, if there is one.
    fn respond(&mut self, response: Response) {
        if matches!(response, Response::Quit) {
            if let Ok(mut flag) = self.stopped.lock() {
                *flag = true;
            }
            let _ = self.responses.send(Response::Quit);
            return;
        }
        if let Some((labels, selected)) = &self.choice {
            if labels.is_empty() {
                return;
            }
            let index = match response {
                Response::Choice(index) if index < labels.len() => index,
                // Space/Enter/click confirm the highlighted answer.
                _ => *selected,
            };
            let _ = self.responses.send(Response::Choice(index));
            self.choice = None;
            return;
        }
        if self.waiting && matches!(response, Response::Advance) {
            self.waiting = false;
            let _ = self.responses.send(Response::Advance);
        }
    }

    fn present(&mut self) {
        // Draw first: the frame is a pure function of the state, and the
        // mutable borrow of the surface may not overlap it.
        let frame = self.draw();
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return;
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        if surface.resize(width, height).is_err() {
            return;
        }
        if let Ok(mut buffer) = surface.buffer_mut() {
            // softbuffer wants 0RGB; the frame's alpha byte is always 0xFF.
            for (out, &px) in buffer.iter_mut().zip(frame.pixels.iter()) {
                *out = px & 0x00FF_FFFF;
            }
            let _ = buffer.present();
        }
    }
}

impl ApplicationHandler<UiEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("🏺 Kintsugi")
            .with_inner_size(LogicalSize::new(960.0, 720.0))
            .with_min_inner_size(LogicalSize::new(480.0, 360.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("🏺 kintsugi-desktop: could not open a window: {error}");
                event_loop.exit();
                return;
            }
        };
        match Context::new(window.clone()).and_then(|context| {
            Surface::new(&context, window.clone()).map(|surface| (context, surface))
        }) {
            Ok((context, surface)) => {
                self.context = Some(context);
                self.surface = Some(surface);
                self.window = Some(window);
            }
            Err(error) => {
                eprintln!("🏺 kintsugi-desktop: could not open a pixel surface: {error}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UiEvent) {
        let UiEvent::Story(request) = event;
        match request {
            Request::Present(state) => {
                self.state = *state;
                self.waiting = true;
            }
            Request::Choose(labels) => {
                self.choice = Some((labels, 0));
            }
            Request::Finished => {
                self.finished = true;
                self.waiting = false;
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.respond(Response::Quit);
                event_loop.exit();
            }
            WindowEvent::Resized(_) => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.present(),
            WindowEvent::MouseInput { state, button, .. }
                if state == ElementState::Pressed && button == MouseButton::Left =>
            {
                self.respond(Response::Advance);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        self.respond(Response::Quit);
                        event_loop.exit();
                    }
                    Key::Named(NamedKey::Space)
                    | Key::Named(NamedKey::Enter)
                    | Key::Named(NamedKey::ArrowRight)
                    | Key::Named(NamedKey::ArrowDown) => self.respond(Response::Advance),
                    Key::Named(NamedKey::ArrowUp) => {
                        if let Some((labels, selected)) = &mut self.choice {
                            if !labels.is_empty() {
                                *selected = (*selected + labels.len() - 1) % labels.len();
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                            }
                        }
                    }
                    Key::Character(text) => {
                        // 1–9 answer a choice directly.
                        if let Some((labels, _)) = &self.choice {
                            if let Some(digit) = text.chars().next().and_then(|c| c.to_digit(10)) {
                                let index = (digit as usize).saturating_sub(1);
                                if index < labels.len() {
                                    self.respond(Response::Choice(index));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Run the windowed host until the story ends or the reader leaves.
///
/// `mount`, `script` and the shell's own notes come from the caller; this
/// function owns the loop and the threads, and nothing about engines.
pub fn run(mount: &dyn EngineMount, script: Script, extra_notes: Vec<String>) -> Result<()> {
    let (story_tx, story_rx) = std::sync::mpsc::channel::<Request>();
    let (response_tx, response_rx) = std::sync::mpsc::channel::<Response>();
    let stopped = Arc::new(Mutex::new(false));

    let event_loop = EventLoop::<UiEvent>::with_user_event()
        .build()
        .map_err(|e| Error::Io(format!("creating the event loop: {e}")))?;
    let proxy: EventLoopProxy<UiEvent> = event_loop.create_proxy();

    // The forwarder: the story thread speaks into a channel; the loop only
    // wakes for events. This thread is the tap on the shoulder.
    let forwarder = std::thread::spawn(move || {
        while let Ok(request) = story_rx.recv() {
            if proxy.send_event(UiEvent::Story(request)).is_err() {
                break;
            }
        }
    });

    let mut notes = extra_notes;
    notes.extend(initial_note(&mount.info().notes, &script.warnings, &[]));
    let (pen, pen_note) = match Pen::load(20.0) {
        Some(pen) => (Some(pen), None),
        None => (
            None,
            Some("no usable system font found (KINTSUGI_FONT can name one)".to_string()),
        ),
    };
    notes.extend(pen_note);

    let mut app = App {
        window: None,
        context: None,
        surface: None,
        state: GameState {
            seam_note: if notes.is_empty() {
                None
            } else {
                Some(notes.join(" · "))
            },
            ..GameState::default()
        },
        choice: None,
        waiting: false,
        finished: false,
        pen,
        responses: response_tx,
        stopped: stopped.clone(),
    };

    // The story thread borrows the mount, which lives on the caller's
    // stack; a scoped thread makes that borrow legal, and the scope also
    // guarantees the story is over before `run` returns.
    let story_result = std::thread::scope(|scope| {
        let story_stopped = stopped.clone();
        let story = scope.spawn(move || {
            let mut host = ChannelHost::new(mount, story_tx, response_rx, story_stopped);
            Interpreter::new(script).run(&mut host)
        });

        let loop_result = event_loop
            .run_app(&mut app)
            .map_err(|e| Error::Io(format!("running the event loop: {e}")));

        // The loop is over: let the story thread out politely, and do not
        // leave the forwarder talking to nobody.
        if let Ok(mut flag) = app.stopped.lock() {
            *flag = true;
        }
        // A closed window never blocks the exit: nudge the story awake.
        let _ = app.responses.send(Response::Quit);
        let story_result = story
            .join()
            .unwrap_or_else(|_| Err(Error::Io("the story thread panicked".to_string())));
        loop_result?;
        story_result
    });
    let _ = forwarder.join();
    story_result
}
