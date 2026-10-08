//! The stage: the program table, the resource names, and the walk that turns
//! a compiled story into commands with the scenery attached.
//!
//! What the flat string table cannot say, the code says in four proven
//! instructions (measured on the 1,062,484-byte story this seam was written
//! against, sample sizes in `docs/RESEARCH-BSX.md`):
//!
//! * `31 <index:u32>` — a program's entry marker; the program table is found
//!   by looking for the record whose entries all point at one of these.
//! * `1a <box:u8> <line:u32>` — show a line of the story.
//! * `08 <resource:u32>` — show or play a named resource; the name classifies
//!   it (`bgm*` is music, `se*` a sound, the digits a voice, the rest a
//!   picture).
//! * `<ch> 00 00 00 <resource:u32>` — the voice sitting in front of its line.
//! * `2c <id:u32> <program:u32> <label_line:u32>` — offer a choice; on
//!   selection, enter the named program. `2d ff ff ff ff` closes the list.
//!
//! The walk is prioritised, not linear: at each offset the longest proven
//! instruction wins and the walk steps over it, so an operand byte can never
//! be mistaken for an opcode. The housekeeping instructions between anchors
//! have ambiguous lengths (the research notes say why), so they are skipped
//! byte by byte rather than parsed — the anchors are what a playthrough
//! needs, and they are self-delimiting.
//!
//! What is still not read: the instruction that calls a program (so programs
//! play in file order, one after another, and the choice instructions are the
//! only transfers honoured), `06 <u32>`, and records whose meaning is unknown.
//! The warnings say so, on screen, every time.

use std::collections::BTreeMap;

use kintsugi_core::script::{ChoiceOption, Command};

use crate::script::{Record, Story};

/// The longest label the UI is asked to draw: a program's name, or its index
/// when the table has no name for it.
fn program_label(index: usize, name: Option<&str>) -> String {
    match name {
        Some(name) if !name.is_empty() => format!("bsx:{index}:{name}"),
        _ => format!("bsx:{index}"),
    }
}

/// One program: a named stretch of code with a measured boundary.
#[derive(Clone, Debug)]
pub struct Program {
    /// Its index in the program table.
    pub index: usize,
    /// Its name, when the story carries names.
    pub name: Option<String>,
    /// Absolute offset of the first byte (the `31` marker).
    pub start: usize,
    /// Absolute offset one past the last byte.
    pub end: usize,
}

/// One instruction the walk trusts, in the order the code holds them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Anchor {
    /// `1a <box> <line>` — show a line.
    Show { at: usize, channel: u8, line: usize },
    /// `08 <resource>` — show or play a resource.
    Resource { at: usize, resource: usize },
    /// `<ch> 00 00 00 <resource>` — the voice in front of a line.
    Voice { at: usize, resource: usize },
    /// `2c <id> <program> <label_line>` — offer an option.
    Offer {
        at: usize,
        target: usize,
        label_line: usize,
    },
    /// `2d ff ff ff ff` — the option list is complete.
    OfferListEnd { at: usize },
    /// `03 <program>` as a program's final instruction: where the story goes
    /// when this program ends. Measured on the release: ten sites, each the
    /// last five bytes of its program — the opening spine, the endings
    /// returning to the main menu, and six branches naming their own merge
    /// point. The same byte pair mid-program is the housekeeping shape of
    /// the music macros, so position, not pattern, is what proves it.
    Call { at: usize, target: usize },
}

/// The guarded regions of a program: `02|03 01 <var> <op> 01 <value> 06
/// <rel32>` — a condition twelve bytes long, then the guard instruction,
/// whose forward target is `rel` past the code base. The bytes between the
/// guard and its target run only when the condition holds (measured on the
/// release: all 92 `06` sites carry exactly this preamble, every target
/// forward). The walk never evaluates the condition; it only needs to know
/// WHICH bytes a condition owns.
fn guard_regions(bytes: &[u8], program: &Program, base: usize) -> Vec<(usize, usize)> {
    let mut regions = Vec::new();
    let mut at = program.start;
    while at + 17 <= program.end {
        let matches = matches!(bytes.get(at), Some(0x02) | Some(0x03))
            && bytes.get(at + 1) == Some(&0x01)
            && bytes.get(at + 7) == Some(&0x01)
            && bytes.get(at + 12) == Some(&0x06);
        if matches {
            if let Some(rel) = u32_at(bytes, at + 13) {
                let target = base.saturating_add(rel);
                if at + 17 < target {
                    regions.push((at + 17, target.min(program.end)));
                    at += 17;
                    continue;
                }
            }
        }
        at += 1;
    }
    regions
}

