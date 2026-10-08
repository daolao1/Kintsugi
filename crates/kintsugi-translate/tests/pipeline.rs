//! End-to-end: script → extract → translate → apply → interpret.
//! Proves the glaze fits the body without any network.

use kintsugi_core::runtime::Interpreter;
use kintsugi_core::script::{Command, Script};
use kintsugi_core::vfs::VirtualPath;

use kintsugi_translate::{MockTranslator, Translator, apply, extract};

#[derive(Default)]
struct CollectingHost {
    lines: Vec<String>,
}

impl kintsugi_core::runtime::Host for CollectingHost {
    fn show_text(&mut self, speaker: Option<&str>, text: &str) -> kintsugi_core::Result<()> {
        match speaker {
            Some(s) => self.lines.push(format!("[{s}] {text}")),
            None => self.lines.push(text.to_string()),
        }
        Ok(())
    }

    fn event(&mut self, _event: kintsugi_core::runtime::Event<'_>) -> kintsugi_core::Result<()> {
        Ok(())
    }

    fn choose(
        &mut self,
        options: &[kintsugi_core::script::ChoiceOption],
    ) -> kintsugi_core::Result<usize> {
        Ok(options.len().saturating_sub(1))
    }
}

#[test]
fn translated_script_plays_through_the_runtime() {
    let mut script = Script::new(VirtualPath::new("story.bdt"));
    script.commands = vec![
        Command::Label("start".into()),
        Command::Narration("深夜の工房。".into()),
        Command::Dialogue {
            speaker: Some("藍子".into()),
            text: "直せますか？".into(),
        },
    ];

    let entries = extract(&script);
    assert_eq!(entries.len(), 2);

    let mock = MockTranslator::new("⟦EN⟧ ");
    let translated = mock.translate(&entries, "ja", "en").unwrap();
    let merged = apply(&script, &translated).unwrap();

    let mut host = CollectingHost::default();
    Interpreter::new(merged).run(&mut host).unwrap();
    assert_eq!(
        host.lines,
        vec![
            "⟦EN⟧ 深夜の工房。".to_string(),
            "[藍子] ⟦EN⟧ 直せますか？".to_string(),
        ]
    );
}

#[test]
fn labels_and_structure_survive_translation() {
    let mut script = Script::new(VirtualPath::new("s.bdt"));
    script.commands = vec![
        Command::Label("start".into()),
        Command::Jump("start".into()),
        Command::Narration("金継ぎの夜。".into()),
        Command::Wait(30),
    ];
    let translated = MockTranslator::new("T:")
        .translate(&extract(&script), "ja", "en")
        .unwrap();
    let merged = apply(&script, &translated).unwrap();
    assert_eq!(merged.commands[0], Command::Label("start".into()));
    assert_eq!(merged.commands[1], Command::Jump("start".into()));
    assert_eq!(
        merged.commands[2],
        Command::Narration("T:金継ぎの夜。".into())
    );
    assert_eq!(merged.commands[3], Command::Wait(30));
}
