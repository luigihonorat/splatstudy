// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Little-endian binary reading and writing, for the file formats. Reading
//! never panics: short or malformed input gives a [`ReadError`].

use std::fmt;

use glam::Vec3;

/// Why reading stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// The data ended before the value at this offset.
    End { offset: usize },
    /// The data at this offset isn't what the format allows.
    Invalid { offset: usize, what: &'static str },
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::End { offset } => write!(f, "file ends early (at byte {offset})"),
            Self::Invalid { offset, what } => write!(f, "{what} (at byte {offset})"),
        }
    }
}

impl std::error::Error for ReadError {}

/// Reads values from the front of a byte slice.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

macro_rules! read_number {
    ($($name:ident: $t:ty),*) => {$(
        pub fn $name(&mut self) -> Result<$t, ReadError> {
            let bytes = self.take(size_of::<$t>())?;
            Ok(<$t>::from_le_bytes(bytes.try_into().expect("sized")))
        }
    )*};
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Bytes read so far.
    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// The next `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], ReadError> {
        if n > self.remaining() {
            return Err(ReadError::End { offset: self.pos });
        }
        let bytes = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(bytes)
    }

    pub fn skip(&mut self, n: usize) -> Result<(), ReadError> {
        self.take(n).map(drop)
    }

    /// An error about the data just before the current position.
    pub fn invalid(&self, what: &'static str) -> ReadError {
        ReadError::Invalid {
            offset: self.pos,
            what,
        }
    }

    read_number!(u8: u8, u16: u16, u32: u32, u64: u64, i32: i32, f32: f32, f64: f64);

    pub fn vec3(&mut self) -> Result<Vec3, ReadError> {
        Ok(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }

    /// A length as written by [`Writer::len`], checked against the bytes
    /// left so a corrupt length can't ask for a huge allocation: each item
    /// takes at least `item_size` bytes.
    pub fn len(&mut self, item_size: usize) -> Result<usize, ReadError> {
        let start = self.pos;
        let n = self.u64()?;
        let fits = usize::try_from(n)
            .ok()
            .filter(|&n| n.saturating_mul(item_size.max(1)) <= self.remaining());
        fits.ok_or(ReadError::Invalid {
            offset: start,
            what: "length longer than the file",
        })
    }

    /// A string as written by [`Writer::str`].
    pub fn str(&mut self) -> Result<String, ReadError> {
        let n = self.len(1)?;
        let start = self.pos;
        let bytes = self.take(n)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ReadError::Invalid {
            offset: start,
            what: "text that isn't UTF-8",
        })
    }

    /// Bytes as written by [`Writer::bytes`].
    pub fn bytes(&mut self) -> Result<&'a [u8], ReadError> {
        let n = self.len(1)?;
        self.take(n)
    }
}

/// Appends values to a growing buffer.
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

macro_rules! write_number {
    ($($name:ident: $t:ty),*) => {$(
        pub fn $name(&mut self, v: $t) -> &mut Self {
            self.buf.extend_from_slice(&v.to_le_bytes());
            self
        }
    )*};
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    write_number!(u8: u8, u16: u16, u32: u32, u64: u64, i32: i32, f32: f32, f64: f64);

    pub fn vec3(&mut self, v: Vec3) -> &mut Self {
        self.f32(v.x).f32(v.y).f32(v.z)
    }

    /// Each vector as [`Writer::vec3`] writes it, in one copy: a clay of a
    /// million vertices is written in milliseconds, not a tenth of a second.
    pub fn vec3s(&mut self, vs: &[Vec3]) -> &mut Self {
        if cfg!(target_endian = "little") {
            self.raw(bytemuck::cast_slice(vs))
        } else {
            vs.iter().fold(self, |w, &v| w.vec3(v))
        }
    }

    /// Each number as [`Writer::u32`] writes it, in one copy.
    pub fn u32s(&mut self, vs: &[u32]) -> &mut Self {
        if cfg!(target_endian = "little") {
            self.raw(bytemuck::cast_slice(vs))
        } else {
            vs.iter().fold(self, |w, &v| w.u32(v))
        }
    }

    /// Makes room for `n` more bytes at once.
    pub fn reserve(&mut self, n: usize) -> &mut Self {
        self.buf.reserve(n);
        self
    }

    /// Overwrites the eight bytes at `at` with `v`, as [`Writer::u64`]
    /// wrote them: for a length known only once what follows is written.
    pub fn set_u64(&mut self, at: usize, v: u64) -> &mut Self {
        self.buf[at..at + 8].copy_from_slice(&v.to_le_bytes());
        self
    }

    /// A count of items that follow, read back by [`Reader::len`].
    pub fn len(&mut self, n: usize) -> &mut Self {
        self.u64(n as u64)
    }

    /// Length-prefixed UTF-8.
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }

    /// Length-prefixed bytes.
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.len(b.len()).raw(b)
    }

    /// Bytes as they are, without a length.
    pub fn raw(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }

    /// Bytes written so far.
    pub fn position(&self) -> usize {
        self.buf.len()
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip() {
        let mut w = Writer::new();
        w.u8(7)
            .u32(0xDEAD_BEEF)
            .f64(-1.5)
            .vec3(Vec3::new(1.0, 2.0, 3.0))
            .str("塑像 sculpt")
            .bytes(&[1, 2, 3]);
        let data = w.finish();
        let mut r = Reader::new(&data);
        assert_eq!(r.u8(), Ok(7));
        assert_eq!(r.u32(), Ok(0xDEAD_BEEF));
        assert_eq!(r.f64(), Ok(-1.5));
        assert_eq!(r.vec3(), Ok(Vec3::new(1.0, 2.0, 3.0)));
        assert_eq!(r.str().as_deref(), Ok("塑像 sculpt"));
        assert_eq!(r.bytes(), Ok(&[1u8, 2, 3][..]));
        assert!(r.is_empty());
    }

    #[test]
    fn many_values_write_as_one_at_a_time() {
        let vs = [
            Vec3::new(1.0, -2.5, f32::MIN_POSITIVE),
            Vec3::NAN,
            Vec3::ZERO,
        ];
        let us = [0, 7, u32::MAX, 0x0102_0304];
        let mut each = Writer::new();
        vs.iter().for_each(|&v| {
            each.vec3(v);
        });
        us.iter().for_each(|&u| {
            each.u32(u);
        });
        each.u64(9);
        let mut many = Writer::new();
        many.reserve(64).vec3s(&vs).u32s(&us).u64(0).set_u64(52, 9);
        assert_eq!(each.finish(), many.finish());
    }

    #[test]
    fn short_or_corrupt_input_is_an_error_not_a_panic() {
        let mut w = Writer::new();
        w.str("hello").u32(5);
        let data = w.finish();
        // Every truncation fails cleanly somewhere.
        for end in 0..data.len() {
            let mut r = Reader::new(&data[..end]);
            assert!(r.str().and_then(|_| r.u32()).is_err(), "cut at {end}");
        }
        // A huge length is refused before allocating.
        let mut w = Writer::new();
        w.u64(u64::MAX).raw(b"abc");
        assert!(matches!(
            Reader::new(&w.finish()).str(),
            Err(ReadError::Invalid { .. })
        ));
        // Bad UTF-8.
        let mut w = Writer::new();
        w.bytes(&[0xFF, 0xFE]);
        assert!(Reader::new(&w.finish()).str().is_err());
    }
}
