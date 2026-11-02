// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Reference files kept apart from the scene, by their content.
//!
//! An autosave is written every few seconds, but the reference in it (often
//! 20 to 200 MB) rarely changes. So the autosave's scene names the file by
//! a [`BlobKey`] in a `REFH` chunk, and the shell stores the bytes once
//! under that key.

use crate::io::{ReadError, Reader, Writer};

/// Names a file by its content: a hash of the bytes and their length.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlobKey {
    pub hash: u64,
    pub len: u64,
}

impl BlobKey {
    /// The key of `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        Self {
            hash: content_hash(bytes),
            len: bytes.len() as u64,
        }
    }

    /// The key as text, for stores keyed by strings: hash and length in
    /// hex, `"1f…c0-4a3b00"`.
    pub fn name(&self) -> String {
        format!("{:016x}-{:x}", self.hash, self.len)
    }

    /// True if `bytes` are what this key names (same length, same hash).
    pub fn matches(&self, bytes: &[u8]) -> bool {
        bytes.len() as u64 == self.len && content_hash(bytes) == self.hash
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        w.u64(self.hash).u64(self.len);
    }

    pub(crate) fn read(r: &mut Reader) -> Result<Self, ReadError> {
        Ok(Self {
            hash: r.u64()?,
            len: r.u64()?,
        })
    }
}

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// An odd multiplier with its bits spread out (2^64 / φ). FNV's own prime
/// has few bits set, which mixes too slowly when a whole word goes in at
/// once.
const MULTIPLIER: u64 = 0x9e37_79b9_7f4a_7c15;

/// A 64-bit hash of `bytes`, for telling files apart, not for security.
///
/// FNV-1a, but taking eight bytes per step, each step folded with a shift
/// so the word's high bits reach the low ones too. Eight bytes per multiply
/// keeps a 200 MB splat to tens of milliseconds, even in the browser.
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut h = OFFSET ^ bytes.len() as u64;
    let (words, tail) = bytes.as_chunks::<8>();
    for word in words {
        h = (h ^ u64::from_le_bytes(*word)).wrapping_mul(MULTIPLIER);
        h ^= h >> 29;
    }
    for &b in tail {
        h = (h ^ u64::from(b)).wrapping_mul(MULTIPLIER);
        h ^= h >> 29;
    }
    // A final mix, so a change in the last word still moves every bit.
    h = (h ^ (h >> 32)).wrapping_mul(MULTIPLIER);
    h ^ (h >> 29)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn the_same_bytes_give_the_same_key() {
        let bytes: Vec<u8> = (0..10_000u32).map(|i| (i * 7 + i / 13) as u8).collect();
        assert_eq!(BlobKey::of(&bytes), BlobKey::of(&bytes.clone()));
        assert!(BlobKey::of(&bytes).matches(&bytes));
        assert!(!BlobKey::of(&bytes).matches(&bytes[1..]));
    }

    #[test]
    fn any_one_bit_flipped_changes_the_hash() {
        // Every bit of a short file, including the tail past the last whole
        // word, and the top bits of each word.
        let bytes: Vec<u8> = (0..37u8).map(|i| i.wrapping_mul(31)).collect();
        let base = content_hash(&bytes);
        let mut seen = HashSet::from([base]);
        for at in 0..bytes.len() {
            for bit in 0..8 {
                let mut changed = bytes.clone();
                changed[at] ^= 1 << bit;
                assert!(seen.insert(content_hash(&changed)), "byte {at} bit {bit}");
            }
        }
    }

    #[test]
    fn length_counts_even_for_zeros() {
        let hashes: HashSet<u64> = (0..64).map(|n| content_hash(&vec![0; n])).collect();
        assert_eq!(hashes.len(), 64);
    }

    #[test]
    fn keys_read_back_and_name_themselves() {
        let key = BlobKey {
            hash: 0x0123_4567_89ab_cdef,
            len: 0x4a3b00,
        };
        let mut w = Writer::new();
        key.write(&mut w);
        let data = w.finish();
        assert_eq!(BlobKey::read(&mut Reader::new(&data)), Ok(key));
        assert!(BlobKey::read(&mut Reader::new(&data[..15])).is_err());
        assert_eq!(key.name(), "0123456789abcdef-4a3b00");
    }
}
