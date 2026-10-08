# 🏺 Kintsugi Engine · 金缮引擎

**Repairing old games with gold.**
**以金缮之艺，续老游戏之命。**

---

## The name

金継ぎ (*kintsugi*) is the Japanese craft of repairing broken pottery with
lacquer and gold. The repaired vessel is not hidden or made to look new: the
seam is **filled with gold and left visible**, because the break is part of the
object's history, not a defect to be erased.

Old games break the same way. The studio is gone. The source code is gone. The
platform they ran on is gone. What is left is a folder of files nobody can read
any more.

Kintsugi repairs them with modern code, and **keeps the repair visible**:

* every heuristic, workaround, and guess is printed to the user, not swallowed;
* a format that is only half-understood is exposed as half-understood;
* nothing in the original game folder is ever modified — the folder is
  read-only to this tool and every artifact is written outside it;
* when the engine cannot do something, it says so and names the file, instead
  of producing something that looks like it worked.

> 器は、砕ける前よりも美しくなった。
> *The vessel became more beautiful than before it was broken.*

---

## What it does today

| Capability | State |
| --- | --- |
| **Body** — engine-agnostic asset/script IR, VFS, runtime, detection registry, zero dependencies | working |
| **Seam №1 — BlueGale (ブルーゲイル)**: SNN+INX archives, ZBM/BBM bitmaps, BDT scripts | working |
| **Glaze — HD upscaling**: `nearest`, `bilinear`, `bicubic`, `lanczos3`, `anime4k` (Anime4K-style preset) | working |
| **Glaze — frame interpolation (插帧)**: `interpolate` command, gap-filling contract, size-aware sequencing, blend backend; motion-compensated backends plug into the same trait | working |
| **Glaze — script translation**: JSONL interchange + LLM backend (OpenAI-compatible), glossary, offline `--mock` | working |
| **Translation write-back**: a repaired script written outside the game folder, byte-preserving and CP932-strict | working |
| **Install**: put a repaired script into a copy of the game, refusing a patch whose lines do not line up, reading the copy back to prove it landed, recording what it wrote in `.kintsugi-install` so a re-install needs no manual `rm -rf`, and removing a copy it made if the install does not finish — an install either finishes or leaves nothing | working |
| **Shells — Windows / macOS / Linux CLI** | working |
| **Shell — Android APK** (Kotlin + JNI over the same Rust engine) | built in CI from the same commit; the APK is unpacked to prove all four ABIs are inside — running it on a device is not automated yet ([PLATFORMS](docs/PLATFORMS.md)) |
| **Seam contract**: `kintsugi-testkit` — a new engine runs the same checks BlueGale does (a name is never evidence, `Certain` means mountable, changing nothing changes nothing) | working, and tested against eight deliberately broken seams |
| **Seam №2…N** — other engines | the reason the body exists; [adding one](docs/ADDING-AN-ENGINE.md) is a crate and two registration lines |
| BlueGale **AMV** video | *not implemented* — an honest hole, see [research notes](docs/RESEARCH-BlueGale.md) |

---

## Quick start

```sh
git clone git@github.com:daolao1/Kintsugi.git
cd Kintsugi

cargo test                                     # 137 tests, all fixtures synthesized
cargo run -p kintsugi -- demo                  # write a tiny game, detect it, play it, glaze it
cargo run -p kintsugi -- detect  ./demo-game
cargo run -p kintsugi -- inspect ./demo-game
cargo run -p kintsugi -- play    ./demo-game
cargo run -p kintsugi -- upscale ./demo-game --asset title.bbm --method anime4k --factor 4 -o title-x4.png
cargo run -p kintsugi -- interpolate ./demo-game --factor 4 -o frames/
cargo run -p kintsugi -- translate ./demo-game --mock --write-script story.en.bdt
```

The `demo` command synthesizes a BlueGale-shaped game in `demo-game/`
(byte-exact formats, zero copyrighted bytes), so the whole pipeline can be run
the moment the repository is cloned. Here is what it prints:

