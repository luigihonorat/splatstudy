// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The application: owns the editor state, the egui context and the GPU
//! resources, takes input events and draws frames.

use egui::{
    Align2, Color32, CursorIcon, Frame, Key, Margin, PointerButton, Pos2, Rect, RichText, Sense,
    Stroke,
};
use glam::{Vec2, Vec3};

use crate::camera::OrbitCamera;
use crate::input::{Button, InputEvent, InputState, Modifiers, PointerKind, PointerPhase};
use crate::navigation::{NavAction, NavStyle};
use crate::render::{self, ViewTarget, clay::ClayPass, clay::GpuMesh};
use crate::sculpt::{Brush, DEFAULT_LEVEL, Dab, Hit, Mesh, Sculpt};
use crate::ui::{self, fonts, i18n::Lang, i18n::Tr, menu, theme, theme::ThemeMode};

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
}

/// The browser keeps a tab open and owns some shortcuts (Ctrl+N), so the
/// web build leaves those items out.
const WEB: bool = cfg!(target_arch = "wasm32");
const ISSUES_URL: &str = "https://github.com/luigihonorat/splatstudy/issues";
const REPO_URL: &str = "https://github.com/luigihonorat/splatstudy";
const SITE_URL: &str = "https://splatstudy.com";
/// Every build ships the notices; the web page has them next to it.
const LICENCES_URL: &str = if WEB {
    "licenses.html"
} else {
    "https://splatstudy.com/licenses.html"
};

const SCULPT: usize = 0;
const REFERENCE: usize = 1;

/// Navigation speeds: dolly per point dragged, and per point scrolled.
const DOLLY_DRAG: f32 = 0.005;
const DOLLY_WHEEL: f32 = 0.0015;
/// Width of the tool panels, in points.
const PANEL_WIDTH: f32 = 196.0;
/// Dabs are placed every this fraction of the tool radius along a stroke.
const SPACING: f32 = 0.15;

struct ToolSettings {
    tool: Brush,
    /// Radius on screen, in points.
    radius: f32,
    /// Strength per tool at full pen pressure, 0..1, in `Brush` order.
    strength: [f32; 3],
    symmetry: bool,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            tool: Brush::Clay,
            radius: 48.0,
            strength: [0.5, 0.5, 1.0],
            symmetry: true,
        }
    }
}

fn tool_index(tool: Brush) -> usize {
    match tool {
        Brush::Clay => 0,
        Brush::Smooth => 1,
        Brush::Move => 2,
    }
}

