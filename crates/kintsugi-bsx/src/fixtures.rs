//! Fixtures for this seam's games, built in memory byte by byte.
//!
//! Nothing here comes from a game. The point of a fixture is that it can be
//! read, argued with, and trusted in a test: every byte is written by the
//! builder below, from the layouts in `docs/RESEARCH-BSX.md`, so a test failure
//! means the seam changed rather than that somebody's disc did.
//!
//! The builders are also the seam's own understanding of the formats written
//! down twice — once as a reader, once as a writer — which is what catches a
//! reader that accepts things a writer would never produce.

use std::fs;
use std::path::{Path, PathBuf};

use kintsugi_core::error::{Error, Result};

use crate::image::{COMPOSITION_MAGIC, GRAPHICS_MAGIC};

/// Marks a folder this module made, so a demo can be rewritten but never
/// written over somebody's game. The same file name the other seam's demo
/// writer uses, for the same reason.
pub const DEMO_MARKER: &str = ".kintsugi-demo";

/// Bytes of header a BSG carries past its magic.
const BSG_HEADER_LEN: usize = 0x3E;

/// Bytes in one BSArc index record.
pub const RECORD_SIZE: usize = 0x28;

/// Bytes of a record's fixed name field.
pub const NAME_SIZE: usize = 0x20;

/// Build a `BSS-Graphics` image with run-length data from RGBA pixels,
/// bottom-up the way the real ones are stored.
///
/// The pixels are given top-down, so the builder does the flip: a fixture that
/// lied about the row order would make the decoder's own flip untestable.
pub fn make_bsg(width: u16, height: u16, rgba: &[[u8; 4]]) -> Vec<u8> {
    assert_eq!(
        rgba.len(),
        width as usize * height as usize,
        "a fixture needs one colour per pixel"
    );
    // Four byte planes: blue, green, red, alpha, each one run-length stream.
    let mut planes = Vec::new();
    for channel in 0..4 {
        let mut stream = Vec::new();
        let mut row = height as usize;
        while row > 0 {
            row -= 1; // stored bottom-up
            for col in 0..width as usize {
                let pixel = rgba[row * width as usize + col];
                let byte = match channel {
                    0 => pixel[2],
                    1 => pixel[1],
                    2 => pixel[0],
                    _ => pixel[3],
                };
                stream.push(byte);
            }
        }
        planes.extend_from_slice(&encode_rle_literals(&stream));
    }
    build_bsg(0, 0, width, height, &planes, None)
}

/// Build a `BSS-Composition` image: the same payload behind a 0x20 byte
/// container header.
pub fn make_composition(width: u16, height: u16, rgba: &[[u8; 4]]) -> Vec<u8> {
    let plain = make_bsg(width, height, rgba);
    let mut out = vec![0u8; 0x20];
    out[..COMPOSITION_MAGIC.len()].copy_from_slice(COMPOSITION_MAGIC);
    out.extend_from_slice(&plain);
    out
}

/// Build an indexed `BSS-Graphics` image with a palette.
pub fn make_indexed_bsg(width: u16, height: u16, indices: &[u8], palette: &[[u8; 3]]) -> Vec<u8> {
    assert_eq!(indices.len(), width as usize * height as usize);
    let mut stream = Vec::new();
    let mut row = height as usize;
    while row > 0 {
        row -= 1;
        for col in 0..width as usize {
            stream.push(indices[row * width as usize + col]);
        }
    }
    let data = encode_rle_literals(&stream);
    let mut palette_bytes = vec![0u8; 0x100 * 4];
    for (i, colour) in palette.iter().enumerate() {
        // Stored blue-green-red-unused, like a BMP palette.
        palette_bytes[i * 4] = colour[2];
        palette_bytes[i * 4 + 1] = colour[1];
        palette_bytes[i * 4 + 2] = colour[0];
    }
    build_bsg(0, 2, width, height, &data, Some(&palette_bytes))
}

/// One run-length plane: the plane's byte count, then one packet of literals.
///
/// Real streams mix literals and repeats; a fixture that used repeats
/// everywhere would never exercise the literal path, so the *pixels* decide and
/// the encoder keeps it simple by writing literals.
fn encode_rle_literals(bytes: &[u8]) -> Vec<u8> {
    let mut stream = Vec::new();
    for chunk in bytes.chunks(128) {
        stream.push((chunk.len() - 1) as u8);
        stream.extend_from_slice(chunk);
    }
    let mut out = (stream.len() as i32).to_le_bytes().to_vec();
    out.extend_from_slice(&stream);
    out
}

