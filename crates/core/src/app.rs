// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The application: owns the editor state, the egui context and the GPU
//! resources, takes input events and draws frames.

use egui::{
    Align, Align2, Color32, CursorIcon, Frame, Layout, Margin, Rect, RichText, Sense, Stroke,
};

use crate::camera::OrbitCamera;
use crate::input::{InputEvent, InputState, PointerKind};
use crate::render::{self, ViewTarget, clay::ClayPass, clay::GpuMesh};
use crate::sculpt::{DEFAULT_LEVEL, Mesh};
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
    /// Share of the width given to the sculpt view.
    split: f32,
    views: [ViewTarget; 2],
    /// Where each view sits, in points; set by the last layout.
    view_rects: [Rect; 2],
    cameras: [OrbitCamera; 2],
    clay: ClayPass,
    #[expect(dead_code, reason = "the brushes (next step) edit it")]
    mesh: Mesh,
    gpu_mesh: GpuMesh,
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
        let mesh = Mesh::icosphere(DEFAULT_LEVEL);
        let gpu_mesh = GpuMesh::new(&device, &mesh);
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
            mesh,
            gpu_mesh,
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
            key: egui::Key::F2,
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
            egui::Pos2::ZERO,
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
        egui::Panel::top("bar")
            .frame(
                Frame::NONE
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(20, 12)),
            )
            .show(ui, |ui| self.bar(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| self.views_ui(ui));
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

        for (index, rect) in [left, right].into_iter().enumerate() {
            self.navigate(ui, rect, index);
        }

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

        if self.debug {
            self.debug_hud(ui, left);
        }
    }

    /// Houdini-style navigation: left drag turns, middle drag pans, right
    /// drag and the wheel dolly. (In the sculpt view the left button will
    /// sculpt, with Alt or Space held to navigate.)
    fn navigate(&mut self, ui: &egui::Ui, rect: Rect, index: usize) {
        let response = ui.interact(
            rect,
            ui.scope_id().with(("view", index)),
            Sense::click_and_drag(),
        );
        let delta = response.drag_delta();
        let camera = &mut self.cameras[index];
        if response.dragged_by(egui::PointerButton::Primary) {
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
    }

    fn debug_hud(&self, ui: &egui::Ui, rect: Rect) {
        let p = self.theme.palette();
        let text = match self.input.last {
            Some(e) => {
                let kind = match e.kind {
                    PointerKind::Mouse => "mouse",
                    PointerKind::Pen => {
                        if e.eraser {
                            "pen (eraser)"
                        } else {
                            "pen"
                        }
                    }
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
