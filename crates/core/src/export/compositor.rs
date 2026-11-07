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
//!
//! A screenshot is one such frame, with what the app draws over each view
//! ([`ViewOverlay`]: the mirror, the grid, the plumb line) and a picture
//! reference panned and zoomed as in its view ([`Compositor::frame_image`]).

use egui::{Align2, Color32, FontId, Pos2, Rect, TextureId, vec2};

use super::layout::{Layout, Placement, PxRect, place};
use crate::reference::RefImage;
use crate::render::{self, VIEW_FORMAT, ViewTarget};
use crate::ui::{fonts, guides, theme};

/// The interface is laid out as if the frame were this many points on its
/// short side, so text has the same size in every frame shape.
const SHORT_SIDE_POINTS: f32 = 540.0;
/// The credit line's distance from the reference view's corner, in points,
/// as in the app.
const CREDIT_MARGIN: egui::Vec2 = vec2(14.0, 12.0);
const CREDIT_SIZE: f32 = 11.0;
/// The session clock, in the sculpt view's bottom left corner, across
/// from the credit.
const CLOCK_SIZE: f32 = 12.0;
/// The title, in the frame's top left corner.
const TITLE_MARGIN: egui::Vec2 = vec2(24.0, 20.0);
const TITLE_SIZE: f32 = 26.0;

/// What goes over the views in a frame.
pub enum Overlay<'a> {
    /// The views, with the reference's credit along the bottom of its view,
    /// the session's elapsed time in the sculpt's bottom left corner, and a
    /// title with its opacity (0..1) in the frame's top left corner.
    Views {
        credit: Option<&'a str>,
        clock: Option<&'a str>,
        title: Option<(&'a str, f32)>,
    },
    /// The closing card, without the views.
    EndCard(&'a EndCard),
}

/// What the app draws over one view, for a screenshot. Videos leave it
/// all off.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewOverlay {
    /// The view flipped left to right; the guides are not.
    pub mirror: bool,
    /// The grid's rows, when it shows.
    pub grid: Option<u32>,
    /// Where the plumb line hangs, as a share of the view's width.
    pub plumb: Option<f32>,
}

/// How the reference view pans and zooms a picture: its centre moved off
/// the view's centre by `offset` times the size at which it fits the view,
/// and shown `zoom` times that size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageFraming {
    pub offset: egui::Vec2,
    pub zoom: f32,
}

/// The share of a view a picture fills when it fits, as in the app.
pub const IMAGE_FIT: f32 = 0.95;

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
    sculpt: Option<ViewTarget>,
    reference: Option<ViewTarget>,
    sculpt_texture: Option<TextureId>,
    reference_texture: Option<TextureId>,
    output: ViewTarget,
    /// A picture shown in the reference half instead of its view.
    image: Option<egui::TextureHandle>,
    /// The picture as the reference view frames it; without, it fits
    /// inside the half.
    framing: Option<ImageFraming>,
    /// Over the sculpt, then over the reference.
    overlays: [ViewOverlay; 2],
    /// The sculpt's background, then the reference's, as in the app.
    backgrounds: [Color32; 2],
    frames: u64,
}

impl Compositor {
    /// A compositor for `size` frames split by `layout`.
    pub fn new(device: &wgpu::Device, size: [u32; 2], layout: Layout) -> Self {
        Self::placed(device, size, place(size, layout))
    }

