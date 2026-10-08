//! The `BSG` still image: BlueGale's and Bishop's shared picture format.
//!
//! # The format
//!
//! Read out of 765 real images of a 2008 BlueGale release and cross-checked
//! against GARbro's independent `ArcFormats/Bishop/ImageBSG.cs`:
//!
//! ```text
//! 0x00  "BSS-Graphics\0"   16 bytes, or:
//! 0x00  "BSS-Composition\0"
//! 0x20  "BSS-Graphics\0"   what follows is relative to 0x20
//! base+0x12  i32 unpacked size
//! base+0x16  u16 width
//! base+0x18  u16 height
//! base+0x20  i16 x offset (for composed images)
//! base+0x22  i16 y offset
//! base+0x30  u8  colour mode: 0 = BGRA, 1 = BGR, 2 = 8-bit indexed
//! base+0x31  u8  compression: 0 = stored, 1 = run length, 2 = LZ
//! base+0x32  i32 data offset (relative to base)
//! base+0x36  i32 data size
//! base+0x3A  i32 palette offset (relative to base)
//! ```
//!
//! An indexed image's palette is 256 entries of four bytes, blue-green-red-unused,
//! the same layout a BMP uses.
//!
//! # What this seam does and does not decode
//!
//! Colour modes 0, 1 and 2 with **stored** or **run-length** data: that is
//! everything the 765 images checked use (compression 1 in every one of them,
//! colour modes 0, 1 and 2 all present). LZ data (compression 2) is refused
//! with its name in the message rather than half-guessed: the layout is
//! recorded in `docs/RESEARCH-BSX.md`, so whoever meets a game that needs it
//! can implement it against a real file instead of a memory.
//!
//! # Why rows are flipped
//!
//! The pixels are stored bottom-up, the way a BMP is, and the body's [`Image`]
//! is top-down. The flip is not cosmetic: an unflipped repair would hand back a
//! vertically mirrored picture, which is the kind of quiet wrongness this
//! project exists to avoid.

use kintsugi_core::asset::{Image, ImageFormat};
use kintsugi_core::bytes::Reader;
use kintsugi_core::error::{Error, Result};

/// Magic of a plain image.
pub const GRAPHICS_MAGIC: &[u8] = b"BSS-Graphics\0";

/// Magic of a composed image: a container whose payload starts 0x20 bytes in.
pub const COMPOSITION_MAGIC: &[u8] = b"BSS-Composition\0";

/// Bytes of header this decoder reads past the magic.
const HEADER_LEN: usize = 0x3E;

/// Palette entries in an indexed image.
const PALETTE_ENTRIES: usize = 0x100;

/// Bytes of one palette entry.
const PALETTE_ENTRY: usize = 4;

/// Refuse images larger than this many pixels (about a 256 MB RGBA buffer),
/// matching the body's BMP decoder.
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;

fn corrupt(detail: impl Into<String>) -> Error {
    Error::corrupt("BSG", detail)
}

fn unsupported(detail: impl Into<String>) -> Error {
    Error::unsupported("BSG", detail)
}

/// The fields of a BSG header that this decoder uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BsgHeader {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// 0 = BGRA, 1 = BGR, 2 = indexed.
    pub colour_mode: u8,
    /// 0 = stored, 1 = run length, 2 = LZ.
    pub compression: u8,
    /// Bytes the decoder is expected to write.
    pub unpacked_size: u32,
    /// Where the pixel data starts, absolute.
    pub data_offset: usize,
    /// How many bytes of pixel data there are.
    pub data_size: usize,
    /// Where the palette starts, absolute.
    pub palette_offset: usize,
}

impl BsgHeader {
    /// Bytes per pixel before the byte planes are interleaved: 4 for a true
    /// colour image, 1 for an indexed one.
    pub fn pixel_size(&self) -> usize {
        if self.colour_mode == 2 { 1 } else { 4 }
    }

    /// How many byte planes the data is split into, one per channel.
    pub fn planes(&self) -> usize {
        match self.colour_mode {
            0 => 4,
            1 => 3,
            _ => 1,
        }
    }

    /// Bytes one decoded image needs.
    pub fn expected_size(&self) -> u64 {
        (self.width as u64) * (self.height as u64) * (self.pixel_size() as u64)
    }
}

