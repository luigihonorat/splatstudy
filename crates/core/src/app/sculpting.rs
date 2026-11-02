// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Sculpting: the tools, strokes on the clay and the sculpt panel.

use std::sync::Arc;

use egui::{Key, Pos2, Rect, Stroke};
use glam::{Vec2, Vec3};

use super::{App, Drag, Request, SCULPT};
use crate::input::{FilePurpose, PointerKind};
use crate::meshio::{self, Soup};
use crate::reference::Reference;
use crate::render::clay::GpuMesh;
use crate::sculpt::{Brush, DEFAULT_LEVEL, Dab, Hit, Mesh};
use crate::ui::{self, i18n::Tr, menu, theme};
use crate::voxel;

/// Dabs are placed every this fraction of the tool radius along a stroke.
const SPACING: f32 = 0.15;

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

pub(super) struct ToolSettings {
    pub(super) tool: Brush,
    /// Radius on screen, in points.
    pub(super) radius: f32,
    /// Strength per tool at full pen pressure, 0..1, in `Brush::ALL` order.
    pub(super) strength: [f32; 7],
    pub(super) symmetry: bool,
    /// Pen pressure scales the size as well as the strength.
    pub(super) pressure_size: bool,
    /// Remesh resolution: cells across the clay's longest side.
    pub(super) resolution: f32,
    /// Clay from splats: the density that counts as material, 0..1.
    pub(super) threshold: f32,
    /// Asked for in this frame, done at the start of the next, so the
    /// notice saying so shows while it runs.
    pending: Option<Pending>,
}

/// A slow mesh operation waiting for the next frame.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pending {
    Remesh,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            tool: Brush::Clay,
            radius: 48.0,
            strength: BRUSHES.map(|(_, _, strength)| strength),
            symmetry: true,
            pressure_size: false,
            resolution: voxel::DEFAULT_RESOLUTION as f32,
            threshold: voxel::DEFAULT_THRESHOLD,
            pending: None,
        }
    }
}

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

/// `Brush::ALL` lists the brushes in declaration order.
fn tool_index(tool: Brush) -> usize {
    tool as usize
}

/// The clay every scene starts from, and every timelapse.
pub(super) fn start_mesh() -> Mesh {
    Mesh::icosphere(DEFAULT_LEVEL)
}

