// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Sculpting: the tools, strokes on the clay and the sculpt panel.

mod levels;
mod tool_pie;

use std::sync::Arc;

use egui::{Key, Pos2, Rect, Stroke};
use glam::{Mat4, Vec2, Vec3};

use super::{App, Drag, Request, SCULPT};
use crate::camera::OrbitCamera;
use crate::input::{FilePurpose, PointerKind};
use crate::meshio::{self, Soup};
use crate::quad;
use crate::reference::Content;
use crate::render::clay::GpuMesh;
pub(super) use tool_pie::TOOL_PIE_KEY;

use crate::sculpt::{
    Brush, Dab, Falloff, Hit, Levels, MAX_TRIANGLES, MAX_VERTICES, Mesh, START_LEVEL, Sculpt,
    cube_sphere,
};
use crate::ui::{self, i18n::Tr, icons, menu, theme};
use crate::voxel;

/// Dabs are placed every this fraction of the tool radius along a stroke.
const SPACING: f32 = 0.15;

/// Sides of the brush outline drawn on the clay.
const CURSOR_SEGMENTS: usize = 48;
/// Length of the normal tick at the centre of that outline, as a share of
/// the brush radius.
const TICK: f32 = 0.3;

/// The stabiliser's longest string, in points.
const MAX_STABILISER: f32 = 120.0;

/// Each brush with its shortcut and starting strength, in `Brush::ALL` order.
/// (F frames the sculpt, so flatten takes T, for trim.)
pub(super) const BRUSHES: [(Brush, Key, f32); 7] = [
    (Brush::Move, Key::M, 1.0),
    (Brush::Clay, Key::C, 0.5),
    (Brush::Flatten, Key::T, 0.5),
    (Brush::Smooth, Key::S, 0.5),
    (Brush::Inflate, Key::I, 0.5),
    (Brush::Crease, Key::X, 0.5),
    (Brush::Pinch, Key::P, 0.5),
];

pub(super) fn brush_name(tr: &Tr, brush: Brush) -> &'static str {
    match brush {
        Brush::Move => tr.move_,
        Brush::Clay => tr.clay,
        Brush::Flatten => tr.flatten,
        Brush::Smooth => tr.smooth,
        Brush::Inflate => tr.inflate,
        Brush::Crease => tr.crease,
        Brush::Pinch => tr.pinch,
    }
}

/// The icon that stands for `brush`, on the toolbar, in its flyout and
/// in the tool pie.
pub(super) fn brush_icon(brush: Brush) -> fn(&egui::Painter, Rect, egui::Color32) {
    match brush {
        Brush::Move => icons::move_tool,
        Brush::Clay => icons::clay_tool,
        Brush::Flatten => icons::flatten_tool,
        Brush::Smooth => icons::smooth_tool,
        Brush::Inflate => icons::inflate_tool,
        Brush::Crease => icons::crease_tool,
        Brush::Pinch => icons::pinch_tool,
    }
}

/// The keys that pick `brush`, as the shortcuts window and the toolbar's
/// tooltip show them: Shift smooths with any tool while it is held.
pub(super) fn brush_keys(brush: Brush) -> String {
    let key = BRUSHES[tool_index(brush)].1.name();
    if brush == Brush::Smooth {
        format!("{key} / Shift")
    } else {
        key.to_owned()
    }
}

/// The brush's profile across its width, rim to rim: how much of the
/// effect each point under it gets.
fn falloff_curve(ui: &mut egui::Ui, falloff: Falloff) {
    const STEPS: usize = 48;
    let size = egui::vec2(ui.available_width().min(160.0), 30.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let visuals = ui.visuals();
    let ink = visuals.widgets.active.fg_stroke.color;
    let faint = visuals.widgets.inactive.fg_stroke.color.gamma_multiply(0.5);
    let painter = ui.painter_at(rect.expand(1.0));
    painter.hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, faint));
    let points = (0..=STEPS)
        .map(|i| {
            let x = i as f32 / STEPS as f32 * 2.0 - 1.0;
            let weight = falloff.weight(x.abs(), 1.0);
            egui::pos2(
                rect.center().x + x * rect.width() / 2.0,
                rect.bottom() - weight * (rect.height() - 2.0),
            )
        })
        .collect();
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.0, ink)));
}

fn falloff_name(tr: &Tr, falloff: Falloff) -> &'static str {
    match falloff {
        Falloff::Smooth => tr.mesh.falloff_smooth,
        Falloff::Linear => tr.mesh.falloff_linear,
        Falloff::Sharp => tr.mesh.falloff_sharp,
    }
}

pub(super) struct ToolSettings {
    pub(super) tool: Brush,
    /// Radius on screen, in points.
    pub(super) radius: f32,
    /// Strength per tool at full pen pressure, 0..1, in `Brush::ALL` order.
    pub(super) strength: [f32; 7],
    /// How each tool fades towards its rim, in `Brush::ALL` order.
    pub(super) falloff: [Falloff; 7],
    pub(super) symmetry: bool,
    /// Stroke stabiliser: how far the brush trails the pointer, in
    /// points; 0 is off.
    pub(super) stabiliser: f32,
    /// Remesh resolution: cells across the clay's longest side.
    pub(super) resolution: f32,
    /// Clay from splats: the density that counts as material, 0..1.
    pub(super) threshold: f32,
    /// Quad remesh: about how many quads.
    pub(super) quads: f32,
    /// Asked for in the frame at this time, done at the start of the next,
    /// so the notice saying so shows while it runs.
    pending: Option<(Pending, f64)>,
    /// The tool pie menu, while it is open.
    pie: Option<crate::ui::pie::Pie>,
}

