# BlueGale's BSX engine — what the files actually hold

Research notes for `crates/kintsugi-bsx`. This is the second engine seam, and the
first one written against a game that shipped **after** the SNN/INX era: the
2008 BlueGale release *鬼父 ～愛娘強制発情～* (a 937 MB CD image, `ONI.ISO`).

Everything below was read out of that release and then checked against a second,
independent source before it was written down. Where the two disagreed, the
disagreement is recorded rather than hidden. `kintsugi-bsx` implements only what
is marked **implemented**; anything else is refused by name at the point of use.

## How the game is laid out

```text
ONI.ISO (ISO 9660, 2048-byte sectors)
├── def.ini                     installer configuration, CP932
├── autorun.inf
├── exe/
│   ├── oni.exe                 the engine, 2008-09-29
│   ├── bsx.ini                 the engine's own system file: "ONI@BLUEGALE"
│   ├── bsx.dat                 the compiled story, "BSScript Rev.6"
│   ├── scene.dat               plain text: "0,0,marina_replay_01\r\n" …
│   ├── cg.dat                  binary gallery table
│   ├── Graphics.bsa            88 MB, 144 entries, all .bsg
│   ├── GraphicsBU.bsa          168 MB, 516 entries, .bsg and .bmp
│   ├── GraphicsEv.bsa          192 MB, 291 entries, .bsg and .bmp
│   ├── Voice.bsa               373 MB, 3410 entries, all .ogg
│   ├── bgm.bsa                 26 MB, 13 entries, all .ogg
│   └── se.bsa                  3.7 MB, 31 entries, all .ogg
├── text/*.txt                  CP932 text, one file per character
├── movie/  cg/  pcm/
```

The disc was mounted read-only to look at it (`hdiutil attach -readonly`), and
the files are **not** in this repository: no commercial game data is committed,
here or in any fixture. Every fixture in `crates/kintsugi-bsx/src/fixtures.rs`
is built byte by byte from the layouts below.

## `BSArc` — the resource container

Prior art: GARbro's `ArcFormats/Bishop/ArcBSA.cs` (MIT), which documents the
same container for the studio **Bishop**. BlueGale and Bishop shipped the same
middleware; the magic and both index layouts match, so GARbro's reader is an
independent check on ours rather than the source of it.

```text
0x00  "BSArc"             5 bytes
0x05  00 00 00            padding
0x08  u16 version         1..=3
0x0A  u16 count           number of index records
0x0C  u32 index_offset    where the records start
      …                   entry data
index_offset:
      count × 0x28        records: 32-byte CP932 name, u32 offset, u32 size
```

Verified against all six archives of the release: 4,405 entries, every name
decodes as CP932, every extent lies inside its file, and in all six the index
sits at the very end, at exactly `file_size − count × 0x28`.

Two notes where the real file and the prior art differ, both worth keeping:

* The game's archives declare **version 2** but store the **fixed 0x28-byte**
  record layout that GARbro's reader handles as its *v1* path. GARbro tries v2
  first and falls back to v1, which is why it works. Our reader checks the
  version *and* validates the layout against the file size, so neither is
  trusted on its own.
* Names carry a hierarchy: a record named `>Voice10` descends into a directory
  and a bare `<` comes back up to the parent. **This release uses it** — 28
  marker records across the six archives (`Graphics.bsa` 4, `GraphicsBU.bsa`
  14, `Voice.bsa` 10) — and those markers are why the mounted view shows
  `voice/voice14/14100250.ogg` naming a file three levels down, in an archive
  whose records have no slashes in them at all. A reader that treated a marker
  as a file would list 4,405 files where the game has 4,377 and would put 28
  entries with no bytes and no meaning in front of a user.
* Those 28 markers are the whole of the difference between the records an
  archive declares and the files it holds, archive by archive: `Graphics.bsa`
  144 − 4 = 140, `GraphicsBU.bsa` 516 − 14 = 502, `GraphicsEv.bsa` 291 − 0 =
  291, `Voice.bsa` 3,410 − 10 = 3,400, `bgm.bsa` and `se.bsa` unchanged — 4,377
  files. Every marker is a record with offset 0 and size 0, so a reader can also
  recognise one without knowing the convention.
* Records may **share bytes**: the extents in `GraphicsBU.bsa` declare
  168,369,600 bytes and cover 167,930,792, so 438,824 bytes are named more than
  once, the same picture under several names. Anything that walks an archive to
  transform it — upscaling every image, say — has to decide what to do about
  that, and "once per name" and "once per byte range" are different answers.

