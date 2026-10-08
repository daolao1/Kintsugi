//! Minimal BMP decoder: the dialect old Japanese engines actually used.
//!
//! Supports `BM` files with `BITMAPINFOHEADER`-or-larger headers, BI_RGB
//! (uncompressed), 8-bit paletted / 24-bit / 32-bit, bottom-up and top-down
//! rows. That covers the BMP payloads inside BlueGale's ZBM/BBM images and
//! most sibling engines of that era; anything else fails loudly instead of
//! guessing.

use crate::asset::{Image, ImageFormat};
use crate::bytes::Reader;
use crate::error::{Error, Result};

/// Refuse images larger than this many pixels (about a 256 MB RGBA buffer).
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;

fn unsupported(detail: impl Into<String>) -> Error {
    Error::unsupported("BMP", detail)
}

fn corrupt(detail: impl Into<String>) -> Error {
    Error::corrupt("BMP", detail)
}

/// Decode an uncompressed BMP into an RGBA8 [`Image`].
///
/// 32-bit BI_RGB images get alpha forced to 255: the engines of that era
/// used the fourth byte as padding and rendered opaque, so opaque is the
/// faithful repair.
pub fn decode_bmp(data: &[u8]) -> Result<Image> {
    let mut reader = Reader::new(data);
    if reader.remaining() < 14 || reader.take(2)? != b"BM" {
        return Err(unsupported("missing 'BM' signature"));
    }
    let data_offset = {
        let mut r = Reader::new(data);
        r.seek(10)?;
        r.u32le()? as usize
    };

    // The info header starts after the 14-byte file header.
    reader.seek(14)?;
    let header_size = reader.u32le()? as usize;
    if header_size < 40 {
        return Err(unsupported(format!(
            "header size {header_size} (BITMAPCOREHEADER) is not supported"
        )));
    }
    if 14 + header_size > data.len() {
        return Err(corrupt("info header larger than file"));
    }
    let width = reader.i32le()?;
    let raw_height = reader.i32le()?;
    let planes = reader.u16le()?;
    let bits_per_pixel = reader.u16le()?;
    let compression = reader.u32le()?;

    let top_down = raw_height < 0;
    let width = width.unsigned_abs();
    let height = raw_height.unsigned_abs();
    if width == 0 || height == 0 {
        return Err(corrupt(format!("degenerate size {width}x{height}")));
    }
    if planes != 1 {
        return Err(corrupt(format!("planes = {planes}, expected 1")));
    }
    if compression != 0 {
        return Err(unsupported(format!(
            "compression method {compression} (only BI_RGB is supported)"
        )));
    }
    if !matches!(bits_per_pixel, 8 | 24 | 32) {
        return Err(unsupported(format!(
            "{bits_per_pixel} bits per pixel (only 8/24/32 are supported)"
        )));
    }
    if (width as u64) * (height as u64) > MAX_PIXELS {
        return Err(unsupported(format!(
            "{width}x{height} exceeds the {MAX_PIXELS}-pixel limit"
        )));
    }

    // biClrUsed lives at absolute offset 46 in BITMAPINFOHEADER files.
    let colors_used = {
        let mut r = Reader::new(data);
        r.seek(46)?;
        r.u32le()? as usize
    };

    let palette = read_palette(
        data,
        14 + header_size,
        bits_per_pixel,
        colors_used,
        data_offset,
    )?;

    let bytes_per_pixel = bits_per_pixel as usize / 8;
    let source_stride = width as usize * bytes_per_pixel;
    let row_stride = (source_stride + 3) & !3usize;

    let mut image = Image::new(width, height);
    for row in 0..height as usize {
        // BMP rows are stored bottom-up unless height was negative.
        let source_row = if top_down {
            row
        } else {
            height as usize - 1 - row
        };
        let row_start = data_offset
            .checked_add(
                source_row
                    .checked_mul(row_stride)
                    .ok_or_else(|| corrupt("row offset overflow"))?,
            )
            .ok_or_else(|| corrupt("row offset overflow"))?;
        let row_bytes = data
            .get(row_start..row_start + source_stride)
            .ok_or_else(|| corrupt("pixel rows truncated"))?;

        let dst_row = row * width as usize * 4;
        for column in 0..width as usize {
            let (r, g, b, a) = match bits_per_pixel {
                8 => {
                    let index = row_bytes[column] as usize;
                    let Some([pr, pg, pb]) = palette.get(index) else {
                        return Err(corrupt(format!(
                            "palette index {index} out of {} entries",
                            palette.len()
                        )));
                    };
                    (*pr, *pg, *pb, 255)
                }
                24 => {
                    let p = column * 3;
                    (row_bytes[p + 2], row_bytes[p + 1], row_bytes[p], 255)
                }
                _ => {
                    let p = column * 4;
                    (row_bytes[p + 2], row_bytes[p + 1], row_bytes[p], 255)
                }
            };
            let dst = dst_row + column * 4;
            image.data[dst] = r;
            image.data[dst + 1] = g;
            image.data[dst + 2] = b;
            image.data[dst + 3] = a;
        }
    }
    image.format = ImageFormat::Rgba8;
    Ok(image)
}

fn read_palette(
    data: &[u8],
    offset: usize,
    bits_per_pixel: u16,
    colors_used: usize,
    data_offset: usize,
) -> Result<Vec<[u8; 3]>> {
    if bits_per_pixel != 8 {
        return Ok(Vec::new());
    }
    if offset > data_offset || data_offset > data.len() {
        return Err(corrupt(format!(
            "palette at {offset} does not fit before pixel data at {data_offset}"
        )));
    }
    // `biClrUsed == 0` means "256 entries" on paper, but the engines of this
    // era routinely wrote only the entries that fit before the pixel data.
    // The palette therefore ends where the pixels begin, whichever is less.
    let available = (data_offset - offset) / 4;
    let declared = match colors_used {
        0 => 256,
        n if n > 256 => {
            return Err(corrupt(format!("palette claims {n} entries")));
        }
        n => n,
    };
    let count = declared.min(available);
    let mut reader = Reader::new(data);
    reader.seek(offset)?;
    let mut palette = Vec::with_capacity(count);
    for _ in 0..count {
        let b = reader.u8()?;
        let g = reader.u8()?;
        let r = reader.u8()?;
        let _reserved = reader.u8()?;
        palette.push([r, g, b]);
    }
    Ok(palette)
}