/// Does this look like a BSG image? Only the magic is read.
pub fn is_bsg(data: &[u8]) -> bool {
    data.starts_with(GRAPHICS_MAGIC) || data.starts_with(COMPOSITION_MAGIC)
}

/// Read a BSG header without decoding pixels.
pub fn header(data: &[u8], label: &str) -> Result<BsgHeader> {
    let where_ = |detail: String| {
        if label.is_empty() {
            corrupt(detail)
        } else {
            corrupt(format!("'{label}': {detail}"))
        }
    };
    let base = if data.starts_with(COMPOSITION_MAGIC) {
        0x20
    } else if data.starts_with(GRAPHICS_MAGIC) {
        0
    } else {
        return Err(unsupported(format!(
            "'{label}' does not start with 'BSS-Graphics' or 'BSS-Composition'"
        )));
    };
    if data.len() < base + HEADER_LEN {
        return Err(where_(format!(
            "a header needs {} bytes, the file has {}",
            base + HEADER_LEN,
            data.len()
        )));
    }
    let mut reader = Reader::new(data);
    reader.seek(base + 0x12)?;
    let unpacked_size = reader.u32le()?;
    let width = reader.u16le()? as u32;
    let height = reader.u16le()? as u32;
    reader.seek(base + 0x30)?;
    let colour_mode = reader.u8()?;
    let compression = reader.u8()?;
    let data_offset = reader.i32le()?;
    let data_size = reader.i32le()?;
    let palette_offset = reader.i32le()?;
    if data_offset < 0 || data_size < 0 || palette_offset < 0 {
        return Err(where_(
            "a negative offset or size: this is not the layout this seam knows".to_string(),
        ));
    }
    Ok(BsgHeader {
        width,
        height,
        colour_mode,
        compression,
        unpacked_size,
        data_offset: base + data_offset as usize,
        data_size: data_size as usize,
        palette_offset: base + palette_offset as usize,
    })
}

/// Decode a BSG image into the body's RGBA8 form.
pub fn decode_bsg(data: &[u8], label: &str) -> Result<Image> {
    let header = header(data, label)?;
    let where_ = |detail: String| {
        if label.is_empty() {
            corrupt(detail)
        } else {
            corrupt(format!("'{label}': {detail}"))
        }
    };

    if header.width == 0 || header.height == 0 {
        return Err(where_(format!(
            "{}x{} is not an image",
            header.width, header.height
        )));
    }
    let pixels = (header.width as u64) * (header.height as u64);
    if pixels > MAX_PIXELS {
        return Err(unsupported(format!(
            "'{label}' is {pixels} pixels, past this decoder's limit of {MAX_PIXELS}"
        )));
    }
    if header.colour_mode > 2 {
        return Err(unsupported(format!(
            "'{label}' has colour mode {}, and this seam knows 0 (BGRA), 1 (BGR) and 2 (indexed)",
            header.colour_mode
        )));
    }
    let expected = header.expected_size();
    if header.unpacked_size as u64 != expected {
        return Err(where_(format!(
            "the header says {} unpacked bytes, but {}x{} in mode {} needs {expected}",
            header.unpacked_size, header.width, header.height, header.colour_mode
        )));
    }
    let end = header
        .data_offset
        .checked_add(header.data_size)
        .ok_or_else(|| where_("the data extent overflows".to_string()))?;
    if end > data.len() {
        return Err(where_(format!(
            "pixel data runs to {end}, past the end of a {} byte file",
            data.len()
        )));
    }

    let mut planes = vec![0u8; expected as usize];
    match header.compression {
        0 => unpack_stored(data, &header, &mut planes, &where_)?,
        1 => unpack_run_length(data, &header, &mut planes, &where_)?,
        2 => {
            return Err(unsupported(format!(
                "'{label}' is LZ compressed (compression 2): the layout is documented in \
                 docs/RESEARCH-BSX.md but this seam has no file to verify an implementation \
                 against, so it refuses instead of guessing"
            )));
        }
        other => {
            return Err(unsupported(format!(
                "'{label}' has compression {other}, and this seam knows 0 (stored), 1 (run \
                 length) and 2 (LZ)"
            )));
        }
    }

    let palette = if header.colour_mode == 2 {
        let palette_end = header
            .palette_offset
            .checked_add(PALETTE_ENTRIES * PALETTE_ENTRY)
            .ok_or_else(|| where_("the palette extent overflows".to_string()))?;
        if palette_end > data.len() {
            return Err(where_(format!(
                "a {PALETTE_ENTRIES} entry palette at {} runs to {palette_end}, past the end \
                 of a {} byte file",
                header.palette_offset,
                data.len()
            )));
        }
        Some(&data[header.palette_offset..palette_end])
    } else {
        None
    };

    Ok(interleave(&header, &planes, palette))
}

