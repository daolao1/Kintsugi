//! Script IR: the body's vocabulary for stories, independent of any engine.
//!
//! A seam translates its engine's opcodes into [`Command`]s once; the
//! interpreter, renderer, and future hosts all speak that IR. Opcodes a seam
//! has not reverse-engineered yet are carried through as [`Command::RawLine`]
//! so nothing is lost while the repair continues.

use std::collections::HashMap;

use crate::vfs::VirtualPath;

/// One selectable branch of a [`Command::Choice`].
#[derive(Clone, PartialEq, Debug)]
pub struct ChoiceOption {
    /// Text shown to the player.
    pub label: String,
    /// Label jumped to when chosen.
    pub goto: String,
}

/// One step of a story.
#[derive(Clone, PartialEq, Debug)]
pub enum Command {
    /// Narrator text.
    Narration(String),
    /// Spoken text, with a speaker name when the engine provides one.
    Dialogue {
        /// Who is speaking, if known.
        speaker: Option<String>,
        /// The line.
        text: String,
    },
    /// An engine-specific line that is not yet classified.
    ///
    /// This is the honest crack in the ceramic: visible, kept, waiting to be
    /// filled with gold (a typed opcode) when someone reverse-engineers it.
    RawLine(String),
    /// A jump target.
    Label(String),
    /// Unconditional jump to a label.
    Jump(String),
    /// Present a choice and jump to the picked option's label.
    Choice(Vec<ChoiceOption>),
    /// Change the background image (asset name as the engine spells it).
    SetBackground(String),
    /// Show a character sprite.
    ShowCharacter {
        /// Character id.
        id: String,
        /// Sprite asset name.
        sprite: String,
    },
    /// Remove a character from the scene.
    HideCharacter(String),
    /// Start looping music; `None` stops it.
    PlayMusic(Option<String>),
    /// Play a one-shot sound effect.
    PlaySound(String),
    /// Pause for `ms` milliseconds.
    Wait(u32),
    /// End of the script.
    End,
}

/// A translated story: the seam's full output plus its visible work notes.
#[derive(Clone, Debug)]
pub struct Script {
    /// Virtual path this script was read from.
    pub source: VirtualPath,
    /// The translated commands.
    pub commands: Vec<Command>,
    /// Work notes the seam wants preserved in reports (skipped lines,
    /// heuristics applied, unknown opcodes). Part of the gold, not noise.
    pub warnings: Vec<String>,
}

impl Script {
    /// An empty script from `source`.
    pub fn new(source: impl Into<VirtualPath>) -> Self {
        Self {
            source: source.into(),
            commands: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Map from label name to command index; the first occurrence of a
    /// duplicated label wins (old games do ship duplicate labels, and the
    /// original interpreters simply took the first match).
    pub fn label_index(&self) -> HashMap<String, usize> {
        let mut index = HashMap::with_capacity(self.commands.len() / 8 + 1);
        for (i, command) in self.commands.iter().enumerate() {
            if let Command::Label(name) = command {
                index.entry(name.clone()).or_insert(i);
            }
        }
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_index_first_occurrence_wins() {
        let mut script = Script::new("test.bdt");
        script.commands = vec![
            Command::Label("start".into()),
            Command::Narration("a".into()),
            Command::Label("start".into()),
            Command::Narration("b".into()),
        ];
        let index = script.label_index();
        assert_eq!(index.get("start"), Some(&0));
    }
}
