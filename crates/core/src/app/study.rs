// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Study aids for comparing the sculpt with the reference: camera link,
//! silhouette, ghost, mirror view, plumb line, grid, the pose timer, the
//! silhouette match score, the proportion measure and the lighting.

use egui::{Align2, Color32, CursorIcon, Key, Pos2, Rect, Sense, Stroke, Vec2, pos2};

use super::toolbar::{ViewOption, lighting_name};
use super::views::shown;
use super::{App, REFERENCE, SCULPT};
use crate::camera::OrbitCamera;
use crate::input::Modifiers;
use crate::reference::{Aabb, Alignment, Content};
use crate::render::clay::{self, Lighting, Look, Overlap};
use crate::render::readback::Readback;
use crate::render::{Slot, ViewTarget};
use crate::ui::{fonts, guides, menu, theme};

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

/// The aids each view has of its own. Its projection is its camera's.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Aids {
    /// Shown flipped left to right, to see the form afresh.
    pub(super) mirror: bool,
    pub(super) plumb: bool,
    pub(super) grid: bool,
    /// Filled flat: only the outline counts.
    pub(super) silhouette: bool,
    /// How the clay, or a mesh reference, is lit.
    pub(super) lighting: Lighting,
}

/// Which aids are on.
pub(super) struct StudyState {
    pub(super) layout: Layout,
    /// The reference on the left and the sculpt on the right.
    pub(super) swapped: bool,
    /// Navigating either view turns the other the same way.
    pub(super) link: bool,
    /// Each view's own aids (indexed by `SCULPT` and `REFERENCE`).
    pub(super) views: [Aids; 2],
    /// The view the pointer was last over: the view menu switches its
    /// aids.
    pub(super) focus: usize,
    /// Where the plumb line hangs in each view, as a share of its width
    /// (indexed by `SCULPT` and `REFERENCE`).
    pub(super) plumb_x: [f32; 2],
    /// The view whose plumb line is being moved by its bob.
    pub(super) plumb_drag: Option<usize>,
    /// Rows of the grid; its cells are square.
    pub(super) grid_divisions: u32,
    /// The reference laid over the sculpt view, see-through.
    pub(super) ghost: bool,
    pub(super) timer: PoseTimer,
    pub(super) measure: Measure,
    pub(super) matching: SilhouetteMatch,
    /// The clay's edges drawn over it: quads as quads.
    pub(super) wireframe: bool,
}

/// How near the plumb line's bob a press must land to take the line, in
/// points.
const BOB_REACH: f32 = 9.0;

/// The proportion measure, as a pencil held at arm's length: a line
/// dragged over each view, and the clay's length as a share of the
/// reference's. With the cameras linked both views show the form at the
/// same scale for their height, so lengths count in view heights.
#[derive(Default)]
pub(super) struct Measure {
    pub(super) on: bool,
    /// The line in each view (indexed by `SCULPT` and `REFERENCE`): its
    /// ends in view heights from the view's centre, in the rendered image,
    /// so the mirror view flips it with the form it measures.
    pub(super) lines: [Option<[Vec2; 2]>; 2],
    /// The view a line is being dragged in.
    pub(super) drawing: Option<usize>,
}

/// Lines shorter than this, in points, are a click: they clear the view's line.
const MEASURE_MIN: f32 = 4.0;

impl Measure {
    /// The length of view `index`'s line, in view heights.
    fn length(&self, index: usize) -> Option<f32> {
        self.lines[index].map(|[a, b]| (b - a).length())
    }

    /// The clay's measured length as a share of the reference's.
    pub(super) fn ratio(&self) -> Option<f32> {
        let reference = self.length(REFERENCE).filter(|&l| l > 1e-4)?;
        Some(self.length(SCULPT)? / reference)
    }
}

/// A point of a view in view heights from its centre, and back.
fn to_heights(rect: Rect, pos: Pos2) -> Vec2 {
    (pos - rect.center()) / rect.height().max(1.0)
}

fn from_heights(rect: Rect, at: Vec2) -> Pos2 {
    rect.center() + at * rect.height()
}

/// The silhouette match: with the cameras linked, the clay and the
/// reference are rendered flat from the same camera, small, and read back;
/// the score is how much their outlines overlap. Compared again once the
/// views have been still for a moment after a stroke or a turn, never
/// every frame.
#[derive(Default)]
pub(super) struct SilhouetteMatch {
    /// Tint the sculpt view where the clay is too big and too small.
    pub(super) tint: bool,
    /// What the views showed last frame, and since when.
    seen: Option<(MatchKey, f64)>,
    /// The two small flat renders, kept between comparisons.
    targets: Option<[ViewTarget; 2]>,
    pending: Option<PendingMatch>,
    result: Option<MatchResult>,
}

/// Everything a comparison depends on: when it changes, the score is due again.
#[derive(Clone, Debug, PartialEq)]
struct MatchKey {
    camera: OrbitCamera,
    /// The flat renders' size, which has the sculpt view's shape.
    size: [u32; 2],
    /// The clay: its log's length, its vertex count and a sample of its
    /// positions (opening a scene can bring a log as long).
    clay: (usize, usize, u64),
    /// The reference: its file's bytes (by address) and placement.
    reference: (usize, Alignment, Option<Aabb>),
}

/// Flat renders on their way back from the GPU: the clay's, then the
/// reference's. The browser maps buffers between frames, so each is taken
/// when it is there.
struct PendingMatch {
    key: MatchKey,
    readbacks: [Readback; 2],
    pixels: [Option<Vec<u8>>; 2],
}

struct MatchResult {
    key: MatchKey,
    /// 0..1; `None` when neither outline shows.
    iou: Option<f32>,
    /// Where the clay is too big and too small, the size of the renders.
    difference: egui::TextureHandle,
}

/// How long the views stay still before the silhouettes are compared, in
/// seconds.
const MATCH_STILL: f64 = 0.25;
/// The flat renders hold about this many pixels, in the sculpt view's shape.
const MATCH_PIXELS: f32 = 256.0 * 256.0;

