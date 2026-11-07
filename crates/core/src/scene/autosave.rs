// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Autosave, as the shells see it. The app makes the bytes and the shells
//! keep them: IndexedDB in the browser, a file on desktop.
//!
//! A shell drives it like this:
//!
//! 1. At start, unless told not to, it reads the saved scene, asks
//!    [`reference_key`](super::reference_key) which reference file goes
//!    with it, fetches that file, and hands both to
//!    `App::restore_autosave`.
//! 2. It watches `App::scene_revision` after every frame, through a
//!    [`Pace`]. About two seconds after the last change, or once the
//!    store is half a minute behind (someone sculpting without a pause),
//!    when `App::autosave_ready(false)` says no stroke or export is under
//!    way, it takes `App::autosave_bytes` and stores them; when the page
//!    or window is going away it does the same with `autosave_ready(true)`
//!    at once.
//! 3. `autosave_bytes` gives `None` when there is nothing worth keeping (a
//!    new scene): the shell then drops what it kept.
//! 4. If storing fails, it tells the app once through
//!    `App::autosave_trouble` and stops autosaving.

use std::sync::Arc;

use super::BlobKey;

/// What to store for an autosave.
pub struct Autosave {
    /// The scene, its reference named by key (a `REFH` chunk).
    pub scene: Vec<u8>,
    /// The reference file and its key. Store it once per key, and keep
    /// only the one the latest scene names.
    pub reference: Option<(BlobKey, Arc<[u8]>)>,
    /// The clay has been worked on. Until then the browser isn't asked to
    /// keep the storage for good (Firefox asks the user).
    pub sculpted: bool,
}

/// Why autosave stopped or left something out; the app tells the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// There is nowhere to store it (a private window, storage blocked).
    Unavailable,
    /// The storage is full.
    Full,
    /// The reference file doesn't fit; the clay is still saved.
    ReferenceTooBig,
}

/// Seconds without a change before saving.
pub const QUIET: f64 = 2.0;
/// Seconds after which unsaved changes are saved even without a quiet
/// moment, at the next pause between strokes.
pub const MAX_WAIT: f64 = 30.0;

/// When to save: once the scene has been quiet a moment, or when the last
/// save is old and a steady rhythm of strokes never leaves a quiet moment.
/// Times are seconds on any clock that only goes forward.
#[derive(Clone, Copy, Debug)]
pub struct Pace {
    /// The revision last seen, and when it changed.
    seen: u64,
    changed_at: f64,
    /// When the last save was taken, or watching began.
    saved_at: f64,
}

impl Pace {
    /// Starts watching at `revision`, as stored (or with nothing to store).
    pub fn new(revision: u64, now: f64) -> Self {
        Self {
            seen: revision,
            changed_at: now,
            saved_at: now,
        }
    }

    /// Notes the scene's `revision` at `now`. True when a save is due, if
    /// there is something unsaved: the scene has been quiet for [`QUIET`],
    /// or the last save is older than [`MAX_WAIT`].
    pub fn due(&mut self, revision: u64, now: f64) -> bool {
        if revision != self.seen {
            self.seen = revision;
            self.changed_at = now;
        }
        now - self.changed_at >= QUIET || now - self.saved_at >= MAX_WAIT
    }

    /// A save was taken at `now` (or tried: a failed one waits as long
    /// before the next try).
    pub fn saved(&mut self, now: f64) {
        self.saved_at = now;
    }

    /// The scene at `revision` is what is stored now, as after a restore.
    pub fn reset(&mut self, revision: u64, now: f64) {
        *self = Self::new(revision, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_waits_for_a_quiet_moment() {
        let mut pace = Pace::new(0, 0.0);
        assert!(!pace.due(1, 10.0));
        assert!(!pace.due(1, 11.9));
        assert!(pace.due(1, 12.0));
        // Each change starts the wait again.
        pace.saved(12.0);
        assert!(!pace.due(2, 13.0));
        assert!(!pace.due(3, 14.5));
        assert!(pace.due(3, 16.5));
    }

    #[test]
    fn steady_sculpting_is_saved_every_half_minute() {
        let mut pace = Pace::new(0, 0.0);
        // A stroke every second, never two quiet seconds in a row.
        let mut saves = Vec::new();
        for second in 1..=95 {
            let now = f64::from(second);
            if pace.due(u64::try_from(second).unwrap(), now) {
                saves.push(second);
                pace.saved(now);
            }
        }
        assert_eq!(saves, [30, 60, 90]);
    }

    #[test]
    fn an_old_save_is_overdue_at_the_first_change() {
        // Nothing changed for a long while, then a stroke: it is saved as
        // soon as the shell sees it is unsaved and between strokes.
        let mut pace = Pace::new(0, 0.0);
        assert!(pace.due(0, 300.0));
        assert!(pace.due(1, 301.0));
        pace.saved(301.0);
        assert!(!pace.due(2, 302.0));
    }

    #[test]
    fn a_restore_starts_the_clock_again() {
        let mut pace = Pace::new(0, 0.0);
        pace.reset(7, 100.0);
        assert!(!pace.due(7, 101.0));
        assert!(pace.due(7, 102.0));
        assert!(!pace.due(8, 103.0));
    }
}
