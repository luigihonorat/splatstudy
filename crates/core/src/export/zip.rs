// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! A zip archive with stored (uncompressed) files, for handing many images
//! to the browser as one download. PNG images are compressed already.
//!
//! Each file is a local header and the data; at the end come the central
//! directory, one entry per file, and the end record pointing at it. No
//! zip64, and the whole archive is held in memory until the download, so
//! it stops growing at [`BUDGET`].

/// The archive would pass its limit: [`BUDGET`], or 4 GiB, which plain
/// zip can't address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZipFull;

/// The most an archive holds, in bytes. In the browser it lives in the
/// wasm heap (4 GiB at most, often less in a tab) and is copied once more
/// into a Blob for the download, so it stays well under that.
pub const BUDGET: usize = 512 << 20;

/// A generous guess at the size of `frames` PNG frames of `size` pixels,
/// headers included: a byte per pixel. The clay on its black background
/// usually compresses to far less.
pub fn estimate(size: [u32; 2], frames: u64) -> u64 {
    let per_frame = u64::from(size[0]) * u64::from(size[1]) + 30 + 46 + 32;
    per_frame * frames + 22
}

/// Every file gets this date, 1980-01-01 00:00, the earliest zip has; a
/// timelapse's images don't need one.
const DOS_DATE: u16 = (1 << 5) | 1;
const DOS_TIME: u16 = 0;
const VERSION: u16 = 20;
/// File names are UTF-8.
const UTF8: u16 = 1 << 11;

pub struct Zip {
    buf: Vec<u8>,
    central: Vec<u8>,
    count: u16,
    /// The most the finished archive may hold, in bytes.
    limit: usize,
}

impl Default for Zip {
    fn default() -> Self {
        Self::with_limit(BUDGET)
    }
}

impl Zip {
    /// An archive of at most [`BUDGET`] bytes.
    pub fn new() -> Self {
        Self::default()
    }

    /// An archive of at most `limit` bytes (and never past 4 GiB).
    pub fn with_limit(limit: usize) -> Self {
        Self {
            buf: Vec::new(),
            central: Vec::new(),
            count: 0,
            limit: limit.min(u32::MAX as usize),
        }
    }

    /// Stores a file. At most 65,535 files, and the limit in all.
    pub fn add(&mut self, name: &str, data: &[u8]) -> Result<(), ZipFull> {
        let offset = self.buf.len();
        let end = offset + 30 + name.len() + data.len();
        let central_end = self.central.len() + 46 + name.len() + 22;
        let whole = end + central_end;
        if whole > self.limit || self.count == u16::MAX || name.len() > 0xFFFF {
            return Err(ZipFull);
        }
        // Grow by doubling as a Vec does, but never past the limit, and
        // with room for the directory, so `finish` doesn't grow it again:
        // at most the limit plus half of it in memory while it moves.
        if whole > self.buf.capacity() {
            let room = whole.next_power_of_two().min(self.limit).max(whole);
            self.buf.reserve_exact(room - self.buf.len());
        }
        let crc = crc32(data);
        let size = data.len() as u32;
        let le16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
        let le32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());

        let b = &mut self.buf;
        le32(b, 0x0403_4B50);
        for v in [VERSION, UTF8, 0, DOS_TIME, DOS_DATE] {
            le16(b, v);
        }
        for v in [crc, size, size] {
            le32(b, v);
        }
        le16(b, name.len() as u16);
        le16(b, 0);
        b.extend_from_slice(name.as_bytes());
        b.extend_from_slice(data);

