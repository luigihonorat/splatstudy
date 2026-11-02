// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! An MP4 file (ISO base media, ISO/IEC 14496-12 and -15) holding one H.264
//! video track, written from encoded frames.
//!
//! The frames come from an encoder in "avc" form: each is one access unit
//! as length-prefixed NAL units, and the decoder configuration (the
//! `avcC` record with the SPS and PPS) arrives separately. There are no
//! B-frames, so decode order is display order and no `ctts` is needed.
//!
//! Layout: `ftyp`, then `moov` (so players and upload sites can start
//! before the end of the file), then `mdat` with the frames in order,
//! grouped into chunks of about a second.
//!
//! ```text
//! moov
//!   mvhd
//!   trak
//!     tkhd
//!     mdia
//!       mdhd
//!       hdlr
//!       minf
//!         vmhd
//!         dinf > dref > url
//!         stbl
//!           stsd > avc1 > avcC
//!           stts  stss  stsc  stsz  co64
//! ```

/// Movie-level times are in milliseconds.
const MOVIE_TIMESCALE: u32 = 1000;
const TRACK_ID: u32 = 1;

/// Why the frames couldn't be made into a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mp4Error {
    /// The encoder never gave its configuration.
    NoConfig,
    NoFrames,
    /// The first frame must be a key frame, or nothing can be decoded.
    StartsWithoutKey,
}

impl std::fmt::Display for Mp4Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoConfig => "the encoder gave no configuration",
            Self::NoFrames => "no frames were encoded",
            Self::StartsWithoutKey => "the video doesn't start with a key frame",
        })
    }
}

/// Collects encoded frames, then writes the file.
pub struct Mp4 {
    width: u16,
    height: u16,
    fps: u32,
    /// The `avcC` record (AVCDecoderConfigurationRecord).
    config: Option<Vec<u8>>,
    /// Each frame's size and whether it is a key frame.
    samples: Vec<(u32, bool)>,
    /// The frames, back to back, as they will be in `mdat`.
    data: Vec<u8>,
}

impl Mp4 {
    pub fn new(width: u16, height: u16, fps: u32) -> Self {
        Self {
            width,
            height,
            fps: fps.max(1),
            config: None,
            samples: Vec::new(),
            data: Vec::new(),
        }
    }

    /// The decoder configuration, as WebCodecs gives it in
    /// `decoderConfig.description`.
    pub fn set_config(&mut self, avcc: Vec<u8>) {
        self.config = Some(avcc);
    }

    pub fn has_config(&self) -> bool {
        self.config.is_some()
    }

    /// Adds the next frame, in display order.
    pub fn push(&mut self, frame: &[u8], key: bool) {
        self.samples.push((frame.len() as u32, key));
        self.data.extend_from_slice(frame);
    }

    pub fn frames(&self) -> usize {
        self.samples.len()
    }

    /// Media time units per second, and per frame: whole numbers for the
    /// usual rates.
    fn timescale(&self) -> (u32, u32) {
        if 90_000 % self.fps == 0 {
            (90_000, 90_000 / self.fps)
        } else {
            (self.fps * 1000, 1000)
        }
    }

    /// Frames per chunk: about one second's worth.
    fn chunk_len(&self) -> usize {
        self.fps as usize
    }

    /// The whole file.
    pub fn finish(self) -> Result<Vec<u8>, Mp4Error> {
        let config = self.config.as_deref().ok_or(Mp4Error::NoConfig)?;
        match self.samples.first() {
            None => return Err(Mp4Error::NoFrames),
            Some(&(_, false)) => return Err(Mp4Error::StartsWithoutKey),
            Some(_) => {}
        }
        let ftyp = ftyp();
        let big = self.data.len() as u64 + 8 > u64::from(u32::MAX);
        let mdat_header = if big { 16 } else { 8 };
        // The offsets in co64 don't change the size of moov, so measure it
        // with any offset first.
        let moov_len = self.moov(config, 0).len() as u64;
        let first = ftyp.len() as u64 + moov_len + mdat_header;
        let moov = self.moov(config, first);
        debug_assert_eq!(moov.len() as u64, moov_len);

        let mut out = Vec::with_capacity(first as usize + self.data.len());
        out.extend_from_slice(&ftyp);
        out.extend_from_slice(&moov);
        if big {
            out.extend_from_slice(&1u32.to_be_bytes());
            out.extend_from_slice(b"mdat");
            out.extend_from_slice(&(self.data.len() as u64 + 16).to_be_bytes());
        } else {
            out.extend_from_slice(&(self.data.len() as u32 + 8).to_be_bytes());
            out.extend_from_slice(b"mdat");
        }
        out.extend_from_slice(&self.data);
        Ok(out)
    }

