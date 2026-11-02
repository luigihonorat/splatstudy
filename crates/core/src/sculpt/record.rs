// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The record of a session: every call that changed the clay, with the time
//! it was made. The brushes are deterministic, so applying the same calls to
//! the same start mesh rebuilds the study step by step, as a timelapse.

use glam::Vec3;

use super::{Dab, Mesh, Sculpt};

/// Pauses longer than this, in seconds, are shortened to it in playback.
const MAX_GAP: f64 = 2.0;

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Begin,
    Dab(Dab),
    Grab {
        centre: Vec3,
        radius: f32,
        symmetry: bool,
    },
    Drag(Vec3),
    End,
    Undo,
    Redo,
    /// The hierarchy was refitted (see `Sculpt::refresh`).
    Refresh,
}

#[derive(Clone, Copy, Debug)]
pub struct Entry {
    /// Seconds, on the app's clock.
    pub time: f64,
    pub event: Event,
}

impl Sculpt {
    /// A sculpt that keeps no log of its own, for playing one back.
    fn replaying(mesh: Mesh) -> Sculpt {
        Sculpt {
            recording: false,
            ..Sculpt::new(mesh)
        }
    }

    /// Does what a log entry recorded.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Begin => self.begin_stroke(),
            Event::Dab(dab) => self.dab(dab),
            Event::Grab {
                centre,
                radius,
                symmetry,
            } => self.grab(centre, radius, symmetry),
            Event::Drag(offset) => self.drag(offset),
            Event::End => self.end_stroke(),
            Event::Undo => self.undo(),
            Event::Redo => self.redo(),
            Event::Refresh => self.refresh(),
        }
    }
}

/// Plays a log back onto a copy of the mesh it started from.
pub struct Replay {
    /// The clay as far as it has been played.
    pub sculpt: Sculpt,
    start: Mesh,
    entries: Vec<Entry>,
    /// When each entry plays, from 0: the recorded times with long pauses shortened.
    times: Vec<f64>,
    /// How many entries have been applied.
    next: usize,
}

impl Replay {
    pub fn new(start: Mesh, entries: Vec<Entry>) -> Replay {
        let mut times = Vec::with_capacity(entries.len());
        let mut t = 0.0;
        for (i, entry) in entries.iter().enumerate() {
            if i > 0 {
                t += (entry.time - entries[i - 1].time).clamp(0.0, MAX_GAP);
            }
            times.push(t);
        }
        Replay {
            sculpt: Sculpt::replaying(start.clone()),
            start,
            entries,
            times,
            next: 0,
        }
    }

    /// Playback length, in seconds.
    pub fn duration(&self) -> f64 {
        self.times.last().copied().unwrap_or(0.0)
    }

    /// Playback time reached so far.
    pub fn position(&self) -> f64 {
        self.next.checked_sub(1).map_or(0.0, |i| self.times[i])
    }

    pub fn finished(&self) -> bool {
        self.next == self.entries.len()
    }

    /// Plays up to `time`, starting over from the start mesh if it lies
    /// behind. Applies at most `budget` entries, so a long jump spreads over
    /// several frames; true once `time` is reached.
    pub fn seek(&mut self, time: f64, budget: usize) -> bool {
        if time < self.position() {
            self.sculpt = Sculpt::replaying(self.start.clone());
            self.next = 0;
        }
        for _ in 0..budget {
            match self.times.get(self.next) {
                Some(&t) if t <= time => {
                    self.sculpt.apply(self.entries[self.next].event);
                    self.next += 1;
                }
                _ => return true,
            }
        }
        self.times.get(self.next).is_none_or(|&t| t > time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sculpt::Brush;

    /// A session that uses every brush, Move, undo and redo, with refits at
    /// uneven moments as frames would make them.
    fn session(s: &mut Sculpt) {
        let mut clock = 0.0;
        for (k, brush) in Brush::ALL.into_iter().enumerate() {
            clock += 1.5;
            s.set_clock(clock);
            let at = Vec3::new(0.3 - k as f32 * 0.1, 0.2, 0.9).normalize();
            s.begin_stroke();
            if brush == Brush::Move {
                s.grab(at, 0.3, true);
                for i in 1..5 {
                    s.drag(Vec3::new(0.0, 0.02, 0.03) * i as f32);
                    if i % 2 == 0 {
                        s.refresh();
                    }
                }
            } else {
                for i in 0..8 {
                    s.set_clock(clock + i as f64 * 0.02);
                    s.dab(Dab {
                        brush,
                        centre: (at + Vec3::Y * (i as f32 * 0.03)).normalize(),
                        radius: 0.25,
                        strength: 0.7,
                        invert: i % 3 == 2,
                        symmetry: true,
                        direction: if i == 0 { Vec3::ZERO } else { Vec3::Y },
                    });
                    if i % 3 == 0 {
                        s.refresh();
                    }
                }
            }
            s.end_stroke();
            s.refresh();
        }
        // A long pause, then second thoughts.
        s.set_clock(clock + 60.0);
        s.undo();
        s.undo();
        s.redo();
    }

    #[test]
    fn replay_rebuilds_the_session_exactly() {
        let mut live = Sculpt::new(Mesh::icosphere(4));
        session(&mut live);
        let mut replay = Replay::new(Mesh::icosphere(4), live.log().to_vec());
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert!(replay.finished());
        assert_eq!(replay.sculpt.mesh.positions, live.mesh.positions);
        assert_eq!(replay.sculpt.mesh.normals, live.mesh.normals);
        assert!(replay.sculpt.log().is_empty(), "playback keeps no log");
    }

    #[test]
    fn seeking_back_and_in_small_steps_gives_the_same_clay() {
        let mut live = Sculpt::new(Mesh::icosphere(4));
        session(&mut live);
        let mut replay = Replay::new(Mesh::icosphere(4), live.log().to_vec());
        let half = replay.duration() / 2.0;
        assert!(replay.seek(half, usize::MAX));
        let at_half = replay.sculpt.mesh.positions.clone();
        assert_ne!(at_half, live.mesh.positions);
        // Forward to the end ten entries at a time, then back to the middle.
        while !replay.seek(f64::INFINITY, 10) {}
        assert_eq!(replay.sculpt.mesh.positions, live.mesh.positions);
        assert!(replay.seek(half, usize::MAX));
        assert_eq!(replay.sculpt.mesh.positions, at_half);
    }

    #[test]
    fn long_pauses_are_shortened() {
        let mut live = Sculpt::new(Mesh::icosphere(2));
        session(&mut live);
        let recorded = live.log().last().unwrap().time - live.log()[0].time;
        let replay = Replay::new(Mesh::icosphere(2), live.log().to_vec());
        assert!(recorded > 60.0);
        assert!(replay.duration() < recorded - 55.0, "{}", replay.duration());
    }
}
