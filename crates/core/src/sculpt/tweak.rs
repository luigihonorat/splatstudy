// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Tweaks: one step of an operation over the whole clay at once, from the
//! panel's tweak group. Smooth and sharpen work on the surface's fine
//! grain; shrink and inflate move the whole surface in or out.
//!
//! Each is plain arithmetic over the vertices in index order, every new
//! position worked out from the old ones before any moves: the result
//! doesn't depend on the order, so a symmetric clay stays symmetric, and a
//! replay of the log (see `record.rs`) gets the same bits. The mask holds
//! the clay back as it does under the brushes. A tweak works on the clay's
//! current subdivision level, like a stroke: leaving the level carries it
//! to the others (see `levels.rs`).

use glam::Vec3;

use super::mesh::Mesh;
use super::{Event, Sculpt};

/// One of the tweaks. Its number is its byte in the log: never renumber.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Tweak {
    /// Taubin's smoothing: a smoothing pass, then a slightly stronger
    /// pass the other way, so the form keeps its volume.
    Smooth = 0,
    /// The opposite of smoothing: each vertex moves away from its
    /// neighbours' average, along its normal.
    Sharpen = 1,
    /// Every vertex moves inwards along its normal.
    Shrink = 2,
    /// Every vertex moves outwards along its normal.
    Inflate = 3,
}

impl Tweak {
    /// By their log byte.
    pub const ALL: [Tweak; 4] = [Tweak::Smooth, Tweak::Sharpen, Tweak::Shrink, Tweak::Inflate];
}

/// Taubin's λ: the smoothing pass moves each vertex this share of the way
/// to its neighbours' average.
const LAMBDA: f32 = 0.5;
/// Taubin's μ: the pass back, a little stronger than λ so that what a
/// form would lose to smoothing comes back (pass-band about 0.11).
const MU: f32 = -0.53;
/// Sharpen moves each vertex this share of its height above its
/// neighbours' average further, along its normal.
const SHARPEN: f32 = 0.4;
/// Shrink and inflate move the surface by this share of the clay's
/// largest side: a hundredth of the radius of a sphere.
pub const SWELL: f32 = 0.005;

/// Where every vertex of `mesh` goes in one step of `op`, held back by
/// the mask. Vertices on an open border stay under smooth and sharpen,
/// which would otherwise pull the border in.
pub fn tweaked(mesh: &Mesh, op: Tweak) -> Vec<Vec3> {
    let n = mesh.positions.len();
    let mask = &mesh.layers.mask;
    let free = |v: usize| 1.0 - mask.get(v);
    match op {
        Tweak::Smooth => {
            let free: Vec<f32> = (0..n)
                .map(|v| if inside(mesh, v) { free(v) } else { 0.0 })
                .collect();
            let once = relax(mesh, &mesh.positions, LAMBDA, &free);
            relax(mesh, &once, MU, &free)
        }
        Tweak::Sharpen => (0..n)
            .map(|v| {
                let p = mesh.positions[v];
                let w = free(v);
                if w <= 0.0 || !inside(mesh, v) {
                    return p;
                }
                let Some(average) = average(mesh, &mesh.positions, v) else {
                    return p;
                };
                let normal = mesh.normals[v];
                let height = (p - average).dot(normal);
                p + normal * (height * SHARPEN * w)
            })
            .collect(),
        Tweak::Shrink | Tweak::Inflate => {
            let sign = if op == Tweak::Shrink { -1.0 } else { 1.0 };
            let step = sign * SWELL * largest_side(&mesh.positions);
            (0..n)
                .map(|v| {
                    let p = mesh.positions[v];
                    let w = free(v);
                    if w <= 0.0 {
                        return p;
                    }
                    p + mesh.normals[v] * (step * w)
                })
                .collect()
        }
    }
}

