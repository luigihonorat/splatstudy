// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Reading a view's pixels back from the GPU, for tests, comparing
//! silhouettes and exporting video.
//!
//! Record the copy with [`Readback::copy`] before submitting, call
//! [`Readback::map`] after the submit, then call [`Readback::try_take`] on
//! later frames until the pixels are there. The browser maps the buffer by
//! itself; on desktop `try_take` polls the device. Desktop code and tests
//! can block with [`Readback::wait`] instead.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use super::ViewTarget;

const PENDING: u8 = 0;
const READY: u8 = 1;
const FAILED: u8 = 2;

/// The GPU could not map the buffer (the device was lost, for example).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadbackFailed;

/// A copy of one view target's resolved colour on its way to the CPU.
pub struct Readback {
    buffer: wgpu::Buffer,
    size: [u32; 2],
    /// Bytes per row in the buffer: rows are padded to the copy alignment.
    padded_row: u32,
    state: Arc<AtomicU8>,
}

impl Readback {
    /// Records a copy of `target`'s pixels into `encoder`, after the passes
    /// that draw them.
    pub fn copy(
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &ViewTarget,
    ) -> Self {
        Self::copy_texture(device, encoder, &target.resolved)
    }

    /// Records a copy of a whole RGBA8 `texture` (one sample, made with
    /// `COPY_SRC`) into `encoder`.
    pub fn copy_texture(
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
    ) -> Self {
        let size = [texture.width(), texture.height()];
        let padded_row = (size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(padded_row) * u64::from(size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
        Self {
            buffer,
            size,
            padded_row,
            state: Arc::new(AtomicU8::new(PENDING)),
        }
    }

    /// Asks for the buffer to be mapped. Call once, after the submit that
    /// holds the copy.
    pub fn map(&self) {
        let state = Arc::clone(&self.state);
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let done = if result.is_ok() { READY } else { FAILED };
                state.store(done, Ordering::Release);
            });
    }

    /// The width and height of the pixels.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// The pixels as RGBA8 rows, top row first, once the GPU has them;
    /// `None` while still waiting.
    pub fn try_take(&self, device: &wgpu::Device) -> Result<Option<Vec<u8>>, ReadbackFailed> {
        // Lets desktop backends run the map callback; the browser runs it
        // from its own event loop and ignores this.
        let _ = device.poll(wgpu::PollType::Poll);
        match self.state.load(Ordering::Acquire) {
            PENDING => Ok(None),
            READY => self.take().map(Some),
            _ => Err(ReadbackFailed),
        }
    }

    /// Blocks until the pixels are there. Not on the web, which can't block.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn wait(&self, device: &wgpu::Device) -> Result<Vec<u8>, ReadbackFailed> {
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|_| ReadbackFailed)?;
        self.try_take(device)?.ok_or(ReadbackFailed)
    }

    /// Copies the rows out without their padding and unmaps the buffer.
    fn take(&self) -> Result<Vec<u8>, ReadbackFailed> {
        let row = (self.size[0] * 4) as usize;
        let mut pixels = Vec::with_capacity(row * self.size[1] as usize);
        {
            let mapped = self
                .buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|_| ReadbackFailed)?;
            for padded in mapped.chunks_exact(self.padded_row as usize) {
                pixels.extend_from_slice(&padded[..row]);
            }
        }
        self.buffer.unmap();
        Ok(pixels)
    }
}