/// Compression 0: the bytes are the pixels, except that a BGR image stores
/// three bytes per pixel and the body wants four.
fn unpack_stored(
    data: &[u8],
    header: &BsgHeader,
    planes: &mut [u8],
    where_: &impl Fn(String) -> Error,
) -> Result<()> {
    let source = &data[header.data_offset..header.data_offset + header.data_size];
    match header.colour_mode {
        1 => {
            if header.data_size != (header.width as usize) * (header.height as usize) * 3 {
                return Err(where_(format!(
                    "a stored BGR image needs {} bytes of data, the header says {}",
                    (header.width as usize) * (header.height as usize) * 3,
                    header.data_size
                )));
            }
            for (pixel, bgr) in source.chunks_exact(3).enumerate() {
                planes[pixel * 4..pixel * 4 + 3].copy_from_slice(bgr);
                planes[pixel * 4 + 3] = 255;
            }
        }
        _ => {
            if header.data_size < planes.len() {
                return Err(where_(format!(
                    "a stored image needs {} bytes of data, the header says {}",
                    planes.len(),
                    header.data_size
                )));
            }
            planes.copy_from_slice(&source[..planes.len()]);
        }
    }
    Ok(())
}

/// Compression 1: one run-length stream per byte plane.
///
/// Each plane starts with the number of bytes its stream occupies, then
/// packets: a non-negative count means "this many literal bytes follow, plus
/// one", a negative one means "repeat the next byte", where the repeat count is
/// `1 - count`. Bytes are written every `pixel_size` bytes, so the planes
/// interleave rather than concatenate.
fn unpack_run_length(
    data: &[u8],
    header: &BsgHeader,
    planes: &mut [u8],
    where_: &impl Fn(String) -> Error,
) -> Result<()> {
    let step = header.pixel_size();
    let mut reader = Reader::new(data);
    reader.seek(header.data_offset)?;
    for plane in 0..header.planes() {
        let mut remaining = reader.i32le()?;
        if remaining < 0 {
            return Err(where_(format!(
                "plane {plane} declares {remaining} bytes of stream"
            )));
        }
        if remaining as usize > reader.remaining() {
            return Err(where_(format!(
                "plane {plane} declares {remaining} bytes of stream but only {} are left",
                reader.remaining()
            )));
        }
        let mut dst = plane;
        while remaining > 0 {
            let count = reader.u8()? as i8;
            remaining -= 1;
            if count >= 0 {
                for _ in 0..=count {
                    let byte = reader.u8()?;
                    remaining -= 1;
                    if dst >= planes.len() {
                        return Err(where_(format!(
                            "the run-length stream of plane {plane} writes past the end of the \
                             image"
                        )));
                    }
                    planes[dst] = byte;
                    dst += step;
                }
            } else {
                let repeats = 1 - count as i32;
                let byte = reader.u8()?;
                remaining -= 1;
                for _ in 0..repeats {
                    if dst >= planes.len() {
                        return Err(where_(format!(
                            "the run-length stream of plane {plane} writes past the end of the \
                             image"
                        )));
                    }
                    planes[dst] = byte;
                    dst += step;
                }
            }
        }
    }
    Ok(())
}

