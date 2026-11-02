// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Autosave on desktop: the same bytes as in the browser, as files in the
//! app's folder (SDL's pref path). `autosave.splatstudy` is the scene, its
//! reference named by a content key; `autosave-<key>.ref` is that
//! reference file, written once per file. Failing is never fatal: the app
//! says so once and goes on without autosave.
//!
//! A restore that crashes the app would crash it again at every start, so
//! a `restoring` file marks a restore under way until its first frame is
//! drawn. Found at start, the restore is only offered.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use splatstudy_core::App;
use splatstudy_core::scene::autosave::{Autosave as Bytes, Trouble};
use splatstudy_core::scene::{self, BlobKey};

const SCENE: &str = "autosave.splatstudy";
const PREFIX: &str = "autosave-";
const SUFFIX: &str = ".ref";
/// There while a restore is under way, until a frame shows it.
const RESTORING: &str = "restoring";
/// Time without a change before saving.
const QUIET: Duration = Duration::from_secs(2);

fn blob_path(folder: &Path, key: BlobKey) -> PathBuf {
    folder.join(format!("{PREFIX}{}{SUFFIX}", key.name()))
}

pub struct Autosave {
    /// `None` once writing failed, or when there is no folder.
    folder: Option<PathBuf>,
    seen: u64,
    changed_at: Instant,
    stored: u64,
    /// A restore was done; the `restoring` file goes once a frame shows it.
    await_frame: bool,
}

/// Opens the stored scene in `app`, marking the restore under way. Whether
/// it came with its reference; `None` if nothing was restored.
fn restore_from(app: &mut App, folder: &Path) -> Option<bool> {
    let scene = std::fs::read(folder.join(SCENE)).ok()?;
    let flag = folder.join(RESTORING);
    if let Err(e) = std::fs::write(&flag, b"") {
        log::warn!("could not write {}: {e}", flag.display());
    }
    let key = scene::reference_key(&scene).ok().flatten();
    let reference = key
        .and_then(|key| std::fs::read(blob_path(folder, key)).ok())
        .map(Arc::<[u8]>::from);
    let with_reference = reference.is_some();
    if app.restore_autosave(&scene, reference) {
        Some(with_reference)
    } else {
        let _ = std::fs::remove_file(&flag);
        None
    }
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
            seen: revision,
            changed_at: Instant::now(),
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

    /// Saves when the scene changed and has been quiet a moment, or at
    /// once when the app is `leaving`. Call it after every frame and once
    /// before quitting. Also restores the autosave when the user asks.
    pub fn tick(&mut self, app: &mut App, leaving: bool) {
        if app.take_restore_request() {
            match crate::config::folder().and_then(|folder| restore_from(app, &folder)) {
                Some(_) => {
                    self.await_frame = true;
                    self.seen = app.scene_revision();
                    self.stored = self.seen;
                }
                None => app.autosave_gone(),
            }
        }
        let revision = app.scene_revision();
        if revision != self.seen {
            self.seen = revision;
            self.changed_at = Instant::now();
        }
        let quiet = leaving || self.changed_at.elapsed() >= QUIET;
        if self.folder.is_none() || revision == self.stored || !quiet {
            return;
        }
        if !app.autosave_ready(leaving) {
            return;
        }
        let bytes = app.autosave_bytes();
        let folder = self.folder.clone().expect("checked");
        match write(&folder, bytes) {
            Ok(()) => {
                self.stored = revision;
                // What is stored now is new work: start with it next time.
                let _ = std::fs::remove_file(folder.join(RESTORING));
            }
            Err(e) => {
                log::warn!("autosave failed: {e}");
                self.folder = None;
                let full = e.kind() == std::io::ErrorKind::StorageFull;
                app.autosave_trouble(if full {
                    Trouble::Full
                } else {
                    Trouble::Unavailable
                });
            }
        }
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