/// A slow operation waiting for the next frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Pending {
    Remesh,
    QuadRemesh,
    Subdivide,
    FromReference,
    /// One subdivision level up (true) or down.
    Level(bool),
    /// Saving the scene (see `files.rs`): a fine clay takes a second to
    /// pack.
    SaveScene,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            tool: Brush::Clay,
            radius: 48.0,
            strength: BRUSHES.map(|(_, _, strength)| strength),
            falloff: [Falloff::Smooth; 7],
            // A study from a reference is rarely symmetric: mirroring is
            // asked for, not assumed.
            symmetry: false,
            stabiliser: 0.0,
            resolution: voxel::DEFAULT_RESOLUTION as f32,
            threshold: voxel::DEFAULT_THRESHOLD,
            quads: quad::DEFAULT_QUADS as f32,
            pending: None,
            pie: None,
        }
    }
}

/// Subdivision stops short of this many vertices: beyond it, strokes in
/// the browser stop following the pen smoothly. The start clay's second
/// subdivision (1,572,866) just fits.
const MAX_SUBDIVIDED_VERTICES: usize = 1_600_000;

/// Imported meshes with fewer vertices are too coarse to sculpt: remeshed.
const MIN_IMPORT_VERTICES: usize = 5_000;
/// Imported meshes with more are too heavy to sculpt smoothly: remeshed.
const MAX_IMPORT_VERTICES: usize = 1_500_000;
/// Long edges more than this many times the short ones: too uneven for
/// the brushes, remeshed.
const MAX_EDGE_SPREAD: f32 = 8.0;

/// Whether an imported mesh should be remeshed before sculpting: open or
/// non-manifold, too coarse or too fine, or with very uneven triangles.
fn needs_remesh(soup: &Soup) -> bool {
    let n = soup.positions.len();
    if !soup.is_closed() || !(MIN_IMPORT_VERTICES..=MAX_IMPORT_VERTICES).contains(&n) {
        return true;
    }
    let mut lengths: Vec<f32> = soup
        .triangles
        .iter()
        .flat_map(|t| {
            let [a, b, c] = t.map(|v| soup.positions[v as usize]);
            [a.distance(b), b.distance(c), c.distance(a)]
        })
        .collect();
    lengths.sort_by(f32::total_cmp);
    let at = |q: f32| lengths[((lengths.len() - 1) as f32 * q) as usize];
    at(0.95) > at(0.05) * MAX_EDGE_SPREAD
}

/// Where a brush on a string `length` long ends up when the pointer moves
/// to `pointer`: it stays put while the string is slack, and is pulled
/// straight towards the pointer once it is taut. The pressure moves the
/// same share of the way. A zero length gives the pointer itself.
fn follow(
    brush: Pos2,
    brush_pressure: f32,
    pointer: Pos2,
    pressure: f32,
    length: f32,
) -> (Pos2, f32) {
    let gap = brush.distance(pointer);
    if length <= 0.0 {
        return (pointer, pressure);
    }
    if gap <= length {
        return (brush, brush_pressure);
    }
    let t = (gap - length) / gap;
    (
        brush.lerp(pointer, t),
        brush_pressure + (pressure - brush_pressure) * t,
    )
}

/// Walks a stroke's brush from `from` to `to`, calling `dab` every
/// `spacing` points along the way with the pressure interpolated there.
/// `carry` is how far the brush went since the last dab; returns it for
/// the next step.
fn walk(
    from: Pos2,
    from_pressure: f32,
    carry: f32,
    to: Pos2,
    pressure: f32,
    spacing: f32,
    mut dab: impl FnMut(Pos2, f32),
) -> f32 {
    let length = from.distance(to);
    let mut along = spacing - carry;
    while along <= length {
        let t = along / length.max(1e-6);
        dab(
            from.lerp(to, t),
            from_pressure + (pressure - from_pressure) * t,
        );
        along += spacing;
    }
    length - (along - spacing)
}

/// `Brush::ALL` lists the brushes in declaration order.
fn tool_index(tool: Brush) -> usize {
    tool as usize
}

/// The clay every new scene starts from: a sphere with the topology of a
/// subdivided cube, with the levels below it to step down to.
pub(super) fn start_clay() -> (Mesh, Levels) {
    cube_sphere(START_LEVEL)
}

/// A new sculpt of the start clay; its levels are in its first checkpoint.
pub(super) fn start_sculpt() -> Sculpt {
    let (mesh, levels) = start_clay();
    Sculpt::with_levels(mesh, Some(levels))
}

