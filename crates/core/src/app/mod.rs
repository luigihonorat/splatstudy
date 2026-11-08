// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The application: owns the editor state, the egui context and the GPU
//! resources, takes input events and draws frames.
//!
//! This file holds the state, the layout and the frame. Each part of the
//! editor adds its own `impl App` block in its own file:
//!
//! - `views`: the two views, the divider, navigation and viewpoints
//! - `sculpting`: tools, strokes and the sculpt panel
//! - `playback`: watching the timelapse
//! - `reference`: the reference and its panel
//! - `gallery`: the bundled casts to choose a reference from
//! - `study`: comparison aids (camera link, silhouette, ghost, guides, timer)
//! - `calipers`: lengths set between two points, over both views
//! - `tweak`: one step of smoothing, sharpening, shrinking or inflating
//!   over the whole clay, from the sculpt panel
//! - `toolbar`: each view's toolbar of view options
//! - `background`: each view's background colour, and the window that
//!   picks it
//! - `panels`: hiding the side panels (Tab, Shift+Tab) and choosing their
//!   groups (the window menu)
//! - `files`: scenes, opening and importing
//! - `export`: exporting the timelapse
//! - `screenshot`: a picture of the views
//! - `help`: getting started and preferences
//! - `start`: the options in the page's URL, applied at start
//! - `menus` and `shortcuts`: shared; they call a hook in each part's file,
//!   so a part grows without touching them.

mod background;
mod calipers;
mod export;
mod files;
mod gallery;
mod help;
mod menus;
mod panels;
mod playback;
mod reference;
mod screenshot;
mod sculpting;
mod shortcuts;
mod start;
pub use start::StartOptions;
mod study;
mod toolbar;
mod tweak;
mod views;

use egui::{CursorIcon, Frame, Key, Margin, PointerButton, Pos2, Rect};
use glam::Vec3;
use panels::Group;

use crate::camera::OrbitCamera;
use crate::input::{FilePurpose, InputEvent, InputState, PointerEvent};
use crate::navigation::{NavAction, NavStyle};
use crate::render::clay::{ClayPass, GpuMesh};
use crate::render::{self, Slot, ViewTarget};
use crate::sculpt::Sculpt;
use crate::ui::{self, fonts, i18n::Lang, theme::ThemeMode};

/// What the shell should do after a frame.
#[derive(Clone, Debug)]
pub struct FrameOutput {
    pub cursor: CursorIcon,
    /// Seconds until the next frame is needed; `None` waits for input.
    pub repaint_after: Option<f64>,
    /// The user chose exit: the shell should close the app.
    pub quit: bool,
    /// A link the shell should open in the browser.
    pub open_url: Option<String>,
    /// Things only the shell can do, in the order they were asked for.
    pub requests: Vec<Request>,
}

/// Something only the shell can do: pick, save and fetch files.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// Let the user pick a file with one of these extensions (lowercase, no
    /// dot). It comes back as [`InputEvent::File`] with this purpose.
    OpenFile {
        purpose: FilePurpose,
        extensions: &'static [&'static str],
    },
    /// Save bytes under a suggested name: a save dialog on desktop, a
    /// download on the web.
    SaveFile {
        name: String,
        mime: &'static str,
        bytes: Vec<u8>,
    },
    /// Let the user pick a folder (desktop). It comes back as
    /// [`InputEvent::Folder`].
    PickFolder,
    /// Write a file into a picked folder (desktop).
    WriteFile { path: String, bytes: Vec<u8> },
    /// Fetch a file shipped with the web build, such as a sample, by its
    /// path next to the page (the desktop app downloads the gallery's from
    /// the web app). It comes back as [`InputEvent::File`], empty if it
    /// couldn't be fetched.
    Fetch { url: String, purpose: FilePurpose },
}

/// How long a notice stays on screen, in seconds.
const NOTICE_SECONDS: f64 = 4.0;

/// The browser keeps a tab open and owns some shortcuts (Ctrl+N), so the
/// web build leaves those items out.
const WEB: bool = cfg!(target_arch = "wasm32");
const ISSUES_URL: &str = "https://github.com/luigihonorat/splatstudy/issues";
const REPO_URL: &str = "https://github.com/luigihonorat/splatstudy";
/// The website; the web app itself is at app.splatstudy.com.
const SITE_URL: &str = "https://splatstudy.com";
/// Every build ships the notices; the web page has them next to it.
const LICENCES_URL: &str = if WEB {
    "licenses.html"
} else {
    "https://app.splatstudy.com/licenses.html"
};

const SCULPT: usize = 0;
const REFERENCE: usize = 1;

/// Width of the tool panels, in points.
const PANEL_WIDTH: f32 = 196.0;