/// The size of the flat renders for a view of `aspect`.
fn match_size(aspect: f32) -> [u32; 2] {
    let side = MATCH_PIXELS.sqrt();
    let root = aspect.clamp(0.1, 10.0).sqrt();
    [
        (side * root).round().max(16.0) as u32,
        (side / root).round().max(16.0) as u32,
    ]
}

/// The difference between two flat renders, pixel by pixel: where the clay
/// alone is inked, where the reference alone is, and clear elsewhere.
fn difference_image(size: [u32; 2], clay: &[u8], reference: &[u8]) -> egui::ColorImage {
    let over = theme::CLAY_OVER.gamma_multiply(0.6);
    let under = theme::CLAY_UNDER.gamma_multiply(0.6);
    let pixels = clay
        .as_chunks::<4>()
        .0
        .iter()
        .zip(reference.as_chunks::<4>().0)
        .map(|(c, r)| match (clay::inked(c), clay::inked(r)) {
            (true, false) => over,
            (false, true) => under,
            _ => Color32::TRANSPARENT,
        })
        .collect();
    egui::ColorImage::new([size[0] as usize, size[1] as usize], pixels)
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
            link: true,
            views: [Aids::default(); 2],
            focus: SCULPT,
            plumb_x: [0.5; 2],
            plumb_drag: None,
            grid_divisions: 4,
            ghost: false,
            timer: PoseTimer::default(),
            measure: Measure::default(),
            matching: SilhouetteMatch::default(),
            wireframe: false,
        }
    }
}

/// How the reference part should draw the reference into a pass: the
/// study aids decide, the reference part knows how (splats, mesh).
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(dead_code)] // read by the reference part's drawing, once it lands
pub(super) struct ReferenceLook {
    /// Filled flat with `ink`: no light, no own colours.
    pub(super) flat: bool,
    /// The silhouette's colour: `theme::SILHOUETTE_INK`, or a dark one
    /// over a light background.
    pub(super) ink: Color32,
    /// 0..1.
    pub(super) opacity: f32,
    /// Laid over everything the pass has drawn as one see-through layer,
    /// rather than hidden by what is in front of it: the ghost. Meshes do
    /// this with `ClayPass::prepare_ghost` and `draw_ghost`; splats skip
    /// the depth test.
    pub(super) over: bool,
    /// Where the light comes from, for a mesh reference that isn't flat.
    pub(super) lighting: Lighting,
}

impl App {
    /// The clay's look in the sculpt view.
    pub(super) fn sculpt_look(&self) -> Look {
        let aids = &self.study.views[SCULPT];
        let look = if aids.silhouette {
            Look::flat(self.view_inks(SCULPT).silhouette)
        } else {
            Look::lit(theme::CLAY)
        };
        look.with_lighting(aids.lighting)
    }

    /// Draws the clay's edges over the clay just drawn into `slot` as the
    /// sculpt view shows it, when the wireframe is on: the live clay, or
    /// the timelapse's. Not over a silhouette, which is all outline.
    pub(super) fn draw_wireframe(&self, pass: &mut wgpu::RenderPass<'_>, slot: Slot) {
        if !self.study.wireframe || self.study.views[SCULPT].silhouette {
            return;
        }
        let (gpu, mesh) = match self.playback.as_ref().and_then(|p| p.played.as_ref()) {
            Some(p) => (&p.gpu_mesh, &p.replay.sculpt.mesh),
            None => (&self.gpu_mesh, &self.sculpt.mesh),
        };
        self.clay.draw_wire(&self.device, pass, slot, gpu, mesh);
    }

    /// The reference's look in its own view.
    #[allow(dead_code)] // called by the reference part's drawing, once it lands
    pub(super) fn reference_look(&self) -> ReferenceLook {
        let aids = &self.study.views[REFERENCE];
        ReferenceLook {
            flat: aids.silhouette,
            ink: self.view_inks(REFERENCE).silhouette,
            opacity: 1.0,
            over: false,
            lighting: aids.lighting,
        }
    }

    /// The reference's look as a ghost over the sculpt view, when that is
    /// on: as the sculpt view shows its clay.
    pub(super) fn ghost_look(&self) -> Option<ReferenceLook> {
        let aids = &self.study.views[SCULPT];
        self.study.ghost.then_some(ReferenceLook {
            flat: aids.silhouette,
            ink: self.view_inks(SCULPT).silhouette,
            opacity: theme::GHOST_OPACITY,
            over: true,
            lighting: aids.lighting,
        })
    }

    /// Lays the reference over the sculpt drawn into `slot`, a target of
    /// `size` pixels, seen with `camera`, when the ghost is on. Called last
    /// in the pass.
    pub(super) fn draw_ghost(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        slot: Slot,
        camera: &OrbitCamera,
        size: [u32; 2],
    ) {
        let Some(look) = self.ghost_look() else {
            return;
        };
        self.draw_reference_sized(pass, slot, camera, size, &look);
    }

    /// The camera link, at the top of the view menu.
    pub(super) fn link_item(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        if menu::item(ui, tr.link_cameras, "L", self.study.link).clicked() {
            self.toggle_link();
        }
    }

    /// Linking starts from the sculpt's view, the one being worked on.
    pub(super) fn toggle_link(&mut self) {
        self.study.link = !self.study.link;
        self.follow_camera(SCULPT);
    }

    /// The view whose aids the view menu switches: the one the pointer was
    /// last over, or the one that shows.
    pub(super) fn focused_view(&self) -> usize {
        let shown = self.view_rects.map(shown);
        let focus = self.study.focus;
        if shown[focus] || !shown[1 - focus] {
            focus
        } else {
            1 - focus
        }
    }

    /// The view a key switches the aids of: the one under the pointer, or
    /// else the one it was last over.
    pub(super) fn key_view(&self, ui: &egui::Ui) -> usize {
        let pointer = ui.input(|i| i.pointer.hover_pos());
        [SCULPT, REFERENCE]
            .into_iter()
            .find(|&index| {
                let rect = self.view_rects[index];
                shown(rect) && pointer.is_some_and(|p| rect.contains(p))
            })
            .unwrap_or_else(|| self.focused_view())
    }