impl App {
    /// Undo, redo, tool and size keys.
    pub(super) fn sculpt_shortcuts(&mut self, ui: &egui::Ui) {
        use egui::Modifiers as M;
        let pressed = |m: M, key: Key| ui.input_mut(|i| i.consume_key(m, key));
        // The timelapse shows the clay as it was: no sculpting until it closes.
        if self.playback.is_none() {
            self.tool_pie_keys(ui);
            if pressed(M::COMMAND | M::SHIFT, Key::Z) || pressed(M::COMMAND, Key::Y) {
                self.sculpt.redo();
            } else if pressed(M::COMMAND, Key::Z) {
                self.sculpt.undo();
            }
            for (brush, key, _) in BRUSHES {
                if pressed(M::NONE, key) {
                    self.tool.tool = brush;
                }
            }
            if pressed(M::SHIFT, Key::R) {
                self.ask_remesh();
            }
            if pressed(M::SHIFT, Key::Q) {
                self.ask_quad_remesh();
            }
            if pressed(M::SHIFT, Key::U) {
                self.ask_subdivide();
            }
            self.level_shortcuts(ui);
        }
        if pressed(M::NONE, Key::OpenBracket) {
            self.tool.radius = (self.tool.radius / 1.2).max(4.0);
        }
        if pressed(M::NONE, Key::CloseBracket) {
            self.tool.radius = (self.tool.radius * 1.2).min(400.0);
        }
    }

    /// Asks the shell for a mesh file to replace the clay with. For the
    /// file menu's "import sculpt mesh" item.
    #[allow(dead_code)] // until the file menu calls it
    pub(super) fn ask_import_sculpt(&mut self) {
        self.request(Request::OpenFile {
            purpose: FilePurpose::Sculpt,
            extensions: meshio::EXTENSIONS,
        });
    }

    /// Replaces the clay with a mesh from an OBJ or STL file: welded, fitted
    /// to the unit sphere, and remeshed if it can't be sculpted as it is.
    /// Undo brings the previous clay back.
    pub(super) fn import_sculpt(&mut self, name: &str, bytes: Arc<[u8]>) {
        let Ok(soup) = meshio::read(name, &bytes) else {
            return self.cannot_open(name);
        };
        let mut soup = soup.weld();
        soup.normalise();
        let remeshed = needs_remesh(&soup);
        let mesh = if remeshed {
            voxel::from_triangles(
                &soup.positions,
                &soup.triangles,
                self.tool.resolution.round() as u32,
                1,
            )
        } else {
            Some(Mesh::new(soup.positions, soup.triangles))
        };
        let Some(mesh) = mesh else {
            return self.cannot_open(name);
        };
        if !self.replace_clay(mesh) {
            return;
        }
        self.playback = None;
        self.frame_sculpt();
        let tr = self.lang.tr();
        let done = if remeshed {
            tr.mesh.imported_remeshed
        } else {
            tr.mesh.imported
        };
        self.notify(format!("{name} — {done}"));
    }

    /// Puts a new mesh in place of the clay, as one undoable step, and
    /// makes its GPU buffers. Refuses, with a notice, a mesh too heavy to
    /// sculpt or to fit the GPU's buffers; true if it was put in place.
    pub(super) fn replace_clay(&mut self, mesh: Mesh) -> bool {
        self.replace_clay_with_levels(mesh, None)
    }

    /// [`App::replace_clay`], with subdivision levels built for `mesh`.
    fn replace_clay_with_levels(&mut self, mesh: Mesh, levels: Option<Levels>) -> bool {
        if mesh.positions.len() > MAX_VERTICES || mesh.triangles.len() > MAX_TRIANGLES {
            self.notify(self.lang.tr().mesh.too_heavy);
            return false;
        }
        self.sculpt.replace_mesh_with_levels(mesh, levels);
        self.sync_clay();
        true
    }

    /// Replaces the clay with the shape of the loaded reference (inside its
    /// crop box), at the remesh resolution. Undo brings the previous clay
    /// back.
    fn clay_from_reference(&mut self) {
        let tr = self.lang.tr();
        let Some(reference) = &self.reference.current else {
            return;
        };
        let resolution = self.tool.resolution.round() as u32;
        match voxel::clay_from_reference(reference, resolution, self.tool.threshold) {
            Some(mesh) => {
                if self.replace_clay(mesh) {
                    self.playback = None;
                    self.frame_sculpt();
                    self.notify(tr.mesh.made_from_reference);
                }
            }
            None => self.notify(tr.mesh.nothing_left),
        }
    }

    /// Remeshes the clay at the next frame, saying so first.
    fn ask_remesh(&mut self) {
        self.ask(Pending::Remesh, self.lang.tr().mesh.remesh);
    }

    /// Runs a slow operation at the next frame, after a notice saying so.
    /// Edits of the clay wait while the timelapse shows.
    pub(super) fn ask(&mut self, pending: Pending, what: &str) {
        if self.playback.is_none() || pending == Pending::SaveScene {
            self.tool.pending = Some((pending, self.now));
            self.notify(format!("{what}…"));
        }
    }

