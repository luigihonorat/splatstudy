// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Flyouts: a column of choices, each an icon and a name, that opens
//! beside a toolbar button. A click on a row picks it and closes the
//! column; a press anywhere else closes it without a choice. The button
//! itself opens and closes it, and the caller closes it on Escape.
//!
//! The flyout knows only rows and which one is chosen; what they mean is
//! the caller's. It lies on the same layer as the toolbar and is drawn
//! after it, so its rows take the pointer before the view beneath.

use egui::{Align2, Color32, CursorIcon, Pos2, Rect, Sense, Stroke, pos2, vec2};

use super::icons::Icon;
use super::{fonts, theme};

/// One choice: its icon and its name.
pub struct Row<'a> {
    pub icon: Icon<'a>,
    pub label: &'a str,
}

/// Room around the rows inside the panel, between the toolbar and the
/// panel, after the name, and kept clear of the edges of the area the
/// flyout must stay in, in points.
const PAD: f32 = 2.0;
const GAP: f32 = 4.0;
const TEXT_PAD: f32 = 14.0;
const INSET: f32 = 8.0;
/// The names' size.
const TEXT: f32 = 14.0;

/// What an open flyout did this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Still open.
    Open,
    /// The row at this index was picked; the flyout closes.
    Picked(usize),
    /// Closed without a choice: a press elsewhere.
    Closed,
}

/// Where a flyout of `count` rows, each `width` wide, sits beside the
/// toolbar `button` that opened it: just past the toolbar's edge, towards
/// `right` or the left, its first row level with the button, and moved up
/// as far as it must to stay inside `within` (but never above its top).
/// Gives the panel and each row. A row is as tall as the button, its icon
/// in a square as large before its name.
pub fn layout(
    button: Rect,
    right: bool,
    within: Rect,
    count: usize,
    width: f32,
) -> (Rect, Vec<Rect>) {
    let row = button.height();
    let size = vec2(width + 2.0 * PAD, count as f32 * row + 2.0 * PAD);
    let x = if right {
        button.right() + PAD + GAP
    } else {
        button.left() - PAD - GAP - size.x
    };
    let y = (button.top() - PAD)
        .min(within.bottom() - INSET - size.y)
        .max(within.top() + INSET);
    let panel = Rect::from_min_size(pos2(x, y).round(), size);
    let rows = (0..count)
        .map(|k| {
            let at = panel.min + vec2(PAD, PAD + k as f32 * row);
            Rect::from_min_size(at, vec2(width, row))
        })
        .collect();
    (panel, rows)
}

/// A flyout to show: where, its rows, and the one in use.
pub struct Flyout<'a> {
    pub id: egui::Id,
    /// The button that opened it, and whether the flyout opens to its
    /// right.
    pub button: Rect,
    pub right: bool,
    /// The area it stays inside.
    pub within: Rect,
    pub rows: &'a [Row<'a>],
    /// The row underlined, as the one in use.
    pub chosen: Option<usize>,
}

impl Flyout<'_> {
    /// The panel and its rows, as [`Flyout::show`] lays them out.
    pub fn layout(&self, ctx: &egui::Context) -> (Rect, Vec<Rect>) {
        let widest = self
            .rows
            .iter()
            .map(|row| {
                ctx.fonts_mut(|f| {
                    f.layout_no_wrap(
                        row.label.to_owned(),
                        fonts::light(TEXT),
                        Color32::PLACEHOLDER,
                    )
                    .size()
                    .x
                })
            })
            .fold(0.0, f32::max);
        let width = self.button.height() + widest + TEXT_PAD;
        layout(
            self.button,
            self.right,
            self.within,
            self.rows.len(),
            width.ceil(),
        )
    }

    /// Draws the flyout and takes its clicks: a click on a row picks it.
    /// A press outside both the flyout and its button closes it; one on
    /// the button is the caller's, to close it on its click.
    pub fn show(&self, ui: &egui::Ui) -> Outcome {
        let ctx = ui.ctx();
        let (panel, rows) = self.layout(ctx);
        let painter = ui.painter().with_clip_rect(ctx.content_rect());
        // The gaps round the rows, too, so a press there is the flyout's.
        ui.interact(panel, self.id.with("panel"), Sense::click_and_drag());
        painter.rect_filled(panel, 0.0, theme::TOOLBAR_BG);
        let mut outcome = Outcome::Open;
        for (k, (row, rect)) in self.rows.iter().zip(rows).enumerate() {
            let response = ui
                .interact(rect, self.id.with(k), Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand);
            let hovered = response.hovered();
            let chosen = self.chosen == Some(k);
            if hovered {
                painter.rect_filled(rect, 0.0, theme::TOOL_HOVER);
            }
            let colour = if chosen || hovered {
                theme::TOOL_ON
            } else {
                theme::TOOL_OFF
            };
            let square = Rect::from_min_size(rect.min, vec2(rect.height(), rect.height()));
            (row.icon)(&painter, square, colour);
            let text = painter.text(
                pos2(square.right(), rect.center().y),
                Align2::LEFT_CENTER,
                row.label,
                fonts::light(TEXT),
                colour,
            );
            if chosen {
                let y = (text.bottom() + 1.0).round() + 0.5;
                painter.hline(text.x_range(), y, Stroke::new(1.0, colour));
            }
            if response.clicked() {
                outcome = Outcome::Picked(k);
            }
        }
        let pressed_at = ui.input(|i| {
            i.pointer
                .any_pressed()
                .then(|| i.pointer.press_origin())
                .flatten()
        });
        if outcome == Outcome::Open
            && pressed_at.is_some_and(|p: Pos2| !panel.contains(p) && !self.button.contains(p))
        {
            outcome = Outcome::Closed;
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button() -> Rect {
        Rect::from_min_size(pos2(210.0, 300.0), vec2(30.0, 30.0))
    }

    fn view() -> Rect {
        Rect::from_min_size(pos2(200.0, 30.0), vec2(600.0, 800.0))
    }

    #[test]
    fn it_opens_beside_the_button_towards_either_side() {
        let (panel, rows) = layout(button(), true, view(), 7, 120.0);
        assert_eq!(panel.left(), button().right() + PAD + GAP);
        assert_eq!(rows[0].top(), button().top(), "level with the button");
        assert_eq!(rows.len(), 7);
        for pair in rows.windows(2) {
            assert_eq!(pair[1].top(), pair[0].bottom());
        }
        assert!(rows.iter().all(|r| panel.contains_rect(*r)));
        let (panel, _) = layout(button(), false, view(), 7, 120.0);
        assert_eq!(panel.right(), button().left() - PAD - GAP);
    }

    #[test]
    fn it_moves_up_to_stay_inside() {
        // A button near the bottom of a short view.
        let short = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 260.0));
        let low = Rect::from_min_size(pos2(10.0, 200.0), vec2(22.0, 22.0));
        let (panel, rows) = layout(low, true, short, 7, 100.0);
        assert!(panel.bottom() <= short.bottom() - INSET);
        assert_eq!(rows[0].height(), 22.0, "rows as tall as the button");
        // Never above the top, even if it is too tall to fit.
        let tiny = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 100.0));
        let (panel, _) = layout(low, true, tiny, 7, 100.0);
        assert_eq!(panel.top(), tiny.top() + INSET);
    }
}
