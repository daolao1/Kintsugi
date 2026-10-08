//! 🏺 The player: a game's story as one offline HTML file.
//!
//! The terminal host is where the engine is proved, but a terminal is a poor
//! place to read a visual novel: twelve thousand lines scroll past and the
//! reader cannot stop, go back, or keep their place. This writes the same
//! commands into a file that any browser opens — no network, no server, no
//! dependency — so the repaired story can be read on the same three platforms
//! the engine is maintained for.
//!
//! Two rules shape it, and both come from the project they are part of:
//!
//! * **The seams stay visible.** A line the seam could not classify is marked
//!   as such in the page itself, and the seam's own warnings are printed at the
//!   top, in its own words. A reader who cannot tell decoded from guessed is
//!   being told a story about the game rather than the game's story.
//! * **Nothing is invented.** The gallery shows the pictures that are in the
//!   game and nothing else. It does not claim which line shows which picture,
//!   because the code that would say so is not decoded yet — so the page keeps
//!   the two apart instead of pairing them for effect.

use std::fmt::Write as _;
use std::path::Path;

use kintsugi_core::error::{Error, Result};
use kintsugi_core::script::{Command, Script};

/// A picture to put in the gallery: where it is in the game, and the file the
/// browser will load it from.
pub struct Picture {
    /// Path inside the game, which is also the name the reader sees.
    pub name: String,
    /// File name, relative to the page.
    pub file: String,
}

/// What the page says about itself, beyond the story.
pub struct Player<'a> {
    /// What the game calls itself — the folder, usually.
    pub game: &'a str,
    /// Which seam mounted it.
    pub engine: &'a str,
    /// The script inside the game that this story was read from.
    pub source: &'a str,
    /// Pictures for the gallery, in the order they should appear.
    pub pictures: &'a [Picture],
}

/// One line of story, ready to be written into the page.
struct Line {
    /// What the seam read.
    text: String,
    /// `narration`, `dialogue`, `raw` — the classification, or the crack.
    kind: &'static str,
    /// A speaker, when the engine named one.
    speaker: Option<String>,
}

/// Write the player to `path` and return how many lines of story it holds.
///
/// The page is complete in itself: styles and script are inline, so it opens
/// from a file manager with no network at all.
pub fn write_player(path: &Path, script: &Script, player: &Player<'_>) -> Result<usize> {
    let lines = collect(script);
    let html = render(&lines, script, player);
    std::fs::write(path, html)
        .map_err(|e| Error::Io(format!("writing {}: {e}", path.display())))?;
    Ok(lines.len())
}

/// The commands that are a line of story, in the order the engine reads them.
///
/// Everything else the IR can hold — labels, jumps, choices, scene directions —
/// is not a line a reader reads, so the player does not print it as one. What
/// is left keeps its classification: that is the whole point of the seam having
/// decoded the bytecode in the first place.
fn collect(script: &Script) -> Vec<Line> {
    let mut lines = Vec::new();
    for command in &script.commands {
        match command {
            Command::Narration(text) => lines.push(Line {
                text: text.clone(),
                kind: "narration",
                speaker: None,
            }),
            Command::Dialogue { speaker, text } => lines.push(Line {
                text: text.clone(),
                kind: "dialogue",
                speaker: speaker.clone(),
            }),
            Command::RawLine(text) => lines.push(Line {
                text: text.clone(),
                kind: "raw",
                speaker: None,
            }),
            Command::Label(name) => lines.push(Line {
                text: format!("${name}"),
                kind: "label",
                speaker: None,
            }),
            Command::Jump(name) => lines.push(Line {
                text: format!("→ ${name}"),
                kind: "jump",
                speaker: None,
            }),
            // An option is a jump with words attached: the words are the
            // lines just before it, and the label is where picking it goes.
            // The player shows the jump, because that is what the engine
            // handed over — inventing an option line here would be inventing
            // dialogue the game never had.
            Command::Choice(options) => {
                for option in options {
                    lines.push(Line {
                        text: format!("{} → ${}", option.label, option.goto),
                        kind: "choice",
                        speaker: None,
                    });
                }
            }
            _ => {}
        }
    }
    lines
}

