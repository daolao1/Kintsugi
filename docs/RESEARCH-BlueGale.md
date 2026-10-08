# RESEARCH — BlueGale (ブルーゲイル)

Notes for Kintsugi's first engine seam. BlueGale shipped visual novels from the
late 1990s into the 2010s (the VNDB listing at
`/tmp/kintsugi-research/vndb-bluegale.html` opens with Gekkoujuu, 1997, and runs
through the 『鬼父』 titles). Nothing came from a specification: each fact was
read from a community reference implementation and checked against
`crates/kintsugi-bluegale/`. A claim is **verified** only when both agree;
anything else is marked **unconfirmed** or **not yet verified** with a way to
settle it.

## Sources

| Source | Consulted copy | Used for |
| --- | --- | --- |
| [GARbro](https://github.com/morkt/GARbro) (MIT): `ArcFormats/BlueGale/{ArcSNN,ImageZBM,ImageBBM,VideoAMV}.cs` | `/tmp/kintsugi-research/{ArcSNN,ImageZBM,ImageBBM,VideoAMV}.cs` | SNN+INX, ZBM, BBM, AMV |
| GARbro, `ArcFormats/BitStream.cs` | `/tmp/kintsugi-research/BitStream.cs` | the MSB-first bit reader |
| GARbro repository listing | `/tmp/kintsugi-research/garbro-tree.json` | which upstream file lives where |
| SExtractor Python extractor | `/tmp/kintsugi-research/extract_BlueGale_bdt.py` (`src/` upstream) | BDT container, `indexwww.dat` |

The listing matters: GARbro's BlueGale module has exactly those four files and
**no** script format.

## 0. Corrections to earlier assumptions

**`ArcBDT.cs` and `BdtTables.cs` are not BlueGale.** `ArcBDT.cs` is namespace
`GameRes.Formats.FC01`, calls itself "Fairytale resource archive", carries
signature `'PACK'` (`0x4B434150`), reads its index at offset 12, opens
`dt0NN.bdt` archives and names Fairytale's 『Tsuki Jong』 (2002-12-28);
`BdtTables.cs` holds that format's AGSI key tables. Both live in
`ArcFormats/FC01/`, the shared `.bdt` extension is a coincidence, and **no
BlueGale label or opcode table has been found** anywhere (§4). Also wrong or
imprecise: `BitStream.cs` is shared infrastructure, whose `MsbBitStream.GetBits`
fixes ZBM's bit order as MSB-first (§2); "one flag bit per group" (ZBM has one
discarded leading bit, then self-describing 8-bit tokens whose own MSB is the
flag); and `README.md`'s credits, which list BDT under `ArcFormats/BlueGale/`
(being corrected separately).

## 1. SNN + INX — the resource archive pair

A `.snn` data blob with a sibling `.inx` index, little-endian throughout:

| Offset | Size | Field |
| --- | --- | --- |
| `0x00` | 4 | `u32` entry count |
| `0x04 + i*0x48` | `0x40` | file name, CP932, NUL-padded |
| `0x44 + i*0x48` | 4 | `u32` offset into the `.snn` |
| `0x48 + i*0x48` | 4 | `u32` size in bytes |

Implementation: `crates/kintsugi-bluegale/src/snn.rs` (`parse_inx`,
`SnnArchive::open`, `SnnSource`), mounted by
`crates/kintsugi-bluegale/src/plugin.rs`. Reference: `ArcSNN.cs`.

Validation, where both agree: the count must be sane (ours `1 ..= 1 << 20`,
`MAX_ENTRIES`; GARbro calls `IsSaneCount` and rejects `4 + count*0x48` past the
INX end, and since that helper is not in the consulted copy, equality of the
bound is **not yet verified**); a truncated index fails (`Reader::take`); and
every record must satisfy `offset + size <= snn_len` — a `u64` addition in
`SnnArchive::open`, `entry.CheckPlacement(file.MaxOffset)` in GARbro. **All**
records must validate: one bad record rejects the whole archive in both
implementations (GARbro returns `null`), and a rejected archive contributes
nothing. One archive that validates end to end is enough for `certain`, whether
it holds one entry or ten thousand.

