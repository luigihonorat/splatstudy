// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Pie menus: hold a key and a ring of choices opens around the pointer.
//! Moving towards a slice highlights it, letting go of the key picks it,
//! and a click on a slice picks it too (for a pen, with the key held in the
//! other hand). Letting go in the dead centre, a click there, or Escape
//! closes it without a choice.
//!
//! The menu knows only labels and which one is chosen; what they mean is
//! the caller's. While it is open it lies over the whole window and takes
//! every press, so nothing under it (a stroke on the clay, a button) starts.

use std::f32::consts::{FRAC_PI_2, TAU};

use egui::{Color32, Event, Key, Order, Pos2, Rect, Sense, Stroke};

use super::{fonts, theme};

/// Around the centre, a disc where nothing is chosen, in points.
pub const DEAD_ZONE: f32 = 22.0;
/// From the centre to the middle of each label, in points.
const LABEL_RADIUS: f32 = 84.0;
/// Room around the labels inside the ring's edge, in points.
const MARGIN: f32 = 14.0;

/// The slice of a menu of `count` choices that a pointer at `pos` points
/// to, for a menu centred on `centre`; `None` in the dead centre. The first
/// slice is straight up and they go round clockwise, each centred on its
/// direction, so with 4 choices the second is to the right.
pub fn slice_at(centre: Pos2, pos: Pos2, count: usize) -> Option<usize> {
    let d = pos - centre;
    if count == 0 || d.length() < DEAD_ZONE {
        return None;
    }
    let width = TAU / count as f32;
    // Screen y points down, so a growing angle turns clockwise; up is −90°.
    let from_top = d.y.atan2(d.x) + FRAC_PI_2 + width / 2.0;
    Some((from_top.rem_euclid(TAU) / width) as usize % count)
}

/// The direction slice `index` of `count` points to, as a unit vector on
/// the screen.
fn direction(index: usize, count: usize) -> egui::Vec2 {
    let angle = index as f32 / count as f32 * TAU - FRAC_PI_2;
    egui::vec2(angle.cos(), angle.sin())
}

/// What an open menu did this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Still open.
    Open,
    /// The choice at this index was picked; the menu closes.
    Picked(usize),
    /// Closed without a choice.
    Cancelled,
}

/// An open pie menu: where it opened and the key that holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pie {
    pub centre: Pos2,
    key: Key,
}

impl Pie {
    /// Opens a menu on the pointer if `key` was just pressed (not
    /// repeated) with no modifiers while the pointer is in `within`,
    /// taking the press so that nothing else sees it (a focused button
    /// would take Space as a click).
    pub fn open(ui: &egui::Ui, key: Key, within: Rect) -> Option<Pie> {
        ui.input_mut(|i| {
            let centre = i.pointer.latest_pos().filter(|p| within.contains(*p))?;
            let at = i.events.iter().position(|e| {
                matches!(e, Event::Key { key: k, pressed: true, repeat: false, modifiers, .. }
                    if *k == key && modifiers.is_none())
            })?;
            // Its repeats while held, too.
            i.events.remove(at);
            i.events
                .retain(|e| !matches!(e, Event::Key { key: k, .. } if *k == key));
            Some(Pie { centre, key })
        })
    }

    /// The keys: letting go of the menu's key picks the slice under the
    /// pointer, or nothing in the dead centre; Escape closes it. Takes
    /// both keys' events, so they do nothing else while it is open. Call
    /// it early in the frame, before other shortcuts.
    pub fn keys(&self, ui: &egui::Ui, count: usize) -> Outcome {
        ui.input_mut(|i| {
            let escape = i.consume_key(egui::Modifiers::NONE, Key::Escape);
            let released = i
                .events
                .iter()
                .any(|e| matches!(e, Event::Key { key, pressed: false, .. } if *key == self.key));
            i.events
                .retain(|e| !matches!(e, Event::Key { key, .. } if *key == self.key));
            // Losing the window's focus loses the key's release too.
            let held = i.key_down(self.key) && i.focused;
            if escape {
                Outcome::Cancelled
            } else if released || !held {
                match i
                    .pointer
                    .latest_pos()
                    .and_then(|p| slice_at(self.centre, p, count))
                {
                    Some(index) => Outcome::Picked(index),
                    None => Outcome::Cancelled,
                }
            } else {
                Outcome::Open
            }
        })
    }

