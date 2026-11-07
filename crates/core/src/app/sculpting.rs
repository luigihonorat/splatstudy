// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Sculpting: the tools, strokes on the clay and the sculpt panel.

mod levels;
mod tool_pie;

use std::sync::Arc;

use egui::{Key, Pos2, Rect, Stroke};
use glam::{Mat4, Vec2, Vec3};

use super::panels::Group;
use super::{App, Drag, Request, SCULPT};
use crate::camera::OrbitCamera;
use crate::input::{Button, FilePurpose, PointerEvent, PointerKind, PointerPhase};
use crate::meshio::{self, Soup};
use crate::quad;
use crate::render::clay::GpuMesh;
pub(super) use tool_pie::TOOL_PIE_KEY;

use crate::sculpt::{
    Brush, Buildup, Dab, Falloff, Hit, LayerOp, Levels, MAX_HARDNESS, MAX_TRIANGLES, MAX_VERTICES,
    Mesh, START_LEVEL, Sculpt, cube_sphere,
};
use crate::ui::{self, i18n::Tr, icons, menu};
use crate::voxel;

/// Dabs are placed every this fraction of their radius along a stroke,
/// or closer for a hard edge (see [`spacing`]).
const SPACING: f32 = 0.15;
/// The widest gap between dabs, as a share of the falloff's drop to the
/// rim: wider, and each dab's edge shows as a step along the stroke.
const GAP_PER_SHOULDER: f32 = 0.75;

/// Sides of the brush outline drawn on the clay.
const CURSOR_SEGMENTS: usize = 48;
/// Length of the normal tick at the centre of that outline, as a share of
/// the brush radius.
const TICK: f32 = 0.3;

/// The stabiliser's longest string, in points.
const MAX_STABILISER: f32 = 120.0;

/// Pen pressure scales the brush's strength and its size: its size at the
/// lightest touch, as a share of its full size.
const MIN_PRESSURE_SIZE: f32 = 0.2;

/// The brush's size at `pressure`, as a share of its full size.
fn pressure_size(pressure: f32) -> f32 {
    MIN_PRESSURE_SIZE + (1.0 - MIN_PRESSURE_SIZE) * pressure.clamp(0.0, 1.0)
}

/// How far apart a stroke's dabs are, as a share of their radius: closer
/// for a hard edge, whose drop to the rim must span more than a gap.
fn spacing(falloff: Falloff) -> f32 {
    SPACING.min(GAP_PER_SHOULDER * falloff.shoulder())
}

/// How long the size and strength show by the brush after they change,
/// in seconds.
const READOUT: f64 = 1.2;

/// How long a newly chosen brush's name shows at the top of the sculpt
/// view, and the last part of it, in which it fades out, in seconds.
const BRUSH_FLASH: f64 = 1.0;
const BRUSH_FADE: f64 = 0.3;

/// Each brush with its letter key and starting strength, in `Brush::ALL`
/// order; its digit is in `DIGITS`.
/// (F frames the sculpt, so flatten takes T, for trim; M is move, so the
/// mask takes A.)
pub(super) const BRUSHES: [(Brush, Key, f32); 8] = [
    (Brush::Move, Key::M, 1.0),
    (Brush::Clay, Key::C, 0.5),
    (Brush::Flatten, Key::T, 0.5),
    (Brush::Smooth, Key::S, 0.5),
    (Brush::Inflate, Key::I, 0.5),
    (Brush::Crease, Key::X, 0.5),
    (Brush::Pinch, Key::P, 0.5),
    (Brush::Mask, Key::A, 1.0),
];

/// The digit row, 1 to 9 then 0: tool `k` of `BRUSHES` takes `DIGITS[k]`,
/// so a new tool gets the next digit. 9 and 0 are free.
const DIGITS: [Key; 10] = [
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
    Key::Num5,
    Key::Num6,
    Key::Num7,
    Key::Num8,
    Key::Num9,
    Key::Num0,
];
const _: () = assert!(BRUSHES.len() <= DIGITS.len(), "a tool without a digit");

pub(super) fn brush_name(tr: &Tr, brush: Brush) -> &'static str {
    match brush {
        Brush::Move => tr.move_,
        Brush::Clay => tr.clay,
        Brush::Flatten => tr.flatten,
        Brush::Smooth => tr.smooth,
        Brush::Inflate => tr.inflate,
        Brush::Crease => tr.crease,
        Brush::Pinch => tr.pinch,
        Brush::Mask => tr.mesh.mask,
        Brush::BlurMask => tr.mesh.blur_mask,
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
        Brush::Mask => icons::mask_tool,
        Brush::BlurMask => icons::blur_mask_tool,
    }
}

/// The digit that picks `brush`, as the help shows it.
pub(super) fn brush_digit(brush: Brush) -> &'static str {
    DIGITS[tool_index(brush)].name()
}

/// The keys that pick `brush`, as the shortcuts window and the toolbar's
/// tooltip show them: its digit, its letter, and for smooth Shift, which
/// smooths with any tool while it is held.
pub(super) fn brush_keys(brush: Brush) -> String {
    let index = tool_index(brush);
    let keys = format!("{} / {}", brush_digit(brush), BRUSHES[index].1.name());
    if brush == Brush::Smooth {
        format!("{keys} / Shift")
    } else {
        keys
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

fn buildup_name(tr: &Tr, buildup: Buildup) -> &'static str {
    match buildup {
        Buildup::Layer => tr.mesh.buildup_layer,
        Buildup::Accumulate => tr.mesh.buildup_accumulate,
    }
}

/// A heading, then its options in a row; the chosen one underlined.
fn choices<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    heading: &str,
    options: &[(&str, T)],
    chosen: &mut T,
) {
    ui::section(ui, heading);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        for &(text, value) in options {
            if ui::choice(ui, text, *chosen == value).clicked() {
                *chosen = value;
            }
        }
    });
}

/// What it switches, then on / off.
fn switch(ui: &mut egui::Ui, tr: &Tr, on: &mut bool, label: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        let muted = ui.visuals().widgets.inactive.fg_stroke.color;
        ui.label(egui::RichText::new(label).color(muted));
        for (text, value) in [(tr.mesh.switch_on, true), (tr.mesh.switch_off, false)] {
            if ui::choice(ui, text, *on == value).clicked() {
                *on = value;
            }
        }
    });
}

