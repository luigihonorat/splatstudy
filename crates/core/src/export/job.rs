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
//! finished clay, optionally one turn around it, then the end card (see
//! [`Timeline`]).

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
/// The turn around the finished clay, in seconds.
const TURN_SECONDS: u32 = 5;
/// A slow turntable during the timelapse takes about this long per turn,
/// in whole turns, so it ends where it began.
const SLOW_TURN_SECONDS: f64 = 30.0;
/// A length fitted to a target leaves the timelapse at least this long.
const MIN_TIMELAPSE_SECONDS: f64 = 2.0;
/// The title is shown this long at the start, then fades out.
const TITLE_SECONDS: f64 = 3.0;
const TITLE_FADE_SECONDS: f64 = 0.75;
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

/// How the camera moves while the clay is at work. The session's own
/// camera moves aren't recorded, so they can't be replayed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CameraMotion {
    /// The view as it is now, all along.
    #[default]
    Fixed,
    /// Slow whole turns around the clay, easing in and out.
    Turntable,
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
    /// The reference is laid over the sculpt as a ghost.
    pub ghost: bool,
    /// How the camera moves during the timelapse.
    pub camera: CameraMotion,
    /// One turn around the finished clay before the end card.
    pub turntable: bool,
    /// The reference's camera turns with the sculpt's (the cameras are
    /// linked); otherwise it keeps still.
    pub turn_reference: bool,
    /// The session's elapsed time in a corner of the sculpt.
    pub clock: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            layout: Layout::default(),
            shape: Shape::default(),
            short_side: 1080,
            fps: 30,
            speed: 60.0,
            look: Look::lit(theme::CLAY),
            ghost: false,
            camera: CameraMotion::default(),
            turntable: false,
            turn_reference: false,
            clock: false,
        }
    }
}

impl Settings {
    /// Frames showing the clay at work, for a session of `duration`
    /// playback seconds.
    pub fn timelapse_frames(&self, duration: f64) -> u64 {
        (duration.max(0.0) / self.speed * f64::from(self.fps)).floor() as u64 + 1
    }

    /// The parts of the video, in frames.
    pub fn timeline(&self, duration: f64) -> Timeline {
        let frames = |seconds: u32| u64::from(seconds * self.fps);
        Timeline {
            timelapse: self.timelapse_frames(duration),
            hold: frames(HOLD_SECONDS),
            turn: if self.turntable {
                frames(TURN_SECONDS)
            } else {
                0
            },
            card: frames(CARD_SECONDS),
        }
    }

    /// The whole video's length in seconds, end card included.
    pub fn seconds(&self, duration: f64) -> f64 {
        self.timeline(duration).total() as f64 / f64::from(self.fps)
    }

    /// Seconds of video after the timelapse.
    fn closing_seconds(&self) -> f64 {
        let turn = if self.turntable { TURN_SECONDS } else { 0 };
        f64::from(HOLD_SECONDS + turn + CARD_SECONDS)
    }

    /// The speed that makes the whole video about `target` seconds long
    /// for a session of `duration`. Never slower than the session itself,
    /// so a short session makes a shorter video.
    pub fn fit_speed(&self, duration: f64, target: f64) -> f64 {
        let timelapse = (target - self.closing_seconds()).max(MIN_TIMELAPSE_SECONDS);
        (duration / timelapse).max(1.0)
    }

    /// How far the sculpt's camera has turned at frame `index`, in
    /// radians.
    pub fn turn_at(&self, timeline: &Timeline, index: u64) -> f32 {
        use std::f64::consts::TAU;
        let angle = match timeline.at(index) {
            Moment::Timelapse(i) if self.camera == CameraMotion::Turntable => {
                let span = timeline.timelapse.saturating_sub(1).max(1);
                let seconds = span as f64 / f64::from(self.fps);
                let turns = (seconds / SLOW_TURN_SECONDS).round().max(1.0);
                TAU * turns * ease(i as f64 / span as f64)
            }
            Moment::Turn(t) => TAU * ease(t),
            _ => 0.0,
        };
        angle as f32
    }
}