/// Lay out a BSG header around `data`.
fn build_bsg(
    base: usize,
    colour_mode: u8,
    width: u16,
    height: u16,
    data: &[u8],
    palette: Option<&[u8]>,
) -> Vec<u8> {
    let pixel_size = if colour_mode == 2 { 1 } else { 4 };
    let unpacked = (width as u32) * (height as u32) * (pixel_size as u32);
    let mut out = vec![0u8; base + BSG_HEADER_LEN];
    out[..GRAPHICS_MAGIC.len()].copy_from_slice(GRAPHICS_MAGIC);
    if base > 0 {
        out[..COMPOSITION_MAGIC.len()].copy_from_slice(COMPOSITION_MAGIC);
        out[base..base + GRAPHICS_MAGIC.len()].copy_from_slice(GRAPHICS_MAGIC);
    }
    out[base + 0x12..base + 0x16].copy_from_slice(&unpacked.to_le_bytes());
    out[base + 0x16..base + 0x18].copy_from_slice(&width.to_le_bytes());
    out[base + 0x18..base + 0x1A].copy_from_slice(&height.to_le_bytes());
    out[base + 0x30] = colour_mode;
    out[base + 0x31] = 1; // run length: what every real image of this family uses
    let data_offset = out.len() - base;
    out[base + 0x32..base + 0x36].copy_from_slice(&(data_offset as i32).to_le_bytes());
    out[base + 0x36..base + 0x3A].copy_from_slice(&(data.len() as i32).to_le_bytes());
    out.extend_from_slice(data);
    let palette_offset = out.len() - base;
    out[base + 0x3A..base + 0x3E].copy_from_slice(&(palette_offset as i32).to_le_bytes());
    if let Some(palette) = palette {
        out.extend_from_slice(palette);
    }
    out
}

/// Build a `BSArc` archive holding `entries`, in the layout the real ones use:
/// header, then the file data, then the index records.
pub fn make_bsarc(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(crate::bsarc::MAGIC);
    out.extend_from_slice(&[0, 0, 0]); // padding up to offset 8
    out.extend_from_slice(&2u16.to_le_bytes()); // version
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let placeholder = out.len();
    out.extend_from_slice(&0u32.to_le_bytes()); // index offset, filled in below

    let mut records = Vec::new();
    for (name, bytes) in entries {
        let offset = out.len() as u32;
        out.extend_from_slice(bytes);
        let mut record = [0u8; RECORD_SIZE];
        let raw = name.as_bytes();
        assert!(
            raw.len() < NAME_SIZE,
            "'{name}' does not fit a record's name"
        );
        record[..raw.len()].copy_from_slice(raw);
        record[0x20..0x24].copy_from_slice(&offset.to_le_bytes());
        record[0x24..0x28].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        records.push(record);
    }
    let index_offset = out.len() as u32;
    out[placeholder..placeholder + 4].copy_from_slice(&index_offset.to_le_bytes());
    for record in records {
        out.extend_from_slice(&record);
    }
    out
}

/// A `bsx.dat` header: the magic, the version, and nothing else.
///
/// Detection reads the magic and refuses to guess past it, so this is exactly
/// as much of a story file as a detection fixture needs. The real layout — a
/// table of addresses and lengths, then a heap of CP932 text — is documented in
/// `docs/RESEARCH-BSX.md` and is not what this builder claims to produce.
pub fn make_bsx_dat() -> Vec<u8> {
    let mut out = crate::SCRIPT_MAGIC.to_vec();
    out.extend_from_slice(b" Rev.6\0\0");
    out.extend_from_slice(&[0u8; 0x100 - 16]);
    out
}

