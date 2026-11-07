// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Watching the timelapse in the sculpt view.

use super::{App, SCULPT};
use crate::render::clay::GpuMesh;
use crate::sculpt::{Checkpoint, Entry, Mesh, Replay};
use crate::ui;

/// Log entries a timelapse may apply per frame: a fast replay lags behind
/// rather than stalling the app.
const REPLAY_BUDGET: usize = 200;
/// Timelapse speeds, as multiples of the time spent sculpting.
const REPLAY_SPEEDS: [f64; 3] = [10.0, 60.0, 300.0];

/// The playback length of a session's log, in seconds, without building
/// its clay: only the times matter, so a tiny start mesh will do.
pub(super) fn playback_length(log: &[Entry]) -> f64 {
    let tiny = Mesh::icosphere(0);
    Replay::new(vec![Checkpoint::start(&tiny)], log.to_vec()).duration()
}

/// The timelapse's own copy of the clay, played up to the playhead.
pub(super) struct Played {
    pub(super) replay: Replay,
    pub(super) gpu_mesh: GpuMesh,
}

/// The timelapse being watched in the sculpt view.
pub(super) struct Playback {
    /// The clay as far as it has been played. `None` while an export runs:
    /// the export plays its own copy, and a fine clay held three times
    /// over (the live one, this one and the export's) can run a browser
    /// tab out of memory. Made again from the live sculpt once the export
    /// is over, and played up to the playhead.
    pub(super) played: Option<Played>,
    /// The playback length, in seconds; kept for the panel while there is
    /// no clay.
    duration: f64,
    pub(super) playing: bool,
    /// Playback seconds per second.
    speed: f64,
    /// Where the playhead is, in playback seconds; the clay may lag behind it.
    pub(super) head: f64,
    /// The app clock at the last frame.
    last: f64,
    /// The playhead is being dragged: going back waits for the release,
    /// since each step back replays from a checkpoint.
    scrubbing: bool,
}

impl Playback {
    /// Playing from the start of a session `duration` seconds long, with
    /// no clay yet.
    fn new(duration: f64, now: f64) -> Self {
        Self {
            played: None,
            duration,
            playing: true,
            speed: REPLAY_SPEEDS[1],
            head: 0.0,
            last: now,
            scrubbing: false,
        }
    }

    /// Lets go of the clay while an export runs, paused where it was: back
    /// from the export, the timelapse waits there.
    pub(super) fn suspend(&mut self) {
        self.played = None;
        self.playing = false;
        self.scrubbing = false;
    }

    /// Moves the playhead with the clock, now at `now`.
    fn tick(&mut self, now: f64) {
        if self.playing {
            // A long gap between frames (a hidden tab) doesn't jump ahead.
            let elapsed = (now - self.last).clamp(0.0, 0.1);
            self.head = (self.head + elapsed * self.speed).min(self.duration);
            self.playing = self.head < self.duration;
        }
        self.last = now;
    }

    /// The session is now `duration` seconds long: the playhead stays
    /// inside it.
    fn fit(&mut self, duration: f64) {
        self.duration = duration;
        self.head = self.head.min(duration);
    }
}

impl App {
    /// Plays the session back from its first checkpoint on a separate copy
    /// of the clay; the live sculpt waits untouched. While an export runs,
    /// the copy waits for the export to end.
    pub(super) fn start_playback(&mut self) {
        // An open stroke would be logged half done.
        self.sculpt.end_stroke();
        self.drags[SCULPT] = None;
        let mut playback = Playback::new(playback_length(self.sculpt.log()), self.now);
        if !self.exporting() {
            playback.played = Some(self.played());
        }
        self.playback = Some(playback);
    }

    /// A copy of the live clay to play back, at the start.
    fn played(&self) -> Played {
        let mut replay = Replay::of(&self.sculpt);
        replay.sculpt.take_topology_changed();
        Played {
            gpu_mesh: GpuMesh::upload(&self.device, &self.queue, &replay.sculpt.mesh),
            replay,
        }
    }

