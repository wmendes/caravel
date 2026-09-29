//! Strict little-endian reader and writer (spec §0.2, §8.1 INV-D4, INV-D6).
//!
//! Every Caravel format is fixed-width little-endian with explicit length
//! prefixes. Decoders are strict: exact lengths, no trailing bytes, enums only
//! accept listed values and booleans only accept 0 or 1.

use alloc::vec::Vec;

/// Why a byte string is not a valid encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The input ended before a field.
    UnexpectedEnd,
    /// Bytes remain after a complete object.
    TrailingBytes,
    /// The 8-byte magic is not the expected one.
    BadMagic,
    /// The version field is not a supported version.
    BadVersion,
    /// A byte that is not one of the listed enum values.
    BadEnum,
    /// A boolean byte other than 0 or 1.
    BadBool,
    /// Reserved flag bits are set.
    BadFlags,
    /// A length field does not match the exact encoded length of its payload.
    BadLength,
    /// Two fields that must agree do not (for example a receipt's status and code).
    Inconsistent,
}

/// Why a value cannot be encoded: a collection is longer than its count field allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncodeError;

/// Reads fields from a byte slice, front to back.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Bytes read so far.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Bytes left to read.
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Takes the next `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::UnexpectedEnd)?;
        let out = self
            .buf
            .get(self.pos..end)
            .ok_or(DecodeError::UnexpectedEnd)?;
        self.pos = end;
        Ok(out)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    pub fn i64(&mut self) -> Result<i64, DecodeError> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    /// 16-byte two's complement little-endian (spec §0.2).
    pub fn i128(&mut self) -> Result<i128, DecodeError> {
        Ok(i128::from_le_bytes(self.array()?))
    }

    /// A boolean byte: exactly 0 or 1.
    pub fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError::BadBool),
        }
    }

    /// Checks an 8-byte ASCII magic.
    pub fn magic(&mut self, expected: &[u8; 8]) -> Result<(), DecodeError> {
        if &self.array::<8>()? == expected {
            Ok(())
        } else {
            Err(DecodeError::BadMagic)
        }
    }

    /// Ends decoding: any unread byte is an error.
    pub fn finish(self) -> Result<(), DecodeError> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes)
        }
    }
}

/// Appends fields to a byte buffer.
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(n: usize) -> Self {
        Self {
            buf: Vec::with_capacity(n),
        }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn i128(&mut self, v: i128) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }

    /// Writes a collection count, failing if it does not fit the count field.
    pub fn count_u8(&mut self, n: usize) -> Result<(), EncodeError> {
        self.u8(u8::try_from(n).map_err(|_| EncodeError)?);
        Ok(())
    }

    pub fn count_u16(&mut self, n: usize) -> Result<(), EncodeError> {
        self.u16(u16::try_from(n).map_err(|_| EncodeError)?);
        Ok(())
    }

    pub fn count_u32(&mut self, n: usize) -> Result<(), EncodeError> {
        self.u32(u32::try_from(n).map_err(|_| EncodeError)?);
        Ok(())
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_are_little_endian() {
        let mut w = Writer::new();
        w.u16(0x0102);
        w.u32(0x0304_0506);
        w.i128(-2);
        let bytes = w.into_vec();
        assert_eq!(&bytes[..6], &[0x02, 0x01, 0x06, 0x05, 0x04, 0x03]);
        assert_eq!(bytes[6], 0xFE);
        assert!(bytes[7..22].iter().all(|b| *b == 0xFF));
        let mut r = Reader::new(&bytes);
        assert_eq!(r.u16(), Ok(0x0102));
        assert_eq!(r.u32(), Ok(0x0304_0506));
        assert_eq!(r.i128(), Ok(-2));
        assert_eq!(r.finish(), Ok(()));
    }

    #[test]
    fn strict_bool_and_trailing_bytes() {
        assert_eq!(Reader::new(&[2]).bool(), Err(DecodeError::BadBool));
        let mut r = Reader::new(&[1, 0]);
        assert_eq!(r.bool(), Ok(true));
        assert_eq!(r.finish(), Err(DecodeError::TrailingBytes));
        assert_eq!(
            Reader::new(&[1, 2, 3]).u32(),
            Err(DecodeError::UnexpectedEnd)
        );
    }

    #[test]
    fn take_never_panics_on_huge_lengths() {
        let mut r = Reader::new(&[0; 4]);
        assert_eq!(r.take(usize::MAX), Err(DecodeError::UnexpectedEnd));
        assert_eq!(r.position(), 0);
    }

    #[test]
    fn counts_must_fit() {
        let mut w = Writer::new();
        assert_eq!(w.count_u8(256), Err(EncodeError));
        assert_eq!(w.count_u16(65_535), Ok(()));
    }
}
