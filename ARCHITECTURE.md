# Architecture

> 确定项目架构 这一套需要适配多种引擎的
> *"Settle the architecture — this has to fit many engines."*

That requirement is the whole design. Kintsugi is not a BlueGale tool with delusions
of grandeur; it is a **body** that stays useful while engines come and go, and
the BlueGale seam is the first proof that the body works.

---

## The five layers

```
shells      the delivery vehicle for one platform: CLI, Android activity, future web/iOS
   │        contains no engine logic at all
host        wires the registry, implements Host (what the game can ask for)
   │        kintsugi CLI · kintsugi-android bridge
glaze       cross-engine tooling that operates on the body's IR
   │        kintsugi-video (upscale, interpolate) · kintsugi-translate
seams       one crate per engine: bytes → the body's vocabulary
   │        kintsugi-bluegale (SNN/INX/ZBM/BBM/BDT)
body        engine-agnostic core, zero dependencies
            kintsugi-core (bytes, vfs, detect, asset, script, codec, runtime, registry)
```

| crate | layer | may depend on | dependencies today |
| --- | --- | --- | --- |
| `kintsugi-core` | body | nothing | **none** |
| `kintsugi-bluegale` | seam | core | `encoding_rs` (CP932) |
| `kintsugi-video` | glaze | core | none |
| `kintsugi-translate` | glaze | core | `serde`, `serde_json`, `ureq` |
| `kintsugi` | host + shell | core, seams, glaze | none of its own |
| `kintsugi-android` | host + shell | core, seams, glaze | `jni` (optional feature) |

The arrows only ever point **down**. A seam cannot see the glaze, the glaze
cannot see a seam, and the body cannot see either.

### Why the body has zero dependencies

Not asceticism — a guarantee. `kintsugi-core` cannot accidentally acquire a
dependency on an engine, a platform, an HTTP client, or a codec fad, because
it *cannot acquire a dependency at all*. Whatever else rots in twenty years,
the body still compiles, and every seam still has a vocabulary to translate
into. This is the one rule that is not negotiable.

A seam, by contrast, may pull whatever its engine needs: CP932 tables, zlib,
an audio decoder. That cost is paid by the engine that needs it, not by every
user of the body. The glaze sits with the host rather than in the core for the
same reason: upscaling and translation are *capabilities the host offers*, and
they operate on `Image` and `Script` — types every engine already speaks — so
they serve engines that do not exist yet.

---

## The seam contract

Two traits, in `kintsugi-core::plugin`:

```rust
pub trait EnginePlugin: Send + Sync {
    fn metadata(&self) -> &'static PluginMetadata;
    fn detect(&self, vfs: &Vfs) -> Result<Vec<Detection>>;
    fn mount(&self, vfs: &Vfs) -> Result<Box<dyn EngineMount>>;
}

pub trait EngineMount: Send {
    fn info(&self) -> &MountInfo;                 // engine id + visible notes
    fn vfs(&self) -> &Vfs;                        // mounted archives over loose files
    fn read_image(&self, path: &VirtualPath) -> Result<Image>;      // optional
    fn read_audio(&self, path: &VirtualPath) -> Result<Audio>;      // optional
    fn read_script(&self, path: &VirtualPath) -> Result<Script>;    // optional
    fn write_script(&self, path: &VirtualPath,
                    replacements: &BTreeMap<usize, String>) -> Result<WrittenScript>; // optional
}
```

Every method except `info` and `vfs` has a default that **refuses**:

```rust
Err(Error::unsupported(self.info().engine.clone(), "this seam does not decode images yet"))
```

That default is a design decision, not a convenience. A seam that cannot do
something must say so, in the user's face, with its own name attached. A silent
empty `Vec<u8>` or a blank image would be indistinguishable from success.

The contract is **executable**, not just described: `kintsugi-testkit` turns
the rules on this page into `assert_seam_contract(&plugin, &[fixture])`, which
every seam runs in its own test suite (BlueGale's is
`crates/kintsugi-bluegale/tests/conformance.rs`). It checks that a name is
never evidence, that a `Certain` verdict can actually be mounted, that a seam
names itself in every verdict and mount, that an empty folder is nobody's game,
and that `write_script` with no replacements returns the file byte-for-byte.
The checker has its own tests against eight deliberately broken seams, so the
rules are enforced rather than asserted. Adding engine №2 starts at
[docs/ADDING-AN-ENGINE.md](docs/ADDING-AN-ENGINE.md).