/// The channel bytes the voice instruction has been measured to use.
const VOICE_CHANNELS: [u8; 8] = [1, 2, 3, 6, 7, 8, 18, 19];

/// The only bytes ever observed directly after a real `08 <resource>` (705
/// measured sites, nine values, all of them opcodes; the research notes carry
/// the count). A candidate followed by anything else is an operand byte.
const AFTER_RESOURCE: [u8; 9] = [0x1a, 0x11, 0x08, 0x00, 0x10, 0x04, 0x31, 0x01, 0x09];

fn u32_at(bytes: &[u8], at: usize) -> Option<usize> {
    let four: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(four) as usize)
}

/// Find the program table: the record whose `(code_off, name_off)` pairs all
/// point at a `31 <self-index>` marker. Self-verifying, because a coincidental
/// byte pattern cannot arrange two hundred and eleven correct markers.
pub fn programs(story: &Story) -> Option<Vec<Program>> {
    let bytes = story.bytes();
    let base = story.code_base();
    for record in story.records() {
        if record.len % 8 != 0 || record.len < 16 || record.len / 8 > 4096 {
            continue;
        }
        let count = record.len / 8;
        let mut starts = Vec::with_capacity(count);
        let mut name_offsets = Vec::with_capacity(count);
        let mut verified = true;
        for i in 0..count {
            let Some(code_off) = u32_at(bytes, record.at + i * 8) else {
                verified = false;
                break;
            };
            let Some(name_off) = u32_at(bytes, record.at + i * 8 + 4) else {
                verified = false;
                break;
            };
            let start = base.checked_add(code_off)?;
            // The marker must say this program's own index.
            if bytes.get(start) != Some(&0x31) || u32_at(bytes, start + 1) != Some(i) {
                verified = false;
                break;
            }
            starts.push(start);
            name_offsets.push(name_off);
        }
        if !verified {
            continue;
        }

        // Names: the offsets may be absolute or relative to a sibling record.
        let names = read_names(bytes, story.records(), &name_offsets);

        // Boundaries: a program ends where the next one begins; the last ends
        // where the table itself sits.
        let mut order: Vec<usize> = (0..count).collect();
        order.sort_by_key(|&i| starts[i]);
        let mut programs = Vec::with_capacity(count);
        for (position, &i) in order.iter().enumerate() {
            let end = match order.get(position + 1) {
                Some(&next) => starts[next],
                None => record.at,
            };
            programs.push(Program {
                index: i,
                name: names.as_ref().and_then(|n| n.get(i).cloned()).flatten(),
                start: starts[i],
                end: end.max(starts[i]),
            });
        }
        programs.sort_by_key(|program| program.start);
        return Some(programs);
    }
    None
}

/// Read `count` NUL-terminated CP932 names, trying the offsets as absolute
/// first and as record-relative second. Returns one entry per offset; an
/// unreadable name is `Some(None)` rather than a shifted table.
fn read_names(bytes: &[u8], records: &[Record], offsets: &[usize]) -> Option<Vec<Option<String>>> {
    let read_one = |at: usize| -> Option<String> {
        let end = bytes
            .get(at..)?
            .iter()
            .position(|&b| b == 0)
            .map(|p| at + p)?;
        let raw = bytes.get(at..end)?;
        if raw.is_empty() || raw.len() > 64 {
            return None;
        }
        Some(crate::decode_cp932(raw).0)
    };
    let absolute: Vec<Option<String>> = offsets.iter().map(|&at| read_one(at)).collect();
    if absolute.iter().flatten().count() * 2 >= offsets.len() {
        return Some(absolute);
    }
    for record in records {
        let relative: Vec<Option<String>> = offsets
            .iter()
            .map(|&off| read_one(record.at.checked_add(off)?))
            .collect();
        if relative.iter().flatten().count() * 2 >= offsets.len() {
            return Some(relative);
        }
    }
    None
}