**Implemented:** reading, mounting (under a prefix taken from the archive's
name, so two archives holding `b01a.bsg` cannot shadow each other), and byte
ranges — reading a picture out of a 192 MB archive reads the picture, not the
archive.

## `BSG` — the pictures

Prior art: GARbro's `ArcFormats/Bishop/ImageBSG.cs`.

```text
0x00  "BSS-Graphics\0"    16 bytes, or "BSS-Composition\0" with
0x20  "BSS-Graphics\0"    everything below relative to 0x20
+0x12 i32 unpacked size
+0x16 u16 width
+0x18 u16 height
+0x20 i16 x offset        used by composed images
+0x22 i16 y offset
+0x30 u8  colour mode     0 = BGRA, 1 = BGR, 2 = 8-bit indexed
+0x31 u8  compression     0 = stored, 1 = run length, 2 = LZ
+0x32 i32 data offset
+0x36 i32 data size
+0x3A i32 palette offset
```

An indexed image's palette is 256 entries of four bytes, blue-green-red-unused —
the same layout a BMP uses. Pixels are stored **bottom-up**; the body's `Image`
is top-down, so the seam flips rows.

Colour-mode and compression distribution over all 765 images of the release:

| colour mode | compression | count |
| --- | --- | --- |
| 0 (BGRA) | 1 (run length) | 747 |
| 1 (BGR) | 1 (run length) | 14 |
| 2 (indexed) | 1 (run length) | 4 |

Of the 765 images, **456 are `BSS-Composition`** — a second header at `+0x20`
composing a picture from parts at `±x`/`±y` offsets — and 309 are plain. The
168 BMPs are the only other image files. Together with the 3,444 Ogg files that
is 4,377 — every file these archives hold, with none unaccounted for. The mount
reports 4,408 for the whole game, and the 31 that are not in an archive are the
disc's own loose files: 12 in `exe/`, among them the story (`bsx.dat`), its
config (`bsx.ini`), a CG list (`cg.dat`, 2,504 bytes, not a `BSScript`) and a
`scene.dat` that has not been opened.

**Implemented:** colour modes 0, 1, 2 with compression 0 and 1. Every declared
`unpacked size` in the release equals `width × height × 4` (or `× 1` for indexed),
and the decoder refuses a file where it does not.

Checked rather than assumed, one representative file per kind — the smallest of
each colour mode, a composition, and one of the BMPs — carried all the way
through `upscale` on the release itself:

| colour mode | file | decoded as |
| --- | --- | --- |
| 0 | `graphics/system/msg_info_str1.bsg` | 48×126 |
| 0 | `graphics/system/dm_slot.bsg` | 192×304 |
| 0 | `graphics/system/spray_anime.bsg` | 200×1200 |
| 1 | `graphics/system/logo.bsg` | 800×600 |
| 1 | `graphics/system/scene0_thumb.bsg` | 120×1800 |
| 1 | `graphics/system/scene1_thumb.bsg` | 120×1620 |
| 2 | `graphics/system/item0_foot.bsg` | 624×64 |
| 2 | `graphics/system/message_hit.bsg` | 800×150 |
| 2 | `graphics/system/title_hit.bsg` | 800×600 |
| 0, composition | `graphics/system/log_vp.bsg` | 39×36 |
| — (BMP) | `graphicsbu/12/bu12f0031a01m.bmp` | 66×120 |

All three colour modes, both container kinds and the BMPs are in that table
because all of them decoded; every one was carried through `upscale --factor 2`
on the release itself. Compression 0 does not appear in it because **this
release has none**: all 765 images declare compression 1, so the stored path is
implemented and exercised by fixtures only, and nothing in this release would
notice if it were wrong. That is worth knowing before trusting it.

That table is also where the next gap shows: `scene0_thumb` at 120×1800 divides
evenly into ten strips of 120×180, and `spray_anime` at 200×1200 into six of
200×200 — **strips of frames**. The body's `Image` has no notion of frames: one
file is one picture. So `upscale` works on them and `interpolate` cannot, which
is a body API question rather than a format one. Recorded here because "the
release has animations" and "this seam can interpolate them" are not the same
sentence.

The run-length layout was checked against **all 765** images of the release, and
not by eye: for every one of them, all of its planes' streams ended at exactly
`data offset + data size` — no byte left over, none missing — and each stream
wrote exactly `width × height` bytes. A layout half-understood would not survive
that on a single file, let alone on 765.

### Upscaling a real picture