### Detection: an extension is never evidence

`Detection` carries a `Confidence` on an ordered ladder, and the ladder is
enforced by construction:

| verdict | requires |
| --- | --- |
| `Certain` | a **verified structural signature** — e.g. an INX whose records all pass `offset + size <= snn_len`, or a BBM whose three probe words are an XORed `BM` header |
| `Likely` | a plausible structure that would need one more check to be certain |
| `Possible` | the right extension and enough shape to be worth trying |
| `Unlikely` | a guess |

`Registry::mount_best` refuses to mount anything below `Possible`, and a
`Certain` verdict can never be produced by a file name — the signatures that
earn it are in the seam's code, visible and testable. Detection is also
strictly read-only: no writing, no moving, no "repairing" the fan's files
while deciding what they are.

### Why the registry is static

Seams are linked into the host and registered in a `Vec<Arc<dyn EnginePlugin>>`.
Loading `.so`/`.dll` plugins would be more fashionable, but Rust has no stable
ABI: a dynamically loaded seam would break whenever the compiler changed, which
is precisely the failure mode a preservation project must not have. Static
linking means a checkout of this repository builds a working engine in twenty
years, with no plugin ABI to rot.

The trait boundary is nevertheless **dylib-shaped** — `Send + Sync`, static
metadata, no lifetimes tied to the host, no generic parameters — so a dynamic
loader can be added later as an optimisation without rewriting a single seam.

---

## The platform seam: `Host`

Everything a game can ask a platform to do goes through one trait
(`kintsugi-core::runtime::Host`):

```rust
fn show_text(&mut self, speaker: Option<&str>, text: &str) -> Result<()>;
fn event(&mut self, event: Event<'_>) -> Result<()>;      // background, sprite, music, sfx, wait
fn choose(&mut self, options: &[ChoiceOption]) -> Result<usize>;
```

Three implementations exist today, and none of them is special:

| host | platform | what it does with a line |
| --- | --- | --- |
| `TerminalHost` | CLI | prints it, gold on dark; reads choices from stdin, or answers the first option with `--auto` |
| `TranscriptHost` | Android (and the C ABI) | records it into a `String` the UI draws; answers the first option **and says so in the transcript** |
| `RecordingHost` | tests | records for assertions |

That is the entire difference between a desktop CLI and a phone app. The
interpreter, the mounting, the asset decoding, the upscaling, and the
translation are the same code on every platform — which is also why the C ABI
harness and the JNI symbol check in CI are meaningful tests rather than
ceremony.

---

## Repair honesty, mechanically

The philosophy is not a slogan; it is enforced in four places.

### 1. Originals are never written

Seams take `&Vfs`, which is read-only by construction. No seam contains a write
at all: `EngineMount::write_script` returns `WrittenScript` bytes and never
touches a disk. Every write in the workspace is (a) a host writing a path the
user named — `crates/kintsugi/src/main.rs` for `-o out.png`, `--write-script`,
`--jsonl-dir`, the copy `install --into` builds, and
`crates/kintsugi-android/src/lib.rs` for the PNG the Android shell asks for —
or (b) the glaze helper `kintsugi-translate::write_jsonl`, which the host calls
with a path the user named. Nothing else in the tree opens a file for writing
outside tests and the fixture generator.

Naming a path is not enough, though, so the host enforces the shape of it:
**the game folder is read-only.** `ensure_outside_game` refuses `--write-script`,
`upscale -o`, and `interpolate -o` that resolve inside the directory being read,
which covers the file being read, any other original in there, and the glaze's
own artifacts. `demo` is the one writer inside a folder it created, guarded by a
`.kintsugi-demo` marker so it cannot plant fixtures over a real installation.
`install --into` is the other one, and it is guarded the same way from the other
side: it writes a `.kintsugi-install` manifest listing every file it wrote with
its size and a checksum, and it will only write into a folder that is empty or
that **verifies against that manifest** — same file set, same sizes, same
checksums. A copy with a save game in it, or a copy whose script was hand-edited,
is refused by name. So re-running the pipeline after fixing a translation is a
one-liner rather than a manual `rm -rf`, and the folder is never the thing that
decides: the manifest is checked against it, not believed.
The residual hole — a hard link to an original made outside the folder — needs
file identity that portable std does not expose, and is documented rather than
half-checked.

