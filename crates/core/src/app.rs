// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The application: owns the editor state, the egui context and the GPU
//! resources, takes input events and draws frames.

use egui::{
    Align, Align2, Color32, CursorIcon, Frame, Key, Layout, Margin, Pos2, Rect, RichText, Sense,
    Stroke,
};
use glam::{Vec2, Vec3};

use crate::camera::OrbitCamera;
use crate::input::{InputEvent, InputState, PointerKind, PointerPhase};
use crate::render::{self, ViewTarget, clay::ClayPass, clay::GpuMesh};
use crate::sculpt::{Brush, DEFAULT_LEVEL, Dab, Hit, Mesh, Sculpt};
use crate::ui::{self, fonts, i18n::Lang, theme::ThemeMode};

/// What the shell should do after a frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameOutput {
    pub cursor: CursorIcon,
    /// Seconds until the next frame is needed; `None` waits for input.
    pub repaint_after: Option<f64>,
}

const SCULPT: usize = 0;
const REFERENCE: usize = 1;

/// Navigation speeds: dolly per point dragged, and per point scrolled.
const DOLLY_DRAG: f32 = 0.005;
const DOLLY_WHEEL: f32 = 0.0015;
/// Width of the tool panels, in points.
const PANEL_WIDTH: f32 = 196.0;
/// Dabs are placed every this fraction of the brush radius along a stroke.
const SPACING: f32 = 0.15;

struct BrushSettings {
    brush: Brush,
    /// Radius on screen, in points.
    radius: f32,
    /// Strength per brush, 0..1, in `Brush` order.
    strength: [f32; 3],
    symmetry: bool,
    pressure_size: bool,
    pressure_strength: bool,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            brush: Brush::Clay,
            radius: 48.0,
            strength: [0.5, 0.5, 1.0],
            symmetry: true,
            pressure_size: false,
            pressure_strength: true,
        }
    }
}

fn brush_index(brush: Brush) -> usize {
    match brush {
        Brush::Clay => 0,
        Brush::Smooth => 1,
        Brush::Move => 2,
    }
}