`cg046.bsg` — a 1.5 MB composition, 800×600 — taken to 1600×1200 twice, once
with `--method anime4k` and once with `--method bilinear`, both out of the
192 MB archive on the ISO, in about two seconds each. The mean absolute
difference between neighbouring pixels (luminance, one sample in four rows and
sixteen columns) is **4.29 for anime4k against 2.98 for bilinear**: an
edge-directed method should leave a harder edge where the art has one, and on
real art it does. That is the whole claim the preset makes, measured rather
than asserted — and it is measured on this release, not on a fixture.

### A dead end worth recording

`GraphicsBU.bsa` holds 168 eight-bit BMPs whose names are a BSG's name plus `m`
— `bu10f0061a02m.bmp` beside `bu10f0061a02.bsg`. They look like thumbnails of
the picture next to them, which would have settled the row-order question
outright, because a BMP's orientation is not in doubt.

They are not thumbnails. Every one of the 168 pairs was decoded and compared,
in all four combinations of vertical flip and horizontal mirror, at the exact
integer scale the sizes imply: the closest combination still differs by 149 of
255 per channel, which is what comparing two unrelated pictures looks like. The
`m` files are their own artwork — the gallery's portrait thumbnails, cropped
from something else — and the naming is a trap rather than a hint.

The check is written down here for two reasons: whoever implements the gallery
next should not build it on that naming, and nobody should redo this afternoon.

One assumption is still an assumption, and it is written down here so that it can
be wrong in public: **rows are stored bottom-up**. GARbro's reader flips them
(`CreateFlipped`) and the first real image decoded with this seam's rules reads
as a right-side-up title screen, but a person looking at a picture is the only
judge of that, and no person has looked yet. If a picture ever comes out upside
down, `crates/kintsugi-bsx/src/image.rs`, the call to `interleave`, is the line
to change — and this paragraph is the note that says so.

**Not implemented, deliberately:** compression 2 (LZ). It never appears in this
release, so an implementation could not be verified against a real file. The
layout, for whoever meets one — each plane is `control byte, i32 length − 5`,
then a byte `c`; `c == control` starts a match (`offset` byte, `count` byte,
`offset -= 1` when `offset > control`, `offset *= pixel size`, `count` bytes
copied from `dst − offset` stepping the pixel size), and a doubled control byte
is a literal; after the stream, every plane is delta-filtered with
`plane[i] += plane[i − pixel size]`. It is refused with that sentence's file
name in the message.

## `BSScript` — the story

`bsx.dat` is 1,062,484 bytes of `BSScript Rev.6`. The seam **reads, translates
and repairs it**; what follows is what is established, and how.

* Magic (16 bytes: `BSScript Rev.6`), then seven little-endian u32s at
  `0x10..0x2C` — `0x100`, `2`, `0x18`, `4`, `0xc`, `0xe`, `0x110`. Their meaning
  is **not** established. They are why the record list starts at `0x2C`, and
  nothing else is claimed for them.
* At `0x2C`, `(offset, size)` pairs: **thirteen** of them. The fourteenth is
  `(0x01030000, 1)`, and every pair after it points past the end of the file —
  which is what says the list has ended. The reader uses that rule (the first
  pair that is not a record inside the file) rather than a count from a header
  field it cannot explain.
* The list is **not** a partition: record 0 spans `0x4534e` + 11,864, ending at
  `0x481a6`, and records 1 (`0x45460`+1,688) and 2 (`0x45b00`+2,006) lie inside
  that span. Record 3 (`0x462e0`+15,528) starts inside it and ends after it.
* Four of the thirteen records are **string tables**: a `u32` index of
  block-relative offsets immediately followed by a block of NUL-terminated CP932
  text, one line per entry. In this file they hold 4 `@` variables, 12 `#`
  variables, 20 cast names, and the story: `(0x52ad0, 47,456)` for the index and
  `(0x5e430, 676,388)` for the block, 11,864 lines.
* The story is the **last** record *and* larger than every other string table.
  Both are required and a file that satisfies only one is refused: with a
  damaged story block, a reader that took the largest table would quietly decide
  the cast list was the story and rewrite a game's names into a translation.
  With the story unreadable, no candidate satisfies both, and the seam says so.
* The index is exact rather than a sample: for every entry `k`, the byte before
  `offsets[k]` is a NUL and no other byte of the line before it is, `offsets[0]`
  is 0, and the last line's terminator is the block's last byte. All 11,864
  lines satisfy that, and no line contains a control byte (checked: zero of
  11,864).
