//! PNG encoder — lossless export for everything the body can decode.
//!
//! Zero dependencies means writing the one PNG flavor we need by hand:
//! 8-bit RGBA, filter 0, and zlib streams made only of *stored* (uncompressed)
//! deflate blocks. Larger files, byte-perfect pixels, no compressor in the
//! body.

use std::sync::OnceLock;

use crate::asset::{Image, ImageFormat};
use crate::error::{Error, Result};

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// Encode `image` as an 8-bit RGBA PNG.
pub fn encode_png(image: &Image) -> Result<Vec<u8>> {
    if image.format != ImageFormat::Rgba8 {
        return Err(Error::unsupported(
            "PNG export",
            format!("pixel format {:?} is not RGBA8", image.format),
        ));
    }
    let width = image.width as usize;
    let height = image.height as usize;
    let stride = width * 4;

    // Raw scanlines: one filter byte (0 = None) before each row.
    let mut raw = Vec::with_capacity(height * (stride + 1));
    for row in 0..height {
        raw.push(0u8);
        raw.extend_from_slice(&image.data[row * stride..(row + 1) * stride]);
    }

    let mut out = Vec::with_capacity(raw.len() + 128);
    out.extend_from_slice(PNG_SIGNATURE);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&image.width.to_be_bytes());
    ihdr.extend_from_slice(&image.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // depth 8, RGBA, deflate, adaptive, no interlace
    write_chunk(&mut out, b"IHDR", &ihdr);

    let mut idat = Vec::with_capacity(raw.len() + raw.len() / 65_535 * 5 + 6);
    idat.extend_from_slice(&[0x78, 0x01]); // zlib: deflate, no preset dict, fastest
    let mut offset = 0usize;
    while offset < raw.len() {
        let len = (raw.len() - offset).min(65_535);
        let is_last = offset + len == raw.len();
        idat.push(u8::from(is_last)); // BFINAL, BTYPE = 00 (stored)
        idat.extend_from_slice(&(len as u16).to_le_bytes());
        idat.extend_from_slice(&(!(len as u16)).to_le_bytes());
        idat.extend_from_slice(&raw[offset..offset + len]);
        offset += len;
    }
    if raw.is_empty() {
        // A zlib stream needs at least one block even when there is nothing.
        idat.push(1);
        idat.extend_from_slice(&0u16.to_le_bytes());
        idat.extend_from_slice(&u16::MAX.to_le_bytes());
    }
    idat.extend_from_slice(&adler32(&raw).to_be_bytes());
    write_chunk(&mut out, b"IDAT", &idat);

    write_chunk(&mut out, b"IEND", b"");
    Ok(out)
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn crc_table() -> &'static [u32; 256] {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        table
    })
}

fn crc32(data: &[u8]) -> u32 {
    let table = crc_table();
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc = table[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + byte as u32) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::Image;

    fn sample() -> Image {
        let mut image = Image::new(3, 2);
        for (i, px) in image.data.chunks_exact_mut(4).enumerate() {
            px[0] = i as u8 * 10;
            px[1] = 255 - i as u8 * 10;
            px[2] = (i * 37 % 256) as u8;
            px[3] = 255;
        }
        image
    }

    #[test]
    fn png_structure_is_valid() {
        let png = encode_png(&sample()).unwrap();
        assert_eq!(&png[..8], PNG_SIGNATURE);
        // IHDR length must be 13.
        assert_eq!(&png[8..12], &13u32.to_be_bytes());
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..20], &3u32.to_be_bytes());
        assert_eq!(&png[20..24], &2u32.to_be_bytes());
        assert_eq!(&png[24..29], &[8, 6, 0, 0, 0]);
    }

    #[test]
    fn zlib_stream_round_trips_through_stored_blocks() {
        let png = encode_png(&sample()).unwrap();
        // Signature(8) + IHDR chunk (4 length + 4 type + 13 data + 4 CRC).
        let idat_start = 8 + 4 + 4 + 13 + 4;
        assert_eq!(&png[idat_start + 4..idat_start + 8], b"IDAT");
        let idat_len =
            u32::from_be_bytes(png[idat_start..idat_start + 4].try_into().unwrap()) as usize;
        let stream = &png[idat_start + 8..idat_start + 8 + idat_len];

        assert_eq!(&stream[..2], &[0x78, 0x01]);
        let mut body = &stream[2..stream.len() - 4];
        let mut inflated: Vec<u8> = Vec::new();
        loop {
            let header = body[0];
            let btype = (header >> 1) & 0b11;
            assert_eq!(btype, 0, "only stored blocks are expected");
            let len = u16::from_le_bytes([body[1], body[2]]) as usize;
            let nlen = u16::from_le_bytes([body[3], body[4]]);
            assert_eq!(!(len as u16), nlen);
            inflated.extend_from_slice(&body[5..5 + len]);
            body = &body[5 + len..];
            if header & 1 == 1 {
                break;
            }
        }
        assert!(body.is_empty());
        // Scanlines: 2 rows * (1 filter byte + 3*4 pixel bytes).
        assert_eq!(inflated.len(), 2 * (1 + 12));
        assert_eq!(inflated[0], 0, "filter byte must be 0");
        // First pixel matches the sample.
        assert_eq!(&inflated[1..5], &[0, 255, 0, 255]);
    }

    #[test]
    fn crc_and_adler_agree_with_reference_values() {
        // Classic check values: CRC32("123456789") = 0xCBF43926,
        // Adler32("Wikipedia") = 0x11E60398.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }
}