    /// Does the operation asked for in an earlier frame. One asked for in
    /// this frame (from a menu, which comes before the panel) waits for
    /// the next, so the frame showing its notice is drawn first.
    fn run_pending(&mut self) {
        let Some((pending, asked)) = self.tool.pending else {
            return;
        };
        if asked >= self.now {
            return;
        }
        self.tool.pending = None;
        match pending {
            Pending::Remesh => self.remesh(),
            Pending::QuadRemesh => self.quad_remesh(),
            Pending::Subdivide => self.subdivide(),
            Pending::FromReference => self.clay_from_reference(),
            Pending::Level(up) => self.change_level(up),
            Pending::SaveScene => self.save_scene(),
        }
    }

    /// Rebuilds the clay as an even mesh at the chosen resolution.
    pub(super) fn remesh(&mut self) {
        let tr = self.lang.tr();
        match voxel::remesh(&self.sculpt.mesh, self.tool.resolution.round() as u32) {
            Some(mesh) => {
                let count = mesh.positions.len();
                if self.replace_clay(mesh) {
                    let m = tr.mesh;
                    self.notify(format!("{} ({count} {})", m.remeshed, m.vertices));
                }
            }
            None => self.notify(tr.mesh.nothing_left),
        }
    }

    /// Quad remeshes the clay at the next frame, saying so first.
    fn ask_quad_remesh(&mut self) {
        self.ask(Pending::QuadRemesh, self.lang.tr().mesh.quad_remesh);
    }

    /// Rebuilds the clay as even quads following its shape (see
    /// [`quad::remesh`]); leaves it as it is, with a notice, if that fails.
    pub(super) fn quad_remesh(&mut self) {
        let m = self.lang.tr().mesh;
        let quads = self.tool.quads.round() as u32;
        match quad::remesh(&self.sculpt.mesh, quads) {
            Some(mesh) => {
                let count = mesh.positions.len();
                if self.replace_clay(mesh) {
                    self.notify(format!("{} ({count} {})", m.quad_remeshed, m.vertices));
                }
            }
            None => self.notify(m.quad_failed),
        }
    }

    /// Subdivides the clay at the next frame, saying so first; or says
    /// why not, if the result would be too heavy to sculpt.
    fn ask_subdivide(&mut self) {
        let m = self.lang.tr().mesh;
        if self.sculpt.subdivided_len() > MAX_SUBDIVIDED_VERTICES {
            if self.playback.is_none() {
                self.notify(m.too_dense);
            }
        } else {
            self.ask(Pending::Subdivide, m.subdivide);
        }
    }

    /// Adds a subdivision level above the top one, going up to the top
    /// first: every triangle of the clay split into four and smoothed.
    pub(super) fn subdivide(&mut self) {
        while self.sculpt.change_level(true) {}
        let m = self.lang.tr().mesh;
        if self.sculpt.subdivide() {
            self.sync_clay();
            let count = self.sculpt.mesh.positions.len();
            self.notify(format!("{} ({count} {})", m.subdivided, m.vertices));
        } else {
            self.notify(m.cannot_subdivide);
        }
    }

    /// Mesh formats in the file → export menu. Always the live clay, even
    /// while the timelapse is showing.
    pub(super) fn mesh_export_items(&mut self, ui: &mut egui::Ui) {
        if menu::action(ui, true, "OBJ…", "") {
            let bytes = meshio::obj::write(&self.sculpt.mesh);
            self.request(Request::SaveFile {
                name: "sculpt.obj".into(),
                mime: "model/obj",
                bytes,
            });
        }
        if menu::action(ui, true, "STL…", "") {
            let bytes = meshio::stl::write(&self.sculpt.mesh);
            self.request(Request::SaveFile {
                name: "sculpt.stl".into(),
                mime: "model/stl",
                bytes,
            });
        }
    }

    /// Whole-mesh edits in the edit menu.
    pub(super) fn mesh_edit_items(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        let live = self.playback.is_none();
        if menu::action(ui, live, tr.new_sphere, "") {
            self.new_sphere();
        }
        if menu::action(ui, live && self.has_reference(), tr.mesh.from_reference, "") {
            self.ask(Pending::FromReference, tr.mesh.from_reference);
        }
        ui.separator();
        if menu::action(ui, live, tr.mesh.remesh, "Shift+R") {
            self.ask_remesh();
        }
        if menu::action(ui, live, tr.mesh.quad_remesh, "Shift+Q") {
            self.ask_quad_remesh();
        }
        if menu::action(ui, live, tr.mesh.subdivide, "Shift+U") {
            self.ask_subdivide();
        }
        self.level_items(ui);
        ui.separator();
        if menu::action(ui, live, tr.mirror_sculpt, "") {
            self.sculpt.mirror(true);
        }
        if menu::action(ui, live, tr.mesh.mirror_to_positive, "") {
            self.sculpt.mirror(false);
        }
    }

    /// Puts a fresh sphere in place of the clay, as one undoable step; the
    /// reference, the views and the timelapse stay.
    fn new_sphere(&mut self) {
        let (mesh, levels) = start_clay();
        if self.replace_clay_with_levels(mesh, Some(levels)) {
            self.frame_sculpt();
        }
    }