/// One Laplacian pass over `from`: each vertex `factor` of the way to its
/// neighbours' average, times its `free`.
fn relax(mesh: &Mesh, from: &[Vec3], factor: f32, free: &[f32]) -> Vec<Vec3> {
    (0..from.len())
        .map(|v| {
            let p = from[v];
            let w = free[v] * factor;
            match average(mesh, from, v) {
                Some(average) if w != 0.0 => p + (average - p) * w,
                _ => p,
            }
        })
        .collect()
}

/// The average of vertex `v`'s neighbours in `positions`; `None` for a
/// vertex without any.
fn average(mesh: &Mesh, positions: &[Vec3], v: usize) -> Option<Vec3> {
    let ring = mesh.neighbours.of(v as u32);
    if ring.is_empty() {
        return None;
    }
    let sum: Vec3 = ring.iter().map(|&u| positions[u as usize]).sum();
    Some(sum / ring.len() as f32)
}

/// Whether vertex `v` is inside the surface rather than on an open border
/// (or where it isn't a simple fan): it has as many neighbours as
/// triangles.
fn inside(mesh: &Mesh, v: usize) -> bool {
    let v = v as u32;
    mesh.neighbours.of(v).len() == mesh.vertex_faces.of(v).len()
}

/// The largest side of the box around `positions`.
fn largest_side(positions: &[Vec3]) -> f32 {
    let (min, max) = positions.iter().fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(min, max), &p| (min.min(p), max.max(p)),
    );
    (max - min).max_element().max(0.0)
}

