// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The guides drawn over a view, the same in the app and in a screenshot:
//! the grid and the plumb line; and how a length is marked over a view,
//! by the measure and the calipers.

use egui::{Painter, Pos2, Rect, Stroke, Vec2, pos2};

use super::theme;

/// How high the plumb line's bob hangs over the view's bottom edge, in
/// points.
pub const BOB_LIFT: f32 = 32.0;
const BOB_RADIUS: f32 = 3.5;

/// Where the bob of a plumb line hanging at `share` of `rect`'s width is.
pub fn bob(rect: Rect, share: f32) -> Pos2 {
    pos2(
        (rect.left() + share * rect.width()).round() + 0.5,
        rect.bottom() - BOB_LIFT,
    )
}

/// The plumb line at `share` of `rect`'s width, with its bob; returns
/// where the bob is.
pub fn plumb_line(painter: &Painter, rect: Rect, share: f32) -> Pos2 {
    let bob = bob(rect, share);
    painter.vline(bob.x, rect.y_range(), Stroke::new(1.0, theme::GUIDE));
    painter.circle_filled(bob, BOB_RADIUS, theme::GUIDE);
    bob
}

/// `divisions` rows of square cells, with the columns laid out from the
/// view's centre so a line or a cell's middle falls on it, as on the rows.
pub fn grid(painter: &Painter, rect: Rect, divisions: u32) {
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

/// Half the length of the tick across each end of a marked length, in
/// points.
pub const TICK: f32 = 5.0;

/// The unit vector square to the line from `a` to `b`: the way its end
/// ticks run, and its label sits.
pub fn across([a, b]: [Pos2; 2]) -> Vec2 {
    (b - a).normalized().rot90()
}

/// A length marked as on a ruler: the line from `a` to `b`, and a short
/// tick across each end.
pub fn ruler(painter: &Painter, ends: [Pos2; 2], stroke: Stroke) {
    painter.line_segment(ends, stroke);
    let tick = across(ends) * TICK;
    for end in ends {
        painter.line_segment([end - tick, end + tick], stroke);
    }
}

/// Where a marked length's label goes: beside the middle, `gap` points
/// out, on the side away from `centre` (the view's).
pub fn label_at(ends: [Pos2; 2], centre: Pos2, gap: f32) -> Pos2 {
    let [a, b] = ends;
    let middle = a + (b - a) / 2.0;
    let across = across(ends);
    let side = if across.dot(middle - centre) >= 0.0 {
        across
    } else {
        -across
    };
    middle + side * gap
}
