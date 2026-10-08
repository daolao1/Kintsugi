//! Synthetic shards: build BlueGale-format files from scratch.
//!
//! Testing a repair needs broken pots. These builders produce *valid* SNN,
//! ZBM, BBM, and BDT files — bit-for-bit the layouts the reference
//! implementations expect — plus [`write_demo_game`], a tiny synthetic
//! BlueGale-style release that exercises the whole seam end to end without
//! a single copyrighted byte.

use std::fs;
use std::path::{Path, PathBuf};

use kintsugi_core::error::{Error, Result};

use crate::snn::{INX_NAME_SIZE, INX_RECORD_SIZE};

/// Build an uncompressed BMP: 24-bit or 8-bit paletted, bottom-up rows.
pub fn bmp_bytes(width: u32, height: u32, palette: &[[u8; 3]], pixels: &[u8]) -> Vec<u8> {
    let bpp: u16 = if palette.is_empty() { 24 } else { 8 };
    let bytes_per_pixel = bpp as usize / 8;
    let stride = width as usize * bytes_per_pixel;
    let padded = (stride + 3) & !3usize;
    let palette_bytes = if bpp == 8 { palette.len() * 4 } else { 0 };
    let data_offset = 14 + 40 + palette_bytes;
    let size = data_offset + padded * height as usize;

    let mut out = vec![0u8; size];
    out[0..2].copy_from_slice(b"BM");
    out[2..6].copy_from_slice(&(size as u32).to_le_bytes());
    out[10..14].copy_from_slice(&(data_offset as u32).to_le_bytes());

    let info = &mut out[14..];
    info[0..4].copy_from_slice(&40u32.to_le_bytes());
    info[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    info[8..12].copy_from_slice(&(height as i32).to_le_bytes());
    info[12..14].copy_from_slice(&1u16.to_le_bytes());
    info[14..16].copy_from_slice(&bpp.to_le_bytes());

    if bpp == 8 {
        // Real encoders record the true palette size in biClrUsed.
        out[46..50].copy_from_slice(&(palette.len() as u32).to_le_bytes());
        for (i, [r, g, b]) in palette.iter().enumerate() {
            let at = 14 + 40 + i * 4;
            out[at] = *b;
            out[at + 1] = *g;
            out[at + 2] = *r;
            out[at + 3] = 0;
        }
    }

    for row in 0..height as usize {
        let target = height as usize - 1 - row;
        let src = &pixels[row * stride..row * stride + stride];
        out[data_offset + target * padded..][..stride].copy_from_slice(src);
    }
    out
}

/// Pack `data` into BlueGale's LZ stream using literal tokens only.
///
/// Good enough for fixtures: valid streams the real decoder accepts,
/// without implementing a compressor.
pub fn lz_pack_literals(data: &[u8]) -> Vec<u8> {
    let mut bits: Vec<bool> = Vec::with_capacity(data.len() * 9 + 8);
    bits.push(false); // the discarded leading bit

    let mut offset = 0usize;
    while offset < data.len() {
        let chunk = (data.len() - offset).min(0x7F);
        // 8-bit literal token (high bit clear).
        for i in (0..8).rev() {
            bits.push((chunk >> i) & 1 == 1);
        }
        for &byte in &data[offset..offset + chunk] {
            for i in (0..8).rev() {
                bits.push((byte >> i) & 1 == 1);
            }
        }
        offset += chunk;
    }

    let mut packed = Vec::with_capacity(bits.len() / 8 + 1);
    let mut acc = 0u8;
    let mut n = 0u32;
    for bit in bits {
        acc = (acc << 1) | u8::from(bit);
        n += 1;
        if n == 8 {
            packed.push(acc);
            n = 0;
        }
    }
    if n > 0 {
        packed.push(acc << (8 - n));
    }
    packed
}

/// Wrap a BMP as a ZBM (LZ-packed, optionally with the XOR obfuscation).
///
/// Encoding mirrors decoding: obfuscate the payload first (the decoder
/// un-XORs after unpacking), then pack it as a literal-only LZ stream.
pub fn make_zbm(bmp: &[u8], obfuscate: bool) -> Vec<u8> {
    let mut unpacked = bmp.to_vec();
    if obfuscate {
        let end = unpacked.len().min(crate::zbm::ZBM_XOR_PREFIX);
        for byte in &mut unpacked[..end] {
            *byte ^= 0xFF;
        }
    }
    let packed = lz_pack_literals(&unpacked);

    let mut out = Vec::with_capacity(14 + packed.len());
    out.extend_from_slice(crate::zbm::ZBM_MAGIC);
    out.extend_from_slice(&1i16.to_le_bytes());
    out.extend_from_slice(&(unpacked.len() as u32).to_le_bytes());
    out.extend_from_slice(&14u32.to_le_bytes());
    out.extend_from_slice(&packed);
    out
}

/// Wrap a BMP as a BBM (first 100 bytes XOR-obfuscated).
pub fn make_bbm(bmp: &[u8]) -> Vec<u8> {
    let mut out = bmp.to_vec();
    let end = out.len().min(crate::bbm::BBM_XOR_PREFIX);
    for byte in &mut out[..end] {
        *byte ^= 0xFF;
    }
    out
}

/// Encode text as a BDT script (CP932 + whole-file XOR 0xFF).
///
/// Panics if the text is not CP932-clean: fixtures must be text a real
/// BlueGale tool could have written.
pub fn make_bdt(text: &str) -> Vec<u8> {
    crate::encode_cp932(text)
        .expect("fixture text must be CP932-clean")
        .into_iter()
        .map(|b| b ^ 0xFF)
        .collect()
}

/// Build an INX index for `(name, offset, size)` records.
pub fn make_inx(entries: &[(&str, u32, u32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + entries.len() * INX_RECORD_SIZE);
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (name, offset, size) in entries {
        let mut field = [0u8; INX_NAME_SIZE];
        field[..name.len()].copy_from_slice(name.as_bytes());
        out.extend_from_slice(&field);
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
    }
    out
}

/// Concatenate blobs and return `(snn_bytes, (offset, size) list)`.
pub fn make_snn(blobs: &[&[u8]]) -> (Vec<u8>, Vec<(u32, u32)>) {
    let mut data = Vec::new();
    let mut placements = Vec::with_capacity(blobs.len());
    for blob in blobs {
        placements.push((data.len() as u32, blob.len() as u32));
        data.extend_from_slice(blob);
    }
    (data, placements)
}

/// A tiny synthetic "BlueGale release" for tests and `kintsugi demo`.
///
/// Contains:
/// * `game.inx` + `game.snn` — one archive holding a title screen (BBM),
///   a room background (ZBM), a hero portrait (ZBM, obfuscated), and an
///   Ogg stub;
/// * `story.bdt` — the kintsugi story itself, in Japanese, labels and all.
///
/// Returns the list of created file paths.
/// The note a demo folder carries, so a second run may overwrite it while a
/// real game folder is never touched by accident.
const DEMO_MARKER: &str = ".kintsugi-demo";

/// Write a tiny, complete, BlueGale-shaped game into `dir`.
///
/// Every byte is synthesized here: the archive, the index, the images, and the
/// script. Nothing is copied from a real game, so the whole pipeline can be
/// exercised without owning one — and nothing copyrighted is in this
/// repository. Returns the files written.
pub fn write_demo_game(dir: &Path) -> Result<Vec<PathBuf>> {
    // This is the one command that writes *into* a folder, which makes it the
    // one that could land on a real game: `demo --dir <game folder>` must not
    // overwrite originals. A folder this command made carries the marker below
    // and may be rewritten; any other folder with files in it is refused, so
    // the demo can be re-run without becoming a way to clobber someone's game.
    let marker = dir.join(DEMO_MARKER);
    if dir.exists() && !marker.exists() {
        let mut occupied: Vec<String> = fs::read_dir(dir)
            .map_err(|e| Error::Io(format!("reading {}: {e}", dir.display())))?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        occupied.sort();
        if !occupied.is_empty() {
            let shown: Vec<&str> = occupied.iter().take(3).map(String::as_str).collect();
            return Err(Error::unsupported(
                "demo game",
                format!(
                    "'{}' already holds {} file(s) ({}{}) and was not written by this \
                     command; refusing to write a demo game over them — delete that \
                     folder or point --dir at a new one",
                    dir.display(),
                    occupied.len(),
                    shown.join(", "),
                    if occupied.len() > shown.len() {
                        ", …"
                    } else {
                        ""
                    }
                ),
            ));
        }
    }
    fs::create_dir_all(dir).map_err(|e| Error::Io(format!("creating {}: {e}", dir.display())))?;

    // 4x3 title logo, 24bpp: a gold seam across a dark field.
    let mut title_pixels = Vec::new();
    for y in 0..3u32 {
        for x in 0..4u32 {
            let on_gold_seam = y == 1;
            let (r, g, b) = if on_gold_seam {
                (212, 175, 55) // gold
            } else {
                (34 - (x * 4) as u8, 28 - (y * 4) as u8, 24) // near-black pottery
            };
            title_pixels.extend_from_slice(&[b, g, r]);
        }
    }
    let title_bmp = bmp_bytes(4, 3, &[], &title_pixels);
    let title_bbm = make_bbm(&title_bmp);

    // 4x4 room background, 24bpp gradient.
    let mut room_pixels: Vec<u8> = Vec::new();
    for y in 0..4u8 {
        for x in 0..4u8 {
            let (r, g, b) = (40 + x * 20, 30 + y * 20, 60u8);
            room_pixels.extend_from_slice(&[b, g, r]);
        }
    }
    let room_bmp = bmp_bytes(4, 4, &[], &room_pixels);
    let room_zbm = make_zbm(&room_bmp, false);

    // 3x3 hero portrait, 8bpp paletted, obfuscated ZBM.
    let palette = [
        [0, 0, 0],
        [212, 175, 55],
        [255, 255, 255],
        [120, 80, 40],
        [200, 160, 90],
    ];
    let face_pixels: Vec<u8> = vec![
        3, 4, 3, //
        4, 1, 4, //
        3, 4, 3, //
    ];
    let face_bmp = bmp_bytes(3, 3, &palette, &face_pixels);
    let face_zbm = make_zbm(&face_bmp, true);

    let ogg_stub = {
        let mut v = b"OggS".to_vec();
        v.extend_from_slice(&[0u8; 32]);
        v
    };

    // A four-frame cutscene, all 5x3, drifting from night to dawn: a frame
    // sequence of one size, which is what frame interpolation (插帧) needs
    // and what a real game's cutscene folder looks like. The other images are
    // deliberately different sizes, because a game directory really does hold
    // images that are not frames of anything.
    let mut cut_frames = Vec::new();
    for step in 0..4u8 {
        let mut pixels = Vec::new();
        for y in 0..3u8 {
            for x in 0..5u8 {
                // The x term keeps every column distinct, so a blended
                // in-between is visibly different from both its neighbours.
                let (r, g, b) = (
                    20 + step * 55 + x * 3,
                    16 + step * 30 + y * 8,
                    40u8.saturating_sub(step * 8),
                );
                pixels.extend_from_slice(&[b, g, r]);
            }
        }
        let bmp = bmp_bytes(5, 3, &[], &pixels);
        cut_frames.push(make_zbm(&bmp, false));
    }

    let mut blobs: Vec<&[u8]> = vec![&title_bbm, &room_zbm, &face_zbm, &ogg_stub];
    blobs.extend(cut_frames.iter().map(|frame| frame.as_slice()));
    let mut names = vec!["TITLE.BBM", "ROOM.ZBM", "FACE.ZBM", "OPENING.OGG"];
    names.extend(["CUT01.ZBM", "CUT02.ZBM", "CUT03.ZBM", "CUT04.ZBM"]);

    let (snn, placements) = make_snn(&blobs);
    let records: Vec<(&str, u32, u32)> = names
        .iter()
        .zip(placements)
        .map(|(name, (offset, size))| (*name, offset, size))
        .collect();
    let inx = make_inx(&records);

    let story = [
        "$start",
        "深夜の工房。陶器の砕ける音が、静寂を裂いた。",
        "祖母の形見の茶碗が、五つの欠片に散らばっている。",
        "$shatter",
        "「直せるものなら、直したい……」",
        "だが彼女は、傷を隠すことをよしとしなかった。",
        "$repair",
        "金を練り、漆で欠片を合わせていく。",
        "亀裂を消すのではなく、金の川として描き直す。",
        "$seam",
        // U+2015 double dash and 繕 are both in CP932; an em dash and the
        // simplified 缮 are not, which is exactly what the strict encoder
        // refuses (see `encode_cp932`).
        "それは修復ではなく、金継ぎ――金繕いだった。",
        "器は、砕ける前よりも美しくなった。",
        "欠けた時間ごと、黄金で結ばれて。",
        "%fin",
    ]
    .join("\r\n");
    let story_bdt = make_bdt(&format!("{story}\r\n"));

    let inx_path = dir.join("game.inx");
    let snn_path = dir.join("game.snn");
    let bdt_path = dir.join("story.bdt");
    fs::write(&inx_path, &inx)
        .map_err(|e| Error::Io(format!("writing {}: {e}", inx_path.display())))?;
    fs::write(&snn_path, &snn)
        .map_err(|e| Error::Io(format!("writing {}: {e}", snn_path.display())))?;
    fs::write(&bdt_path, &story_bdt)
        .map_err(|e| Error::Io(format!("writing {}: {e}", bdt_path.display())))?;

    // Bookkeeping, not part of the game, so it is written but not listed: the
    // marker is what lets the next run know this folder is a demo folder.
    let marker_path = dir.join(DEMO_MARKER);
    fs::write(
        &marker_path,
        "This folder holds a synthetic demo game written by `kintsugi demo`,\n\
         not a real one. It carries no copyrighted bytes and is safe to delete.\n",
    )
    .map_err(|e| Error::Io(format!("writing {}: {e}", marker_path.display())))?;

    Ok(vec![inx_path, snn_path, bdt_path])
}
