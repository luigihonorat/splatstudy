// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The video encoder a shell lends the core: frames go in as pixels, H.264
//! comes out, and the core writes the MP4 itself ([`super::mp4`]).
//!
//! The browser has one (WebCodecs); desktop has none for now and exports
//! images instead. Encoders work in the background, so everything here
//! returns at once and the exporter polls.

/// The codec every encoder is asked for: H.264 High profile, level 4.0,
/// enough for 1920×1080 (or 1080×1920) at 30 frames per second.
pub const CODEC: &str = "avc1.640028";

/// What to encode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Bits per second.
    pub bitrate: u32,
}

impl VideoConfig {
    /// A bitrate that keeps clay and text crisp: about 8 Mbit/s at 1080p 30.
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        let bits = u64::from(width) * u64::from(height) * u64::from(fps) * 13 / 100;
        Self {
            width,
            height,
            fps,
            bitrate: bits.clamp(1_000_000, 20_000_000) as u32,
        }
    }
}

/// Whether video can be made here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// Still finding out (the browser answers asynchronously).
    Checking,
    Yes,
    No,
}

/// One encoded frame: an access unit as length-prefixed NAL units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub key: bool,
}

/// Encodes frames to H.264, in the background.
pub trait VideoEncoder {
    /// Whether [`CODEC`] can be encoded at 1080p here.
    fn support(&self) -> Support;

    /// Starts a new video, dropping any earlier one.
    fn start(&mut self, config: &VideoConfig) -> Result<(), String>;

    /// Frames handed over and not yet encoded; the exporter waits while
    /// there are a few, so frames don't pile up in memory.
    fn pending(&self) -> usize;

    /// Encodes the frame numbered `index` (from 0): RGBA8 rows, top first,
    /// the configured width and height.
    fn encode(&mut self, rgba: &[u8], index: u64, key: bool) -> Result<(), String>;

    /// No more frames: encodes what is pending, then [`done`](Self::done)
    /// turns true.
    fn finish(&mut self);

    /// The frames encoded since the last call, in order.
    fn take(&mut self) -> Vec<EncodedFrame>;

    /// The decoder configuration (`avcC`), once the first frame is encoded.
    fn config(&self) -> Option<Vec<u8>>;

    /// True once [`finish`](Self::finish) has encoded everything.
    fn done(&self) -> bool;

    /// What went wrong, if the encoder gave up.
    fn error(&self) -> Option<String>;

    /// Drops the video under way.
    fn cancel(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrate_scales_with_the_frame() {
        assert_eq!(VideoConfig::new(1920, 1080, 30).bitrate, 8_087_040);
        let small = VideoConfig::new(64, 64, 15);
        assert_eq!(small.bitrate, 1_000_000, "a floor for tiny frames");
    }
}
