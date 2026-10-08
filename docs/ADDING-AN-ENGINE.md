# Adding an engine

> The architecture exists so that this document is short and boring.

An engine is **one crate** that turns one dead studio's file formats into the
body's vocabulary. It does not touch the body, the glaze, the CLI, or the
Android app — except for one line of registration in each host's registry, and
that line is the only place a host is allowed to learn an engine's name.

This guide uses a fictional engine, `ExampleSoft`, and mirrors what
`crates/kintsugi-bluegale` actually does. Read that crate alongside it.

---

## 0. Before writing code

You are reverse-engineering somebody's work. Do it in the open:

1. **Look for prior art first.** GARbro, SExtractor, asmodean, xclann,
   `vn-tools`, and old forum threads are where formats are documented. Reading
   them is faster than a hex editor, and citing them is part of the job.
2. **Write down what you know and what you are guessing.** Every fact goes into
   `docs/RESEARCH-<Engine>.md` with its source. The guesses go into a table of
   open questions. This file is not documentation for users; it is evidence for
   the next contributor — including you, in a year.
3. **Synthesize your test data.** Never commit bytes from a commercial game.
   Write a fixture builder (see step 5) that emits byte-exact files from code.

---

## 1. Create the crate

```sh
cargo new --lib crates/kintsugi-examplesoft
```

```toml
# crates/kintsugi-examplesoft/Cargo.toml
[package]
name = "kintsugi-examplesoft"
description = "Golden seam: the ExampleSoft engine."
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
kintsugi-core.workspace = true
encoding_rs = { workspace = true }     # only if the engine needs CP932
```

Add `"crates/kintsugi-examplesoft"` to `members` and
`kintsugi-examplesoft = { path = "crates/kintsugi-examplesoft" }` to
`[workspace.dependencies]` in the root `Cargo.toml`.

**A seam may take dependencies. The body may not.** If you find yourself
wanting to add a crate to `kintsugi-core`, the thing you are building is glaze
or a seam, not body.

---

## 2. Write the detectors

Detection is a claim about evidence, and the ladder is enforced:

```rust
use kintsugi_core::detect::{Confidence, Detection};
use kintsugi_core::vfs::Vfs;

pub fn detect(vfs: &Vfs) -> Vec<Detection> {
    let mut out = Vec::new();

    // Certain: a verified structural signature. Here, an index whose every
    // record points inside the data file — a fact that cannot happen by
    // accident in a file that is not this format.
    if let Some((count, bytes)) = index_and_data(vfs) {
        let valid = count_valid_records(&bytes);
        if valid > 0 && valid == count {
            out.push(Detection::new(
                "examplesoft",
                Confidence::Certain,
                format!("{valid} index record(s), all pointing inside the data file"),
            ));
        }
    }

    // Loose files alone can only ever be Likely: the container is missing, so
    // the structure we would verify is missing with it.
    if out.is_empty() && vfs.find_by_extension(&["exb"]).len() > 0 {
        out.push(Detection::new(
            "examplesoft",
            Confidence::Likely,
            "loose .exb script(s) with no archive beside them",
        ));
    }

    out
}
```

Rules:

* **Never** return `Certain` because of a file extension, a folder name, or a
  magic number you have not verified. `Certain` is for signatures you could
  explain to a sceptic.
* Put the *evidence* in the note. It is printed to the user; it is the visible
  gold.
* Detection reads. It never writes, moves, renames, or repairs.
* Return one verdict per opinion, strongest first — `Registry::detect_all`
  ranks them and `mount_best` refuses anything below `Possible`.

---

## 3. Translate the formats

One module per format, each with a `parse_*` that returns body types:

| your format | return type | notes |
| --- | --- | --- |
| archive/index | mount-time `Vfs` overlay | validate every offset against the data file's length |
| bitmap | `kintsugi_core::asset::Image` | RGBA8; decode to the body's format, not the engine's |
| audio | `kintsugi_core::asset::Audio` | pass-through container is fine — say so in a note |
| script | `kintsugi_core::script::Script` | see the honesty rules below |

Three habits make the difference between a seam that ages well and one that
rots:

**Bound-check everything, and fail loudly.** A corrupt file must produce
`Error::corrupt(...)` naming the offset, not a panic and not a truncated asset:

```rust
if offset as usize + size as usize > data.len() {
    return Err(Error::corrupt(
        "ExampleSoft archive",
        format!("entry '{name}' runs past the end of the data file"),
    ));
}
```

**Do not classify what you have not verified.** If you know the container but
not the opcodes, emit `Command::RawLine` for the lines you cannot type. It
plays, it can be translated by opt-in, and it is *visibly* unclassified:

```rust
// We know these are script lines. We do not know which are dialogue and
// which are commands, so they stay raw — the crack stays visible.
script.commands.push(Command::RawLine(line.to_string()));
```

**Record every workaround.** `MountInfo::note(...)` is how a heuristic becomes
visible instead of invisible:

```rust
info.note(format!("mounted '{name}': {count} entries"));
info.note("3 palette entrie(s) short of 256; clamped to what the file holds");
```