/// The resource names: a record of `u32` offsets into a sibling record of
/// NUL-terminated CP932 names. The story's own string table has the same
/// shape, so it is excluded by identity, and the candidate with the most
/// numeric names wins — a story has few all-digit lines, a resource table is
/// full of them (the voices).
pub fn resources(story: &Story) -> Option<Vec<String>> {
    let bytes = story.bytes();
    let mut best: Option<(usize, Vec<String>)> = None;
    for record in story.records() {
        if record.len % 4 != 0 || record.len < 8 || record.len / 4 > 1 << 16 {
            continue;
        }
        // The story's own index has the same shape as the resource names';
        // it is excluded by identity, not outscored.
        if record.at == story.table_index_at() {
            continue;
        }
        let count = record.len / 4;
        let mut offsets = Vec::with_capacity(count);
        let mut ascending = true;
        for i in 0..count {
            let Some(off) = u32_at(bytes, record.at + i * 4) else {
                ascending = false;
                break;
            };
            if i > 0 && off < offsets[i - 1] {
                ascending = false;
            }
            offsets.push(off);
        }
        if !ascending {
            continue;
        }
        // The offsets name a block: relative to a sibling record, or
        // absolute. Every base is tried; the one that parses the most names
        // (then the most numeric ones) speaks for this record.
        let mut best_for_record: Option<(usize, usize, Vec<String>)> = None;
        for base in story
            .records()
            .iter()
            .filter(|sibling| sibling.at != record.at)
            .map(|sibling| sibling.at)
            .chain(std::iter::once(0usize))
        {
            let Some(limit) = base.checked_add(offsets.iter().max().copied().unwrap_or(0)) else {
                continue;
            };
            if limit >= bytes.len() {
                continue;
            }
            let mut names = Vec::with_capacity(count);
            let mut parsed = 0usize;
            for &off in &offsets {
                match bytes.get(base + off..).and_then(|rest| {
                    let end = rest.iter().position(|&b| b == 0)?;
                    let name = crate::decode_cp932(&rest[..end]).0;
                    // A decoded control character is the walk reading the
                    // wrong block, not a resource with a strange name.
                    if name.chars().any(|c| c.is_control()) {
                        None
                    } else {
                        Some(name)
                    }
                }) {
                    Some(name) if !name.is_empty() => {
                        parsed += 1;
                        names.push(name);
                    }
                    _ => names.push(String::new()),
                }
            }
            if parsed * 2 < count {
                continue;
            }
            let numeric = names
                .iter()
                .filter(|name| name.chars().all(|c| c.is_ascii_digit()) && !name.is_empty())
                .count();
            let better = match &best_for_record {
                None => true,
                Some((best_parsed, best_numeric, _)) => {
                    (parsed, numeric) > (*best_parsed, *best_numeric)
                }
            };
            if better {
                best_for_record = Some((parsed, numeric, names));
            }
        }
        if let Some((_, numeric, names)) = best_for_record {
            if best.as_ref().is_none_or(|(score, _)| numeric > *score) {
                best = Some((numeric, names));
            }
        }
    }
    best.map(|(_, names)| names)
}