/// HTML-escape text that came out of a 2008 game.
///
/// The story is not markup and must never be read as any: a stray `<` in a
/// dialogue line would otherwise eat the rest of the page.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn render(lines: &[Line], script: &Script, player: &Player<'_>) -> String {
    let mut html = String::with_capacity(lines.len() * 96 + 8192);
    let narration = lines.iter().filter(|l| l.kind == "narration").count();
    let dialogue = lines.iter().filter(|l| l.kind == "dialogue").count();
    let raw = lines.iter().filter(|l| l.kind == "raw").count();

    html.push_str("<!doctype html>\n<html lang=\"ja\">\n<head>\n<meta charset=\"utf-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    let _ = writeln!(html, "<title>{} — kintsugi</title>", escape(player.game));
    html.push_str(STYLE);
    html.push_str("</head>\n<body>\n");

    // The header says what this file is and what it is not, before the story
    // starts. A reader who skips it still gets the marks on the lines.
    html.push_str("<header>\n");
    html.push_str("<h1>🏺 <span class=\"gold\">kintsugi</span> player</h1>\n");
    let _ = writeln!(
        html,
        "<p class=\"sub\">{} · <span class=\"dim\">{}</span> · <span class=\"dim\">source: {}</span></p>",
        escape(player.game),
        escape(player.engine),
        escape(player.source)
    );
    let _ = writeln!(
        html,
        "<p class=\"counts\">{} line(s): <span class=\"k-narration\">{} narration</span>, \
         <span class=\"k-dialogue\">{} dialogue</span>, <span class=\"k-raw\">{} unclassified</span></p>",
        lines.len(),
        narration,
        dialogue,
        raw
    );
    if !script.warnings.is_empty() {
        html.push_str("<details class=\"warnings\" open>\n<summary>what this seam says about its own reading</summary>\n<ul>\n");
        for warning in &script.warnings {
            let _ = writeln!(html, "<li>{}</li>", escape(warning));
        }
        html.push_str("</ul>\n</details>\n");
    }
    html.push_str(
        "<p class=\"keys\"><kbd>→</kbd> / <kbd>space</kbd> next · <kbd>←</kbd> back · \
         <kbd>Home</kbd> · <kbd>End</kbd> · <kbd>o</kbd> show the marks · click the page</p>\n",
    );
    html.push_str("</header>\n");

    html.push_str("<main id=\"stage\">\n");
    for (index, line) in lines.iter().enumerate() {
        let speaker = match &line.speaker {
            Some(speaker) => format!("<span class=\"speaker\">{}</span>", escape(speaker)),
            None => String::new(),
        };
        let _ = writeln!(
            html,
            "<div class=\"line k-{}\" data-n=\"{}\" id=\"l{}\">{}{}</div>",
            line.kind,
            index + 1,
            index + 1,
            speaker,
            escape(&line.text)
        );
    }
    html.push_str("</main>\n");

    if !player.pictures.is_empty() {
        html.push_str("<section id=\"gallery\">\n");
        let _ = writeln!(
            html,
            "<h2>this game's own pictures <span class=\"dim\">({})</span></h2>",
            player.pictures.len()
        );
        html.push_str(
            "<p class=\"note\">These are the game's pictures, in the game's own order. \
             Which line shows which picture is decided by code this seam does not read yet, \
             so the page does not pair them: no claim is made here that a picture belongs \
             to the line you happen to be reading.</p>\n<div class=\"grid\">\n",
        );
        for picture in player.pictures {
            let _ = writeln!(
                html,
                "<figure><img loading=\"lazy\" src=\"{}\" alt=\"{}\"><figcaption>{}</figcaption></figure>",
                escape(&picture.file),
                escape(&picture.name),
                escape(&picture.name)
            );
        }
        html.push_str("</div>\n</section>\n");
    }

    // The last word is the promise the whole project is built on.
    html.push_str(
        "<footer>\n<p>Read in the order the game's own code shows these lines. \
         A branch is not decoded yet, so this is every line the story can reach, \
         not one playthrough. Nothing here was written to the game: the original \
         files are untouched, and the marks of what is decoded stay visible.</p>\n\
         <p class=\"gold\">Repairing old games with gold. · 以金缮之艺，续老游戏之命。</p>\n</footer>\n",
    );

    html.push_str(SCRIPT);
    html.push_str("</body>\n</html>\n");
    html
}

