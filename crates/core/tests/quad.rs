// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! A quad remesh in the middle of a session: undo brings the clay back,
//! and the timelapse replays through it bit for bit.

use glam::Vec3;
use splatstudy_core::quad;
use splatstudy_core::sculpt::{Brush, Dab, Mesh, Replay, Sculpt};

fn strokes(s: &mut Sculpt, clock: f64, at: Vec3) -> f64 {
    let mut clock = clock;
    for brush in [Brush::Clay, Brush::Inflate, Brush::Smooth] {
        s.begin_stroke();
        for i in 0..6 {
            clock += 0.05;
            s.set_clock(clock);
            s.dab(Dab {
                brush,
                centre: (at + Vec3::Y * (i as f32 * 0.04)).normalize(),
                radius: 0.3,
                strength: 0.6,
                invert: false,
                symmetry: true,
                direction: Vec3::Y,
                buildup: Default::default(),
            });
            s.refresh();
        }
        s.end_stroke();
    }
    clock
}

#[test]
fn replay_through_a_quad_remesh_equals_the_live_sculpt_bit_for_bit() {
    let mut live = Sculpt::new(Mesh::icosphere(4));
    let clock = strokes(&mut live, 0.0, Vec3::Z);
    let before = live.mesh.positions.clone();
    let remeshed = quad::remesh(&live.mesh, 1500).expect("quad remeshed");
    assert!(!remeshed.quads.is_empty());
    live.set_clock(clock + 1.0);
    live.replace_mesh(remeshed);
    assert!(!live.mesh.quads.is_empty(), "the quads stay with the clay");

    // Undo brings the clay back as it was, redo the quads.
    live.undo();
    assert_eq!(live.mesh.positions, before);
    live.redo();
    assert!(!live.mesh.quads.is_empty());
    let clock = strokes(&mut live, clock + 2.0, Vec3::X);

    let mut replay = Replay::of(&live);
    assert!(replay.seek(f64::INFINITY, usize::MAX));
    assert!(replay.finished());
    assert_eq!(replay.sculpt.mesh.triangles, live.mesh.triangles);
    assert_eq!(replay.sculpt.mesh.positions, live.mesh.positions);
    assert_eq!(replay.sculpt.mesh.normals, live.mesh.normals);

    // Back to before the quad remesh, and forward again in small steps.
    assert!(replay.seek(clock * 0.2, usize::MAX));
    assert_eq!(replay.sculpt.mesh.triangles.len(), before.len() * 2 - 4);
    while !replay.seek(f64::INFINITY, 5) {}
    assert_eq!(replay.sculpt.mesh.positions, live.mesh.positions);
}
