// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Watching the timelapse in the sculpt view.

use super::sculpting::start_mesh;
use super::{App, SCULPT};
use crate::render::clay::GpuMesh;
use crate::sculpt::Replay;
use crate::ui;

/// Log entries a timelapse may apply per frame: a fast replay lags behind
/// rather than stalling the app.
const REPLAY_BUDGET: usize = 200;
/// Timelapse speeds, as multiples of the time spent sculpting.
const REPLAY_SPEEDS: [f64; 3] = [10.0, 60.0, 300.0];

/// The timelapse being watched in the sculpt view.
pub(super) struct Playback {
    pub(super) replay: Replay,
    pub(super) gpu_mesh: GpuMesh,
    playing: bool,
    /// Playback seconds per second.
    speed: f64,
    /// Where the playhead is, in playback seconds; the clay may lag behind it.
    head: f64,
    /// The app clock at the last frame.
    last: f64,
}

impl App {
    /// Plays the session back from the first stroke on a separate copy of
    /// the clay; the live sculpt waits untouched.
    pub(super) fn start_playback(&mut self) {
        // An open stroke would be logged half done.
        self.sculpt.end_stroke();
        self.drags[SCULPT] = None;
        let replay = Replay::new(start_mesh(), self.sculpt.log().to_vec());
        self.playback = Some(Playback {
            gpu_mesh: GpuMesh::new(&self.device, &replay.sculpt.mesh),
            replay,
            playing: true,
            speed: REPLAY_SPEEDS[1],
            head: 0.0,
            last: self.now,
        });
    }

    /// Moves the playhead with the clock and brings the clay up to it.
    fn advance_playback(&mut self, ui: &egui::Ui) {
        let Some(p) = &mut self.playback else {
            return;
        };
        let duration = p.replay.duration();
        if p.playing {
            // A long gap between frames (a hidden tab) doesn't jump ahead.
            let elapsed = (self.now - p.last).clamp(0.0, 0.1);
            p.head = (p.head + elapsed * p.speed).min(duration);
            p.playing = p.head < duration;
        }
        p.last = self.now;
        let caught_up = p.replay.seek(p.head, REPLAY_BUDGET);
        if p.playing || !caught_up {
            ui.ctx().request_repaint();
        }
    }

    pub(super) fn playback_panel(&mut self, ui: &mut egui::Ui) {
        self.advance_playback(ui);
        let tr = self.lang.tr();
        let Some(p) = &mut self.playback else {
            return;
        };
        ui::section(ui, tr.timelapse);
        let at_end = p.head >= p.replay.duration();
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
        let duration = p.replay.duration() as f32;
        let clock = |seconds: f32| {
            let s = seconds.max(0.0) as u32;
            format!("{}:{:02}", s / 60, s % 60)
        };
        // The playhead on the left, the length on the right.
        if ui::slider(
            ui,
            &clock(head),
            &mut head,
            0.0..=duration.max(1e-3),
            |_| clock(duration),
        )
        .changed()
        {
            p.head = head as f64;
        }
        self.playback_export_items(ui);
        ui.add_space(10.0);
        if ui::choice(ui, tr.back_to_sculpt, false).clicked() {
            self.playback = None;
        }
    }
}
