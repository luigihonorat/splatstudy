// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Study aids for comparing the sculpt with the reference: camera link,
//! silhouette, ghost, mirror view, plumb line, grid and the pose timer.

use egui::{Align2, CursorIcon, Key, Rect, Sense, Stroke, pos2};

use super::views::shown;
use super::{App, REFERENCE, SCULPT};
use crate::render::Slot;
use crate::render::clay::Look;
use crate::ui::{fonts, menu, theme};

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
    pub(super) timer: PoseTimer,
}

/// Round lengths offered, in minutes, and numbers of rounds.
pub(super) const TIMER_MINUTES: [u32; 6] = [1, 2, 5, 10, 20, 30];
pub(super) const TIMER_ROUNDS: [u32; 4] = [1, 3, 5, 10];
/// How long the views flash when a round ends, in seconds.
const FLASH_SECONDS: f64 = 1.2;

/// Where the timer's clock is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Clock {
    Off,
    /// The current round started at `start`, moved later by any pauses.
    Running {
        start: f64,
    },
    Paused {
        elapsed: f64,
    },
}

/// Timed rounds, as in a life-drawing class: a few minutes per pose, or
/// per side of the cast. It runs from the frame clock, in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PoseTimer {
    pub(super) minutes: u32,
    pub(super) rounds: u32,
    pub(super) clock: Clock,
    /// The round under way, from 0.
    pub(super) round: u32,
    /// When the last round ended, for the flash.
    pub(super) ended_at: Option<f64>,
}

impl Default for PoseTimer {
    fn default() -> Self {
        Self {
            minutes: 5,
            rounds: 1,
            clock: Clock::Off,
            round: 0,
            ended_at: None,
        }
    }
}

impl PoseTimer {
    fn length(&self) -> f64 {
        f64::from(self.minutes.max(1)) * 60.0
    }

    pub(super) fn start(&mut self, now: f64) {
        self.round = 0;
        self.clock = Clock::Running { start: now };
    }

    pub(super) fn stop(&mut self) {
        self.clock = Clock::Off;
    }

    /// Pauses a running clock, resumes a paused one, starts a stopped one.
    pub(super) fn toggle(&mut self, now: f64) {
        self.clock = match self.clock {
            Clock::Running { start } => Clock::Paused {
                elapsed: now - start,
            },
            Clock::Paused { elapsed } => Clock::Running {
                start: now - elapsed,
            },
            Clock::Off => {
                self.start(now);
                return;
            }
        };
    }

    /// Seconds left in the current round.
    pub(super) fn remaining(&self, now: f64) -> f64 {
        let elapsed = match self.clock {
            Clock::Off => 0.0,
            Clock::Running { start } => now - start,
            Clock::Paused { elapsed } => elapsed,
        };
        (self.length() - elapsed).max(0.0)
    }

    /// Moves on when the round is over. Returns `Some(true)` when the last
    /// round just ended, `Some(false)` for any other round, `None` if none
    /// ended. Rounds missed while the app slept all count.
    pub(super) fn tick(&mut self, now: f64) -> Option<bool> {
        let Clock::Running { start } = self.clock else {
            return None;
        };
        let length = self.length();
        let ended = ((now - start) / length).floor();
        if ended < 1.0 {
            return None;
        }
        self.ended_at = Some(now);
        self.round += ended as u32;
        if self.round >= self.rounds {
            self.round = self.rounds.saturating_sub(1);
            self.clock = Clock::Off;
            return Some(true);
        }
        self.clock = Clock::Running {
            start: start + ended * length,
        };
        Some(false)
    }

    /// How strong the end-of-round flash is now, 1 fading to 0.
    pub(super) fn flash(&self, now: f64) -> f32 {
        self.ended_at.map_or(0.0, |at| {
            (1.0 - (now - at) / FLASH_SECONDS).clamp(0.0, 1.0) as f32
        })
    }
}

