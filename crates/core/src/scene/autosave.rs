// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Autosave, as the shells see it. The app makes the bytes and the shells
//! keep them: IndexedDB in the browser, a file on desktop.
//!
//! A shell drives it like this:
//!
//! 1. At start, unless told not to, it reads the saved scene and opens it
//!    through [`restore`]: that asks [`reference_key`](super::reference_key)
//!    which reference file goes with it, has the shell fetch that file,
//!    and hands both to `App::restore_autosave`. A restore that crashed
//!    the app would crash it again at every start, so the shell keeps a
//!    flag (a file, a localStorage key) while one is under way, until a
//!    frame shows it or the app leaves normally; found at start, the
//!    restore is only offered (`App::offer_restore`).
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

use std::future::Future;
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

/// Opens a stored `scene`: `read` fetches the reference file it names,
/// which can take seconds for a big splat, then `open` opens both in the
/// app (`App::restore_autosave`). The restore is flagged under way
/// (`flag(true)`) only after the file is read, just around `open` and
/// the first frame: a reload or a closed window while reading leaves no
/// flag, so the next start restores again instead of only offering, which
/// would let the first change replace the session. A failed `open` drops
/// the flag at once (`flag(false)`); otherwise the shell drops it once a
/// frame is drawn.
///
/// `None` if the scene didn't open; otherwise the key of the reference
/// file, when it came back too.
pub async fn restore<R>(
    scene: &[u8],
    read: impl FnOnce(BlobKey) -> R,
    mut flag: impl FnMut(bool),
    open: impl FnOnce(&[u8], Option<Arc<[u8]>>) -> bool,
) -> Option<Option<BlobKey>>
where
    R: Future<Output = Option<Arc<[u8]>>>,
{
    let key = super::reference_key(scene).ok().flatten();
    let reference = match key {
        Some(key) => read(key).await,
        None => None,
    };
    let found = key.filter(|_| reference.is_some());
    flag(true);
    if open(scene, reference) {
        Some(found)
    } else {
        flag(false);
        None
    }
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
    use std::cell::Cell;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use super::*;
    use crate::reference::{Content, RefImage, Reference, Source};
    use crate::scene::SceneWriter;

    /// An autosave naming a reference file stored apart, and that file.
    fn stored() -> (Vec<u8>, Arc<[u8]>) {
        let file: Arc<[u8]> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let reference = Reference::new(
            Source {
                name: "cast.png".into(),
                bytes: file.clone(),
            },
            Content::Image(RefImage::default()),
        );
        let mut w = SceneWriter::new();
        w.reference_by_key(&reference, BlobKey::of(&file));
        (w.finish(), file)
    }

    /// A read that never finishes: the page goes before it does.
    struct Slow;

    impl Future for Slow {
        type Output = Option<Arc<[u8]>>;
        fn poll(self: std::pin::Pin<&mut Self>, _: &mut Context) -> Poll<Self::Output> {
            Poll::Pending
        }
    }

    fn run<F: Future>(future: F) -> Poll<F::Output> {
        pin!(future)
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn a_reload_while_the_reference_is_read_leaves_no_restore_flag() {
        let (scene, _) = stored();
        let flagged = Cell::new(false);
        let opened = Cell::new(false);
        let restoring = restore(
            &scene,
            |_| Slow,
            |on| flagged.set(on),
            |_, _| {
                opened.set(true);
                true
            },
        );
        // The page goes while the file is still being read.
        assert!(run(restoring).is_pending());
        assert!(!opened.get());
        assert!(!flagged.get(), "the next start would only offer it");
    }

    #[test]
    fn the_restore_is_flagged_from_the_open_until_a_frame() {
        let (scene, file) = stored();
        let flags = std::cell::RefCell::new(Vec::new());
        let restored = run(restore(
            &scene,
            |key| {
                assert!(key.matches(&file));
                std::future::ready(Some(file.clone()))
            },
            |on| flags.borrow_mut().push(on),
            |_, reference| {
                // Flagged while the app opens it, which may crash it.
                assert_eq!(*flags.borrow(), [true]);
                assert!(reference.is_some());
                true
            },
        ));
        assert_eq!(restored, Poll::Ready(Some(Some(BlobKey::of(&file)))));
        // Still flagged: the shell drops it once a frame is drawn.
        assert_eq!(*flags.borrow(), [true]);
    }

    #[test]
    fn a_scene_that_does_not_open_drops_the_flag() {
        let (scene, _) = stored();
        let flags = std::cell::RefCell::new(Vec::new());
        let restored = run(restore(
            &scene,
            |_| std::future::ready(None),
            |on| flags.borrow_mut().push(on),
            |_, reference| {
                assert!(reference.is_none());
                false
            },
        ));
        assert_eq!(restored, Poll::Ready(None));
        assert_eq!(*flags.borrow(), [true, false]);
    }

    #[test]
    fn without_its_file_the_scene_opens_with_no_key() {
        let (scene, _) = stored();
        let restored = run(restore(
            &scene,
            |_| std::future::ready(None),
            |_| {},
            |_, _| true,
        ));
        assert_eq!(restored, Poll::Ready(Some(None)));
    }

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