**And the copy says what it is.** Every command prints the seams it used; the
copy carries the same list as a file. `.kintsugi-install` is plain
tab-separated text — path last, so paths with spaces parse — recording the tool
version, the engine, the game folder, the script, the patch (size, checksum,
where it came from) and every file written, with its size and checksum. The
checksum is FNV-1a: it **identifies, it does not authenticate**, which is the
whole job — noticing that a file is not the one Kintsugi wrote. The host has no
dependencies, so the format needs none either, and `.kintsugi-install` can be
read in a terminal by the person deciding whether to trust the copy.

### 2. A patch changes only what it translates

`bdt::rewrite_bdt` walks the original file line by line with the *same* walk
`parse_bdt` uses, and replaces the text of the lines whose command index
appears in `replacements`. Line endings, blank lines, `\t` indentation, the
`$`/`%` sigil on labels (we have not verified the engine treats them alike, so
they are never rewritten), a missing trailing newline, and any bytes we did not
understand all survive byte for byte — because the writer *copies* them from the
de-XORed original instead of decoding and re-encoding the file. That detail is
load-bearing: CP932 contains hundreds of characters with more than one valid
spelling (the NEC and IBM duplicate rows, e.g. `87 90` and `81 E0` are both
`≒`), so a re-encoding writer would silently re-spell lines nobody asked to
change while reporting `replaced == 0`. Three tests hold this down, including an
untouched file that must come back byte-identical, with an assertion that the
round trip really is lossy for that input so the test cannot pass vacuously.

It also refuses replacements that would change the file's *structure* rather
than its words: a line break inside a translation, or a translation that starts
with `$`/`%` and would turn prose into a label. Command indices that do not
exist in the original are reported as `unmatched` rather than dropped, because
a translation landing on the wrong line is the worst outcome this tool could
produce.

### 3. Text that cannot be encoded is refused, by name

Old Japanese games store text in CP932. Rust's `encoding_rs` encoder replaces
characters the code page cannot hold with HTML numeric character references —
an em dash becomes the literal ASCII `&#8212;` and the game draws *that*.

This actually happened during development: the demo script contained an em dash
and a simplified-Chinese character in a Japanese line, and the CLI printed
`金継ぎ&#8212;&#8212;金&#32558;`. A repair that looks successful and is garbage
is worse than a failure. So the encoder compares its own output against a
round trip and refuses:

```
error: cannot write text as CP932: character(s) —缮 are not in that code page,
       and substituting them would change the game's words
```

The characters are named, so a human can decide: reword the line, or add a font
hack. The engine will not decide for them.

### 4. What is not understood is shown as not understood

BlueGale BDT lines are the standing example. The community knows the
*container*; nobody has published the *opcode* semantics of ordinary lines.
So every non-label line becomes `Command::RawLine` — playable, translatable by
opt-in (`--only-typed` opts out), and visibly unclassified:

```
· raw     それは修復ではなく、金継ぎ――金繕いだった。
```

`RawLine` is the crack in the ceramic. When someone reverse-engineers the
opcodes, the seam retypes them as `Dialogue`/`Jump`/`Choice` and every other
part of the system — extraction, translation, playback — picks it up with no
changes. The architecture is built to make that upgrade boring.

---

## Data-flow traces

**`kintsugi inspect GAME`**

```
Vfs::from_directory → Registry::detect_all → mount_best
    → mount.info().notes  (every heuristic, workaround, and skip)
    → Vfs::walk           (what is actually in there)
    → read_script → Command::Label / RawLine preview
```

**`kintsugi install GAME --script out.bdt --into COPY`**

Where the repair stops being a file and becomes a game you can play — in a copy,
because the original is not a place this tool writes. `install` parses the patch
with the seam's own reader, refuses any line where the patch's prose and the
game's structure disagree (ids drift between versions, and a repair on the wrong
line is worse than none), rebuilds the script from the *copy's* original bytes,
and then mounts the copy and reads it back to check that the number of changed
lines equals the number of replacements applied.

**`kintsugi translate GAME --write-script out.bdt`**