/// Seconds as the clock shows them, rounded up: `m:ss`.
fn clock_text(seconds: f64) -> String {
    let whole = seconds.ceil() as u64;
    format!("{}:{:02}", whole / 60, whole % 60)
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
            timer: PoseTimer::default(),
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
        self.draw_reference_as(
            pass,
            Slot::SculptView,
            &self.cameras[SCULPT],
            self.views[SCULPT].aspect(),
            &look,
        );
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
        ui.separator();
        if menu::item(ui, tr.silhouette, "K", self.study.silhouette).clicked() {
            self.study.silhouette = !self.study.silhouette;
        }
        if menu::item(ui, tr.ghost, "O", self.study.ghost).clicked() {
            self.study.ghost = !self.study.ghost;
        }
        ui.separator();
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
        ui.separator();
        menu::submenu(ui, st.pose_timer, |ui| {
            ui.set_min_width(140.0);
            let now = self.now;
            let timer = &mut self.study.timer;
            let toggle = match timer.clock {
                Clock::Off => st.start,
                Clock::Running { .. } => tr.pause,
                Clock::Paused { .. } => st.resume,
            };
            if menu::action(ui, true, toggle, "") {
                timer.toggle(now);
            }
            if menu::action(ui, timer.clock != Clock::Off, st.stop, "") {
                timer.stop();
            }
            ui.separator();
            for n in TIMER_MINUTES {
                if menu::item(ui, &(st.minutes)(n), "", timer.minutes == n).clicked() {
                    timer.minutes = n;
                }
            }
            ui.separator();
            for n in TIMER_ROUNDS {
                if menu::item(ui, &(st.rounds)(n), "", timer.rounds == n).clicked() {
                    timer.rounds = n;
                }
            }
        });
    }

    /// The pose timer over the views: the clock in the top right corner of
    /// the sculpt view (click to pause), and a soft flash over both views
    /// when a round ends.
    pub(super) fn timer_ui(&mut self, ui: &egui::Ui, area: Rect, rects: [Rect; 2]) {
        let now = self.now;
        if self.study.timer.tick(now) == Some(true) {
            self.notify(self.lang.tr().study.time_up);
        }
        let flash = self.study.timer.flash(now);
        if flash > 0.0 {
            let veil = theme::VIEW_INK.gamma_multiply(0.22 * flash);
            ui.painter().rect_filled(area, 0.0, veil);
            ui.ctx().request_repaint();
        }
        let timer = &mut self.study.timer;
        if timer.clock == Clock::Off {
            return;
        }
        let rect = if shown(rects[SCULPT]) {
            rects[SCULPT]
        } else {
            rects[REFERENCE]
        };
        let remaining = timer.remaining(now);
        let painter = ui.painter();
        let clock =
            painter.layout_no_wrap(clock_text(remaining), fonts::light(26.0), theme::VIEW_INK);
        let corner = rect.right_top() + egui::vec2(-14.0, 8.0);
        let clock_rect =
            Rect::from_min_size(corner - egui::vec2(clock.size().x, 0.0), clock.size());
        let response = ui
            .interact(clock_rect, ui.scope_id().with("pose timer"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        if response.clicked() {
            timer.toggle(now);
        }
        let running = matches!(timer.clock, Clock::Running { .. });
        let colour = if running || response.hovered() {
            theme::VIEW_INK
        } else {
            theme::VIEW_TEXT
        };
        painter.galley(clock_rect.min, clock, colour);
        if timer.rounds > 1 {
            painter.text(
                egui::pos2(clock_rect.right(), clock_rect.bottom() + 2.0),
                Align2::RIGHT_TOP,
                format!("{} / {}", timer.round + 1, timer.rounds),
                fonts::regular(12.0),
                theme::VIEW_TEXT,
            );
        }
        if running {
            // Again when the shown second changes.
            let next = remaining - (remaining.ceil() - 1.0);
            let next = if next <= 0.0 { 1.0 } else { next };
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(next + 0.01));
        }
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
    let offset = if n.is_multiple_of(2) { 0.0 } else { 0.5 };
    let reach = (rect.width() / 2.0 / cell).ceil() as i32 + 1;
    for k in -reach..=reach {
        let x = rect.center().x + (k as f32 + offset) * cell;
        if x > rect.left() + 0.5 && x < rect.right() - 0.5 {
            painter.vline(x.round() + 0.5, rect.y_range(), stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timer(minutes: u32, rounds: u32) -> PoseTimer {
        PoseTimer {
            minutes,
            rounds,
            ..PoseTimer::default()
        }
    }

    #[test]
    fn rounds_follow_each_other_then_the_timer_stops() {
        let mut t = timer(1, 3);
        t.start(100.0);
        assert_eq!(t.tick(130.0), None);
        assert_eq!(t.remaining(130.0), 30.0);
        assert_eq!(t.tick(160.5), Some(false));
        assert_eq!(t.round, 1);
        // The next round counts from when the last one ended, not from
        // the frame that noticed.
        assert_eq!(t.remaining(160.5), 59.5);
        assert_eq!(t.flash(160.5), 1.0);
        assert_eq!(t.tick(220.0), Some(false));
        assert_eq!(t.tick(280.0), Some(true));
        assert_eq!(t.clock, Clock::Off);
        assert_eq!(t.round, 2, "the last round stays shown");
        assert_eq!(t.tick(400.0), None);
    }

    #[test]
    fn a_pause_keeps_the_time_left() {
        let mut t = timer(5, 1);
        t.start(0.0);
        t.toggle(100.0);
        assert_eq!(t.clock, Clock::Paused { elapsed: 100.0 });
        assert_eq!(t.tick(1000.0), None, "no round ends while paused");
        assert_eq!(t.remaining(1000.0), 200.0);
        t.toggle(1000.0);
        assert_eq!(t.remaining(1050.0), 150.0);
        assert_eq!(t.tick(1200.0), Some(true));
    }

    #[test]
    fn rounds_missed_while_asleep_all_count() {
        let mut t = timer(1, 5);
        t.start(0.0);
        assert_eq!(t.tick(150.0), Some(false));
        assert_eq!(t.round, 2);
        assert_eq!(t.remaining(150.0), 30.0);
    }

    #[test]
    fn the_flash_fades_and_the_clock_rounds_up() {
        let mut t = timer(1, 2);
        t.start(0.0);
        t.tick(60.0);
        assert!(t.flash(60.6) > 0.0 && t.flash(60.6) < 1.0);
        assert_eq!(t.flash(70.0), 0.0);
        assert_eq!(clock_text(300.0), "5:00");
        assert_eq!(clock_text(299.2), "5:00");
        assert_eq!(clock_text(59.0), "0:59");
        assert_eq!(clock_text(0.0), "0:00");
    }
}