    /// Makes the clay's GPU buffers again after its mesh was replaced (a
    /// remesh, an import, or undoing one, from the panel, the keys or the
    /// edit menu). The panel runs after the menus and before the frame
    /// uploads the clay, so checking here catches every case.
    pub(super) fn sync_clay(&mut self) {
        if self.sculpt.take_topology_changed() {
            self.gpu_mesh = GpuMesh::upload(&self.device, &self.queue, &self.sculpt.mesh);
            self.sculpt.take_dirty();
            // A stroke in progress held vertices of the old mesh.
            if !matches!(self.drags[SCULPT], Some(Drag::Navigate { .. })) {
                self.drags[SCULPT] = None;
            }
        }
    }

    /// What the sculpt panel did each frame, done whether it shows or
    /// not: the slow operation asked for last frame, and the timelapse.
    pub(super) fn sculpt_upkeep(&mut self, ui: &egui::Ui) {
        self.run_pending();
        self.sync_clay();
        self.advance_playback(ui);
    }

    /// The sculpt panel, in groups that fold away: the current tool's
    /// settings (the tool is picked on the toolbar), the subdivision levels,
    /// and the remeshes.
    pub(super) fn sculpt_panel(&mut self, ui: &mut egui::Ui) {
        if self.playback.is_some() {
            self.playback_panel(ui);
            return;
        }
        let tr = self.lang.tr();
        let m = tr.mesh;
        ui::group(ui, "tool settings", m.tool_settings, |ui| {
            self.tool_settings(ui)
        });
        ui::group(ui, "subdivision", m.subdivision, |ui| {
            if ui::choice(ui, m.subdivide, false).clicked() {
                self.ask_subdivide();
            }
            self.levels_panel(ui);
        });
        ui::group(ui, "remeshing", m.remeshing, |ui| self.remesh_controls(ui));
    }

