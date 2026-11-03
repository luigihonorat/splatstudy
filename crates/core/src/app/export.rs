// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Exporting the timelapse as a video or images: the export window, and
//! driving an [`Job`] one step per frame.
//!
//! Video needs an encoder from the shell ([`App::set_video_encoder`]; the
//! browser lends WebCodecs). Without one, the frames are saved as PNG
//! images: in a zip on the web, into a chosen folder on desktop.

use egui::{Align2, RichText, Stroke, TextureId};

use super::study::ReferenceLook;
use super::{App, Request, WEB};
use crate::camera::OrbitCamera;
use crate::export::{
    CameraMotion, Captions, EndCard, Gpu, Job, Layout, Progress, Settings, Shape, Sink, Support,
    VideoEncoder, clock,
    mp4::Mp4,
    zip::{self, Zip},
};
use crate::reference::{Aabb, Alignment, Content, Credit, Reference};
use crate::render::Slot;
use crate::sculpt::{Checkpoint, Mesh, Replay};
use crate::ui::{self, menu, theme};

/// Session seconds per video second, as in the timelapse panel.
const SPEEDS: [f64; 3] = [10.0, 60.0, 300.0];
/// Lengths the whole video can be fitted to, in seconds.
const FITS: [f64; 3] = [15.0, 30.0, 60.0];
const VIDEO_SHORT_SIDE: u32 = 1080;
const VIDEO_FPS: u32 = 30;
/// Images in a zip are held in memory until the download: smaller and
/// fewer of them.
const ZIP_SHORT_SIDE: u32 = 720;
const ZIP_FPS: u32 = 15;
/// Fewer images per second when a long video's would not fit in the zip.
const ZIP_FPS_CHOICES: [u32; 3] = [ZIP_FPS, 10, 5];
/// Width of the export window's contents, in points.
const WINDOW_WIDTH: f32 = 360.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Video,
    Images,
}

/// How fast the timelapse plays.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pace {
    /// Session seconds per video second.
    Speed(f64),
    /// As fast as makes the whole video about this many seconds long.
    Fit(f64),
}

/// The export being set up or running.
pub(super) struct ExportState {
    open: bool,
    layout: Layout,
    /// The user chose the layout, rather than it following the reference.
    layout_picked: bool,
    shape: Shape,
    pace: Pace,
    camera: CameraMotion,
    /// One turn around the finished clay before the end card.
    turntable: bool,
    /// The reference over the sculpt; follows the view's ghost each time
    /// the window opens.
    ghost: bool,
    /// The session's elapsed time in a corner.
    clock: bool,
    /// A title at the start.
    titled: bool,
    title: String,
    /// The title was typed, so it no longer follows the scene's name.
    title_typed: bool,
    format: Format,
    /// The playback length of the session, worked out when the window
    /// opens, again whenever the log has changed while it is open, and
    /// when the export starts.
    duration: f64,
    /// The log's length and last time when `duration` was worked out.
    duration_for: Option<(usize, u64)>,
    /// Lent by the shell; `None` where there is no video encoder.
    encoder: Option<Box<dyn VideoEncoder>>,
    job: Option<Job>,
    /// How the reference is drawn in its half and as the ghost, fixed when
    /// the export starts.
    looks: Option<[ReferenceLook; 2]>,
    /// The reference the running export shows and credits.
    shown: Option<Shown>,
    /// Frames done and in all, as of the last step.
    progress: (u64, u64),
    /// The job's composed frame, shown in the window.
    preview: Option<TextureId>,
    /// A preview to free next frame: this frame may still paint it.
    retired: Option<TextureId>,
    /// Asked the shell for a folder for the images (desktop).
    waiting_for_folder: bool,
}

impl Default for ExportState {
    fn default() -> Self {
        Self {
            open: false,
            layout: Layout::default(),
            layout_picked: false,
            shape: Shape::default(),
            pace: Pace::Speed(SPEEDS[1]),
            camera: CameraMotion::Fixed,
            turntable: true,
            ghost: false,
            clock: true,
            titled: false,
            title: String::new(),
            title_typed: false,
            format: Format::Video,
            duration: 0.0,
            duration_for: None,
            encoder: None,
            job: None,
            looks: None,
            shown: None,
            progress: (0, 0),
            preview: None,
            retired: None,
            waiting_for_folder: false,
        }
    }
}