pub(super) struct ToolSettings {
    pub(super) tool: Brush,
    /// Radius on screen, in points.
    pub(super) radius: f32,
    /// Strength per tool at full pen pressure, 0..1, in `Brush::ALL` order.
    pub(super) strength: [f32; 8],
    /// How hard each tool's edge is, from smooth (0) to past sharp
    /// ([`MAX_HARDNESS`]), in `Brush::ALL` order (see [`Falloff::Hard`]).
    pub(super) hardness: [f32; 8],
    /// Whether the clay builds more where a stroke passes again.
    pub(super) buildup: Buildup,
    /// Whether the clay fills the hollows first (see `Dab::fill`).
    pub(super) fill: bool,
    /// When the size or strength last changed, to show them by the brush.
    readout: Option<f64>,
    /// When a brush was last chosen, to show its name.
    flash: Option<f64>,
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

impl ToolSettings {
    /// How `tool` fades towards its rim: its hardness, to the slider's step.
    pub(super) fn falloff(&self, tool: Brush) -> Falloff {
        Falloff::Hard(self.hardness[tool_index(tool)].round() as u8)
    }
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
    /// Saving a copy of it without its timelapse.
    SaveCopy,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            tool: Brush::Clay,
            radius: 48.0,
            strength: BRUSHES.map(|(_, _, strength)| strength),
            // Clay starts with the hardest edge, building up as it passes
            // again and filling the hollows first; the rest start smooth.
            hardness: BRUSHES.map(|(brush, _, _)| {
                if brush == Brush::Clay {
                    f32::from(MAX_HARDNESS)
                } else {
                    0.0
                }
            }),
            buildup: Buildup::Accumulate,
            fill: true,
            readout: None,
            flash: None,
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

/// Which of a frame's `samples` go on with a stroke started by the press
/// `by`: that pointer's own, while it is pressed, with some pressure. Any
/// other pointer's (a mouse event the system makes from a pen, say) are
/// not part of the stroke, and neither is what a pen reports after it
/// lifts, if it touches down again within the frame. In the frame the
/// stroke `started`, they start at its press.
fn stroke_samples(
    samples: &[PointerEvent],
    by: Option<PointerEvent>,
    started: bool,
) -> impl Iterator<Item = &PointerEvent> {
    let ours = move |s: &PointerEvent| by.is_none_or(|p| s.id == p.id && s.kind == p.kind);
    let press =
        |s: &PointerEvent| s.phase == PointerPhase::Down && s.button == Some(Button::Primary);
    let from = if started {
        samples
            .iter()
            .rposition(|s| ours(s) && press(s))
            .unwrap_or(0)
    } else {
        0
    };
    let mut pressed = true;
    samples[from..].iter().filter(move |s| {
        if !ours(s) {
            return false;
        }
        match s.phase {
            PointerPhase::Down if s.button == Some(Button::Primary) => pressed = true,
            PointerPhase::Up if s.button == Some(Button::Primary) => pressed = false,
            PointerPhase::Cancel => pressed = false,
            _ => {}
        }
        pressed && matches!(s.phase, PointerPhase::Move | PointerPhase::Down) && s.pressure > 0.0
    })
}

/// Where `tool`'s settings are kept: `Brush::ALL` lists the brushes in
/// declaration order. The mask blur, held with Ctrl + Shift, has none of
/// its own: Shift smooths, and it smooths the mask with the Smooth
/// brush's strength and falloff.
fn tool_index(tool: Brush) -> usize {
    match tool {
        Brush::BlurMask => Brush::Smooth as usize,
        tool => tool as usize,
    }
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
    /// Whether a stroke, a grab or a mask tap is under way in the sculpt
    /// view. Undo and redo wait until it ends: undone under an open
    /// stroke, a level change or a subdivide would swap the clay, and the
    /// stroke would then close as an undo step of the other level's
    /// vertices; an ordinary step undone would come partly back.
    pub(super) fn sculpt_drag_open(&self) -> bool {
        matches!(
            self.drags[SCULPT],
            Some(Drag::Stroke { .. } | Drag::Grab { .. } | Drag::MaskTap { .. })
        )
    }

    /// Undo, redo, tool and size keys.
    pub(super) fn sculpt_shortcuts(&mut self, ui: &egui::Ui) {
        use egui::Modifiers as M;
        let pressed = |m: M, key: Key| ui.input_mut(|i| i.consume_key(m, key));
        // The timelapse shows the clay as it was: no sculpting until it closes.
        if self.playback.is_none() {
            self.tool_pie_keys(ui);
            // The keys are taken even mid-stroke, so they don't fall
            // through to something else, but do nothing until it ends.
            let busy = self.sculpt_drag_open();
            if pressed(M::COMMAND | M::SHIFT, Key::Z) || pressed(M::COMMAND, Key::Y) {
                if !busy {
                    self.sculpt.redo();
                }
            } else if pressed(M::COMMAND, Key::Z) && !busy {
                self.sculpt.undo();
            }
            // Before the tools: plain M would take Alt+M too.
            if pressed(M::COMMAND, Key::I) {
                self.sculpt.edit_layer(LayerOp::InvertMask);
            }
            if pressed(M::ALT, Key::M) {
                self.sculpt.edit_layer(LayerOp::ClearMask);
            }
            for ((brush, key, _), digit) in BRUSHES.into_iter().zip(DIGITS) {
                if pressed(M::NONE, digit) || pressed(M::NONE, key) {
                    self.choose_brush(brush);
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
        // Strength first: plain [ ] would take Shift + [ ] too. Browsers
        // name the shifted keys { }, the desktop [ ] with Shift.
        let index = tool_index(self.tool.tool);
        let strength = self.tool.strength[index];
        if pressed(M::SHIFT, Key::OpenBracket) || pressed(M::NONE, Key::OpenCurlyBracket) {
            self.tool.strength[index] = (strength - 0.05).max(0.0);
        } else if pressed(M::SHIFT, Key::CloseBracket) || pressed(M::NONE, Key::CloseCurlyBracket) {
            self.tool.strength[index] = (strength + 0.05).min(1.0);
        }
        let radius = self.tool.radius;
        if pressed(M::NONE, Key::OpenBracket) {
            self.tool.radius = (self.tool.radius / 1.2).max(4.0);
        }
        if pressed(M::NONE, Key::CloseBracket) {
            self.tool.radius = (self.tool.radius * 1.2).min(400.0);
        }
        if self.tool.radius != radius || self.tool.strength[index] != strength {
            self.tool.readout = Some(self.now);
            // The brush may have been drawn already this frame.
            ui.ctx().request_repaint();
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
        if self.playback.is_none() || matches!(pending, Pending::SaveScene | Pending::SaveCopy) {
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
            Pending::SaveCopy => self.save_copy(),
        }
    }

    /// Rebuilds the clay as an even mesh at the chosen resolution, with
    /// its mask and chalk.
    pub(super) fn remesh(&mut self) {
        let tr = self.lang.tr();
        match voxel::remesh(&self.sculpt.mesh, self.tool.resolution.round() as u32) {
            Some(mut mesh) => {
                mesh.layers = self.sculpt.layers_for(&mesh);
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
    /// [`quad::remesh`]), with its mask and chalk; leaves it as it is,
    /// with a notice, if that fails.
    pub(super) fn quad_remesh(&mut self) {
        let m = self.lang.tr().mesh;
        let quads = self.tool.quads.round() as u32;
        match quad::remesh(&self.sculpt.mesh, quads) {
            Some(mut mesh) => {
                mesh.layers = self.sculpt.layers_for(&mesh);
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

    /// Whole-mesh edits in the object menu.
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
        ui.separator();
        let layers = &self.sculpt.mesh.layers;
        let (masked, chalked) = (!layers.mask.is_empty(), !layers.chalk.is_empty());
        let m = tr.mesh;
        for (op, label, keys, enabled) in [
            (LayerOp::InvertMask, m.invert_mask, self.keys("I"), true),
            (LayerOp::ClearMask, m.clear_mask, "Alt+M".to_owned(), masked),
            (LayerOp::AddChalk, m.add_chalk, String::new(), true),
            (LayerOp::RemoveChalk, m.remove_chalk, String::new(), chalked),
        ] {
            if menu::action(ui, live && enabled, label, &keys) {
                self.sculpt.edit_layer(op);
            }
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
    /// and the remeshes; those the window menu left in.
    pub(super) fn sculpt_panel(&mut self, ui: &mut egui::Ui) {
        if self.playback.is_some() {
            self.playback_panel(ui);
            return;
        }
        let tr = self.lang.tr();
        let m = tr.mesh;
        if self.panels.holds(Group::ToolSettings) {
            ui::group(ui, "tool settings", m.tool_settings, |ui| {
                self.tool_settings(ui)
            });
        }
        if self.panels.holds(Group::Subdivision) {
            ui::group(ui, "subdivision", m.subdivision, |ui| {
                if ui::choice(ui, m.subdivide, false).clicked() {
                    self.ask_subdivide();
                }
                self.levels_panel(ui);
            });
        }
        if self.panels.holds(Group::Remeshing) {
            ui::group(ui, "remeshing", m.remeshing, |ui| self.remesh_controls(ui));
        }
        if self.panels.holds(Group::QuadRemesh) {
            ui::group(ui, "quad", m.quad_remesh, |ui| {
                self.quad_remesh_controls(ui)
            });
        }
    }

    /// The tools, and the chosen one's size, strength, stabiliser,
    /// falloff and symmetry.
    fn tool_settings(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        let tool = self.tool.tool;
        let index = tool_index(tool);
        let size = ui::slider(ui, tr.size, &mut self.tool.radius, 4.0..=400.0, |v| {
            format!("{v:.0}")
        });
        let strength = &mut self.tool.strength[index];
        let strength = ui::slider(ui, tr.strength, strength, 0.0..=1.0, |v| {
            format!("{:.0}%", v * 100.0)
        });
        if size.changed() || strength.changed() {
            self.tool.readout = Some(self.now);
            // The brush may have been drawn already this frame.
            ui.ctx().request_repaint();
        }
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
        if tool == Brush::Clay {
            let options = Buildup::ALL.map(|b| (buildup_name(tr, b), b));
            choices(ui, tr.mesh.buildup, &options, &mut self.tool.buildup);
            ui.add_space(4.0);
            switch(ui, tr, &mut self.tool.fill, tr.mesh.fill);
        }
        let hardness = &mut self.tool.hardness[index];
        ui::slider(
            ui,
            tr.mesh.falloff,
            hardness,
            0.0..=f32::from(MAX_HARDNESS),
            |v| format!("{v:.0}"),
        );
        falloff_curve(ui, self.tool.falloff(tool));
        ui.add_space(10.0);
        switch(ui, tr, &mut self.tool.symmetry, tr.mesh.symmetry_x);
    }

    /// The voxel remesh, its button over its resolution.
    fn remesh_controls(&mut self, ui: &mut egui::Ui) {
        let m = self.lang.tr().mesh;
        if ui::choice(ui, m.remesh, false).clicked() {
            self.ask_remesh();
        }
        self.resolution_slider(ui);
    }

    /// The quad remesh, its button over its number of quads.
    fn quad_remesh_controls(&mut self, ui: &mut egui::Ui) {
        let m = self.lang.tr().mesh;
        if ui::choice(ui, m.quad_remesh, false).clicked() {
            self.ask_quad_remesh();
        }
        let range = quad::MIN_QUADS as f32..=quad::MAX_QUADS as f32;
        ui::typed_slider(ui, m.quads, &mut self.tool.quads, range, 100.0, |v| {
            format!("{v:.0}")
        });
    }

    /// The voxel resolution, which the remesh and the mesh from the
    /// reference share.
    fn resolution_slider(&mut self, ui: &mut egui::Ui) {
        let m = self.lang.tr().mesh;
        let range = voxel::MIN_RESOLUTION as f32..=voxel::MAX_RESOLUTION as f32;
        ui::slider(ui, m.resolution, &mut self.tool.resolution, range, |v| {
            format!("{v:.0}")
        });
    }

    /// The mesh from the reference, in the reference panel: the button,
    /// then its resolution (the remesh's too) and for splats the density
    /// it keeps.
    pub(super) fn mesh_from_reference_controls(&mut self, ui: &mut egui::Ui) {
        let m = self.lang.tr().mesh;
        let live = self.playback.is_none();
        if ui
            .add_enabled_ui(live, |ui| ui::choice(ui, m.from_reference, false))
            .inner
            .clicked()
        {
            self.ask(Pending::FromReference, m.from_reference);
        }
        self.resolution_slider(ui);
        let splats = matches!(
            self.reference.current.as_ref().map(|r| &r.content),
            Some(crate::reference::Content::Splats(_))
        );
        if splats {
            let threshold = &mut self.tool.threshold;
            ui::slider(ui, m.threshold, threshold, 0.05..=1.0, |v| {
                format!("{:.0}%", v * 100.0)
            });
        }
    }

    /// Makes `brush` the tool, its name showing for a moment at the top
    /// of the sculpt view.
    pub(super) fn choose_brush(&mut self, brush: Brush) {
        self.tool.tool = brush;
        self.tool.flash = Some(self.now);
        self.dirty = true;
    }

    /// The chosen brush's name in a box at the top of the sculpt view
    /// (`rect`), for a moment after it was chosen, fading out.
    pub(super) fn brush_flash(&mut self, ui: &egui::Ui, rect: Rect) {
        let Some(at) = self.tool.flash else {
            return;
        };
        let age = self.now - at;
        if age >= BRUSH_FLASH {
            self.tool.flash = None;
            return;
        }
        let opacity = ((BRUSH_FLASH - age) / BRUSH_FADE).min(1.0) as f32;
        let p = self.theme.palette();
        let name = brush_name(self.lang.tr(), self.tool.tool);
        egui::Area::new(egui::Id::unique("brush flash"))
            .fixed_pos(rect.center_top() + egui::vec2(0.0, 12.0))
            .pivot(egui::Align2::CENTER_TOP)
            .interactable(false)
            .show(ui.ctx(), |ui| {
                ui.multiply_opacity(opacity);
                egui::Frame::NONE
                    .fill(p.bg)
                    .stroke(Stroke::new(1.0, p.border))
                    .inner_margin(egui::Margin::symmetric(14, 6))
                    // An area at a centre pivot lays out in almost no width,
                    // so the name would wrap letter by letter.
                    .show(ui, |ui| ui.add(egui::Label::new(name).extend()));
            });
        // Each frame while it fades, then once more to take it away.
        let wait = if age < BRUSH_FLASH - BRUSH_FADE {
            BRUSH_FLASH - BRUSH_FADE - age
        } else {
            0.0
        };
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(wait));
    }

    /// True while the pen is used with its eraser end: the brush does
    /// its opposite action, as with the invert key. A stroke keeps the
    /// end it started with, whatever other pointer moves meanwhile.
    fn erasing(&self) -> bool {
        let pointer = match self.drags[SCULPT] {
            Some(Drag::Stroke { pointer, .. }) => pointer,
            _ => self.input.last,
        };
        pointer.is_some_and(|e| e.kind == PointerKind::Pen && e.eraser)
    }

    /// The press that starts a stroke this frame: the last left press
    /// among the frame's samples. Not the frame's last sample, which may
    /// be the lift, or another pointer's: a mouse move the system adds
    /// where a pen lifted would make a dab of full pressure.
    fn stroke_press(&self) -> Option<PointerEvent> {
        self.input
            .samples
            .iter()
            .rev()
            .find(|s| s.phase == PointerPhase::Down && s.button == Some(Button::Primary))
            .or(self.input.last.as_ref())
            .copied()
    }

    /// The samples of this frame that go on with the stroke under way, in
    /// view points, with their pressure. `started` is true in the frame
    /// the stroke started in.
    pub(super) fn stroke_samples(
        &self,
        index: usize,
        rect: Rect,
        started: bool,
    ) -> Vec<(Pos2, f32)> {
        let Some(Drag::Stroke { pointer, .. }) = self.drags[SCULPT] else {
            return Vec::new();
        };
        stroke_samples(&self.input.samples, pointer, started)
            .map(|s| {
                let pos = self.view_pos_in(index, rect, egui::pos2(s.pos[0], s.pos[1]));
                (pos, s.brush_pressure())
            })
            .collect()
    }

    pub(super) fn start_stroke(&mut self, rect: Rect, pos: Pos2, hit: Hit) {
        self.sculpt.begin_stroke();
        let camera = &self.cameras[SCULPT];
        let scale = camera.world_per_point(hit.point, rect.height());
        if self.held_tool() == Brush::Move {
            let (right, up) = camera.right_up();
            self.sculpt.set_falloff(self.tool.falloff(Brush::Move));
            self.sculpt
                .grab(hit.point, self.tool.radius * scale, self.tool.symmetry);
            self.drags[SCULPT] = Some(Drag::Grab {
                start: pos,
                right,
                up,
                scale,
            });
        } else {
            let pointer = self.stroke_press();
            let pressure = pointer.map_or(1.0, |e| e.brush_pressure());
            self.drags[SCULPT] = Some(Drag::Stroke {
                last: pos,
                pressure,
                carry: 0.0,
                pointer,
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
            pointer,
        }) = self.drags[SCULPT]
        else {
            return;
        };
        let (to, pressure) = follow(last, last_pressure, to, pressure, self.tool.stabiliser);
        // Pressure sizes each dab: spaced for the smaller at either end.
        let size = self.tool.radius * pressure_size(last_pressure.min(pressure));
        let spacing = (size * spacing(self.tool.falloff(self.held_tool()))).max(1.0);
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
            // The last dab moved the clay out of its boxes: a ray through
            // them would miss it, and find the far side of the form.
            self.sculpt.refresh();
            if let Some(hit) = self.pick(rect, pos) {
                self.dab_at(rect, pressure, hit, direction);
            }
        }
        self.drags[SCULPT] = Some(Drag::Stroke {
            last: to,
            pressure,
            carry,
            pointer,
        });
    }

    /// The tool a stroke uses with the keys held now: in ZBrush style Ctrl
    /// masks, whatever the tool (and Ctrl + Alt unmasks), and Ctrl + Shift
    /// blurs the mask; Shift smooths.
    fn held_tool(&self) -> Brush {
        let (m, shift) = self.ctx.input(|i| (i.modifiers.into(), i.modifiers.shift));
        if self.nav.masks(m) {
            if shift { Brush::BlurMask } else { Brush::Mask }
        } else if shift {
            Brush::Smooth
        } else {
            self.tool.tool
        }
    }

    /// One dab: pen pressure scales the tool's strength and size.
    fn dab_at(&mut self, rect: Rect, pressure: f32, hit: Hit, direction: Vec3) {
        let m = self.ctx.input(|i| i.modifiers.into());
        let tool = self.held_tool();
        let index = tool_index(tool);
        let pressure = pressure.clamp(0.0, 1.0);
        let falloff = self.tool.falloff(tool);
        // Closer dabs each lay less, so a stroke builds as much; a stroke
        // in layers stops at its layer however close they are.
        let layers = tool == Brush::Clay && self.tool.buildup == Buildup::Layer;
        let density = if layers {
            1.0
        } else {
            spacing(falloff) / SPACING
        };
        let strength = self.tool.strength[index] * pressure * density;
        let size = pressure_size(pressure);
        let scale = self.cameras[SCULPT].world_per_point(hit.point, rect.height());
        self.sculpt.set_falloff(falloff);
        self.sculpt.dab(Dab {
            brush: tool,
            centre: hit.point,
            radius: self.tool.radius * size * scale,
            strength,
            invert: self.nav.inverts(m) || self.erasing(),
            symmetry: self.tool.symmetry,
            direction,
            buildup: self.tool.buildup,
            fill: self.tool.fill,
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
    /// lying on the clay under the brush, or a circle on the screen where there is no clay. A fainter ring inside marks
    /// where the brush works at half strength. The outline is dashed while
    /// the brush does its opposite action, and a held tool (Shift smooths)
    /// shows its icon. In a stabilised stroke the brush trails the pointer,
    /// and a thin line joins them. Just after the size or strength
    /// changed, both show by the brush.
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
        let ink = self.view_inks(SCULPT).ink.gamma_multiply(0.6);
        let line = Stroke::new(1.0, ink);
        let painter = ui.painter().with_clip_rect(rect);
        let held = self.held_tool();
        let falloff = self.tool.falloff(held);
        let m = ui.input(|i| i.modifiers.into());
        // Not for the mask tools: the blur has no opposite action, and the
        // mask's isn't dashed either.
        let dashed = !matches!(held, Brush::Mask | Brush::BlurMask)
            && (self.nav.inverts(m) || self.erasing());
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
        let outline = |ring: Vec<Pos2>, line: Stroke| {
            let mut ring: Vec<Pos2> = ring.into_iter().map(|p| self.view_pos(rect, p)).collect();
            if dashed {
                ring.push(ring[0]);
                painter.extend(egui::Shape::dashed_line(&ring, line, 4.0, 3.0));
            } else {
                painter.add(egui::Shape::closed_line(ring, line));
            }
        };
        let draw = |ring: Vec<Pos2>, tick: [Pos2; 2], line: Stroke| {
            outline(ring, line);
            painter.line_segment(tick.map(|p| self.view_pos(rect, p)), line);
        };
        let half = half_weight(falloff);
        let faint = Stroke::new(1.0, ink.gamma_multiply(0.45));
        let hit = self.pick(rect, brush);
        // The brush's size on the clay, in the clay's units.
        let radius =
            hit.map(|hit| self.tool.radius * camera.world_per_point(hit.point, rect.height()));
        let on_clay = hit.zip(radius).and_then(|(hit, radius)| {
            let normal = self.sculpt.normal_at(&hit);
            let full = surface_outline(camera, rect, hit.point, normal, radius)?;
            let inner = surface_outline(camera, rect, hit.point, normal, radius * half);
            Some((full, inner))
        });
        match on_clay {
            Some(((ring, tick), inner)) => {
                draw(ring, tick, line);
                if let Some((inner, _)) = inner {
                    outline(inner, faint);
                }
            }
            None => {
                let r = self.tool.radius;
                let ring = |r: f32| -> Vec<Pos2> {
                    (0..CURSOR_SEGMENTS)
                        .map(|i| {
                            let a = i as f32 / CURSOR_SEGMENTS as f32 * std::f32::consts::TAU;
                            brush + egui::vec2(a.cos(), a.sin()) * r
                        })
                        .collect()
                };
                outline(ring(r), line);
                outline(ring(r * half), faint);
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
            let grey = self.view_inks(SCULPT).mirror.gamma_multiply(0.6);
            draw(ring, tick, Stroke::new(1.0, grey));
        }
        painter.circle_filled(centre, 1.5, ink);
        // A tool held by a key, not the one chosen: its icon by the brush.
        if held != self.tool.tool {
            let icon = Rect::from_min_size(centre + egui::vec2(8.0, -22.0), egui::vec2(14.0, 14.0));
            brush_icon(held)(&painter, icon, ink);
        }
        if let Some(at) = self.tool.readout {
            let age = self.now - at;
            if age < READOUT {
                let tr = self.lang.tr();
                let strength = self.tool.strength[tool_index(self.tool.tool)];
                let text = format!(
                    "{} {:.0}   {} {:.0}%",
                    tr.size,
                    self.tool.radius,
                    tr.strength,
                    strength * 100.0
                );
                let font = egui::TextStyle::Small.resolve(ui.style());
                painter.text(
                    centre + egui::vec2(10.0, 10.0),
                    egui::Align2::LEFT_TOP,
                    text,
                    font,
                    self.view_inks(SCULPT).ink,
                );
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(READOUT - age));
            }
        }
    }
}

/// How far out, as a share of the radius, a brush with `falloff` works at
/// half strength.
fn half_weight(falloff: Falloff) -> f32 {
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..20 {
        let mid = (lo + hi) / 2.0;
        if falloff.weight(mid, 1.0) > 0.5 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
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
    fn the_inner_ring_is_where_the_brush_works_at_half_strength() {
        for falloff in (0..=MAX_HARDNESS).step_by(10).map(Falloff::Hard) {
            let t = half_weight(falloff);
            assert!((falloff.weight(t, 1.0) - 0.5).abs() < 1e-3, "{falloff:?}");
        }
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
    fn ctrl_masks_with_any_tool_and_ctrl_on_the_background_inverts_or_clears() {
        use super::super::study::tests::Harness;
        use crate::input::Modifiers;
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        h.frames(1);
        let held = |h: &mut Harness, ctrl: bool, alt: bool| {
            h.app.push_event(crate::InputEvent::Modifiers(Modifiers {
                ctrl: ctrl && !cfg!(target_os = "macos"),
                command: ctrl,
                alt,
                ..Default::default()
            }));
        };
        let masked = |h: &Harness| {
            let mask = &h.app.sculpt.mesh.layers.mask;
            mask.values().iter().filter(|&&m| m > 0.0).count()
        };
        let rect = h.app.view_rects[SCULPT];
        let tool = h.app.tool.tool;
        assert_ne!(tool, Brush::Mask);
        let before = h.app.sculpt.mesh.positions.clone();

        // Ctrl + a stroke on the clay paints the mask and moves nothing.
        held(&mut h, true, false);
        h.drag(rect.center(), rect.center() + egui::vec2(60.0, 0.0));
        let painted = masked(&h);
        assert!(painted > 0);
        assert_eq!(h.app.sculpt.mesh.positions, before);
        assert_eq!(h.app.tool.tool, tool, "the tool in hand stays");
        // Ctrl + Alt takes it away.
        held(&mut h, true, true);
        h.drag(rect.center(), rect.center() + egui::vec2(60.0, 0.0));
        assert!(masked(&h) < painted);

        // Ctrl + a tap on the background inverts it, without turning.
        held(&mut h, true, false);
        let yaw = h.app.cameras[SCULPT].yaw;
        let corner = rect.left_bottom() + egui::vec2(60.0, -60.0);
        assert!(h.app.pick(rect, corner).is_none(), "off the clay");
        h.drag(corner, corner);
        let n = h.app.sculpt.mesh.positions.len();
        assert!(masked(&h) > n / 2);
        assert_eq!(h.app.cameras[SCULPT].yaw, yaw);
        // Ctrl + a drag on the background clears it.
        h.drag(corner, corner + egui::vec2(80.0, -40.0));
        assert!(h.app.sculpt.mesh.layers.mask.is_empty());
        assert_eq!(h.app.cameras[SCULPT].yaw, yaw);

        // Without Ctrl, the tool sculpts as before.
        held(&mut h, false, false);
        h.drag(rect.center(), rect.center() + egui::vec2(60.0, 0.0));
        assert_ne!(h.app.sculpt.mesh.positions, before);
    }

    #[test]
    fn ctrl_shift_blurs_the_mask_in_zbrush_style_only() {
        use super::super::study::tests::Harness;
        use crate::input::Modifiers;
        use crate::navigation::NavStyle;
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        h.frames(1);
        let held = |h: &mut Harness, ctrl: bool, shift: bool| {
            h.app.push_event(crate::InputEvent::Modifiers(Modifiers {
                ctrl: ctrl && !cfg!(target_os = "macos"),
                command: ctrl,
                shift,
                ..Default::default()
            }));
            h.frames(1);
        };
        let rect = h.app.view_rects[SCULPT];
        let tool = h.app.tool.tool;
        let before = h.app.sculpt.mesh.positions.clone();
        // A mask with a hard edge, painted with Ctrl.
        held(&mut h, true, false);
        h.drag(rect.center(), rect.center() + egui::vec2(60.0, 0.0));
        let painted = h.app.sculpt.mesh.layers.clone();
        assert!(!painted.mask.is_empty());

        // Ctrl + Shift: the blur, its icon by the brush, the tool in hand
        // kept.
        held(&mut h, true, true);
        assert_eq!(h.app.held_tool(), Brush::BlurMask);
        h.drag(
            rect.center() + egui::vec2(0.0, 20.0),
            rect.center() + egui::vec2(60.0, 20.0),
        );
        let blurred = h.app.sculpt.mesh.layers.clone();
        assert_ne!(blurred, painted, "the mask didn't change");
        assert_eq!(h.app.sculpt.mesh.positions, before, "the clay moved");
        assert_eq!(h.app.tool.tool, tool);
        let blurs = h.app.sculpt.log().iter().filter(
            |e| matches!(e.event, crate::sculpt::Event::Dab(d) if d.brush == Brush::BlurMask),
        );
        assert!(blurs.count() > 0);
        // One undo step.
        h.app.sculpt.undo();
        assert_eq!(h.app.sculpt.mesh.layers, painted);

        // Blender and Maya styles have no mask key: Shift still smooths.
        for style in [NavStyle::Blender, NavStyle::Maya] {
            h.app.nav = style;
            held(&mut h, true, true);
            assert_eq!(h.app.held_tool(), Brush::Smooth, "{style:?}");
        }
    }

    #[test]
    fn the_mask_keys_pick_the_tool_invert_and_clear_and_the_menu_adds_chalk() {
        use super::super::study::tests::Harness;
        use crate::input::Modifiers;
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        h.frames(1);
        h.key(Key::A, false);
        assert_eq!(h.app.tool.tool, Brush::Mask);
        let command = Modifiers {
            command: true,
            ctrl: !cfg!(target_os = "macos"),
            ..Default::default()
        };
        h.chord(Key::I, command);
        let n = h.app.sculpt.mesh.positions.len();
        assert_eq!(h.app.sculpt.mesh.layers.mask.values().len(), n);
        assert!(
            h.app
                .sculpt
                .mesh
                .layers
                .mask
                .values()
                .iter()
                .all(|&m| m == 1.0)
        );
        assert_eq!(h.app.tool.tool, Brush::Mask, "Ctrl+I isn't inflate");
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        h.chord(Key::M, alt);
        assert!(h.app.sculpt.mesh.layers.mask.is_empty());
        assert_eq!(h.app.tool.tool, Brush::Mask, "Alt+M isn't move");
        // Undo brings the mask back, as one step each.
        h.chord(Key::Z, command);
        assert!(!h.app.sculpt.mesh.layers.mask.is_empty());
        assert!(h.app.sculpt.edit_layer(LayerOp::AddChalk));
        h.frames(1);
        assert_eq!(h.app.sculpt.mesh.vertex(0).layers[..2], [255, 255]);
    }

    #[test]
    fn undo_waits_for_the_stroke_under_way_to_end() {
        // A level up, a stroke, Ctrl+Z with the pen still down, let go,
        // Ctrl+Z again: the first undo used to take the level back under
        // the open stroke, which then closed as a step of the finer
        // level's vertices, and the second undo indexed past the end of
        // the coarser clay.
        use super::super::study::tests::Harness;
        use crate::PointerPhase::{Down, Move, Up};
        use crate::input::Modifiers;
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let command = Modifiers {
            command: true,
            ctrl: !cfg!(target_os = "macos"),
            ..Default::default()
        };
        let (top, count) = h.app.sculpt.levels().unwrap();
        h.key(Key::D, true);
        h.frames(2);
        assert_eq!(h.app.sculpt.levels(), Some((top - 1, count)));
        h.key(Key::D, false);
        h.frames(2);
        assert_eq!(h.app.sculpt.levels(), Some((top, count)));
        let before = h.app.sculpt.mesh.positions.clone();

        let rect = h.app.view_rects[SCULPT];
        let from = rect.center() - egui::vec2(60.0, 0.0);
        h.pointer(Move, from);
        h.pointer(Down, from);
        for k in 1..=4 {
            h.pointer(Move, from + egui::vec2(30.0 * k as f32, 0.0));
        }
        assert!(h.app.sculpt_drag_open());
        h.chord(Key::Z, command);
        assert_eq!(
            h.app.sculpt.levels(),
            Some((top, count)),
            "undone mid-stroke"
        );
        h.pointer(Up, from + egui::vec2(120.0, 0.0));
        assert!(!h.app.sculpt_drag_open());
        assert_ne!(
            h.app.sculpt.mesh.positions, before,
            "the stroke built nothing"
        );

        // Once the pen is up, undo takes the stroke back, then the level.
        h.chord(Key::Z, command);
        assert_eq!(h.app.sculpt.mesh.positions, before);
        assert_eq!(h.app.sculpt.levels(), Some((top, count)));
        h.chord(Key::Z, command);
        assert_eq!(h.app.sculpt.levels(), Some((top - 1, count)));
        h.chord(Key::Z, command);
        h.frames(1);
    }

    #[test]
    fn symmetry_starts_off() {
        let tool = ToolSettings::default();
        assert!(!tool.symmetry);
        assert_eq!(tool.tool, Brush::Clay);
    }

    /// A clay stroke along X over the top of a sphere, dabs spaced for
    /// `falloff` and as strong as `dab_at` makes them: how much it builds
    /// along its ridge, and the worst ripple there, both in world units.
    fn ridge(falloff: Falloff, spacing: f32) -> (f32, f32) {
        let mut s = Sculpt::new(Mesh::icosphere(7));
        s.set_falloff(falloff);
        let (r, step) = (0.15, 0.002);
        let height = |s: &Sculpt| -> Vec<f32> {
            (0..=200)
                .map(|i| {
                    let x = -0.2 + i as f32 * step;
                    s.raycast(Vec3::new(x, 0.0, 3.0), -Vec3::Z).unwrap().point.z
                })
                .collect()
        };
        let before = height(&s);
        s.begin_stroke();
        let mut x = -0.35;
        while x <= 0.35 {
            let hit = s.raycast(Vec3::new(x, 0.0, 3.0), -Vec3::Z).unwrap();
            s.dab(Dab {
                brush: Brush::Clay,
                centre: hit.point,
                radius: r,
                strength: 0.5 * spacing / SPACING,
                invert: false,
                symmetry: false,
                direction: Vec3::X,
                buildup: Buildup::Accumulate,
                fill: true,
            });
            s.refresh();
            x += spacing * r;
        }
        s.end_stroke();
        let rise: Vec<f32> = height(&s).iter().zip(&before).map(|(a, b)| a - b).collect();
        let mean = rise.iter().sum::<f32>() / rise.len() as f32;
        // Each sample against the average of its neighbours half a gap away.
        let k = (spacing * r / step / 2.0).round().max(1.0) as usize;
        let ripple = (k..rise.len() - k)
            .map(|i| (rise[i] - (rise[i - k] + rise[i + k]) / 2.0).abs())
            .fold(0.0, f32::max);
        (mean, ripple)
    }

    #[test]
    fn a_hard_edge_lays_a_smooth_ridge_as_high_as_a_soft_spacing_would() {
        let hardest = Falloff::Hard(MAX_HARDNESS);
        let (stepped, steps) = ridge(hardest, SPACING);
        let (smooth, ripple) = ridge(hardest, spacing(hardest));
        // At the usual spacing each dab leaves a step; closer, none shows,
        // and the stroke builds as much.
        assert!(steps > 0.02 * stepped, "{steps} on {stepped}");
        assert!(ripple < 0.005 * smooth, "{ripple} on {smooth}");
        assert!(
            (smooth - stepped).abs() < 0.05 * stepped,
            "{smooth} vs {stepped}"
        );
        // Softer edges keep the usual spacing.
        for h in [0, 60, 90] {
            assert_eq!(spacing(Falloff::Hard(h)), SPACING, "{h}");
        }
    }

    #[test]
    fn clay_starts_hard_filling_and_accumulating() {
        let tool = ToolSettings::default();
        assert_eq!(tool.falloff(Brush::Clay), Falloff::Hard(MAX_HARDNESS));
        assert_eq!(tool.buildup, Buildup::Accumulate);
        assert!(tool.fill);
        assert_eq!(tool.falloff(Brush::Smooth), Falloff::Hard(0));
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
    #[test]
    fn a_fast_stroke_leaves_the_far_side_of_the_clay_alone() {
        use super::super::study::tests::Harness;
        use crate::{InputEvent, PointerEvent, PointerKind, PointerPhase};
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let rect = h.app.view_rects[SCULPT];
        let camera = h.app.cameras[SCULPT];
        let towards_eye = camera.eye() - camera.target;
        let before = h.app.sculpt.mesh.positions.clone();
        // A quick pen stroke: many samples in one frame, each dab lifting
        // the clay where the next one looks for it.
        let event = |phase, x: f32| {
            InputEvent::Pointer(PointerEvent {
                id: 1,
                kind: PointerKind::Pen,
                phase,
                pos: [x, rect.center().y],
                button: matches!(phase, PointerPhase::Down | PointerPhase::Up)
                    .then_some(crate::Button::Primary),
                pressure: if phase == PointerPhase::Up { 0.0 } else { 1.0 },
                eraser: false,
            })
        };
        let x0 = rect.center().x - 80.0;
        h.app.push_event(event(PointerPhase::Move, x0));
        h.frames(1);
        h.app.push_event(event(PointerPhase::Down, x0));
        h.frames(1);
        for i in 1..=40 {
            h.app
                .push_event(event(PointerPhase::Move, x0 + i as f32 * 4.0));
        }
        h.frames(1);
        h.app.push_event(event(PointerPhase::Up, x0 + 160.0));
        h.frames(1);
        let after = &h.app.sculpt.mesh.positions;
        let moved = |far: bool| {
            before
                .iter()
                .zip(after)
                .filter(|(a, b)| a != b && ((**a - camera.target).dot(towards_eye) < 0.0) == far)
                .count()
        };
        assert!(moved(false) > 100, "the stroke built nothing");
        assert_eq!(moved(true), 0, "clay moved on the far side");
    }

    /// A pen event, as the desktop shell sends them.
    fn pen(phase: crate::PointerPhase, pos: Pos2, pressure: f32) -> crate::PointerEvent {
        use crate::PointerPhase::{Down, Up};
        crate::PointerEvent {
            id: 1 << 32 | 1,
            kind: PointerKind::Pen,
            phase,
            pos: [pos.x, pos.y],
            button: matches!(phase, Down | Up).then_some(crate::Button::Primary),
            pressure,
            eraser: false,
        }
    }

    /// A mouse event: `pressure` is 1.0 while its left button is down.
    fn mouse(phase: crate::PointerPhase, pos: Pos2, pressure: f32) -> crate::PointerEvent {
        crate::PointerEvent {
            id: 0,
            kind: PointerKind::Mouse,
            ..pen(phase, pos, pressure)
        }
    }

    /// The dabs that `frames` of pointer events lay, each frame's events
    /// pushed and then the frame drawn. `frames` gets the sculpt view.
    fn dabs_of(frames: impl FnOnce(Rect) -> Vec<Vec<crate::PointerEvent>>) -> Option<Vec<Dab>> {
        use super::super::study::tests::Harness;
        use crate::sculpt::Event;
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return None;
        };
        let start = h.app.sculpt.log().len();
        for events in frames(h.app.view_rects[SCULPT]) {
            for event in events {
                h.app.push_event(crate::InputEvent::Pointer(event));
            }
            h.frames(1);
        }
        let dabs: Vec<Dab> = h.app.sculpt.log()[start..]
            .iter()
            .filter_map(|e| match e.event {
                Event::Dab(dab) => Some(dab),
                _ => None,
            })
            .collect();
        assert!(!dabs.is_empty(), "no dabs");
        Some(dabs)
    }

    /// A pen stroke across the clay that presses hardest (0.5) as it
    /// starts and fades towards 0.1 as it lifts, a sample a frame: its
    /// frames, and where it lifts.
    fn fading_stroke(rect: Rect) -> (Vec<Vec<crate::PointerEvent>>, Pos2) {
        use crate::PointerPhase::{Down, Move, Up};
        let at = |i: usize| rect.center() + egui::vec2(-60.0 + i as f32 * 6.0, 0.0);
        let mut frames = vec![vec![pen(Move, at(0), 0.0)], vec![pen(Down, at(0), 0.5)]];
        for i in 1..=20 {
            frames.push(vec![pen(Move, at(i), 0.5 - 0.4 * i as f32 / 20.0)]);
        }
        frames.push(vec![pen(Up, at(20), 0.1)]);
        (frames, at(20))
    }

    /// No dab of a stroke whose pen pressed hardest at its first dab is
    /// stronger or larger than that first dab.
    fn assert_none_above_the_first(dabs: &[Dab]) {
        let first = dabs[0];
        for (i, dab) in dabs.iter().enumerate() {
            assert!(
                dab.strength <= first.strength * 1.001,
                "dab {i} of {}: strength {} above the pen's hardest, {}",
                dabs.len(),
                dab.strength,
                first.strength
            );
            // The size also goes by the depth of the clay under the dab.
            assert!(
                dab.radius <= first.radius * 1.05,
                "dab {i} of {}: radius {} above the pen's largest, {}",
                dabs.len(),
                dab.radius,
                first.radius
            );
        }
    }

    #[test]
    fn a_pen_that_bounces_as_it_lifts_lays_no_blob() {
        use crate::PointerPhase::{Down, Move, Up};
        // The pen lifts, touches again lightly for an instant (within one
        // frame), and the system adds a mouse move where it lifted: that
        // touch's one dab takes the pen's pressure, not the mouse's.
        let Some(dabs) = dabs_of(|rect| {
            let (mut frames, end) = fading_stroke(rect);
            frames.push(vec![
                pen(Down, end, 0.1),
                pen(Up, end, 0.1),
                mouse(Move, end, 0.0),
            ]);
            frames
        }) else {
            return;
        };
        assert_none_above_the_first(&dabs);
        let last = dabs.last().unwrap();
        assert!(
            (last.strength / dabs[0].strength - 0.2).abs() < 0.01,
            "the touch's dab took pressure {} rather than the pen's 0.1",
            last.strength / dabs[0].strength * 0.5
        );
    }

    #[test]
    fn a_mouse_event_amid_a_pen_stroke_is_not_part_of_it() {
        use crate::PointerPhase::Move;
        // A mouse sample with its button down (full pressure) among the
        // pen's, near the lift.
        let Some(dabs) = dabs_of(|rect| {
            let (mut frames, end) = fading_stroke(rect);
            let at = frames.len() - 3;
            frames[at].push(mouse(Move, end, 1.0));
            frames
        }) else {
            return;
        };
        assert_none_above_the_first(&dabs);
    }

    #[test]
    fn a_pen_sample_after_its_lift_is_not_part_of_the_stroke() {
        use crate::PointerPhase::{Down, Move, Up};
        // SDL sends a lift first, then the lifting message's position and
        // pressure; here the pen touches down again within the frame, so
        // the stroke goes on, but without the sample taken in the air.
        let Some(dabs) = dabs_of(|rect| {
            let (mut frames, end) = fading_stroke(rect);
            let up = frames.pop().unwrap();
            assert_eq!(up[0].phase, Up);
            let near = end + egui::vec2(6.0, 0.0);
            frames.push(vec![
                pen(Up, end, 0.1),
                pen(Move, near, 0.9),
                pen(Down, near, 0.1),
            ]);
            for i in 1..=5 {
                frames.push(vec![pen(Move, near + egui::vec2(i as f32 * 6.0, 0.0), 0.1)]);
            }
            frames.push(vec![pen(Up, near + egui::vec2(30.0, 0.0), 0.1)]);
            frames
        }) else {
            return;
        };
        assert_none_above_the_first(&dabs);
    }

    #[test]
    fn an_eraser_stroke_stays_inverted_through_a_stray_mouse_event() {
        use crate::PointerPhase::Move;
        let Some(dabs) = dabs_of(|rect| {
            let (mut frames, end) = fading_stroke(rect);
            for event in frames.iter_mut().flatten() {
                event.eraser = true;
            }
            let at = frames.len() - 3;
            frames[at].push(mouse(Move, end, 0.0));
            frames
        }) else {
            return;
        };
        assert!(dabs.len() > 10);
        assert!(
            dabs.iter().all(|d| d.invert),
            "a dab of the eraser stroke built up"
        );
    }

    #[test]
    fn mouse_and_touch_strokes_press_fully() {
        use crate::PointerPhase::{Down, Move, Up};
        // Browsers report 0.5 for a touch.
        let touch = |phase, pos: Pos2| crate::PointerEvent {
            id: 7,
            kind: PointerKind::Touch,
            ..pen(phase, pos, if phase == Up { 0.0 } else { 0.5 })
        };
        let Some(dabs) = dabs_of(|rect| {
            let at = |i: usize, y: f32| rect.center() + egui::vec2(-60.0 + i as f32 * 6.0, y);
            let mut frames = vec![vec![mouse(Move, at(0, -40.0), 0.0)]];
            frames.push(vec![mouse(Down, at(0, -40.0), 1.0)]);
            frames.extend((1..=10).map(|i| vec![mouse(Move, at(i, -40.0), 1.0)]));
            frames.push(vec![mouse(Up, at(10, -40.0), 0.0)]);
            frames.push(vec![touch(Down, at(0, 40.0))]);
            frames.extend((1..=10).map(|i| vec![touch(Move, at(i, 40.0))]));
            frames.push(vec![touch(Up, at(10, 40.0))]);
            frames
        }) else {
            return;
        };
        assert!(dabs.len() > 20);
        let full = dabs[0].strength;
        assert!(full > 0.0);
        assert!(
            dabs.iter().all(|d| d.strength == full),
            "a mouse or touch dab pressed less than fully"
        );
    }
}
