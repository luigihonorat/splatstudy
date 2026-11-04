// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Thin line icons, drawn with the painter: one function per icon, each in
//! a square of [`SIZE`] points. Straight lines sit on half points from a
//! whole-point corner, so a 1-point line covers whole pixels at 1× and 2×.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

/// The side of an icon's square, in points.
pub const SIZE: f32 = 16.0;

/// Draws an icon centred in a rectangle, in a colour: one of the
/// functions here, or a closure picking one's variant.
pub type Icon<'a> = &'a dyn Fn(&Painter, Rect, Color32);

/// Draws in an icon's square, in its own coordinates (0..16, y down).
struct Pen<'a> {
    painter: &'a Painter,
    origin: Pos2,
    colour: Color32,
}

impl<'a> Pen<'a> {
    /// The square of [`SIZE`] at the centre of `rect`, its corner on a
    /// whole point.
    fn new(painter: &'a Painter, rect: Rect, colour: Color32) -> Self {
        let corner = rect.center() - vec2(SIZE, SIZE) / 2.0;
        Self {
            painter,
            origin: pos2(corner.x.round(), corner.y.round()),
            colour,
        }
    }

    fn at(&self, x: f32, y: f32) -> Pos2 {
        self.origin + vec2(x, y)
    }

    fn stroke(&self) -> Stroke {
        Stroke::new(1.0, self.colour)
    }

    fn line(&self, a: (f32, f32), b: (f32, f32)) {
        self.painter
            .line_segment([self.at(a.0, a.1), self.at(b.0, b.1)], self.stroke());
    }

    fn polyline(&self, points: &[(f32, f32)], closed: bool) {
        let points: Vec<Pos2> = points.iter().map(|&(x, y)| self.at(x, y)).collect();
        self.painter.add(if closed {
            Shape::closed_line(points, self.stroke())
        } else {
            Shape::line(points, self.stroke())
        });
    }

    fn circle(&self, c: (f32, f32), r: f32) {
        self.painter
            .circle_stroke(self.at(c.0, c.1), r, self.stroke());
    }

    fn disc(&self, c: (f32, f32), r: f32) {
        self.painter
            .circle_filled(self.at(c.0, c.1), r, self.colour);
    }

    /// An ellipse around `c` with half-axes `rx` and `ry`.
    fn ellipse(&self, c: (f32, f32), rx: f32, ry: f32) {
        let points: Vec<(f32, f32)> = (0..32)
            .map(|i| {
                let a = i as f32 / 32.0 * std::f32::consts::TAU;
                (c.0 + rx * a.cos(), c.1 + ry * a.sin())
            })
            .collect();
        self.polyline(&points, true);
    }

    /// An arc around `c` from angle `from` to `to`, radians, y down.
    fn arc(&self, c: (f32, f32), r: f32, from: f32, to: f32) -> Vec<(f32, f32)> {
        let points: Vec<(f32, f32)> = (0..=24)
            .map(|i| {
                let a = from + (to - from) * i as f32 / 24.0;
                (c.0 + r * a.cos(), c.1 + r * a.sin())
            })
            .collect();
        self.polyline(&points, false);
        points
    }
}

/// A floor in perspective: its sides and lines across close in on the
/// horizon.
pub fn perspective(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    pen.polyline(&[(1.5, 13.5), (5.5, 3.5), (10.5, 3.5), (14.5, 13.5)], true);
    pen.line((8.0, 3.5), (8.0, 13.5));
    pen.line((3.5, 8.5), (12.5, 8.5));
}

/// A wire globe: an outline with an equator and a meridian.
pub fn wireframe(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    pen.circle((8.0, 8.0), 6.5);
    pen.ellipse((8.0, 8.0), 6.5, 2.5);
    pen.ellipse((8.0, 8.0), 2.5, 6.5);
}

/// Where the light comes from, for the lightings in
/// [`crate::render::clay::Lighting`] order: studio (above left), raking
/// (low from the side) and top.
pub fn lighting(painter: &Painter, rect: Rect, colour: Color32, which: usize) {
    let pen = Pen::new(painter, rect, colour);
    let (sun, form) = match which {
        1 => ((2.0, 8.5), (10.0, 9.0)),
        2 => ((8.0, 2.0), (8.0, 10.0)),
        _ => ((3.0, 3.0), (9.5, 9.5)),
    };
    pen.circle(form, 5.0);
    pen.disc(sun, 1.8);
    // The lit side: a short arc of the form facing the light.
    let towards = (sun.1 - form.1).atan2(sun.0 - form.0);
    pen.arc(form, 3.0, towards - 0.9, towards + 0.9);
}

/// A grid of three by three cells.
pub fn grid(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    for v in [1.5, 6.5, 9.5, 14.5] {
        pen.line((v, 1.5), (v, 14.5));
        pen.line((1.5, v), (14.5, v));
    }
}