    /// Moves the playhead with the clock and brings the clay up to it.
    pub(super) fn advance_playback(&mut self, ui: &egui::Ui) {
        let Some(p) = &self.playback else {
            return;
        };
        if p.played.is_none() && !self.exporting() {
            // Back from an export: the clay again, played up to the
            // playhead over the next frames.
            let played = self.played();
            if let Some(p) = &mut self.playback {
                p.fit(played.replay.duration());
                p.played = Some(played);
            }
        }
        let Some(p) = &mut self.playback else {
            return;
        };
        if p.played.is_none() {
            // An export is running; the playhead waits.
            p.last = self.now;
            return;
        }
        p.tick(self.now);
        let Some(played) = &mut p.played else {
            return;
        };
        // Dragged back: the clay waits for the release (forward is cheap).
        let replay = &mut played.replay;
        let waiting = p.scrubbing && p.head < replay.position();
        let caught_up = waiting || replay.seek(p.head, REPLAY_BUDGET);
        // Past a remesh, or back to the start: other triangles.
        let sculpt = &mut replay.sculpt;
        if sculpt.take_topology_changed() {
            played.gpu_mesh = GpuMesh::upload(&self.device, &self.queue, &sculpt.mesh);
            sculpt.take_dirty();
        }
        if p.playing || !caught_up {
            ui.ctx().request_repaint();
        }
    }

    pub(super) fn playback_panel(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        let Some(p) = &mut self.playback else {
            return;
        };
        ui::section(ui, tr.timelapse);
        let at_end = p.head >= p.duration;
        if ui::choice(ui, if p.playing { tr.pause } else { tr.play }, false).clicked() {
            if at_end {
                p.head = 0.0;
            }
            p.playing = !p.playing;
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            for speed in REPLAY_SPEEDS {
                if ui::choice(ui, format!("×{speed}"), p.speed == speed).clicked() {
                    p.speed = speed;
                }
            }
        });
        ui.add_space(6.0);
        let mut head = p.head as f32;
        let duration = p.duration as f32;
        let clock = |seconds: f32| {
            let s = seconds.max(0.0) as u32;
            format!("{}:{:02}", s / 60, s % 60)
        };
        // The playhead on the left, the length on the right.
        let slider = ui::slider(
            ui,
            &clock(head),
            &mut head,
            0.0..=duration.max(1e-3),
            |_| clock(duration),
        );
        if slider.changed() {
            p.head = head as f64;
        }
        p.scrubbing = slider.dragged();
        self.playback_export_items(ui);
        ui.add_space(10.0);
        if ui::choice(ui, tr.back_to_sculpt, false).clicked() {
            self.playback = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sculpt::Event;

    #[test]
    fn the_playhead_follows_the_clock_and_stops_at_the_end() {
        let mut p = Playback::new(100.0, 0.0);
        p.tick(0.05);
        assert!(
            (p.head - 0.05 * REPLAY_SPEEDS[1]).abs() < 1e-9,
            "{}",
            p.head
        );
        // A long gap counts as a tenth of a second.
        p.tick(10.0);
        assert!(
            (p.head - 0.15 * REPLAY_SPEEDS[1]).abs() < 1e-9,
            "{}",
            p.head
        );
        for i in 1..=100 {
            p.tick(10.0 + i as f64 * 0.1);
        }
        assert_eq!(p.head, 100.0);
        assert!(!p.playing);
    }

    #[test]
    fn an_export_pauses_the_timelapse_where_it_was() {
        let mut p = Playback::new(100.0, 0.0);
        p.tick(0.1);
        let head = p.head;
        p.scrubbing = true;
        p.suspend();
        assert!(p.played.is_none() && !p.playing && !p.scrubbing);
        // However long the export takes, the playhead stays.
        p.tick(30.0);
        p.tick(60.0);
        assert_eq!(p.head, head);
        // Made again from a session that is now shorter: still inside it.
        p.fit(head / 2.0);
        assert_eq!(p.head, head / 2.0);
        p.fit(100.0);
        assert_eq!(p.head, head / 2.0);
    }

    #[test]
    fn the_playback_length_needs_only_the_times() {
        assert_eq!(playback_length(&[]), 0.0);
        let log = [
            Entry {
                time: 3.0,
                event: Event::Rebuild,
            },
            Entry {
                time: 4.5,
                event: Event::Rebuild,
            },
        ];
        assert_eq!(playback_length(&log), 1.5);
    }
}