    /// The tools, and the chosen one's size, strength, stabiliser,
    /// falloff and symmetry.
    fn tool_settings(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui::slider(ui, tr.size, &mut self.tool.radius, 4.0..=400.0, |v| {
            format!("{v:.0}")
        });
        let strength = &mut self.tool.strength[tool_index(self.tool.tool)];
        ui::slider(ui, tr.strength, strength, 0.0..=1.0, |v| {
            format!("{:.0}%", v * 100.0)
        });
        let off = tr.mesh.off;
        let stabiliser = &mut self.tool.stabiliser;
        ui::slider(
            ui,
            tr.mesh.stabiliser,
            stabiliser,
            0.0..=MAX_STABILISER,
            |v| {
                if v < 0.5 {
                    off.to_owned()
                } else {
                    format!("{v:.0}")
                }
            },
        );
        // Below half a point it is off, as the slider says.
        if *stabiliser < 0.5 {
            *stabiliser = 0.0;
        }
        ui::section(ui, tr.mesh.falloff);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let chosen = &mut self.tool.falloff[tool_index(self.tool.tool)];
            for falloff in Falloff::ALL {
                if ui::choice(ui, falloff_name(tr, falloff), *chosen == falloff).clicked() {
                    *chosen = falloff;
                }
            }
        });
        falloff_curve(ui, self.tool.falloff[tool_index(self.tool.tool)]);
        ui.add_space(10.0);
        if ui::choice(ui, tr.symmetry, self.tool.symmetry).clicked() {
            self.tool.symmetry = !self.tool.symmetry;
        }
    }

    /// The voxel remesh and the mesh from the reference, which share its
    /// resolution, then the quad remesh.
    fn remesh_controls(&mut self, ui: &mut egui::Ui) {
        let m = self.lang.tr().mesh;
        let range = voxel::MIN_RESOLUTION as f32..=voxel::MAX_RESOLUTION as f32;
        ui::slider(ui, m.resolution, &mut self.tool.resolution, range, |v| {
            format!("{v:.0}")
        });
        if ui::choice(ui, m.remesh, false).clicked() {
            self.ask_remesh();
        }
        // Clay from the reference: the threshold only matters for splats.
        let loaded = self.has_reference();
        let splats = matches!(
            self.reference.current.as_ref().map(|r| &r.content),
            Some(Content::Splats(_))
        );
        if splats {
            let threshold = &mut self.tool.threshold;
            ui::slider(ui, m.threshold, threshold, 0.05..=1.0, |v| {
                format!("{:.0}%", v * 100.0)
            });
        }
        if ui
            .add_enabled_ui(loaded, |ui| ui::choice(ui, m.from_reference, false))
            .inner
            .clicked()
        {
            self.ask(Pending::FromReference, m.from_reference);
        }
        ui.add_space(10.0);
        let range = quad::MIN_QUADS as f32..=quad::MAX_QUADS as f32;
        ui::typed_slider(ui, m.quads, &mut self.tool.quads, range, 100.0, |v| {
            format!("{v:.0}")
        });
        if ui::choice(ui, m.quad_remesh, false).clicked() {
            self.ask_quad_remesh();
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

    pub(super) fn start_stroke(&mut self, rect: Rect, pos: Pos2, hit: Hit) {
        self.sculpt.begin_stroke();
        let camera = &self.cameras[SCULPT];
        let scale = camera.world_per_point(hit.point, rect.height());
        if self.tool.tool == Brush::Move {
            let (right, up) = camera.right_up();
            self.sculpt
                .set_falloff(self.tool.falloff[tool_index(Brush::Move)]);
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
            self.dab_at(rect, pressure, hit, Vec3::ZERO);
        }
    }

    /// Continues a clay or smooth stroke towards the pointer at `to`. With
    /// the stabiliser on, the brush trails the pointer on a string and
    /// moves only when the pointer pulls it; dabs follow the brush at even
    /// spacing, with the pressure interpolated between samples.
    pub(super) fn stroke_to(&mut self, rect: Rect, to: Pos2, pressure: f32) {
        let Some(Drag::Stroke {
            last,
            pressure: last_pressure,
            carry,
        }) = self.drags[SCULPT]
        else {
            return;
        };
        let (to, pressure) = follow(last, last_pressure, to, pressure, self.tool.stabiliser);
        let spacing = (self.tool.radius * SPACING).max(1.0);
        // The way the brush heads, in the view plane; brushes that follow
        // the stroke take its part along the surface.
        let (right, up) = self.cameras[SCULPT].right_up();
        let d = to - last;
        let direction = (right * d.x - up * d.y).normalize_or_zero();
        let mut dabs = Vec::new();
        let carry = walk(
            last,
            last_pressure,
            carry,
            to,
            pressure,
            spacing,
            |pos, p| {
                dabs.push((pos, p));
            },
        );
        for (pos, pressure) in dabs {
            if let Some(hit) = self.pick(rect, pos) {
                self.dab_at(rect, pressure, hit, direction);
            }
        }
        self.drags[SCULPT] = Some(Drag::Stroke {
            last: to,
            pressure,
            carry,
        });
    }

    /// One dab: pen pressure scales the tool's strength.
    fn dab_at(&mut self, rect: Rect, pressure: f32, hit: Hit, direction: Vec3) {
        let (m, shift) = self.ctx.input(|i| (i.modifiers.into(), i.modifiers.shift));
        let tool = if shift { Brush::Smooth } else { self.tool.tool };
        let pressure = pressure.clamp(0.0, 1.0);
        let strength = self.tool.strength[tool_index(tool)] * pressure;
        let scale = self.cameras[SCULPT].world_per_point(hit.point, rect.height());
        self.sculpt.set_falloff(self.tool.falloff[tool_index(tool)]);
        self.sculpt.dab(Dab {
            brush: tool,
            centre: hit.point,
            radius: self.tool.radius * scale,
            strength,
            invert: self.nav.inverts(m),
            symmetry: self.tool.symmetry,
            direction,
        });
    }

    /// The clay under a point of the sculpt view.
    pub(super) fn pick(&self, rect: Rect, pos: Pos2) -> Option<Hit> {
        let ndc = Vec2::new(
            (pos.x - rect.left()) / rect.width() * 2.0 - 1.0,
            1.0 - (pos.y - rect.top()) / rect.height() * 2.0,
        );
        let (origin, dir) = self.cameras[SCULPT].ray(ndc, rect.width() / rect.height());
        self.sculpt.raycast(origin, dir)
    }

    /// The tool outline, while the pointer is over the clay view: a circle
    /// lying on the clay under the brush, or a circle on the screen where
    /// there is no clay. In a stabilised stroke the brush trails the
    /// pointer, and a thin line joins them.
    pub(super) fn tool_cursor(&self, ui: &egui::Ui, rect: Rect) {
        let Some(screen) = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| rect.contains(*p))
        else {
            return;
        };
        if self.playback.is_some()
            || self.pie_open()
            || self.flyout.is_some()
            || matches!(self.drags[SCULPT], Some(Drag::Navigate { .. }))
        {
            return;
        }
        let ink = theme::VIEW_INK.gamma_multiply(0.6);
        let line = Stroke::new(1.0, ink);
        let painter = ui.painter().with_clip_rect(rect);
        // Strokes work in the rendered view's points; the mirror view shows
        // it flipped, and `view_pos` maps either way.
        let pointer = self.view_pos(rect, screen);
        let brush = match self.drags[SCULPT] {
            Some(Drag::Stroke { last, .. }) if self.tool.stabiliser > 0.0 => last,
            _ => pointer,
        };
        let centre = self.view_pos(rect, brush);
        if brush != pointer {
            painter.line_segment([centre, screen], Stroke::new(1.0, ink.gamma_multiply(0.5)));
            painter.circle_filled(screen, 1.5, ink);
        }
        let camera = &self.cameras[SCULPT];
        let draw = |ring: Vec<Pos2>, tick: [Pos2; 2], line: Stroke| {
            let ring = ring.into_iter().map(|p| self.view_pos(rect, p)).collect();
            painter.add(egui::Shape::closed_line(ring, line));
            painter.line_segment(tick.map(|p| self.view_pos(rect, p)), line);
        };
        let hit = self.pick(rect, brush);
        // The brush's size on the clay, in the clay's units.
        let radius =
            hit.map(|hit| self.tool.radius * camera.world_per_point(hit.point, rect.height()));
        let outline = hit.zip(radius).and_then(|(hit, radius)| {
            let normal = self.sculpt.normal_at(&hit);
            surface_outline(camera, rect, hit.point, normal, radius)
        });
        match outline {
            Some((ring, tick)) => draw(ring, tick, line),
            None => {
                painter.circle_stroke(centre, self.tool.radius, line);
            }
        }
        // With symmetry on, the other side the stroke reaches, in grey.
        if self.tool.symmetry
            && let Some((hit, radius)) = hit.zip(radius)
            && let Some(other) = mirrored_hit(&self.sculpt, camera.eye(), hit.point, radius)
            && let Some((ring, tick)) = surface_outline(
                camera,
                rect,
                other.point,
                self.sculpt.normal_at(&other),
                radius,
            )
        {
            let grey = theme::MIRROR_INK.gamma_multiply(0.6);
            draw(ring, tick, Stroke::new(1.0, grey));
        }
        painter.circle_filled(centre, 1.5, ink);
    }
}

