//! BBM: BlueGale's obfuscated bitmap.
//!
//! A standard BMP whose first 100 bytes are XOR-ed with `0xFF`. Detection
//! keys on the obfuscated values: `0xB2BD` where `BM` should be, and
//! `0xFFFFFFFF` in the reserved field — then the deobfuscated header must
//! look like a `BITMAPINFOHEADER` bitmap (size `0x28`).
//!
//! Verified against GARbro's `ArcFormats/BlueGale/ImageBBM.cs` (morkt, MIT).

use kintsugi_core::asset::Image;
use kintsugi_core::codec::bmp::decode_bmp;
use kintsugi_core::error::{Error, Result};

/// Obfuscated `BM`: `0x42 ^ 0xFF` followed by `0x4D ^ 0xFF`.
pub const BBM_MAGIC: [u8; 2] = [b'B' ^ 0xFF, b'M' ^ 0xFF];
/// Bytes the obfuscation touches, at most.
pub const BBM_XOR_PREFIX: usize = 100;

/// Whether `data` starts like a BBM image.
pub fn is_bbm(data: &[u8]) -> bool {
    if data.len() < 0x20 {
        return false;
    }
    if data[0..2] != BBM_MAGIC {
        return false;
    }
    if reader_u32le(data, 6) != 0xFFFF_FFFF {
        return false;
    }
    // The deobfuscated header-size field (offset 0x0E) must be 0x28.
    reader_u32le(data, 0x0E) ^ 0xFFFF_FFFF == 0x28
}

fn reader_u32le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

/// Decode a BBM image into RGBA8.
pub fn decode_bbm(data: &[u8]) -> Result<Image> {
    if !is_bbm(data) {
        return Err(Error::unsupported(
            "BBM",
            "signature mismatch (expected obfuscated 'BM' + reserved 0xFFFFFFFF)",
        ));
    }
    let mut plain = data.to_vec();
    let end = plain.len().min(BBM_XOR_PREFIX);
    for byte in &mut plain[..end] {
        *byte ^= 0xFF;
    }
    decode_bmp(&plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_requires_all_three_marks() {
        let mut data = vec![0u8; 0x20];
        data[0] = BBM_MAGIC[0];
        data[1] = BBM_MAGIC[1];
        data[6..10].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        // header size field, still obfuscated: 0x28 ^ 0xFFFFFFFF
        data[0x0E..0x12].copy_from_slice(&(0x28u32 ^ 0xFFFF_FFFF).to_le_bytes());
        assert!(is_bbm(&data));

        data[6] = 0;
        assert!(!is_bbm(&data));
    }
}
