// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Composes exported frames offscreen: the sculpt and the reference are
//! drawn into their own targets (the `Export*` slots), then placed on the
//! frame with the 1px rule and the credit line by an egui context of the
//! compositor's own, so the app's interface never shows up in a video.
//!
//! Each frame: draw into [`Compositor::sculpt_view`] and
//! [`Compositor::reference_view`], call [`Compositor::compose`], then read
//! [`Compositor::output`] back with [`Readback`](crate::render::readback::Readback).

use egui::{Align2, Color32, FontId, Pos2, Rect, TextureId, vec2};

use super::layout::{Layout, Placement, PxRect, place};
use crate::reference::RefImage;
use crate::render::{self, VIEW_FORMAT, ViewTarget};
use crate::ui::{fonts, theme};

/// The interface is laid out as if the frame were this many points on its
/// short side, so text has the same size in every frame shape.
const SHORT_SIDE_POINTS: f32 = 540.0;
/// The credit line's distance from the reference view's corner, in points,
/// as in the app.
const CREDIT_MARGIN: egui::Vec2 = vec2(14.0, 12.0);
const CREDIT_SIZE: f32 = 11.0;

/// What goes over the views in a frame.
pub enum Overlay<'a> {
    /// The views, with the reference's credit along the bottom of its view.
    Views { credit: Option<&'a str> },
    /// The closing card, without the views.
    EndCard(&'a EndCard),
}

/// The closing card: what was studied and who made it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EndCard {
    /// The reference's title; the card shows only the footer without one.
    pub title: String,
    /// Author, licence, source, one per line.
    pub lines: Vec<String>,
    pub footer: String,
}

pub struct Compositor {
    ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    size: [u32; 2],
    ppp: f32,
    placement: Placement,
    sculpt: ViewTarget,
    reference: Option<ViewTarget>,
    sculpt_texture: TextureId,
    reference_texture: Option<TextureId>,
    output: ViewTarget,
    /// A picture shown in the reference half instead of its view.
    image: Option<egui::TextureHandle>,
    frames: u64,
}

impl Compositor {
    /// A compositor for `size` frames split by `layout`.
    pub fn new(device: &wgpu::Device, size: [u32; 2], layout: Layout) -> Self {
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts::definitions());
        theme::apply(&ctx, theme::ThemeMode::Dark);
        let mut renderer = egui_wgpu::Renderer::new(
            device,
            VIEW_FORMAT,
            egui_wgpu::RendererOptions {
                // Noise would only cost bits in the video.
                dithering: false,
                ..Default::default()
            },
        );
        let placement = place(size, layout);
        let sculpt = ViewTarget::new(device, placement.sculpt.size());
        let reference = placement
            .reference
            .map(|rect| ViewTarget::new(device, rect.size()));
        // Views are drawn pixel for pixel, so nearest is exact.
        let filter = wgpu::FilterMode::Nearest;
        let sculpt_texture =
            renderer.register_native_texture(device, &sculpt.resolved_view, filter);
        let reference_texture = reference
            .as_ref()
            .map(|r| renderer.register_native_texture(device, &r.resolved_view, filter));
        Self {
            ctx,
            renderer,
            size,
            ppp: size[0].min(size[1]) as f32 / SHORT_SIDE_POINTS,
            placement,
            sculpt,
            reference,
            sculpt_texture,
            reference_texture,
            output: output_target(device, size),
            image: None,
            frames: 0,
        }
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    pub fn placement(&self) -> &Placement {
        &self.placement
    }

    /// Shows `image` in the reference half, fitted inside it, instead of
    /// the reference view.
    pub fn set_image(&mut self, image: &RefImage) {
        let size = [image.size[0] as usize, image.size[1] as usize];
        if image.rgba.len() != size[0] * size[1] * 4 {
            return;
        }
        let pixels = egui::ColorImage::from_rgba_unmultiplied(size, &image.rgba);
        self.image = Some(
            self.ctx
                .load_texture("reference", pixels, egui::TextureOptions::LINEAR),
        );
    }

    /// Where the sculpt is drawn, before [`compose`](Self::compose).
    pub fn sculpt_view(&self) -> &ViewTarget {
        &self.sculpt
    }

    /// Where the reference is drawn; `None` when the layout leaves it out.
    pub fn reference_view(&self) -> Option<&ViewTarget> {
        self.reference.as_ref()
    }

    /// The composed frame, for reading back or showing as a preview.
    pub fn output(&self) -> &ViewTarget {
        &self.output
    }

    /// Records the composed frame into `encoder`. The returned command
    /// buffers upload egui's data and go into the same submit, before the
    /// encoder's.
    pub fn compose(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        overlay: &Overlay,
    ) -> Vec<wgpu::CommandBuffer> {
        let ppp = self.ppp;
        let screen = Rect::from_min_size(
            Pos2::ZERO,
            vec2(self.size[0] as f32, self.size[1] as f32) / ppp,
        );
        let mut raw = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(self.frames as f64 / 30.0),
            ..Default::default()
        };
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(ppp);
        self.frames += 1;