/// Turn decoded planes into a top-down RGBA image.
fn interleave(header: &BsgHeader, planes: &[u8], palette: Option<&[u8]>) -> Image {
    let width = header.width as usize;
    let height = header.height as usize;
    let mut image = Image::new(header.width, header.height);
    let out = &mut image.data;
    for row in 0..height {
        // Stored bottom-up, handed over top-down.
        let source_row = height - 1 - row;
        for col in 0..width {
            let pixel = source_row * width + col;
            let dst = (row * width + col) * 4;
            match header.colour_mode {
                2 => {
                    let index = planes[pixel] as usize;
                    let entry = &palette.expect("indexed images carry a palette")
                        [index * PALETTE_ENTRY..index * PALETTE_ENTRY + 3];
                    out[dst] = entry[2];
                    out[dst + 1] = entry[1];
                    out[dst + 2] = entry[0];
                    out[dst + 3] = 255;
                }
                1 => {
                    out[dst] = planes[pixel * 4 + 2];
                    out[dst + 1] = planes[pixel * 4 + 1];
                    out[dst + 2] = planes[pixel * 4];
                    out[dst + 3] = 255;
                }
                _ => {
                    out[dst] = planes[pixel * 4 + 2];
                    out[dst + 1] = planes[pixel * 4 + 1];
                    out[dst + 2] = planes[pixel * 4];
                    out[dst + 3] = planes[pixel * 4 + 3];
                }
            }
        }
    }
    image.format = ImageFormat::Rgba8;
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a BSG image with the given header fields and payload, laid out the
    /// way the real ones are: header, then data, then palette, all relative to
    /// the base.
    fn build(
        base: usize,
        colour_mode: u8,
        compression: u8,
        width: u16,
        height: u16,
        payload: &[u8],
        palette: Option<&[u8]>,
    ) -> Vec<u8> {
        let pixel_size = if colour_mode == 2 { 1 } else { 4 };
        let unpacked = (width as u32) * (height as u32) * (pixel_size as u32);
        let mut out = vec![0u8; base + HEADER_LEN];
        out[..GRAPHICS_MAGIC.len()].copy_from_slice(GRAPHICS_MAGIC);
        if base > 0 {
            out[..COMPOSITION_MAGIC.len()].copy_from_slice(COMPOSITION_MAGIC);
            out[base..base + GRAPHICS_MAGIC.len()].copy_from_slice(GRAPHICS_MAGIC);
        }
        out[base + 0x12..base + 0x16].copy_from_slice(&unpacked.to_le_bytes());
        out[base + 0x16..base + 0x18].copy_from_slice(&width.to_le_bytes());
        out[base + 0x18..base + 0x1A].copy_from_slice(&height.to_le_bytes());
        out[base + 0x30] = colour_mode;
        out[base + 0x31] = compression;
        let data_offset = out.len() - base;
        out[base + 0x32..base + 0x36].copy_from_slice(&(data_offset as i32).to_le_bytes());
        out[base + 0x36..base + 0x3A].copy_from_slice(&(payload.len() as i32).to_le_bytes());
        out.extend_from_slice(payload);
        let palette_offset = out.len() - base;
        out[base + 0x3A..base + 0x3E].copy_from_slice(&(palette_offset as i32).to_le_bytes());
        if let Some(palette) = palette {
            out.extend_from_slice(palette);
        }
        out
    }

    /// One run-length plane: literal runs and repeats, in stream order.
    enum Packet {
        Literal(Vec<u8>),
        Repeat(u8, usize),
    }

    fn rle_plane(packets: &[Packet]) -> Vec<u8> {
        let mut stream = Vec::new();
        for packet in packets {
            match packet {
                Packet::Literal(bytes) => {
                    // A count of n means n+1 bytes, so a 256 byte run is split.
                    for chunk in bytes.chunks(128) {
                        stream.push((chunk.len() - 1) as u8);
                        stream.extend_from_slice(chunk);
                    }
                }
                Packet::Repeat(byte, count) => {
                    let mut left = *count;
                    while left > 0 {
                        let run = left.min(128);
                        stream.push((1i32 - run as i32) as i8 as u8);
                        stream.push(*byte);
                        left -= run;
                    }
                }
            }
        }
        let mut out = (stream.len() as i32).to_le_bytes().to_vec();
        out.extend_from_slice(&stream);
        out
    }

    #[test]
    fn something_that_is_not_a_bsg_is_refused() {
        let error = decode_bsg(b"not an image at all", "x.bsg").unwrap_err();
        assert!(error.to_string().contains("BSS-Graphics"), "{error}");
    }

    #[test]
    fn a_stored_bgr_image_decodes_and_flips() {
        // Two rows, one pixel each: the top row is red, the bottom row is blue,
        // and they are stored bottom-up.
        let payload = [255, 0, 0, 0, 0, 255];
        let bytes = build(0, 1, 0, 1, 2, &payload, None);
        let image = decode_bsg(&bytes, "x.bsg").expect("a stored BGR image");
        assert_eq!((image.width, image.height), (1, 2));
        assert_eq!(&image.rgba()[0..4], &[255, 0, 0, 255], "top row is red");
        assert_eq!(&image.rgba()[4..8], &[0, 0, 255, 255], "bottom row is blue");
    }

    #[test]
    fn a_run_length_image_decodes_and_flips() {
        // 2x2 BGRA, stored the way the real files are: one stream per channel
        // covering the whole picture, bottom row first. Each plane below writes
        // the dark bottom row as two literals and the bright top row as one
        // repeat of two, so a decoder that mixed the streams up would produce a
        // picture no colour of which is in this list.
        let plane = |bottom: [u8; 2], top: u8| {
            rle_plane(&[Packet::Literal(bottom.to_vec()), Packet::Repeat(top, 2)])
        };
        let mut payload = Vec::new();
        for plane in [
            plane([10, 11], 200), // blue
            plane([20, 21], 100), // green
            plane([30, 31], 50),  // red
            plane([255, 255], 255),
        ] {
            payload.extend_from_slice(&plane);
        }
        let bytes = build(0, 0, 1, 2, 2, &payload, None);
        let image = decode_bsg(&bytes, "x.bsg").expect("a run length image");
        assert_eq!(
            &image.rgba()[0..4],
            &[50, 100, 200, 255],
            "rows are stored bottom-up, so the bright stored top row comes out first"
        );
        assert_eq!(
            &image.rgba()[8..12],
            &[30, 20, 10, 255],
            "and the stored bottom row comes out last"
        );
    }

    #[test]
    fn an_indexed_image_uses_its_palette() {
        let mut palette = vec![0u8; PALETTE_ENTRIES * PALETTE_ENTRY];
        palette[4..7].copy_from_slice(&[9, 8, 7]); // entry 1: blue 9, green 8, red 7
        let index_plane = rle_plane(&[Packet::Literal(vec![1, 0])]);
        let bytes = build(0, 2, 1, 2, 1, &index_plane, Some(&palette));
        let image = decode_bsg(&bytes, "x.bsg").expect("an indexed image");
        assert_eq!(&image.rgba()[0..4], &[7, 8, 9, 255]);
        assert_eq!(&image.rgba()[4..8], &[0, 0, 0, 255]);
    }

    #[test]
    fn a_composed_image_reads_its_header_from_the_payload() {
        let payload = [1, 2, 3, 255];
        let bytes = build(0x20, 0, 0, 1, 1, &payload, None);
        let image = decode_bsg(&bytes, "x.bsg").expect("a composed image");
        assert_eq!(&image.rgba()[0..4], &[3, 2, 1, 255]);
    }

    #[test]
    fn lz_compression_is_refused_by_name() {
        let bytes = build(0, 0, 2, 1, 1, &[0, 0, 0, 0], None);
        let error = decode_bsg(&bytes, "x.bsg").unwrap_err();
        assert!(
            error.to_string().contains("LZ") && error.to_string().contains("RESEARCH-BSX"),
            "the refusal should name what it cannot do and where the layout is written: {error}"
        );
    }

    #[test]
    fn an_unpacked_size_that_does_not_match_the_pixels_is_refused() {
        // Two by two in mode 0 is sixteen bytes of pixels; say twelve and the
        // file is describing a picture it does not contain.
        let mut bytes = build(0, 0, 0, 2, 2, &[0u8; 16], None);
        bytes[0x12..0x16].copy_from_slice(&12u32.to_le_bytes());
        let error = decode_bsg(&bytes, "x.bsg").unwrap_err();
        assert!(error.to_string().contains("needs 16"), "{error}");
    }

    #[test]
    fn a_run_length_stream_that_runs_off_the_end_is_refused() {
        // One plane claims a long stream but carries a single packet.
        let payload = [4i32.to_le_bytes()[0], 4, 0, 1, 2];
        let bytes = build(0, 0, 1, 2, 2, &payload, None);
        let error = decode_bsg(&bytes, "x.bsg").unwrap_err();
        assert!(
            error.to_string().contains("only") && error.to_string().contains("left"),
            "{error}"
        );
    }

    #[test]
    fn a_truncated_file_is_refused_not_panicked_on() {
        for length in 0..0x60 {
            let bytes = build(0, 0, 1, 2, 2, &[0u8; 8], None);
            let _ = decode_bsg(&bytes[..length.min(bytes.len())], "x.bsg");
        }
    }
}