Differences: GARbro discovers the pair from the `.snn` and looks for `Inx`, we
enumerate `.inx` and look for the sibling `.snn`; we also reject an empty name
after NUL-trimming (GARbro does not check) and decode the `0x40`-byte name as
CP932, whereas the encoding of GARbro's two-argument `ReadString` overload is
invisible in `ArcSNN.cs`, so **the reference's name encoding is unconfirmed**
(§8.6). `VirtualPath::new` lower-cases paths, so the mounted view shows
`title.zbm` for `TITLE.ZBM`; every valid pair is mounted, each pushed to the
front of the `Vfs` in sorted order, so with several pairs the alphabetically
last shadows the rest (**not yet verified**). The `.snn` has no header: raw
entry data that only the INX gives structure to.

## 2. ZBM — compressed bitmaps (`amp_`)

`crates/kintsugi-bluegale/src/zbm.rs`; reference `ImageZBM.cs`.

| Offset | Size | Field |
| --- | --- | --- |
| `0x00` | 4 | magic `amp_` (`0x5F706D61`) |
| `0x04` | 2 | `i16` version — must be `1` |
| `0x06` | 4 | `u32` unpacked payload size |
| `0x0A` | 4 | `u32` offset of the packed payload |

Both require `unpacked_size >= 0x36` (= 54 = 14-byte BMP file header + 40-byte
`BITMAPINFOHEADER`, the smallest header-only payload) and `data_offset >= 14`;
we also require `data_offset <= len` and cap the payload at 256 MiB
(`MAX_UNPACKED_SIZE`) — a refusal of an attacker-controlled size that no real
file is known to approach.

**The LZ stream** (`ZbmFormat.Unpack`; ours `zbm::lz_unpack`). Bits are
MSB-first: `MsbBitStream.GetBits` shifts cached bits left and returns the top
`count`, and `crates/kintsugi-core/src/bytes.rs`'s `MsbBitReader` agrees
(`(b >> (7 - bit)) & 1`). Exactly **one** bit is read and discarded before the
loop — the only flag in the format; tokens carry their own. Then, per 8-bit
token `t`: `t == 0` ends the stream; `t <= 0x7F` copies the next `t` literal
bytes (8 bits each); `t > 0x7F` is a back-reference — 10 more bits of distance,
and `t & 0x7F` bytes copied from `written - offset` **byte by byte**, so a copy
may overlap and read what it just wrote (`Binary.CopyOverlapped` in GARbro).
Both clip the copy to the remaining space and stop when the bits run out.

We are stricter, and fail loudly, where GARbro is silent: `offset == 0` or an
offset past the bytes written, and a stream ending before the declared size, are
both `Error::Corrupt` — whether any real file relies on the reference's
tolerance (a short buffer) is **unconfirmed**.

**Obfuscation.** After unpacking, if the first bytes are `0xBD 0xB2` (`'B' ^
0xFF`, `'M' ^ 0xFF`) the first `min(100, len)` bytes are XOR-ed with `0xFF`
(`zbm::deobfuscate`; `ZbmFormat.Decrypt`). The payload was obfuscated *before*
packing, so un-XORing happens *after* unpacking; `fixtures::make_zbm` mirrors
that. The result is a plain BMP, which we hand to
`crates/kintsugi-core/src/codec/bmp.rs::decode_bmp`; GARbro instead reads
width/height/bpp from BMP-absolute offsets `0x12`/`0x16`/`0x1C`. `is_zbm` checks
only length ≥ 14 and the magic, so a loose ZBM is weak evidence (§6); version,
size, and offset are enforced by `decode_zbm`.

## 3. BBM — obfuscated bitmap

A complete BMP whose first `min(100, len)` bytes are XOR-ed with `0xFF`
(`crates/kintsugi-bluegale/src/bbm.rs`; `ImageBBM.cs`). Detection runs three
probes on the **still-obfuscated** bytes:

| Probe | What the plaintext means |
| --- | --- |
| `data[0..2] == [0xBD, 0xB2]` | `'B' ^ 0xFF`, `'M' ^ 0xFF`: an XORed `BM` signature |
| `u32le(6) == 0xFFFFFFFF` | bytes 6..10 are `bfReserved1` + `bfReserved2`, zero in any normal BMP |
| `u32le(0x0E) ^ 0xFFFFFFFF == 0x28` | offset `0x0E` is `biSize`; `0x28` = 40 = `BITMAPINFOHEADER` |

The third probe is what makes the test honest: the first two could coincide by
accident, but requiring the *de-obfuscated* header size to be exactly 40 both
identifies a Windows 3.x-style BMP and proves the XOR was the right transform.
GARbro checks the same three facts (LE `0xB2BD` at 0, `0xFFFFFFFF` at 6, then a
`0x20`-byte de-XOR and `ToInt32(0xE) == 0x28`); we additionally require
`len >= 0x20`. Decoding re-XORs the first 100 bytes and rejoins them
(`PrefixStream` in GARbro). Our BMP decoder is narrower than GARbro's WPF one —
it refuses `BITMAPCOREHEADER` and RLE rather than guessing (§8.9).