        let to_points = |r: PxRect| {
            Rect::from_min_size(
                Pos2::new(r.x as f32, r.y as f32) / ppp,
                vec2(r.w as f32, r.h as f32) / ppp,
            )
        };
        let placement = self.placement;
        let textures = (self.sculpt_texture, self.reference_texture);
        let image = self.image.as_ref().map(|i| (i.id(), i.size_vec2()));
        let ctx = self.ctx.clone();
        let mut output = ctx.run_ui(raw, |ui| {
            let painter = ui.painter();
            painter.rect_filled(screen, 0.0, theme::VIEW_BG);
            match overlay {
                Overlay::Views { credit } => {
                    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                    painter.image(textures.0, to_points(placement.sculpt), uv, Color32::WHITE);
                    if let (Some(rect), Some(texture)) = (placement.reference, textures.1) {
                        let rect = to_points(rect);
                        match image {
                            Some((id, size)) => {
                                let fitted = fit(size, rect.shrink(CREDIT_MARGIN.x));
                                painter.image(id, fitted, uv, Color32::WHITE);
                            }
                            None => {
                                painter.image(texture, rect, uv, Color32::WHITE);
                            }
                        }
                        if let Some(credit) = credit.filter(|c| !c.is_empty()) {
                            credit_line(painter, rect, credit);
                        }
                    }
                    if let Some(divider) = placement.divider {
                        painter.rect_filled(to_points(divider), 0.0, theme::DARK.border);
                    }
                }
                Overlay::EndCard(card) => end_card(painter, screen, card),
            }
        });

        for (id, deltas) in output.textures_delta.set.drain() {
            for delta in deltas {
                self.renderer.update_texture(device, queue, id, &delta);
            }
        }
        let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: self.size,
            pixels_per_point: ppp,
        };
        let buffers = self
            .renderer
            .update_buffers(device, queue, encoder, &jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("export frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.output.resolved_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(render::clear_color(theme::VIEW_BG)),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            self.renderer
                .render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        for id in output.textures_delta.free.drain() {
            self.renderer.free_texture(&id);
        }
        buffers
    }
}

/// The largest rectangle of `size`'s proportions inside `rect`, centred.
fn fit(size: egui::Vec2, rect: Rect) -> Rect {
    let scale = (rect.width() / size.x.max(1.0)).min(rect.height() / size.y.max(1.0));
    Rect::from_center_size(rect.center(), size * scale)
}

/// The credit along the bottom of the reference view, clipped to it.
fn credit_line(painter: &egui::Painter, rect: Rect, credit: &str) {
    let font = fonts::regular(CREDIT_SIZE);
    let wrap = rect.width() - 2.0 * CREDIT_MARGIN.x;
    let galley = painter.layout(credit.to_owned(), font, theme::VIEW_TEXT, wrap.max(10.0));
    let at = rect.left_bottom() + vec2(CREDIT_MARGIN.x, -CREDIT_MARGIN.y - galley.size().y);
    painter
        .with_clip_rect(rect)
        .galley(at, galley, theme::VIEW_TEXT);
}

fn end_card(painter: &egui::Painter, screen: Rect, card: &EndCard) {
    let width = (screen.width() - 80.0).min(640.0);
    let centre = screen.center().x;
    let mut y = screen.center().y - 60.0;
    let mut line = |text: &str, font: FontId, colour: Color32, gap: f32| {
        let galley = painter.layout(text.to_owned(), font, colour, width);
        let at = Pos2::new(centre - galley.size().x / 2.0, y);
        y += galley.size().y + gap;
        painter.galley(at, galley, colour);
    };
    if !card.title.is_empty() {
        line(&card.title, fonts::light(28.0), theme::VIEW_INK, 14.0);
        for text in card.lines.iter().filter(|l| !l.is_empty()) {
            line(text, fonts::regular(14.0), theme::VIEW_TEXT, 6.0);
        }
    }
    painter.text(
        Pos2::new(centre, screen.bottom() - 40.0),
        Align2::CENTER_BOTTOM,
        &card.footer,
        fonts::regular(12.0),
        theme::VIEW_TEXT,
    );
}

/// A frame-sized texture that egui draws into and the readback copies
/// from. Only the resolved texture is used: the multisampled and depth
/// views of a [`ViewTarget`] are left one pixel large.
fn output_target(device: &wgpu::Device, size: [u32; 2]) -> ViewTarget {
    let texture = |label, size: [u32; 2], samples, format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let resolved = texture(
        "export frame",
        size,
        1,
        VIEW_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
    );
    let unused = wgpu::TextureUsages::RENDER_ATTACHMENT;
    let msaa = texture("unused", [1, 1], render::SAMPLES, VIEW_FORMAT, unused);
    let depth = texture(
        "unused",
        [1, 1],
        render::SAMPLES,
        render::DEPTH_FORMAT,
        unused,
    );
    ViewTarget {
        resolved_view: resolved.create_view(&Default::default()),
        resolved,
        msaa_view: msaa.create_view(&Default::default()),
        depth_view: depth.create_view(&Default::default()),
        size,
        egui_id: None,
    }
}
