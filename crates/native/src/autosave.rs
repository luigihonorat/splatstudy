// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Autosave on desktop: the same bytes as in the browser, as files in the
//! app's folder (SDL's pref path). `autosave.splatstudy` is the scene, its
//! reference named by a content key; `autosave-<key>.ref` is that
//! reference file, written once per file. Failing is never fatal: a
//! failed write is tried again at the next change (a file held a moment by
//! an antivirus or an indexer makes a rename fail on Windows); a full disk,
//! or a few failures in a row, and the app says so once and goes on
//! without autosave.
//!
//! A restore that crashes the app would crash it again at every start, so
//! a `restoring` file marks a restore under way, from the moment the app
//! opens it (the reference file read) until its first frame is drawn, or
//! the app quits normally. Found at start, the restore is only offered.

use std::path::{Path, PathBuf};
use std::time::Instant;

use splatstudy_core::App;
use splatstudy_core::scene::BlobKey;
use splatstudy_core::scene::autosave::{self, Autosave as Bytes, Pace, Trouble};

const SCENE: &str = "autosave.splatstudy";
const PREFIX: &str = "autosave-";
const SUFFIX: &str = ".ref";
/// There while a restore is under way, until a frame shows it.
const RESTORING: &str = "restoring";
/// Failed writes in a row before autosave stops.
const MAX_FAILURES: u32 = 3;

fn blob_path(folder: &Path, key: BlobKey) -> PathBuf {
    folder.join(format!("{PREFIX}{}{SUFFIX}", key.name()))
}

pub struct Autosave {
    /// `None` once autosave stopped, or when there is no folder.
    folder: Option<PathBuf>,
    /// Failed writes in a row, and the revision the last one was of: it
    /// isn't tried again until the scene changes (or the app quits).
    failures: u32,
    failed: Option<u64>,
    /// When to save; its clock counts seconds from `started`.
    pace: Pace,
    started: Instant,
    stored: u64,
    /// A restore was done; the `restoring` file goes once a frame shows it.
    await_frame: bool,
}

/// Opens the stored scene in `app`, marking the restore under way. Whether
/// it came with its reference; `None` if nothing was restored.
fn restore_from(app: &mut App, folder: &Path) -> Option<bool> {
    let scene = std::fs::read(folder.join(SCENE)).ok()?;
    let path = folder.join(RESTORING);
    let flag = |on: bool| {
        if !on {
            let _ = std::fs::remove_file(&path);
        } else if let Err(e) = std::fs::write(&path, b"") {
            log::warn!("could not write {}: {e}", path.display());
        }
    };
    let read = |key| std::future::ready(crate::files::read(&blob_path(folder, key)).ok());
    let open = |scene: &[u8], reference| app.restore_autosave(scene, reference);
    let key = pollster::block_on(autosave::restore(&scene, read, flag, open))?;
    Some(key.is_some())
}

impl Autosave {
    /// Opens the last session's autosave in `app` unless `fresh`, or
    /// only offers it if the last restore never got to draw a frame, and
    /// starts watching from there. Returns the options still to apply:
    /// without `sample=` when the session came back with its own
    /// reference.
    pub fn restore(app: &mut App, options: &[String], fresh: bool) -> (Self, Vec<String>) {
        let folder = crate::config::folder();
        let mut options = options.to_vec();
        let mut await_frame = false;
        if let Some(folder) = folder.as_deref().filter(|_| !fresh) {
            if folder.join(RESTORING).exists() {
                if folder.join(SCENE).exists() {
                    log::warn!("the last restore didn't finish; offering it instead");
                    app.offer_restore();
                } else {
                    let _ = std::fs::remove_file(folder.join(RESTORING));
                }
            } else if let Some(with_reference) = restore_from(app, folder) {
                await_frame = true;
                if with_reference {
                    options.retain(|o| !o.starts_with("sample="));
                }
            }
        }
        let revision = app.scene_revision();
        let autosave = Self {
            folder,
            failures: 0,
            failed: None,
            pace: Pace::new(revision, 0.0),
            started: Instant::now(),
            stored: revision,
            await_frame,
        };
        (autosave, options)
    }

    /// A frame was drawn: a restore before it didn't crash the app.
    pub fn frame_drawn(&mut self) {
        if std::mem::take(&mut self.await_frame)
            && let Some(folder) = &self.folder
        {
            let _ = std::fs::remove_file(folder.join(RESTORING));
        }
    }