```
🏺 Kintsugi — 金缮引擎 · demo

wrote synthetic demo game into demo-game:
  · demo-game/game.inx
  · demo-game/game.snn
  · demo-game/story.bdt

detecting engine…
●●● bluegale — 1 SNN archive(s) with valid INX indexes (8 entries)

playing story.bdt
  ————————————————————————
深夜の工房。陶器の砕ける音が、静寂を裂いた。
祖母の形見の茶碗が、五つの欠片に散らばっている。
「直せるものなら、直したい……」
だが彼女は、傷を隠すことをよしとしなかった。
金を練り、漆で欠片を合わせていく。
亀裂を消すのではなく、金の川として描き直す。
それは修復ではなく、金継ぎ――金繕いだった。
器は、砕ける前よりも美しくなった。
欠けた時間ごと、黄金で結ばれて。
  ————————————————————————

glazing (upscaling) the title screen…
  · 4×3 → 16×12 demo-game/title-x4-anime4k.png

修复完成 — the pot plays again, seams on display.
```

`●●●` is the detection confidence (certain / likely / possible / unlikely), and
the sentence after it is the *evidence*, not a label.

### Talking to your own game

```sh
cargo run --release -p kintsugi -- inspect "/path/to/game"      # what is this?
cargo run --release -p kintsugi -- play    "/path/to/game"      # read its script in the terminal
cargo run --release -p kintsugi -- upscale "/path/to/game"      # list image assets
```

Originals are never written to. The only files Kintsugi creates are the ones
you ask for (`-o out.png`, `--jsonl-dir dir`, `install --into copy`, the demo
game), and the game folder is read-only to every command — checked in the code
(`ensure_outside_game`), not promised in prose.

### Translating a script

```sh
export KINTSUGI_API_KEY=sk-...          # or OPENAI_API_KEY
cargo run --release -p kintsugi -- translate "/path/to/game" \
    --script story.bdt --source ja --target en --model gpt-4o-mini \
    --glossary glossary.txt --jsonl-dir out/ \
    --write-script out/story.en.bdt
```

* the script is first extracted to a **JSONL interchange file**, which you can
  read, hand-edit, or translate with any other tool;
* `--mock` runs the whole pipeline offline, which is how the pipeline is
  tested without a network or a bill;
* the translated script is applied to a **copy** in memory, and `--write-script`
  writes a patch **outside** the game folder. Writing inside it is refused
  outright (exit `2`) — not only over the file being read, but over any other
  original in there (`game.snn`, say) and for the glaze's output too. The
  promise is a check in the code (`ensure_outside_game`), not a sentence in
  this file. What it cannot see is a hard link to an original made *outside*
  the folder: that needs file identity, which portable std does not expose, and
  the hole is written down rather than half-patched;
* the patch is byte-preserving **literally**: only the lines that were actually
  translated are re-encoded, and every other byte is copied straight from the
  original, so labels, indentation, line endings, blank lines and trailing
  bytes survive exactly. This is not decoration — CP932 has hundreds of
  characters with more than one valid spelling (the NEC and IBM duplicate rows
  — `87 90` and `81 E0` are both `≒`), so re-encoding the whole file would
  quietly re-spell lines nobody asked to touch. Lines whose id has no home in
  the original file are reported, not dropped;
* a translation the target code page cannot hold is **refused by name** (see
  the CP932 note below) rather than silently mangled.

Installing the patch is a command, not a suggestion to copy files by hand:

```sh
cargo run --release -p kintsugi -- install "/path/to/game" \
    --script out/story.en.bdt --into /tmp/repaired
```

It writes the repair into a **copy**, never into the game it read, and it does
three things a hand-copy does not:

* it **refuses a patch that does not line up**. The patch is parsed by the same
  seam that installs it, line by line; if the patch has prose where the game has
  a label, or a label where the game has prose, the install stops before the
  copy is even made. Ids drift when a patch and a game come from different
  versions, and a repair that lands on the wrong line is worse than no repair;
* it **rebuilds rather than overwrites**: the bytes come from the seam's
  writer applied to the copy's own original, so untouched lines keep their exact
  original bytes — including the CP932 spellings just described;
* it **reads the result back**. The copy is mounted again, the script parsed
  again, and the number of changed lines compared with the number of
  replacements applied; if they disagree the install fails and says so. It also
  re-checks that the game folder and the original script are byte-for-byte what
  they were, because a tool that only claims this is a tool you have to trust.