impl App {
    /// Undo, redo, tool and size keys.
    pub(super) fn sculpt_shortcuts(&mut self, ui: &egui::Ui) {
        use egui::Modifiers as M;
        let pressed = |m: M, key: Key| ui.input_mut(|i| i.consume_key(m, key));
        // The timelapse shows the clay as it was: no sculpting until it closes.
        if self.playback.is_none() {
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
        self.playback = None;
        self.replace_clay(mesh);
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
    /// makes its GPU buffers.
    pub(super) fn replace_clay(&mut self, mesh: Mesh) {
        self.sculpt.replace_mesh(mesh);
        self.sync_clay();
    }

    /// The controls for making clay from the reference, for the reference
    /// panel: the threshold (for splats) and the button, enabled when a
    /// reference is `loaded`. True when the button was clicked: the caller
    /// then hands the reference to [`App::clay_from_reference`].
    #[allow(dead_code)] // until the reference panel shows them
    pub(super) fn reference_to_clay_controls(&mut self, ui: &mut egui::Ui, loaded: bool) -> bool {
        let tr = self.lang.tr();
        let live = self.playback.is_none();
        ui.add_enabled_ui(loaded && live, |ui| {
            ui::slider(
                ui,
                tr.mesh.threshold,
                &mut self.tool.threshold,
                0.05..=1.0,
                |v| format!("{:.0}%", v * 100.0),
            );
            ui::choice(ui, tr.mesh.from_reference, false).clicked()
        })
        .inner
    }

    /// Replaces the clay with the shape of the reference, at the remesh
    /// resolution. Undo brings the previous clay back. The reference lives
    /// in the reference part's state, so take it out to call this:
    /// `if let Some(r) = state.take() { self.clay_from_reference(&r); state = Some(r) }`.
    #[allow(dead_code)] // until the reference panel calls it
    pub(super) fn clay_from_reference(&mut self, reference: &Reference) {
        let tr = self.lang.tr();
        let resolution = self.tool.resolution.round() as u32;
        match voxel::clay_from_reference(reference, resolution, self.tool.threshold) {
            Some(mesh) => {
                self.playback = None;
                self.replace_clay(mesh);
                self.notify(tr.mesh.made_from_reference);
            }
            None => self.notify(tr.mesh.nothing_left),
        }
    }

    /// Remeshes the clay at the next frame, saying so first.
    fn ask_remesh(&mut self) {
        if self.playback.is_none() {
            self.tool.pending = Some(Pending::Remesh);
            let tr = self.lang.tr();
            self.notify(format!("{}…", tr.mesh.remesh));
        }
    }

    /// Does the operation asked for in the last frame.
    fn run_pending(&mut self) {
        match self.tool.pending.take() {
            Some(Pending::Remesh) => self.remesh(),
            None => {}
        }
    }

    /// Rebuilds the clay as an even mesh at the chosen resolution.
    pub(super) fn remesh(&mut self) {
        let tr = self.lang.tr();
        match voxel::remesh(&self.sculpt.mesh, self.tool.resolution.round() as u32) {
            Some(mesh) => {
                self.replace_clay(mesh);
                self.notify(tr.mesh.remeshed);
            }
            None => self.notify(tr.mesh.nothing_left),
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
        if menu::action(ui, live, tr.mirror_sculpt, "") {
            self.sculpt.mirror(true);
        }
        if menu::action(ui, live, tr.mesh.mirror_to_positive, "") {
            self.sculpt.mirror(false);
        }
        if menu::action(ui, live, tr.mesh.remesh, "Shift+R") {
            self.ask_remesh();
        }
    }

    /// Makes the clay's GPU buffers again after its mesh was replaced (a
    /// remesh, an import, or undoing one, from the panel, the keys or the
    /// edit menu). The panel runs after the menus and before the frame
    /// uploads the clay, so checking here catches every case.
    pub(super) fn sync_clay(&mut self) {
        if self.sculpt.take_topology_changed() {
            self.gpu_mesh = GpuMesh::new(&self.device, &self.sculpt.mesh);
            self.sculpt.take_dirty();
            // A stroke in progress held vertices of the old mesh.
            if !matches!(self.drags[SCULPT], Some(Drag::Navigate { .. })) {
                self.drags[SCULPT] = None;
            }
        }
    }

    pub(super) fn sculpt_panel(&mut self, ui: &mut egui::Ui) {
        self.run_pending();
        self.sync_clay();
        if self.playback.is_some() {
            self.playback_panel(ui);
            return;
        }
        let tr = self.lang.tr();
        ui::section(ui, tr.tools);
        for (brush, _, _) in BRUSHES {
            if ui::choice(ui, brush_name(tr, brush), self.tool.tool == brush).clicked() {
                self.tool.tool = brush;
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
        if ui::choice(ui, tr.pressure_size, self.tool.pressure_size).clicked() {
            self.tool.pressure_size = !self.tool.pressure_size;
        }
        ui.add_space(10.0);
        ui::section(ui, tr.mesh.mesh);
        let range = voxel::MIN_RESOLUTION as f32..=voxel::MAX_RESOLUTION as f32;
        ui::slider(
            ui,
            tr.mesh.resolution,
            &mut self.tool.resolution,
            range,
            |v| format!("{v:.0}"),
        );
        if ui::choice(ui, tr.mesh.remesh, false).clicked() {
            self.ask_remesh();
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
        let recorded = !self.sculpt.log().is_empty();
        if ui
            .add_enabled_ui(recorded, |ui| ui::choice(ui, tr.watch_timelapse, false))
            .inner
            .clicked()
        {
            self.start_playback();
        }
        self.sculpt.refresh();
        self.sync_clay();
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

    /// Continues a clay or smooth stroke to `to`, placing dabs at even
    /// spacing and interpolating the pressure between samples.
    pub(super) fn stroke_to(&mut self, rect: Rect, to: Pos2, pressure: f32) {
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
        // The way the pointer heads, in the view plane; brushes that follow
        // the stroke take its part along the surface.
        let (right, up) = self.cameras[SCULPT].right_up();
        let d = to - last;
        let direction = (right * d.x - up * d.y).normalize_or_zero();
        let mut along = spacing - carry;
        while along <= length {
            let t = along / length.max(1e-6);
            if let Some(hit) = self.pick(rect, last.lerp(to, t)) {
                let pressure = last_pressure + (pressure - last_pressure) * t;
                self.dab_at(rect, pressure, hit, direction);
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

    /// One dab: pen pressure scales the tool's strength, and its size too if
    /// that is on.
    fn dab_at(&mut self, rect: Rect, pressure: f32, hit: Hit, direction: Vec3) {
        let (ctrl, shift) = self.ctx.input(|i| (i.modifiers.ctrl, i.modifiers.shift));
        let tool = if shift { Brush::Smooth } else { self.tool.tool };
        let pressure = pressure.clamp(0.0, 1.0);
        let strength = self.tool.strength[tool_index(tool)] * pressure;
        let size = if self.tool.pressure_size {
            // Never quite vanishing, so a light touch still shows.
            0.1 + 0.9 * pressure
        } else {
            1.0
        };
        let scale = self.cameras[SCULPT].world_per_point(hit.point, rect.height());
        self.sculpt.dab(Dab {
            brush: tool,
            centre: hit.point,
            radius: self.tool.radius * size * scale,
            strength,
            invert: ctrl,
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

    /// The tool outline around the pointer, while it is over the clay view.
    pub(super) fn tool_cursor(&self, ui: &egui::Ui, rect: Rect) {
        let Some(pos) = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| rect.contains(*p))
        else {
            return;
        };
        if self.playback.is_some() || matches!(self.drags[SCULPT], Some(Drag::Navigate { .. })) {
            return;
        }
        let ink = theme::VIEW_INK.gamma_multiply(0.6);
        let painter = ui.painter().with_clip_rect(rect);
        painter.circle_stroke(pos, self.tool.radius, Stroke::new(1.0, ink));
        painter.circle_filled(pos, 1.5, ink);
    }
}
