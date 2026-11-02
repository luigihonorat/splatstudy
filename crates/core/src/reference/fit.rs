// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Placing a reference in the clay's space: centre it and scale it so what
//! the crop keeps fits a sphere of radius 1. Splat captures carry floaters
//! far from the subject, so the box is taken between the 2nd and 98th
//! percentile on each axis rather than from the extremes.

use glam::Vec3;

use super::{Aabb, Content, Reference};

/// Points looked at for a box; larger sets are sampled evenly.
const SAMPLE: usize = 200_000;
pub const LOW: f32 = 0.02;
pub const HIGH: f32 = 0.98;

/// The box between the `low` and `high` fractions of the points on each
/// axis. Non-finite points are ignored; `None` if nothing is left.
pub fn percentile_box(points: &[Vec3], low: f32, high: f32) -> Option<Aabb> {
    let step = points.len().div_ceil(SAMPLE).max(1);
    let sample: Vec<Vec3> = points
        .iter()
        .step_by(step)
        .copied()
        .filter(|p| p.is_finite())
        .collect();
    if sample.is_empty() {
        return None;
    }
    let last = sample.len() - 1;
    let lo = ((last as f32 * low).floor() as usize).min(last);
    let hi = ((last as f32 * high).ceil() as usize).clamp(lo, last);
    let mut min = Vec3::ZERO;
    let mut max = Vec3::ZERO;
    let mut axis: Vec<f32> = Vec::with_capacity(sample.len());
    for a in 0..3 {
        axis.clear();
        axis.extend(sample.iter().map(|p| p[a]));
        let (_, &mut v, _) = axis.select_nth_unstable_by(lo, f32::total_cmp);
        min[a] = v;
        let (_, &mut v, _) = axis.select_nth_unstable_by(hi, f32::total_cmp);
        max[a] = v;
    }
    Some(Aabb { min, max })
}

impl Aabb {
    /// Halves first, so bounds near the float limit don't overflow.
    pub fn centre(&self) -> Vec3 {
        self.min * 0.5 + self.max * 0.5
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    /// The box grown by `fraction` of its size on every side.
    pub fn grown(&self, fraction: f32) -> Aabb {
        let d = self.size() * fraction;
        Aabb {
            min: self.min - d,
            max: self.max + d,
        }
    }
}

impl Reference {
    /// The points that place the reference: splat centres or mesh vertices,
    /// in file coordinates. Empty for pictures.
    pub fn points(&self) -> &[Vec3] {
        match &self.content {
            Content::Splats(s) => &s.positions,
            Content::Mesh(m) => &m.positions,
            Content::Image(_) => &[],
        }
    }

    /// Centres and scales the reference so the part inside the crop box
    /// fits radius 1, ignoring the outer 2 % on each side of each axis.
    /// Keeps the up axis and the turn.
    pub fn fit(&mut self) {
        let kept: Vec<Vec3>;
        let points = match self.crop {
            Some(crop) => {
                kept = self
                    .points()
                    .iter()
                    .copied()
                    .filter(|&p| crop.contains(p))
                    .collect();
                &kept[..]
            }
            None => self.points(),
        };
        let Some(bounds) = percentile_box(points, LOW, HIGH) else {
            return;
        };
        // Halves first, so bounds near the float limit don't overflow; a
        // centre or size that still isn't finite leaves the file as it is.
        let radius = (bounds.max * 0.5 - bounds.min * 0.5).length();
        let centre = bounds.centre();
        self.align.centre = if centre.is_finite() {
            centre
        } else {
            Vec3::ZERO
        };
        self.align.scale = if radius > 1e-12 && radius.is_finite() {
            1.0 / radius
        } else {
            1.0
        };
    }

    /// The box the crop sliders span, in file coordinates: the fitted box
    /// with room around it, without the floaters far away.
    pub fn crop_range(&self) -> Option<Aabb> {
        let inner = percentile_box(self.points(), LOW, HIGH)?;
        let all = percentile_box(self.points(), 0.0, 1.0)?;
        let grown = inner.grown(0.25);
        Some(Aabb {
            min: grown.min.max(all.min),
            max: grown.max.min(all.max),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::reference::{Source, Splats};

    fn cloud() -> Vec<Vec3> {
        // A 10 × 10 × 10 grid from 0 to 9, centred on 4.5, plus two floaters.
        let mut points: Vec<Vec3> = (0..1000)
            .map(|i| Vec3::new((i % 10) as f32, (i / 10 % 10) as f32, (i / 100) as f32))
            .collect();
        points.push(Vec3::splat(1000.0));
        points.push(Vec3::new(f32::NAN, 0.0, 0.0));
        points
    }

    #[test]
    fn floaters_do_not_move_the_box() {
        let b = percentile_box(&cloud(), LOW, HIGH).unwrap();
        assert_eq!(b.min, Vec3::ZERO);
        assert_eq!(b.max, Vec3::splat(9.0));
        assert!(percentile_box(&[], LOW, HIGH).is_none());
    }

    #[test]
    fn fitting_puts_the_cropped_part_in_the_unit_sphere() {
        let points = cloud();
        let n = points.len();
        let splats = Splats {
            positions: points,
            scales: vec![Vec3::splat(0.1); n],
            rotations: vec![glam::Quat::IDENTITY; n],
            colors: vec![[255; 4]; n],
        };
        let source = Source {
            name: "grid".into(),
            bytes: Arc::from([]),
        };
        let mut r = Reference::new(source, Content::Splats(splats));
        r.fit();
        assert_eq!(r.align.centre, Vec3::splat(4.5));
        let corner = r.align.matrix().transform_point3(Vec3::splat(9.0));
        assert!((corner.length() - 1.0).abs() < 1e-5);

        // Crop to the lower half in y: the centre and scale follow.
        r.crop = Some(Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::new(10.0, 4.0, 10.0),
        });
        r.fit();
        assert_eq!(r.align.centre, Vec3::new(4.5, 2.0, 4.5));
        let range = r.crop_range().unwrap();
        assert!(
            range.max.x < 20.0,
            "the floater stays out of the slider range"
        );
        assert_eq!(range.min.x, 0.0, "nor beyond the points");
    }

    #[test]
    fn bounds_near_the_float_limit_still_fit() {
        let big = 3.0e38;
        let points = vec![Vec3::splat(big), Vec3::splat(big * 0.9), Vec3::splat(big)];
        let n = points.len();
        let splats = Splats {
            positions: points,
            scales: vec![Vec3::splat(0.1); n],
            rotations: vec![glam::Quat::IDENTITY; n],
            colors: vec![[255; 4]; n],
        };
        let source = Source {
            name: "far".into(),
            bytes: Arc::from([]),
        };
        let mut r = Reference::new(source, Content::Splats(splats));
        r.fit();
        assert!(r.align.centre.is_finite(), "{}", r.align.centre);
        assert!(r.align.centre.x > big * 0.8);
        assert!(r.align.scale.is_finite() && r.align.scale > 0.0);
    }
}