    /// Seconds since watching began.
    fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Saves when the scene changed and has been quiet a moment, or has
    /// gone unsaved for long (at the next pause between strokes), or at
    /// once when the app is `leaving`. Call it after every frame and once
    /// before quitting. Also restores the autosave when the user asks.
    pub fn tick(&mut self, app: &mut App, leaving: bool) {
        // Quitting normally after a restore, before its first frame (the
        // window closed while a big scene opened): it didn't crash the
        // app, so the next start restores it again.
        if leaving {
            self.frame_drawn();
        }
        if app.take_restore_request() {
            match crate::config::folder().and_then(|folder| restore_from(app, &folder)) {
                Some(_) => {
                    self.await_frame = true;
                    self.stored = app.scene_revision();
                    self.pace.reset(self.stored, self.now());
                }
                None => app.autosave_gone(),
            }
        }
        let revision = app.scene_revision();
        let now = self.now();
        let due = self.pace.due(revision, now) || leaving;
        let tried = self.failed == Some(revision) && !leaving;
        if self.folder.is_none() || revision == self.stored || !due || tried {
            return;
        }
        if !app.autosave_ready(leaving) {
            return;
        }
        let bytes = app.autosave_bytes();
        self.pace.saved(now);
        let folder = self.folder.clone().expect("checked");
        match write(&folder, bytes) {
            Ok(()) => {
                self.stored = revision;
                self.failures = 0;
                self.failed = None;
                // What is stored now is new work: start with it next time.
                let _ = std::fs::remove_file(folder.join(RESTORING));
            }
            Err(e) => {
                if let Some(trouble) = self.failed_write(revision, &e) {
                    app.autosave_trouble(trouble);
                }
            }
        }
    }

    /// A write of `revision` failed. What to tell the user when autosave
    /// stops: at once for a full disk, otherwise after a few failures in
    /// a row; until then it is tried again at the next change.
    fn failed_write(&mut self, revision: u64, e: &std::io::Error) -> Option<Trouble> {
        self.failures += 1;
        self.failed = Some(revision);
        let full = e.kind() == std::io::ErrorKind::StorageFull;
        if !full && self.failures < MAX_FAILURES {
            log::warn!("autosave failed, trying again at the next change: {e}");
            return None;
        }
        log::warn!("autosave failed: {e}");
        self.folder = None;
        Some(if full {
            Trouble::Full
        } else {
            Trouble::Unavailable
        })
    }
}

/// Writes the reference file if it isn't there yet, then the scene (by way
/// of a temporary file, so a crash mid-write keeps the last one), then
/// removes reference files no longer named.
fn write(folder: &Path, bytes: Option<Bytes>) -> std::io::Result<()> {
    let keep = match &bytes {
        Some(Bytes {
            reference: Some((key, file)),
            ..
        }) => {
            let path = blob_path(folder, *key);
            if !path.exists() {
                let partial = path.with_extension("partial");
                std::fs::write(&partial, file)?;
                std::fs::rename(&partial, &path)?;
            }
            Some(path)
        }
        _ => None,
    };
    let scene = folder.join(SCENE);
    match bytes {
        Some(save) => {
            let partial = scene.with_extension("partial");
            std::fs::write(&partial, &save.scene)?;
            std::fs::rename(&partial, &scene)?;
        }
        None => match std::fs::remove_file(&scene) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        },
    }
    for entry in std::fs::read_dir(folder)?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let ours = name.starts_with(PREFIX) && name.ends_with(SUFFIX);
        if ours && keep.as_deref() != Some(entry.path().as_path()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    fn watching() -> Autosave {
        Autosave {
            folder: Some(PathBuf::from("autosave")),
            failures: 0,
            failed: None,
            pace: Pace::new(0, 0.0),
            started: Instant::now(),
            stored: 0,
            await_frame: false,
        }
    }

    #[test]
    fn a_failed_write_is_tried_again_and_a_few_in_a_row_stop_autosave() {
        let mut autosave = watching();
        let denied = Error::from(ErrorKind::PermissionDenied);
        assert_eq!(autosave.failed_write(1, &denied), None);
        assert!(autosave.folder.is_some());
        assert_eq!(autosave.failed, Some(1));
        assert_eq!(autosave.failed_write(2, &denied), None);
        assert_eq!(
            autosave.failed_write(3, &denied),
            Some(Trouble::Unavailable)
        );
        assert!(autosave.folder.is_none());
    }

    #[test]
    fn a_full_disk_stops_autosave_at_once() {
        let mut autosave = watching();
        let full = Error::from(ErrorKind::StorageFull);
        assert_eq!(autosave.failed_write(1, &full), Some(Trouble::Full));
        assert!(autosave.folder.is_none());
    }
}