    fn moov(&self, config: &[u8], first_offset: u64) -> Vec<u8> {
        let (timescale, delta) = self.timescale();
        let count = self.samples.len() as u32;
        let media_duration = u64::from(count) * u64::from(delta);
        let movie_duration = media_duration * u64::from(MOVIE_TIMESCALE) / u64::from(timescale);
        let (w, h) = (u32::from(self.width), u32::from(self.height));

        let mut b = Boxes::default();
        b.open(b"moov");
        b.full(b"mvhd", 0, 0);
        b.u32(0).u32(0); // created, modified
        b.u32(MOVIE_TIMESCALE).u32(movie_duration as u32);
        b.u32(0x0001_0000).u16(0x0100).u16(0); // rate 1.0, volume 1.0, reserved
        b.u32(0).u32(0);
        b.matrix();
        b.zeros(24); // pre_defined
        b.u32(TRACK_ID + 1); // next track
        b.close();

        b.open(b"trak");
        b.full(b"tkhd", 0, 0x3); // enabled, in the movie
        b.u32(0).u32(0).u32(TRACK_ID).u32(0);
        b.u32(movie_duration as u32);
        b.u32(0).u32(0);
        b.u16(0).u16(0).u16(0).u16(0); // layer, group, volume, reserved
        b.matrix();
        b.u32(w << 16).u32(h << 16);
        b.close();

        b.open(b"mdia");
        b.full(b"mdhd", 0, 0);
        b.u32(0).u32(0).u32(timescale).u32(media_duration as u32);
        b.u16(0x55C4).u16(0); // language "und", pre_defined
        b.close();
        b.full(b"hdlr", 0, 0);
        b.u32(0).raw(b"vide").zeros(12).raw(b"VideoHandler\0");
        b.close();

        b.open(b"minf");
        b.full(b"vmhd", 0, 1);
        b.u16(0).u16(0).u16(0).u16(0); // graphics mode, colour
        b.close();
        b.open(b"dinf");
        b.full(b"dref", 0, 0);
        b.u32(1);
        b.full(b"url ", 0, 1); // the data is in this file
        b.close();
        b.close();
        b.close();

        b.open(b"stbl");
        b.full(b"stsd", 0, 0);
        b.u32(1);
        b.open(b"avc1");
        b.zeros(6).u16(1); // reserved, data reference 1
        b.zeros(16); // pre_defined, reserved
        b.u16(self.width).u16(self.height);
        b.u32(0x0048_0000).u32(0x0048_0000); // 72 dpi
        b.u32(0).u16(1); // reserved, one frame per sample
        let mut name = [0u8; 32];
        let label = b"SplatStudy";
        name[0] = label.len() as u8;
        name[1..=label.len()].copy_from_slice(label);
        b.raw(&name);
        b.u16(0x0018).u16(0xFFFF); // depth 24, pre_defined -1
        b.open(b"avcC");
        b.raw(config);
        b.close();
        b.close();
        b.close();

        b.full(b"stts", 0, 0);
        b.u32(1).u32(count).u32(delta);
        b.close();

        let keys: Vec<u32> = (1..=count)
            .filter(|&n| self.samples[n as usize - 1].1)
            .collect();
        b.full(b"stss", 0, 0);
        b.u32(keys.len() as u32);
        for k in keys {
            b.u32(k);
        }
        b.close();

        let per_chunk = self.chunk_len();
        let chunks = self.samples.len().div_ceil(per_chunk);
        let rest = self.samples.len() - (chunks - 1) * per_chunk;
        b.full(b"stsc", 0, 0);
        // First chunk, frames per chunk, sample description.
        if rest == per_chunk || chunks == 1 {
            b.u32(1).u32(1).u32(rest as u32).u32(1);
        } else {
            b.u32(2);
            b.u32(1).u32(per_chunk as u32).u32(1);
            b.u32(chunks as u32).u32(rest as u32).u32(1);
        }
        b.close();

        b.full(b"stsz", 0, 0);
        b.u32(0).u32(count); // sizes vary, count
        for &(size, _) in &self.samples {
            b.u32(size);
        }
        b.close();

        b.full(b"co64", 0, 0);
        b.u32(chunks as u32);
        let mut offset = first_offset;
        for chunk in self.samples.chunks(per_chunk) {
            b.u64(offset);
            offset += chunk.iter().map(|&(size, _)| u64::from(size)).sum::<u64>();
        }
        b.close();

        b.close(); // stbl
        b.close(); // minf
        b.close(); // mdia
        b.close(); // trak
        b.close(); // moov
        b.finish()
    }
}