        let c = &mut self.central;
        le32(c, 0x0201_4B50);
        for v in [VERSION, VERSION, UTF8, 0, DOS_TIME, DOS_DATE] {
            le16(c, v);
        }
        for v in [crc, size, size] {
            le32(c, v);
        }
        // Name length, extra, comment, disk, internal attributes.
        for v in [name.len() as u16, 0, 0, 0, 0] {
            le16(c, v);
        }
        le32(c, 0); // external attributes
        le32(c, offset as u32);
        c.extend_from_slice(name.as_bytes());
        self.count += 1;
        Ok(())
    }

    /// Bytes so far, for showing progress and keeping under limits.
    pub fn len(&self) -> usize {
        self.buf.len() + self.central.len()
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn finish(mut self) -> Vec<u8> {
        let start = self.buf.len() as u32;
        let size = self.central.len() as u32;
        self.buf.append(&mut self.central);
        let b = &mut self.buf;
        b.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
        for v in [0, 0, self.count, self.count] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&size.to_le_bytes());
        b.extend_from_slice(&start.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        self.buf
    }
}

/// CRC-32 as zip, PNG and gzip use it (reflected, polynomial 0xEDB88320).
pub fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            table[i] = c;
            i += 1;
        }
        table
    };
    !data.iter().fold(!0u32, |c, &b| {
        TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le16(d: &[u8], at: usize) -> usize {
        u16::from_le_bytes([d[at], d[at + 1]]) as usize
    }

    fn le32(d: &[u8], at: usize) -> usize {
        u32::from_le_bytes(d[at..at + 4].try_into().unwrap()) as usize
    }

    #[test]
    fn archives_stop_at_their_limit_without_growing_past_it() {
        let data = vec![7u8; 1000];
        let mut zip = Zip::with_limit(10_000);
        let mut added = 0;
        while zip.add(&format!("f{added}.png"), &data).is_ok() {
            added += 1;
            assert!(zip.buf.capacity() <= 10_000, "{}", zip.buf.capacity());
        }
        // Each file costs 1000 bytes and two headers with its name.
        assert_eq!(added, 9);
        let capacity = zip.buf.capacity();
        let d = zip.finish();
        assert!(d.len() <= 10_000);
        assert_eq!(d.capacity(), capacity, "finishing doesn't grow it");
        assert_eq!(le16(&d, d.len() - 22 + 10), 9);
        // A guess errs on the large side, for frames that are all noise.
        assert!(estimate([1280, 720], 100) > 100 * 1280 * 720);
        assert!(estimate([1280, 720], 5400) > BUDGET as u64);
    }

    #[test]
    fn crc_matches_the_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn files_read_back_through_the_central_directory() {
        let files: [(&str, Vec<u8>); 3] = [
            ("frame_00000.png", vec![1, 2, 3]),
            ("frame_00001.png", (0..=255).collect()),
            ("塑像.txt", Vec::new()),
        ];
        let mut zip = Zip::new();
        for (name, data) in &files {
            zip.add(name, data).unwrap();
        }
        let d = zip.finish();
        // The end record is the last 22 bytes (no comment).
        let end = d.len() - 22;
        assert_eq!(le32(&d, end), 0x0605_4B50);
        assert_eq!(le16(&d, end + 10), files.len());
        let (size, start) = (le32(&d, end + 12), le32(&d, end + 16));
        assert_eq!(
            start + size,
            end,
            "the directory ends where the end record starts"
        );

        let mut at = start;
        for (name, data) in &files {
            assert_eq!(le32(&d, at), 0x0201_4B50);
            let crc = le32(&d, at + 16);
            let stored = le32(&d, at + 20);
            let name_len = le16(&d, at + 28);
            let local = le32(&d, at + 42);
            assert_eq!(&d[at + 46..at + 46 + name_len], name.as_bytes());
            assert_eq!(crc as u32, crc32(data));
            assert_eq!(stored, data.len());
            // The local header agrees and the data follows it.
            assert_eq!(le32(&d, local), 0x0403_4B50);
            assert_eq!(le16(&d, local + 8), 0, "stored");
            assert_eq!(le32(&d, local + 14), crc);
            let body = local + 30 + le16(&d, local + 26) + le16(&d, local + 28);
            assert_eq!(&d[body..body + stored], &data[..]);
            at += 46 + name_len;
        }
        assert_eq!(at, end);
    }
}
