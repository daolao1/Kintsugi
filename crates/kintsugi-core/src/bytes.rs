//! Little-endian byte reader and an MSB-first bit reader.
//!
//! Old Japanese game formats are little-endian on the byte level and, for
//! their LZ streams, almost always big-endian (MSB-first) on the bit level.
//! These two readers cover both halves of that convention.

use crate::error::{Error, Result};

fn truncated(context: &str) -> Error {
    Error::corrupt(context, "unexpected end of data")
}

/// A little-endian cursor over a byte slice.
#[derive(Debug, Clone, Copy)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Start reading at the beginning of `data`.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Current byte offset.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Bytes left unread.
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    /// Jump to an absolute offset.
    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.data.len() {
            return Err(truncated("reader"));
        }
        self.pos = pos;
        Ok(())
    }

    /// Read one byte.
    pub fn u8(&mut self) -> Result<u8> {
        let b = self
            .data
            .get(self.pos)
            .copied()
            .ok_or_else(|| truncated("reader"))?;
        self.pos += 1;
        Ok(b)
    }

    /// Read a little-endian `u16`.
    pub fn u16le(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    /// Read a little-endian `i16`.
    pub fn i16le(&mut self) -> Result<i16> {
        Ok(self.u16le()? as i16)
    }

    /// Read a little-endian `u32`.
    pub fn u32le(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Read a little-endian `i32`.
    pub fn i32le(&mut self) -> Result<i32> {
        Ok(self.u32le()? as i32)
    }

    /// Borrow the next `n` bytes and advance.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| truncated("reader"))?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| truncated("reader"))?;
        self.pos = end;
        Ok(slice)
    }
}

/// A big-endian (MSB-first) bit reader, the kind old Japanese LZ streams use.
///
/// Bits are consumed from the most significant bit of each byte towards the
/// least significant, then from the next byte. Reads never straddle more
/// than 24 bits, matching the classic bit-reader implementations the formats
/// were reverse-engineered from.
#[derive(Debug, Clone)]
pub struct MsbBitReader<'a> {
    data: &'a [u8],
    byte: usize,
    bit: u32,
}

impl<'a> MsbBitReader<'a> {
    /// Start reading at the beginning of `data`.
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte: 0,
            bit: 0,
        }
    }

    fn next_bit(&mut self) -> Result<bool> {
        let b = *self
            .data
            .get(self.byte)
            .ok_or_else(|| truncated("bit stream"))?;
        let on = (b >> (7 - self.bit)) & 1 == 1;
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.byte += 1;
        }
        Ok(on)
    }

    /// Read `n` bits (1..=24), MSB first. The first bit read is the most
    /// significant bit of the returned value.
    pub fn bits(&mut self, n: u32) -> Result<u32> {
        debug_assert!((1..=24).contains(&n), "bit reads are capped at 24 bits");
        let mut acc = 0u32;
        for _ in 0..n {
            let on = self.next_bit()?;
            acc = (acc << 1) | u32::from(on);
        }
        Ok(acc)
    }

    /// Read a single bit.
    pub fn bit(&mut self) -> Result<bool> {
        self.next_bit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian() {
        let data = [0xEF, 0xBE, 0xAD, 0xDE, 0x00];
        let mut r = Reader::new(&data);
        assert_eq!(r.u8().unwrap(), 0xEF);
        // Reads continue where the last one stopped: BE AD.
        assert_eq!(r.u16le().unwrap(), 0xADBE);
        let mut wide = Reader::new(&data);
        assert_eq!(wide.u32le().unwrap(), 0xDEAD_BEEF);
    }

    #[test]
    fn take_and_seek() {
        let data = [1, 2, 3, 4];
        let mut r = Reader::new(&data);
        assert_eq!(r.take(2).unwrap(), &[1, 2]);
        assert_eq!(r.position(), 2);
        r.seek(0).unwrap();
        assert_eq!(r.take(4).unwrap(), &[1, 2, 3, 4]);
        assert!(r.take(1).is_err());
        assert!(r.seek(9).is_err());
    }

    #[test]
    fn bit_reader_is_msb_first() {
        // 0b10110000 → bits 1,0,1,1 then 0,0,0,0
        let data = [0b1011_0000];
        let mut r = MsbBitReader::new(&data);
        assert!(r.bit().unwrap());
        assert!(!r.bit().unwrap());
        assert!(r.bit().unwrap());
        assert!(r.bit().unwrap());
        // next four bits: 0000 → value 0
        assert_eq!(r.bits(4).unwrap(), 0);
        assert!(r.bit().is_err());
    }

    #[test]
    fn bit_reader_spans_bytes() {
        // bits(4) over 0b11001111 gives 0b1100, then bits(4) gives 0b1111
        let data = [0b1100_1111];
        let mut r = MsbBitReader::new(&data);
        assert_eq!(r.bits(4).unwrap(), 0b1100);
        assert_eq!(r.bits(4).unwrap(), 0b1111);
    }

    #[test]
    fn bit_reader_multi_byte_value() {
        let data = [0b0000_0010, 0b0100_0001]; // 8-bit "2", 8-bit "A" (0x41)
        let mut r = MsbBitReader::new(&data);
        assert_eq!(r.bits(8).unwrap(), 2);
        assert_eq!(r.bits(8).unwrap(), 0x41);
    }
}