## 4. BDT — the script container

Evidence: the SExtractor Python extractor (consulted copy at
`/tmp/kintsugi-research/extract_BlueGale_bdt.py`) and
`crates/kintsugi-bluegale/src/bdt.rs`. **Not** `ArcBDT.cs` (§0).

Verified: the whole file is XOR-ed with `0xFF` (the extractor's `decrypt`, our
`decode_bdt_text`), so the transform is its own inverse. The plaintext is CP932
text, line-oriented, with CRLF endings — the extractor splits on a per-engine
`ExVar.contentSeparate` whose value lives outside the consulted copy, so
"always CRLF" is **not yet verified**; our loader splits on LF and strips one
trailing CR, accepting both. Label lines match `^\t*[$%](.*)$`: leading tabs,
then `$` or `%`, then the name. The extractor indexes group 1, so both sigils
feed the engine's label index; our parser strips tabs, accepts either sigil, and
stores `Command::Label(name)`, dropping the sigil itself.

**Ordinary (non-label) lines are not understood.** We do not know which are
dialogue, which are engine commands, or what the arguments mean. The extractor
does not answer this either: its `parseImp`/`replaceOnceImp` delegate to
SExtractor's generic BIN machinery, whose control-code rules live in
`extract_BIN.py`, not in the consulted copy. So every non-label line becomes
`Command::RawLine` — kept, shown verbatim in play mode (`runtime.rs` routes it
to `host.show_text`; the CLI marks it `raw`), ready to be retyped once someone
reverse-engineers the opcodes. Guessing a dialogue/command split would be a fake
repair; an unclassified raw line is a visible crack.

**Writing back.** `bdt::rewrite_bdt` (`bdt.rs`), exposed as
`EngineMount::write_script` (`plugin.rs`), walks the original with the *same*
command indexing as `parse_bdt` and replaces only the text of the named lines.
It never goes through `Command::Label`: sigils, `\t` indentation, CRLF or LF
endings, blank lines, a missing trailing newline, and unknown bytes survive
verbatim, and labels are never rewritten precisely because the sigil is not in
the IR. A translation containing a line break, or one that would start with
`$`/`%` after tabs, is refused as a structural change rather than a wording
change; ids with no home in the original come back as `unmatched`, not dropped.
Nine writer tests in `bdt.rs` (13 there in total) cover this, plus
`crates/kintsugi/tests/patch.rs` at the host level. Unverified: no real `.bdt`
has been round-tripped, so "byte-identical apart from the translated text" is
proven only against our own fixtures (§8.3).

**`indexwww.dat`.** The extractor can export the engine's label index: exactly
`0xFA0` (4000) records of 16 bytes, `struct.pack('<8sII', name, offset,
length)`, unused slots being 16 zero bytes. `name` is truncated to 8 bytes;
`offset` is the position of the name minus one, pointing at the sigil byte;
`length` is the distance to the next label's offset, 0 for the last. So the
script *file* can be written back byte-preservingly, but the index is still
never emitted: a patch that changes a label's text or position may leave a stale
`indexwww.dat`, and whether the engine reads it or regenerates it is
**unconfirmed** (§8.4). `BdtTables.cs` belongs to FC01 (§0), not BlueGale.

Two faults in our own code: `looks_like_bdt` counts only a `$`/`%` at column 0
while `parse_bdt` accepts tab-indented labels as the extractor's regex does, so
an all-indented script would fail detection; and `parse_bdt` never calls
`looks_like_bdt`, so a non-script `.bdt` (an FC01 `PACK` archive, say) would
decode into garbage lines instead of being refused — though simply adding the
call would trade that risk for false refusals, since the heuristic also demands
60% CP932-printable bytes (§8.10).

## 5. AMV — deliberately not implemented

`VideoAMV.cs` describes the container: magic `ampV` (`0x56706D61`), `i16`
version 1 at offset 4, unpacked frame size at `0x16`, width/height at
`0x1A`/`0x1E`, a frame count at `0x2A`, and from `0x32` a list of
`(u32 size, packed frame)` pairs. Each frame uses the *same* ZBM LZ routine,
into `unpacked_size + 0x36` bytes at output offset `0x0E`, and GARbro
synthesizes a BMP header around it (`BM`, total size at 2, `header_size + 0x0E`
at `0x0A` — the pixel-data offset, because the frame begins 14 bytes in).