fn ftyp() -> Vec<u8> {
    let mut b = Boxes::default();
    b.open(b"ftyp");
    b.raw(b"isom").u32(0x200);
    for brand in [b"isom", b"iso2", b"avc1", b"mp41"] {
        b.raw(brand);
    }
    b.close();
    b.finish()
}

/// Big-endian box writing: `open` leaves room for the size, `close`
/// fills it in.
#[derive(Default)]
struct Boxes {
    buf: Vec<u8>,
    open: Vec<usize>,
}

impl Boxes {
    fn open(&mut self, kind: &[u8; 4]) {
        self.open.push(self.buf.len());
        self.buf.extend_from_slice(&[0; 4]);
        self.buf.extend_from_slice(kind);
    }

    /// A "full box": version and flags after the header.
    fn full(&mut self, kind: &[u8; 4], version: u8, flags: u32) {
        self.open(kind);
        self.u32(u32::from(version) << 24 | flags);
    }

    fn close(&mut self) {
        let start = self.open.pop().expect("a box is open");
        let size = (self.buf.len() - start) as u32;
        self.buf[start..start + 4].copy_from_slice(&size.to_be_bytes());
    }

    fn u16(&mut self, v: u16) -> &mut Self {
        self.raw(&v.to_be_bytes())
    }

    fn u32(&mut self, v: u32) -> &mut Self {
        self.raw(&v.to_be_bytes())
    }

    fn u64(&mut self, v: u64) -> &mut Self {
        self.raw(&v.to_be_bytes())
    }

    fn raw(&mut self, bytes: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(bytes);
        self
    }

    fn zeros(&mut self, n: usize) -> &mut Self {
        self.buf.resize(self.buf.len() + n, 0);
        self
    }

    /// The identity transform, in 16.16 and 2.30 fixed point.
    fn matrix(&mut self) {
        for v in [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000u32] {
            self.u32(v);
        }
    }