/// Preset viewpoints: label, key, yaw and pitch. Right and left are the
/// viewer's, as seen in the front view.
fn viewpoints(tr: &Tr) -> [(&'static str, Option<Key>, f32, f32); 6] {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};
    [
        (tr.front, Some(Key::Num1), 0.0, 0.0),
        (tr.back, None, PI, 0.0),
        (tr.left, None, -FRAC_PI_2, 0.0),
        (tr.right, Some(Key::Num3), FRAC_PI_2, 0.0),
        (tr.top, Some(Key::Num7), 0.0, FRAC_PI_2),
        (tr.three_quarter, None, FRAC_PI_4, 0.3),
    ]
}

/// What a drag in a view is doing.
#[derive(Clone, Copy)]
enum Drag {
    /// Clay or smooth: dabs along the path. `carry` is the distance travelled
    /// since the last dab.
    Stroke {
        last: Pos2,
        pressure: f32,
        carry: f32,
    },
    /// Move: the surface follows the pointer within the view plane.
    Grab {
        start: Pos2,
        right: Vec3,
        up: Vec3,
        scale: f32,
    },
    /// Turning, panning or zooming the view while `button` is held.
    Navigate {
        action: NavAction,
        button: PointerButton,
    },
}

pub struct App {
    device: wgpu::Device,
    queue: wgpu::Queue,
    ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    input: InputState,
    events: Vec<egui::Event>,
    focused: bool,
    lang: Lang,
    theme: ThemeMode,
    nav: NavStyle,
    /// Shortcuts read Cmd instead of Ctrl.
    mac: bool,
    show_shortcuts: bool,
    show_about: bool,
    quit: bool,
    debug: bool,
    /// Share of the central width given to the sculpt view.
    split: f32,
    views: [ViewTarget; 2],
    /// Where each view sits, in points; set by the last layout.
    view_rects: [Rect; 2],
    cameras: [OrbitCamera; 2],
    drags: [Option<Drag>; 2],
    clay: ClayPass,
    sculpt: Sculpt,
    gpu_mesh: GpuMesh,
    tool: ToolSettings,
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
        let sculpt = Sculpt::new(Mesh::icosphere(DEFAULT_LEVEL));
        let gpu_mesh = GpuMesh::new(&device, &sculpt.mesh);
        let clay = ClayPass::new(&device);
        Self {
            device,
            queue,
            ctx,
            renderer,
            input: InputState::default(),
            events: Vec::new(),
            focused: true,
            lang,
            theme,
            nav: NavStyle::default(),
            mac: cfg!(target_os = "macos"),
            show_shortcuts: false,
            show_about: false,
            quit: false,
            debug: false,
            split: 0.5,
            views,
            view_rects: [Rect::NOTHING; 2],
            cameras: [OrbitCamera::default(); 2],
            drags: [None; 2],
            clay,
            sculpt,
            gpu_mesh,
            tool: ToolSettings::default(),
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
        self.dirty = true;
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
        let screen = Rect::from_min_size(
            Pos2::ZERO,
            egui::vec2(size_px[0] as f32, size_px[1] as f32) / ppp,
        );
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

        let resized = self.fit_views(ppp);
        for range in self.sculpt.take_dirty() {
            self.gpu_mesh.write(&self.queue, &self.sculpt.mesh, range);
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
        }
    }

    fn render_views(&self, encoder: &mut wgpu::CommandEncoder) {
        let sculpt = &self.views[SCULPT];
        self.clay.prepare(
            &self.queue,
            &self.cameras[SCULPT],
            sculpt.aspect(),
            theme::CLAY,
        );
        {
            let mut pass = sculpt.begin_pass(encoder, theme::VIEW_BG);
            self.clay.draw(&mut pass, &self.gpu_mesh);
        }
        // Nothing loaded in the reference view yet: just its background.
        drop(self.views[REFERENCE].begin_pass(encoder, theme::VIEW_BG));
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
        egui::Panel::left("sculpt tools")
            .exact_size(PANEL_WIDTH)
            .resizable(false)
            .frame(panel_frame)
            .show(ui, |ui| self.sculpt_panel(ui));
        egui::Panel::right("reference tools")
            .exact_size(PANEL_WIDTH)
            .resizable(false)
            .frame(panel_frame)
            .show(ui, |ui| self.reference_panel(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| self.views_ui(ui));
        self.windows(ui);
    }

    fn shortcuts(&mut self, ui: &egui::Ui) {
        use egui::Modifiers as M;
        let pressed = |m: M, key: Key| ui.input_mut(|i| i.consume_key(m, key));
        if pressed(M::COMMAND | M::SHIFT, Key::Z) || pressed(M::COMMAND, Key::Y) {
            self.sculpt.redo();
        } else if pressed(M::COMMAND, Key::Z) {
            self.sculpt.undo();
        }
        if pressed(M::NONE, Key::OpenBracket) {
            self.tool.radius = (self.tool.radius / 1.2).max(4.0);
        }
        if pressed(M::NONE, Key::CloseBracket) {
            self.tool.radius = (self.tool.radius * 1.2).min(400.0);
        }
        for (_, key, yaw, pitch) in viewpoints(self.lang.tr()) {
            if key.is_some_and(|key| pressed(M::NONE, key)) {
                self.view_from(yaw, pitch);
            }
        }
        if pressed(M::NONE, Key::F) {
            self.frame_sculpt();
        }
        if pressed(M::NONE, Key::Home) {
            self.reset_view();
        }
        if !WEB && pressed(M::COMMAND, Key::N) {
            self.new_scene();
        }
        if !WEB && pressed(M::COMMAND, Key::Q) {
            self.quit = true;
        }
        self.sculpt.refresh();
    }

    /// A shortcut with the platform's command key, as menus show it.
    fn keys(&self, key: &str) -> String {
        format!("{}+{key}", if self.mac { "Cmd" } else { "Ctrl" })
    }

    fn bar(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(tr.menu_file, |ui| self.file_menu(ui));
            ui.menu_button(tr.menu_edit, |ui| self.edit_menu(ui));
            ui.menu_button(tr.menu_view, |ui| self.view_menu(ui));
            ui.menu_button(tr.navigation, |ui| self.navigation_menu(ui));
            ui.menu_button(tr.menu_language, |ui| self.language_menu(ui));
            ui.menu_button(tr.menu_help, |ui| self.help_menu(ui));
        });
    }

    // Items that do nothing yet are listed disabled, so the menus keep their
    // shape while the features land. They get a shortcut when they work.

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui.set_min_width(menu::MIN_WIDTH);
        // Browsers keep Ctrl+N for a new window.
        let new_keys = if WEB { String::new() } else { self.keys("N") };
        if menu::action(ui, true, tr.new_scene, &new_keys) {
            self.new_scene();
        }
        menu::action(ui, false, tr.open_scene, "");
        if !WEB {
            menu::action(ui, false, tr.open_recent, "");
        }
        menu::action(ui, false, tr.save_scene, "");
        menu::action(ui, false, tr.save_scene_as, "");
        menu::action(ui, false, tr.revert, "");
        ui.separator();
        menu::action(ui, false, tr.import_reference, "");
        menu::action(ui, false, tr.import_sculpt, "");
        ui.separator();
        menu::submenu(ui, tr.export, |ui| {
            ui.set_min_width(120.0);
            menu::action(ui, false, "OBJ…", "");
            menu::action(ui, false, "STL…", "");
        });
        if !WEB {
            ui.separator();
            if menu::action(ui, true, tr.exit, &self.keys("Q")) {
                self.quit = true;
            }
        }
    }

    fn edit_menu(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui.set_min_width(menu::MIN_WIDTH);
        if menu::action(ui, self.sculpt.can_undo(), tr.undo, &self.keys("Z")) {
            self.sculpt.undo();
        }
        if menu::action(ui, self.sculpt.can_redo(), tr.redo, &self.keys("Shift+Z")) {
            self.sculpt.redo();
        }
        ui.separator();
        menu::action(ui, false, tr.mirror_sculpt, "");
        menu::action(ui, false, tr.remove_reference, "");
        ui.separator();
        menu::action(ui, false, tr.preferences, "");
    }

    fn view_menu(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui.set_min_width(menu::MIN_WIDTH);
        menu::action(ui, false, tr.link_cameras, "");
        menu::submenu(ui, tr.viewpoints, |ui| {
            ui.set_min_width(160.0);
            for (label, key, yaw, pitch) in viewpoints(tr) {
                if menu::action(ui, true, label, key.map_or("", Key::name)) {
                    self.view_from(yaw, pitch);
                }
            }
        });
        if menu::action(ui, true, tr.frame_sculpt, "F") {
            self.frame_sculpt();
        }
        if menu::action(ui, true, tr.reset_view, "Home") {
            self.reset_view();
        }
        ui.separator();
        for label in [
            tr.silhouette,
            tr.ghost,
            tr.mirror_view,
            tr.plumb_line,
            tr.grid,
        ] {
            menu::action(ui, false, label, "");
        }
        ui.separator();
        if menu::action(ui, true, tr.switch_theme, "") {
            self.theme = self.theme.toggled();
            ui::theme::apply(ui.ctx(), self.theme);
        }
    }

    fn navigation_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(menu::MIN_WIDTH);
        for style in NavStyle::ALL {
            if menu::item(ui, style.name(), "", self.nav == style).clicked() {
                self.nav = style;
            }
        }
        ui.separator();
        menu::note(ui, self.nav_hint());
    }

    fn language_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(menu::MIN_WIDTH);
        for lang in Lang::ALL {
            if menu::item(ui, lang.native_name(), "", self.lang == lang).clicked() {
                self.lang = lang;
            }
        }
    }

    fn help_menu(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui.set_min_width(menu::MIN_WIDTH);
        if menu::action(ui, true, tr.shortcuts, "") {
            self.show_shortcuts = true;
        }
        menu::action(ui, false, tr.getting_started, "");
        if menu::action(ui, true, tr.report_issue, "") {
            ui.ctx().open_url(egui::OpenUrl::new_tab(ISSUES_URL));
        }
        if menu::action(ui, true, tr.licences, "") {
            ui.ctx().open_url(egui::OpenUrl::new_tab(LICENCES_URL));
        }
        ui.separator();
        if menu::action(ui, true, tr.about, "") {
            self.show_about = true;
        }
    }

    /// The keyboard shortcuts and about windows, opened from the help menu.
    fn windows(&mut self, ui: &egui::Ui) {
        let tr = self.lang.tr();
        // Wide enough that the centred title isn't cut short.
        let window = |title: &'static str, width: f32| {
            egui::Window::new(title)
                .collapsible(false)
                .resizable(false)
                .min_width(width)
                .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
        };

        let mut open = self.show_shortcuts;
        window(tr.shortcuts, 420.0)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                let muted = ui.visuals().widgets.inactive.fg_stroke.color;
                let mut rows = vec![
                    (tr.undo, self.keys("Z")),
                    (tr.redo, self.keys("Shift+Z")),
                    (tr.size, "[  ]".to_owned()),
                ];
                for (label, key, _, _) in viewpoints(tr) {
                    if let Some(key) = key {
                        rows.push((label, key.name().to_owned()));
                    }
                }
                rows.push((tr.frame_sculpt, "F".to_owned()));
                rows.push((tr.reset_view, "Home".to_owned()));
                if !WEB {
                    rows.push((tr.new_scene, self.keys("N")));
                    rows.push((tr.exit, self.keys("Q")));
                }
                egui::Grid::new("shortcuts")
                    .num_columns(2)
                    .spacing([32.0, 6.0])
                    .show(ui, |ui| {
                        for (label, keys) in rows {
                            ui.label(label);
                            ui.label(RichText::new(keys).color(muted));
                            ui.end_row();
                        }
                    });
                ui::section(ui, tr.navigation);
                ui.label(RichText::new(self.nav_hint()).color(muted));
            });
        self.show_shortcuts = open;

        let mut open = self.show_about;
        window(tr.about, 340.0)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.set_max_width(320.0);
                let muted = ui.visuals().widgets.inactive.fg_stroke.color;
                ui.label(tr.about_text);
                ui.label(
                    RichText::new(format!(
                        "splatstudy {} · GPL-3.0-or-later",
                        env!("CARGO_PKG_VERSION")
                    ))
                    .color(muted),
                );
                for (text, url) in [
                    ("splatstudy.com", SITE_URL),
                    ("github.com/luigihonorat/splatstudy", REPO_URL),
                ] {
                    if ui::choice(ui, text, false).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    }
                }
            });
        self.show_about = open;
    }

    fn nav_hint(&self) -> &'static str {
        let tr = self.lang.tr();
        match self.nav {
            NavStyle::ZBrush => tr.hint_zbrush,
            NavStyle::Blender => tr.hint_blender,
            NavStyle::Maya => tr.hint_maya,
        }
    }

    /// A fresh sphere seen from the default view.
    fn new_scene(&mut self) {
        self.sculpt = Sculpt::new(Mesh::icosphere(DEFAULT_LEVEL));
        self.gpu_mesh = GpuMesh::new(&self.device, &self.sculpt.mesh);
        self.reset_view();
    }

    /// Turns both views to the same direction, so they can be compared.
    fn view_from(&mut self, yaw: f32, pitch: f32) {
        for camera in &mut self.cameras {
            camera.look_from(yaw, pitch);
        }
    }

    fn reset_view(&mut self) {
        self.cameras = [OrbitCamera::default(); 2];
    }

    /// Fits the sculpt view to the clay's bounding sphere.
    fn frame_sculpt(&mut self) {
        let positions = &self.sculpt.mesh.positions;
        if positions.is_empty() {
            return;
        }
        let (min, max) = positions
            .iter()
            .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), &p| {
                (lo.min(p), hi.max(p))
            });
        let centre = (min + max) / 2.0;
        let radius = positions
            .iter()
            .map(|p| p.distance(centre))
            .fold(0.0, f32::max);
        self.cameras[SCULPT].frame(centre, radius);
    }

    fn sculpt_panel(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui::section(ui, tr.tools);
        for (tool, label) in [
            (Brush::Clay, tr.clay),
            (Brush::Smooth, tr.smooth),
            (Brush::Move, tr.move_),
        ] {
            if ui::choice(ui, label, self.tool.tool == tool).clicked() {
                self.tool.tool = tool;
            }
        }
        ui.add_space(10.0);
        ui::slider(ui, tr.size, &mut self.tool.radius, 4.0..=400.0, |v| {
            format!("{v:.0}")
        });
        let strength = &mut self.tool.strength[tool_index(self.tool.tool)];
        ui::slider(ui, tr.strength, strength, 0.0..=1.0, |v| {
            format!("{:.0}%", v * 100.0)
        });
        ui.add_space(10.0);
        if ui::choice(ui, tr.symmetry, self.tool.symmetry).clicked() {
            self.tool.symmetry = !self.tool.symmetry;
        }
        ui.add_space(10.0);
        ui::section(ui, tr.history);
        let (can_undo, can_redo) = (self.sculpt.can_undo(), self.sculpt.can_redo());
        if ui
            .add_enabled_ui(can_undo, |ui| ui::choice(ui, tr.undo, false))
            .inner
            .clicked()
        {
            self.sculpt.undo();
        }
        if ui
            .add_enabled_ui(can_redo, |ui| ui::choice(ui, tr.redo, false))
            .inner
            .clicked()
        {
            self.sculpt.redo();
        }
        if ui::choice(ui, tr.new_sphere, false).clicked() {
            self.new_scene();
        }
        self.sculpt.refresh();
    }

    fn reference_panel(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui::section(ui, tr.reference_tools);
        // Loading references comes next; the control shows where it will be.
        ui.add_enabled_ui(false, |ui| ui::choice(ui, tr.open_reference, false));
    }

    fn views_ui(&mut self, ui: &mut egui::Ui) {
        let p = self.theme.palette();
        let tr = self.lang.tr();
        let area = ui.max_rect();

        // The divider between the two views, draggable.
        let x = area.left() + (area.width() * self.split).round();
        let handle = Rect::from_x_y_ranges(x - 4.0..=x + 4.0, area.y_range());
        let handle = ui.interact(handle, ui.scope_id().with("split"), Sense::drag());
        if handle.dragged()
            && let Some(pos) = handle.interact_pointer_pos()
        {
            self.split = ((pos.x - area.left()) / area.width()).clamp(0.2, 0.8);
        }
        if handle.hovered() || handle.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }

        let left = Rect::from_x_y_ranges(area.left()..=x, area.y_range());
        let right = Rect::from_x_y_ranges(x + 1.0..=area.right(), area.y_range());
        self.view_rects = [left, right];
        self.view_input(ui, left, SCULPT);
        self.view_input(ui, right, REFERENCE);

        let painter = ui.painter();
        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        for (view, rect) in self.views.iter().zip([left, right]) {
            if let Some(id) = view.egui_id {
                painter.image(id, rect, uv, Color32::WHITE);
            }
        }
        painter.hline(area.x_range(), area.top() + 0.5, Stroke::new(1.0, p.border));
        painter.vline(x + 0.5, area.y_range(), Stroke::new(1.0, p.border));

        let label_font = fonts::regular(12.0);
        for (rect, text) in [(left, tr.sculpt), (right, tr.reference)] {
            painter.text(
                rect.left_top() + egui::vec2(14.0, 12.0),
                Align2::LEFT_TOP,
                text,
                label_font.clone(),
                theme::VIEW_TEXT,
            );
        }
        // Empty reference view: say what can go there.
        painter.with_clip_rect(right).text(
            right.center(),
            Align2::CENTER_CENTER,
            tr.drop_hint,
            fonts::light(16.0),
            theme::VIEW_TEXT,
        );

        self.tool_cursor(ui, left);
        if self.debug {
            self.debug_hud(ui, left);
        }
    }

    /// Pointer input in one view: the navigation preset decides what each
    /// press starts; in the sculpt view a left press on the clay that the
    /// preset leaves free sculpts. The reference view has nothing to sculpt,
    /// so a free left drag turns it.
    fn view_input(&mut self, ui: &egui::Ui, rect: Rect, index: usize) {
        let response = ui.interact(
            rect,
            ui.scope_id().with(("view", index)),
            Sense::click_and_drag(),
        );
        let (m, pointer, delta, scroll) = ui.input(|i| {
            let m = Modifiers {
                alt: i.modifiers.alt,
                ctrl: i.modifiers.ctrl,
                shift: i.modifiers.shift,
                command: i.modifiers.command,
            };
            (
                m,
                i.pointer.interact_pos(),
                i.pointer.delta(),
                i.smooth_scroll_delta.y,
            )
        });

        if self.drags[index].is_none() && response.hovered() {
            for (button, ours) in [
                (PointerButton::Primary, Button::Primary),
                (PointerButton::Middle, Button::Middle),
                (PointerButton::Secondary, Button::Secondary),
            ] {
                if !ui.input(|i| i.pointer.button_pressed(button)) {
                    continue;
                }
                let hit = match (index, ours, pointer) {
                    (SCULPT, Button::Primary, Some(pos)) => self.pick(rect, pos).map(|h| (pos, h)),
                    _ => None,
                };
                match (self.nav.action(ours, m, hit.is_some()), hit) {
                    (Some(action), _) => {
                        self.drags[index] = Some(Drag::Navigate { action, button })
                    }
                    (None, Some((pos, hit))) => self.start_stroke(rect, pos, hit),
                    (None, None) if index == REFERENCE && ours == Button::Primary => {
                        self.drags[index] = Some(Drag::Navigate {
                            action: NavAction::Orbit,
                            button,
                        });
                    }
                    (None, None) => {}
                }
                break;
            }
        }

        match self.drags[index] {
            Some(Drag::Navigate { action, button })
                if ui.input(|i| i.pointer.button_down(button)) =>
            {
                let camera = &mut self.cameras[index];
                match action {
                    NavAction::Orbit => camera.orbit(delta.x, delta.y),
                    NavAction::Pan => camera.pan(delta.x, delta.y, rect.height()),
                    NavAction::Zoom => camera.dolly((delta.y * DOLLY_DRAG).exp()),
                }
            }
            Some(Drag::Stroke { .. }) if ui.input(|i| i.pointer.primary_down()) => {
                let samples: Vec<_> = self
                    .input
                    .samples
                    .iter()
                    .filter(|s| {
                        matches!(s.phase, PointerPhase::Move | PointerPhase::Down)
                            && s.pressure > 0.0
                    })
                    .map(|s| (egui::pos2(s.pos[0], s.pos[1]), s.pressure))
                    .collect();
                for (pos, pressure) in samples {
                    self.stroke_to(rect, pos, pressure);
                }
            }
            Some(Drag::Grab {
                start,
                right,
                up,
                scale,
            }) if ui.input(|i| i.pointer.primary_down()) => {
                if let Some(pos) = pointer {
                    let d = pos - start;
                    self.sculpt.drag((right * d.x - up * d.y) * scale);
                }
            }
            Some(_) => {
                self.sculpt.end_stroke();
                self.drags[index] = None;
            }
            None => {}
        }

        if response.hovered() && scroll != 0.0 {
            self.cameras[index].dolly((-scroll * DOLLY_WHEEL).exp());
        }
        if index == SCULPT {
            self.sculpt.refresh();
        }
    }

    fn pressure(&self) -> f32 {
        self.input.last.map_or(1.0, |e| {
            if e.kind == PointerKind::Mouse {
                1.0
            } else {
                e.pressure
            }
        })
    }

    fn start_stroke(&mut self, rect: Rect, pos: Pos2, hit: Hit) {
        self.sculpt.begin_stroke();
        let camera = &self.cameras[SCULPT];
        let scale = camera.world_per_point(hit.point, rect.height());
        if self.tool.tool == Brush::Move {
            let (right, up) = camera.right_up();
            self.sculpt
                .grab(hit.point, self.tool.radius * scale, self.tool.symmetry);
            self.drags[SCULPT] = Some(Drag::Grab {
                start: pos,
                right,
                up,
                scale,
            });
        } else {
            let pressure = self.pressure();
            self.drags[SCULPT] = Some(Drag::Stroke {
                last: pos,
                pressure,
                carry: 0.0,
            });
            self.dab_at(rect, pressure, hit);
        }
    }

    /// Continues a clay or smooth stroke to `to`, placing dabs at even
    /// spacing and interpolating the pressure between samples.
    fn stroke_to(&mut self, rect: Rect, to: Pos2, pressure: f32) {
        let Some(Drag::Stroke {
            last,
            pressure: last_pressure,
            carry,
        }) = self.drags[SCULPT]
        else {
            return;
        };
        let spacing = (self.tool.radius * SPACING).max(1.0);
        let length = last.distance(to);
        let mut along = spacing - carry;
        while along <= length {
            let t = along / length.max(1e-6);
            if let Some(hit) = self.pick(rect, last.lerp(to, t)) {
                self.dab_at(rect, last_pressure + (pressure - last_pressure) * t, hit);
            }
            along += spacing;
        }
        let carry = length - (along - spacing);
        self.drags[SCULPT] = Some(Drag::Stroke {
            last: to,
            pressure,
            carry,
        });
    }

    /// One dab: pen pressure scales the tool's strength.
    fn dab_at(&mut self, rect: Rect, pressure: f32, hit: Hit) {
        let (ctrl, shift) = self.ctx.input(|i| (i.modifiers.ctrl, i.modifiers.shift));
        let tool = if shift { Brush::Smooth } else { self.tool.tool };
        let strength = self.tool.strength[tool_index(tool)] * pressure.clamp(0.0, 1.0);
        let scale = self.cameras[SCULPT].world_per_point(hit.point, rect.height());
        self.sculpt.dab(Dab {
            brush: tool,
            centre: hit.point,
            radius: self.tool.radius * scale,
            strength,
            invert: ctrl,
            symmetry: self.tool.symmetry,
        });
    }

    /// The clay under a point of the sculpt view.
    fn pick(&self, rect: Rect, pos: Pos2) -> Option<Hit> {
        let ndc = Vec2::new(
            (pos.x - rect.left()) / rect.width() * 2.0 - 1.0,
            1.0 - (pos.y - rect.top()) / rect.height() * 2.0,
        );
        let (origin, dir) = self.cameras[SCULPT].ray(ndc, rect.width() / rect.height());
        self.sculpt.raycast(origin, dir)
    }

    /// The tool outline around the pointer, while it is over the clay view.
    fn tool_cursor(&self, ui: &egui::Ui, rect: Rect) {
        let Some(pos) = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| rect.contains(*p))
        else {
            return;
        };
        if matches!(self.drags[SCULPT], Some(Drag::Navigate { .. })) {
            return;
        }
        let ink = theme::VIEW_INK.gamma_multiply(0.6);
        let painter = ui.painter().with_clip_rect(rect);
        painter.circle_stroke(pos, self.tool.radius, Stroke::new(1.0, ink));
        painter.circle_filled(pos, 1.5, ink);
    }

    fn debug_hud(&self, ui: &egui::Ui, rect: Rect) {
        let text = match self.input.last {
            Some(e) => {
                let kind = match e.kind {
                    PointerKind::Mouse => "mouse",
                    PointerKind::Pen if e.eraser => "pen (eraser)",
                    PointerKind::Pen => "pen",
                    PointerKind::Touch => "touch",
                };
                format!("{kind}  pressure {:.3}  {:?}", e.pressure, e.phase)
            }
            None => "no pointer yet".to_owned(),
        };
        ui.painter().text(
            rect.left_bottom() + egui::vec2(14.0, -12.0),
            Align2::LEFT_BOTTOM,
            text,
            fonts::regular(12.0),
            theme::VIEW_INK,
        );
    }
}