```sh
$ cargo run -p kintsugi -- install demo-game --script story.en.bdt --into repaired
copied 5 file(s), 2799 byte(s), to repaired
the copy mounts as bluegale
  [mount] mounted 'game.snn': 8 entries
installed story.bdt into the copy: 9 of 14 line(s) differ from the original
the original game folder is untouched (5 file(s), 'story.bdt' byte-identical)
→ play the repaired copy: kintsugi play repaired --auto
wrote .kintsugi-install: what this copy is, and every file kintsugi put in it
```

`9 of 14` because a script is not all prose: the four `$`/`%` labels are
structure, counted in the total and never rewritten. The copy's `story.bdt` is
byte-for-byte the patch `translate` wrote — a property the tests assert, since
it means the two commands agree about what a repair is.

That last line is the other half of the honesty: the copy carries a manifest of
what was done to it, so "what did the repair tool touch?" is a question you
answer by reading a file in the copy, not by trusting this README.

```
$ cat repaired/.kintsugi-install
kintsugi-install	1
tool	0.1.1
engine	bluegale
game	/tmp/kt-doc2/demo-game
script	story.bdt
patch	468	c8195db99f0670db	/tmp/kt-doc2/story.en.bdt
file	139	1825695238904e2f	.kintsugi-demo
file	580	057a2ac45b0b3fa8	game.inx
file	818	3b66838dae77b258	game.snn
file	468	c8195db99f0670db	story.bdt
file	848	a38519df61d68354	title-x4-anime4k.png
```

And it is what makes translating a game a loop rather than a one-shot: fix a
glossary entry, re-translate, and install again over your own previous copy —
no `rm -rf` by hand, which is where a real game folder gets deleted by mistake.
The manifest is checked *against* the folder rather than believed: same files,
same sizes, same checksums, or the install refuses.

```sh
$ cargo run -p kintsugi -- install demo-game --script story.en2.bdt --into repaired
replacing this folder's own previous install (5 file(s), every one unchanged since kintsugi wrote it, patch story.en.bdt)
copied 5 file(s), 2799 byte(s), to repaired
...
```

A copy that has been *used* is refused by name — `save01.dat`, or a script that
was hand-edited after kintsugi wrote it — because a tool that overwrites a folder
it did not just write cannot tell a stale copy from someone's installation.

The offline path is the tested one, end to end:

```sh
$ cargo run -p kintsugi -- translate demo-game --mock --no-play --write-script story.en.bdt
extracted 9 line(s), including unclassified raw lines (ASCII command-like lines are passed through automatically)
translating ja → en via mock
  total: 9/9 line(s) changed
  [warn] translated 9 line(s) via kintsugi-translate (0 skipped); source: story.bdt
repaired script: story.en.bdt (468 byte(s), 9/9 changed line(s))
```

### Glazing pixels

```sh
cargo run --release -p kintsugi -- upscale ./demo-game --asset title.bbm --method anime4k --factor 4
```

| method | what it is |
| --- | --- |
| `nearest` | block replication — the honest baseline, no invented pixels |
| `bilinear` | 2×2 weighted average |
| `bicubic` | Catmull-Rom |
| `lanczos3` | windowed-sinc, the best general-purpose resampler here |
| `anime4k` | **Anime4K-style preset**: classic v0.9 recipe — Lanczos-3 base plus edge-adaptive sharpening on luma only |

**Honesty note on `anime4k`.** Anime4K v2 and later are convolutional neural
networks; shipping CNN weights inside a dependency-free core is not what this
project is. The `anime4k` method implements the *classic* Anime4K recipe and is
labelled Anime4K-*style* everywhere it appears. Its behaviour is measured, not
claimed: uniform fields are reproduced **exactly** (the filter has unity gain),
and hard black/white edges acquire the ~1% halo that sinc side-lobes
necessarily produce — Lanczos ringing is a property of the algorithm, not a
bug, and it is written down in the source next to the enum variant.

### Frame interpolation (插帧)

```sh
cargo run --release -p kintsugi -- interpolate "/path/to/game" --factor 3 -o out-frames/
```

```
$ cargo run -p kintsugi -- interpolate demo-game --factor 4 -o frames
interpolated: 4 frame(s) → 13 frame(s) at ×4 (blend)
  sequence: cut01.zbm, cut02.zbm, cut03.zbm, cut04.zbm (5x3)
  → frames
· not a frame: face.zbm (3x3, not 5x3 like the other frames)
· not a frame: room.zbm (4x4, not 5x3 like the other frames)
· not a frame: title.bbm (4x3, not 5x3 like the other frames)
  originals are frames 0, ×, 2×, …; only the gaps between them were filled
```

