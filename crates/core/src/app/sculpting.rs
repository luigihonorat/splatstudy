// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Sculpting: the tools, strokes on the clay and the sculpt panel.

use std::sync::Arc;

use egui::{Key, Pos2, Rect, Stroke};
use glam::{Vec2, Vec3};

use super::{App, Drag, Request, SCULPT};
use crate::input::PointerKind;
use crate::meshio;
use crate::sculpt::{Brush, DEFAULT_LEVEL, Dab, Hit, Mesh};
use crate::ui::{self, i18n::Tr, menu, theme};

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
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            tool: Brush::Clay,
            radius: 48.0,
            strength: BRUSHES.map(|(_, _, strength)| strength),
            symmetry: true,
            pressure_size: false,
        }
    }
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
        }
        if pressed(M::NONE, Key::OpenBracket) {
            self.tool.radius = (self.tool.radius / 1.2).max(4.0);
        }
        if pressed(M::NONE, Key::CloseBracket) {
            self.tool.radius = (self.tool.radius * 1.2).min(400.0);
        }
    }

    /// Replaces the clay with a mesh from a file. Not written yet.
    pub(super) fn import_sculpt(&mut self, name: &str, _bytes: Arc<[u8]>) {
        self.cannot_open(name);
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
        menu::action(ui, false, tr.mirror_sculpt, "");
    }

    pub(super) fn sculpt_panel(&mut self, ui: &mut egui::Ui) {
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
