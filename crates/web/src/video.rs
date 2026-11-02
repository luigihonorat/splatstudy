// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Video encoding in the browser with WebCodecs, for exporting the
//! timelapse.
//!
//! web-sys's `VideoEncoder` bindings need unstable cfg flags, so a few
//! lines of JavaScript do the encoding, and the core's
//! [`VideoEncoder`] trait polls them. Frames go in as RGBA pixels; the
//! encoder gives H.264 access units ("avc" format: length-prefixed NAL
//! units) and the `avcC` record, which the core muxes into an MP4.

use js_sys::Uint8Array;
use splatstudy_core::export::video::CODEC;
use splatstudy_core::export::{EncodedFrame, Support, VideoConfig, VideoEncoder};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
const message = e => String((e && e.message) || e);

export class SplatstudyVideo {
  constructor(codec) {
    this.codec = codec;
    // 0: still asking, 1: H.264 at 1080p works, 2: no video here.
    this.support = 0;
    this.reset();
    if (typeof VideoEncoder === 'undefined' || typeof VideoFrame === 'undefined') {
      this.support = 2;
      return;
    }
    const probe = { codec, width: 1920, height: 1080, bitrate: 8e6, framerate: 30, avc: { format: 'avc' } };
    VideoEncoder.isConfigSupported(probe).then(
      r => { this.support = r.supported ? 1 : 2; },
      () => { this.support = 2; });
  }
  reset() {
    this.encoder = null;
    this.chunks = [];
    this.current = null;
    this.config = null;
    this.error = '';
    this.done = false;
  }
  start(width, height, fps, bitrate) {
    this.cancel();
    this.reset();
    this.width = width;
    this.height = height;
    this.fps = fps;
    try {
      this.encoder = new VideoEncoder({
        output: (chunk, meta) => {
          const data = new Uint8Array(chunk.byteLength);
          chunk.copyTo(data);
          this.chunks.push({ data, key: chunk.type === 'key' });
          const description = meta && meta.decoderConfig && meta.decoderConfig.description;
          if (description && !this.config) {
            this.config = ArrayBuffer.isView(description)
              ? new Uint8Array(description.buffer, description.byteOffset, description.byteLength).slice()
              : new Uint8Array(description).slice();
          }
        },
        error: e => { this.error = message(e); },
      });
      this.encoder.configure({
        codec: this.codec, width, height, bitrate, framerate: fps,
        latencyMode: 'quality', avc: { format: 'avc' },
      });
      return '';
    } catch (e) {
      return message(e);
    }
  }
  pending() {
    return this.encoder ? this.encoder.encodeQueueSize : 0;
  }
  encode(rgba, index, key) {
    try {
      const step = 1e6 / this.fps;
      const frame = new VideoFrame(rgba, {
        format: 'RGBA', codedWidth: this.width, codedHeight: this.height,
        timestamp: Math.round(index * step), duration: Math.round(step),
      });
      this.encoder.encode(frame, { keyFrame: key });
      frame.close();
      return '';
    } catch (e) {
      return message(e);
    }
  }
  finish() {
    if (!this.encoder) {
      this.done = true;
      return;
    }
    this.encoder.flush().then(() => { this.done = true; }, e => { this.error = message(e); });
  }
  next() {
    this.current = this.chunks.shift() || null;
    return this.current !== null;
  }
  data() { return this.current.data; }
  key() { return this.current.key; }
  cancel() {
    if (this.encoder && this.encoder.state !== 'closed') {
      try { this.encoder.close(); } catch (e) { }
    }
    this.encoder = null;
  }
}
"#)]
extern "C" {
    type SplatstudyVideo;

    #[wasm_bindgen(constructor)]
    fn new(codec: &str) -> SplatstudyVideo;
    #[wasm_bindgen(method, getter)]
    fn support(this: &SplatstudyVideo) -> u32;
    #[wasm_bindgen(method)]
    fn start(this: &SplatstudyVideo, width: u32, height: u32, fps: u32, bitrate: u32) -> String;
    #[wasm_bindgen(method)]
    fn pending(this: &SplatstudyVideo) -> u32;
    #[wasm_bindgen(method)]
    fn encode(this: &SplatstudyVideo, rgba: &[u8], index: f64, key: bool) -> String;
    #[wasm_bindgen(method)]
    fn finish(this: &SplatstudyVideo);
    #[wasm_bindgen(method)]
    fn next(this: &SplatstudyVideo) -> bool;
    #[wasm_bindgen(method)]
    fn data(this: &SplatstudyVideo) -> Uint8Array;
    #[wasm_bindgen(method)]
    fn key(this: &SplatstudyVideo) -> bool;
    #[wasm_bindgen(method, getter)]
    fn config(this: &SplatstudyVideo) -> JsValue;
    #[wasm_bindgen(method, getter)]
    fn done(this: &SplatstudyVideo) -> bool;
    #[wasm_bindgen(method, getter)]
    fn error(this: &SplatstudyVideo) -> String;
    #[wasm_bindgen(method)]
    fn cancel(this: &SplatstudyVideo);
}

/// The browser's H.264 encoder, lent to the core.
pub struct WebVideo {
    js: SplatstudyVideo,
}

impl WebVideo {
    /// Starts asking the browser whether it can encode; the answer comes
    /// through [`VideoEncoder::support`].
    pub fn new() -> Self {
        Self {
            js: SplatstudyVideo::new(CODEC),
        }
    }
}

impl Default for WebVideo {
    fn default() -> Self {
        Self::new()
    }
}

/// An empty string means it worked.
fn result(error: String) -> Result<(), String> {
    if error.is_empty() { Ok(()) } else { Err(error) }
}

impl VideoEncoder for WebVideo {
    fn support(&self) -> Support {
        match self.js.support() {
            0 => Support::Checking,
            1 => Support::Yes,
            _ => Support::No,
        }
    }

    fn start(&mut self, config: &VideoConfig) -> Result<(), String> {
        result(
            self.js
                .start(config.width, config.height, config.fps, config.bitrate),
        )
    }

    fn pending(&self) -> usize {
        self.js.pending() as usize
    }

    fn encode(&mut self, rgba: &[u8], index: u64, key: bool) -> Result<(), String> {
        result(self.js.encode(rgba, index as f64, key))
    }

    fn finish(&mut self) {
        self.js.finish();
    }

    fn take(&mut self) -> Vec<EncodedFrame> {
        let mut frames = Vec::new();
        while self.js.next() {
            frames.push(EncodedFrame {
                data: self.js.data().to_vec(),
                key: self.js.key(),
            });
        }
        frames
    }

    fn config(&self) -> Option<Vec<u8>> {
        let config = self.js.config();
        config.dyn_ref::<Uint8Array>().map(|bytes| bytes.to_vec())
    }

    fn done(&self) -> bool {
        self.js.done()
    }

    fn error(&self) -> Option<String> {
        Some(self.js.error()).filter(|e| !e.is_empty())
    }

    fn cancel(&mut self) {
        self.js.cancel();
    }
}