The contract is **gap filling**: `n` frames at factor `f` become
`(n-1)*f + 1` — every gap gets `f-1` evenly spaced in-betweens and the
originals stay in place, in order (verified byte-for-byte: the file written for
frame 0 is identical to the original's PNG). Nothing is extrapolated past the
last frame, because the host holds it for its duration and inventing a frame
that was never on the disc would be a fake repair.

Three consequences worth knowing:

* **Images of different sizes are never blended.** A game directory holds a
  title logo, a background, and a portrait as well as a cutscene; only
  same-sized images can be frames of one sequence, so the longest same-sized
  run wins and every image left out is **named with its size** rather than
  silently dropped. (That rule lives in `kintsugi_video::group_by_size`, where
  it is tested, not in the CLI.)
* **A frame that cannot be read is named, not faked.**
* **BlueGale's `.amv` is not demuxed** (see the research notes), so this
  operates on image sequences the seam can hand over — which is exactly what an
  extracted cutscene or a slideshow-style scene is.

`BlendInterpolator` is the honest baseline: linear crossfade, *exactly* right on
fades and dissolves, and on fast motion it shows the same double-image the
original hardware would. Motion-compensated or RIFE-style backends implement
the same `FrameInterpolator` trait; no glaze code changes when one arrives.

---

## Architecture in one picture

```
                    ┌──────────────────────────────────────────────┐
   shells           │  kintsugi (CLI)      platforms/android (APK) │   per-platform,
   (delivery)       │  Windows · macOS · Linux      Android · JNI  │   replaceable
                    └───────────────┬──────────────────────────────┘
                                    │  Host trait: show_text / event / choose
                    ┌───────────────▼──────────────────────────────┐
   host             │  kintsugi — registry, CLI, TerminalHost      │   wires everything
                    └───────┬───────────────────────────┬──────────┘
                            │                           │
                    ┌───────▼────────┐        ┌─────────▼──────────┐
   glaze            │ kintsugi-video │        │ kintsugi-translate │   cross-engine
   (tooling)        │ upscale · interp│       │ JSONL · LLM backend│   capability
                    └───────┬────────┘        └─────────┬──────────┘
                            │   engine-agnostic IR: Image, Script
                    ┌───────▼───────────────────────────▼──────────┐
   seams            │  kintsugi-bluegale  ·  …your engine here…   │   one crate per
   (engines)        │  SNN · INX · ZBM · BBM · BDT                 │   engine
                    └───────────────┬──────────────────────────────┘
                                    │  EnginePlugin / EngineMount
                    ┌───────────────▼──────────────────────────────┐
   body             │  kintsugi-core — zero dependencies           │   never learns
   (engine)         │  bytes · vfs · detect · asset · script ·      │   an engine name
                    │  codec (bmp/png) · runtime · registry         │
                    └──────────────────────────────────────────────┘
```

Four rules hold this together:

1. **The body has zero dependencies.** `kintsugi-core` cannot pull a crate, so
   it cannot accidentally depend on an engine, a platform, or a codec fad.
2. **A seam may pull dependencies.** CP932 decoding needs a table; a future
   engine may need zlib or an audio codec. That cost is paid by the engine that
   needs it, not by every user of the body.
3. **Glaze is host-side, not core.** Upscaling and translation operate on the
   engine-agnostic IR, so they serve every engine — including ones that do not
   exist yet.
4. **A shell contains no engine logic.** The Android activity and the CLI both
   call the same functions; the difference between platforms is a `Host`
   implementation, nothing more.

Full rationale, including why the plugin registry is static rather than
`dlopen`-based: [ARCHITECTURE.md](ARCHITECTURE.md).

---

## One engine, three deliverables

Android, macOS, and Windows are built from the same commit and **smoke-tested
by running the artifact** in CI: each build plays the demo game and the job
fails if the output is wrong. See
[`.github/workflows/release.yml`](.github/workflows/release.yml).

| platform | artifact | how the engine gets there |
| --- | --- | --- |
| macOS | `kintsugi-macos-universal.tar.gz` (arm64 + x86_64, `lipo`-joined) | native build |
| Windows | `kintsugi-windows-x86_64.zip` | native build on `windows-latest` |
| Linux | `kintsugi-linux-x86_64.tar.gz` | native build |
| Android | `app-debug.apk` (arm64-v8a, armeabi-v7a, x86_64, x86) | `cargo ndk` → `jniLibs` → Gradle |

Android details, including the folder-picker limitation and how to load a real
game today: [platforms/android/README.md](platforms/android/README.md) and
[docs/PLATFORMS.md](docs/PLATFORMS.md).

---

## The CP932 note (why "it worked" is not enough)

Old Japanese games store text in CP932 (Windows-31J). Encoding text *back* into
CP932 is where naive tools silently corrupt games: Rust's `encoding_rs` encoder
replaces characters the code page cannot hold with HTML numeric character
references, so an em dash (U+2014) becomes the eight literal ASCII characters
`&#8212;`, and the game happily draws that on screen.

This was not a hypothetical. The demo script originally contained an em dash
and a simplified-Chinese character in a Japanese line, and the CLI printed
`金継ぎ&#8212;&#8212;金&#32558;` — a successful-looking repair that had actually
written garbage. The engine now refuses, and you can watch it do so by making
the offline translator emit one of those characters:

```sh
$ KINTSUGI_MOCK_MARKER='—' cargo run -p kintsugi -- translate demo-game \
      --mock --no-play --write-script refusal.bdt
extracted 9 line(s), including unclassified raw lines (ASCII command-like lines are passed through automatically)
translating ja → en via mock
  total: 9/9 line(s) changed
  [warn] translated 9 line(s) via kintsugi-translate (0 skipped); source: story.bdt
🏺 kintsugi: cannot write text as CP932: character(s) — are not in that code page, and substituting them would change the game's words
$ echo $?
1
$ ls refusal.bdt
ls: refusal.bdt: No such file or directory     # nothing was written
```

The failure names the exact characters, so the fix (reword the line, or add a
font hack) is a decision a human makes with the evidence in hand. It is covered
by `refuses_text_the_code_page_cannot_hold` in the seam and by
`an_unwritable_translation_is_refused_by_name…` in the host's patch tests.
This is the whole philosophy in one bug: **a repair that cannot be done honestly
should not be done quietly.**

---

## Documentation

| document | what is in it |
| --- | --- |
| [ARCHITECTURE.md](ARCHITECTURE.md) | the four layers, the zero-dependency rule, the plugin contract, the honesty rules |
| [docs/ADDING-AN-ENGINE.md](docs/ADDING-AN-ENGINE.md) | how to add engine №2: the trait, the detection ladder, the test fixtures |
| [docs/PLATFORMS.md](docs/PLATFORMS.md) | how Android, macOS, and Windows are built, tested, and released together |
| [docs/RESEARCH-BlueGale.md](docs/RESEARCH-BlueGale.md) | every reverse-engineered fact, its source, and what is still unknown |

## Credits and prior art

Reverse engineering stands on other people's work, and the seams should show:

* **[GARbro](https://github.com/morkt/GARbro)** (MIT) — the reference
  implementations of BlueGale's archive and image formats, under
  `ArcFormats/BlueGale/` (`ArcSNN.cs`, `ImageZBM.cs`, `ImageBBM.cs`,
  `VideoAMV.cs`), with the shared MSB-first bit reader at
  `ArcFormats/BitStream.cs`. Kintsugi's BlueGale seam was written by reading
  those files; the research notes cite them per format. GARbro has **no**
  BlueGale script format — the `.bdt` reader (and the `ArcBDT.cs` in its
  `ArcFormats/FC01/` tree) belongs to a different engine.
* **SExtractor** — its Python BDT extractor was consulted for the script
  container layout and for `indexwww.dat`. This is the only evidence for the
  BDT format, so everything Kintsugi knows about script lines beyond their
  container is marked unverified in the research notes.
* **[Anime4K](https://github.com/bloc97/Anime4K)** by bloc97 — the shader
  family whose classic recipe the `anime4k` preset follows. The name is used
  with that credit and the "-style" qualifier everywhere, because v2+ is a CNN
  and this is not one.
* **[VNDB](https://vndb.org/p182)** — used to establish which titles BlueGale
  actually shipped, so the seam's scope is a fact rather than a guess.

## License

MIT — see [LICENSE](LICENSE). Test fixtures are synthesized byte by byte; no
game data, artwork, or script text from any commercial title is included.