/// Eases in and out: 0 at 0, 1 at 1, still at both ends.
pub fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    (1.0 - (std::f64::consts::PI * t).cos()) / 2.0
}

/// The parts of an exported video, in frames, one after the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeline {
    /// The clay at work.
    pub timelapse: u64,
    /// Still, on the finished clay.
    pub hold: u64,
    /// Turning once around the finished clay.
    pub turn: u64,
    pub card: u64,
}

/// What a frame shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Moment {
    /// Frame `n` of the timelapse.
    Timelapse(u64),
    Hold,
    /// The turn, from 0 to just under 1.
    Turn(f64),
    Card,
}

impl Timeline {
    pub fn total(&self) -> u64 {
        self.timelapse + self.hold + self.turn + self.card
    }

    pub fn at(&self, index: u64) -> Moment {
        let mut i = index;
        if i < self.timelapse {
            return Moment::Timelapse(i);
        }
        i -= self.timelapse;
        if i < self.hold {
            return Moment::Hold;
        }
        i -= self.hold;
        if i < self.turn {
            return Moment::Turn(i as f64 / self.turn as f64);
        }
        Moment::Card
    }
}

/// What a job writes over the frames.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Captions {
    /// Along the bottom of the reference view.
    pub credit: Option<String>,
    /// Over the views at the start.
    pub title: Option<String>,
    pub card: EndCard,
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

/// "m:ss", for the session's elapsed time and the video's length.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The camera for a view `aspect` wide: in a view taller than wide, the
/// field of view is widened so the camera's view still fits across, as
/// it does up and down in the app's wide views.
pub fn fit_camera(camera: &OrbitCamera, aspect: f32) -> OrbitCamera {
    let mut camera = *camera;
    if aspect < 1.0 {
        camera.fov_y = 2.0 * ((camera.fov_y / 2.0).tan() / aspect.max(0.1)).atan();
    }
    camera
}

/// What a step needs from the app.
pub struct Gpu<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub clay: &'a ClayPass,
}