/// Write a tiny, complete, BSX-shaped game into `dir`, and list what was written.
///
/// The shape is the one a real release has: a configuration file, the compiled
/// story's magic, a loose picture, and an archive holding two more. It exists so
/// the shell can be tested end to end against a **second** engine without any
/// game data — `kintsugi detect`, `inspect` and `upscale` all have to work on a
/// folder they have never seen, and a fixture that only the seam's own tests use
/// would not prove that.
pub fn write_demo_game(dir: &Path) -> Result<Vec<PathBuf>> {
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
                    "'{}' already holds {} file(s) ({}{}) and was not written by this command; \
                     refusing to write a demo game over them — delete that folder or point at a \
                     new one",
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

    // A gold seam across dark pottery, the same picture the other demo draws:
    // at four by four it is small enough to check by eye and by byte.
    let gold = [212u8, 175, 55, 255];
    let mut pottery = Vec::new();
    for y in 0..4u16 {
        for x in 0..4u16 {
            // A gold seam across dark pottery: dark enough that a channel swap
            // would be visible, and not one flat colour, so a decoder that
            // dropped a plane could not pass by accident.
            let shade = 34 - (x as u8) * 4;
            pottery.push(if y == 1 {
                gold
            } else {
                [shade, shade - 6, 24, 255]
            });
        }
    }
    let room = make_bsg(4, 4, &pottery);
    // An indexed picture as well, because indexed images take a different path
    // through the decoder and a demo that only ever wrote true colour would
    // leave that path untested by the shell.
    let indexed = make_indexed_bsg(
        3,
        1,
        &[0, 1, 2],
        &[[10, 10, 10], [212, 175, 55], [240, 240, 240]],
    );
    let title = make_composition(1, 1, &[[240, 240, 240, 255]]);
    let archive = make_bsarc(&[("room.bsg", &room), ("face.bsg", &indexed)]);

    let written = vec![
        (dir.join("exe").join("bsx.ini"), demo_config()),
        (dir.join("exe").join("bsx.dat"), make_bsx_dat()),
        (dir.join("exe").join("Graphics.bsa"), archive),
        (dir.join("exe").join("title.bsg"), title),
    ];
    let mut paths = Vec::new();
    for (path, bytes) in &written {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::Io(format!("creating {}: {e}", parent.display())))?;
        }
        fs::write(path, bytes)
            .map_err(|e| Error::Io(format!("writing {}: {e}", path.display())))?;
        paths.push(path.clone());
    }
    fs::write(&marker, b"kintsugi demo game\n")
        .map_err(|e| Error::Io(format!("writing {}: {e}", marker.display())))?;
    Ok(paths)
}

/// The configuration file a real release carries: the game and the studio on
/// the first line, then the file names the engine looks for.
fn demo_config() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"DEMO@BLUEGALE");
    out.resize(0x20, 0);
    out.extend_from_slice(b"DEMO");
    out.resize(0x30, 0);
    out.extend_from_slice(b"disk.ini");
    out.resize(0x3E, 0);
    out.extend_from_slice(b"bsx.dat");
    out.resize(0x4C, 0);
    out
}

/// A whole BSX release, small enough to reason about: a configuration that
/// names the system, one archive holding one picture, and a loose picture
/// beside it.
///
/// The story file is deliberately absent: this seam does not read BSScript
/// yet, so a fixture that carried one would either have to be ignored — which
/// would make the test weaker than it looks — or would fail. When the script
/// reader lands, this builder grows a script and the conformance test grows a
/// `.script(...)` line with it.
pub fn demo_release() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let picture = make_bsg(
        2,
        2,
        &[
            [200, 40, 40, 255],
            [40, 200, 40, 255],
            [40, 40, 200, 255],
            [240, 240, 240, 255],
        ],
    );
    let archive = make_bsarc(&[("room.bsg", &picture)]);
    let loose = make_bsg(1, 1, &[[10, 20, 30, 255]]);
    (archive, loose, demo_config())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::decode_bsg;

    #[test]
    fn a_built_picture_decodes_back_to_its_pixels() {
        let pixels = [
            [1, 2, 3, 255],
            [4, 5, 6, 128],
            [7, 8, 9, 255],
            [10, 11, 12, 0],
        ];
        let bytes = make_bsg(2, 2, &pixels);
        let image = decode_bsg(&bytes, "fixture.bsg").expect("a built picture decodes");
        assert_eq!(image.rgba(), pixels.concat().as_slice());
    }

    #[test]
    fn a_built_composition_decodes_back_to_its_pixels() {
        let pixels = [[1, 2, 3, 255]];
        let bytes = make_composition(1, 1, &pixels);
        let image = decode_bsg(&bytes, "fixture.bsg").expect("a built composition decodes");
        assert_eq!(image.rgba(), &[1, 2, 3, 255]);
    }

    #[test]
    fn a_built_indexed_picture_decodes_back_to_its_palette() {
        let bytes = make_indexed_bsg(2, 1, &[0, 1], &[[9, 8, 7], [6, 5, 4]]);
        let image = decode_bsg(&bytes, "fixture.bsg").expect("a built indexed picture decodes");
        assert_eq!(image.rgba(), &[9, 8, 7, 255, 6, 5, 4, 255]);
    }

    #[test]
    fn a_built_archive_lists_and_reads_its_entries() {
        let archive = make_bsarc(&[("a.bsg", b"first"), ("b.bsg", b"second!")]);
        let parsed = crate::bsarc::BsarcArchive::open(&archive).expect("a built archive parses");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed.entries()[0].path.as_str(), "a.bsg");
        assert_eq!(parsed.data_bytes(), 12);
    }
}