/// A plumb line: a thread hanging from a hook, its bob at the end.
pub fn plumb(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    pen.line((5.5, 1.5), (10.5, 1.5));
    pen.line((8.5, 1.5), (8.5, 10.0));
    pen.polyline(&[(8.5, 9.5), (11.0, 12.5), (8.5, 15.0), (6.0, 12.5)], true);
}

/// A form filled flat: only its outline counts.
pub fn silhouette(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    pen.disc((8.0, 5.0), 3.5);
    let shoulders: Vec<Pos2> = [
        (2.0, 15.0),
        (3.5, 10.5),
        (8.0, 9.0),
        (12.5, 10.5),
        (14.0, 15.0),
    ]
    .iter()
    .map(|&(x, y)| pen.at(x, y))
    .collect();
    painter.add(Shape::convex_polygon(shoulders, colour, Stroke::NONE));
}

/// The reference's silhouette, filled, and the clay's outline over it,
/// a little off.
pub fn difference(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    painter.circle_filled(pen.at(6.0, 8.0), 4.5, colour.gamma_multiply(0.45));
    pen.circle((10.0, 8.0), 4.5);
}

/// The reference laid over the sculpt: an outline, and a second one
/// faint across it.
pub fn ghost(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    pen.polyline(&[(1.5, 5.5), (10.5, 5.5), (10.5, 14.5), (1.5, 14.5)], true);
    let faint = Pen {
        colour: colour.gamma_multiply(0.5),
        ..Pen::new(painter, rect, colour)
    };
    faint.polyline(
        &[(10.5, 1.5), (14.5, 1.5), (14.5, 10.5), (10.5, 10.5)],
        false,
    );
    faint.polyline(&[(5.5, 5.5), (5.5, 1.5), (10.5, 1.5)], false);
}

/// Two halves facing each other across the mirror's line.
pub fn mirror(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    for y in [1.0, 4.5, 8.0, 11.5] {
        pen.line((8.0, y), (8.0, y + 2.0));
    }
    pen.polyline(&[(6.0, 3.5), (1.5, 8.0), (6.0, 12.5)], true);
    pen.polyline(&[(10.0, 3.5), (14.5, 8.0), (10.0, 12.5)], true);
}

/// The camera link: two links of a chain.
pub fn link(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    for (left, right) in [(1.5, 9.5), (6.5, 14.5)] {
        let link = Rect::from_min_max(pen.at(left, 5.5), pen.at(right, 10.5));
        painter.rect_stroke(link, 2.5, pen.stroke(), egui::StrokeKind::Middle);
    }
}

/// Framing: four corners around a dot.
pub fn frame(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    for (x, y, dx, dy) in [
        (1.5, 1.5, 1.0, 1.0),
        (14.5, 1.5, -1.0, 1.0),
        (1.5, 14.5, 1.0, -1.0),
        (14.5, 14.5, -1.0, -1.0),
    ] {
        pen.polyline(&[(x, y + 4.0 * dy), (x, y), (x + 4.0 * dx, y)], false);
    }
    pen.disc((8.0, 8.0), 1.5);
}

/// Back to the start: a turn of an arrow round to its head.
pub fn reset(painter: &Painter, rect: Rect, colour: Color32) {
    use std::f32::consts::PI;
    let pen = Pen::new(painter, rect, colour);
    let points = pen.arc((8.0, 8.5), 5.5, -0.35 * PI, 1.35 * PI);
    // The head, at the arc's start, pointing along the turn.
    let (x, y) = points[0];
    pen.polyline(&[(x - 3.5, y - 1.0), (x, y), (x - 0.5, y + 3.5)], false);
}

/// The side panels: a window with a column down each side.
pub fn panels(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    pen.polyline(&[(1.5, 2.5), (14.5, 2.5), (14.5, 13.5), (1.5, 13.5)], true);
    pen.line((5.5, 2.5), (5.5, 13.5));
    pen.line((10.5, 2.5), (10.5, 13.5));
}

/// An arrowhead at `tip`, pointing along `(dx, dy)`, one of the four
/// axis directions, its arms reaching `arm` back and `arm` aside.
fn head(pen: &Pen, tip: (f32, f32), (dx, dy): (f32, f32), arm: f32) {
    let (px, py) = (-dy, dx);
    let arm_at = |side: f32| {
        (
            tip.0 + arm * (side * px - dx),
            tip.1 + arm * (side * py - dy),
        )
    };
    pen.polyline(&[arm_at(1.0), tip, arm_at(-1.0)], false);
}

/// Move: a cross with an arrowhead on each arm, every way the clay can go.
pub fn move_tool(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    pen.line((8.5, 1.5), (8.5, 15.5));
    pen.line((1.5, 8.5), (15.5, 8.5));
    for (tip, d) in [
        ((8.5, 1.5), (0.0, -1.0)),
        ((8.5, 15.5), (0.0, 1.0)),
        ((1.5, 8.5), (-1.0, 0.0)),
        ((15.5, 8.5), (1.0, 0.0)),
    ] {
        head(&pen, tip, d, 2.5);
    }
}