/// The page's styles: dark lacquer, gold seams, and the marks readable.
const STYLE: &str = r#"<style>
:root { --lacquer:#0d0c0b; --ink:#e8e2d8; --gold:#d8b45a; --dim:#8a8175; --raw:#c96f4a; }
* { box-sizing: border-box; }
html, body { margin:0; background:var(--lacquer); color:var(--ink); }
body { font-family: "Hiragino Mincho ProN", "Yu Mincho", "Noto Serif JP", serif;
       line-height:2.1; padding:0 0 6rem; }
header, footer { max-width:52rem; margin:0 auto; padding:2rem 1.5rem 0; }
h1 { font-size:1.1rem; letter-spacing:.14em; margin:0 0 .3rem; font-weight:400; }
h2 { font-size:.95rem; letter-spacing:.1em; font-weight:400; margin:3rem 0 .5rem; }
.gold { color:var(--gold); }
.dim { color:var(--dim); }
.sub, .counts, .keys, .note { font-size:.8rem; color:var(--dim); margin:.2rem 0;
       line-height:1.9; font-family:-apple-system,"Helvetica Neue",sans-serif; }
.k-narration { color:#b9c7a8; } .k-dialogue { color:#a9c0d8; } .k-raw { color:var(--raw); }
kbd { border:1px solid #3a352e; border-radius:.25rem; padding:0 .3rem; font-size:.75rem; }
.warnings { border-left:2px solid var(--gold); padding:.2rem 0 .2rem 1rem; margin:1.2rem 0; }
.warnings summary { font-size:.8rem; color:var(--gold); cursor:pointer;
       font-family:-apple-system,"Helvetica Neue",sans-serif; }
.warnings ul { margin:.6rem 0 0; padding-left:1.1rem; }
.warnings li { font-size:.78rem; color:var(--dim); line-height:1.8; margin:.35rem 0;
       font-family:-apple-system,"Helvetica Neue",sans-serif; }
main { max-width:52rem; margin:0 auto; padding:2rem 1.5rem 0; }
.line { padding:.55rem .8rem; border-left:2px solid transparent; border-radius:.2rem;
        opacity:.32; transition:opacity .18s, border-color .18s; cursor:pointer; }
.line.seen { opacity:.62; }
.line.now { opacity:1; border-left-color:var(--gold); background:#151310; }
.line.k-raw { border-left-style:dotted; }
.line .speaker { color:var(--gold); margin-right:.6rem; font-size:.9em; }
body.nomarks .line.k-raw { border-left-color:transparent; }
body.nomarks .line.k-raw::after { content:""; }
.line.k-raw::after { content:" ⟨unclassified⟩"; color:var(--raw); font-size:.62em;
        letter-spacing:.08em; font-family:-apple-system,sans-serif; }
body.nomarks .line.k-raw::after { content:""; }
#gallery { max-width:52rem; margin:0 auto; padding:0 1.5rem; }
.grid { display:grid; grid-template-columns:repeat(auto-fill,minmax(11rem,1fr)); gap:.7rem; }
figure { margin:0; }
figure img { width:100%; height:auto; display:block; border:1px solid #26221d; }
figcaption { font-size:.65rem; color:var(--dim); font-family:-apple-system,sans-serif;
        margin-top:.25rem; word-break:break-all; }
footer { border-top:1px solid #26221d; margin-top:3rem; padding-bottom:2rem; }
footer p { font-size:.75rem; color:var(--dim); line-height:1.9;
        font-family:-apple-system,sans-serif; }
</style>
"#;

/// The page's behaviour, small enough to read in one sitting.
const SCRIPT: &str = r#"<script>
(function () {
  var lines = Array.prototype.slice.call(document.querySelectorAll('.line'));
  if (!lines.length) { return; }
  var at = 0;
  function show(next, scroll) {
    at = Math.max(0, Math.min(lines.length - 1, next));
    lines.forEach(function (line, i) {
      line.classList.toggle('now', i === at);
      line.classList.toggle('seen', i < at);
    });
    if (scroll) { lines[at].scrollIntoView({ block: 'center', behavior: 'smooth' }); }
    try { localStorage.setItem('kintsugi-at', String(at)); } catch (e) {}
  }
  try {
    var kept = parseInt(localStorage.getItem('kintsugi-at'), 10);
    if (!isNaN(kept)) { at = kept; }
  } catch (e) {}
  show(at, false);
  document.addEventListener('keydown', function (event) {
    var key = event.key;
    if (key === 'ArrowRight' || key === ' ' || key === 'PageDown' || key === 'Enter') {
      show(at + 1, true); event.preventDefault();
    } else if (key === 'ArrowLeft' || key === 'PageUp') {
      show(at - 1, true); event.preventDefault();
    } else if (key === 'Home') { show(0, true); event.preventDefault(); }
    else if (key === 'End') { show(lines.length - 1, true); event.preventDefault(); }
    else if (key === 'o') { document.body.classList.toggle('nomarks'); }
  });
  document.addEventListener('click', function (event) {
    if (event.target.closest('#gallery')) { return; }
    show(at + (event.shiftKey ? -1 : 1), true);
  });
  var here = document.createElement('div');
  here.style.cssText = 'position:fixed;right:.8rem;bottom:.8rem;font-size:.7rem;' +
    'color:#8a8175;font-family:-apple-system,sans-serif;background:#0d0c0b;padding:.2rem .5rem;' +
    'border:1px solid #26221d;border-radius:.2rem';
  document.body.appendChild(here);
  setInterval(function () {
    here.textContent = (at + 1) + ' / ' + lines.length;
  }, 120);
})();
</script>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use kintsugi_core::vfs::VirtualPath;

    #[test]
    fn a_dialogue_line_cannot_become_markup() {
        // A stray `<` in a 2008 script would eat the rest of the page.
        assert_eq!(
            escape("a < b & c > d \"e\" 'f'"),
            "a &lt; b &amp; c &gt; d &quot;e&quot; &#39;f&#39;"
        );
    }

    #[test]
    fn only_the_commands_a_reader_reads_become_lines() {
        let script = Script {
            source: VirtualPath::new("story.bdt"),
            commands: vec![
                Command::Label("start".to_string()),
                Command::Narration("the first line".to_string()),
                Command::Dialogue {
                    speaker: Some("her".to_string()),
                    text: "the second line".to_string(),
                },
                Command::RawLine("the crack".to_string()),
                Command::SetBackground("bg/room".to_string()),
                Command::Wait(3),
                Command::End,
            ],
            warnings: vec!["something the seam wants kept".to_string()],
        };
        let lines = collect(&script);
        let kinds: Vec<&str> = lines.iter().map(|line| line.kind).collect();
        assert_eq!(kinds, ["label", "narration", "dialogue", "raw"]);
        assert_eq!(lines[2].speaker.as_deref(), Some("her"));
    }

    #[test]
    fn the_seam_warnings_and_the_marks_reach_the_page() {
        let script = Script {
            source: VirtualPath::new("story.bdt"),
            commands: vec![Command::RawLine("an unclassified line".to_string())],
            warnings: vec!["<not markup>".to_string()],
        };
        let html = render(
            &collect(&script),
            &script,
            &Player {
                game: "a game",
                engine: "bluegale",
                source: "story.bdt",
                pictures: &[],
            },
        );
        assert!(
            html.contains("&lt;not markup&gt;"),
            "a warning was not escaped"
        );
        assert!(html.contains("k-raw"), "the crack lost its mark");
        assert!(html.contains("source: story.bdt"));
    }
}