/// A point in the world as seen in a view of `rect`, in its points (before
/// any mirroring); `None` behind the camera.
fn project(to_clip: Mat4, rect: Rect, p: Vec3) -> Option<Pos2> {
    let clip = to_clip * p.extend(1.0);
    if clip.w <= 1e-6 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some(egui::pos2(
        rect.left() + (ndc.x + 1.0) * 0.5 * rect.width(),
        rect.top() + (1.0 - ndc.y) * 0.5 * rect.height(),
    ))
}

/// Two mirrored dabs closer than this share of the radius are one: the
/// same rule as `Sculpt::dab`.
const SAME_DAB: f32 = 0.05;
/// The mirror image of a point is taken to be on the clay when a ray
/// towards it meets the surface within this share of the brush radius:
/// the clay may be a little lopsided, but the dab still reaches it.
const MIRROR_SLACK: f32 = 0.5;

/// Where the mirrored half of a symmetric dab at `point` lands, as an eye
/// at `eye` sees it: the point mirrored across the symmetry plane (x = 0,
/// as the dabs are), found on the clay by a ray from the eye. `None` when
/// the two dabs are one, or when the mirrored point is out of sight:
/// facing away, behind other clay, or off the clay. `radius` is the
/// brush's, in the clay's units.
fn mirrored_hit(sculpt: &Sculpt, eye: Vec3, point: Vec3, radius: f32) -> Option<Hit> {
    let target = Vec3::new(-point.x, point.y, point.z);
    if target.distance(point) <= radius * SAME_DAB {
        return None;
    }
    let hit = sculpt.raycast(eye, (target - eye).normalize_or_zero())?;
    (hit.point.distance(target) <= radius * MIRROR_SLACK).then_some(hit)
}