impl Sculpt {
    /// One step of `op` over the whole clay (see [`Tweak`]): one undo
    /// step, logged as such. A stroke still open is closed first. False,
    /// with nothing done or logged, if it would move nothing (the clay
    /// all masked).
    pub fn tweak(&mut self, op: Tweak) -> bool {
        if self.stroke_open {
            self.end_stroke();
        }
        let targets = tweaked(&self.mesh, op);
        let indices: Vec<u32> = (0..targets.len() as u32)
            .filter(|&v| targets[v as usize] != self.mesh.positions[v as usize])
            .collect();
        if indices.is_empty() {
            return false;
        }
        // Before anything moves: a reopened sculpt may log a checkpoint of
        // the clay as it is first.
        self.record(Event::Tweak(op));
        let before = indices
            .iter()
            .map(|&v| self.mesh.positions[v as usize])
            .collect();
        let layers = indices.iter().map(|&v| self.layers_at(v)).collect();
        let after: Vec<Vec3> = indices.iter().map(|&v| targets[v as usize]).collect();
        self.apply_positions(&indices, &after);
        self.push_edit(indices, before, layers);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{Reader, Writer};
    use crate::sculpt::record::{read_log, write_log};
    use crate::sculpt::{Brush, Dab, Replay};

    /// A little hash of `i`, from -1 to 1.
    fn noise(i: usize) -> f32 {
        let x = (i as u32).wrapping_mul(2_654_435_761) ^ 0x9e37_79b9;
        let x = x.wrapping_mul(0x85eb_ca6b) ^ (x >> 13);
        (x % 20_001) as f32 / 10_000.0 - 1.0
    }

    /// A unit sphere with every vertex moved in or out by up to 2%.
    fn noisy_sphere() -> Mesh {
        let sphere = Mesh::icosphere(4);
        let positions = sphere
            .positions
            .iter()
            .enumerate()
            .map(|(i, &p)| p * (1.0 + 0.02 * noise(i)))
            .collect();
        Mesh::new(positions, sphere.triangles)
    }

    /// A sphere bumped the same on both sides of the X = 0 plane.
    fn symmetric_lumpy_sphere() -> Mesh {
        let sphere = Mesh::icosphere(4);
        let positions = sphere
            .positions
            .iter()
            .map(|&p| p * (1.0 + 0.05 * (6.0 * p.x.abs()).sin() * (4.0 * p.y + p.z).cos()))
            .collect();
        Mesh::new(positions, sphere.triangles)
    }

    fn volume(mesh: &Mesh) -> f32 {
        mesh.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|v| mesh.positions[v as usize]);
                a.dot(b.cross(c)) / 6.0
            })
            .sum()
    }

    /// How far, on average, each vertex is from its neighbours' average.
    fn roughness(mesh: &Mesh) -> f32 {
        let n = mesh.positions.len();
        (0..n)
            .map(|v| {
                average(mesh, &mesh.positions, v)
                    .unwrap()
                    .distance(mesh.positions[v])
            })
            .sum::<f32>()
            / n as f32
    }

    fn mean_radius(mesh: &Mesh) -> f32 {
        mesh.positions.iter().map(|p| p.length()).sum::<f32>() / mesh.positions.len() as f32
    }

    #[test]
    fn smoothing_keeps_the_volume_and_takes_the_roughness_off() {
        let mut s = Sculpt::new(noisy_sphere());
        let (volume_before, rough_before) = (volume(&s.mesh), roughness(&s.mesh));
        assert!(s.tweak(Tweak::Smooth));
        let change = volume(&s.mesh) / volume_before - 1.0;
        assert!(change.abs() < 0.01, "volume changed by {change}");
        assert!(roughness(&s.mesh) < rough_before * 0.8);
        // Several clicks still hold the volume, and go on smoothing.
        for _ in 0..4 {
            s.tweak(Tweak::Smooth);
        }
        let change = volume(&s.mesh) / volume_before - 1.0;
        assert!(change.abs() < 0.01, "volume changed by {change}");
        assert!(roughness(&s.mesh) < rough_before * 0.5);
    }

    #[test]
    fn sharpening_adds_roughness() {
        let mut s = Sculpt::new(noisy_sphere());
        let (volume_before, rough_before) = (volume(&s.mesh), roughness(&s.mesh));
        assert!(s.tweak(Tweak::Sharpen));
        let rough = roughness(&s.mesh);
        assert!(rough > rough_before * 1.1, "{rough} vs {rough_before}");
        assert!(
            rough < rough_before * 2.0,
            "gentle: {rough} vs {rough_before}"
        );
        let change = volume(&s.mesh) / volume_before - 1.0;
        assert!(change.abs() < 0.01, "volume changed by {change}");
    }

    #[test]
    fn shrink_and_inflate_move_the_sphere_by_one_step() {
        let mut s = Sculpt::new(Mesh::icosphere(4));
        let triangles = s.mesh.triangles.clone();
        let step = SWELL * largest_side(&s.mesh.positions);
        assert!((step - 0.01).abs() < 1e-4, "a hundredth of the radius");
        let radius = mean_radius(&s.mesh);
        assert!(s.tweak(Tweak::Shrink));
        assert!((mean_radius(&s.mesh) - (radius - step)).abs() < 1e-4);
        assert!(s.tweak(Tweak::Inflate));
        assert!(s.tweak(Tweak::Inflate));
        // The sizes inflate measures from grew a little with the first.
        assert!((mean_radius(&s.mesh) - (radius + step)).abs() < 2e-4);
        // Still the same closed surface, every vertex out where it was.
        assert_eq!(s.mesh.triangles, triangles);
        assert!(
            s.mesh
                .positions
                .iter()
                .all(|p| (p.length() - 1.01).abs() < 2e-3)
        );
        assert!(volume(&s.mesh) > 0.0);
    }

    #[test]
    fn masked_clay_stays_put_under_every_tweak() {
        for op in Tweak::ALL {
            let mut s = Sculpt::new(noisy_sphere());
            let n = s.mesh.positions.len();
            // The +X half frozen, a band left of it half free.
            for v in 0..n {
                let x = s.mesh.positions[v].x;
                let mask = if x > 0.0 {
                    1.0
                } else if x > -0.3 {
                    0.5
                } else {
                    0.0
                };
                s.mesh.layers.mask.set(v, mask, n);
            }
            let before = s.mesh.positions.clone();
            assert!(s.tweak(op), "{op:?}");
            for (v, (&now, &was)) in s.mesh.positions.iter().zip(&before).enumerate() {
                if was.x > 0.0 {
                    assert_eq!(now, was, "{op:?} moved masked vertex {v}");
                }
            }
            assert!(
                (0..n).any(|v| before[v].x < -0.3 && s.mesh.positions[v] != before[v]),
                "{op:?} moved nothing"
            );
            // All masked: nothing to do, and nothing logged.
            for v in 0..n {
                s.mesh.layers.mask.set(v, 1.0, n);
            }
            let log = s.log().len();
            assert!(!s.tweak(op));
            assert_eq!(s.log().len(), log);
        }
    }

    #[test]
    fn a_symmetric_clay_stays_symmetric_and_one_undo_restores_it() {
        let mesh = symmetric_lumpy_sphere();
        // Each vertex's mirror twin across X = 0.
        let twins: Vec<usize> = mesh
            .positions
            .iter()
            .map(|p| {
                let m = Vec3::new(-p.x, p.y, p.z);
                (0..mesh.positions.len())
                    .min_by(|&a, &b| {
                        let (a, b) = (mesh.positions[a], mesh.positions[b]);
                        a.distance(m).total_cmp(&b.distance(m))
                    })
                    .unwrap()
            })
            .collect();
        for (v, &t) in twins.iter().enumerate() {
            let (p, q) = (mesh.positions[v], mesh.positions[t]);
            assert!(
                Vec3::new(-p.x, p.y, p.z).distance(q) < 1e-5,
                "a twin for {v}"
            );
        }
        for op in Tweak::ALL {
            let mut s = Sculpt::new(mesh.clone());
            let before = s.mesh.positions.clone();
            for _ in 0..3 {
                assert!(s.tweak(op));
            }
            for (v, &t) in twins.iter().enumerate() {
                let (p, q) = (s.mesh.positions[v], s.mesh.positions[t]);
                let off = Vec3::new(-p.x, p.y, p.z).distance(q);
                assert!(off < 1e-5, "{op:?}: vertex {v} is {off} off its twin");
            }
            // One click, one undo step; redo puts it back.
            let third = s.mesh.positions.clone();
            s.undo();
            assert_ne!(s.mesh.positions, third);
            s.redo();
            assert_eq!(s.mesh.positions, third);
            s.undo();
            s.undo();
            s.undo();
            assert_eq!(s.mesh.positions, before, "{op:?} did not undo");
        }
    }

    #[test]
    fn a_tweak_ends_the_stroke_under_way_first() {
        let mut s = Sculpt::new(Mesh::icosphere(4));
        let start = s.mesh.positions.clone();
        s.begin_stroke();
        s.dab(dab(Vec3::Z));
        let stroked = s.mesh.positions.clone();
        assert!(s.tweak(Tweak::Inflate));
        s.undo();
        assert_eq!(s.mesh.positions, stroked, "the tweak alone undone");
        s.undo();
        assert_eq!(s.mesh.positions, start, "then the stroke");
    }

    fn dab(centre: Vec3) -> Dab {
        Dab {
            brush: Brush::Clay,
            centre,
            radius: 0.3,
            strength: 0.5,
            invert: false,
            symmetry: true,
            direction: Vec3::ZERO,
            buildup: Default::default(),
            fill: false,
            wipes: true,
        }
    }

    fn stroke(s: &mut Sculpt, clock: &mut f64, brush: Brush, at: Vec3) {
        *clock += 1.0;
        s.set_clock(*clock);
        s.begin_stroke();
        for i in 0..5 {
            s.dab(Dab {
                direction: if i == 0 { Vec3::ZERO } else { Vec3::Y },
                brush,
                ..dab((at + Vec3::Y * 0.04 * i as f32).normalize())
            });
            s.refresh();
        }
        s.end_stroke();
        s.refresh();
    }

    /// Strokes and every tweak, undone and redone, on two levels and
    /// partly masked.
    fn session(s: &mut Sculpt, clock: &mut f64) {
        stroke(s, clock, Brush::Clay, Vec3::new(0.3, 0.0, 1.0));
        for op in Tweak::ALL {
            *clock += 1.0;
            s.set_clock(*clock);
            assert!(s.tweak(op));
            s.refresh();
        }
        s.undo();
        s.undo();
        s.redo();
        stroke(s, clock, Brush::Clay, Vec3::new(-0.4, 0.2, 1.0));
        stroke(s, clock, Brush::Mask, Vec3::new(0.1, -0.1, 1.0));
        assert!(!s.mesh.layers.mask.is_empty());
        assert!(s.tweak(Tweak::Smooth));
        assert!(s.tweak(Tweak::Inflate));
        s.refresh();
    }

    fn assert_replays(live: &Sculpt) {
        let mut replay = Replay::of(live);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_eq!(replay.sculpt.mesh.triangles, live.mesh.triangles);
        assert_eq!(replay.sculpt.mesh.positions, live.mesh.positions);
        assert_eq!(replay.sculpt.mesh.normals, live.mesh.normals);
    }

    #[test]
    fn a_session_of_tweaks_replays_bit_for_bit() {
        let mut live = Sculpt::new(Mesh::icosphere(3));
        let mut clock = 0.0;
        session(&mut live, &mut clock);
        assert!(
            live.log()
                .iter()
                .any(|e| e.event == Event::Tweak(Tweak::Sharpen))
        );
        assert_replays(&live);

        // The log saves and reads back, tweaks and all.
        let mut w = Writer::new();
        write_log(&mut w, live.log());
        let log = read_log(&mut Reader::new(&w.finish())).unwrap();
        assert_eq!(log, live.log());

        // Reopened and tweaked on, it still replays.
        let mut current =
            Mesh::from_ordered(live.mesh.positions.clone(), live.mesh.triangles.clone());
        current.layers = live.mesh.layers.clone();
        let mut reopened = Sculpt::with_history(current, None, log, live.checkpoints().to_vec());
        clock += 10.0;
        reopened.set_clock(clock);
        assert!(reopened.tweak(Tweak::Inflate));
        reopened.refresh();
        assert_replays(&reopened);

        // Up a level, tweaked there, down, tweaked, up again.
        clock += 1.0;
        reopened.set_clock(clock);
        assert!(reopened.subdivide());
        assert!(reopened.tweak(Tweak::Sharpen));
        reopened.refresh();
        assert!(reopened.change_level(false));
        assert!(reopened.tweak(Tweak::Shrink));
        assert!(reopened.change_level(true));
        assert!(reopened.tweak(Tweak::Smooth));
        reopened.refresh();
        assert_replays(&reopened);
    }

    #[test]
    fn tweaks_take_a_tag_of_their_own_and_older_logs_still_read() {
        // A log from before tweaks, written byte by byte: a mirror and a
        // layer edit, the last tags there were then.
        let mut w = Writer::new();
        w.len(3);
        w.f64(1.0).u8(9).u8(1);
        w.f64(2.0).u8(15).u8(0);
        w.f64(3.0).u8(7);
        let old = read_log(&mut Reader::new(&w.finish())).unwrap();
        let events: Vec<Event> = old.iter().map(|e| e.event).collect();
        assert_eq!(
            events,
            [
                Event::Mirror { to_negative: true },
                Event::Layer(crate::sculpt::LayerOp::ALL[0]),
                Event::Refresh,
            ]
        );

        // Each tweak is tag 16 and its own byte, and reads back.
        for op in Tweak::ALL {
            let entry = crate::sculpt::Entry {
                time: 4.0,
                event: Event::Tweak(op),
            };
            let mut w = Writer::new();
            entry.write(&mut w);
            let bytes = w.finish();
            assert_eq!(bytes[8..], [16, op as u8]);
            let read = crate::sculpt::Entry::read(&mut Reader::new(&bytes)).unwrap();
            assert_eq!(read, entry);
        }
        // A tweak this version doesn't know is an error, not a panic.
        let mut w = Writer::new();
        w.f64(5.0).u8(16).u8(4);
        assert!(crate::sculpt::Entry::read(&mut Reader::new(&w.finish())).is_err());
    }
}
