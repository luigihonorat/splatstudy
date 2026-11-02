// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! One export under way: the timelapse replayed frame by frame, composed,
//! read back and handed to a video encoder, a zip or a folder.
//!
//! The browser can't wait for the GPU, so nothing here blocks: the app
//! calls [`Job::step`] every frame. Each step collects the frames the GPU
//! has finished, hands them on, and starts a few more, keeping at most
//! [`IN_FLIGHT`] on their way back and the encoder's queue short.
//!
//! The video is the timelapse at the chosen speed, a second on the
//! finished clay, then the end card.

use std::collections::VecDeque;
use std::path::Path;

use super::compositor::{Compositor, EndCard, Overlay};
use super::layout::{Layout, Shape};
use super::mp4::Mp4;
use super::video::{VideoConfig, VideoEncoder};
use super::zip::Zip;
use crate::camera::OrbitCamera;
use crate::render::Slot;
use crate::render::clay::{ClayPass, GpuMesh, Look};
use crate::render::readback::Readback;
use crate::sculpt::Replay;
use crate::ui::theme;

/// Frames on their way back from the GPU, and frames waiting in the
/// encoder, at most.
pub const IN_FLIGHT: usize = 3;
/// Frames started per step, so the app stays responsive.
const STARTS_PER_STEP: usize = 2;
/// Seconds on the finished clay, then on the end card.
const HOLD_SECONDS: u32 = 1;
const CARD_SECONDS: u32 = 3;
/// A key frame every this many seconds, for seeking.
const KEY_SECONDS: u32 = 2;

/// Where the frames go.
pub enum Sink {
    /// H.264 through the shell's encoder, muxed here.
    Video(Mp4),
    /// PNG images in a zip, downloaded at the end (web without video).
    Zip(Zip),
    /// PNG images written one by one into a folder (desktop).
    Folder(String),
}

/// What to export.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub layout: Layout,
    pub shape: Shape,
    /// The frame's short side in pixels: 1080, or 720 for the zip.
    pub short_side: u32,
    pub fps: u32,
    /// Session seconds per video second.
    pub speed: f64,
    /// How the clay is shaded (lit, or the silhouette).
    pub look: Look,
}

impl Settings {
    /// Frames showing the clay at work, for a session of `duration`
    /// playback seconds.
    pub fn timelapse_frames(&self, duration: f64) -> u64 {
        (duration.max(0.0) / self.speed * f64::from(self.fps)).floor() as u64 + 1
    }

    /// The whole video's length in seconds, end card included.
    pub fn seconds(&self, duration: f64) -> f64 {
        self.timelapse_frames(duration) as f64 / f64::from(self.fps)
            + f64::from(HOLD_SECONDS + CARD_SECONDS)
    }
}

/// How far a job is.
#[derive(Debug)]
pub enum Progress {
    Working {
        done: u64,
        total: u64,
    },
    /// A file for the shell to save.
    File {
        name: String,
        mime: &'static str,
        bytes: Vec<u8>,
    },
    /// Every image is written into the folder.
    Written {
        frames: u64,
    },
    Failed(String),
}

/// What a step needs from the app.
pub struct Gpu<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub clay: &'a ClayPass,
}

pub struct Job {
    settings: Settings,
    compositor: Compositor,
    replay: Replay,
    gpu_mesh: GpuMesh,
    /// Sculpt view, then reference view.
    cameras: [OrbitCamera; 2],
    credit: Option<String>,
    card: EndCard,
    /// The file name without its extension.
    name: String,
    sink: Sink,
    timelapse: u64,
    total: u64,
    /// The next frame to render.
    next: u64,
    /// Frames handed to the sink.
    done: u64,
    in_flight: VecDeque<(u64, Readback)>,
    /// The encoder has been told there are no more frames.
    flushing: bool,
}