/// Which reference an export shows, and where: the file (the same bytes,
/// not a copy) and its placement. The export draws the app's reference as
/// it is, so if any of this changes, the frames to come would show
/// something else under the credit taken at the start.
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    name: String,
    bytes: (usize, usize),
    align: Alignment,
    crop: Option<Aabb>,
}

impl Shown {
    fn of(reference: &Reference) -> Self {
        let bytes = &reference.source.bytes;
        Self {
            name: reference.source.name.clone(),
            bytes: (bytes.as_ptr() as usize, bytes.len()),
            align: reference.align,
            crop: reference.crop,
        }
    }
}

/// The credit as one line: title, author and licence.
fn credit_line(credit: &Credit) -> Option<String> {
    let parts: Vec<&str> = [&credit.title, &credit.author, &credit.licence]
        .into_iter()
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

impl App {
    /// Lends the core a video encoder; without one, exports are images.
    pub fn set_video_encoder(&mut self, encoder: Box<dyn VideoEncoder>) {
        self.export.encoder = Some(encoder);
    }

    /// An export is under way (or waiting for its folder).
    pub(super) fn exporting(&self) -> bool {
        self.export.job.is_some() || self.export.waiting_for_folder
    }

    fn video_support(&self) -> Support {
        self.export
            .encoder
            .as_ref()
            .map_or(Support::No, |e| e.support())
    }

    /// Video when it can be made and is chosen, otherwise images.
    fn export_format(&self) -> Format {
        match self.video_support() {
            Support::Yes => self.export.format,
            _ => Format::Images,
        }
    }

    /// The reference can be laid over the sculpt: it is 3D.
    fn can_ghost(&self) -> bool {
        self.scene_reference()
            .is_some_and(|r| !matches!(r.content, Content::Image(_)))
    }

    /// The window's settings, for frames `short_side` pixels high (or
    /// wide) at `fps`.
    fn export_settings(&self, short_side: u32, fps: u32) -> Settings {
        let e = &self.export;
        let mut settings = Settings {
            layout: e.layout,
            shape: e.shape,
            short_side,
            fps,
            speed: 1.0,
            look: self.sculpt_look(),
            ghost: e.ghost && self.can_ghost(),
            camera: e.camera,
            turntable: e.turntable,
            turn_reference: self.study.link,
            clock: e.clock,
        };
        settings.speed = match e.pace {
            Pace::Speed(speed) => speed,
            Pace::Fit(seconds) => settings.fit_speed(e.duration, seconds),
        };
        settings
    }

    /// Works the session's playback length out again if the log changed
    /// since the last time, or always with `force`. The window isn't
    /// modal: the user can sculpt on while it is open.
    fn refresh_duration(&mut self, force: bool) {
        let log = self.sculpt.log();
        let key = Some((log.len(), log.last().map_or(0, |e| e.time.to_bits())));
        if !force && key == self.export.duration_for {
            return;
        }
        // Only the times matter here, so a tiny start mesh will do.
        let tiny = Mesh::icosphere(0);
        let start = Checkpoint::start(&tiny);
        self.export.duration = Replay::new(vec![start], log.to_vec()).duration();
        self.export.duration_for = key;
    }

    fn open_export(&mut self) {
        self.refresh_duration(true);
        // Without a reference its half would stay black. A layout the user
        // picked stays.
        if !self.export.layout_picked {
            self.export.layout = if self.scene_reference().is_none() {
                Layout::SculptOnly
            } else {
                self.export.shape.natural_layout()
            };
        }
        self.export.ghost = self.study.ghost;
        if !self.export.title_typed {
            self.export.title = self.scene_stem();
        }
        self.export.open = true;
    }

    /// Stops an export under way, dropping what it made so far.
    pub(super) fn cancel_export(&mut self) {
        if let Some(mut job) = self.export.job.take() {
            job.cancel(self.export.encoder.as_deref_mut());
        }
        self.export.waiting_for_folder = false;
        self.end_export();
    }

    fn end_export(&mut self) {
        if let Some(id) = self.export.retired.take() {
            self.renderer.free_texture(&id);
        }
        self.export.retired = self.export.preview.take();
    }

    /// The folder picked for an image sequence (desktop). An empty path
    /// means the picker was cancelled: the window goes back to its
    /// settings.
    /// A save or image write failed in the shell. It arrives a frame after
    /// the request, so it replaces an "exported" notice shown meanwhile.
    pub(super) fn on_write_failed(&mut self, message: &str) {
        let tr = self.lang.tr();
        self.notify(format!("{} — {message}", tr.video.write_failed));
    }

    pub(super) fn on_folder(&mut self, path: String) {
        if std::mem::take(&mut self.export.waiting_for_folder) && !path.is_empty() {
            self.start_export(Sink::Folder(path), VIDEO_SHORT_SIDE, VIDEO_FPS);
        }
    }

    /// Starts exporting with the window's settings.
    fn begin_export(&mut self) {
        self.refresh_duration(false);
        match (self.export_format(), WEB) {
            (Format::Video, _) => {
                let size = self.export.shape.size(VIDEO_SHORT_SIDE);
                let mp4 = Mp4::new(size[0] as u16, size[1] as u16, VIDEO_FPS);
                self.start_export(Sink::Video(mp4), VIDEO_SHORT_SIDE, VIDEO_FPS);
            }
            (Format::Images, true) => {
                let v = self.lang.tr().video;
                match self.zip_fps() {
                    Some(fps) => {
                        if fps < ZIP_FPS {
                            self.notify(format!("{} ({fps} fps)", v.zip_fewer));
                        }
                        self.start_export(Sink::Zip(Zip::new()), ZIP_SHORT_SIDE, fps);
                    }
                    None => self.notify(v.zip_too_long),
                }
            }
            (Format::Images, false) => {
                self.export.waiting_for_folder = true;
                self.request(Request::PickFolder);
            }
        }
    }

    /// The highest frame rate whose images likely fit in a zip in memory,
    /// if any does.
    fn zip_fps(&self) -> Option<u32> {
        let size = self.export.shape.size(ZIP_SHORT_SIDE);
        ZIP_FPS_CHOICES.into_iter().find(|&fps| {
            let frames = self
                .export_settings(ZIP_SHORT_SIDE, fps)
                .timeline(self.export.duration)
                .total();
            zip::estimate(size, frames) <= zip::BUDGET as u64
        })
    }

    /// Stops a running export whose reference was changed or removed: the
    /// rest of its frames would show something else under the credit it
    /// started with.
    fn check_export_reference(&mut self) {
        if self.export.job.is_none() {
            return;
        }
        let shown = self.scene_reference().map(Shown::of);
        if shown != self.export.shown {
            self.cancel_export();
            self.notify(self.lang.tr().video.reference_changed);
        }
    }

    fn start_export(&mut self, sink: Sink, short_side: u32, fps: u32) {
        // The fitted speed is worked out from the session as it is now.
        self.refresh_duration(true);
        let tr = self.lang.tr();
        let settings = self.export_settings(short_side, fps);
        self.export.shown = self.scene_reference().map(Shown::of);
        let reference_look = self.reference_look();
        let ghost_look = self.ghost_look().unwrap_or(ReferenceLook {
            opacity: theme::GHOST_OPACITY,
            over: true,
            ..reference_look
        });
        self.export.looks = Some([reference_look, ghost_look]);
        let replay = Replay::of(&self.sculpt);
        let reference = self.scene_reference();
        let card = EndCard {
            title: reference.map_or_else(String::new, |r| {
                if r.credit.title.is_empty() {
                    r.source.name.clone()
                } else {
                    r.credit.title.clone()
                }
            }),
            lines: reference.map_or_else(Vec::new, |r| {
                vec![
                    r.credit.author.clone(),
                    r.credit.licence.clone(),
                    r.credit.url.clone(),
                ]
            }),
            footer: format!("{} · splatstudy.com", tr.video.made_with),
        };
        let title = self.export.title.trim();
        let captions = Captions {
            credit: reference.and_then(|r| credit_line(&r.credit)),
            title: (self.export.titled && !title.is_empty()).then(|| title.to_owned()),
            card,
        };
        let name = format!("{}-timelapse", self.scene_stem());
        let job = Job::new(
            &self.device,
            settings,
            replay,
            self.cameras,
            captions,
            name,
            sink,
            self.export.encoder.as_deref_mut(),
        );
        match job {
            Ok(mut job) => {
                if let Some(Content::Image(image)) = self.scene_reference().map(|r| &r.content) {
                    job.set_image(image);
                }
                let id = self.renderer.register_native_texture(
                    &self.device,
                    job.preview(),
                    wgpu::FilterMode::Linear,
                );
                self.export.preview = Some(id);
                self.export.progress = (0, job.total());
                self.export.job = Some(job);
            }
            Err(err) => self.notify(format!("{} ({err})", tr.video.failed)),
        }
    }

    /// Moves the export on by what can be done this frame.
    fn step_export(&mut self, ui: &egui::Ui) {
        let Some(mut job) = self.export.job.take() else {
            return;
        };
        let mut encoder = self.export.encoder.take();
        let mut writes = Vec::new();
        let progress = {
            let gpu = Gpu {
                device: &self.device,
                queue: &self.queue,
                clay: &self.clay,
            };
            job.step(
                &gpu,
                encoder.as_deref_mut(),
                &mut |pass, slot, camera, size| {
                    self.draw_export_reference(pass, slot, camera, size);
                },
                &mut writes,
            )
        };
        for (path, bytes) in writes {
            self.request(Request::WriteFile { path, bytes });
        }
        let tr = self.lang.tr();
        match progress {
            Progress::Working { done, total } => {
                self.export.progress = (done, total);
                self.export.job = Some(job);
                ui.ctx().request_repaint();
            }
            Progress::File { name, mime, bytes } => {
                self.request(Request::SaveFile { name, mime, bytes });
                self.notify(tr.video.exported);
                self.export.open = false;
                self.end_export();
            }
            Progress::Written { .. } => {
                self.notify(tr.video.images_written);
                self.export.open = false;
                self.end_export();
            }
            Progress::Failed(err) => {
                log::warn!("export failed: {err}");
                job.cancel(encoder.as_deref_mut());
                self.notify(format!("{} ({err})", tr.video.failed));
                self.end_export();
            }
        }
        self.export.encoder = encoder;
    }

    /// Draws the reference into an exported frame, seen from `camera`: in
    /// its own half as the reference view shows it (the silhouette
    /// included), or over the sculpt as the ghost. Pictures draw nothing
    /// here: the compositor shows them.
    fn draw_export_reference(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        slot: Slot,
        camera: &OrbitCamera,
        size: [u32; 2],
    ) {
        let [reference, ghost] = self.export.looks.unwrap_or_else(|| {
            let look = self.reference_look();
            [
                look,
                ReferenceLook {
                    opacity: theme::GHOST_OPACITY,
                    over: true,
                    ..look
                },
            ]
        });
        let look = match slot {
            Slot::ExportSculpt => ghost,
            _ => reference,
        };
        self.draw_reference_sized(pass, slot, camera, size, &look);
    }

    /// Video items in the file → export menu, after the mesh formats.
    pub(super) fn video_export_items(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        if menu::action(ui, self.export.job.is_none(), tr.video.menu_item, "") {
            self.open_export();
        }
    }

    /// Export controls in the timelapse panel.
    pub(super) fn playback_export_items(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui.add_space(6.0);
        if ui::choice(ui, tr.video.panel_item, self.export.open).clicked() {
            self.open_export();
        }
    }

    /// The export window, while one is open. Also moves a running export
    /// on, open or not.
    pub(super) fn export_window(&mut self, ui: &egui::Ui) {
        if self.export.job.is_none()
            && let Some(id) = self.export.retired.take()
        {
            self.renderer.free_texture(&id);
        }
        self.check_export_reference();
        self.step_export(ui);
        if !self.export.open {
            return;
        }
        let tr = self.lang.tr();
        let running = self.export.job.is_some() || self.export.waiting_for_folder;
        if !running {
            self.refresh_duration(false);
        }
        let mut open = true;
        let mut window = egui::Window::new(tr.video.title)
            .collapsible(false)
            .resizable(false)
            .min_width(WINDOW_WIDTH)
            .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO);
        // A running export ends with cancel, not by closing.
        if !running {
            window = window.open(&mut open);
        }
        window.show(ui.ctx(), |ui| {
            ui.set_width(WINDOW_WIDTH);
            if running {
                self.export_progress_ui(ui);
            } else {
                self.export_settings_ui(ui);
            }
        });
        if !open {
            self.export.open = false;
        }
    }

    fn export_settings_ui(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        let v = tr.video;
        let muted = ui.visuals().widgets.inactive.fg_stroke.color;
        let note = |ui: &mut egui::Ui, text: &str| {
            ui.label(
                RichText::new(text)
                    .font(ui::fonts::regular(12.0))
                    .color(muted),
            );
        };
        let row = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                add(ui);
            });
        };
        let can_ghost = self.can_ghost();
        let e = &mut self.export;

        ui::section(ui, v.layout);
        row(ui, &mut |ui| {
            for (layout, label) in [
                (Layout::SideBySide, v.side_by_side),
                (Layout::Stacked, v.stacked),
                (Layout::SculptOnly, v.sculpt_only),
            ] {
                if ui::choice(ui, label, e.layout == layout).clicked() {
                    e.layout = layout;
                    e.layout_picked = true;
                }
            }
        });
        ui::section(ui, v.frame);
        row(ui, &mut |ui| {
            for (shape, label) in [
                (Shape::Wide, v.wide),
                (Shape::Square, v.square),
                (Shape::Tall, v.tall),
            ] {
                if ui::choice(ui, label, e.shape == shape).clicked() {
                    e.shape = shape;
                    if e.layout != Layout::SculptOnly {
                        e.layout = shape.natural_layout();
                    }
                }
            }
        });

        ui::section(ui, v.speed);
        row(ui, &mut |ui| {
            for speed in SPEEDS {
                if ui::choice(ui, format!("×{speed}"), e.pace == Pace::Speed(speed)).clicked() {
                    e.pace = Pace::Speed(speed);
                }
            }
        });
        row(ui, &mut |ui| {
            note(ui, v.fit_to);
            for seconds in FITS {
                let label = format!("{seconds}{}", v.seconds);
                if ui::choice(ui, label, e.pace == Pace::Fit(seconds)).clicked() {
                    e.pace = Pace::Fit(seconds);
                }
            }
        });

        ui::section(ui, v.camera);
        row(ui, &mut |ui| {
            for (camera, label) in [
                (CameraMotion::Fixed, v.fixed),
                (CameraMotion::Turntable, v.turntable),
            ] {
                if ui::choice(ui, label, e.camera == camera).clicked() {
                    e.camera = camera;
                }
            }
        });
        row(ui, &mut |ui| {
            if ui::choice(ui, v.turn_at_end, e.turntable).clicked() {
                e.turntable = !e.turntable;
            }
        });

        ui::section(ui, v.show);
        row(ui, &mut |ui| {
            if can_ghost && ui::choice(ui, v.ghost, e.ghost).clicked() {
                e.ghost = !e.ghost;
            }
            if ui::choice(ui, v.elapsed, e.clock).clicked() {
                e.clock = !e.clock;
            }
            if ui::choice(ui, v.title_item, e.titled).clicked() {
                e.titled = !e.titled;
            }
        });
        if e.titled {
            let field = egui::TextEdit::singleline(&mut e.title).desired_width(f32::INFINITY);
            if ui.add(field).changed() {
                e.title_typed = true;
            }
        }

        let fps = if self.export_format() == Format::Images && WEB {
            ZIP_FPS
        } else {
            VIDEO_FPS
        };
        let settings = self.export_settings(VIDEO_SHORT_SIDE, fps);
        let mut length = format!(
            "{} {}",
            v.length,
            clock(settings.seconds(self.export.duration))
        );
        if let Pace::Fit(_) = self.export.pace {
            length += &format!(" · ×{}", settings.speed.round());
        }
        ui.add_space(4.0);
        note(ui, &length);

        ui::section(ui, v.file);
        let support = self.video_support();
        let format = self.export_format();
        let e = &mut self.export;
        row(ui, &mut |ui| {
            let video = ui
                .add_enabled_ui(support == Support::Yes, |ui| {
                    ui::choice(ui, v.video, format == Format::Video)
                })
                .inner;
            if video.clicked() {
                e.format = Format::Video;
            }
            if ui::choice(ui, v.images, format == Format::Images).clicked() {
                e.format = Format::Images;
            }
        });
        let hint = match (support, format) {
            (Support::Checking, _) => Some(v.checking),
            (Support::No, _) if WEB => Some(v.no_video),
            (_, Format::Images) if WEB => Some(v.images_zip),
            (_, Format::Images) => Some(v.images_folder),
            _ => None,
        };
        if let Some(hint) = hint {
            note(ui, hint);
        }
        if WEB && format == Format::Images && self.zip_fps().is_none() {
            note(ui, v.zip_too_long);
        }

        ui.add_space(12.0);
        let recorded = !self.sculpt.log().is_empty();
        if !recorded {
            note(ui, v.nothing_recorded);
        }
        let ready = recorded && support != Support::Checking;
        if ui
            .add_enabled_ui(ready, |ui| ui::choice(ui, v.start, false))
            .inner
            .clicked()
        {
            self.begin_export();
        }
    }

    fn export_progress_ui(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        let visuals = ui.visuals();
        let (text, muted, line) = (
            visuals.widgets.active.fg_stroke.color,
            visuals.widgets.inactive.fg_stroke.color,
            visuals.widgets.noninteractive.bg_stroke.color,
        );
        if let (Some(id), Some(job)) = (self.export.preview, &self.export.job) {
            let [w, h] = job.size().map(|v| v as f32);
            let scale = (WINDOW_WIDTH / w).min(WINDOW_WIDTH / h);
            ui.vertical_centered(|ui| {
                ui.image(egui::load::SizedTexture::new(id, egui::vec2(w, h) * scale));
            });
            ui.add_space(10.0);
        }
        let (done, total) = self.export.progress;
        if total > 0 {
            ui.label(
                RichText::new(format!("{done} / {total} {}", tr.video.frames))
                    .font(ui::fonts::regular(12.0))
                    .color(muted),
            );
            // A 1px track, filled in the text colour as far as it got.
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::hover());
            let y = rect.center().y;
            let x = rect.left() + rect.width() * (done as f32 / total as f32).min(1.0);
            ui.painter()
                .hline(rect.x_range(), y, Stroke::new(1.0, line));
            ui.painter()
                .hline(rect.left()..=x, y, Stroke::new(1.0, text));
            ui.add_space(8.0);
        }
        if ui::choice(ui, tr.video.cancel, false).clicked() {
            self.cancel_export();
            self.export.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_lines_skip_what_is_missing() {
        let credit = Credit {
            title: "Bust".into(),
            author: String::new(),
            licence: "CC0".into(),
            url: "https://example.org".into(),
        };
        assert_eq!(credit_line(&credit).as_deref(), Some("Bust · CC0"));
        assert_eq!(credit_line(&Credit::default()), None);
    }

    /// The whole app offscreen, and a frame of it. `None` without a GPU.
    fn app() -> Option<(App, wgpu::TextureView)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).ok()?;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame"),
            size: wgpu::Extent3d {
                width: 1280,
                height: 720,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let app = App::new(device, queue, format, crate::Lang::En);
        Some((app, target.create_view(&Default::default())))
    }

    /// A stroke of two dabs, `seconds` apart on the sculpt's clock.
    fn stroke(app: &mut App, from: f64, seconds: f64) {
        use crate::sculpt::{Brush, Dab};
        let dab = Dab {
            brush: Brush::Clay,
            centre: glam::Vec3::Z,
            radius: 0.4,
            strength: 0.5,
            invert: false,
            symmetry: false,
            direction: glam::Vec3::ZERO,
        };
        app.sculpt.set_clock(from);
        app.sculpt.begin_stroke();
        app.sculpt.dab(dab);
        app.sculpt.set_clock(from + seconds);
        app.sculpt.dab(dab);
        app.sculpt.end_stroke();
    }

    #[test]
    fn the_session_length_follows_sculpting_while_the_window_is_open() {
        let Some((mut app, view)) = app() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let mut time = 0.0;
        let mut frame = |app: &mut App| {
            time += 1.0 / 60.0;
            app.frame(time, &view, [1280, 720], 1.0);
        };
        stroke(&mut app, 1.0, 1.5);
        app.open_export();
        let first = app.export.duration;
        assert!(first > 1.0, "{first}");

        // Sculpting on with the window open: the shown length follows.
        stroke(&mut app, 10.0, 1.0);
        frame(&mut app);
        let second = app.export.duration;
        assert!(second > first + 0.9, "{second} after {first}");

        // And the export starts from the session as it is then.
        app.export.open = false;
        stroke(&mut app, 20.0, 1.0);
        app.start_export(Sink::Zip(Zip::new()), ZIP_SHORT_SIDE, ZIP_FPS);
        assert!(app.export.duration > second + 0.9);
        app.cancel_export();
    }
}
