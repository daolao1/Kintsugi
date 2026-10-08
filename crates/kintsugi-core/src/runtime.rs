//! The interpreter: walks the script IR and talks to a [`Host`].
//!
//! The body never opens a window or plays a sound itself — it narrates to
//! whatever host is listening (a terminal today, a GUI or recorder
//! tomorrow). That boundary is what lets the same repaired game run
//! anywhere the body compiles.

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::script::{ChoiceOption, Command, Script};

/// A scene event, reported to the host as it happens.
#[derive(Clone, PartialEq, Debug)]
pub enum Event<'a> {
    /// Background changed to this asset.
    Background(&'a str),
    /// A character sprite appeared.
    CharacterShown {
        /// Character id.
        id: &'a str,
        /// Sprite asset name.
        sprite: &'a str,
    },
    /// A character left the scene.
    CharacterHidden {
        /// Character id.
        id: &'a str,
    },
    /// Music changed; `None` means silence.
    Music(Option<&'a str>),
    /// A one-shot sound effect.
    Sound(&'a str),
    /// A timed pause.
    Wait(u32),
}

/// Everything the interpreter needs from the outside world.
pub trait Host {
    /// Show one line of text to the player.
    fn show_text(&mut self, speaker: Option<&str>, text: &str) -> Result<()>;

    /// A scene event occurred (bg, sprites, audio, waits).
    fn event(&mut self, event: Event<'_>) -> Result<()>;

    /// Ask the player to choose; returns the index into `options`.
    fn choose(&mut self, options: &[ChoiceOption]) -> Result<usize>;
}

/// Guard against runaway `Jump` cycles in repaired scripts.
pub const STEP_BUDGET: u64 = 1_000_000;

/// Runs a [`Script`] against a [`Host`].
#[derive(Debug)]
pub struct Interpreter {
    script: Script,
    labels: HashMap<String, usize>,
}

impl Interpreter {
    /// Prepare to run `script`; labels are indexed once, up front.
    pub fn new(script: Script) -> Self {
        let labels = script.label_index();
        Self { script, labels }
    }

    /// The wrapped script.
    pub fn script(&self) -> &Script {
        &self.script
    }

    fn jump(&self, target: &str) -> Result<usize> {
        self.labels
            .get(target)
            .copied()
            .ok_or_else(|| Error::Script {
                context: self.script.source.to_string(),
                detail: format!("undefined label '{target}'"),
            })
    }

    /// Run the whole script against `host`.
    ///
    /// Stops at [`Command::End`], after the last command, or on the first
    /// error (undefined label, host failure, step budget exhausted).
    pub fn run<H: Host>(&self, host: &mut H) -> Result<()> {
        let commands = &self.script.commands;
        let mut ip: usize = 0;
        let mut steps: u64 = 0;
        loop {
            if ip >= commands.len() {
                return Ok(());
            }
            steps += 1;
            if steps > STEP_BUDGET {
                return Err(Error::Script {
                    context: self.script.source.to_string(),
                    detail: format!(
                        "exceeded the {STEP_BUDGET}-step budget — a jump cycle is cycling"
                    ),
                });
            }
            match &commands[ip] {
                Command::Label(_) => {}
                Command::Narration(text) => host.show_text(None, text)?,
                Command::Dialogue { speaker, text } => {
                    host.show_text(speaker.as_deref(), text)?;
                }
                Command::RawLine(line) => host.show_text(None, line)?,
                Command::Jump(target) => {
                    ip = self.jump(target)?;
                    continue;
                }
                Command::Choice(options) => {
                    if options.is_empty() {
                        return Err(Error::Script {
                            context: self.script.source.to_string(),
                            detail: "choice with no options".into(),
                        });
                    }
                    let picked = host.choose(options)?;
                    let Some(option) = options.get(picked) else {
                        return Err(Error::Script {
                            context: self.script.source.to_string(),
                            detail: format!("host picked option {picked} of {}", options.len()),
                        });
                    };
                    ip = self.jump(&option.goto)?;
                    continue;
                }
                Command::SetBackground(name) => host.event(Event::Background(name))?,
                Command::ShowCharacter { id, sprite } => {
                    host.event(Event::CharacterShown { id, sprite })?
                }
                Command::HideCharacter(id) => host.event(Event::CharacterHidden { id })?,
                Command::PlayMusic(name) => host.event(Event::Music(name.as_deref()))?,
                Command::PlaySound(name) => host.event(Event::Sound(name))?,
                Command::Wait(ms) => host.event(Event::Wait(*ms))?,
                Command::End => return Ok(()),
            }
            ip += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::Event;

    #[derive(Default)]
    struct RecordingHost {
        lines: Vec<String>,
        events: Vec<String>,
        pick: usize,
    }

    impl Host for RecordingHost {
        fn show_text(&mut self, speaker: Option<&str>, text: &str) -> Result<()> {
            match speaker {
                Some(s) => self.lines.push(format!("[{s}] {text}")),
                None => self.lines.push(text.to_string()),
            }
            Ok(())
        }

        fn event(&mut self, event: Event<'_>) -> Result<()> {
            self.events.push(format!("{event:?}"));
            Ok(())
        }

        fn choose(&mut self, options: &[ChoiceOption]) -> Result<usize> {
            self.pick = self.pick.min(options.len() - 1);
            Ok(self.pick)
        }
    }

    fn script() -> Script {
        let mut s = Script::new("test");
        s.commands = vec![
            Command::SetBackground("room.zbm".into()),
            Command::Narration("夜の工房。".into()),
            Command::Label("start".into()),
            Command::Dialogue {
                speaker: Some("職人".into()),
                text: "金で継ごう。".into(),
            },
            Command::Choice(vec![
                ChoiceOption {
                    label: "金で継ぐ".into(),
                    goto: "gold".into(),
                },
                ChoiceOption {
                    label: "白で隠す".into(),
                    goto: "hide".into(),
                },
            ]),
            Command::Jump("bad".into()),
            Command::Label("gold".into()),
            Command::ShowCharacter {
                id: "eri".into(),
                sprite: "eri_smile".into(),
            },
            Command::PlayMusic(Some("opening.ogg".into())),
            Command::Narration("金色の川。".into()),
            Command::End,
            Command::Label("hide".into()),
            Command::Narration("到達不能".into()),
        ];
        s
    }

    #[test]
    fn walks_the_happy_path_with_choice() {
        let mut host = RecordingHost {
            pick: 0,
            ..Default::default()
        };
        Interpreter::new(script()).run(&mut host).unwrap();
        assert_eq!(
            host.lines,
            vec!["夜の工房。", "[職人] 金で継ごう。", "金色の川。"]
        );
        assert_eq!(
            host.events,
            vec![
                "Background(\"room.zbm\")",
                "CharacterShown { id: \"eri\", sprite: \"eri_smile\" }",
                "Music(Some(\"opening.ogg\"))",
            ]
        );
    }

    #[test]
    fn second_choice_leads_elsewhere() {
        let mut host = RecordingHost {
            pick: 1,
            ..Default::default()
        };
        Interpreter::new(script()).run(&mut host).unwrap();
        assert!(host.lines.contains(&"到達不能".to_string()));
        assert!(!host.lines.contains(&"金色の川。".to_string()));
    }

    #[test]
    fn undefined_label_is_an_error() {
        let mut s = Script::new("bad");
        s.commands = vec![Command::Jump("nowhere".into())];
        let mut host = RecordingHost::default();
        let err = Interpreter::new(s).run(&mut host).unwrap_err();
        assert!(matches!(err, Error::Script { ref detail, .. } if detail.contains("nowhere")));
    }

    #[test]
    fn jump_cycles_hit_the_step_budget() {
        let mut s = Script::new("loop");
        s.commands = vec![Command::Label("l".into()), Command::Jump("l".into())];
        let mut host = RecordingHost::default();
        let err = Interpreter::new(s).run(&mut host).unwrap_err();
        assert!(matches!(err, Error::Script { ref detail, .. } if detail.contains("budget")));
    }
}