    fn finish(self) -> Vec<u8> {
        debug_assert!(self.open.is_empty(), "every box is closed");
        self.buf
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A box found while walking a file: its type, where its content
    /// starts and ends.
    #[derive(Debug)]
    struct Found {
        kind: [u8; 4],
        body: std::ops::Range<usize>,
    }

    fn be32(d: &[u8], at: usize) -> u32 {
        u32::from_be_bytes(d[at..at + 4].try_into().unwrap())
    }

    fn be64(d: &[u8], at: usize) -> u64 {
        u64::from_be_bytes(d[at..at + 8].try_into().unwrap())
    }

    /// The boxes directly inside `range`, checking that they tile it.
    fn children(d: &[u8], range: std::ops::Range<usize>) -> Vec<Found> {
        let mut at = range.start;
        let mut out = Vec::new();
        while at < range.end {
            let mut size = be32(d, at) as usize;
            let kind: [u8; 4] = d[at + 4..at + 8].try_into().unwrap();
            let mut header = 8;
            if size == 1 {
                size = be64(d, at + 8) as usize;
                header = 16;
            }
            assert!(
                size >= header && at + size <= range.end,
                "box {kind:?} overruns"
            );
            out.push(Found {
                kind,
                body: at + header..at + size,
            });
            at += size;
        }
        assert_eq!(at, range.end, "boxes tile their parent");
        out
    }

    fn find<'a>(boxes: &'a [Found], kind: &[u8; 4]) -> &'a Found {
        boxes
            .iter()
            .find(|b| &b.kind == kind)
            .unwrap_or_else(|| panic!("no {}", String::from_utf8_lossy(kind)))
    }

    /// Follows a path of boxes from the top of the file.
    fn path(d: &[u8], kinds: &[&[u8; 4]]) -> std::ops::Range<usize> {
        let mut range = 0..d.len();
        for kind in kinds {
            let inside = children(d, range.clone());
            range = find(&inside, kind).body.clone();
        }
        range
    }

    /// Where each frame is in the file, worked out from the sample tables
    /// the way a player does.
    pub(crate) fn frame_ranges(d: &[u8]) -> Vec<std::ops::Range<usize>> {
        let stbl = [b"moov", b"trak", b"mdia", b"minf", b"stbl"];
        let table = |kind: &[u8; 4]| {
            let mut p: Vec<&[u8; 4]> = stbl.to_vec();
            p.push(kind);
            path(d, &p)
        };
        let stsz = table(b"stsz");
        let count = be32(d, stsz.start + 8) as usize;
        let sizes: Vec<usize> = (0..count)
            .map(|i| be32(d, stsz.start + 12 + 4 * i) as usize)
            .collect();
        let co64 = table(b"co64");
        let chunks = be32(d, co64.start + 4) as usize;
        let offsets: Vec<usize> = (0..chunks)
            .map(|i| be64(d, co64.start + 8 + 8 * i) as usize)
            .collect();
        let stsc = table(b"stsc");
        let runs: Vec<(usize, usize)> = (0..be32(d, stsc.start + 4) as usize)
            .map(|i| {
                let at = stsc.start + 8 + 12 * i;
                (be32(d, at) as usize, be32(d, at + 4) as usize)
            })
            .collect();
        let mut ranges = Vec::new();
        let mut sample = 0;
        for (c, &offset) in offsets.iter().enumerate() {
            let chunk_number = c + 1;
            let per_chunk = runs
                .iter()
                .rev()
                .find(|(first, _)| *first <= chunk_number)
                .unwrap()
                .1;
            let mut at = offset;
            for _ in 0..per_chunk {
                ranges.push(at..at + sizes[sample]);
                at += sizes[sample];
                sample += 1;
            }
        }
        assert_eq!(sample, count, "the chunks hold every frame");
        ranges
    }

    /// Frames of different sizes, each filled with its own number.
    fn frames(n: usize) -> Vec<Vec<u8>> {
        (0..n).map(|i| vec![i as u8; 50 + (i * 37) % 200]).collect()
    }

    fn movie(n: usize, fps: u32) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut mp4 = Mp4::new(1920, 1080, fps);
        mp4.set_config(vec![1, 0x64, 0, 0x28, 0xFF, 0xE1, 0, 0, 1, 0, 0]);
        let frames = frames(n);
        for (i, f) in frames.iter().enumerate() {
            mp4.push(f, i % (2 * fps as usize) == 0);
        }
        (mp4.finish().expect("muxes"), frames)
    }

    #[test]
    fn boxes_tile_the_file_and_moov_comes_first() {
        let (d, frames) = movie(70, 30);
        let top = children(&d, 0..d.len());
        let kinds: Vec<&[u8; 4]> = top.iter().map(|b| &b.kind).collect();
        assert_eq!(kinds, [b"ftyp", b"moov", b"mdat"]);
        let total: usize = frames.iter().map(Vec::len).sum();
        assert_eq!(top[2].body.len(), total, "mdat holds exactly the frames");
        // Every box down to the sample tables tiles its parent.
        let stbl = path(&d, &[b"moov", b"trak", b"mdia", b"minf", b"stbl"]);
        let tables: Vec<[u8; 4]> = children(&d, stbl).iter().map(|b| b.kind).collect();
        assert_eq!(
            tables,
            [*b"stsd", *b"stts", *b"stss", *b"stsc", *b"stsz", *b"co64"]
        );
        let avc1 = path(&d, &[b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd"]);
        // stsd: version/flags and a count, then avc1 (78 bytes of fields) and avcC.
        let entry = children(&d, avc1.start + 8..avc1.end);
        assert_eq!(&entry[0].kind, b"avc1");
        let inner = children(&d, entry[0].body.start + 78..entry[0].body.end);
        assert_eq!(&inner[0].kind, b"avcC");
        assert_eq!(inner[0].body.len(), 11);
        assert_eq!(be32(&d, entry[0].body.start + 24) >> 16, 1920);
    }

    #[test]
    fn sample_tables_point_at_every_frame() {
        // 70 frames at 30 fps: chunks of 30, 30 and 10. Also one short
        // chunk alone, and chunks that come out even.
        for (n, fps) in [(70, 30), (7, 30), (60, 30), (1, 15), (45, 15)] {
            let (d, frames) = movie(n, fps);
            let ranges = frame_ranges(&d);
            assert_eq!(ranges.len(), n);
            for (range, frame) in ranges.iter().zip(&frames) {
                assert_eq!(&d[range.clone()], &frame[..], "{n} frames at {fps}");
            }
        }
    }

    #[test]
    fn timing_and_key_frames() {
        let (d, _) = movie(70, 30);
        let stbl = [b"moov", b"trak", b"mdia", b"minf", b"stbl"];
        let mut p = stbl.to_vec();
        p.push(b"stts");
        let stts = path(&d, &p);
        assert_eq!(be32(&d, stts.start + 4), 1);
        assert_eq!(be32(&d, stts.start + 8), 70);
        assert_eq!(be32(&d, stts.start + 12), 3000, "90 kHz / 30");
        let mut p = stbl.to_vec();
        p.push(b"stss");
        let stss = path(&d, &p);
        // Key frames every 2 s: frames 1, 61.
        assert_eq!(be32(&d, stss.start + 4), 2);
        assert_eq!(be32(&d, stss.start + 8), 1);
        assert_eq!(be32(&d, stss.start + 12), 61);
        let mdhd = path(&d, &[b"moov", b"trak", b"mdia", b"mdhd"]);
        assert_eq!(be32(&d, mdhd.start + 12), 90_000);
        assert_eq!(be32(&d, mdhd.start + 16), 70 * 3000);
        let mvhd = path(&d, &[b"moov", b"mvhd"]);
        assert_eq!(be32(&d, mvhd.start + 16), 2333, "milliseconds");
    }

    #[test]
    fn incomplete_videos_are_refused() {
        let mut mp4 = Mp4::new(16, 16, 30);
        mp4.push(&[1, 2, 3], true);
        assert_eq!(mp4.finish(), Err(Mp4Error::NoConfig));
        let mut mp4 = Mp4::new(16, 16, 30);
        mp4.set_config(vec![1]);
        assert_eq!(mp4.finish(), Err(Mp4Error::NoFrames));
        let mut mp4 = Mp4::new(16, 16, 30);
        mp4.set_config(vec![1]);
        mp4.push(&[1], false);
        assert_eq!(mp4.finish(), Err(Mp4Error::StartsWithoutKey));
    }

    /// Muxes a raw H.264 stream into an MP4, for checking the muxer with
    /// real players and ffprobe:
    ///
    /// ```text
    /// ffmpeg -f lavfi -i testsrc=size=640x360:rate=30 -t 3 -c:v libx264 -bf 0 -g 60 in.h264
    /// SPLATSTUDY_H264=in.h264 SPLATSTUDY_MP4=out.mp4 cargo test -p splatstudy-core mux_a_real_stream -- --ignored
    /// ffprobe -show_streams -count_frames out.mp4
    /// ```
    #[test]
    #[ignore = "needs an H.264 file; see the comment"]
    fn mux_a_real_stream() {
        let input = std::env::var("SPLATSTUDY_H264").expect("SPLATSTUDY_H264");
        let output = std::env::var("SPLATSTUDY_MP4").expect("SPLATSTUDY_MP4");
        let size: Vec<u16> = std::env::var("SPLATSTUDY_SIZE")
            .unwrap_or("640x360".into())
            .split('x')
            .map(|v| v.parse().unwrap())
            .collect();
        let stream = std::fs::read(input).unwrap();
        // Split the Annex B stream at its start codes.
        let mut nals: Vec<&[u8]> = Vec::new();
        let mut i = 0;
        let mut start = None;
        while i + 3 <= stream.len() {
            if stream[i..i + 3] == [0, 0, 1] {
                if let Some(s) = start {
                    let mut end = i;
                    while end > s && stream[end - 1] == 0 {
                        end -= 1;
                    }
                    nals.push(&stream[s..end]);
                }
                i += 3;
                start = Some(i);
            } else {
                i += 1;
            }
        }
        nals.push(&stream[start.unwrap()..]);
        let (mut sps, mut pps) = (None, None);
        let mut mp4 = Mp4::new(size[0], size[1], 30);
        let mut unit = Vec::new();
        for nal in nals {
            match nal[0] & 0x1F {
                7 => sps = Some(nal.to_vec()),
                8 => pps = Some(nal.to_vec()),
                kind => {
                    unit.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                    unit.extend_from_slice(nal);
                    // A slice ends the access unit (one slice per frame).
                    if kind == 1 || kind == 5 {
                        mp4.push(&unit, kind == 5);
                        unit.clear();
                    }
                }
            }
        }
        let (sps, pps) = (sps.unwrap(), pps.unwrap());
        let mut avcc = vec![1, sps[1], sps[2], sps[3], 0xFF, 0xE1];
        avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        avcc.extend_from_slice(&sps);
        avcc.push(1);
        avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        avcc.extend_from_slice(&pps);
        mp4.set_config(avcc);
        let frames = mp4.frames();
        std::fs::write(&output, mp4.finish().unwrap()).unwrap();
        eprintln!("{frames} frames to {output}");
    }
}