/// Draws the reference into an export pass; see [`Job::step`].
pub type DrawReference<'a> =
    dyn FnMut(&mut wgpu::RenderPass<'_>, Slot, &OrbitCamera, [u32; 2]) + 'a;

pub struct Job {
    settings: Settings,
    compositor: Compositor,
    replay: Replay,
    gpu_mesh: GpuMesh,
    /// Sculpt view, then reference view.
    cameras: [OrbitCamera; 2],
    captions: Captions,
    /// The file name without its extension.
    name: String,
    sink: Sink,
    /// The session's playback length, in seconds.
    duration: f64,
    timeline: Timeline,
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
        captions: Captions,
        name: String,
        sink: Sink,
        encoder: Option<&mut (dyn VideoEncoder + 'e)>,
    ) -> Result<Job, String> {
        let size = settings.shape.size(settings.short_side);
        if let Sink::Video(_) = sink {
            let encoder = encoder.ok_or("no video encoder")?;
            encoder.start(&VideoConfig::new(size[0], size[1], settings.fps))?;
        }
        let duration = replay.duration();
        let timeline = settings.timeline(duration);
        Ok(Job {
            compositor: Compositor::new(device, size, settings.layout),
            gpu_mesh: GpuMesh::new(device, &replay.sculpt.mesh),
            settings,
            replay,
            cameras,
            captions,
            name,
            sink,
            duration,
            timeline,
            total: timeline.total(),
            next: 0,
            done: 0,
            in_flight: VecDeque::new(),
            flushing: false,
        })
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn timeline(&self) -> Timeline {
        self.timeline
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
    /// a target of the size given in pixels: into the reference half
    /// ([`Slot::ExportReference`]), or over the clay as the ghost
    /// ([`Slot::ExportSculpt`], last in the pass). `writes` gets the images
    /// for a folder sink.
    pub fn step<'e>(
        &mut self,
        gpu: &Gpu,
        mut encoder: Option<&mut (dyn VideoEncoder + 'e)>,
        draw_reference: &mut DrawReference<'_>,
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
    fn render(&mut self, gpu: &Gpu, draw_reference: &mut DrawReference<'_>) -> Readback {
        let index = self.next;
        let settings = self.settings;
        let fps = f64::from(settings.fps);
        let moment = self.timeline.at(index);
        let time = match moment {
            Moment::Timelapse(i) => i as f64 * settings.speed / fps,
            _ => f64::INFINITY,
        };
        self.replay.seek(time, usize::MAX);
        let sculpt = &mut self.replay.sculpt;
        if sculpt.take_topology_changed() {
            // A checkpoint with other triangles: the index buffer can't be
            // updated in place.
            self.gpu_mesh = GpuMesh::upload(gpu.device, gpu.queue, &sculpt.mesh);
            sculpt.take_dirty();
        } else {
            for range in sculpt.take_dirty() {
                self.gpu_mesh.write(gpu.queue, &sculpt.mesh, range);
            }
        }

        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let elapsed;
        let overlay = if moment == Moment::Card {
            Overlay::EndCard(&self.captions.card)
        } else {
            let turn = settings.turn_at(&self.timeline, index);
            let mut cameras = self.cameras;
            cameras[0].yaw += turn;
            if settings.turn_reference {
                cameras[1].yaw += turn;
            }
            if let Some(view) = self.compositor.sculpt_view() {
                let camera = fit_camera(&cameras[0], view.aspect());
                gpu.clay.prepare(
                    gpu.queue,
                    Slot::ExportSculpt,
                    &camera,
                    view.aspect(),
                    &settings.look,
                );
                let mut pass = view.begin_pass(&mut encoder, theme::VIEW_BG);
                gpu.clay.draw(&mut pass, Slot::ExportSculpt, &self.gpu_mesh);
                if settings.ghost {
                    draw_reference(&mut pass, Slot::ExportSculpt, &camera, view.size);
                }
            }
            if let Some(view) = self.compositor.reference_view() {
                let mut pass = view.begin_pass(&mut encoder, theme::VIEW_BG);
                draw_reference(
                    &mut pass,
                    Slot::ExportReference,
                    &fit_camera(&cameras[1], view.aspect()),
                    view.size,
                );
            }
            elapsed = settings.clock.then(|| clock(time.min(self.duration)));
            let seconds = index as f64 / fps;
            let title = self.captions.title.as_deref().and_then(|title| {
                let left = 1.0 - (seconds - TITLE_SECONDS) / TITLE_FADE_SECONDS;
                (left > 0.0).then_some((title, left.min(1.0) as f32))
            });
            // Credited wherever the reference shows: in its half, or as
            // the ghost over the sculpt.
            let credited = settings.layout.shows_reference() || settings.ghost;
            Overlay::Views {
                credit: self.captions.credit.as_deref().filter(|_| credited),
                clock: elapsed.as_deref(),
                title,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn tall_views_see_as_far_across_as_wide_ones_see_up() {
        let camera = OrbitCamera::default();
        assert_eq!(fit_camera(&camera, 16.0 / 9.0), camera);
        let tall = fit_camera(&camera, 9.0 / 16.0);
        // Half the width seen across equals what the original sees up.
        let across = (tall.fov_y / 2.0).tan() * 9.0 / 16.0;
        assert!((across - (camera.fov_y / 2.0).tan()).abs() < 1e-5);
    }

    #[test]
    fn the_timeline_runs_timelapse_hold_turn_then_card() {
        let settings = Settings {
            fps: 10,
            speed: 10.0,
            turntable: true,
            ..Settings::default()
        };
        // 60 s at ×10: 6 s, plus the first frame.
        let t = settings.timeline(60.0);
        assert_eq!((t.timelapse, t.hold, t.turn, t.card), (61, 10, 50, 30));
        assert_eq!(t.total(), 151);
        assert_eq!(t.at(0), Moment::Timelapse(0));
        assert_eq!(t.at(60), Moment::Timelapse(60));
        assert_eq!(t.at(61), Moment::Hold);
        assert_eq!(t.at(71), Moment::Turn(0.0));
        assert_eq!(t.at(96), Moment::Turn(0.5));
        assert_eq!(t.at(121), Moment::Card);
        assert_eq!(t.at(10_000), Moment::Card);
        assert!((settings.seconds(60.0) - 15.1).abs() < 1e-9);
        // Without the turn, the hold goes straight to the card.
        let plain = Settings {
            turntable: false,
            ..settings
        };
        assert_eq!(plain.timeline(60.0).at(71), Moment::Card);
    }

    #[test]
    fn fitted_speeds_give_the_target_length() {
        for turntable in [false, true] {
            let mut settings = Settings {
                turntable,
                ..Settings::default()
            };
            // A 20-minute session.
            for target in [15.0, 30.0, 60.0] {
                settings.speed = settings.fit_speed(1200.0, target);
                let seconds = settings.seconds(1200.0);
                assert!((seconds - target).abs() < 0.1, "{target} gave {seconds}");
            }
        }
        // A one-minute session isn't slowed down to fill a minute.
        let settings = Settings::default();
        assert_eq!(settings.fit_speed(30.0, 60.0), 1.0);
        // A target shorter than the ending keeps a little timelapse.
        let settings = Settings {
            turntable: true,
            ..Settings::default()
        };
        assert_eq!(settings.fit_speed(100.0, 5.0), 50.0);
    }

    #[test]
    fn the_turntable_eases_once_around_and_comes_back() {
        let settings = Settings {
            fps: 10,
            speed: 10.0,
            turntable: true,
            ..Settings::default()
        };
        let t = settings.timeline(60.0);
        // A fixed camera stays still through the timelapse and the hold.
        for i in 0..71 {
            assert_eq!(settings.turn_at(&t, i), 0.0);
        }
        // The turn starts still, is half way round in the middle and
        // nearly back at the end, and the card doesn't turn.
        let first = settings.turn_at(&t, 71);
        let second = settings.turn_at(&t, 72);
        let middle = settings.turn_at(&t, 96);
        let last = settings.turn_at(&t, 120);
        assert_eq!(first, 0.0);
        assert!(second < 0.01, "eases in: {second}");
        assert!((middle - TAU / 2.0).abs() < 1e-5);
        assert!(TAU - last < 0.01, "eases out: {last}");
        assert_eq!(settings.turn_at(&t, 121), 0.0);
        // Turning all the way: always onward.
        let angles: Vec<f32> = (71..121).map(|i| settings.turn_at(&t, i)).collect();
        assert!(angles.windows(2).all(|w| w[1] > w[0]));

        // A slow turntable through a 6 s timelapse: one whole turn, from
        // the view as it is back to it.
        let slow = Settings {
            camera: CameraMotion::Turntable,
            ..settings
        };
        assert_eq!(slow.turn_at(&t, 0), 0.0);
        assert!((slow.turn_at(&t, 30) - TAU / 2.0).abs() < 1e-5);
        assert!((slow.turn_at(&t, 60) - TAU).abs() < 1e-5);
        assert_eq!(slow.turn_at(&t, 61), 0.0, "the hold is where it began");
        // A long one turns about every 30 s.
        let long = slow.timeline(3600.0);
        let end = long.timelapse - 1;
        assert!((slow.turn_at(&long, end) - 12.0 * TAU).abs() < 1e-3);
    }

    #[test]
    fn clocks_read_minutes_and_seconds() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(62.4), "1:02");
        assert_eq!(clock(-3.0), "0:00");
        assert_eq!(clock(3599.6), "60:00");
    }
}