Kintsugi implements none of it: `plugin.rs` lists `amv` in the extension
metadata but no code path reads one, and detection never returns a verdict for
an `.amv` file — the hole is visible in `README.md` and `lib.rs`. Why not now:
GARbro's support is *frame extraction* (one BMP per frame), not playback, and
the consulted file says nothing about timing, delta coding, audio, or how the
engine drives frames, so "AMV support" would be a demuxer we cannot check
against anything. Video demux/decode is a large, separate task, and a seam that
fakes it would be worse than one that names the file and refuses; reusing the
ZBM LZ is the one cheap part, and the rest needs real footage.

## 6. Detection honesty and the confidence ladder

`crates/kintsugi-core/src/detect.rs` orders `unlikely < possible < likely <
certain`; the CLI renders it `·`, `●`, `●●`, `●●●` (`print_verdicts` in
`crates/kintsugi/src/main.rs`), and `Registry::mount_best` refuses to mount
anything below `possible`.

**An extension is never evidence.** `find_by_extension` only selects
candidates; the verdict comes from parsing bytes. A folder of `.bdt`/`.zbm` junk
yields no verdict (`plugin_detects_rejects_empty_dir`,
`bdt::tests::detection_heuristic`), and `read_image` refuses unknown bytes *by
naming all three expected signatures*.

