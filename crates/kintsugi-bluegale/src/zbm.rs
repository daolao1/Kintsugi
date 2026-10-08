//! ZBM: BlueGale's packed bitmap (`amp_` magic).
//!
//! Layout (verified against GARbro's `ArcFormats/BlueGale/ImageZBM.cs`,
//! morkt, MIT):
//!
//! ```text
//! offset size field
//!      0    4 magic "amp_"
//!      4    2 version, must be 1
//!      6    4 unpacked size of the payload
//!     10    4 offset of the packed payload
//! ```
//!
//! The payload is an LZ stream — MSB-first bits, one discarded leading
//! bit, then a loop of: an 8-bit token; if the token's high bit is set, a
//! 10-bit back-reference offset and `token & 0x7F` copied bytes (copies may
//! overlap); otherwise `token` literal bytes. The result is a standard BMP,
//! except some files have their first 100 bytes XOR-ed with `0xFF` (and
//! then every one of those starts with `BD B2`, i.e. `BM` obfuscated).

use kintsugi_core::asset::Image;
use kintsugi_core::bytes::MsbBitReader;
use kintsugi_core::codec::bmp::decode_bmp;
use kintsugi_core::error::{Error, Result};

/// ZBM files start with these four bytes.
pub const ZBM_MAGIC: &[u8; 4] = b"amp_";
/// Number of leading bytes the XOR obfuscation touches, at most.
pub const ZBM_XOR_PREFIX: usize = 100;

/// Whether `data` starts like a ZBM image.
pub fn is_zbm(data: &[u8]) -> bool {
    data.len() >= 14 && &data[0..4] == ZBM_MAGIC
}

/// Refuse absurd unpacked payloads (about 256 MB of bitmap).
pub const MAX_UNPACKED_SIZE: usize = 256 * 1024 * 1024;

/// Decode a ZBM image into RGBA8.
pub fn decode_zbm(data: &[u8]) -> Result<Image> {
    let mut reader = kintsugi_core::bytes::Reader::new(data);
    if reader.take(4)? != ZBM_MAGIC {
        return Err(Error::unsupported("ZBM", "missing 'amp_' signature"));
    }
    let version = reader.i16le()?;
    if version != 1 {
        return Err(Error::unsupported(
            "ZBM",
            format!("version {version} (only 1 is known)"),
        ));
    }
    let unpacked_size = reader.u32le()? as usize;
    let data_offset = reader.u32le()? as usize;
    if unpacked_size < 0x36 {
        return Err(Error::corrupt(
            "ZBM",
            format!("unpacked size {unpacked_size} is smaller than a BMP header"),
        ));
    }
    if unpacked_size > MAX_UNPACKED_SIZE {
        return Err(Error::unsupported(
            "ZBM",
            format!("unpacked size {unpacked_size} exceeds the {MAX_UNPACKED_SIZE}-byte limit"),
        ));
    }
    if data_offset < 14 || data_offset > data.len() {
        return Err(Error::corrupt(
            "ZBM",
            format!("data offset {data_offset} outside the file"),
        ));
    }

    let mut raw = lz_unpack(&data[data_offset..], unpacked_size)?;
    deobfuscate(&mut raw);
    decode_bmp(&raw)
}

/// Undo the first-100-bytes XOR when the obfuscated `BM` signature is seen.
pub fn deobfuscate(data: &mut [u8]) {
    if data.len() >= 2 && data[0] == b'B' ^ 0xFF && data[1] == b'M' ^ 0xFF {
        let end = data.len().min(ZBM_XOR_PREFIX);
        for byte in &mut data[..end] {
            *byte ^= 0xFF;
        }
    }
}