/// What a left-button drag in the sculpt view is doing.
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
    /// Started on the background or with Alt/Space: turns the view.
    Orbit,
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
    debug: bool,
    /// Share of the central width given to the sculpt view.
    split: f32,
    views: [ViewTarget; 2],
    /// Where each view sits, in points; set by the last layout.
    view_rects: [Rect; 2],
    cameras: [OrbitCamera; 2],
    clay: ClayPass,
    sculpt: Sculpt,
    gpu_mesh: GpuMesh,
    brush: BrushSettings,
    drag: Option<Drag>,
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
        let theme = ThemeMode::Light;
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
            debug: false,
            split: 0.5,
            views,
            view_rects: [Rect::NOTHING; 2],
            cameras: [OrbitCamera::default(); 2],
            clay,
            sculpt,
            gpu_mesh,
            brush: BrushSettings::default(),
            drag: None,
            dirty: true,
        }
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
        }
    }

    fn render_views(&self, encoder: &mut wgpu::CommandEncoder) {
        let p = self.theme.palette();
        let sculpt = &self.views[SCULPT];
        self.clay.prepare(
            &self.queue,
            &self.cameras[SCULPT],
            sculpt.aspect(),
            p.vp_geo,
        );
        {
            let mut pass = sculpt.begin_pass(encoder, p.vp_bg);
            self.clay.draw(&mut pass, &self.gpu_mesh);
        }
        // Nothing loaded in the reference view yet: just its background.
        drop(self.views[REFERENCE].begin_pass(encoder, p.vp_bg));
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
                    .inner_margin(Margin::symmetric(20, 12)),
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
    }

    fn shortcuts(&mut self, ui: &egui::Ui) {
        let (redo, undo, smaller, larger) = ui.input_mut(|i| {
            let redo = i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::Z)
                || i.consume_key(egui::Modifiers::COMMAND, Key::Y);
            let undo = i.consume_key(egui::Modifiers::COMMAND, Key::Z);
            let smaller = i.consume_key(egui::Modifiers::NONE, Key::OpenBracket);
            let larger = i.consume_key(egui::Modifiers::NONE, Key::CloseBracket);
            (redo, undo, smaller, larger)
        });
        if redo {
            self.sculpt.redo();
        } else if undo {
            self.sculpt.undo();
        }
        if smaller {
            self.brush.radius = (self.brush.radius / 1.2).max(4.0);
        }
        if larger {
            self.brush.radius = (self.brush.radius * 1.2).min(400.0);
        }
        self.sculpt.refresh();
    }

    fn bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("splatstudy").font(fonts::regular(18.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let tr = self.lang.tr();
                let other = self.theme.toggled();
                let label = match other {
                    ThemeMode::Light => tr.theme_light,
                    ThemeMode::Dark => tr.theme_dark,
                };
                if ui::choice(ui, label, false).clicked() {
                    self.theme = other;
                    ui::theme::apply(ui.ctx(), other);
                }
                ui.add_space(8.0);
                for lang in Lang::ALL.into_iter().rev() {
                    if ui::choice(ui, lang.native_name(), lang == self.lang).clicked() {
                        self.lang = lang;
                    }
                }
            });
        });
    }

    fn sculpt_panel(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui::section(ui, tr.brush);
        for (brush, label) in [
            (Brush::Clay, tr.clay),
            (Brush::Smooth, tr.smooth),
            (Brush::Move, tr.move_),
        ] {
            if ui::choice(ui, label, self.brush.brush == brush).clicked() {
                self.brush.brush = brush;
            }
        }
        ui.add_space(10.0);
        ui::slider(ui, tr.size, &mut self.brush.radius, 4.0..=400.0, |v| {
            format!("{v:.0}")
        });
        let strength = &mut self.brush.strength[brush_index(self.brush.brush)];
        ui::slider(ui, tr.strength, strength, 0.0..=1.0, |v| {
            format!("{:.0}%", v * 100.0)
        });
        ui.add_space(10.0);
        for (label, on) in [
            (tr.symmetry, &mut self.brush.symmetry),
            (tr.pressure_size, &mut self.brush.pressure_size),
            (tr.pressure_strength, &mut self.brush.pressure_strength),
        ] {
            if ui::choice(ui, label, *on).clicked() {
                *on = !*on;
            }
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
            self.sculpt = Sculpt::new(Mesh::icosphere(DEFAULT_LEVEL));
            self.gpu_mesh = GpuMesh::new(&self.device, &self.sculpt.mesh);
        }
        self.sculpt.refresh();
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            let muted = ui.visuals().widgets.inactive.fg_stroke.color;
            ui.label(
                RichText::new(tr.navigation_hint)
                    .font(fonts::regular(11.0))
                    .color(muted),
            );
        });
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

        self.sculpt_view_input(ui, left);
        self.navigate(ui, right, REFERENCE, false);

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
                p.muted,
            );
        }
        // Empty reference view: say what can go there.
        painter.text(
            right.center(),
            Align2::CENTER_CENTER,
            tr.drop_hint,
            fonts::light(16.0),
            p.muted,
        );

        self.brush_cursor(ui, left);
        if self.debug {
            self.debug_hud(ui, left);
        }
    }

    /// Navigation with the middle and right buttons and the wheel, and with
    /// the left button when `left_turns` (Alt/Space, or the reference view).
    fn navigate(
        &mut self,
        ui: &egui::Ui,
        rect: Rect,
        index: usize,
        sculpt_view: bool,
    ) -> egui::Response {
        let response = ui.interact(
            rect,
            ui.scope_id().with(("view", index)),
            Sense::click_and_drag(),
        );
        let delta = response.drag_delta();
        let camera = &mut self.cameras[index];
        if !sculpt_view && response.dragged_by(egui::PointerButton::Primary) {
            camera.orbit(delta.x, delta.y);
        } else if response.dragged_by(egui::PointerButton::Middle) {
            camera.pan(delta.x, delta.y, rect.height());
        } else if response.dragged_by(egui::PointerButton::Secondary) {
            camera.dolly((delta.y * DOLLY_DRAG).exp());
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                camera.dolly((-scroll * DOLLY_WHEEL).exp());
            }
        }
        response
    }

    /// Left button in the sculpt view: sculpts where it lands on the clay,
    /// turns the view on the background or with Alt/Space held.
    fn sculpt_view_input(&mut self, ui: &egui::Ui, rect: Rect) {
        let response = self.navigate(ui, rect, SCULPT, true);
        let (pressed, down, navigating, pointer, delta) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.modifiers.alt || i.key_down(Key::Space),
                i.pointer.interact_pos(),
                i.pointer.delta(),
            )
        });

        if pressed && response.hovered() && self.drag.is_none() {
            self.drag = Some(Drag::Orbit);
            if !navigating
                && let Some(pos) = pointer
                && let Some(hit) = self.pick(rect, pos)
            {
                self.start_stroke(rect, pos, hit);
            }
        }

        match self.drag {
            Some(Drag::Orbit) if down => self.cameras[SCULPT].orbit(delta.x, delta.y),
            Some(Drag::Stroke { .. }) if down => {
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
            }) if down => {
                if let Some(pos) = pointer {
                    let d = pos - start;
                    self.sculpt.drag((right * d.x - up * d.y) * scale);
                }
            }
            Some(_) if !down => {
                self.sculpt.end_stroke();
                self.drag = None;
            }
            _ => {}
        }
        self.sculpt.refresh();
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
        if self.brush.brush == Brush::Move {
            let (right, up) = camera.right_up();
            self.sculpt
                .grab(hit.point, self.brush.radius * scale, self.brush.symmetry);
            self.drag = Some(Drag::Grab {
                start: pos,
                right,
                up,
                scale,
            });
        } else {
            let pressure = self.pressure();
            self.drag = Some(Drag::Stroke {
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
        }) = self.drag
        else {
            return;
        };
        let spacing = (self.brush.radius * SPACING).max(1.0);
        let length = last.distance(to);
        let mut along = spacing - carry;
        while along <= length {
            let t = along / length.max(1e-6);
            let pos = last.lerp(to, t);
            if let Some(hit) = self.pick(rect, pos) {
                self.dab_at(rect, last_pressure + (pressure - last_pressure) * t, hit);
            }
            along += spacing;
        }
        let carry = length - (along - spacing);
        self.drag = Some(Drag::Stroke {
            last: to,
            pressure,
            carry,
        });
    }

    fn dab_at(&mut self, rect: Rect, pressure: f32, hit: Hit) {
        let (ctrl, shift) = self.ctx.input(|i| (i.modifiers.ctrl, i.modifiers.shift));
        let brush = if shift {
            Brush::Smooth
        } else {
            self.brush.brush
        };
        let pressure = pressure.clamp(0.0, 1.0);
        let size = if self.brush.pressure_size {
            pressure.max(0.1)
        } else {
            1.0
        };
        let strength = self.brush.strength[brush_index(brush)]
            * if self.brush.pressure_strength {
                pressure
            } else {
                1.0
            };
        let scale = self.cameras[SCULPT].world_per_point(hit.point, rect.height());
        self.sculpt.dab(Dab {
            brush,
            centre: hit.point,
            radius: self.brush.radius * size * scale,
            strength,
            invert: ctrl,
            symmetry: self.brush.symmetry,
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

    /// The brush outline around the pointer, while it is over the clay view.
    fn brush_cursor(&self, ui: &egui::Ui, rect: Rect) {
        let (hover, navigating) = ui.input(|i| {
            (
                i.pointer.hover_pos(),
                i.modifiers.alt || i.key_down(Key::Space),
            )
        });
        let Some(pos) = hover.filter(|p| rect.contains(*p)) else {
            return;
        };
        if navigating || matches!(self.drag, Some(Drag::Orbit)) {
            return;
        }
        let p = self.theme.palette();
        let radius = if self.brush.pressure_size && self.drag.is_some() {
            self.brush.radius * self.pressure().max(0.1)
        } else {
            self.brush.radius
        };
        let painter = ui.painter().with_clip_rect(rect);
        painter.circle_stroke(pos, radius, Stroke::new(1.0, p.text.gamma_multiply(0.6)));
        painter.circle_filled(pos, 1.5, p.text.gamma_multiply(0.6));
    }

    fn debug_hud(&self, ui: &egui::Ui, rect: Rect) {
        let p = self.theme.palette();
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
            p.text,
        );
    }
}