/// Clay: a mound built up on the surface, and a plus for the clay added.
pub fn clay_tool(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    pen.line((0.5, 14.5), (15.5, 14.5));
    // The mound: the top of a circle standing on the surface.
    let (c, r): ((f32, f32), f32) = ((8.5, 18.0), 7.5);
    let foot = ((c.1 - 14.5) / r).asin();
    pen.arc(
        c,
        r,
        std::f32::consts::PI + foot,
        std::f32::consts::TAU - foot,
    );
    pen.line((8.5, 1.0), (8.5, 8.0));
    pen.line((5.0, 4.5), (12.0, 4.5));
}

/// Flatten: a dome with its top cut flat, under the plane that cut it.
pub fn flatten_tool(painter: &Painter, rect: Rect, colour: Color32) {
    use std::f32::consts::PI;
    let pen = Pen::new(painter, rect, colour);
    let (c, r): ((f32, f32), f32) = ((8.0, 16.0), 8.0);
    let flat: f32 = 10.5;
    // Where the dome meets the flat top, as an angle from the centre.
    let meet = ((c.1 - flat) / r).asin();
    let side = |from: f32, to: f32| -> Vec<(f32, f32)> {
        (0..=8)
            .map(|i| {
                let a = from + (to - from) * i as f32 / 8.0;
                (c.0 + r * a.cos(), c.1 + r * a.sin())
            })
            .collect()
    };
    // And where it stands on the surface.
    let foot = ((c.1 - 14.5) / r).asin();
    let mut profile = side(PI + foot, PI + meet);
    profile.extend(side(2.0 * PI - meet, 2.0 * PI - foot));
    pen.polyline(&profile, false);
    pen.line((0.5, 14.5), (15.5, 14.5));
    pen.line((0.5, 6.5), (15.5, 6.5));
    pen.line((0.5, 7.5), (15.5, 7.5));
}

/// Smooth: a wave that dies down to a level line.
pub fn smooth_tool(painter: &Painter, rect: Rect, colour: Color32) {
    use std::f32::consts::TAU;
    let pen = Pen::new(painter, rect, colour);
    let points: Vec<(f32, f32)> = (0..=40)
        .map(|i| {
            let t = i as f32 / 40.0;
            let amplitude = 4.5 * (1.0 - t).powf(1.5);
            (0.5 + 15.0 * t, 8.5 - amplitude * (t * 2.25 * TAU).sin())
        })
        .collect();
    pen.polyline(&points, false);
}

/// Inflate: a balloon, blown up, its knot below.
pub fn inflate_tool(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    pen.circle((8.0, 6.5), 5.5);
    pen.polyline(&[(8.0, 12.0), (6.5, 13.5), (9.5, 13.5)], true);
    pen.polyline(&[(8.0, 13.5), (7.0, 15.0), (8.5, 15.5)], false);
}

/// Crease: a sharp V cut into a level surface.
pub fn crease_tool(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    pen.polyline(
        &[
            (0.5, 5.5),
            (4.5, 5.5),
            (8.0, 13.5),
            (11.5, 5.5),
            (15.5, 5.5),
        ],
        false,
    );
}

/// Pinch: two arrows meeting at a ridge, pulling the clay together.
pub fn pinch_tool(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    pen.line((8.5, 2.5), (8.5, 14.5));
    pen.line((0.5, 8.5), (6.0, 8.5));
    head(&pen, (6.0, 8.5), (1.0, 0.0), 2.5);
    pen.line((11.0, 8.5), (15.5, 8.5));
    head(&pen, (11.0, 8.5), (-1.0, 0.0), 2.5);
}

/// Mask: a round of clay, half of it hatched, held still.
pub fn mask_tool(painter: &Painter, rect: Rect, colour: Color32) {
    let pen = Pen::new(painter, rect, colour);
    let (c, r) = ((8.0, 8.0), 6.5);
    pen.circle(c, r);
    pen.line((8.0, c.1 - r), (8.0, c.1 + r));
    // Diagonals across the left half, each from the circle to the middle
    // line or the circle again.
    let s = std::f32::consts::FRAC_1_SQRT_2;
    for k in [-6.0f32, -3.0, 0.0, 3.0] {
        let reach = (r * r - k * k).max(0.0).sqrt();
        // Along (1, -1) from the point `k` along (1, 1) off the centre.
        let to = reach.min(-k);
        if to <= -reach {
            continue;
        }
        let at = |t: f32| (c.0 + (k + t) * s, c.1 + (k - t) * s);
        pen.line(at(-reach), at(to));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_square_starts_on_a_whole_point() {
        let ctx = egui::Context::default();
        let painter = Painter::new(ctx, egui::LayerId::background(), Rect::EVERYTHING);
        let rect = Rect::from_min_size(pos2(10.3, 20.7), vec2(28.0, 28.0));
        let pen = Pen::new(&painter, rect, Color32::WHITE);
        assert_eq!(pen.origin, pos2(16.0, 27.0));
        // The square sits at the centre, within a point.
        let centre = pen.origin + vec2(SIZE, SIZE) / 2.0;
        assert!((centre - rect.center()).length() < 1.0);
    }
}
