// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The brush's size and strength at the pointer: hold Shift + Space over
//! the sculpt view and the panel's two sliders open where the pointer is,
//! the pointer on the size slider's handle. They stay while Space is held
//! (Shift may be let go), as the tool pie does; letting go of Space,
//! Escape or a press outside them closes them.
//!
//! Either key may come first. Shift then Space opens them on Space's
//! press (the pie opens only on a Space with no modifiers). Space then
//! Shift opens the pie first, and the settings take its place as soon as
//! Shift comes down. While they are open no stroke or view drag starts,
//! and Shift doesn't smooth.

use egui::{Event, Key, Order, Pos2, Rect, Sense, Stroke, UiBuilder, vec2};

use super::{MAX_SIZE, MIN_SIZE, TOOL_PIE_KEY};
use crate::app::{App, SCULPT};
use crate::ui::{SLIDER_HEIGHT, SLIDER_TRACK_INSET};

/// The sliders' width, in points.
const WIDTH: f32 = 220.0;
/// Room round the sliders inside the panel, in points.
const MARGIN: f32 = 12.0;
/// Between the two sliders, in points.
const GAP: f32 = 6.0;
/// Kept clear of the window's edges, in points.
const INSET: f32 = 8.0;

/// The panel's size.
fn panel_size() -> egui::Vec2 {
    vec2(
        WIDTH + 2.0 * MARGIN,
        2.0 * SLIDER_HEIGHT + GAP + 2.0 * MARGIN,
    )
}

/// Where the panel goes for a pointer at `at`, the size slider's handle
/// at `t` (0 to 1) along its track: the handle under the pointer, so a
/// press there changes nothing until the pointer moves; then moved as
/// little as it takes to stay inside `screen`.
fn place(at: Pos2, t: f32, screen: Rect) -> Rect {
    let size = panel_size();
    let handle = vec2(
        MARGIN + t.clamp(0.0, 1.0) * WIDTH,
        MARGIN + SLIDER_HEIGHT - SLIDER_TRACK_INSET,
    );
    let min = at - handle;
    let room = screen.shrink(INSET);
    let x = min.x.min(room.right() - size.x).max(room.left());
    let y = min.y.min(room.bottom() - size.y).max(room.top());
    Rect::from_min_size(egui::pos2(x, y), size)
}

/// The open settings: where the pointer was when they opened, the
/// brush's centre, and their panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct QuickSettings {
    pub(in crate::app) at: Pos2,
    pub(in crate::app) panel: Rect,
}

impl App {
    /// The settings' keys, before the tool pie's. Opens them on Shift +
    /// Space over the sculpt view, or in place of the pie when Shift joins
    /// its Space; closes them on Space's release or Escape. Takes Space's
    /// events while they are open. True if they were open this frame: the
    /// pie then leaves the keys alone.
    pub(super) fn quick_settings_keys(&mut self, ui: &egui::Ui) -> bool {
        if self.tool.quick.is_some() {
            let open = ui.input_mut(|i| {
                let escape = i.consume_key(egui::Modifiers::NONE, Key::Escape);
                let released = i.events.iter().any(
                    |e| matches!(e, Event::Key { key, pressed: false, .. } if *key == TOOL_PIE_KEY),
                );
                i.events
                    .retain(|e| !matches!(e, Event::Key { key, .. } if *key == TOOL_PIE_KEY));
                // Losing the window's focus loses the key's release too.
                let held = i.key_down(TOOL_PIE_KEY) && i.focused;
                !escape && !released && held
            });
            if !open {
                self.tool.quick = None;
            }
            return true;
        }
        // Shift joins the pie's Space: the settings take its place.
        if let Some(pie) = self.tool.pie {
            let (shift, pointer) = ui.input(|i| (i.modifiers.shift_only(), i.pointer.latest_pos()));
            if shift {
                self.tool.pie = None;
                self.open_quick_settings(pointer.unwrap_or(pie.centre), ui);
                return true;
            }
            return false;
        }
        // Not in the middle of a stroke or a turn of the view.
        if self.drags[SCULPT].is_some() {
            return false;
        }
        let within = self.view_rects[SCULPT];
        let at = ui.input_mut(|i| {
            let at = i.pointer.latest_pos().filter(|p| within.contains(*p))?;
            let press = i.events.iter().position(|e| {
                matches!(e, Event::Key { key, pressed: true, repeat: false, modifiers, .. }
                    if *key == TOOL_PIE_KEY && modifiers.shift_only())
            })?;
            // Its repeats while held, too: a focused button would take
            // Space as a click.
            i.events.remove(press);
            i.events
                .retain(|e| !matches!(e, Event::Key { key, .. } if *key == TOOL_PIE_KEY));
            Some(at)
        });
        match at {
            Some(at) => {
                self.open_quick_settings(at, ui);
                true
            }
            None => false,
        }
    }