| Verdict | BlueGale signature required (`plugin.rs`) |
| --- | --- |
| `certain` | ≥1 `.inx` whose records all parse **and** every `offset + size` fits the sibling `.snn` (`SnnArchive::open` succeeds) |
| `likely` | ≥1 `.bdt` passing `looks_like_bdt`: ≥60% of de-XORed bytes CP932-printable and ≥1 `$`/`%` label |
| `possible` | ≥1 loose `.zbm`/`.bbm` with a valid signature (`amp_`, or BBM's three probes) |

INX/SNN earns `certain` because structure must hold in several independent
places (count, `0x48` stride, non-empty CP932 names, in-bounds placement in a
second file); it is not magic-number-based, so the residual false-positive rate
on random data is small but **not measured against a corpus**. BDT stays at
`likely` because that check is a threshold plus a label count and the line
semantics are unknown. The image verdict is `possible` because a signature
identifies a file, not an engine: the same reference tree carries an unrelated
`ArcFormats/Crowd/ImageZBM.cs`. `mount` joins each `.inx` with its sibling
`.snn`, notes skipped pairs and keeps the original tree underneath, failing with
`Error::Unsupported` when the folder holds nothing; `read_image` also accepts a
plain unobfuscated `BM`, though whether real releases ship those loose is
**not yet verified**.

## 7. The CP932 caveat

Text is decoded and encoded in `crates/kintsugi-bluegale/src/lib.rs` with
`encoding_rs::SHIFT_JIS`, which *is* Windows-31J — CP932 plus the vendor
extensions these tools wrote. Encoding **back** is strict on purpose:
`encoding_rs` substitutes HTML numeric character references for characters the
code page cannot hold — an em dash (U+2014) becomes the eight literal ASCII
characters `&#8212;` — with only an `unmappable` flag to tell the caller, which
a naive one ignores. In a BDT file that is invisible corruption: the game draws
`&#8212;` on screen. `encode_cp932` checks the flag and returns
`Error::Encoding`, naming each offending character once in order of first
appearance, so a human can reword the line or add a font hack. This was a real
bug: the demo script once held an em dash and a simplified-Chinese character and
the CLI printed `金継ぎ&#8212;&#8212;金&#32558;` before the check existed
(`README.md`, `cp932_refuses_characters_it_cannot_hold`). One asymmetry:
`decode_cp932` ignores `had_errors`, so malformed CP932 becomes U+FFFD silently
(§8.9).

## 8. Unresolved questions and TODO

| # | Open question | What we know | How to verify / next step |
| --- | --- | --- | --- |
| 1 | Semantics of ordinary BDT lines | nothing; all become `RawLine` | diff two scenes of a real `story.bdt` for recurring opcode shapes, then confirm against an emulator trace before typing any line |
| 2 | Do `$` and `%` differ? | the extractor indexes both alike; we label both, and the writer therefore never rewrites one into the other | find a script using both and see which names are jump targets; `%fin`-style end markers are unconfirmed |
| 3 | Byte-preserving write-back on a real file | proven against our fixtures only (9 writer tests in `bdt.rs`, plus `crates/kintsugi/tests/patch.rs`); labels are never rewritten | round-trip a real `story.bdt`: change one line, diff, confirm only that line's bytes moved |
| 4 | `indexwww.dat` | `0xFA0` × `<8sII` known from the extractor; we never emit it | patch a label and see whether the game needs a regenerated index or rejects a stale one |
| 5 | Label names beyond 8 bytes | the index truncates them to 8 | check whether a longer translated label still resolves at run time |
| 6 | Are `.inx` names always CP932? | we decode CP932 and stop at the first NUL; GARbro's overload is invisible in `ArcSNN.cs` | dump the `0x40`-byte fields of a real index and confirm valid CP932 |
| 7 | Archive variants: several SNN/INX pairs, `.snn` with no `.inx`, count sentinels, zero-size entries | we mount every pair (alphabetically last shadows the rest) and handle the rest without a specific check | find a release with more than one pair, count `.snn`/`.inx` files and run `parse_inx` on each |
| 8 | ZBM streams that end early; ZBM re-packing | GARbro tolerates a short buffer, we error; no compressor exists (fixtures are literal-only) | decode a real corpus with the error downgraded to a warning; then add back-reference emission and round-trip a real ZBM |
| 9 | BMP dialect coverage; lossy CP932 decode | our decoder refuses `BITMAPCOREHEADER` and RLE; `had_errors` ignored | decode every image of a real game and count refusals; surface `had_errors` as a `Script::warnings` entry |
| 10 | `parse_bdt` does not consult `looks_like_bdt` | a non-script `.bdt` would decode to garbage, but the heuristic counts only column-0 labels and needs ≥60% CP932-printable bytes, so genuine indented or binary-heavy scripts would be falsely refused | decide the trade-off with real files: measure how many real scripts pass `looks_like_bdt` before wiring it into the parser |
| 11 | AMV (whole format); loose plain-BMP images | frame table and ZBM LZ reuse known; `read_image` accepts `BM` | obtain real `.amv` data and a real release directory before extending either path |

Every item needs data from a real game folder, which must never be committed:
the fixtures are synthesized (§9).

## 9. How to re-verify these claims

Everything runnable in-tree was run when this document was written (2026-10-08):
`cargo test -p kintsugi-bluegale` passes 26 unit + 8 integration tests, and
`cargo test -p kintsugi` adds 5 host-level patch tests in
`crates/kintsugi/tests/patch.rs`. Both suites build from a clean checkout with
`cargo build --workspace`.

`cargo run -p kintsugi -- demo` writes `game.inx`/`game.snn`/`story.bdt` into
`demo-game/` (gitignored and regenerated, not checked in), prints the `certain`
INX/SNN verdict of §6, plays the script, and upscales the title screen to
`demo-game/title-x4-anime4k.png`; `--dir DIR` writes elsewhere, `--auto` skips
the prompt, and `detect`/`inspect` on that directory print the verdict and the
mounted file list. The offline write path is
`cargo run -p kintsugi -- translate ./demo-game --mock --auto --no-play --write-script ./demo-game/out.bdt`,
after which `cargo run -p kintsugi -- play ./demo-game --script out.bdt --auto`
reads the patch back; diffing the two files must show only translated lines.

The checks that need no game are the tests: `roundtrip.rs` synthesizes a
release, detects it as `certain`, mounts it, reads images, audio, and script,
and interprets the script end to end; `patch.rs` drives the write path. The ZBM
back-reference path is covered by a hand-assembled bit stream in `zbm.rs`'s
tests, because `fixtures.rs` has no compressor and emits literal-only streams
(which real decoders accept).

**All fixtures are synthesized.** `crates/kintsugi-bluegale/src/fixtures.rs`
writes valid INX, SNN, ZBM, BBM, and BDT bytes from scratch — including the
demo's Japanese script, written for this project — so no copyrighted bytes,
artwork, or text from any commercial title is in the repository. That is also
why "verified" here means "two independent implementations agree", never "seen
in a shipped game". The reference copies behind these notes lived under
`/tmp/kintsugi-research/` (ephemeral); to go further, fetch [GARbro](https://github.com/morkt/GARbro)
(MIT), whose BlueGale support is under `ArcFormats/BlueGale/` with the bit
reader at `ArcFormats/BitStream.cs`, and the SExtractor Python extractor
(`src/extract_BlueGale_bdt.py` in that project). Where a claim is marked
unconfirmed, the fix is a real game folder plus a measurement — never a guess.