/// The brush outline on the clay, as `camera` sees it in a view of `rect`:
/// a circle of `radius` (in the clay's units) around `centre`, lying
/// square to `normal`, and a short tick along the normal. In the rendered
/// view's points; `None` if part of it is behind the camera.
fn surface_outline(
    camera: &OrbitCamera,
    rect: Rect,
    centre: Vec3,
    normal: Vec3,
    radius: f32,
) -> Option<(Vec<Pos2>, [Pos2; 2])> {
    let (u, v) = normal.any_orthonormal_pair();
    let to_clip = camera.projection(rect.width() / rect.height()) * camera.view();
    let ring = (0..CURSOR_SEGMENTS)
        .map(|i| {
            let a = i as f32 / CURSOR_SEGMENTS as f32 * std::f32::consts::TAU;
            project(to_clip, rect, centre + (u * a.cos() + v * a.sin()) * radius)
        })
        .collect::<Option<Vec<Pos2>>>()?;
    let tick = [
        project(to_clip, rect, centre)?,
        project(to_clip, rect, centre + normal * radius * TICK)?,
    ];
    Some((ring, tick))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> Rect {
        Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(800.0, 600.0))
    }

    #[test]
    fn the_outline_facing_the_camera_is_the_screen_circle() {
        let camera = OrbitCamera::default();
        let rect = view();
        let towards_eye = (camera.eye() - camera.target).normalize();
        let radius = 40.0 * camera.world_per_point(camera.target, rect.height());
        let (ring, tick) =
            surface_outline(&camera, rect, camera.target, towards_eye, radius).unwrap();
        let centre = rect.center();
        assert!(tick[0].distance(centre) < 0.01);
        for p in &ring {
            let r = p.distance(centre);
            assert!((r - 40.0).abs() < 0.5, "{r}");
        }
        // Seen end on, the tick is a dot.
        assert!(tick[0].distance(tick[1]) < 1.0);
    }

    #[test]
    fn the_outline_lies_on_the_surface_seen_edge_on() {
        // A surface seen from the side: its outline is a thin ellipse, as
        // wide as the brush across and flat along the view.
        let camera = OrbitCamera::default();
        let rect = view();
        let (_, up) = camera.right_up();
        let radius = 40.0 * camera.world_per_point(camera.target, rect.height());
        let (ring, tick) = surface_outline(&camera, rect, camera.target, up, radius).unwrap();
        let (mut lo, mut hi) = (Pos2::new(f32::MAX, f32::MAX), Pos2::new(f32::MIN, f32::MIN));
        for p in &ring {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        assert!((hi.x - lo.x - 80.0).abs() < 1.0, "{lo:?} {hi:?}");
        assert!(hi.y - lo.y < 4.0, "{lo:?} {hi:?}");
        // The tick points up the screen, a share of the radius long.
        let rise = tick[0].y - tick[1].y;
        assert!((rise - 40.0 * TICK).abs() < 1.0, "{rise}");
    }

    #[test]
    fn the_mirrored_brush_shows_only_where_it_can_be_seen() {
        let sculpt = start_sculpt();
        let camera = OrbitCamera::default();
        let eye = camera.eye();
        let radius = 0.1;
        let first = |dir: Vec3| sculpt.raycast(eye, dir.normalize()).unwrap();
        // From the front, a point to the right: its mirror image is the
        // same place to the left, in sight.
        let right = first(Vec3::new(0.4, 0.2, 0.0) - eye);
        assert!(right.point.x > 0.2);
        let left = mirrored_hit(&sculpt, eye, right.point, radius).unwrap();
        let expected = Vec3::new(-right.point.x, right.point.y, right.point.z);
        assert!(left.point.distance(expected) < 1e-3, "{left:?}");
        // On the symmetry plane, both dabs are one: no second outline.
        let middle = first(Vec3::new(0.0, 0.2, 0.0) - eye);
        assert!(mirrored_hit(&sculpt, eye, middle.point, radius).is_none());
        // From the right side, a point facing the eye: its mirror image is
        // on the far side, behind the clay.
        let mut side = camera;
        side.look_from(std::f32::consts::FRAC_PI_2 * 0.8, 0.0);
        let eye = side.eye();
        let near = sculpt
            .raycast(eye, (side.target - eye).normalize())
            .unwrap();
        assert!(near.point.x > 0.5);
        assert!(mirrored_hit(&sculpt, eye, near.point, radius).is_none());
        // A point off the clay: what the ray meets is too far from it.
        let off = Vec3::new(0.5, 0.0, 1.6);
        assert!(mirrored_hit(&sculpt, camera.eye(), off, radius).is_none());
    }

    #[test]
    fn symmetry_starts_off() {
        let tool = ToolSettings::default();
        assert!(!tool.symmetry);
        assert_eq!(tool.tool, Brush::Clay);
    }

    #[test]
    fn the_stabiliser_trails_the_pointer_and_pressure_follows() {
        let brush = Pos2::new(0.0, 0.0);
        // Off: the brush is the pointer.
        let to = Pos2::new(3.0, 4.0);
        assert_eq!(follow(brush, 0.2, to, 0.8, 0.0), (to, 0.8));
        // Slack: nothing moves.
        assert_eq!(follow(brush, 0.2, to, 0.8, 10.0), (brush, 0.2));
        // Taut: pulled along the string to its length behind the pointer,
        // with the pressure the same share of the way.
        let to = Pos2::new(30.0, 40.0);
        let (at, pressure) = follow(brush, 0.2, to, 0.8, 10.0);
        assert!((at.distance(to) - 10.0).abs() < 1e-4);
        assert!((at - Pos2::new(24.0, 32.0)).length() < 1e-4);
        assert!((pressure - 0.68).abs() < 1e-5);
    }

    #[test]
    fn a_stabilised_shaky_line_comes_out_straight_with_even_dabs() {
        // A pen drawing along x with a shake of ±3 points across.
        let samples: Vec<(Pos2, f32)> = (0..400)
            .map(|i| {
                let shake = match i {
                    0 => 0.0,
                    _ if i % 2 == 0 => 3.0,
                    _ => -3.0,
                };
                (Pos2::new(i as f32, shake), 0.5 + 0.001 * i as f32)
            })
            .collect();
        let spacing = 5.0;
        let mut dabs = Vec::new();
        let (mut brush, mut pressure) = samples[0];
        let mut carry = 0.0;
        for &(pointer, p) in &samples[1..] {
            let (to, to_pressure) = follow(brush, pressure, pointer, p, 30.0);
            carry = walk(brush, pressure, carry, to, to_pressure, spacing, |at, p| {
                dabs.push((at, p));
            });
            (brush, pressure) = (to, to_pressure);
        }
        assert!(dabs.len() > 50);
        // The shake mostly stays out: after the start, dabs keep within
        // a fraction of it of the line.
        for &(at, _) in &dabs[10..] {
            assert!(at.y.abs() < 0.6, "{at:?}");
        }
        // Even spacing along the path, and the pressure rises with it.
        for pair in dabs[10..].windows(2) {
            let gap = pair[0].0.distance(pair[1].0);
            assert!((gap - spacing).abs() < 0.5, "{gap}");
            assert!(pair[1].1 >= pair[0].1);
        }
        // Without the stabiliser every bit of the shake goes into the path.
        let mut raw = Vec::new();
        let mut carry = 0.0;
        for pair in samples.windows(2) {
            let ((a, pa), (b, pb)) = (pair[0], pair[1]);
            carry = walk(a, pa, carry, b, pb, spacing, |at, _| raw.push(at));
        }
        assert!(raw.iter().any(|at| at.y.abs() > 2.0));
    }
}
