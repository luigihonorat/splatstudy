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
//!     edts > elst
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
    /// `decoderConfig.description`, with its level lowered to the lowest
    /// the video meets (see [`lower_level`]).
    pub fn set_config(&mut self, mut avcc: Vec<u8>) {
        lower_level(&mut avcc, self.width, self.height, self.fps);
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

        // The whole track from its first frame, at normal speed. Without
        // B-frames there is nothing to skip, so this only spells out what
        // players assume anyway; ffmpeg writes the same edit, the layout
        // players see most.
        b.open(b"edts");
        b.full(b"elst", 0, 0);
        b.u32(1).u32(movie_duration as u32).u32(0); // one edit: duration, media time 0
        b.u16(1).u16(0); // rate 1.0
        b.close();
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

/// H.264 levels from 4.0 up (ISO/IEC 14496-10, table A-1): `level_idc`,
/// then the most macroblocks per frame, per second, and in the decoded
/// picture buffer. Lower levels are left out: from 4.0 the bitrate limit
/// (25 Mbit/s for High) is above anything the exporter asks for.
const LEVELS: [(u8, u32, u32, u32); 6] = [
    (40, 8_192, 245_760, 32_768),
    (41, 8_192, 245_760, 32_768),
    (42, 8_704, 522_240, 34_816),
    (50, 22_080, 589_824, 110_400),
    (51, 36_864, 983_040, 184_320),
    (52, 36_864, 2_073_600, 184_320),
];

/// Lowers the level an `avcC` record and its first SPS announce to the
/// lowest the video meets, never raising it. Returns the new level, or
/// `None` when the record is left alone (already low enough, or not
/// understood).
///
/// Chromium's WebCodecs encoder (Edge on Windows, checked 2026-11) marks
/// 1080p30 as level 5.0 even when asked for `avc1.640028` (4.0), though
/// its stream, one reference frame at 1080p30, fits 4.0 easily. Players
/// and hardware decoders that stop at 4.0 or 4.1 may refuse a 5.0 file
/// without trying, so the level is put back where the stream really is.
/// Only the level bytes change; the stream itself is the same.
///
/// The SPS is read to its end, so the frames the stream keeps for
/// decoding (its reference frames, and the decoder buffering its VUI may
/// declare) are checked against the lower level's buffer. An SPS that
/// can't be read through to its last bit is left alone.
pub fn lower_level(avcc: &mut [u8], width: u16, height: u16, fps: u32) -> Option<u8> {
    // avcC: version, profile, compatibility, level, length size, SPS count,
    // then the first SPS's length and bytes.
    if avcc.len() < 8 || avcc[0] != 1 || avcc[5] & 0x1F == 0 {
        return None;
    }
    let sps_len = usize::from(u16::from_be_bytes([avcc[6], avcc[7]]));
    let sps = avcc.get(8..8 + sps_len)?;
    // The SPS: NAL header, profile, constraint flags, level.
    if sps.len() < 4 || sps[0] & 0x1F != 7 || sps[3] != avcc[3] {
        return None;
    }
    let frames = buffered_frames(sps)?;
    let mbs = u32::from(width).div_ceil(16) * u32::from(height).div_ceil(16);
    let (level, ..) = LEVELS
        .iter()
        .copied()
        .find(|&(_, fs, mbps, dpb)| mbs <= fs && mbs * fps <= mbps && mbs * frames <= dpb)?;
    if level >= avcc[3] {
        return None;
    }
    avcc[3] = level;
    avcc[8 + 3] = level;
    Some(level)
}

/// How many frames the decoder must keep, from an SPS NAL unit
/// (7.3.2.1.1): `max_num_ref_frames`, or the VUI's
/// `max_dec_frame_buffering` (E.1.1) when it declares more, at least 1.
/// `None` when the SPS has scaling matrices, is cut short, or doesn't end
/// where its fields do: then it isn't understood well enough to relabel.
fn buffered_frames(sps: &[u8]) -> Option<u32> {
    let mut b = BitReader::new(&sps[1..]);
    let profile = b.bits(8)?;
    b.bits(16)?; // constraint flags, level
    b.ue()?; // seq_parameter_set_id
    if [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135].contains(&profile) {
        if b.ue()? == 3 {
            b.bits(1)?; // separate_colour_plane_flag
        }
        b.ue()?; // bit_depth_luma_minus8
        b.ue()?; // bit_depth_chroma_minus8
        b.bits(1)?; // qpprime_y_zero_transform_bypass_flag
        if b.bits(1)? == 1 {
            return None; // seq_scaling_matrix_present_flag: not worth parsing
        }
    }
    b.ue()?; // log2_max_frame_num_minus4
    match b.ue()? {
        0 => {
            b.ue()?; // log2_max_pic_order_cnt_lsb_minus4
        }
        1 => {
            b.bits(1)?;
            b.ue()?; // offset_for_non_ref_pic (signed; only skipped)
            b.ue()?; // offset_for_top_to_bottom_field
            for _ in 0..b.ue()?.min(255) {
                b.ue()?;
            }
        }
        _ => {}
    }
    let refs = b.ue()?; // max_num_ref_frames
    b.bits(1)?; // gaps_in_frame_num_value_allowed_flag
    b.ue()?; // pic_width_in_mbs_minus1
    b.ue()?; // pic_height_in_map_units_minus1
    if b.bits(1)? == 0 {
        b.bits(1)?; // frame_mbs_only_flag off: mb_adaptive_frame_field_flag
    }
    b.bits(1)?; // direct_8x8_inference_flag
    if b.bits(1)? == 1 {
        for _ in 0..4 {
            b.ue()?; // frame_crop_*_offset
        }
    }
    let mut buffering = 0;
    if b.bits(1)? == 1 {
        buffering = vui_buffering(&mut b)?.unwrap_or(0);
    }
    // rbsp_trailing_bits: a one, then zeros to the end.
    if b.bits(1)? != 1 || !b.rest_is_zero() {
        return None;
    }
    Some(refs.max(buffering).max(1))
}

/// Reads `vui_parameters` (E.1.1) up to `max_dec_frame_buffering`, which
/// it gives if the VUI has a `bitstream_restriction`.
fn vui_buffering(b: &mut BitReader) -> Option<Option<u32>> {
    if b.bits(1)? == 1 && b.bits(8)? == 255 {
        b.bits(32)?; // aspect_ratio_idc Extended_SAR: sar_width, sar_height
    }
    if b.bits(1)? == 1 {
        b.bits(1)?; // overscan_appropriate_flag
    }
    if b.bits(1)? == 1 {
        b.bits(4)?; // video_format, video_full_range_flag
        if b.bits(1)? == 1 {
            b.bits(24)?; // colour_primaries, transfer, matrix_coefficients
        }
    }
    if b.bits(1)? == 1 {
        b.ue()?; // chroma_sample_loc_type_top_field
        b.ue()?; // chroma_sample_loc_type_bottom_field
    }
    if b.bits(1)? == 1 {
        b.bits(32)?; // num_units_in_tick
        b.bits(32)?; // time_scale
        b.bits(1)?; // fixed_frame_rate_flag
    }
    let nal_hrd = b.bits(1)? == 1;
    if nal_hrd {
        hrd_parameters(b)?;
    }
    let vcl_hrd = b.bits(1)? == 1;
    if vcl_hrd {
        hrd_parameters(b)?;
    }
    if nal_hrd || vcl_hrd {
        b.bits(1)?; // low_delay_hrd_flag
    }
    b.bits(1)?; // pic_struct_present_flag
    if b.bits(1)? == 0 {
        return Some(None); // no bitstream_restriction
    }
    b.bits(1)?; // motion_vectors_over_pic_boundaries_flag
    for _ in 0..4 {
        b.ue()?; // max_bytes_per_pic_denom … log2_max_mv_length_vertical
    }
    let reorder = b.ue()?; // max_num_reorder_frames
    let buffering = b.ue()?; // max_dec_frame_buffering
    // Reordering happens inside the buffer: more is not a valid SPS.
    (reorder <= buffering && buffering <= 16).then_some(Some(buffering))
}

/// Skips `hrd_parameters` (E.1.2).
fn hrd_parameters(b: &mut BitReader) -> Option<()> {
    let cpbs = b.ue()?; // cpb_cnt_minus1
    if cpbs > 31 {
        return None;
    }
    b.bits(8)?; // bit_rate_scale, cpb_size_scale
    for _ in 0..=cpbs {
        b.ue()?; // bit_rate_value_minus1
        b.ue()?; // cpb_size_value_minus1
        b.bits(1)?; // cbr_flag
    }
    b.bits(20)?; // four delay and offset lengths
    Some(())
}

/// Reads an H.264 RBSP bit by bit, dropping emulation prevention bytes.
struct BitReader<'a> {
    data: &'a [u8],
    byte: usize,
    bit: u8,
    zeros: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte: 0,
            bit: 0,
            zeros: 0,
        }
    }

    fn bit(&mut self) -> Option<u32> {
        if self.bit == 0 {
            // 00 00 03: the 03 is only there to avoid a start code.
            if self.zeros >= 2 && self.data.get(self.byte) == Some(&3) {
                self.byte += 1;
                self.zeros = 0;
            }
            let byte = *self.data.get(self.byte)?;
            self.zeros = if byte == 0 { self.zeros + 1 } else { 0 };
        }
        let value = (self.data[self.byte] >> (7 - self.bit)) & 1;
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.byte += 1;
        }
        Some(u32::from(value))
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        (0..n).try_fold(0, |v, _| Some((v << 1) | self.bit()?))
    }

    /// Only zero bits are left.
    fn rest_is_zero(&mut self) -> bool {
        while self.bit != 0 {
            if self.bit() != Some(0) {
                return false;
            }
        }
        self.data[self.byte.min(self.data.len())..]
            .iter()
            .all(|&b| b == 0)
    }

    /// An unsigned Exp-Golomb number.
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1u32 << zeros) - 1 + self.bits(zeros)?)
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
        // The track: header, edit list, media.
        let trak = path(&d, &[b"moov", b"trak"]);
        let parts: Vec<[u8; 4]> = children(&d, trak).iter().map(|b| b.kind).collect();
        assert_eq!(parts, [*b"tkhd", *b"edts", *b"mdia"]);
        // One edit: the whole movie (70 frames at 30 fps, in ms), from
        // media time 0, at rate 1.
        let elst = path(&d, &[b"moov", b"trak", b"edts", b"elst"]);
        assert_eq!(be32(&d, elst.start + 4), 1);
        assert_eq!(be32(&d, elst.start + 8), 2333);
        assert_eq!(be32(&d, elst.start + 12), 0);
        assert_eq!(be32(&d, elst.start + 16), 0x0001_0000);
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

    /// The `avcC` Edge's WebCodecs gave for a 1920×1080 30 fps export:
    /// High profile, level 5.0, one reference frame.
    const EDGE_AVCC: &str = "01640032ffe1001967640032ac2b403c0113f2e022000007d00001d4c01e3855\
                             4001000468ee3cb0fdf8f800";

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn the_level_comes_down_to_what_the_stream_needs() {
        let original = unhex(EDGE_AVCC);
        assert_eq!(buffered_frames(&original[8..8 + 25]), Some(1));
        let mut avcc = original.clone();
        assert_eq!(lower_level(&mut avcc, 1920, 1080, 30), Some(40));
        assert_eq!((avcc[3], avcc[8 + 3]), (40, 40), "the record and its SPS");
        // Nothing else changes.
        let changed: Vec<usize> = (0..avcc.len())
            .filter(|&i| avcc[i] != original[i])
            .collect();
        assert_eq!(changed, [3, 11]);
        // Through the muxer too.
        let mut mp4 = Mp4::new(1080, 1920, 30);
        mp4.set_config(original.clone());
        assert_eq!(mp4.config.as_deref().map(|c| c[3]), Some(40));

        // Never raised: 4K needs 5.1, more than the 5.0 announced.
        let mut big = original.clone();
        assert_eq!(lower_level(&mut big, 3840, 2160, 30), None);
        assert_eq!(big, original);
        // 1080p at 60 fps needs 4.2.
        let mut fast = original.clone();
        assert_eq!(lower_level(&mut fast, 1920, 1080, 60), Some(42));
        // Records it doesn't understand are left alone, without panicking.
        for len in 0..original.len() {
            let mut cut = original[..len].to_vec();
            lower_level(&mut cut, 1920, 1080, 30);
        }
        let mut odd = original.clone();
        odd[3] = 41; // the record and the SPS disagree
        assert_eq!(lower_level(&mut odd, 1920, 1080, 30), None);
        let mut fake = vec![1, 0x64, 0, 0x28, 0xFF, 0xE1, 0, 0, 1, 0, 0];
        assert_eq!(lower_level(&mut fake, 1920, 1080, 30), None);
    }

    /// Writes an H.264 RBSP bit by bit.
    #[derive(Default)]
    struct BitWriter {
        bytes: Vec<u8>,
        bits: u32,
    }

    impl BitWriter {
        fn bits(&mut self, n: u32, v: u32) -> &mut Self {
            for i in (0..n).rev() {
                if self.bits.is_multiple_of(8) {
                    self.bytes.push(0);
                }
                *self.bytes.last_mut().unwrap() |= (((v >> i) & 1) as u8) << (7 - self.bits % 8);
                self.bits += 1;
            }
            self
        }

        fn ue(&mut self, v: u32) -> &mut Self {
            let n = 32 - (v + 1).leading_zeros();
            self.bits(n - 1, 0).bits(n, v + 1)
        }

        /// The trailing bits, then the NAL unit with emulation prevention.
        fn nal(mut self) -> Vec<u8> {
            self.bits(1, 1);
            let mut out = vec![0x67];
            let mut zeros = 0;
            for b in self.bytes {
                if zeros >= 2 && b <= 3 {
                    out.push(3);
                    zeros = 0;
                }
                out.push(b);
                zeros = if b == 0 { zeros + 1 } else { 0 };
            }
            out
        }
    }

    /// A 1080p High-profile SPS at level 5.0 with a full VUI: colour,
    /// timing, NAL HRD and a bitstream restriction declaring `buffering`
    /// frames (and as many reordered), or none.
    fn sps(buffering: Option<u32>) -> Vec<u8> {
        let mut w = BitWriter::default();
        w.bits(8, 100).bits(8, 0).bits(8, 50).ue(0); // profile, flags, level, id
        w.ue(1).ue(0).ue(0).bits(1, 0).bits(1, 0); // 4:2:0, 8 bits, no matrices
        w.ue(0).ue(2); // frame_num bits, POC type 2
        w.ue(1).bits(1, 0); // one reference frame, no gaps
        w.ue(119).ue(67).bits(1, 1).bits(1, 1); // 120 × 68 MBs, frames, 8x8
        w.bits(1, 1).ue(0).ue(0).ue(0).ue(4); // cropped to 1080
        w.bits(1, 1); // VUI
        w.bits(1, 1).bits(8, 255).bits(16, 1).bits(16, 1); // square pixels
        w.bits(1, 0); // overscan
        w.bits(1, 1)
            .bits(3, 5)
            .bits(1, 0)
            .bits(1, 1)
            .bits(24, 0x01_01_01);
        w.bits(1, 1).ue(0).ue(0); // chroma location
        w.bits(1, 1).bits(32, 1).bits(32, 60).bits(1, 1); // 30 fps
        w.bits(1, 1).ue(1).bits(4, 0).bits(4, 0); // NAL HRD, two CPBs
        for _ in 0..2 {
            w.ue(2_000_000).ue(4_000_000).bits(1, 0);
        }
        w.bits(5, 23).bits(5, 23).bits(5, 23).bits(5, 24);
        w.bits(1, 0).bits(1, 0).bits(1, 0); // no VCL HRD, low delay, pic_struct
        match buffering {
            Some(n) => {
                w.bits(1, 1)
                    .bits(1, 1)
                    .ue(2)
                    .ue(1)
                    .ue(16)
                    .ue(16)
                    .ue(n)
                    .ue(n);
            }
            None => {
                w.bits(1, 0);
            }
        }
        w.nal()
    }

    /// An avcC record holding `sps`, at the level it announces.
    fn avcc(sps: &[u8]) -> Vec<u8> {
        let mut a = vec![1, sps[1], sps[2], sps[3], 0xFF, 0xE1];
        a.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        a.extend_from_slice(sps);
        a.extend_from_slice(&[1, 0, 4, 0x68, 0xEE, 0x3C, 0xB0]);
        a
    }

    #[test]
    fn the_vui_decoder_buffering_counts_against_the_lower_level() {
        let level = |buffering| lower_level(&mut avcc(&sps(buffering)), 1920, 1080, 30);
        assert_eq!(buffered_frames(&sps(Some(3))), Some(3));
        // 4.0 holds four 1080p frames: up to that, it comes down to 4.0.
        assert_eq!(level(None), Some(40));
        assert_eq!(level(Some(1)), Some(40));
        assert_eq!(level(Some(4)), Some(40));
        // Five need 5.0, which it already says.
        assert_eq!(level(Some(5)), None);
        // At 60 fps, 4.2 holds four too.
        let mut fast = avcc(&sps(Some(4)));
        assert_eq!(lower_level(&mut fast, 1920, 1080, 60), Some(42));

        // Not read to its exact end: left alone.
        let mut longer = sps(Some(1));
        longer.push(0x80);
        assert_eq!(buffered_frames(&longer), None);
        assert_eq!(lower_level(&mut avcc(&longer), 1920, 1080, 30), None);
        let mut shorter = sps(Some(1));
        shorter.pop();
        assert_eq!(lower_level(&mut avcc(&shorter), 1920, 1080, 30), None);
        // A trailing zero byte after the stop bit is still the end.
        let mut padded = sps(Some(1));
        padded.push(0);
        assert_eq!(buffered_frames(&padded), Some(1));
        for len in 1..sps(Some(4)).len() {
            assert_eq!(buffered_frames(&sps(Some(4))[..len]), None, "{len}");
        }
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