    /// Draws the menu over everything, with `labels` round it and the one
    /// at `chosen` underlined, and takes presses: one on a slice picks it,
    /// one in the dead centre closes the menu.
    pub fn show(&self, ui: &egui::Ui, labels: &[&str], chosen: Option<usize>) -> Outcome {
        let ctx = ui.ctx();
        let count = labels.len();
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let hovered = pointer.and_then(|p| slice_at(self.centre, p, count));
        let font = fonts::light(16.0);
        let galleys: Vec<_> = labels
            .iter()
            .map(|label| {
                ctx.fonts_mut(|f| {
                    f.layout_no_wrap(label.to_string(), font.clone(), Color32::PLACEHOLDER)
                })
            })
            .collect();
        // The ring reaches just past the farthest label's corner.
        let label_rect = |index: usize, size: egui::Vec2| {
            Rect::from_center_size(self.centre + direction(index, count) * LABEL_RADIUS, size)
        };
        let reach = galleys
            .iter()
            .enumerate()
            .map(|(index, g)| {
                let rect = label_rect(index, g.size());
                [
                    rect.left_top(),
                    rect.right_top(),
                    rect.left_bottom(),
                    rect.right_bottom(),
                ]
                .into_iter()
                .map(|corner| corner.distance(self.centre))
                .fold(0.0, f32::max)
            })
            .fold(LABEL_RADIUS, f32::max)
            + MARGIN;

        let screen = ctx.content_rect();
        let area = egui::Area::new(egui::Id::unique("pie menu"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                // Over the whole window: every press is the menu's.
                let response = ui.allocate_rect(screen, Sense::click_and_drag());
                let painter = ui.painter();
                painter.circle(
                    self.centre,
                    reach,
                    theme::VIEW_BG.gamma_multiply(0.85),
                    Stroke::new(1.0, theme::VIEW_TEXT.gamma_multiply(0.5)),
                );
                painter.circle_stroke(
                    self.centre,
                    DEAD_ZONE,
                    Stroke::new(1.0, theme::VIEW_TEXT.gamma_multiply(0.6)),
                );
                // The way the pointer heads: an arc of the dead zone's
                // edge, the width of the slice it points to.
                if let Some(index) = hovered {
                    let half = TAU / count as f32 / 2.0;
                    let mid = index as f32 / count as f32 * TAU - FRAC_PI_2;
                    let arc = (0..=16)
                        .map(|k| {
                            let a = mid - half + 2.0 * half * k as f32 / 16.0;
                            self.centre + egui::vec2(a.cos(), a.sin()) * DEAD_ZONE
                        })
                        .collect();
                    painter.add(egui::Shape::line(arc, Stroke::new(2.0, theme::VIEW_INK)));
                }
                for (index, galley) in galleys.into_iter().enumerate() {
                    let rect = label_rect(index, galley.size());
                    let lit = hovered == Some(index);
                    let color = if lit || chosen == Some(index) {
                        theme::VIEW_INK
                    } else {
                        theme::VIEW_TEXT
                    };
                    painter.galley(rect.min, galley, color);
                    if chosen == Some(index) {
                        painter.hline(rect.x_range(), rect.bottom() + 1.5, Stroke::new(1.0, color));
                    }
                }
                response
            });
        ctx.set_cursor_icon(egui::CursorIcon::Default);
        let pressed = ctx.input(|i| i.pointer.any_pressed());
        if pressed && area.inner.contains_pointer() {
            match hovered {
                Some(index) => Outcome::Picked(index),
                None => Outcome::Cancelled,
            }
        } else {
            Outcome::Open
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: Pos2 = Pos2::new(400.0, 300.0);

    fn at(dx: f32, dy: f32) -> Pos2 {
        C + egui::vec2(dx, dy)
    }

    #[test]
    fn the_dead_centre_chooses_nothing() {
        assert_eq!(slice_at(C, C, 7), None);
        assert_eq!(slice_at(C, at(DEAD_ZONE - 1.0, 0.0), 7), None);
        assert_eq!(slice_at(C, at(0.0, -(DEAD_ZONE - 1.0)), 7), None);
        assert!(slice_at(C, at(DEAD_ZONE + 1.0, 0.0), 7).is_some());
        assert_eq!(slice_at(C, at(100.0, 0.0), 0), None);
    }

    #[test]
    fn slices_go_round_clockwise_from_the_top() {
        // Four: up, right, down, left.
        assert_eq!(slice_at(C, at(0.0, -50.0), 4), Some(0));
        assert_eq!(slice_at(C, at(50.0, 0.0), 4), Some(1));
        assert_eq!(slice_at(C, at(0.0, 50.0), 4), Some(2));
        assert_eq!(slice_at(C, at(-50.0, 0.0), 4), Some(3));
        // Each slice is centred on its direction: just either side of
        // straight up is still the first.
        assert_eq!(slice_at(C, at(-30.0, -50.0), 4), Some(0));
        assert_eq!(slice_at(C, at(30.0, -50.0), 4), Some(0));
        // Far away counts the same as near: only the direction matters.
        assert_eq!(slice_at(C, at(5000.0, 10.0), 4), Some(1));
    }

    #[test]
    fn seven_slices_meet_where_expected() {
        let width = TAU / 7.0;
        for index in 0..7 {
            // The middle of each slice, and just inside both of its edges.
            for offset in [0.0, -0.49 * width, 0.49 * width] {
                let a = index as f32 * width - FRAC_PI_2 + offset;
                let pos = C + egui::vec2(a.cos(), a.sin()) * 80.0;
                assert_eq!(slice_at(C, pos, 7), Some(index), "{index} {offset}");
            }
            // The labels sit in their own slices.
            let label = C + direction(index, 7) * LABEL_RADIUS;
            assert_eq!(slice_at(C, label, 7), Some(index));
        }
        // Just past the first slice's left edge is the last slice.
        let a = -FRAC_PI_2 - 0.51 * width;
        let pos = C + egui::vec2(a.cos(), a.sin()) * 80.0;
        assert_eq!(slice_at(C, pos, 7), Some(6));
    }
}
