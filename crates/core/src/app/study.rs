// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Study aids for comparing the sculpt with the reference: camera link,
//! silhouette, ghost, mirror view, plumb line, grid and the pose timer.

use egui::{Align2, CursorIcon, Key, Pos2, Rect, Sense, Stroke, pos2};

use super::views::shown;
use super::{App, REFERENCE, SCULPT};
use crate::input::Modifiers;
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
    /// The view whose plumb line is being moved by its bob.
    pub(super) plumb_drag: Option<usize>,
    pub(super) grid: bool,
    /// Rows of the grid; its cells are square.
    pub(super) grid_divisions: u32,
    /// The clay, and the reference, filled flat: only the outline counts.
    pub(super) silhouette: bool,
    /// The reference laid over the sculpt view, see-through.
    pub(super) ghost: bool,
    pub(super) timer: PoseTimer,
}

/// The plumb line's bob: how high it hangs over the view's bottom edge,
/// and how near it a press must land to take the line, in points.
const BOB_LIFT: f32 = 32.0;
const BOB_REACH: f32 = 9.0;

/// Where the bob of a plumb line hanging at `share` of `rect`'s width is.
fn bob(rect: Rect, share: f32) -> Pos2 {
    pos2(
        (rect.left() + share * rect.width()).round() + 0.5,
        rect.bottom() - BOB_LIFT,
    )
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
            plumb_drag: None,
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

    /// A left press at `screen` in view `index`, before the view sees it:
    /// returns true if a study aid took it. The plumb line moves by its bob
    /// alone, and only where the clay is not under it, so a stroke always
    /// wins on the clay.
    pub(super) fn study_press(
        &mut self,
        rect: Rect,
        index: usize,
        screen: Pos2,
        _m: Modifiers,
    ) -> bool {
        if self.on_bob(rect, index, screen) {
            self.study.plumb_drag = Some(index);
            return true;
        }
        false
    }

    /// `screen` is on the bob of view `index`'s plumb line, and the bob is
    /// free to take it: the clay is not under it.
    fn on_bob(&self, rect: Rect, index: usize, screen: Pos2) -> bool {
        if !self.study.plumb || screen.distance(bob(rect, self.study.plumb_x[index])) > BOB_REACH {
            return false;
        }
        let sculpting = index == SCULPT && self.playback.is_none();
        !(sculpting && self.pick(rect, self.view_pos(rect, screen)).is_some())
    }

    /// The plumb line and the grid over each shown view. They are drawn in
    /// screen space, so the mirror view does not flip them, and the grid's
    /// square cells match from view to view whatever their widths.
    pub(super) fn guides(&mut self, ui: &egui::Ui, rects: [Rect; 2]) {
        let (pointer, held) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.primary_down()));
        if !held {
            self.study.plumb_drag = None;
        }
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
            let dragged = self.study.plumb_drag == Some(index);
            if dragged && let Some(pos) = pointer {
                let share = (pos.x - rect.left()) / rect.width();
                self.study.plumb_x[index] = share.clamp(0.0, 1.0);
            }
            let hovered = !dragged
                && self.drags[index].is_none()
                && pointer.is_some_and(|pos| rect.contains(pos) && self.on_bob(rect, index, pos));
            if hovered || dragged {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            let bob = bob(rect, self.study.plumb_x[index]);
            let line = Stroke::new(1.0, theme::GUIDE);
            painter.vline(bob.x, rect.y_range(), line);
            painter.circle_filled(bob, 3.5, theme::GUIDE);
            if hovered || dragged {
                painter.circle_stroke(bob, 7.0, line);
            }
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

    /// The whole app offscreen, driven by input events. `None` without a
    /// GPU adapter, and the tests that need one skip.
    struct Harness {
        app: App,
        view: wgpu::TextureView,
        time: f64,
    }

    const SIZE: [u32; 2] = [1280, 720];

    impl Harness {
        fn new() -> Option<Self> {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
            let (device, queue) =
                pollster::block_on(adapter.request_device(&Default::default())).ok()?;
            let format = wgpu::TextureFormat::Rgba8Unorm;
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("frame"),
                size: wgpu::Extent3d {
                    width: SIZE[0],
                    height: SIZE[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let app = App::new(device, queue, format, crate::Lang::En);
            let mut h = Self {
                app,
                view: target.create_view(&Default::default()),
                time: 0.0,
            };
            h.frames(2);
            Some(h)
        }

        fn frames(&mut self, n: usize) {
            for _ in 0..n {
                self.time += 1.0 / 60.0;
                self.app.frame(self.time, &self.view, SIZE, 1.0);
            }
        }

        fn pointer(&mut self, phase: crate::PointerPhase, pos: Pos2) {
            use crate::{Button, InputEvent, PointerEvent, PointerKind, PointerPhase};
            let button =
                matches!(phase, PointerPhase::Down | PointerPhase::Up).then_some(Button::Primary);
            self.app.push_event(InputEvent::Pointer(PointerEvent {
                id: 1,
                kind: PointerKind::Mouse,
                phase,
                pos: [pos.x, pos.y],
                button,
                pressure: if phase == PointerPhase::Up { 0.0 } else { 1.0 },
                eraser: false,
            }));
            self.frames(1);
        }

        /// A left drag from `from` to `to`, in a few steps.
        fn drag(&mut self, from: Pos2, to: Pos2) {
            use crate::PointerPhase::{Down, Move, Up};
            self.pointer(Move, from);
            self.pointer(Down, from);
            for k in 1..=4 {
                self.pointer(Move, from.lerp(to, k as f32 / 4.0));
            }
            self.pointer(Up, to);
        }
    }

    #[test]
    fn the_plumb_line_moves_by_its_bob_but_never_off_the_clay() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        h.app.study.plumb = true;
        h.frames(1);
        let rect = h.app.view_rects[SCULPT];
        let strokes = |h: &Harness| {
            h.app
                .sculpt
                .log()
                .iter()
                .filter(|e| matches!(e.event, crate::sculpt::record::Event::Begin))
                .count()
        };

        // Off the clay, the bob takes the line along.
        let start = bob(rect, 0.5);
        h.drag(start, start + egui::vec2(80.0, -40.0));
        let moved = h.app.study.plumb_x[SCULPT];
        let expected = (start.x + 80.0 - rect.left()) / rect.width();
        assert!((moved - expected).abs() < 0.01, "{moved} vs {expected}");
        assert_eq!(strokes(&h), 0);

        // Pressing the line itself, away from the bob, does not move it.
        let x = bob(rect, moved).x;
        h.drag(
            pos2(x, rect.top() + 60.0),
            pos2(x + 50.0, rect.top() + 60.0),
        );
        assert_eq!(h.app.study.plumb_x[SCULPT], moved);

        // Close up, the clay fills the view: the press on the bob sculpts.
        h.app.study.plumb_x[SCULPT] = 0.5;
        h.app.cameras[SCULPT].distance = 1.3;
        h.frames(1);
        h.drag(start, start + egui::vec2(80.0, 0.0));
        assert_eq!(h.app.study.plumb_x[SCULPT], 0.5);
        assert_eq!(strokes(&h), 1);
    }
}