    /// Opens the settings for a pointer at `at`; they take the place of
    /// an open flyout, as the pie does.
    fn open_quick_settings(&mut self, at: Pos2, ui: &egui::Ui) {
        let t = (self.tool.radius - MIN_SIZE) / (MAX_SIZE - MIN_SIZE);
        let panel = place(at, t, ui.ctx().content_rect());
        self.tool.quick = Some(QuickSettings { at, panel });
        self.flyout = None;
        ui.ctx().request_repaint();
    }

    /// Draws the open settings over everything: the brush at its size
    /// where they opened, and the panel's size and strength sliders. They
    /// lie over the whole window and take every press, so nothing under
    /// them starts; a press outside the sliders closes them.
    pub(in crate::app) fn quick_settings_ui(&mut self, ui: &egui::Ui) {
        if self.playback.is_some() {
            self.tool.quick = None;
        }
        let Some(quick) = self.tool.quick else {
            return;
        };
        let ctx = ui.ctx().clone();
        let screen = ctx.content_rect();
        let sculpt = self.view_rects[SCULPT];
        let ink = self.view_inks(SCULPT).ink.gamma_multiply(0.6);
        let p = self.theme.palette();
        egui::Area::new(egui::Id::unique("quick settings"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(&ctx, |ui| {
                // Over the whole window: every press is the settings'.
                ui.allocate_rect(screen, Sense::click_and_drag());
                let brush = ui.painter().with_clip_rect(sculpt);
                brush.circle_stroke(quick.at, self.tool.radius, Stroke::new(1.0, ink));
                brush.circle_filled(quick.at, 1.5, ink);
                ui.painter().rect(
                    quick.panel,
                    0.0,
                    p.bg,
                    Stroke::new(1.0, p.border),
                    egui::StrokeKind::Inside,
                );
                let inner = quick.panel.shrink(MARGIN);
                ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
                    ui.spacing_mut().item_spacing.y = GAP;
                    self.size_and_strength(ui);
                });
            });
        let (pressed_at, pointer) = ctx.input(|i| {
            let pressed = i.pointer.any_pressed();
            (
                pressed.then(|| i.pointer.press_origin()).flatten(),
                i.pointer.latest_pos(),
            )
        });
        if pointer.is_none_or(|p| !quick.panel.contains(p)) {
            ctx.set_cursor_icon(egui::CursorIcon::Default);
        }
        if pressed_at.is_some_and(|p| !quick.panel.contains(p)) {
            self.tool.quick = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::study::tests::Harness;
    use crate::input::Modifiers;
    use crate::sculpt::Brush;
    use crate::{InputEvent, PointerPhase};

    fn screen() -> Rect {
        Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 720.0))
    }

    /// Where the size slider's handle is in `panel`, at `t`.
    fn handle(panel: Rect, t: f32) -> Pos2 {
        panel.min
            + vec2(
                MARGIN + t * WIDTH,
                MARGIN + SLIDER_HEIGHT - SLIDER_TRACK_INSET,
            )
    }

    #[test]
    fn the_size_handle_opens_under_the_pointer_inside_the_window() {
        let at = egui::pos2(500.0, 300.0);
        for t in [0.0, 0.11, 0.5, 1.0] {
            let panel = place(at, t, screen());
            assert!((handle(panel, t) - at).length() < 1e-3, "{t}");
            assert_eq!(panel.size(), panel_size());
        }
        // Near each edge and corner it moves in, the pointer still on it
        // or near it.
        let room = screen().shrink(INSET);
        for at in [
            egui::pos2(2.0, 2.0),
            egui::pos2(1278.0, 2.0),
            egui::pos2(2.0, 718.0),
            egui::pos2(1278.0, 718.0),
            egui::pos2(640.0, 715.0),
        ] {
            for t in [0.0, 0.5, 1.0] {
                let panel = place(at, t, screen());
                assert!(room.contains_rect(panel), "{at:?} {t} {panel:?}");
                let near = panel.expand(INSET + 2.0);
                assert!(near.contains(at), "{at:?} {t} {panel:?}");
            }
        }
    }