* **The engine refers to its lines by index, never by address.** Measured on
  this release: of 200 sampled lines (indices 2,000–11,863), all 200 are named
  by index somewhere between `0x100` and the text block — 785 references — while
  only four have any value in that region that could be read as a block offset,
  and 7 hits across 386 KB of data is fewer than chance produces. That is what
  makes a repair which changes line lengths possible at all: the index is
  rewritten in place at the same size (11,864 × 4), the block — the last thing
  in the file — grows or shrinks, and the single directory pair that measures it
  is corrected.
* A repair, byte for byte, for a mock translation of all 11,864 lines: the
  header and the first twelve directory pairs identical; the story's offset
  unchanged (`0x5e430`) and its size corrected from 676,388 to 747,572; the
  bytecode and the name tables from `0x94` to `0x52ad0` identical; 11,863 index
  entries moved and still strictly increasing; the file growing by exactly
  71,184 bytes — 11,864 × 6, the length the prefix added. Verified by re-reading
  the repaired file with a parser written independently of the seam and
  comparing every line against the original.

**The show instruction.** The classification is no longer missing. The code
turns out to be readable as instructions of the form `1a <channel> <line:u32>`:
scanning the file up to the story's index finds 11,986 of them, where the
two-byte prefix alone would appear about six times by chance. The channel byte
is 0 (6,496 times), 2 (3,381) and 1 (2,109); every other value occurs exactly
once, with line number 0, which is what a coincidence looks like. They cover
11,840 of the story's 11,864 lines.

What the channels are is not a guess either — the lines themselves say it.
Channel 0 carries `■■■　真理奈ＥＮＤ　■■■`, `真理奈は先に食べていたらしく、
ミルクティーを飲んでいた。` and `……。`: narration, no quotes. Channels 1 and 2
carry `「う、うぅっ……」`, `「やぁ、おはよう」` and `『クスクス……』`: spoken
lines, in the two text boxes the game draws. So the instruction says which box,
not who is speaking, and the seam gives a dialogue line no speaker rather than
invent one.

Order is the other half. Sorting the instructions by where they sit in the code
and looking at the line numbers, **11,684 of the 11,985 steps are exactly `+1`**:
the code walks the table in 302 runs, the longest 3,549 lines, and those runs are
the story's scenes. Between two runs the story branches or a scene ends — which
of the two is what a branch decoder would have to say, and this is where the
seam stops.

**Not implemented:** the branch instructions, so `read_script` walks the runs one
after the other and hands over every line the story can reach rather than one
playthrough; and the records that are not string tables — `0x4534e`+11,864,
`0x45460`+1,688, `0x45b00`+2,006, `0x462e0`+15,528, `0x49f90`+35,204.

The first record is **not** a table of line references. Read as 4-byte integers,
29 of its 2,966 values are below 11,864 — 1%, against a chance level of 0%. Read
as 2-byte integers it is a different picture: 4,251 of 5,932 values (71.7%) are
below 11,864 against a chance level of 18.1%, and 2,359 of them are zero. So the
story's line numbers really are in this record, four times more often than
coincidence would put them there, but as operands in compiled code — which is
what a "show line N" instruction is — rather than as a table. Its tail is a run
of alternating zero and a slowly increasing value (17,808, 17,813, 17,819,
17,828, …), the shape of a directory rather than of text.

An earlier draft of this section said those indices appeared "near its start",
at `0x453ab`, `0x453f4` and `0x45425`. They do, if you read four bytes starting
at an odd address: the values are real and the alignment was not, and a
coincidence that reads like a finding is the most expensive kind. That sentence
is gone and the measurement above is in its place. Every line is therefore handed over as `Command::RawLine` with a
warning that says so, and `translate` extracts raw lines by default
(`--only-typed` skips them) precisely for seams in this state.

## Audio

Voice, music and effects are Ogg Vorbis (`.ogg`, 3,444 files) — already a
container the body knows, so the seam hands them over untouched.

## What this release changed about the body

* `FileSource` grew a ranged read, and `Vfs` with it. Without it, mounting this
  game would have meant loading 820 MB of archives to list them.
* The engine id is `bsx`, taken from the game's own file names (`bsx.ini`,
  `bsx.dat`) rather than invented: the container is shared with Bishop, so a
  detection that finds only archives reports `Likely` and says so in its note,
  and only a `BSScript` file makes it `Certain`.

## Sources

* GARbro, MIT — `ArcFormats/Bishop/ArcBSA.cs`, `ArcFormats/Bishop/ImageBSG.cs`,
  `GameRes/Image.cs` (`ReadColorMap`, `PaletteFormat.BgrX`).
* The release itself, as listed above. Sizes, offsets and entry counts quoted
  here were produced by reading the files, not by memory.
* ISO 9660 (ECMA-119) for the disc layer, which the body reads directly.
