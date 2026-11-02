// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The reference: what is loaded in the right view, its panel and drawing.
//!
//! Splats and meshes are drawn into the reference view's texture with the
//! reference camera (and, through [`App::draw_reference_as`], into any
//! other slot: a ghost over the sculpt, an export). Pictures are flat, so
//! they are shown by egui over the view with their own pan and zoom. The
//! crop box and the credit line are drawn over the view too.

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::Arc;

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use super::study::ReferenceLook;
use super::{App, REFERENCE, Request, SCULPT};
use crate::camera::OrbitCamera;
use crate::input::FilePurpose;
use crate::reference::load;
use crate::reference::samples::Sample;
use crate::reference::{Aabb, Alignment, Content, Credit, RefMesh, Reference, Source, UpAxis};
use crate::render::Slot;
use crate::render::clay::{ClayPass, GpuMesh, Look};
use crate::render::splat::{SplatLook, SplatPass};
use crate::sculpt::Vertex;
use crate::ui::{self, fonts, menu, theme};

/// Files that open as a reference (lowercase, no dot).
pub(super) const REFERENCE_EXTENSIONS: &[&str] =
    &["ply", "splat", "spz", "obj", "stl", "png", "jpg", "jpeg"];

/// The reference and how it is shown.
#[derive(Default)]
pub(super) struct ReferenceState {
    /// The loaded reference, as placed, cropped and credited. A scene saves
    /// this and gives it back through [`App::set_reference`].
    pub(super) current: Option<Reference>,
    /// What draws it.
    drawn: Option<Drawn>,
    /// Draws mesh references. Their own, so their look never clashes with
    /// the clay's in the same slot.
    mesh_pass: Option<ClayPass>,
    crop: CropSliders,
    image_view: ImageView,
}

enum Drawn {
    Splats(Box<SplatPass>),
    Mesh {
        /// `None` when the crop keeps no triangle.
        mesh: Option<GpuMesh>,
        coloured: bool,
        /// The placement the vertices were moved into.
        built_for: (Alignment, Option<Aabb>),
    },
    Image(egui::TextureHandle),
}

/// The crop box as six sliders: left, right, bottom, top, back and front,
/// as fractions of `range` seen with the reference upright.
#[derive(Clone, Copy)]
struct CropSliders {
    sides: [f32; 6],
    /// In file coordinates; `None` for pictures.
    range: Option<Aabb>,
}