impl Job {
    /// Sets up an export of `replay` (not yet played). With a video sink,
    /// starts the encoder.
    #[allow(clippy::too_many_arguments)]
    pub fn new<'e>(
        device: &wgpu::Device,
        settings: Settings,
        replay: Replay,
        cameras: [OrbitCamera; 2],
        credit: Option<String>,
        card: EndCard,
        name: String,
        sink: Sink,
        encoder: Option<&mut (dyn VideoEncoder + 'e)>,
    ) -> Result<Job, String> {
        let size = settings.shape.size(settings.short_side);
        if let Sink::Video(_) = sink {
            let encoder = encoder.ok_or("no video encoder")?;
            encoder.start(&VideoConfig::new(size[0], size[1], settings.fps))?;
        }
        let timelapse = settings.timelapse_frames(replay.duration());
        let total = timelapse + u64::from((HOLD_SECONDS + CARD_SECONDS) * settings.fps);
        Ok(Job {
            compositor: Compositor::new(device, size, settings.layout),
            gpu_mesh: GpuMesh::new(device, &replay.sculpt.mesh),
            settings,
            replay,
            cameras,
            credit,
            card,
            name,
            sink,
            timelapse,
            total,
            next: 0,
            done: 0,
            in_flight: VecDeque::new(),
            flushing: false,
        })
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    /// The last composed frame, for a preview.
    pub fn preview(&self) -> &wgpu::TextureView {
        &self.compositor.output().resolved_view
    }

    pub fn size(&self) -> [u32; 2] {
        self.compositor.size()
    }

    /// Shows a picture in the reference half instead of what
    /// `draw_reference` draws: pictures are drawn by egui, not in a pass.
    pub fn set_image(&mut self, image: &crate::reference::RefImage) {
        self.compositor.set_image(image);
    }

    /// Does what can be done now without waiting. `draw_reference` draws
    /// the reference into an export pass, seen from the camera given, into
    /// a target of the size given in pixels. `writes` gets the images for a folder sink.
    pub fn step<'e>(
        &mut self,
        gpu: &Gpu,
        mut encoder: Option<&mut (dyn VideoEncoder + 'e)>,
        draw_reference: &mut dyn FnMut(&mut wgpu::RenderPass<'_>, &OrbitCamera, [u32; 2]),
        writes: &mut Vec<(String, Vec<u8>)>,
    ) -> Progress {
        // Frames back from the GPU, in order.
        while let Some((index, readback)) = self.in_flight.front() {
            let pixels = match readback.try_take(gpu.device) {
                Ok(Some(pixels)) => pixels,
                Ok(None) => break,
                Err(_) => return Progress::Failed("the GPU didn't return a frame".into()),
            };
            let index = *index;
            self.in_flight.pop_front();
            if let Err(err) = self.deliver(index, &pixels, encoder.as_deref_mut(), writes) {
                return Progress::Failed(err);
            }
            self.done += 1;
        }
        if let (Sink::Video(mp4), Some(encoder)) = (&mut self.sink, encoder.as_deref_mut()) {
            if let Some(err) = encoder.error() {
                return Progress::Failed(err);
            }
            for frame in encoder.take() {
                mp4.push(&frame.data, frame.key);
            }
            if !mp4.has_config()
                && let Some(config) = encoder.config()
            {
                mp4.set_config(config);
            }
        }

        // More frames, while there is room.
        for _ in 0..STARTS_PER_STEP {
            let waiting = match (&self.sink, encoder.as_deref()) {
                (Sink::Video(_), Some(encoder)) => encoder.pending(),
                _ => 0,
            };
            if self.next >= self.total || self.in_flight.len() + waiting >= IN_FLIGHT {
                break;
            }
            let readback = self.render(gpu, draw_reference);
            self.in_flight.push_back((self.next, readback));
            self.next += 1;
        }

        if self.done < self.total {
            return Progress::Working {
                done: self.done,
                total: self.total,
            };
        }
        self.finish(encoder)
    }

    /// Plays the timelapse up to frame `self.next` and starts drawing it.
    fn render(
        &mut self,
        gpu: &Gpu,
        draw_reference: &mut dyn FnMut(&mut wgpu::RenderPass<'_>, &OrbitCamera, [u32; 2]),
    ) -> Readback {
        let index = self.next;
        let time = if index < self.timelapse {
            index as f64 * self.settings.speed / f64::from(self.settings.fps)
        } else {
            f64::INFINITY
        };
        self.replay.seek(time, usize::MAX);
        let sculpt = &mut self.replay.sculpt;
        for range in sculpt.take_dirty() {
            self.gpu_mesh.write(gpu.queue, &sculpt.mesh, range);
        }

        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let card_from = self.total - u64::from(CARD_SECONDS * self.settings.fps);
        let overlay = if index >= card_from {
            Overlay::EndCard(&self.card)
        } else {
            let view = self.compositor.sculpt_view();
            gpu.clay.prepare(
                gpu.queue,
                Slot::ExportSculpt,
                &self.cameras[0],
                view.aspect(),
                &self.settings.look,
            );
            {
                let mut pass = view.begin_pass(&mut encoder, theme::VIEW_BG);
                gpu.clay.draw(&mut pass, Slot::ExportSculpt, &self.gpu_mesh);
            }
            if let Some(view) = self.compositor.reference_view() {
                let mut pass = view.begin_pass(&mut encoder, theme::VIEW_BG);
                draw_reference(&mut pass, &self.cameras[1], view.size);
            }
            Overlay::Views {
                credit: self.credit.as_deref(),
            }
        };
        let mut buffers = self
            .compositor
            .compose(gpu.device, gpu.queue, &mut encoder, &overlay);
        let readback = Readback::copy(gpu.device, &mut encoder, self.compositor.output());
        buffers.push(encoder.finish());
        gpu.queue.submit(buffers);
        readback.map();
        readback
    }

    fn deliver<'e>(
        &mut self,
        index: u64,
        pixels: &[u8],
        encoder: Option<&mut (dyn VideoEncoder + 'e)>,
        writes: &mut Vec<(String, Vec<u8>)>,
    ) -> Result<(), String> {
        let size = self.compositor.size();
        let image_name = || format!("{}_{index:05}.png", self.name);
        match &mut self.sink {
            Sink::Video(_) => {
                let key = index.is_multiple_of(u64::from(KEY_SECONDS * self.settings.fps));
                encoder
                    .ok_or("no video encoder")?
                    .encode(pixels, index, key)?;
            }
            Sink::Zip(zip) => {
                let png = super::png(pixels, size).map_err(|e| e.to_string())?;
                zip.add(&image_name(), &png)
                    .map_err(|_| "the images don't fit in a zip".to_owned())?;
            }
            Sink::Folder(folder) => {
                let png = super::png(pixels, size).map_err(|e| e.to_string())?;
                let path = Path::new(folder).join(image_name());
                writes.push((path.to_string_lossy().into_owned(), png));
            }
        }
        Ok(())
    }

    /// Every frame is out: closes the file.
    fn finish<'e>(&mut self, encoder: Option<&mut (dyn VideoEncoder + 'e)>) -> Progress {
        let working = Progress::Working {
            done: self.done,
            total: self.total,
        };
        match &mut self.sink {
            Sink::Video(mp4) => {
                let Some(encoder) = encoder else {
                    return Progress::Failed("no video encoder".into());
                };
                if !std::mem::replace(&mut self.flushing, true) {
                    encoder.finish();
                }
                if !encoder.done() {
                    return working;
                }
                for frame in encoder.take() {
                    mp4.push(&frame.data, frame.key);
                }
                if let Some(config) = encoder.config() {
                    mp4.set_config(config);
                }
                let size = self.compositor.size();
                let mp4 = std::mem::replace(mp4, Mp4::new(size[0] as u16, size[1] as u16, 1));
                match mp4.finish() {
                    Ok(bytes) => Progress::File {
                        name: format!("{}.mp4", self.name),
                        mime: "video/mp4",
                        bytes,
                    },
                    Err(err) => Progress::Failed(err.to_string()),
                }
            }
            Sink::Zip(zip) => Progress::File {
                name: format!("{}.zip", self.name),
                mime: "application/zip",
                bytes: std::mem::take(zip).finish(),
            },
            Sink::Folder(_) => Progress::Written { frames: self.done },
        }
    }

    /// Stops the encoder; the job can be dropped after.
    pub fn cancel<'e>(&mut self, encoder: Option<&mut (dyn VideoEncoder + 'e)>) {
        if let (Sink::Video(_), Some(encoder)) = (&self.sink, encoder) {
            encoder.cancel();
        }
    }
}
