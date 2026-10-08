//! The NScripter command vocabulary, and the one rule that decides whether a
//! line is a command at all.
//!
//! # Where this list comes from
//!
//! NScripter's command names are an interface, not code: they are what a
//! script says, and a translation tool has to know them to tell prose from
//! commands. The list below was read out of [ONScripter] — ogapee's
//! reimplementation of the engine, GPL-2 — by taking the name of every
//! `*Command()` definition in `ScriptParser_command.cpp` and
//! `ONScripter_command.cpp`. Those two files are the engine's own dispatch
//! table, so this is the vocabulary the engine itself answers to, plus the
//! ONScripter extensions (`lsp`, `csp`, `menu_window`, …) that real games in
//! the wild use. No ONScripter code is copied here; names are facts about the
//! format, like HTML tag names, and `scripts/nscripter-vocabulary.txt` records
//! how the list was extracted so it can be redone.
//!
//! # The rule for "is this line text?"
//!
//! Not "does the first token look like a command", and not "is it Japanese":
//! `ScriptHandler::readToken` sets a text flag when it meets a **multi-byte
//! character outside double quotes**, and `ONScripter::parseLine` checks that
//! flag before it looks at the command table. So a line containing a bare
//! CP932 double-byte character is prose the engine displays, and a line whose
//! bytes are all ASCII is a command — which is why an *untranslated* Japanese
//! script never needs a marker for dialogue.
//!
//! [ONScripter]: https://github.com/ogapee/onscripter

/// The commands NScripter and ONScripter answer to, sorted.
///
/// Sorted rather than grouped because the only thing done with it is
/// [`is_command`], and a sorted list can be checked by a test that does not
/// need to know the order.
pub const COMMANDS: &[&str] = &[
    "add", "addkinsoku", "allsp2hide", "allsp2resume", "allsphide",
    "allspresume", "amsp", "arc", "atoi", "autoclick",
    "automode_time", "autosaveoff", "avi", "bar", "barclear",
    "bdown", "bg", "bgcopy", "bgm", "blt",
    "br", "break", "bsp", "btn", "btndef",
    "btndown", "btntime", "btnwait", "caption", "cell",
    "checkkey", "checkpage", "chvol", "cl", "clickstr",
    "clickvoice", "cmp", "cos", "cselbtn", "cselgoto",
    "csp", "date", "dec", "defaulteffect", "defaultfont",
    "defaultspeed", "definereset", "defmp3vol", "defsevol", "defsub",
    "defvoicevol", "delay", "dim", "div", "draw",
    "drawbg", "drawbg2", "drawclear", "drawfill", "drawsp",
    "drawsp2", "drawsp3", "drawtext", "dv", "dwave",
    "dwavestop", "effect", "effectblank", "effectcut", "end",
    "english", "erasetextwindow", "exbtn", "exec_dll", "fileexist",
    "filelog", "for", "game", "getcselnum", "getcselstr",
    "getcursor", "getcursorpos", "getcursorpos2", "getenter", "getfunction",
    "getinsert", "getlog", "getmclick", "getmouseover", "getmousepos",
    "getmp3vol", "getpage", "getpageup", "getparam", "getreadlang",
    "getreg", "getret", "getsavestr", "getscreenshot", "getsevol",
    "getspmode", "getsppos", "getspsize", "gettab", "gettag",
    "gettaglog", "gettext", "gettimer", "getversion", "getvoicevol",
    "getzxc", "globalon", "gosub", "goto", "humanorder",
    "humanz", "if", "inc", "indent", "input",
    "intlimit", "isdown", "isfull", "ispage", "isskip",
    "itoa", "jumpb", "jumpf", "kidokumode", "kidokuskip",
    "kinsoku", "labellog", "langen", "langjp", "layermessage",
    "ld", "len", "linepage", "loadgame", "loadgosub",
    "locate", "logsp", "lookbackbutton", "lookbackcolor", "lookbackflush",
    "lookbacksp", "loopbgm", "loopbgmstop", "lsp", "lsp2",
    "luacall", "luasub", "maxkaisoupage", "menu_automode", "menu_click_def",
    "menu_click_page", "menu_full", "menu_window", "menuselectcolor", "menuselectvoice",
    "menusetwindow", "mid", "mod", "monocro", "mov",
    "movemousecursor", "movie", "mp3", "mp3fadein", "mp3fadeout",
    "mp3stop", "mp3vol", "mpegplay", "msp", "mul",
    "nega", "next", "nextcsel", "nsa", "nsadir",
    "numalias", "ofscopy", "pagetag", "play", "playstop",
    "pretextgosub", "print", "prnum", "prnumclear", "puttext",
    "quake", "repaint", "reset", "resettimer", "return",
    "rmenu", "rmode", "rnd", "roff", "rubyoff",
    "rubyon", "savedir", "savefileexist", "savegame", "savename",
    "savenumber", "saveoff", "saveon", "savepoint", "savescreenshot",
    "savetime", "select", "selectcolor", "selectvoice", "setcursor",
    "setkinsoku", "setlayer", "setwindow", "setwindow2", "setwindow3",
    "sevol", "shadedistance", "showlangen", "showlangjp", "sin",
    "skip", "skipoff", "soundpressplgin", "sp_rgb_gradation", "spbtn",
    "spclclk", "split", "spreload", "spstr", "stop",
    "stralias", "strsp", "sub", "systemcall", "tablegoto",
    "tal", "tan", "tateyoko", "texec", "textclear",
    "textcolor", "textgosub", "texthide", "textoff", "texton",
    "textshow", "textspeed", "textspeeddefault", "textwindow", "time",
    "transbtn", "transmode", "trap", "underline", "useescspc",
    "usewheel", "v", "versionstr", "voicevol", "vsp",
    "wait", "waittimer", "wave", "wavestop", "windowback",
    "windowchip", "yesnobox", "zenkakko",
];