/// Unpack BlueGale's LZ stream.
///
/// `input` starts at the packed payload. Returns exactly `out_len` bytes
/// when the stream carries that much (streams *may* stop early — the
/// reference decoder breaks out then, and so do we, returning a short
/// buffer only when the caller tolerates it; here we error instead, since
/// every real payload declares its exact size).
pub fn lz_unpack(input: &[u8], out_len: usize) -> Result<Vec<u8>> {
    let mut bits = MsbBitReader::new(input);
    // One discarded leading bit, as the format demands.
    bits.bit()?;
    let mut out = Vec::with_capacity(out_len.min(1 << 22));
    while out.len() < out_len {
        // Reading past the end while asking for a token count ends the
        // stream — the reference decoder breaks its loop there.
        let Ok(count) = bits.bits(8) else { break };
        if count > 0x7F {
            let offset = bits.bits(10)? as usize;
            let copy_len = ((count & 0x7F) as usize).min(out_len - out.len());
            if offset == 0 || offset > out.len() {
                return Err(Error::corrupt(
                    "ZBM LZ stream",
                    format!(
                        "back-reference offset {offset} at output position {}",
                        out.len()
                    ),
                ));
            }
            let mut src = out.len() - offset;
            let end = src + copy_len;
            while src < end {
                // Byte-by-byte on purpose: run-length copies overlap
                // themselves, so each new byte feeds the next read.
                let byte = out[src];
                out.push(byte);
                src += 1;
            }
        } else if count == 0 {
            break;
        } else {
            for _ in 0..count {
                if out.len() >= out_len {
                    break;
                }
                out.push(bits.bits(8)? as u8);
            }
        }
    }
    if out.len() != out_len {
        return Err(Error::corrupt(
            "ZBM LZ stream",
            format!(
                "produced {} bytes but the header promised {out_len}",
                out.len()
            ),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_check() {
        assert!(is_zbm(b"amp_\x01\0\0\0\0\0\0\0\0\0"));
        assert!(!is_zbm(b"BMxx"));
    }

    #[test]
    fn rejects_unknown_version() {
        let mut data = ZBM_MAGIC.to_vec();
        data.extend_from_slice(&2i16.to_le_bytes());
        data.extend_from_slice(&0x100u32.to_le_bytes());
        data.extend_from_slice(&14u32.to_le_bytes());
        assert!(decode_zbm(&data).is_err());
    }

    #[test]
    fn deobfuscate_only_when_marked() {
        let mut marked = vec![b'B' ^ 0xFF, b'M' ^ 0xFF, 1, 2, 3];
        deobfuscate(&mut marked);
        assert_eq!(marked, vec![b'B', b'M', 0xFE, 0xFD, 0xFC]);

        let mut plain = vec![b'B', b'M', 1, 2, 3];
        deobfuscate(&mut plain);
        assert_eq!(plain, vec![b'B', b'M', 1, 2, 3]);
    }

    /// Hand-crafts a bit stream: 1 dummy bit, literal "AB", then a
    /// back-reference of 2 bytes at offset 2 → "ABAB".
    #[test]
    fn lz_unpack_copies_overlap() {
        let mut bits: Vec<bool> = Vec::new();
        let push = |bits: &mut Vec<bool>, value: u32, n: u32| {
            for i in (0..n).rev() {
                bits.push((value >> i) & 1 == 1);
            }
        };
        push(&mut bits, 0, 1); // dummy bit
        push(&mut bits, 2, 8); // literal token: 2 bytes
        push(&mut bits, b'A' as u32, 8);
        push(&mut bits, b'B' as u32, 8);
        push(&mut bits, 0x82, 8); // back-reference token: 2 bytes
        push(&mut bits, 2, 10); // at offset 2

        let mut packed = Vec::new();
        let mut acc = 0u8;
        let mut n = 0;
        for bit in bits {
            acc = (acc << 1) | u8::from(bit);
            n += 1;
            if n == 8 {
                packed.push(acc);
                acc = 0;
                n = 0;
            }
        }
        if n > 0 {
            packed.push(acc << (8 - n));
        }

        let out = lz_unpack(&packed, 4).unwrap();
        assert_eq!(out, b"ABAB");
    }

    #[test]
    fn lz_unpack_rejects_bad_offset() {
        // Dummy bit, then token 0x81: a one-byte back-reference whose
        // offset points one byte back — but nothing has been written yet.
        let bits: [bool; 9] = [
            false, // dummy
            true, false, false, false, false, false, false, true, // token 0x81
        ];
        let mut packed = vec![0u8];
        for (i, bit) in bits.iter().take(8).enumerate() {
            if *bit {
                packed[0] |= 1 << (7 - i);
            }
        }
        packed.push(0); // offset, high bits
        packed.push(0b0100_0000); // offset, low bits -> offset 1
        assert!(lz_unpack(&packed, 1).is_err());
    }
}