```
mount.read_script      → Script (engine-specific bytes already gone)
extract_with_raw       → Vec<TranslationEntry>  (id = command index)
   → JSONL on disk     (readable, editable, translatable by any other tool)
Translator backend     → Vec<TranslationEntry>  (ids echoed back; extra ids dropped)
   → report            (batches, changed, unchanged, hallucinated ids)
apply                  → Script copy, originals untouched
mount.write_script     → WrittenScript { data, replaced, unmatched }
host writes out.bdt    → byte-preserving patch, CP932-strict
```

**`kintsugi upscale GAME --asset title.bbm --method anime4k --factor 4`**

```
mount.read_image → Image (RGBA8)
kintsugi_video::upscale → Image      (engine-agnostic)
kintsugi_core::codec::png::encode_png → bytes
host writes the PNG
```

The glide from engine bytes to engine-agnostic types happens exactly once, in
the seam. Everything after that is the same for every engine that will ever be
added.

---

## Glaze: what it promises, and what it refuses to claim

### Upscaling

| method | claim |
| --- | --- |
| `nearest` | block replication, no invented pixels |
| `bilinear`, `bicubic`, `lanczos3` | standard resamplers, named honestly |
| `anime4k` | **Anime4K-style preset**, the classic v0.9 recipe: Lanczos-3 base + edge-adaptive unsharp on luma |

Anime4K v2 and later are convolutional neural networks. Shipping CNN weights in
a dependency-free core is not this project, so the preset implements the classic
recipe, is named "Anime4K-*style*" everywhere it appears, and its behaviour is
measured rather than asserted: uniform fields come out **identical** (the
sharpening kernel has unity gain — verified for all five methods at five
intensity levels), and hard edges carry the ~1% halo that sinc side-lobes
necessarily produce. Lanczos ringing is a property of the algorithm, not a bug,
and that sentence lives in the source next to the enum variant so nobody has to
rediscover it.

### Frame interpolation

`FrameInterpolator` is a trait so a motion-compensated or RIFE-based backend can
be dropped in later. The contract is **gap filling**, and the tests state it:
`n` frames at factor `f` produce `(n-1)*f + 1` frames; one frame is returned
unchanged because it contains no gap; an empty sequence stays empty; factor 1 is
the identity; factor 0 is an error. `BlendInterpolator` is the honest baseline —
it blends, it does not invent motion, and it does not claim to.

---

## Testing strategy

* **All fixtures are synthesized.** `kintsugi-bluegale::fixtures` writes
  byte-exact ZBM, BBM, INX, SNN, and BDT files from code. No commercial game
  data, artwork, or script text is in this repository, and the tests can
  therefore be run by anyone, anywhere.
* **The seam is tested where it is wired.** The patch round trip lives in
  `crates/kintsugi/tests/patch.rs`, because the host is the only layer allowed
  to know both a seam and the glaze.
* **The C ABI is tested from C.**
  `crates/kintsugi-android/tests/c_abi.c` links the built `cdylib` and calls
  every entry point, including the error paths and `free(NULL)`.
* **The JNI names are checked against `nm`.** A mismatch there is the classic
  Android `UnsatisfiedLinkError` at first launch, so CI greps the exported
  symbols for all five `Java_com_kintsugi_engine_EngineBridge_*` names.
* **Every runnable artifact is smoke-tested by running it.** The macOS, Windows,
  and Linux jobs are built from the same commit and each plays the demo game and
  fails if the output is wrong. The APK is not run — CI unpacks it and proves the
  four `libkintsugi_android.so` slices are inside; an on-device run is still a
  manual step (see [PLATFORMS](PLATFORMS.md)).

---

## Non-goals

Kintsugi will not:

* **modify an original game**, ever, for any reason;
* **guess an engine's opcodes** to make a feature look finished;
* **substitute characters** the target code page cannot hold;
* **ship a feature flag that lies** — a preset named after a neural network
  will not be a resampler wearing its name;
* **hide a failure behind a default value.** If a seam cannot do the thing, the
  user reads a sentence that names the seam and the file.

The list is short because it is enforced: each line corresponds to a mechanism
described above, not to a promise.

---

## Where to go next

* Add engine №2: [`docs/ADDING-AN-ENGINE.md`](ADDING-AN-ENGINE.md)
* Build and release for Android/macOS/Windows: [`docs/PLATFORMS.md`](PLATFORMS.md)
* Every reverse-engineered fact about BlueGale: [`docs/RESEARCH-BlueGale.md`](RESEARCH-BlueGale.md)
