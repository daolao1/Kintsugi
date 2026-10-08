//! Translation entries: the interchange between script IR, JSONL files,
//! and translators.

use std::fs;
use std::path::Path;

use kintsugi_core::error::{Error, Result};
use kintsugi_core::script::{Command, Script};

use serde::{Deserialize, Serialize};

/// One translatable line of a script.
///
/// `id` is the command index in the source script — the only key needed to
/// put a translation back where it belongs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranslationEntry {
    /// Command index in the source script.
    pub id: usize,
    /// Speaker name, when the line is dialogue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    /// The original text.
    pub text: String,
}

/// Extract every text-bearing command as an entry.
///
/// Narration and dialogue are extracted; [`Command::RawLine`] lines are
/// *not* — they may be engine commands in disguise, and translating an
/// opcode is how bricks get bricked. They stay visible in play, and when a
/// seam classifies them later, extraction picks them up automatically.
pub fn extract(script: &Script) -> Vec<TranslationEntry> {
    script
        .commands
        .iter()
        .enumerate()
        .filter_map(|(id, command)| match command {
            Command::Narration(text) => Some(TranslationEntry {
                id,
                speaker: None,
                text: text.clone(),
            }),
            Command::Dialogue { speaker, text } => Some(TranslationEntry {
                id,
                speaker: speaker.clone(),
                text: text.clone(),
            }),
            // A choice's labels are one entry, joined with newlines; the
            // repair side splits them back one per option, in order.
            Command::Choice(options) if !options.is_empty() => Some(TranslationEntry {
                id,
                speaker: None,
                text: options
                    .iter()
                    .map(|option| option.label.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            }),
            _ => None,
        })
        .collect()
}

/// Extract text commands **and** [`Command::RawLine`] lines.
///
/// Most reverse-engineered seams (BlueGale's BDT among them) start out
/// emitting `RawLine` for everything unclassified — including the actual
/// story prose. This mode trusts the line contents instead: the
/// translator's command filter (pure-ASCII lines in non-Latin source
/// languages bypass the model) still guards the opcodes, and the caller
/// accepts the residual risk, visibly.
pub fn extract_with_raw(script: &Script) -> Vec<TranslationEntry> {
    script
        .commands
        .iter()
        .enumerate()
        .filter_map(|(id, command)| match command {
            Command::Narration(text) | Command::RawLine(text) => Some(TranslationEntry {
                id,
                speaker: None,
                text: text.clone(),
            }),
            Command::Dialogue { speaker, text } => Some(TranslationEntry {
                id,
                speaker: speaker.clone(),
                text: text.clone(),
            }),
            // A choice's labels are one entry, joined with newlines; the
            // repair side splits them back one per option, in order.
            Command::Choice(options) if !options.is_empty() => Some(TranslationEntry {
                id,
                speaker: None,
                text: options
                    .iter()
                    .map(|option| option.label.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            }),
            _ => None,
        })
        .collect()
}

/// Apply translations onto a clone of `script`.
///
/// Entries whose id doesn't point at a text-bearing command are skipped
/// with a warning (scripts evolve; ids drift); missing ids are simply left
/// in the original language. A provenance warning is always recorded, so a
/// translated script always announces itself.
pub fn apply(script: &Script, entries: &[TranslationEntry]) -> Result<Script> {
    let mut out = Script::new(script.source.clone());
    out.commands = script.commands.clone();
    out.warnings = script.warnings.clone();

    let mut applied = 0usize;
    let mut skipped = 0usize;
    for entry in entries {
        match out.commands.get_mut(entry.id) {
            Some(Command::Narration(text)) => {
                *text = entry.text.clone();
                applied += 1;
            }
            Some(Command::Dialogue { speaker, text }) => {
                if let Some(new_speaker) = &entry.speaker {
                    *speaker = Some(new_speaker.clone());
                }
                *text = entry.text.clone();
                applied += 1;
            }
            // RawLine lines came from `extract_with_raw`; putting a
            // translation back is exactly as safe as putting it in.
            Some(Command::RawLine(text)) => {
                *text = entry.text.clone();
                applied += 1;
            }
            Some(Command::Choice(options)) => {
                let parts: Vec<&str> = entry.text.split('\n').collect();
                if parts.len() != options.len() {
                    // One label per line, in order — a translation that
                    // joined or split them is refused, not guessed at.
                    skipped += 1;
                    out.warnings.push(format!(
                        "translation id {} is a choice of {} option(s) but the text has {} \
                         line(s); left in the original language",
                        entry.id,
                        options.len(),
                        parts.len()
                    ));
                } else {
                    for (option, label) in options.iter_mut().zip(parts) {
                        option.label = label.to_string();
                    }
                    applied += 1;
                }
            }
            Some(_) => {
                skipped += 1;
                out.warnings.push(format!(
                    "translation id {} no longer points at a text command; skipped",
                    entry.id
                ));
            }
            None => {
                skipped += 1;
                out.warnings.push(format!(
                    "translation id {} is out of range; skipped",
                    entry.id
                ));
            }
        }
    }

    if applied > 0 {
        out.warnings.push(format!(
            "translated {applied} line(s) via kintsugi-translate ({} skipped); \
             source: {}",
            skipped, script.source
        ));
    }
    Ok(out)
}

/// Write entries as JSONL (one object per line, UTF-8).
pub fn write_jsonl(path: &Path, entries: &[TranslationEntry]) -> Result<()> {
    let mut body = String::new();
    for entry in entries {
        let line = serde_json::to_string(entry)
            .map_err(|e| Error::Plugin(format!("serializing entry {}: {e}", entry.id)))?;
        body.push_str(&line);
        body.push('\n');
    }
    fs::write(path, body).map_err(|e| Error::Io(format!("writing {}: {e}", path.display())))
}

/// Append entries to a JSONL file, creating it if needed.
///
/// Resume lives here: a long run writes each batch as it lands, so an
/// interrupted run is a paused run, and the next invocation skips every line
/// the file already holds.
pub fn append_jsonl(path: &Path, entries: &[TranslationEntry]) -> Result<()> {
    use std::io::Write;
    let mut body = String::new();
    for entry in entries {
        let line = serde_json::to_string(entry)
            .map_err(|e| Error::Plugin(format!("serializing entry {}: {e}", entry.id)))?;
        body.push_str(&line);
        body.push('\n');
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| Error::Io(format!("opening {}: {e}", path.display())))?;
    file.write_all(body.as_bytes())
        .map_err(|e| Error::Io(format!("appending to {}: {e}", path.display())))
}

/// Read entries from a JSONL file.
///
/// Blank lines are tolerated (diff-friendly), malformed lines are not.
pub fn read_jsonl(path: &Path) -> Result<Vec<TranslationEntry>> {
    let body = fs::read_to_string(path)
        .map_err(|e| Error::Io(format!("reading {}: {e}", path.display())))?;
    let mut entries = Vec::new();
    for (number, line) in body.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry = serde_json::from_str(line).map_err(|e| {
            Error::corrupt(
                "translation JSONL",
                format!("{}: line {}: {e}", path.display(), number + 1),
            )
        })?;
        entries.push(entry);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kintsugi_core::vfs::VirtualPath;

    fn sample() -> Script {
        let mut script = Script::new(VirtualPath::new("story.bdt"));
        script.commands = vec![
            Command::Label("start".into()),
            Command::Narration("深夜の工房。".into()),
            Command::Dialogue {
                speaker: Some("藍子".into()),
                text: "直せますか？".into(),
            },
            Command::RawLine("SND_PLAY ro-mon.ogg".into()),
        ];
        script
    }

    fn with_choice() -> Script {
        let mut script = sample();
        script.commands.push(Command::Choice(vec![
            kintsugi_core::script::ChoiceOption {
                label: "直す".into(),
                goto: "fix".into(),
            },
            kintsugi_core::script::ChoiceOption {
                label: "直さない".into(),
                goto: "leave".into(),
            },
        ]));
        script
    }

    #[test]
    fn a_choice_is_one_entry_of_joined_labels_and_applies_back() {
        let entries = extract_with_raw(&with_choice());
        let choice = entries.iter().find(|entry| entry.id == 4).expect("choice");
        assert_eq!(choice.text, "直す\n直さない");

        let translated = apply(
            &with_choice(),
            &[TranslationEntry {
                id: 4,
                speaker: None,
                text: "Repair it\nLeave it".to_string(),
            }],
        )
        .expect("apply");
        let Command::Choice(options) = &translated.commands[4] else {
            panic!("command 4 is the choice");
        };
        assert_eq!(options[0].label, "Repair it");
        assert_eq!(options[1].label, "Leave it");
        // The route the label names never changes hands.
        assert_eq!(options[0].goto, "fix");
    }

    #[test]
    fn a_misjoined_choice_is_refused_not_guessed() {
        let translated = apply(
            &with_choice(),
            &[TranslationEntry {
                id: 4,
                speaker: None,
                text: "Repair it, or don't".to_string(),
            }],
        )
        .expect("apply");
        let Command::Choice(options) = &translated.commands[4] else {
            panic!("command 4 is the choice");
        };
        assert_eq!(options[0].label, "直す", "a misjoined label stays original");
        assert!(
            translated
                .warnings
                .iter()
                .any(|warning| warning.contains("choice of 2 option(s)")),
            "the refusal says why: {:?}",
            translated.warnings
        );
    }

    #[test]
    fn extract_picks_text_commands_only() {
        let entries = extract(&sample());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, 1);
        assert!(entries[0].speaker.is_none());
        assert_eq!(entries[1].id, 2);
        assert_eq!(entries[1].speaker.as_deref(), Some("藍子"));
    }

    #[test]
    fn apply_translates_and_records_provenance() {
        let script = sample();
        let entries = vec![
            TranslationEntry {
                id: 1,
                speaker: None,
                text: "The workshop at midnight.".into(),
            },
            TranslationEntry {
                id: 2,
                speaker: Some("Aiko".into()),
                text: "Can you mend it?".into(),
            },
        ];
        let out = apply(&script, &entries).unwrap();
        assert_eq!(
            out.commands[1],
            Command::Narration("The workshop at midnight.".into())
        );
        assert_eq!(
            out.commands[2],
            Command::Dialogue {
                speaker: Some("Aiko".into()),
                text: "Can you mend it?".into()
            }
        );
        // Structure and untranslatable lines survive untouched.
        assert_eq!(out.commands[0], Command::Label("start".into()));
        assert_eq!(
            out.commands[3],
            Command::RawLine("SND_PLAY ro-mon.ogg".into())
        );
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("kintsugi-translate"))
        );
    }

    #[test]
    fn apply_skips_stale_ids_with_warning() {
        let script = sample();
        let entries = vec![
            TranslationEntry {
                id: 0, // a label, not text
                speaker: None,
                text: "nope".into(),
            },
            TranslationEntry {
                id: 99,
                speaker: None,
                text: "nope".into(),
            },
        ];
        let out = apply(&script, &entries).unwrap();
        assert_eq!(out.commands, script.commands);
        assert!(out.warnings.iter().any(|w| w.contains("id 0")));
        assert!(out.warnings.iter().any(|w| w.contains("id 99")));
    }

    #[test]
    fn jsonl_roundtrip() {
        let dir = std::env::temp_dir().join(format!("kintsugi-tr-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("batch.jsonl");
        let entries = vec![
            TranslationEntry {
                id: 1,
                speaker: None,
                text: "深夜の工房。".into(),
            },
            TranslationEntry {
                id: 2,
                speaker: Some("藍子".into()),
                text: "直せますか？".into(),
            },
        ];
        write_jsonl(&path, &entries).unwrap();
        let back = read_jsonl(&path).unwrap();
        assert_eq!(back, entries);
        fs::remove_dir_all(&dir).ok();
    }
}