    /// One of a view's options in the view menu: its toolbar's name and
    /// key, chosen when on.
    fn option_item(&mut self, ui: &mut egui::Ui, option: ViewOption, index: usize) {
        let label = option.label(self.lang.tr(), index, self.reference_kind());
        if menu::item(ui, &label, option.keys(), self.option_on(option, index)).clicked() {
            self.apply(option, index);
        }
    }

    /// The comparison aids in the view menu. Each view has its own: the
    /// items switch those of the view the pointer was last over, named at
    /// their head.
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
        if menu::item(ui, st.measure, "E", self.study.measure.on).clicked() {
            self.toggle_measure();
        }
        ui.separator();
        let index = self.focused_view();
        let view_name = |index| {
            if index == SCULPT {
                st.sculpt_view
            } else {
                st.reference_view
            }
        };
        menu::note(ui, view_name(index));
        for option in [
            ViewOption::Perspective,
            ViewOption::Mirror,
            ViewOption::Silhouette,
            ViewOption::Plumb,
            ViewOption::Grid,
            ViewOption::Calipers,
        ] {
            if option == ViewOption::Calipers && !self.calipers_fit(index) {
                continue;
            }
            self.option_item(ui, option, index);
        }
        menu::submenu(ui, st.grid_divisions, |ui| {
            ui.set_min_width(80.0);
            for n in GRID_DIVISIONS {
                let chosen = self.study.views[index].grid && self.study.grid_divisions == n;
                if menu::item(ui, &n.to_string(), "", chosen).clicked() {
                    self.study.grid_divisions = n;
                    self.study.views[index].grid = true;
                }
            }
        });
        menu::submenu(ui, st.lighting, |ui| {
            ui.set_min_width(120.0);
            for lighting in Lighting::ALL {
                let label = lighting_name(st, lighting);
                let on = self.study.views[index].lighting == lighting;
                if menu::item(ui, label, "B", on).clicked() {
                    self.study.views[index].lighting = lighting;
                }
            }
        });
        // The sculpt view's own.
        if index != SCULPT {
            ui.separator();
            menu::note(ui, view_name(SCULPT));
        }
        for option in [
            ViewOption::Ghost,
            ViewOption::Wireframe,
            ViewOption::Difference,
        ] {
            self.option_item(ui, option, SCULPT);
        }
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
            // Each view lightens, or darkens if it is light.
            for (index, rect) in rects.into_iter().enumerate().filter(|&(_, r)| shown(r)) {
                let veil = self.view_inks(index).ink.gamma_multiply(0.22 * flash);
                ui.painter().rect_filled(rect.intersect(area), 0.0, veil);
            }
            ui.ctx().request_repaint();
        }
        let index = if shown(rects[SCULPT]) {
            SCULPT
        } else {
            REFERENCE
        };
        let (rect, inks) = (rects[index], self.view_inks(index));
        let timer = &mut self.study.timer;
        if timer.clock == Clock::Off {
            return;
        }
        let remaining = timer.remaining(now);
        let painter = ui.painter();
        let clock = painter.layout_no_wrap(clock_text(remaining), fonts::light(26.0), inks.ink);
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
            inks.ink
        } else {
            inks.text
        };
        painter.galley(clock_rect.min, clock, colour);
        if timer.rounds > 1 {
            painter.text(
                egui::pos2(clock_rect.right(), clock_rect.bottom() + 2.0),
                Align2::RIGHT_TOP,
                format!("{} / {}", timer.round + 1, timer.rounds),
                fonts::regular(12.0),
                inks.text,
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
        m: Modifiers,
    ) -> bool {
        if self.caliper_press(rect, index, screen, m) {
            return true;
        }
        if self.on_bob(rect, index, screen) {
            self.study.plumb_drag = Some(index);
            return true;
        }
        // Measuring takes a plain left drag in either view; with a
        // modifier the press still navigates.
        if self.study.measure.on && !(m.alt || m.ctrl || m.command) {
            let at = to_heights(rect, self.view_pos_in(index, rect, screen));
            let measure = &mut self.study.measure;
            measure.lines[index] = Some([at, at]);
            measure.drawing = Some(index);
            return true;
        }
        false
    }

    /// `screen` is on the bob of view `index`'s plumb line, and the bob is
    /// free to take it: the clay is not under it.
    fn on_bob(&self, rect: Rect, index: usize, screen: Pos2) -> bool {
        let plumb = self.study.views[index].plumb;
        if !plumb || screen.distance(guides::bob(rect, self.study.plumb_x[index])) > BOB_REACH {
            return false;
        }
        let sculpting = index == SCULPT && self.playback.is_none();
        !(sculpting && self.pick(rect, self.view_pos(rect, screen)).is_some())
    }

    /// The plumb line and the grid over each shown view. They are drawn in
    /// screen space, so the mirror view does not flip them, and the grid's
    /// square cells match from view to view whatever their widths.
    ///
    /// The other aids drawn over the views follow: the proportion measure
    /// and the silhouette match.
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
            let aids = self.study.views[index];
            if aids.grid {
                guides::grid(&painter, rect, self.study.grid_divisions);
            }
            if !aids.plumb {
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
            let bob = guides::plumb_line(&painter, rect, self.study.plumb_x[index]);
            if hovered || dragged {
                painter.circle_stroke(bob, 7.0, Stroke::new(1.0, theme::GUIDE));
            }
        }
        self.measure_ui(ui, rects, pointer, held);
        self.calipers_ui(ui, rects);
        self.silhouette_match(ui, rects);
    }

    /// The measured lines: the one being dragged follows the pointer; each
    /// shows with its length, the reference's as 1 and the clay's as a
    /// share of it, and a view still without one says what to do.
    fn measure_ui(&mut self, ui: &egui::Ui, rects: [Rect; 2], pointer: Option<Pos2>, held: bool) {
        if let Some(index) = self.study.measure.drawing {
            let rect = rects[index];
            if held {
                if let Some(pos) = pointer {
                    let at = to_heights(rect, self.view_pos_in(index, rect, pos));
                    if let Some([_, end]) = &mut self.study.measure.lines[index] {
                        *end = at;
                    }
                }
            } else {
                let measure = &mut self.study.measure;
                measure.drawing = None;
                let short = measure
                    .length(index)
                    .is_none_or(|l| l * rect.height() < MEASURE_MIN);
                if short {
                    measure.lines[index] = None;
                }
            }
        }
        let measure = &self.study.measure;
        if !measure.on {
            return;
        }
        let st = self.lang.tr().study;
        let ratio = measure.ratio();
        let line = Stroke::new(1.0, theme::GUIDE);
        for (index, rect) in rects.into_iter().enumerate() {
            if !shown(rect) {
                continue;
            }
            let painter = ui.painter().with_clip_rect(rect);
            let Some(ends) = measure.lines[index] else {
                painter.text(
                    pos2(rect.center().x, rect.bottom() - 40.0),
                    Align2::CENTER_BOTTOM,
                    st.measure_hint,
                    fonts::regular(13.0),
                    self.view_inks(index).text,
                );
                continue;
            };
            let [a, b] = ends.map(|at| self.view_pos_in(index, rect, from_heights(rect, at)));
            guides::ruler(&painter, [a, b], line);
            let label = if index == REFERENCE { Some(1.0) } else { ratio };
            if let Some(value) = label.filter(|_| a.distance(b) >= MEASURE_MIN) {
                // Beside the middle, on the side away from the view's centre.
                painter.text(
                    guides::label_at([a, b], rect.center(), 16.0),
                    Align2::CENTER_CENTER,
                    format!("{value:.2}"),
                    fonts::regular(13.0),
                    self.view_inks(index).ink,
                );
            }
        }
    }

    fn toggle_measure(&mut self) {
        let measure = &mut self.study.measure;
        measure.on = !measure.on;
        measure.lines = [None; 2];
        measure.drawing = None;
    }

    /// What the silhouette match compares now, or `None` when there is
    /// nothing to compare: the cameras unlinked, no 3D reference, the
    /// sculpt view hidden or a timelapse playing.
    fn match_key(&self, rects: [Rect; 2]) -> Option<MatchKey> {
        let rect = rects[SCULPT];
        let reference = self.reference.current.as_ref()?;
        // Both views shown: the score compares what the two show.
        if !self.study.link
            || !shown(rect)
            || !shown(rects[REFERENCE])
            || self.playback.is_some()
            || matches!(reference.content, Content::Image(_))
        {
            return None;
        }
        let positions = &self.sculpt.mesh.positions;
        let stride = (positions.len() / 64).max(1);
        let sample = positions.iter().step_by(stride).fold(0u64, |h, p| {
            p.to_array()
                .iter()
                .fold(h, |h, v| h.rotate_left(7) ^ u64::from(v.to_bits()))
        });
        Some(MatchKey {
            camera: self.cameras[SCULPT],
            size: match_size(rect.aspect_ratio()),
            clay: (self.sculpt.log().len(), positions.len(), sample),
            reference: (
                std::sync::Arc::as_ptr(&reference.source.bytes).cast::<u8>() as usize,
                reference.align,
                reference.crop,
            ),
        })
    }

    /// Compares the silhouettes when they are due, takes the pixels when
    /// they are back, and shows the score (and the difference, if on) over
    /// the sculpt view.
    fn silhouette_match(&mut self, ui: &egui::Ui, rects: [Rect; 2]) {
        let Some(key) = self.match_key(rects) else {
            let matching = &mut self.study.matching;
            (matching.seen, matching.pending, matching.result) = (None, None, None);
            return;
        };
        self.take_match();
        let now = self.now;
        let still = self.drags.iter().all(Option::is_none);
        let matching = &mut self.study.matching;
        let since = match &matching.seen {
            Some((seen, since)) if *seen == key => *since,
            _ => {
                matching.seen = Some((key.clone(), now));
                now
            }
        };
        let fresh = matching.result.as_ref().is_some_and(|r| r.key == key);
        if matching.pending.is_some() {
            // The browser maps the buffers between frames: look again.
            ui.ctx().request_repaint();
        } else if !fresh {
            let wait = MATCH_STILL - (now - since);
            if still && wait <= 0.0 {
                self.start_match(key);
                ui.ctx().request_repaint();
            } else {
                let wait = wait.max(0.02);
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(wait));
            }
        }
        self.match_ui(ui, rects[SCULPT], fresh);
    }

    /// Renders the clay and the reference flat into the small targets from
    /// the sculpt camera, and starts reading them back. Its own submit,
    /// before the frame's: the slots' uniforms are written again for the
    /// views after it.
    fn start_match(&mut self, key: MatchKey) {
        let size = key.size;
        let matching = &mut self.study.matching;
        if matching.targets.as_ref().is_none_or(|t| t[0].size != size) {
            matching.targets = Some([
                ViewTarget::new(&self.device, size),
                ViewTarget::new(&self.device, size),
            ]);
        }
        // The clay as it is now, though the frame uploads it later.
        for range in self.sculpt.take_dirty() {
            self.gpu_mesh.write(&self.queue, &self.sculpt.mesh, range);
        }
        let Some(targets) = &self.study.matching.targets else {
            return;
        };
        let camera = key.camera;
        let aspect = size[0] as f32 / size[1] as f32;
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let ink = Look::flat(theme::SILHOUETTE_INK);
        self.clay
            .prepare(&self.queue, Slot::SculptView, &camera, aspect, &ink);
        {
            let mut pass = targets[0].begin_pass(&mut encoder, theme::VIEW_BG);
            self.clay.draw(&mut pass, Slot::SculptView, &self.gpu_mesh);
        }
        {
            let mut pass = targets[1].begin_pass(&mut encoder, theme::VIEW_BG);
            // Measured: the default ink on the default black, whatever
            // the views show.
            let look = ReferenceLook {
                flat: true,
                ink: theme::SILHOUETTE_INK,
                opacity: 1.0,
                over: false,
                lighting: Lighting::default(),
            };
            self.draw_reference_sized(&mut pass, Slot::ReferenceView, &camera, size, &look);
        }
        let readbacks = targets
            .each_ref()
            .map(|t| Readback::copy(&self.device, &mut encoder, t));
        self.queue.submit([encoder.finish()]);
        for readback in &readbacks {
            readback.map();
        }
        self.study.matching.pending = Some(PendingMatch {
            key,
            readbacks,
            pixels: [None, None],
        });
    }

    /// Takes the flat renders' pixels as they arrive, and scores them once
    /// both are there.
    fn take_match(&mut self) {
        let matching = &mut self.study.matching;
        let Some(pending) = &mut matching.pending else {
            return;
        };
        for (readback, pixels) in pending.readbacks.iter().zip(&mut pending.pixels) {
            if pixels.is_some() {
                continue;
            }
            match readback.try_take(&self.device) {
                Ok(taken) => *pixels = taken,
                Err(_) => {
                    log::warn!("could not read the silhouettes back");
                    matching.pending = None;
                    return;
                }
            }
        }
        let [Some(clay), Some(reference)] = &pending.pixels else {
            return;
        };
        let size = pending.key.size;
        let iou = Overlap::of(clay, reference).iou();
        let difference = self.ctx.load_texture(
            "silhouette difference",
            difference_image(size, clay, reference),
            egui::TextureOptions::LINEAR,
        );
        matching.result = Some(MatchResult {
            key: pending.key.clone(),
            iou,
            difference,
        });
        matching.pending = None;
    }

    /// The score in the sculpt view's lower right corner, dimmed while a
    /// newer one is due, and the difference tint over the view while the
    /// score is fresh.
    fn match_ui(&self, ui: &egui::Ui, rect: Rect, fresh: bool) {
        let Some(result) = &self.study.matching.result else {
            return;
        };
        let st = self.lang.tr().study;
        let painter = ui.painter().with_clip_rect(rect);
        let tint = self.study.matching.tint;
        if tint && fresh {
            let uv = if self.study.views[SCULPT].mirror {
                Rect::from_min_max(pos2(1.0, 0.0), pos2(0.0, 1.0))
            } else {
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
            };
            painter.image(result.difference.id(), rect, uv, Color32::WHITE);
        }
        let score = result
            .iou
            .map_or("–".to_owned(), |iou| format!("{:.0}%", iou * 100.0));
        let colour = if fresh {
            self.view_inks(SCULPT).ink
        } else {
            self.view_inks(SCULPT).text
        };
        let corner = rect.right_bottom() + egui::vec2(-14.0, -12.0);
        let shown = painter.text(
            corner,
            Align2::RIGHT_BOTTOM,
            format!("{} {score}", st.match_score),
            fonts::regular(13.0),
            colour,
        );
        if tint {
            // The legend, above the score: a short stroke of each tone.
            let mut y = shown.top() - 8.0;
            for (label, tone) in [
                (st.too_small, theme::CLAY_UNDER),
                (st.too_big, theme::CLAY_OVER),
            ] {
                let text = painter.text(
                    pos2(corner.x, y),
                    Align2::RIGHT_BOTTOM,
                    label,
                    fonts::regular(12.0),
                    self.view_inks(SCULPT).text,
                );
                painter.hline(
                    text.left() - 22.0..=text.left() - 8.0,
                    text.center().y,
                    Stroke::new(3.0, tone),
                );
                y = text.top() - 4.0;
            }
        }
    }

    pub(super) fn study_shortcuts(&mut self, ui: &egui::Ui) {
        use egui::Modifiers as M;
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let pressed = |key: Key| ui.input_mut(|i| i.consume_key(M::NONE, key));
        // Each view's own aids switch in the view under the pointer.
        let index = self.key_view(ui);
        // Before plain W, which would take Shift+W too.
        if ui.input_mut(|i| i.consume_key(M::SHIFT, Key::W)) {
            self.apply(ViewOption::Wireframe, SCULPT);
        }
        if pressed(Key::W) {
            self.study.swapped = !self.study.swapped;
        }
        if pressed(Key::L) {
            self.toggle_link();
        }
        // Before plain K, which would take Shift+K too.
        if ui.input_mut(|i| i.consume_key(M::SHIFT, Key::K)) {
            self.apply(ViewOption::Difference, SCULPT);
        }
        if pressed(Key::O) {
            self.apply(ViewOption::Ghost, SCULPT);
        }
        for (key, option) in [
            (Key::R, ViewOption::Mirror),
            (Key::K, ViewOption::Silhouette),
            (Key::V, ViewOption::Plumb),
            (Key::G, ViewOption::Grid),
            (Key::B, ViewOption::Lighting),
        ] {
            if pressed(key) {
                self.apply(option, index);
            }
        }
        if pressed(Key::E) {
            self.toggle_measure();
        }
        // Esc clears the measured lines, then puts the measure away.
        if self.study.measure.on && pressed(Key::Escape) {
            let measure = &mut self.study.measure;
            if measure.lines.iter().any(Option::is_some) {
                measure.lines = [None; 2];
                measure.drawing = None;
            } else {
                measure.on = false;
            }
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
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
    pub(in crate::app) struct Harness {
        pub(in crate::app) app: App,
        view: wgpu::TextureView,
        time: f64,
    }

    const SIZE: [u32; 2] = [1280, 720];

    impl Harness {
        pub(in crate::app) fn new() -> Option<Self> {
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

        pub(in crate::app) fn frames(&mut self, n: usize) {
            for _ in 0..n {
                self.time += 1.0 / 60.0;
                self.app.frame(self.time, &self.view, SIZE, 1.0);
            }
        }

        /// One frame, and what it asked the shell for.
        pub(in crate::app) fn frame_requests(&mut self) -> Vec<crate::app::Request> {
            self.time += 1.0 / 60.0;
            self.app.frame(self.time, &self.view, SIZE, 1.0).requests
        }

        pub(in crate::app) fn pointer(&mut self, phase: crate::PointerPhase, pos: Pos2) {
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
        pub(in crate::app) fn drag(&mut self, from: Pos2, to: Pos2) {
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
        h.app.study.views[SCULPT].plumb = true;
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
        let start = guides::bob(rect, 0.5);
        h.drag(start, start + egui::vec2(80.0, -40.0));
        let moved = h.app.study.plumb_x[SCULPT];
        let expected = (start.x + 80.0 - rect.left()) / rect.width();
        assert!((moved - expected).abs() < 0.01, "{moved} vs {expected}");
        assert_eq!(strokes(&h), 0);

        // Pressing the line itself, away from the bob, does not move it.
        let x = guides::bob(rect, moved).x;
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

    #[test]
    fn the_measure_compares_lengths_in_view_heights() {
        let mut m = Measure::default();
        assert_eq!(m.ratio(), None);
        m.lines[SCULPT] = Some([Vec2::ZERO, egui::vec2(0.3, 0.4)]);
        assert_eq!(m.ratio(), None, "nothing to compare with yet");
        m.lines[REFERENCE] = Some([egui::vec2(-0.5, 0.0), egui::vec2(0.5, 0.0)]);
        assert!((m.ratio().unwrap() - 0.5).abs() < 1e-6);

        // A view twice as tall shows the form twice as large.
        let small = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        let tall = Rect::from_min_size(pos2(500.0, 0.0), egui::vec2(800.0, 600.0));
        let a = to_heights(small, pos2(200.0, 150.0) + egui::vec2(30.0, 0.0));
        let b = to_heights(tall, pos2(900.0, 300.0) + egui::vec2(60.0, 0.0));
        assert!((a - b).length() < 1e-6);
        assert_eq!(from_heights(tall, b), pos2(960.0, 300.0));
    }

    #[test]
    fn match_renders_keep_the_view_shape_and_size() {
        for aspect in [0.5, 1.0, 1.6, 3.0] {
            let [w, h] = match_size(aspect);
            assert!(
                (w as f32 / h as f32 - aspect).abs() < 0.02 * aspect,
                "{aspect}"
            );
            let pixels = (w * h) as f32;
            assert!(
                (pixels / MATCH_PIXELS - 1.0).abs() < 0.02,
                "{aspect}: {pixels}"
            );
        }
    }

    #[test]
    fn the_difference_tints_too_big_and_too_small_apart() {
        let ink = theme::SILHOUETTE_INK.to_array();
        let bg = theme::VIEW_BG.to_array();
        let clay = [ink, ink, bg, bg].concat();
        let reference = [ink, bg, ink, bg].concat();
        let image = difference_image([4, 1], &clay, &reference);
        assert_eq!(image.pixels[0], Color32::TRANSPARENT, "both");
        assert_eq!(image.pixels[1], theme::CLAY_OVER.gamma_multiply(0.6));
        assert_eq!(image.pixels[2], theme::CLAY_UNDER.gamma_multiply(0.6));
        assert_eq!(image.pixels[3], Color32::TRANSPARENT, "neither");
    }

    /// The sphere the clay starts as, written as an OBJ.
    fn clay_as_obj() -> Vec<u8> {
        let (mesh, _) = super::super::sculpting::start_clay();
        let mut obj = String::new();
        for p in &mesh.positions {
            obj += &format!("v {} {} {}\n", p.x, p.y, p.z);
        }
        for t in mesh.indices().as_chunks::<3>().0 {
            obj += &format!("f {} {} {}\n", t[0] + 1, t[1] + 1, t[2] + 1);
        }
        obj.into_bytes()
    }

    impl Harness {
        /// Runs frames until a fresh silhouette match is in, and gives its
        /// score.
        fn settled_match(&mut self) -> Option<f32> {
            for _ in 0..240 {
                self.frames(1);
                let rects = self.app.view_rects;
                let key = self.app.match_key(rects)?;
                let matching = &self.app.study.matching;
                if let Some(result) = &matching.result
                    && result.key == key
                {
                    return result.iou;
                }
            }
            panic!("no silhouette match after 240 frames");
        }
    }

    #[test]
    fn the_silhouette_match_scores_the_outlines_overlap() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        h.app.study.link = false;
        h.app.load_reference("clay.obj", clay_as_obj().into());
        // Placed where the clay is, as it is, rather than fitted.
        h.app.reference.current.as_mut().unwrap().align = Alignment::default();
        h.frames(2);
        assert!(h.app.match_key(h.app.view_rects).is_none(), "unlinked");
        h.app.study.link = true;
        h.app.follow_camera(SCULPT);

        // The clay itself as the reference: the outlines agree.
        let same = h.settled_match().expect("a score");
        assert!(same > 0.99, "{same}");

        // The reference shrunk to 70 %: about 0.7² of the clay's area.
        let reference = h.app.reference.current.as_mut().unwrap();
        reference.align.scale *= 0.7;
        let smaller = h.settled_match().expect("a score");
        assert!((smaller - 0.49).abs() < 0.06, "{smaller}");

        // A turn of the linked cameras is scored again, once still.
        h.app.cameras[SCULPT].orbit(120.0, 0.0);
        h.app.follow_camera(SCULPT);
        let turned = h.settled_match().expect("a score");
        assert!((turned - smaller).abs() < 0.06, "{turned}");
    }

    /// A `.splat` file of a few small grey splats around the origin.
    fn dots_splat() -> Vec<u8> {
        let mut bytes = Vec::new();
        for i in 0..8 {
            let x = i as f32 * 0.1 - 0.35;
            for v in [x, 0.0, 0.0, 0.05, 0.05, 0.05] {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            bytes.extend_from_slice(&[160, 160, 160, 255, 255, 128, 128, 128]);
        }
        bytes
    }

    #[test]
    fn a_hidden_view_draws_no_reference() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        h.app.study.ghost = true;
        for (layout, sculpt, reference) in [
            (Layout::Sculpt, true, false),
            (Layout::Reference, false, true),
            (Layout::Both, true, true),
        ] {
            h.app.study.layout = layout;
            // A fresh reference, never drawn into any slot yet.
            h.app.load_reference("dots.splat", dots_splat().into());
            h.frames(2);
            assert_eq!(
                h.app.splats_drawn_into(Slot::SculptView),
                sculpt,
                "the ghost, {layout:?}"
            );
            assert_eq!(
                h.app.splats_drawn_into(Slot::ReferenceView),
                reference,
                "the reference, {layout:?}"
            );
        }
    }

    #[test]
    fn the_measure_takes_a_drag_in_each_view_and_esc_clears_it() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        h.app.study.measure.on = true;
        h.frames(1);
        let [sculpt, reference] = h.app.view_rects;
        let strokes_before = h.app.sculpt.log().len();
        // Across the clay: measuring, not sculpting.
        let c = sculpt.center();
        h.drag(c - egui::vec2(30.0, 0.0), c + egui::vec2(30.0, 0.0));
        let c = reference.center();
        h.drag(c - egui::vec2(0.0, 60.0), c + egui::vec2(0.0, 60.0));
        assert_eq!(h.app.sculpt.log().len(), strokes_before);
        let ratio = h.app.study.measure.ratio().expect("both lines");
        assert!((ratio - 0.5).abs() < 0.01, "{ratio}");

        let esc = |h: &mut Harness| {
            for pressed in [true, false] {
                h.app.push_event(crate::InputEvent::Key {
                    key: Key::Escape,
                    pressed,
                    repeat: false,
                });
            }
            h.frames(1);
        };
        esc(&mut h);
        assert!(h.app.study.measure.on);
        assert_eq!(h.app.study.measure.lines, [None; 2]);
        esc(&mut h);
        assert!(!h.app.study.measure.on);
    }

    #[test]
    fn the_wireframe_shows_quads_live_after_an_autosave_and_in_the_timelapse() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        // Shift+W turns it on, and leaves swap sides (W) alone.
        let shift = |h: &mut Harness, on: bool| {
            h.app
                .push_event(crate::InputEvent::Modifiers(crate::input::Modifiers {
                    shift: on,
                    ..Default::default()
                }));
        };
        shift(&mut h, true);
        for pressed in [true, false] {
            h.app.push_event(crate::InputEvent::Key {
                key: Key::W,
                pressed,
                repeat: false,
            });
        }
        h.frames(1);
        shift(&mut h, false);
        assert!(h.app.study.wireframe);
        assert!(!h.app.study.swapped);
        let quads = h.app.sculpt.mesh.quads.len();
        assert_eq!(quads, h.app.sculpt.mesh.triangles.len() / 2);
        let c = h.app.view_rects[SCULPT].center();
        h.drag(c, c + egui::vec2(40.0, 10.0));
        assert!(!h.app.sculpt.log().is_empty());
        h.frames(1);

        let saved = h.app.autosave_bytes().expect("worth keeping").scene;
        h.app.new_scene();
        h.app.sculpt.mesh.quads.clear();
        h.frames(1);
        assert!(h.app.restore_autosave(&saved, None));
        h.frames(1);
        assert_eq!(h.app.sculpt.mesh.quads.len(), quads);

        h.app.start_playback();
        h.frames(3);
        let played = h.app.playback.as_ref().and_then(|p| p.played.as_ref());
        let played = played.expect("playing");
        assert_eq!(played.replay.sculpt.mesh.quads.len(), quads);
        // Over the silhouette it stays off.
        h.app.study.views[SCULPT].silhouette = true;
        h.frames(1);
    }

    impl Harness {
        /// A key pressed and let go, with `shift` held.
        pub(in crate::app) fn key(&mut self, key: Key, shift: bool) {
            self.chord(
                key,
                crate::input::Modifiers {
                    shift,
                    ..Default::default()
                },
            );
        }

        /// Numpad `digit` pressed and let go, for a frame.
        pub(in crate::app) fn numpad(&mut self, digit: u8) {
            for pressed in [true, false] {
                self.app.push_event(crate::InputEvent::Numpad {
                    digit,
                    pressed,
                    repeat: false,
                });
            }
            self.frames(1);
        }

        /// Presses and lets go of `key` with `modifiers` held, for a frame.
        pub(in crate::app) fn chord(&mut self, key: Key, modifiers: crate::input::Modifiers) {
            self.app.push_event(crate::InputEvent::Modifiers(modifiers));
            for pressed in [true, false] {
                self.app.push_event(crate::InputEvent::Key {
                    key,
                    pressed,
                    repeat: false,
                });
            }
            self.frames(1);
            self.app
                .push_event(crate::InputEvent::Modifiers(Default::default()));
        }
    }

    #[test]
    fn keys_switch_the_aids_of_the_view_under_the_pointer() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let [sculpt, reference] = h.app.view_rects;
        h.pointer(crate::PointerPhase::Move, reference.center());
        for key in [Key::G, Key::V, Key::K, Key::R, Key::B] {
            h.key(key, false);
        }
        let aids = h.app.study.views;
        assert!(aids[REFERENCE].grid && aids[REFERENCE].plumb);
        assert!(aids[REFERENCE].silhouette && aids[REFERENCE].mirror);
        assert_eq!(aids[REFERENCE].lighting, Lighting::Raking);
        assert_eq!(
            aids[SCULPT],
            Aids::default(),
            "the sculpt view keeps its own"
        );
        // The view menu switches those of the view the pointer was last
        // over, though it is over the menu now.
        h.pointer(crate::PointerPhase::Move, egui::pos2(5.0, 5.0));
        assert_eq!(h.app.focused_view(), REFERENCE);

        // Over the sculpt view: its own, and the sculpt-only ones.
        h.pointer(crate::PointerPhase::Move, sculpt.center());
        assert_eq!(h.app.focused_view(), SCULPT);
        h.key(Key::G, false);
        h.key(Key::K, true);
        assert!(h.app.study.views[SCULPT].grid);
        assert!(h.app.study.matching.tint);
        assert!(!h.app.study.views[SCULPT].silhouette, "Shift+K is not K");

        // Shown alone, the reference view takes the keys wherever the
        // pointer is.
        h.app.study.layout = Layout::Reference;
        h.frames(1);
        h.pointer(crate::PointerPhase::Move, egui::pos2(5.0, 5.0));
        h.key(Key::G, false);
        assert!(!h.app.study.views[REFERENCE].grid);
        assert!(h.app.study.views[SCULPT].grid);
    }

    #[test]
    fn linked_cameras_share_their_projection() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        assert!(h.app.study.link, "linked at first");
        h.app.apply(ViewOption::Link, SCULPT);
        h.app.apply(ViewOption::Link, SCULPT);
        h.app.apply(ViewOption::Perspective, REFERENCE);
        assert!(h.app.cameras.iter().all(|c| c.ortho));
        h.app.cameras[SCULPT].dolly(2.0);
        h.app.apply(ViewOption::Reset, SCULPT);
        assert!(h.app.cameras.iter().all(|c| c.ortho), "reset keeps it");
        let start = OrbitCamera::default().distance;
        assert!(h.app.cameras.iter().all(|c| c.distance == start));
        h.app.apply(ViewOption::Link, SCULPT);
        h.app.apply(ViewOption::Perspective, SCULPT);
        assert!(!h.app.cameras[SCULPT].ortho && h.app.cameras[REFERENCE].ortho);
    }

    #[test]
    fn the_toolbar_takes_its_clicks_and_never_sculpts() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        // Close up, the clay fills the view under the toolbar too.
        h.app.cameras[SCULPT].distance = 1.3;
        h.frames(1);
        let rects = h.app.view_rects;
        let areas = h.app.toolbar_areas(rects);
        assert!(
            areas[SCULPT].left() < rects[SCULPT].left() + 20.0,
            "on the left"
        );
        assert!(areas[REFERENCE].right() > rects[REFERENCE].right() - 20.0);
        let rect = rects[SCULPT];
        assert!(h.app.pick(rect, areas[SCULPT].center()).is_some());
        let bar = h.app.toolbar_layouts(rects)[SCULPT].clone().unwrap();
        let k = bar
            .options
            .iter()
            .position(|&o| o == ViewOption::Perspective);
        let button = bar.buttons[k.expect("a perspective button")].center();
        h.drag(button, button);
        assert!(h.app.cameras[SCULPT].ortho, "clicked");
        // A drag from the toolbar across the clay is neither a stroke nor
        // a turn.
        let yaw = h.app.cameras[SCULPT].yaw;
        h.drag(button, rect.center());
        assert!(h.app.sculpt.log().is_empty());
        assert_eq!(h.app.cameras[SCULPT].yaw, yaw);
        // Swapped, it follows its view to the right edge.
        h.app.study.swapped = true;
        h.frames(1);
        let rects = h.app.view_rects;
        let areas = h.app.toolbar_areas(rects);
        assert!(areas[SCULPT].right() > rects[SCULPT].right() - 20.0);
        assert!(areas[REFERENCE].left() < rects[REFERENCE].left() + 20.0);
    }

    #[test]
    fn an_orthographic_view_picks_the_point_it_shows() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let camera = &mut h.app.cameras[SCULPT];
        (camera.yaw, camera.pitch, camera.ortho) = (0.0, 0.0, true);
        h.frames(1);
        let rect = h.app.view_rects[SCULPT];
        let front = h.app.pick(rect, rect.center()).expect("the clay");
        let radius = h.app.sculpt.mesh.positions[0].length();
        assert!(
            front.point.z > 0.9 * radius,
            "the near side: {}",
            front.point
        );
        assert!(front.point.truncate().length() < 0.05);
        // Off centre, the point lands where it was picked.
        let camera = h.app.cameras[SCULPT];
        let to_clip = camera.projection(rect.aspect_ratio()) * camera.view();
        for offset in [egui::vec2(60.0, -40.0), egui::vec2(-90.0, 75.0)] {
            let pos = rect.center() + offset;
            let hit = h.app.pick(rect, pos).expect("the clay");
            let ndc = to_clip.project_point3(hit.point);
            let seen = egui::pos2(
                rect.left() + (ndc.x + 1.0) / 2.0 * rect.width(),
                rect.top() + (1.0 - ndc.y) / 2.0 * rect.height(),
            );
            assert!((seen - pos).length() < 0.5, "{seen:?} vs {pos:?}");
            assert!(hit.point.z > 0.0, "the side facing the camera");
        }
        // Zoomed in until the eye is inside the clay: still the near side.
        h.app.cameras[SCULPT].distance = 0.3;
        let hit = h.app.pick(rect, rect.center()).expect("the clay");
        assert!(hit.point.z > 0.9 * radius, "{}", hit.point);
    }

    #[test]
    fn each_views_aids_and_projection_come_back_from_an_autosave() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        // Unlinked, so each view keeps a projection of its own.
        h.app.study.link = false;
        let c = h.app.view_rects[SCULPT].center();
        h.drag(c, c + egui::vec2(30.0, 0.0));
        let st = &mut h.app.study;
        st.views[REFERENCE] = Aids {
            mirror: true,
            plumb: false,
            grid: true,
            silhouette: true,
            lighting: Lighting::Top,
        };
        st.views[SCULPT].plumb = true;
        st.wireframe = true;
        h.app.cameras[REFERENCE].ortho = true;
        let before = h.app.scene_revision();
        h.app.apply(ViewOption::Perspective, SCULPT);
        assert_ne!(h.app.scene_revision(), before, "the projection is saved");
        h.app.apply(ViewOption::Perspective, SCULPT);
        let views = h.app.study.views;
        let saved = h.app.autosave_bytes().expect("worth keeping").scene;
        h.app.new_scene();
        h.app.study.views = Default::default();
        h.app.study.wireframe = false;
        h.app.cameras[REFERENCE].ortho = false;
        assert!(h.app.restore_autosave(&saved, None));
        assert_eq!(h.app.study.views, views);
        assert!(h.app.study.wireframe);
        assert!(h.app.cameras[REFERENCE].ortho && !h.app.cameras[SCULPT].ortho);
    }
}
