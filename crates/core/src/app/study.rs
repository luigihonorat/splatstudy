// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Study aids for comparing the sculpt with the reference: camera link,
//! silhouette, ghost, mirror view, plumb line, grid and the pose timer.

use egui::{CursorIcon, Key, Rect, Sense, Stroke, pos2};

use super::views::shown;
use super::{App, SCULPT};
use crate::render::clay::Look;
use crate::ui::{menu, theme};

/// Which views show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Layout {
    #[default]
    Both,
    Sculpt,
    Reference,
}

/// The grid's choices of divisions, across the view's height.
pub(super) const GRID_DIVISIONS: [u32; 5] = [2, 3, 4, 6, 8];

/// Which aids are on.
pub(super) struct StudyState {
    pub(super) layout: Layout,
    /// The reference on the left and the sculpt on the right.
    pub(super) swapped: bool,
    /// Navigating either view turns the other the same way.
    pub(super) link: bool,
    /// Both views shown flipped left to right, to see the form afresh.
    pub(super) mirror: bool,
    pub(super) plumb: bool,
    /// Where the plumb line hangs in each view, as a share of its width
    /// (indexed by `SCULPT` and `REFERENCE`).
    pub(super) plumb_x: [f32; 2],
    pub(super) grid: bool,
    /// Rows of the grid; its cells are square.
    pub(super) grid_divisions: u32,
    /// The clay, and the reference, filled flat: only the outline counts.
    pub(super) silhouette: bool,
    /// The reference laid over the sculpt view, see-through.
    pub(super) ghost: bool,
}

impl Default for StudyState {
    fn default() -> Self {
        Self {
            layout: Layout::Both,
            swapped: false,
            link: false,
            mirror: false,
            plumb: false,
            plumb_x: [0.5; 2],
            grid: false,
            grid_divisions: 4,
            silhouette: false,
            ghost: false,
        }
    }
}

/// How the reference part should draw the reference into a pass: the
/// study aids decide, the reference part knows how (splats, mesh).
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(dead_code)] // read by the reference part's drawing, once it lands
pub(super) struct ReferenceLook {
    /// Filled flat with `theme::SILHOUETTE_INK`: no light, no own colours.
    pub(super) flat: bool,
    /// 0..1.
    pub(super) opacity: f32,
    /// Laid over everything the pass has drawn as one see-through layer,
    /// rather than hidden by what is in front of it: the ghost. Meshes do
    /// this with `ClayPass::prepare_ghost` and `draw_ghost`; splats skip
    /// the depth test.
    pub(super) over: bool,
}

impl App {
    /// The clay's look in the sculpt view.
    pub(super) fn sculpt_look(&self) -> Look {
        if self.study.silhouette {
            Look::flat(theme::SILHOUETTE_INK)
        } else {
            Look::lit(theme::CLAY)
        }
    }

    /// The reference's look in its own view.
    #[allow(dead_code)] // called by the reference part's drawing, once it lands
    pub(super) fn reference_look(&self) -> ReferenceLook {
        ReferenceLook {
            flat: self.study.silhouette,
            opacity: 1.0,
            over: false,
        }
    }

    /// The reference's look as a ghost over the sculpt view, when that is on.
    pub(super) fn ghost_look(&self) -> Option<ReferenceLook> {
        self.study.ghost.then_some(ReferenceLook {
            flat: self.study.silhouette,
            opacity: theme::GHOST_OPACITY,
            over: true,
        })
    }

    /// Lays the reference over the sculpt view, seen with the sculpt's
    /// camera, when the ghost is on. Called last in the sculpt view's pass.
    pub(super) fn draw_ghost(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some(look) = self.ghost_look() else {
            return;
        };
        // The reference part's drawing plugs in here, once it lands:
        // self.draw_reference_as(pass, Slot::SculptView, &self.cameras[SCULPT],
        //     self.views[SCULPT].aspect(), &look);
        let _ = (pass, look);
    }

    /// The camera link, at the top of the view menu.
    pub(super) fn link_item(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        if menu::item(ui, tr.link_cameras, "L", self.study.link).clicked() {
            self.toggle_link();
        }
    }

    /// Linking starts from the sculpt's view, the one being worked on.
    fn toggle_link(&mut self) {
        self.study.link = !self.study.link;
        self.follow_camera(SCULPT);
    }