**Text encodings: decode faithfully, encode strictly.** If the engine stores
CP932, decode with `encoding_rs::SHIFT_JIS` (`decode_cp932` in the BlueGale
seam) and *encode* with a strict encoder that refuses rather than substituting
HTML character references — see `encode_cp932` and the note in
[ARCHITECTURE.md](../ARCHITECTURE.md). A silent substitution writes literal
`&#8212;` into somebody's game.

---

## 4. Implement the plugin and the mount

```rust
use kintsugi_core::plugin::{EngineMount, EnginePlugin, MountInfo, PluginMetadata};

pub const ENGINE_ID: &str = "examplesoft";

static METADATA: PluginMetadata = PluginMetadata {
    id: ENGINE_ID,
    crate_name: "kintsugi-examplesoft",
    display_name: "ExampleSoft (サンプル)",
    version: env!("CARGO_PKG_VERSION"),
    file_extensions: &["exb", "exa"],
};

pub struct ExamplePlugin;

impl EnginePlugin for ExamplePlugin {
    fn metadata(&self) -> &'static PluginMetadata { &METADATA }

    fn detect(&self, vfs: &Vfs) -> Result<Vec<Detection>> { Ok(detect(vfs)) }

    fn mount(&self, vfs: &Vfs) -> Result<Box<dyn EngineMount>> {
        let mut info = MountInfo::new(ENGINE_ID);
        // …open archives, build the overlaid VFS, add notes…
        Ok(Box::new(ExampleMount { vfs: overlaid, info }))
    }
}

/// The public constructor hosts call.
pub fn plugin() -> ExamplePlugin { ExamplePlugin }
```

`ExampleMount` implements `info`, `vfs`, and whichever of
`read_image` / `read_audio` / `read_script` / `write_script` you can honestly
support. Leave the rest out: the defaults refuse with your engine's name
attached, which is exactly the behaviour you want.

If you implement `write_script`, follow the BlueGale rule — **change only the
lines named in `replacements`, and preserve every other byte**. A patch that
silently reformats a game is worse than no patch. `bdt::rewrite_bdt` shows the
shape: one walk, shared in spirit with the parser, so a command index means the
same thing to both halves.

---

## 5. Synthesize fixtures and test end to end

`crates/kintsugi-examplesoft/src/fixtures.rs` should be able to write a tiny
but **byte-exact** game:

```rust
/// Write a synthetic ExampleSoft game: one archive, one image, one script.
pub fn write_demo_game(dir: &Path) -> Result<Vec<PathBuf>> { /* … */ }
```

Then test the seam the way a user would use it:

```rust
#[test]
fn registry_mounts_demo_game_end_to_end() {
    let temp = TempDir::new("demo");
    fixtures::write_demo_game(&temp.0).unwrap();
    let vfs = Vfs::from_directory(&temp.0).unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(plugin()));
    let (found, mount) = registry.mount_best(&vfs).unwrap();

    assert_eq!(found.metadata().id, "examplesoft");
    assert!(mount.vfs().exists(&"scene.exb".into()));
    assert!(!mount.info().notes.is_empty(), "mounting must leave a trail");
}
```

Cover, at minimum:

* every detector, including the ones that must **not** fire (feed it junk);
* round trips for each format you decode;
* a corrupt-input test per parser, asserting `Error::corrupt` rather than a panic;
* the interpreter running a script end to end through a `Host`;
* if you write scripts back: a test that a one-line translation changes exactly
  one line, and one that an unwritable character is refused by name.

---

## 6. Register it in the hosts

Two lines, one per host:

```rust
// crates/kintsugi/src/main.rs
fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register(Arc::new(kintsugi_bluegale::plugin()));
    registry.register(Arc::new(kintsugi_examplesoft::plugin()));   // ← here
    registry
}
```

```rust
// crates/kintsugi-android/src/lib.rs — same shape
```

Adding the dependency to `crates/kintsugi/Cargo.toml` and
`crates/kintsugi-android/Cargo.toml` is the whole integration. If you find
yourself editing the interpreter, the glaze, or the CLI's command handling to
make your engine work, stop: that means the body is missing an abstraction, and
the fix belongs in `kintsugi-core` where every engine benefits.

---

## 7. Document it

1. `docs/RESEARCH-<Engine>.md` — every format fact, its source, and an open
   questions table. This is the artefact that outlives your interest in the
   project.
2. A row in the README's capability table, with the honest state (including
   formats you deliberately did **not** implement).
3. Credit the prior art you read, by name and URL.

---

## 8. Checklist

- [ ] Crate added to `members` and `[workspace.dependencies]`
- [ ] Body still has zero dependencies (`cargo tree -p kintsugi-core`)
- [ ] Metadata filled in, `plugin()` exported
- [ ] Detectors: `Certain` only from a verified signature; junk must be rejected
- [ ] Every parser bounds-checks and returns `Error::corrupt`
- [ ] Unknown script lines are `RawLine`, not guesses
- [ ] Every heuristic recorded with `MountInfo::note`
- [ ] Strict encoding for text output (refuse, never substitute)
- [ ] Fixtures synthesize byte-exact files; no commercial data committed
- [ ] End-to-end mount test + corrupt-input tests
- [ ] Registered in `crates/kintsugi` and `crates/kintsugi-android`
- [ ] `cargo fmt` · `cargo clippy --all-targets --all-features -- -D warnings` · `cargo test --workspace`
- [ ] Research notes written, README row added, prior art credited
