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

**Implemented:** colour modes 0, 1, 2 with compression 0 and 1. Every declared
`unpacked size` in the release equals `width × height × 4` (or `× 1` for indexed),
and the decoder refuses a file where it does not.

The run-length layout was checked against **all 765** images of the release, and
not by eye: for every one of them, all of its planes' streams ended at exactly
`data offset + data size` — no byte left over, none missing — and each stream
wrote exactly `width × height` bytes. A layout half-understood would not survive
that on a single file, let alone on 765.

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

`bsx.dat` begins with the ASCII magic `BSScript Rev.6`, is 1,062,484 bytes, and
about a third of it is CP932 text. The parts that are established:

* Header: magic (16 bytes), then little-endian u32s `0x100`, `2`, `0x18`, `4`,
  `0xc`, `0xe`, `0x110`.
* From `0x30` on, a table of `(offset, size)` pairs. The last few pairs point
  into the text heap that starts around `0x52990`, and the first text runs
  appear at `0x52a78`. The early pairs overlap each other, so the table is
  hierarchical — regions containing records — rather than a flat list.
* Text is CP932, and `scene.dat` beside it is plain CP932 CSV naming assets
  (`0,0,marina_replay_01`), which is how the format's asset names were confirmed
  without reading any story text.

**Not implemented:** reading the story into the body's IR. The string extents
are visible, but which strings are dialogue, which are menu labels and which are
file names is decided by the compiled code section, and a translation that
guessed would put words into a game without knowing what they replace. So the
seam does **not** implement `primary_script` either: naming `bsx.dat` as the
game's script while being unable to read it would make `translate` and `install`
aim at a file they cannot follow through on. `kintsugi inspect` lists the file;
`kintsugi translate` says the seam cannot read it. When the reader lands, the
script fixture and the `.script(...)` line in
`crates/kintsugi-bsx/tests/conformance.rs` land with it.

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