    /// The comparison aids in the view menu.
    pub(super) fn study_toggle_items(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        let st = tr.study;
        menu::submenu(ui, st.views, |ui| {
            ui.set_min_width(140.0);
            for (layout, label) in [
                (Layout::Both, st.both),
                (Layout::Sculpt, tr.sculpt),
                (Layout::Reference, tr.reference),
            ] {
                if menu::item(ui, label, "", self.study.layout == layout).clicked() {
                    self.study.layout = layout;
                }
            }
        });
        if menu::item(ui, st.swap_sides, "W", self.study.swapped).clicked() {
            self.study.swapped = !self.study.swapped;
        }
        if menu::item(ui, tr.mirror_view, "R", self.study.mirror).clicked() {
            self.study.mirror = !self.study.mirror;
        }
        if menu::item(ui, tr.silhouette, "K", self.study.silhouette).clicked() {
            self.study.silhouette = !self.study.silhouette;
        }
        if menu::item(ui, tr.ghost, "O", self.study.ghost).clicked() {
            self.study.ghost = !self.study.ghost;
        }
        if menu::item(ui, tr.plumb_line, "V", self.study.plumb).clicked() {
            self.study.plumb = !self.study.plumb;
        }
        if menu::item(ui, tr.grid, "G", self.study.grid).clicked() {
            self.study.grid = !self.study.grid;
        }
        menu::submenu(ui, st.grid_divisions, |ui| {
            ui.set_min_width(80.0);
            for n in GRID_DIVISIONS {
                let chosen = self.study.grid && self.study.grid_divisions == n;
                if menu::item(ui, &n.to_string(), "", chosen).clicked() {
                    self.study.grid_divisions = n;
                    self.study.grid = true;
                }
            }
        });
    }

    /// The plumb line and the grid over each shown view. They are drawn in
    /// screen space, so the mirror view does not flip them, and the grid's
    /// square cells match from view to view whatever their widths.
    pub(super) fn guides(&mut self, ui: &egui::Ui, rects: [Rect; 2]) {
        for (index, rect) in rects.into_iter().enumerate() {
            if !shown(rect) {
                continue;
            }
            let painter = ui.painter().with_clip_rect(rect);
            if self.study.grid {
                paint_grid(&painter, rect, self.study.grid_divisions);
            }
            if !self.study.plumb {
                continue;
            }
            // Dragged sideways by a band around the line.
            let at = |share: f32| (rect.left() + share * rect.width()).round();
            let x = at(self.study.plumb_x[index]);
            let band = Rect::from_x_y_ranges(x - 5.0..=x + 5.0, rect.y_range());
            let id = ui.scope_id().with(("plumb", index));
            let response = ui.interact(band, id, Sense::click_and_drag());
            if response.dragged()
                && let Some(pos) = response.interact_pointer_pos()
            {
                let share = (pos.x - rect.left()) / rect.width();
                self.study.plumb_x[index] = share.clamp(0.0, 1.0);
            }
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            let x = at(self.study.plumb_x[index]) + 0.5;
            painter.vline(x, rect.y_range(), Stroke::new(1.0, theme::GUIDE));
            // The bob, low on the line.
            painter.circle_filled(pos2(x, rect.bottom() - 32.0), 3.5, theme::GUIDE);
        }
    }

    pub(super) fn study_shortcuts(&mut self, ui: &egui::Ui) {
        use egui::Modifiers as M;
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let pressed = |key: Key| ui.input_mut(|i| i.consume_key(M::NONE, key));
        if pressed(Key::W) {
            self.study.swapped = !self.study.swapped;
        }
        if pressed(Key::L) {
            self.toggle_link();
        }
        if pressed(Key::R) {
            self.study.mirror = !self.study.mirror;
        }
        if pressed(Key::K) {
            self.study.silhouette = !self.study.silhouette;
        }
        if pressed(Key::O) {
            self.study.ghost = !self.study.ghost;
        }
        if pressed(Key::V) {
            self.study.plumb = !self.study.plumb;
        }
        if pressed(Key::G) {
            self.study.grid = !self.study.grid;
        }
    }
}

/// `divisions` rows of square cells, with the columns laid out from the
/// view's centre so a line or a cell's middle falls on it, as on the rows.
fn paint_grid(painter: &egui::Painter, rect: Rect, divisions: u32) {
    let stroke = Stroke::new(1.0, theme::GUIDE.gamma_multiply(0.45));
    let n = divisions.max(1);
    let cell = rect.height() / n as f32;
    for k in 1..n {
        let y = (rect.top() + k as f32 * cell).round() + 0.5;
        painter.hline(rect.x_range(), y, stroke);
    }
    let offset = if n % 2 == 0 { 0.0 } else { 0.5 };
    let reach = (rect.width() / 2.0 / cell).ceil() as i32 + 1;
    for k in -reach..=reach {
        let x = rect.center().x + (k as f32 + offset) * cell;
        if x > rect.left() + 0.5 && x < rect.right() - 0.5 {
            painter.vline(x.round() + 0.5, rect.y_range(), stroke);
        }
    }
}