/// What the code says about one program, in code order.
fn anchors(story: &Story, program: &Program, resources: usize, programs: usize) -> Vec<Anchor> {
    let bytes = story.bytes();
    let lines = story.strings().len();
    let mut anchors = Vec::new();
    let mut at = program.start;
    while at < program.end.min(bytes.len()) {
        let byte = bytes[at];
        // Longest and rarest first: an accepted anchor's operand bytes are
        // stepped over, so they can never be read as opcodes.
        if byte == 0x2C && at + 13 <= program.end {
            if let (Some(id), Some(target), Some(label_line)) = (
                u32_at(bytes, at + 1),
                u32_at(bytes, at + 5),
                u32_at(bytes, at + 9),
            ) {
                // 24 measured sites have ids 0..23; a generous bound keeps a
                // bigger game readable without opening the door to noise.
                if id < 1024 && target < programs && label_line < lines {
                    anchors.push(Anchor::Offer {
                        at,
                        target,
                        label_line,
                    });
                    at += 13;
                    continue;
                }
            }
        }
        if byte == 0x2D && bytes.get(at..at + 5) == Some(&[0x2D, 0xFF, 0xFF, 0xFF, 0xFF][..]) {
            anchors.push(Anchor::OfferListEnd { at });
            at += 5;
            continue;
        }
        if byte == 0x03 && at + 5 == program.end {
            if let Some(target) = u32_at(bytes, at + 1) {
                if target < programs && target != program.index {
                    anchors.push(Anchor::Call { at, target });
                    at += 5;
                    continue;
                }
            }
        }
        if VOICE_CHANNELS.contains(&byte)
            && at + 8 <= program.end
            && bytes.get(at + 1..at + 4) == Some(&[0, 0, 0][..])
        {
            if let Some(resource) = u32_at(bytes, at + 4) {
                if resource < resources {
                    anchors.push(Anchor::Voice { at, resource });
                    at += 8;
                    continue;
                }
            }
        }
        if byte == 0x1A && at + 6 <= program.end {
            let channel = bytes[at + 1];
            if channel < crate::script::SHOW_CHANNELS {
                if let Some(line) = u32_at(bytes, at + 2) {
                    if line < lines {
                        anchors.push(Anchor::Show { at, channel, line });
                        at += 6;
                        continue;
                    }
                }
            }
        }
        if byte == 0x08 && at + 5 <= program.end {
            if let Some(resource) = u32_at(bytes, at + 1) {
                let next_ok = at + 5 >= program.end || AFTER_RESOURCE.contains(&bytes[at + 5]);
                if resource < resources && next_ok {
                    anchors.push(Anchor::Resource { at, resource });
                    at += 5;
                    continue;
                }
            }
        }
        at += 1;
    }
    anchors
}