    fn modifiers(h: &mut Harness, shift: bool) {
        h.app.push_event(InputEvent::Modifiers(Modifiers {
            shift,
            ..Default::default()
        }));
        h.frames(1);
    }

    fn space(h: &mut Harness, pressed: bool) {
        h.app.push_event(InputEvent::Key {
            key: Key::Space,
            pressed,
            repeat: false,
        });
        h.frames(1);
    }

    fn strokes(h: &Harness) -> usize {
        h.app.sculpt.log().len()
    }

    #[test]
    fn shift_space_opens_size_and_strength_at_the_pointer_in_either_order() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let rect = h.app.view_rects[SCULPT];
        let centre = rect.center();
        let tool = h.app.tool.tool;
        let radius = h.app.tool.radius;
        h.pointer(PointerPhase::Move, centre);

        // Shift, then Space: the settings, not the pie, the size's
        // handle under the pointer.
        modifiers(&mut h, true);
        assert_eq!(h.app.held_tool(), Brush::Smooth, "Shift alone smooths");
        space(&mut h, true);
        let quick = h.app.tool.quick.expect("open");
        assert!(!h.app.pie_open());
        assert!(h.app.popup_open());
        assert_eq!(quick.at, centre);
        let t = (radius - MIN_SIZE) / (MAX_SIZE - MIN_SIZE);
        assert!((handle(quick.panel, t) - centre).length() < 1e-3);
        assert_eq!(h.app.held_tool(), tool, "Shift doesn't smooth while open");
        // Its first frame lays it out.
        h.frames(1);

        // Dragging the size slider to the right grows the brush and shows
        // the readout; nothing is sculpted though the pointer is over the
        // clay.
        h.drag(centre, centre + vec2(60.0, 0.0));
        let grown = h.app.tool.radius;
        assert!(grown > radius + 50.0, "{radius} -> {grown}");
        assert!(h.app.tool.readout.is_some());
        assert_eq!(strokes(&h), 0, "a stroke under the settings");
        // Down to the strength slider: dragging it to its left end.
        let strength = handle(quick.panel, 0.0) + vec2(0.0, SLIDER_HEIGHT + GAP);
        h.drag(strength + vec2(40.0, 0.0), strength - vec2(20.0, 0.0));
        assert_eq!(h.app.tool.strength[super::super::tool_index(tool)], 0.0);
        assert_eq!(h.app.tool.radius, grown);
        assert_eq!(strokes(&h), 0);

        // A press outside closes it, and sculpts nothing.
        let outside = egui::pos2(quick.panel.center().x, quick.panel.bottom() + 80.0);
        assert!(rect.contains(outside));
        h.drag(outside, outside + vec2(40.0, 0.0));
        assert!(!h.app.popup_open());
        assert_eq!(strokes(&h), 0);
        // Space still held, Shift let go: it stays closed, and the pie
        // doesn't open without a new press.
        modifiers(&mut h, false);
        assert!(!h.app.popup_open());
        space(&mut h, false);

        // Space, then Shift: the pie first, then the settings in its place.
        h.pointer(PointerPhase::Move, centre);
        space(&mut h, true);
        assert!(h.app.pie_open());
        modifiers(&mut h, true);
        assert!(!h.app.pie_open());
        assert!(h.app.tool.quick.is_some());
        // Letting go of Shift keeps it; letting go of Space closes it.
        modifiers(&mut h, false);
        assert!(h.app.tool.quick.is_some());
        space(&mut h, false);
        assert!(!h.app.popup_open());
        assert_eq!(h.app.tool.tool, tool, "no tool picked");

        // Escape closes it too.
        modifiers(&mut h, true);
        space(&mut h, true);
        assert!(h.app.tool.quick.is_some());
        h.key(Key::Escape, true);
        assert!(!h.app.popup_open());
        // (The harness lets go of the keys after a chord.)
        modifiers(&mut h, true);
        assert!(!h.app.popup_open());
        space(&mut h, false);
        assert_eq!(h.app.held_tool(), Brush::Smooth, "Shift smooths again");
        modifiers(&mut h, false);

        // Plain Space still opens the pie.
        space(&mut h, true);
        assert!(h.app.pie_open() && h.app.tool.quick.is_none());
        space(&mut h, false);
        assert!(!h.app.popup_open());

        // Closed, the same drag sculpts.
        h.drag(centre, centre + vec2(40.0, 0.0));
        assert!(strokes(&h) > 0);
    }
}