    /// A compositor for `size` frames with the views where `placement`
    /// puts them.
    pub fn placed(device: &wgpu::Device, size: [u32; 2], placement: Placement) -> Self {
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
        let sculpt = placement
            .sculpt
            .map(|rect| ViewTarget::new(device, rect.size()));
        let reference = placement
            .reference
            .map(|rect| ViewTarget::new(device, rect.size()));
        // Views are drawn pixel for pixel, so nearest is exact.
        let filter = wgpu::FilterMode::Nearest;
        let mut register = |target: &ViewTarget| {
            renderer.register_native_texture(device, &target.resolved_view, filter)
        };
        let sculpt_texture = sculpt.as_ref().map(&mut register);
        let reference_texture = reference.as_ref().map(&mut register);
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
            framing: None,
            overlays: [ViewOverlay::default(); 2],
            backgrounds: [theme::VIEW_BG; 2],
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

    /// Pans and zooms the picture as the reference view shows it.
    pub fn frame_image(&mut self, framing: ImageFraming) {
        self.framing = Some(framing);
    }

    /// What is drawn over the sculpt and over the reference.
    pub fn set_overlays(&mut self, sculpt: ViewOverlay, reference: ViewOverlay) {
        self.overlays = [sculpt, reference];
    }

    /// The sculpt's and the reference's backgrounds, for a picture
    /// reference's margins and for the text over each: the colours their
    /// views are cleared to.
    pub fn set_backgrounds(&mut self, backgrounds: [Color32; 2]) {
        self.backgrounds = backgrounds;
    }

    /// The background the sculpt's view, then the reference's, is to be
    /// cleared to.
    pub fn backgrounds(&self) -> [Color32; 2] {
        self.backgrounds
    }

    /// Where the sculpt is drawn, before [`compose`](Self::compose);
    /// `None` when the layout leaves it out.
    pub fn sculpt_view(&self) -> Option<&ViewTarget> {
        self.sculpt.as_ref()
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
        let (framing, [sculpt_overlay, reference_overlay]) = (self.framing, self.overlays);
        let [sculpt_bg, reference_bg] = self.backgrounds;
        let ctx = self.ctx.clone();
        let mut output = ctx.run_ui(raw, |ui| {
            let painter = ui.painter();
            painter.rect_filled(screen, 0.0, theme::VIEW_BG);
            match overlay {
                Overlay::Views {
                    credit,
                    clock,
                    title,
                } => {
                    let sculpt = placement.sculpt.map(to_points);
                    let reference = placement.reference.map(to_points);
                    // Under a picture's margins, as in its view.
                    for (rect, bg) in [(sculpt, sculpt_bg), (reference, reference_bg)] {
                        if let Some(rect) = rect {
                            painter.rect_filled(rect, 0.0, bg);
                        }
                    }
                    if let (Some(rect), Some(texture)) = (sculpt, textures.0) {
                        let uv = uv(sculpt_overlay.mirror);
                        painter.image(texture, rect, uv, Color32::WHITE);
                        view_guides(painter, rect, &sculpt_overlay);
                    }
                    let credit = credit.filter(|c| !c.is_empty());
                    if let (Some(rect), Some(texture)) = (reference, textures.1) {
                        let uv = uv(reference_overlay.mirror);
                        match (image, framing) {
                            (Some((id, size)), Some(framing)) => {
                                let shown = framed(size, rect, framing);
                                painter
                                    .with_clip_rect(rect)
                                    .image(id, shown, uv, Color32::WHITE);
                            }
                            (Some((id, size)), None) => {
                                let fitted = fit(size, rect.shrink(CREDIT_MARGIN.x));
                                painter.image(id, fitted, uv, Color32::WHITE);
                            }
                            (None, _) => {
                                painter.image(texture, rect, uv, Color32::WHITE);
                            }
                        }
                        view_guides(painter, rect, &reference_overlay);
                        if let Some(credit) = credit {
                            credit_line(painter, rect, credit, Align2::LEFT_BOTTOM, reference_bg);
                        }
                    } else if let (Some(rect), Some(credit)) = (sculpt, credit) {
                        // The reference is in the picture only over the
                        // sculpt, as its ghost: credited there, across from
                        // the clock.
                        credit_line(painter, rect, credit, Align2::RIGHT_BOTTOM, sculpt_bg);
                    }
                    if let Some(divider) = placement.divider {
                        painter.rect_filled(to_points(divider), 0.0, theme::DARK.border);
                    }
                    if let (Some(clock), Some(rect)) = (clock, sculpt) {
                        painter.with_clip_rect(rect).text(
                            rect.left_bottom() + vec2(CREDIT_MARGIN.x, -CREDIT_MARGIN.y),
                            Align2::LEFT_BOTTOM,
                            clock,
                            fonts::regular(CLOCK_SIZE),
                            theme::view_inks(sculpt_bg).text,
                        );
                    }
                    if let Some((title, opacity)) = title.filter(|(t, _)| !t.trim().is_empty()) {
                        // Over the view in the frame's top left corner.
                        let under = match (sculpt, reference) {
                            (Some(s), Some(r)) if r.min.x < s.min.x || r.min.y < s.min.y => {
                                reference_bg
                            }
                            (Some(_), _) => sculpt_bg,
                            (None, _) => reference_bg,
                        };
                        let ink = theme::view_inks(under).ink;
                        let colour = ink.gamma_multiply(opacity.clamp(0.0, 1.0));
                        let wrap = screen.width() - 2.0 * TITLE_MARGIN.x;
                        let galley = painter.layout(
                            title.trim().to_owned(),
                            fonts::light(TITLE_SIZE),
                            colour,
                            wrap.max(10.0),
                        );
                        painter.galley(screen.min + TITLE_MARGIN, galley, colour);
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

/// The texture coordinates of a view, flipped left to right in the
/// mirror view.
fn uv(mirror: bool) -> Rect {
    if mirror {
        Rect::from_min_max(Pos2::new(1.0, 0.0), Pos2::new(0.0, 1.0))
    } else {
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
    }
}

/// Where a picture of `size` shows in `rect`, framed as in its view.
fn framed(size: egui::Vec2, rect: Rect, framing: ImageFraming) -> Rect {
    let fit = (rect.width() / size.x.max(1.0)).min(rect.height() / size.y.max(1.0)) * IMAGE_FIT;
    let fitted = size * fit;
    Rect::from_center_size(
        rect.center() + framing.offset * fitted,
        fitted * framing.zoom,
    )
}

/// The grid and the plumb line over a view, clipped to it.
fn view_guides(painter: &egui::Painter, rect: Rect, overlay: &ViewOverlay) {
    let painter = painter.with_clip_rect(rect);
    if let Some(rows) = overlay.grid {
        guides::grid(&painter, rect, rows);
    }
    if let Some(share) = overlay.plumb {
        guides::plumb_line(&painter, rect, share);
    }
}

/// The credit along the bottom of a view, in the corner `align` names
/// (left or right bottom), clipped to it.
fn credit_line(painter: &egui::Painter, rect: Rect, credit: &str, align: Align2, bg: Color32) {
    let font = fonts::regular(CREDIT_SIZE);
    let wrap = rect.width() - 2.0 * CREDIT_MARGIN.x;
    let text = theme::view_inks(bg).text;
    let galley = painter.layout(credit.to_owned(), font, text, wrap.max(10.0));
    let size = galley.size();
    let x = if align == Align2::RIGHT_BOTTOM {
        rect.right() - CREDIT_MARGIN.x - size.x
    } else {
        rect.left() + CREDIT_MARGIN.x
    };
    let at = Pos2::new(x, rect.bottom() - CREDIT_MARGIN.y - size.y);
    let painter = painter.with_clip_rect(rect);
    // On the view's own background, so a guide crossing it never strikes
    // it out.
    let backing = Rect::from_min_size(at, size).expand(CREDIT_SIZE * 0.25);
    painter.rect_filled(backing, 0.0, bg);
    painter.galley(at, galley, text);
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
        msaa,
        depth,
    }
}