/// How a resource name classifies itself. Measured on the release: `bgm*` is
/// music, `se*` a sound effect, an all-digit name a voice file, and the rest
/// are pictures — backgrounds, event images, eyecatches.
fn classify(name: &str) -> ResourceClass {
    if name.is_empty() {
        return ResourceClass::Unknown;
    }
    if name.chars().all(|c| c.is_ascii_digit()) {
        ResourceClass::Voice
    } else if name.starts_with("bgm") {
        ResourceClass::Music
    } else if name.starts_with("se") || name.starts_with("necha") {
        ResourceClass::Sound
    } else {
        ResourceClass::Picture
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResourceClass {
    Music,
    Sound,
    Voice,
    Picture,
    Unknown,
}

/// What the walk produced: the command list, the map from a text command to
/// the story line it shows, and the honest notes about what was not read.
pub struct Emission {
    /// The commands, in play order.
    pub commands: Vec<Command>,
    /// `(command index, story line)` for every command a translation may move.
    pub text: Vec<(usize, usize)>,
    /// The seam's notes, for the warning line.
    pub notes: Vec<String>,
}

/// Walk the whole story into commands. `resolve` turns a resource name into
/// the path a shell can open; a name it cannot resolve is skipped and counted,
/// never guessed.
pub fn emit(
    story: &Story,
    programs: &[Program],
    names: &[String],
    resolve: &dyn Fn(&str) -> Option<String>,
) -> Emission {
    let mut commands = Vec::new();
    let mut text = Vec::new();
    let mut notes = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();

    // The last line each program shows, for the branch-merge rule.
    let all_anchors: Vec<Vec<Anchor>> = programs
        .iter()
        .map(|program| anchors(story, program, names.len(), programs.len()))
        .collect();
    let last_line: Vec<Option<usize>> = all_anchors
        .iter()
        .map(|anchors| {
            anchors
                .iter()
                .filter_map(|anchor| match anchor {
                    Anchor::Show { line, .. } => Some(*line),
                    _ => None,
                })
                .max()
        })
        .collect();

    // Which programs are entered by a choice, and their sibling groups,
    // remembering the choice's owner so the merge search can never point a
    // branch back into it (which would loop the choice forever).
    let mut branch_targets: BTreeMap<usize, (usize, Vec<usize>)> = BTreeMap::new();
    for (index, anchors) in all_anchors.iter().enumerate() {
        let targets: Vec<usize> = anchors
            .iter()
            .filter_map(|anchor| match anchor {
                Anchor::Offer { target, .. } => Some(*target),
                _ => None,
            })
            .collect();
        if !targets.is_empty() {
            // Key the group by its smallest target so the merge is computed
            // once per choice point.
            branch_targets
                .entry(targets.iter().min().copied().unwrap_or(0))
                .or_insert((programs[index].index, targets.clone()));
        }
    }
    // A link that points backward — an ending returning to the main menu —
    // is true of the game but fatal to a walk with no menu state: followed,
    // it would loop forever. Backward links are counted, not followed.
    let start_of: BTreeMap<usize, usize> = programs
        .iter()
        .map(|program| (program.index, program.start))
        .collect();
    let mut backward_links = 0usize;
    let mut forward_link_of: BTreeMap<usize, usize> = BTreeMap::new();
    for (position, anchors) in all_anchors.iter().enumerate() {
        if let Some(Anchor::Call { target, .. }) = anchors
            .iter()
            .find(|anchor| matches!(anchor, Anchor::Call { .. }))
        {
            if start_of[target] > programs[position].start {
                forward_link_of.insert(programs[position].index, *target);
            } else {
                backward_links += 1;
            }
        }
    }
    let mut merge_of: BTreeMap<usize, usize> = BTreeMap::new();
    for (owner, targets) in branch_targets.values() {
        let Some(merge_line) = targets
            .iter()
            .filter_map(|&target| {
                let position = programs.iter().position(|p| p.index == target)?;
                last_line.get(position).copied().flatten()
            })
            .max()
            .map(|line| line + 1)
        else {
            continue;
        };
        // The merge program is the first in file order, outside the sibling
        // group, that shows the line after both branches end.
        for (position, anchors) in all_anchors.iter().enumerate() {
            if programs[position].index == *owner || targets.contains(&programs[position].index) {
                continue;
            }
            let shows_merge = anchors.iter().any(|anchor| match anchor {
                Anchor::Show { line, .. } => *line == merge_line,
                _ => false,
            });
            if shows_merge {
                for &target in targets {
                    merge_of.insert(target, programs[position].index);
                }
                break;
            }
        }
    }

    // The ending cards. The compiler numbered them first — lines 0..5 of the
    // measured story — and placed their programs in front of the story, so a
    // plain address-order read opens with the endings. But every one of
    // those lines is shown from inside a route guard (`02|03 01 <var> <op>
    // 01 <value> 06 <rel>`, measured at all 92 sites of the release): the
    // code plays them at the END of a playthrough, and only when its
    // condition holds. The walk cannot evaluate the guard, but it can see
    // the guard — so a program whose every shown line is guard-gated is the
    // ending of a route, and the honest place to read it is after the story
    // it ends. Programs with no lines (menus, preloaders, replay stubs)
    // stay where they lie.
    let regions_of: Vec<Vec<(usize, usize)>> = programs
        .iter()
        .map(|program| guard_regions(story.bytes(), program, story.code_base()))
        .collect();
    let movers: Vec<usize> = (0..programs.len())
        .filter(|position| {
            let anchors = &all_anchors[*position];
            let shows: Vec<usize> = anchors
                .iter()
                .filter_map(|anchor| match anchor {
                    Anchor::Show { at, .. } => Some(*at),
                    _ => None,
                })
                .collect();
            !shows.is_empty()
                && !anchors
                    .iter()
                    .any(|anchor| matches!(anchor, Anchor::Offer { .. }))
                && shows.iter().all(|at| {
                    regions_of[*position]
                        .iter()
                        .any(|(start, end)| *start <= *at && at < end)
                })
        })
        .collect();
    if !movers.is_empty() {
        notes.push(format!(
            "{} program(s) show every line they have inside route guards the walk reads but \
             does not evaluate — the ending cards of a playthrough — so they are read after \
             the story, not where the compiler placed them.",
            movers.len()
        ));
    }
    let emission_order: Vec<usize> = (0..programs.len())
        .filter(|position| !movers.contains(position))
        .chain(movers.iter().copied())
        .collect();

    let mut unresolved_merges = 0usize;
    for position in emission_order {
        let program = &programs[position];
        commands.push(Command::Label(program_label(
            program.index,
            program.name.as_deref(),
        )));
        let mut offers: Vec<ChoiceOption> = Vec::new();
        let mut offer_lines: Vec<usize> = Vec::new();
        for anchor in &all_anchors[position] {
            match *anchor {
                Anchor::Show { channel, line, .. } => {
                    let text_of = story.strings()[line].clone();
                    let command = match channel {
                        0 => Command::Narration(text_of),
                        _ => Command::Dialogue {
                            speaker: None,
                            text: text_of,
                        },
                    };
                    text.push((commands.len(), line));
                    commands.push(command);
                }
                Anchor::Resource { resource, .. } => {
                    let name = &names[resource];
                    match classify(name) {
                        ResourceClass::Music => match resolve(name) {
                            Some(path) => commands.push(Command::PlayMusic(Some(path))),
                            // A music resource with no file is this engine's
                            // way of stopping the music (measured: bgm99 has
                            // no file on the disc and 14 siblings that do).
                            None => commands.push(Command::PlayMusic(None)),
                        },
                        ResourceClass::Sound | ResourceClass::Voice => match resolve(name) {
                            Some(path) => commands.push(Command::PlaySound(path)),
                            None => unresolved.push(name.clone()),
                        },
                        ResourceClass::Picture => match resolve(name) {
                            Some(path) => commands.push(Command::SetBackground(path)),
                            None => unresolved.push(name.clone()),
                        },
                        ResourceClass::Unknown => {}
                    }
                }
                Anchor::Voice { resource, .. } => {
                    let name = &names[resource];
                    match resolve(name) {
                        Some(path) => commands.push(Command::PlaySound(path)),
                        None => unresolved.push(name.clone()),
                    }
                }
                Anchor::Offer {
                    target, label_line, ..
                } => {
                    let label = story.strings()[label_line].clone();
                    let name = programs
                        .iter()
                        .find(|p| p.index == target)
                        .and_then(|p| p.name.as_deref());
                    offer_lines.push(label_line);
                    offers.push(ChoiceOption {
                        label,
                        goto: program_label(target, name),
                    });
                }
                Anchor::OfferListEnd { .. } => {
                    if !offers.is_empty() {
                        // The labels are story lines too: offered to the
                        // translator under the choice command's own
                        // position, one entry per option, in order.
                        let position = commands.len();
                        for line in offer_lines.drain(..) {
                            text.push((position, line));
                        }
                        commands.push(Command::Choice(std::mem::take(&mut offers)));
                    }
                }
                // A tail call acts when the program ends, which is after the
                // anchors — handled below.
                Anchor::Call { .. } => {}
            }
        }
        // Where this program goes when it ends: the call it names itself,
        // when it names one...
        if let Some(&target) = forward_link_of.get(&program.index) {
            let name = programs
                .iter()
                .find(|p| p.index == target)
                .and_then(|p| p.name.as_deref());
            commands.push(Command::Jump(program_label(target, name)));
        // ...and for a branch that names none, the program that continues
        // the story after both branches, so the route the reader did not
        // pick does not play.
        } else if let Some(&merge) = merge_of.get(&program.index) {
            let name = programs
                .iter()
                .find(|p| p.index == merge)
                .and_then(|p| p.name.as_deref());
            commands.push(Command::Jump(program_label(merge, name)));
        } else if branch_targets
            .values()
            .any(|(_, group)| group.contains(&program.index))
        {
            unresolved_merges += 1;
        }
    }

    let links = forward_link_of.len();
    if links > 0 {
        notes.push(format!(
            "{links} program-to-program link(s) are read from tail calls; the conditional and \
             mid-program calls — the ones sitting inside a guard's region, whose condition \
             the walk reads for placement but never evaluates — are noted but not followed."
        ));
    }
    if backward_links > 0 {
        notes.push(format!(
            "{backward_links} backward link(s) — endings returning to the main menu — are not \
             followed: a walk with no menu state would loop forever."
        ));
    }
    // The lines no instruction shows are still the story's words: kept, at
    // the end, as raw lines, exactly as the text-only walk kept them. Choice
    // labels are shown — as the choice — so they are not "unshown".
    let mut shown: std::collections::BTreeSet<usize> = text.iter().map(|(_, line)| *line).collect();
    for anchors in &all_anchors {
        for anchor in anchors {
            if let Anchor::Offer { label_line, .. } = anchor {
                shown.insert(*label_line);
            }
        }
    }
    let mut kept = 0usize;
    for (line, text_of) in story.strings().iter().enumerate() {
        if !shown.contains(&line) {
            text.push((commands.len(), line));
            commands.push(Command::RawLine(text_of.clone()));
            kept += 1;
        }
    }
    commands.push(Command::End);

    if kept > 0 {
        notes.push(format!(
            "{kept} line(s) the code never shows are kept at the end as raw lines rather than dropped."
        ));
    }
    if !unresolved.is_empty() {
        unresolved.sort();
        unresolved.dedup();
        notes.push(format!(
            "The code names {} resource(s) with no file in the archives ({}): engine built-ins \
             this seam cannot draw, skipped rather than guessed.",
            unresolved.len(),
            unresolved
                .iter()
                .take(6)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if unresolved_merges > 0 {
        notes.push(format!(
            "{unresolved_merges} branch(es) end where the story's continuation could not be \
             found; they fall through to the next program in the file instead."
        ));
    }
    Emission {
        commands,
        text,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::make_staged_story;

    fn staged() -> Story {
        Story::parse(&make_staged_story()).expect("the staged fixture parses")
    }

    #[test]
    fn the_program_table_is_found_by_its_own_markers() {
        let story = staged();
        let programs = programs(&story).expect("the fixture carries a program table");
        assert_eq!(programs.len(), 5);
        // The walk orders programs by address, not by the table: the ending
        // is table program 4 but the first bytes of the code.
        assert_eq!(programs[0].name.as_deref(), Some("ending"));
        assert_eq!(programs[0].index, 4);
        assert_eq!(programs[1].name.as_deref(), Some("opening"));
        assert_eq!(programs[2].name.as_deref(), Some("branch_a"));
        assert!(programs[0].start < programs[1].start);
        assert!(programs[1].start < programs[2].start);
    }

    #[test]
    fn the_resource_names_are_found_and_not_confused_with_the_story() {
        let story = staged();
        let names = resources(&story).expect("the fixture carries resource names");
        assert_eq!(names, vec!["se01", "bgm01", "bg01", "10100001"]);
    }

    #[test]
    fn the_walk_sets_the_scene_before_the_line() {
        let story = staged();
        let programs = programs(&story).unwrap();
        let names = resources(&story).unwrap();
        let emission = emit(&story, &programs, &names, &|name| {
            Some(format!("res/{name}"))
        });
        let text_of: Vec<&str> = emission
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Narration(text) => Some(text.as_str()),
                Command::Dialogue { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        // The story in order, then the guarded ending after it.
        assert_eq!(
            text_of,
            vec![
                "line zero",
                "line three",
                "branch A text",
                "branch B text",
                "the merge",
                "the ending"
            ]
        );
        assert!(
            emission
                .notes
                .iter()
                .any(|note| note.contains("ending cards")),
            "the reorder explains itself: {:?}",
            emission.notes
        );
        // The scenery arrives before its line.
        let first_bg = emission
            .commands
            .iter()
            .position(|c| matches!(c, Command::SetBackground(p) if p == "res/bg01"))
            .expect("a background is set");
        let first_line = emission
            .commands
            .iter()
            .position(|c| matches!(c, Command::Narration(_)))
            .unwrap();
        assert!(
            first_bg < first_line,
            "the stage is set before the story speaks"
        );
        assert!(
            emission
                .commands
                .iter()
                .any(|c| matches!(c, Command::PlayMusic(Some(p)) if p == "res/bgm01")),
            "the music starts"
        );
        assert!(
            emission
                .commands
                .iter()
                .any(|c| matches!(c, Command::PlaySound(p) if p == "res/10100001")),
            "the voice plays"
        );
    }

    #[test]
    fn a_choice_jumps_to_its_branch_and_the_branch_returns_to_the_story() {
        let story = staged();
        let programs = programs(&story).unwrap();
        let names = resources(&story).unwrap();
        let emission = emit(&story, &programs, &names, &|name| {
            Some(format!("res/{name}"))
        });
        let choice = emission
            .commands
            .iter()
            .find_map(|command| match command {
                Command::Choice(options) => Some(options),
                _ => None,
            })
            .expect("the fixture offers a choice");
        assert_eq!(choice.len(), 2);
        assert_eq!(choice[0].label, "pick the first branch");
        assert_eq!(choice[0].goto, "bsx:1:branch_a");
        assert_eq!(choice[1].goto, "bsx:2:branch_b");
        // Both branches jump to the program that shows the next line.
        let jumps: Vec<&str> = emission
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Jump(label) => Some(label.as_str()),
                _ => None,
            })
            .collect();
        // branch_a by its own closing call, branch_b by the merge rule.
        assert_eq!(jumps, vec!["bsx:3:merge", "bsx:3:merge"]);
        // The mid-program 03 pair of the music-macro shape is never followed:
        // no jump names macEjaculate1's fixture analogue, program 1's sibling.
        assert!(
            !jumps.contains(&"bsx:1:branch_a"),
            "a mid-program 03 is not a link: {jumps:?}"
        );
    }

    #[test]
    fn unresolved_resources_are_counted_not_guessed() {
        let story = staged();
        let programs = programs(&story).unwrap();
        let names = resources(&story).unwrap();
        let emission = emit(&story, &programs, &names, &|_| None);
        assert!(
            !emission
                .commands
                .iter()
                .any(|c| matches!(c, Command::SetBackground(_))),
            "nothing is set from a name with no file"
        );
        // An unresolved music name still stops the music: that reading is
        // measured, and silence needs no file.
        assert!(
            emission
                .commands
                .iter()
                .any(|c| matches!(c, Command::PlayMusic(None))),
            "an unresolved music name stops the music"
        );
        assert!(
            emission
                .notes
                .iter()
                .any(|note| note.contains("no file in the archives"))
        );
    }

    #[test]
    fn the_lines_no_instruction_shows_are_kept_as_raw_lines() {
        let story = staged();
        let programs = programs(&story).unwrap();
        let names = resources(&story).unwrap();
        let emission = emit(&story, &programs, &names, &|name| {
            Some(format!("res/{name}"))
        });
        assert!(
            emission
                .commands
                .iter()
                .any(|c| matches!(c, Command::RawLine(text) if text == "never shown")),
            "the unshown line is kept"
        );
        // And the text map still names every command a translation may move:
        // five shown, the guarded ending, one kept, and the two labels under
        // their choice's id.
        let lines: Vec<usize> = emission.text.iter().map(|(_, line)| *line).collect();
        assert_eq!(
            lines.len(),
            9,
            "five shown, one ending, one kept, two choice labels"
        );
        let choice = emission
            .commands
            .iter()
            .position(|c| matches!(c, Command::Choice(_)))
            .expect("the choice is a command");
        let label_entries: Vec<usize> = emission
            .text
            .iter()
            .filter(|(position, _)| *position == choice)
            .map(|(_, line)| *line)
            .collect();
        assert_eq!(
            label_entries.len(),
            2,
            "both labels are offered under the choice's own position"
        );
    }
}