impl Default for CropSliders {
    fn default() -> Self {
        Self {
            sides: [0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
            range: None,
        }
    }
}

/// Pan (in points from the view's centre) and zoom (1 fits the view) of a
/// picture.
#[derive(Clone, Copy)]
struct ImageView {
    offset: Vec2,
    zoom: f32,
}

impl Default for ImageView {
    fn default() -> Self {
        Self {
            offset: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

/// The turn that stands the file upright, without the yaw.
fn upright(up: UpAxis) -> Mat4 {
    Alignment {
        up,
        ..Default::default()
    }
    .matrix()
}

/// The box around `b` once its corners are moved by `m`.
fn transform_box(m: Mat4, b: Aabb) -> Aabb {
    let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
    for i in 0..8 {
        let corner = Vec3::new(
            if i & 1 == 0 { b.min.x } else { b.max.x },
            if i & 2 == 0 { b.min.y } else { b.max.y },
            if i & 4 == 0 { b.min.z } else { b.max.z },
        );
        let p = m.transform_point3(corner);
        min = min.min(p);
        max = max.max(p);
    }
    Aabb { min, max }
}

impl CropSliders {
    fn for_reference(r: &Reference) -> Self {
        let mut sliders = Self {
            range: r.crop_range(),
            ..Self::default()
        };
        if let Some(crop) = r.crop {
            sliders.match_box(crop, r.align.up);
        }
        sliders
    }

    /// The crop box in file coordinates.
    fn to_box(self, up: UpAxis) -> Option<Aabb> {
        let range = self.range?;
        let turn = upright(up);
        let r = transform_box(turn, range);
        let s = r.size();
        let [l, ri, b, t, ba, f] = self.sides;
        let upright_box = Aabb {
            min: r.min + Vec3::new(l, b, ba) * s,
            max: r.min + Vec3::new(ri, t, f) * s,
        };
        Some(transform_box(turn.inverse(), upright_box))
    }

    /// Sets the sliders to show `crop` (file coordinates).
    fn match_box(&mut self, crop: Aabb, up: UpAxis) {
        let Some(range) = self.range else {
            return;
        };
        let turn = upright(up);
        let r = transform_box(turn, range);
        let c = transform_box(turn, crop);
        let s = r.size().max(Vec3::splat(1e-12));
        let lo = ((c.min - r.min) / s).clamp(Vec3::ZERO, Vec3::ONE);
        let hi = ((c.max - r.min) / s).clamp(Vec3::ZERO, Vec3::ONE);
        self.sides = [lo.x, hi.x, lo.y, hi.y, lo.z, hi.z];
    }
}

/// A turn wrapped into -π..π.
fn wrap_angle(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

/// A scroll bar in the app's manner: a square line in the margin, faint
/// until the pointer is over the panel, never a box.
fn thin_scroll_bar() -> egui::style::ScrollStyle {
    egui::style::ScrollStyle {
        content_margin: egui::Margin {
            right: 10,
            ..egui::Margin::ZERO
        },
        bar_width: 4.0,
        floating_width: 2.0,
        handle_min_length: 24.0,
        foreground_color: true,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.0,
        dormant_handle_opacity: 0.25,
        active_handle_opacity: 0.6,
        interact_handle_opacity: 1.0,
        ..egui::style::ScrollStyle::floating()
    }
}

/// Whether a mesh's vertex and index buffers fit the GPU's largest buffer.
fn mesh_fits(limits: &wgpu::Limits, mesh: &RefMesh) -> bool {
    let vertices = (mesh.positions.len() * size_of::<Vertex>()) as u64;
    let indices = (mesh.triangles.len() * size_of::<[u32; 3]>()) as u64;
    vertices.max(indices) <= limits.max_buffer_size
}

/// A mesh reference moved into the clay's space, with normals for shading.
fn place_mesh(device: &wgpu::Device, r: &Reference) -> Drawn {
    let built_for = (r.align, r.crop);
    let colors = match &r.content {
        Content::Mesh(m) => m.colors.as_deref(),
        _ => None,
    };
    let Some((positions, triangles)) = r.world_triangles().filter(|(_, t)| !t.is_empty()) else {
        return Drawn::Mesh {
            mesh: None,
            coloured: false,
            built_for,
        };
    };
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for t in &triangles {
        let [a, b, c] = t.map(|i| positions[i as usize]);
        // Area-weighted: the cross product's length is twice the area.
        let n = (b - a).cross(c - a);
        for &i in t {
            normals[i as usize] += n;
        }
    }
    let vertices: Vec<Vertex> = positions
        .iter()
        .zip(&normals)
        .enumerate()
        .map(|(i, (p, n))| Vertex {
            position: p.to_array(),
            normal: n.normalize_or(Vec3::Y).to_array(),
            color: colors.and_then(|c| c.get(i)).copied().unwrap_or([255; 4]),
        })
        .collect();
    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("reference vertices"),
        contents: bytemuck::cast_slice(&vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("reference indices"),
        contents: bytemuck::cast_slice(&triangles),
        usage: wgpu::BufferUsages::INDEX,
    });
    Drawn::Mesh {
        mesh: Some(GpuMesh {
            vertices,
            indices,
            index_count: (triangles.len() * 3) as u32,
        }),
        coloured: colors.is_some(),
        built_for,
    }
}

impl App {
    /// Loads a file as the reference: placed upright as files of its kind
    /// usually are, fitted to the clay and, for a bundled sample, credited.
    pub(super) fn load_reference(&mut self, name: &str, bytes: Arc<[u8]>) {
        let content = match load::load(name, &bytes) {
            Ok(content) => content,
            Err(e) => {
                log::warn!("{name}: {e}");
                return self.cannot_open(name);
            }
        };
        let mut reference = Reference::new(
            Source {
                name: name.to_owned(),
                bytes,
            },
            content,
        );
        reference.align.up = load::usual_up(name, &reference.content);
        if let Some(sample) = Sample::by_file(name) {
            reference.credit = sample.credit();
        }
        reference.fit();
        self.set_reference(reference);
        // Fitted references sit where the clay does: look at them the same way.
        let camera = &mut self.cameras[REFERENCE];
        camera.target = Vec3::ZERO;
        camera.distance = OrbitCamera::default().distance;
    }

    /// Shows `reference` as it is placed, cropped and credited, replacing
    /// any other. Opening a scene restores its reference this way.
    pub(super) fn set_reference(&mut self, reference: Reference) {
        // Larger buffers than the GPU allows would panic in wgpu: refuse
        // the mesh and keep what was shown.
        if let Content::Mesh(mesh) = &reference.content
            && !mesh_fits(&self.device.limits(), mesh)
        {
            let tr = self.lang.tr();
            let name = &reference.source.name;
            return self.notify(format!("{name} — {}", tr.refs.mesh_too_large));
        }
        self.remove_reference();
        let drawn = match &reference.content {
            Content::Splats(splats) => match SplatPass::new(&self.device, splats) {
                Some(pass) => {
                    if pass.len() < splats.len() {
                        self.notify(self.lang.tr().refs.thinned);
                    }
                    Drawn::Splats(Box::new(pass))
                }
                None => return self.cannot_open(&reference.source.name),
            },
            Content::Mesh(_) => {
                if self.reference.mesh_pass.is_none() {
                    self.reference.mesh_pass = Some(ClayPass::new(&self.device));
                }
                place_mesh(&self.device, &reference)
            }
            Content::Image(image) => {
                let size = [image.size[0] as usize, image.size[1] as usize];
                let pixels = egui::ColorImage::from_rgba_unmultiplied(size, &image.rgba);
                Drawn::Image(self.ctx.load_texture(
                    "reference",
                    pixels,
                    egui::TextureOptions::LINEAR,
                ))
            }
        };
        self.reference.crop = CropSliders::for_reference(&reference);
        self.reference.image_view = ImageView::default();
        self.reference.current = Some(reference);
        self.reference.drawn = Some(drawn);
        self.dirty = true;
    }

    pub(super) fn remove_reference(&mut self) {
        let state = &mut self.reference;
        state.current = None;
        state.drawn = None;
        state.crop = CropSliders::default();
        self.dirty = true;
    }

    /// Moves a mesh reference's vertices again after its placement changed.
    fn refresh_mesh(&mut self) {
        let state = &mut self.reference;
        if let (Some(r), Some(Drawn::Mesh { built_for, .. })) = (&state.current, &state.drawn)
            && *built_for != (r.align, r.crop)
        {
            state.drawn = Some(place_mesh(&self.device, r));
        }
    }

    pub(super) fn reference_panel(&mut self, ui: &mut egui::Ui) {
        // The whole panel scrolls when it is taller than the window (the
        // crop sliders and the credit fields together), behind a thin line
        // that only darkens when it is used.
        ui.spacing_mut().scroll = thin_scroll_bar();
        egui::ScrollArea::vertical()
            .id_salt("reference panel")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let tr = self.lang.tr();
                ui::section(ui, tr.reference_tools);
                if ui::choice(ui, tr.open_reference, false).clicked() {
                    self.request(Request::OpenFile {
                        purpose: FilePurpose::Reference,
                        extensions: REFERENCE_EXTENSIONS,
                    });
                }
                if self.reference.current.is_some() {
                    if ui::choice(ui, tr.remove_reference, false).clicked() {
                        self.remove_reference();
                    }
                    self.reference_controls(ui);
                }
            });
        // Before this frame's views are drawn.
        self.refresh_mesh();
    }

    /// Alignment, crop and credit.
    fn reference_controls(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        let muted = ui.visuals().widgets.inactive.fg_stroke.color;
        let state = &mut self.reference;
        let Some(r) = state.current.as_mut() else {
            return;
        };
        ui.label(
            RichText::new(&r.source.name)
                .font(fonts::regular(11.0))
                .color(muted),
        );
        ui.add_space(6.0);

        if matches!(r.content, Content::Image(_)) {
            if ui::choice(ui, tr.refs.fit_image, false).clicked() {
                state.image_view = ImageView::default();
            }
        } else {
            ui::section(ui, tr.refs.up);
            ui.horizontal(|ui| {
                for (label, up) in [("y", UpAxis::Y), ("z", UpAxis::Z), ("-y", UpAxis::NegY)] {
                    if ui::choice(ui, label, r.align.up == up).clicked() && r.align.up != up {
                        r.align.up = up;
                        // The crop keeps the same part of the file.
                        if let Some(crop) = r.crop {
                            state.crop.match_box(crop, up);
                        }
                    }
                }
            });
            let mut degrees = r.align.yaw.to_degrees();
            if ui::slider(ui, tr.refs.turn, &mut degrees, -180.0..=180.0, |v| {
                format!("{v:.0}°")
            })
            .changed()
            {
                r.align.yaw = degrees.to_radians();
            }
            ui.horizontal(|ui| {
                for (label, turn) in [("-90°", -FRAC_PI_2), ("+90°", FRAC_PI_2)] {
                    if ui::choice(ui, label, false).clicked() {
                        r.align.yaw = wrap_angle(r.align.yaw + turn);
                    }
                }
            });
            if ui::choice(ui, tr.refs.fit, false).clicked() {
                r.fit();
            }

            ui.add_space(10.0);
            let on = r.crop.is_some();
            if ui::choice(ui, tr.refs.crop, on).clicked() {
                r.crop = if on {
                    None
                } else {
                    state.crop.to_box(r.align.up)
                };
                r.fit();
            }
            if r.crop.is_some() {
                let labels = [
                    tr.refs.crop_left,
                    tr.refs.crop_right,
                    tr.refs.crop_bottom,
                    tr.refs.crop_top,
                    tr.refs.crop_back,
                    tr.refs.crop_front,
                ];
                for (i, label) in labels.into_iter().enumerate() {
                    let sides = &mut state.crop.sides;
                    let response = ui::slider(ui, label, &mut sides[i], 0.0..=1.0, |v| {
                        format!("{:.0}%", v * 100.0)
                    });
                    if response.changed() {
                        // Each side stays short of its opposite.
                        let (lo, hi) = (i & !1, i | 1);
                        if i == lo {
                            sides[lo] = sides[lo].min(sides[hi] - 0.01);
                        } else {
                            sides[hi] = sides[hi].max(sides[lo] + 0.01);
                        }
                        r.crop = state.crop.to_box(r.align.up);
                    }
                    // Re-fit once the side is let go, not while it moves.
                    if response.drag_stopped() || response.clicked() {
                        r.fit();
                    }
                }
            }
        }

        ui.add_space(10.0);
        ui::section(ui, tr.refs.credit);
        let credit = &mut r.credit;
        for (label, text) in [
            (tr.refs.title, &mut credit.title),
            (tr.refs.author, &mut credit.author),
            (tr.refs.licence, &mut credit.licence),
            (tr.refs.source, &mut credit.url),
        ] {
            ui.label(RichText::new(label).font(fonts::regular(11.0)).color(muted));
            ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
        }
    }

    /// Reference items in the edit menu.
    pub(super) fn reference_edit_items(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        let loaded = self.reference.current.is_some();
        if menu::action(ui, loaded, tr.remove_reference, "") {
            self.remove_reference();
        }
    }

    /// What is drawn over the reference view, in `rect` as laid out this
    /// frame: a picture with its pan and zoom, the crop box, and the credit
    /// line along the bottom. The views call it once the reference view's
    /// image is drawn, so it never trails the view by a frame.
    pub(super) fn reference_overlay(&mut self, ui: &egui::Ui, rect: Rect) {
        if !rect.is_positive() || self.reference.current.is_none() {
            return;
        }
        let painter = ui.painter_at(rect);
        if let Some(Drawn::Image(texture)) = &self.reference.drawn {
            let (texture, size) = (texture.id(), texture.size_vec2());
            // On top of the view's own response, so a picture takes the
            // pointer for its pan and zoom instead of turning the camera.
            let response = ui.interact(
                rect,
                ui.scope_id().with("reference picture"),
                Sense::click_and_drag(),
            );
            let view = &mut self.reference.image_view;
            if response.dragged() {
                view.offset += response.drag_delta();
            }
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if response.hovered()
                && scroll != 0.0
                && let Some(pointer) = response.hover_pos()
            {
                let zoom = (view.zoom * (scroll * 0.0015).exp()).clamp(0.1, 50.0);
                let centre = rect.center() + view.offset;
                let new_centre = pointer - (pointer - centre) * (zoom / view.zoom);
                view.offset = new_centre - rect.center();
                view.zoom = zoom;
            }
            if response.double_clicked() {
                *view = ImageView::default();
            }
            let fit = (rect.width() / size.x).min(rect.height() / size.y) * 0.95;
            let shown = Rect::from_center_size(rect.center() + view.offset, size * fit * view.zoom);
            // The mirror view shows the picture flipped left to right.
            let uv = if self.study.mirror {
                Rect::from_min_max(Pos2::new(1.0, 0.0), Pos2::new(0.0, 1.0))
            } else {
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
            };
            painter.image(texture, shown, uv, Color32::WHITE);
        }
        let Some(r) = &self.reference.current else {
            return;
        };
        if let Some(crop) = r.crop {
            let camera = &self.cameras[REFERENCE];
            // The box belongs to the 3D view, so it flips with it.
            crop_box(
                &painter,
                rect,
                camera,
                r.align.matrix(),
                crop,
                self.study.mirror,
            );
        }
        if let Some(covered) = paint_credit(&painter, rect, &r.credit) {
            let tr = self.lang.tr();
            let c = &r.credit;
            ui.interact(
                covered,
                ui.scope_id().with("reference credit"),
                Sense::hover(),
            )
            .on_hover_ui(|ui| {
                ui.set_max_width(rect.width().max(240.0).min(480.0));
                for (label, text) in [
                    (tr.refs.title, &c.title),
                    (tr.refs.author, &c.author),
                    (tr.refs.licence, &c.licence),
                    (tr.refs.source, &c.url),
                ] {
                    if !text.trim().is_empty() {
                        ui.label(
                            RichText::new(label)
                                .font(fonts::regular(11.0))
                                .color(ui.visuals().weak_text_color()),
                        );
                        ui.label(text.trim());
                    }
                }
            });
        }
    }

    /// Draws the reference into a pass already cleared to the view's
    /// background, from the reference camera, with the study aids' look
    /// (the silhouette).
    pub(super) fn draw_reference(&self, pass: &mut wgpu::RenderPass<'_>) {
        let aspect = self.views[REFERENCE].aspect();
        let look = self.reference_look();
        self.draw_reference_as(
            pass,
            Slot::ReferenceView,
            &self.cameras[REFERENCE],
            aspect,
            &look,
        );
    }

    /// A reference is loaded.
    pub(super) fn has_reference(&self) -> bool {
        self.reference.current.is_some()
    }

    /// The pixel size of the target a slot draws into, which splats need
    /// for their screen-space footprint: the views' own sizes, and a
    /// 1080-pixel-high frame of `aspect` for exports.
    fn slot_size(&self, slot: Slot, aspect: f32) -> [u32; 2] {
        match slot {
            Slot::SculptView => self.views[SCULPT].size,
            Slot::ReferenceView => self.views[REFERENCE].size,
            Slot::ExportSculpt | Slot::ExportReference => {
                [(1080.0 * aspect).round().max(1.0) as u32, 1080]
            }
        }
    }

    /// Draws the reference into any slot's pass from any camera, with
    /// `look`, preparing and drawing in one call. A look `over` the pass is
    /// the ghost, drawn last. Pictures draw nothing here.
    pub(super) fn draw_reference_as(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        slot: Slot,
        camera: &OrbitCamera,
        aspect: f32,
        look: &ReferenceLook,
    ) {
        let size = self.slot_size(slot, aspect);
        self.draw_reference_sized(pass, slot, camera, size, look);
    }

    /// [`draw_reference_as`](Self::draw_reference_as) into a target of
    /// `size` pixels, for exports, whose frames aren't always 1080 high.
    pub(super) fn draw_reference_sized(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        slot: Slot,
        camera: &OrbitCamera,
        size: [u32; 2],
        look: &ReferenceLook,
    ) {
        let aspect = size[0] as f32 / size[1].max(1) as f32;
        let ReferenceLook {
            flat,
            opacity,
            over,
        } = *look;
        let (Some(r), Some(drawn)) = (&self.reference.current, &self.reference.drawn) else {
            return;
        };
        match drawn {
            Drawn::Splats(splats) => {
                splats.set_placement(r.align.matrix(), r.crop);
                let look = SplatLook {
                    opacity,
                    flat,
                    over,
                };
                splats.prepare(&self.queue, slot, camera, size, &look);
                splats.draw(pass, slot);
            }
            Drawn::Mesh {
                mesh: Some(mesh),
                coloured,
                ..
            } => {
                let Some(clay) = &self.reference.mesh_pass else {
                    return;
                };
                let colour = match (flat, *coloured) {
                    (true, _) => theme::SILHOUETTE_INK,
                    (false, true) => Color32::WHITE,
                    (false, false) => theme::CLAY,
                };
                let look = Look {
                    colour,
                    flat,
                    opacity,
                    vertex_colours: *coloured && !flat,
                };
                if over {
                    clay.prepare_ghost(&self.queue, slot, camera, aspect, &look);
                    clay.draw_ghost(pass, slot, mesh);
                } else {
                    clay.prepare(&self.queue, slot, camera, aspect, &look);
                    clay.draw(pass, slot, mesh);
                }
            }
            Drawn::Mesh { mesh: None, .. } | Drawn::Image(_) => {}
        }
    }
}

/// Room between the credit and the view's edges, as for the view's label.
const CREDIT_MARGIN: Vec2 = Vec2::new(14.0, 12.0);

/// The credit's lines, each as a head and a tail that is never cut short:
/// title, author and licence on one line when `fits` says it fits, else
/// the title over "author — licence". Parts are trimmed; empty ones are
/// left out.
fn credit_lines(c: &Credit, fits: impl Fn(&str) -> bool) -> Vec<[String; 2]> {
    let [title, author, licence] = [&c.title, &c.author, &c.licence].map(|s| s.trim().to_owned());
    let one = [&title, &author, &licence]
        .into_iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" — ");
    if one.is_empty() {
        return Vec::new();
    }
    if fits(&one) {
        return vec![[one, String::new()]];
    }
    // The second line keeps the licence whole: only the author before it
    // gives way.
    let tail = match (author.is_empty(), licence.is_empty()) {
        (_, true) => String::new(),
        (true, false) => licence,
        (false, false) => format!(" — {licence}"),
    };
    [[title, String::new()], [author, tail]]
        .into_iter()
        .filter(|[a, b]| !a.is_empty() || !b.is_empty())
        .collect()
}

/// The credit along the bottom of the reference view, cut short with an
/// ellipsis rather than running off its right edge. Returns what it covers.
fn paint_credit(painter: &egui::Painter, rect: Rect, c: &Credit) -> Option<Rect> {
    let font = fonts::regular(12.0);
    let width = (rect.width() - 2.0 * CREDIT_MARGIN.x).max(0.0);
    let measure = |text: String| painter.layout_no_wrap(text, font.clone(), theme::VIEW_TEXT);
    let cut = |text: String, max: f32| {
        let mut job =
            egui::text::LayoutJob::simple_singleline(text, font.clone(), theme::VIEW_TEXT);
        job.wrap = egui::text::TextWrapping::truncate_at_width(max.max(0.0));
        painter.layout_job(job)
    };
    let lines = credit_lines(c, |line| measure(line.to_owned()).size().x <= width);
    let mut bottom = rect.bottom() - CREDIT_MARGIN.y;
    let mut covered = Rect::NOTHING;
    for [head, tail] in lines.into_iter().rev() {
        // The tail (the licence) is never cut; the head takes what is left.
        let tail = measure(tail);
        let head = cut(head, width - tail.size().x);
        let height = head.size().y.max(tail.size().y);
        let top = bottom - height;
        let head_pos = Pos2::new(rect.left() + CREDIT_MARGIN.x, top);
        let tail_pos = head_pos + Vec2::new(head.size().x, 0.0);
        covered = covered
            .union(Rect::from_min_size(head_pos, head.size()))
            .union(Rect::from_min_size(tail_pos, tail.size()));
        painter.galley(head_pos, head, theme::VIEW_TEXT);
        painter.galley(tail_pos, tail, theme::VIEW_TEXT);
        bottom = top - 2.0;
    }
    covered.is_positive().then_some(covered)
}

/// The crop box's twelve edges, in [`theme::CROP_LINE`], seen from `camera`,
/// flipped left to right in the mirror view like the view's image.
fn crop_box(
    painter: &egui::Painter,
    rect: Rect,
    camera: &OrbitCamera,
    model: Mat4,
    crop: Aabb,
    mirror: bool,
) {
    let view_proj = camera.projection(rect.aspect_ratio()) * camera.view() * model;
    let corners: Vec<Option<Pos2>> = (0..8)
        .map(|i| {
            let p = Vec3::new(
                if i & 1 == 0 { crop.min.x } else { crop.max.x },
                if i & 2 == 0 { crop.min.y } else { crop.max.y },
                if i & 4 == 0 { crop.min.z } else { crop.max.z },
            );
            let clip = view_proj * p.extend(1.0);
            (clip.w > 1e-4).then(|| {
                let ndc = clip.truncate() / clip.w;
                let x = if mirror { -ndc.x } else { ndc.x };
                Pos2::new(
                    rect.left() + (x + 1.0) / 2.0 * rect.width(),
                    rect.top() + (1.0 - ndc.y) / 2.0 * rect.height(),
                )
            })
        })
        .collect();
    let stroke = Stroke::new(1.0, theme::CROP_LINE);
    for a in 0..8usize {
        for bit in [1, 2, 4] {
            let b = a | bit;
            if b != a
                && let (Some(p), Some(q)) = (corners[a], corners[b])
            {
                painter.line_segment([p, q], stroke);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meshes_must_fit_the_largest_buffer() {
        let mesh = RefMesh {
            positions: vec![Vec3::ZERO; 1000],
            triangles: vec![[0, 1, 2]; 4000],
            colors: None,
        };
        let limits = |max_buffer_size| wgpu::Limits {
            max_buffer_size,
            ..wgpu::Limits::default()
        };
        // 28 kB of vertices, 48 kB of indices.
        assert!(mesh_fits(&limits(48_000), &mesh));
        assert!(!mesh_fits(&limits(47_999), &mesh));
        assert!(!mesh_fits(&limits(27_999), &mesh));
    }

    #[test]
    fn crop_sliders_follow_the_up_axis() {
        let range = Aabb {
            min: Vec3::new(0.0, 0.0, 0.0),
            max: Vec3::new(10.0, 20.0, 30.0),
        };
        let mut sliders = CropSliders {
            range: Some(range),
            ..Default::default()
        };
        // Full sliders give the whole range, whichever way is up.
        for up in [UpAxis::Y, UpAxis::Z, UpAxis::NegY] {
            let b = sliders.to_box(up).unwrap();
            assert!(b.min.distance(range.min) < 1e-4 && b.max.distance(range.max) < 1e-4);
        }
        // Z up: "bottom" lifts the file's z, "front" moves its y.
        sliders.sides = [0.0, 1.0, 0.5, 1.0, 0.0, 1.0];
        let b = sliders.to_box(UpAxis::Z).unwrap();
        assert!((b.min.z - 15.0).abs() < 1e-3, "{b:?}");
        assert!(b.min.y.abs() < 1e-3);
        // And back again.
        let mut again = CropSliders {
            range: Some(range),
            ..Default::default()
        };
        again.match_box(b, UpAxis::Z);
        for (a, b) in again.sides.iter().zip(sliders.sides) {
            assert!((a - b).abs() < 1e-4);
        }
        // Upside down, the file's low y is the top.
        sliders.sides = [0.0, 1.0, 0.0, 0.25, 0.0, 1.0];
        let b = sliders.to_box(UpAxis::NegY).unwrap();
        assert!((b.min.y - 15.0).abs() < 1e-3, "{b:?}");
    }

    #[test]
    fn turns_wrap() {
        assert!((wrap_angle(PI + 0.5) - (-PI + 0.5)).abs() < 1e-5);
        assert!((wrap_angle(-FRAC_PI_2) + FRAC_PI_2).abs() < 1e-6);
    }
}