/// What a drag in a view is doing.
#[derive(Clone, Copy)]
enum Drag {
    /// Clay or smooth: dabs along the path. `carry` is the distance travelled
    /// since the last dab. `pointer` is the press that started it: only
    /// that pointer's samples go on with it.
    Stroke {
        last: Pos2,
        pressure: f32,
        carry: f32,
        pointer: Option<PointerEvent>,
    },
    /// Move: the surface follows the pointer within the view plane.
    Grab {
        start: Pos2,
        right: Vec3,
        up: Vec3,
        scale: f32,
    },
    /// Ctrl pressed on the background, in ZBrush style: let go where it
    /// started to invert the mask, or after a drag to clear it.
    MaskTap { start: Pos2 },
    /// Turning, panning or zooming the view while `button` is held.
    /// `free` is the yaw and pitch the turn has reached, which the camera
    /// shows rounded while it snaps to the nearest side.
    Navigate {
        action: NavAction,
        button: PointerButton,
        free: (f32, f32),
    },
}

pub struct App {
    device: wgpu::Device,
    queue: wgpu::Queue,
    ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    input: InputState,
    events: Vec<egui::Event>,
    /// Numpad digits pressed since the last frame, for the viewpoints.
    numpad: Vec<u8>,
    focused: bool,
    lang: Lang,
    theme: ThemeMode,
    nav: NavStyle,
    /// Shortcuts read Cmd instead of Ctrl.
    mac: bool,
    show_shortcuts: bool,
    show_about: bool,
    /// Whether the side panels show.
    panels: panels::Panels,
    quit: bool,
    debug: bool,
    /// Share of the central width given to the sculpt view.
    split: f32,
    views: [ViewTarget; 2],
    /// Where each view sits, in points; set by the last layout.
    view_rects: [Rect; 2],
    cameras: [OrbitCamera; 2],
    drags: [Option<Drag>; 2],
    /// A toolbar button's column of choices, while it is open.
    flyout: Option<toolbar::OpenFlyout>,
    /// Each view's background colour, and the window that picks it.
    background: background::BackgroundState,
    clay: ClayPass,
    sculpt: Sculpt,
    gpu_mesh: GpuMesh,
    tool: sculpting::ToolSettings,
    playback: Option<playback::Playback>,
    // Each part keeps its own state here and adds fields inside its struct.
    #[allow(dead_code)] // until the reference part fills it
    reference: reference::ReferenceState,
    #[allow(dead_code)] // until the study part fills it
    study: study::StudyState,
    calipers: calipers::CaliperState,
    #[allow(dead_code)] // until the files part fills it
    files: files::FileState,
    #[allow(dead_code)] // until the export part fills it
    export: export::ExportState,
    shot: screenshot::ShotState,
    help: help::HelpState,
    /// Asked for since the last frame; handed to the shell with it.
    requests: Vec<Request>,
    /// A short message at the bottom of the views, and when it was first
    /// shown.
    notice: Option<(String, Option<f64>)>,
    /// The clock passed to the current frame, in seconds.
    now: f64,
    dirty: bool,
}

