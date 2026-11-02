// SPDX-License-Identifier: GPL-3.0-or-later WITH AdditionRef-SplatStudy-app-store-exception
//! The application: owns the editor state, the egui context and the GPU
//! resources, takes input events and draws frames.

use egui::{
    Align, Align2, Color32, CursorIcon, Frame, Layout, Margin, Rect, RichText, Sense, Stroke,
};

use crate::input::{InputEvent, InputState, PointerKind};
use crate::render::{self, ViewTarget};
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
        let views = [
            ViewTarget::new(&device, &mut renderer, [1, 1]),
            ViewTarget::new(&device, &mut renderer, [1, 1]),
        ];
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
        for view in &self.views {
            clear(&mut encoder, &view.view, palette.vp_bg);
        }

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

        let painter = ui.painter();
        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        for (view, rect) in self.views.iter().zip([left, right]) {
            painter.image(view.egui_id, rect, uv, Color32::WHITE);
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
        let _ = (SCULPT, REFERENCE);
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

fn clear(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, color: Color32) {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("clear view"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(render::clear_color(color)),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
}