/// Is `token` a command the engine answers to?
///
/// Binary search: the list is sorted and long enough that a linear scan would
/// show up in a profile of a big script.
pub fn is_command(token: &str) -> bool {
    COMMANDS.binary_search(&token).is_ok()
}

/// The first token of a line, up to whitespace.
///
/// The engine's parser reads a token and skips spaces, so this is the same
/// cut, and it is the same cut [`is_command`] is asked about.
pub fn first_token(line: &str) -> &str {
    let trimmed = line.trim_start();
    match trimmed.find(|c: char| c.is_whitespace()) {
        Some(end) => &trimmed[..end],
        None => trimmed,
    }
}

/// Whether a line holds a character the engine reads as text.
///
/// This is the engine's own test: a multi-byte character **outside double
/// quotes** makes the line prose. Quotes matter because `select "選択肢"` is a
/// command with a Japanese argument, and the engine is careful not to mistake
/// it for dialogue.
pub fn has_bare_multibyte(line: &str) -> bool {
    let mut in_quotes = false;
    for ch in line.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            // CP932's double-byte characters are the CJK ones; a translatable
            // line is one where such a character is unquoted. Testing for
            // "not ASCII" rather than a CP932 range keeps this honest for
            // comments and for the rare full-width punctuation.
            _ if !in_quotes && !ch.is_ascii() => return true,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_is_sorted_and_unique() {
        // `is_command` is a binary search, so this is a load-bearing property
        // rather than a style preference.
        for pair in COMMANDS.windows(2) {
            assert!(
                pair[0] < pair[1],
                "'{}' and '{}' are out of order or repeated",
                pair[0],
                pair[1]
            );
        }
        assert!(
            COMMANDS.len() > 250,
            "the engine answers to more than {} commands",
            COMMANDS.len()
        );
    }

    #[test]
    fn the_vocabulary_has_no_furniture() {
        for name in COMMANDS {
            assert!(
                !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "'{name}' is not a command name"
            );
        }
    }

    #[test]
    fn only_the_first_word_is_a_command() {
        assert_eq!(first_token("bg \"bg/room.bmp\",1"), "bg");
        assert_eq!(first_token("  end"), "end");
        assert_eq!(first_token("*start"), "*start");
        assert_eq!(first_token(""), "");
    }

    #[test]
    fn a_bare_multibyte_character_makes_a_line_prose() {
        assert!(has_bare_multibyte("深夜の工房。"));
        assert!(has_bare_multibyte("こんにちは。\\n"));
        // The engine's rule: inside quotes it is an argument, not dialogue.
        assert!(!has_bare_multibyte("select \"選択肢1\",\"選択肢2\""));
        assert!(!has_bare_multibyte("bg \"bg/room.bmp\",1"));
        assert!(!has_bare_multibyte("end"));
        assert!(!has_bare_multibyte(""));
    }
}