impl App {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        lang: Lang,
    ) -> Self {
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts::definitions());
        let theme = ThemeMode::default();
        ui::theme::apply(&ctx, theme);
        let mut renderer = egui_wgpu::Renderer::new(
            &device,
            surface_format,
            egui_wgpu::RendererOptions::default(),
        );
        let mut views = [
            ViewTarget::new(&device, [1, 1]),
            ViewTarget::new(&device, [1, 1]),
        ];
        for view in &mut views {
            view.register(&device, &mut renderer);
        }
        let sculpt = sculpting::start_sculpt();
        let gpu_mesh = GpuMesh::new(&device, &sculpt.mesh);
        let clay = ClayPass::new(&device);
        Self {
            device,
            queue,
            ctx,
            renderer,
            input: InputState::default(),
            events: Vec::new(),
            numpad: Vec::new(),
            focused: true,
            lang,
            theme,
            nav: NavStyle::default(),
            mac: cfg!(target_os = "macos"),
            show_shortcuts: false,
            show_about: false,
            panels: Default::default(),
            quit: false,
            debug: false,
            split: 0.5,
            views,
            view_rects: [Rect::NOTHING; 2],
            cameras: [OrbitCamera::default(); 2],
            drags: [None; 2],
            flyout: None,
            background: Default::default(),
            clay,
            sculpt,
            gpu_mesh,
            tool: sculpting::ToolSettings::default(),
            playback: None,
            reference: Default::default(),
            study: Default::default(),
            calipers: Default::default(),
            files: Default::default(),
            export: Default::default(),
            shot: Default::default(),
            help: Default::default(),
            requests: Vec::new(),
            notice: None,
            now: 0.0,
            dirty: true,
        }
    }

    /// Shows shortcuts with Cmd; the web shell knows this from the browser.
    pub fn set_mac(&mut self, mac: bool) {
        self.mac = mac;
    }

    pub fn set_debug(&mut self, on: bool) {
        self.debug = on;
        self.dirty = true;
    }

    pub fn push_event(&mut self, event: InputEvent) {
        self.dirty = true;
        match event {
            InputEvent::File {
                purpose,
                name,
                bytes,
            } => return self.on_file(purpose, &name, bytes),
            InputEvent::Folder { path } => return self.on_folder(path),
            InputEvent::WriteFailed { message } => return self.on_write_failed(&message),
            InputEvent::Numpad {
                digit,
                pressed: true,
                repeat: false,
            } => return self.numpad.push(digit),
            InputEvent::Numpad { .. } => return,
            _ => {}
        }
        if let InputEvent::Focus(focused) = event {
            self.focused = focused;
        }
        if let InputEvent::Key {
            key: Key::F2,
            pressed: true,
            ..
        } = event
        {
            self.debug = !self.debug;
        }
        self.input.translate(event, &mut self.events);
    }

    /// True if a frame should be drawn now (input arrived or a redraw was asked for).
    pub fn needs_frame(&self) -> bool {
        self.dirty
    }

    /// Lays out and draws one frame into `target`, which is `size_px` large,
    /// with `ppp` physical pixels per logical point. `now` is in seconds.
    pub fn frame(
        &mut self,
        now: f64,
        target: &wgpu::TextureView,
        size_px: [u32; 2],
        ppp: f32,
    ) -> FrameOutput {
        self.dirty = false;
        self.now = now;
        self.sculpt.set_clock(now);
        let screen = Rect::from_min_size(
            Pos2::ZERO,
            egui::vec2(size_px[0] as f32, size_px[1] as f32) / ppp,
        );
        self.panel_keys();
        let mut raw = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(now),
            events: std::mem::take(&mut self.events),
            focused: self.focused,
            ..Default::default()
        };
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(ppp);

        let ctx = self.ctx.clone();
        let mut output = ctx.run_ui(raw, |ui| self.ui(ui));
        self.input.samples.clear();
        let open_url = output
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
                _ => None,
            });

        // A remesh or import from anywhere in the UI: new GPU buffers before
        // uploading.
        self.sync_clay();
        let resized = self.fit_views(ppp);
        if let Some(played) = self.playback.as_mut().and_then(|p| p.played.as_mut()) {
            let sculpt = &mut played.replay.sculpt;
            for range in sculpt.take_dirty() {
                played.gpu_mesh.write(&self.queue, &sculpt.mesh, range);
            }
        } else {
            for range in self.sculpt.take_dirty() {
                self.gpu_mesh.write(&self.queue, &self.sculpt.mesh, range);
            }
        }
        let palette = self.theme.palette();
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.render_views(&mut encoder);

        for (id, deltas) in output.textures_delta.set.drain() {
            for delta in deltas {
                self.renderer
                    .update_texture(&self.device, &self.queue, id, &delta);
            }
        }
        let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen_desc = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size_px,
            pixels_per_point: ppp,
        };
        let mut buffers = self.renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &jobs,
            &screen_desc,
        );
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(render::clear_color(palette.bg)),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            self.renderer
                .render(&mut pass.forget_lifetime(), &jobs, &screen_desc);
        }
        buffers.push(encoder.finish());
        self.queue.submit(buffers);
        for id in output.textures_delta.free.drain() {
            self.renderer.free_texture(&id);
        }

        let repaint_delay = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.repaint_delay.as_secs_f64());
        let repaint_after = if resized {
            Some(0.0)
        } else {
            repaint_delay.filter(|d| d.is_finite() && *d < 1e6)
        };
        if repaint_after == Some(0.0) {
            self.dirty = true;
        }
        FrameOutput {
            cursor: output.platform_output.cursor_icon,
            repaint_after,
            quit: std::mem::take(&mut self.quit),
            open_url,
            requests: std::mem::take(&mut self.requests),
        }
    }

    /// Asks the shell for something it alone can do; it goes out with the
    /// next frame.
    #[allow(dead_code)] // until the first part asks for a file
    fn request(&mut self, request: Request) {
        self.requests.push(request);
        self.dirty = true;
    }

    /// Shows a short message at the bottom of the views for a few seconds.
    fn notify(&mut self, text: impl Into<String>) {
        // Timed from the frame that first shows it: `now` is the last
        // frame's clock, which can be long ago when the app was idle.
        self.notice = Some((text.into(), None));
        self.dirty = true;
    }

    fn render_views(&self, encoder: &mut wgpu::CommandEncoder) {
        // A hidden view's target is 1×1 and not seen: the reference isn't
        // drawn (nor sorted) into it.
        let [sculpt_shown, reference_shown] = self.view_rects.map(views::shown);
        let sculpt = &self.views[SCULPT];
        {
            let mut pass = sculpt.begin_pass(encoder, self.view_background(SCULPT));
            let camera = &self.cameras[SCULPT];
            self.draw_sculpt(
                &mut pass,
                Slot::SculptView,
                camera,
                sculpt.size,
                sculpt_shown,
            );
        }
        let mut pass = self.views[REFERENCE].begin_pass(encoder, self.view_background(REFERENCE));
        if reference_shown {
            self.draw_reference(&mut pass);
        }
    }

    /// Draws the sculpt view's clay (the timelapse's while it plays) into
    /// `slot`, a target of `size` pixels, seen with `camera`; with `aids`,
    /// also what the view draws over it: the wireframe and the ghost. The
    /// screenshot draws it again, larger.
    fn draw_sculpt(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        slot: Slot,
        camera: &OrbitCamera,
        size: [u32; 2],
        aids: bool,
    ) {
        let aspect = size[0] as f32 / size[1].max(1) as f32;
        self.clay
            .prepare(&self.queue, slot, camera, aspect, &self.sculpt_look());
        // The timelapse's clay; the live one while an export holds it back.
        let mesh = self
            .playback
            .as_ref()
            .and_then(|p| p.played.as_ref())
            .map_or(&self.gpu_mesh, |p| &p.gpu_mesh);
        self.clay.draw(pass, slot, mesh);
        if aids {
            self.draw_wireframe(pass, slot);
            self.draw_ghost(pass, slot, camera, size);
        }
    }

    fn fit_views(&mut self, ppp: f32) -> bool {
        let mut resized = false;
        for (view, rect) in self.views.iter_mut().zip(self.view_rects) {
            let size = [
                (rect.width() * ppp).round() as u32,
                (rect.height() * ppp).round() as u32,
            ];
            resized |= view.fit(&self.device, &mut self.renderer, size);
        }
        resized
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let p = self.theme.palette();
        self.shortcuts(ui);
        egui::Panel::top("bar")
            .frame(
                Frame::NONE
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(20, 4)),
            )
            .show(ui, |ui| self.bar(ui));
        let panel_frame = Frame::NONE.fill(p.bg).inner_margin(Margin {
            left: 20,
            right: 16,
            top: 14,
            bottom: 16,
        });
        // Hidden or not, the panels' work goes on each frame: what the
        // keys asked for, the timelapse, the reference's mesh.
        self.panels.settle(self.study.layout);
        self.sculpt_upkeep(ui);
        // A panel with none of its groups left (from the window menu) gives
        // its width to the views; the timelapse's controls keep the left one.
        let sculpt_panel = self.playback.is_some() || self.panels.holds_any(&Group::SCULPT);
        let reference_panel = self.panels.holds_any(&Group::REFERENCE);
        if !self.panels.hidden() && sculpt_panel {
            egui::Panel::left("sculpt tools")
                .exact_size(PANEL_WIDTH)
                .resizable(false)
                .frame(panel_frame)
                .show(ui, |ui| {
                    // Taller than a laptop screen once every tool is there.
                    ui.spacing_mut().scroll = reference::thin_scroll_bar();
                    egui::ScrollArea::vertical()
                        .id_salt("sculpt panel")
                        .auto_shrink([false; 2])
                        .show(ui, |ui| self.sculpt_panel(ui));
                });
        }
        if !self.panels.hidden() && reference_panel {
            egui::Panel::right("reference tools")
                .exact_size(PANEL_WIDTH)
                .resizable(false)
                .frame(panel_frame)
                .show(ui, |ui| self.reference_panel(ui));
        }
        self.sculpt.refresh();
        self.sync_clay();
        self.refresh_mesh();
        egui::CentralPanel::no_frame().show(ui, |ui| self.views_ui(ui));
        self.windows(ui);
        self.notice_ui(ui);
    }

    fn notice_ui(&mut self, ui: &egui::Ui) {
        let now = self.now;
        let Some((text, posted)) = &mut self.notice else {
            return;
        };
        let left = NOTICE_SECONDS - (now - *posted.get_or_insert(now));
        if left <= 0.0 {
            self.notice = None;
            return;
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(left));
        let p = self.theme.palette();
        egui::Area::new(egui::Id::unique("notice"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
            .interactable(false)
            .show(ui.ctx(), |ui| {
                Frame::NONE
                    .fill(p.bg)
                    .stroke(egui::Stroke::new(1.0, p.border))
                    .inner_margin(Margin::symmetric(14, 8))
                    .show(ui, |ui| ui.label(text.as_str()));
            });
    }
}
