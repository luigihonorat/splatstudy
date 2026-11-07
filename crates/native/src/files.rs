// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Files on the desktop: dropped files, open, save and folder dialogs,
//! for the core's file events and requests.
//!
//! SDL runs dialog callbacks on a thread of its own on Windows (and maybe
//! elsewhere), and files are read on worker threads so a large splat doesn't
//! stall the window. Every result goes into a shared inbox; the main loop
//! takes it after each batch of events, and a custom SDL event wakes the loop
//! when something arrives while it sleeps.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use sdl3::EventSubsystem;
use sdl3::dialog::{self, DialogError, DialogFileFilter};
use sdl3::event::EventSender;
use sdl3::video::Window;
use splatstudy_core::reference::samples::Sample;
use splatstudy_core::{App, FilePurpose, InputEvent, Request};

use crate::gallery::{self, Wanted};

/// The custom event that wakes the main loop.
struct Wake;

/// Larger files aren't read: no reference or scene is that big, and the
/// buffer alone could exhaust memory.
const MAX_FILE: u64 = 2 << 30;

/// The newest read. A slow read that finishes after a newer one started is
/// dropped instead of replacing it.
static LATEST: AtomicU64 = AtomicU64::new(0);

/// Results waiting for the main thread, and a way to wake it.
#[derive(Clone)]
struct Inbox {
    events: Arc<Mutex<Vec<InputEvent>>>,
    /// `None` when no custom event could be registered: the loop then picks
    /// results up within its idle timeout.
    wake: Option<Arc<EventSender>>,
}

impl Inbox {
    fn push(&self, event: InputEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
        if let Some(sender) = &self.wake
            && let Err(e) = sender.push_custom_event(Wake)
        {
            log::warn!("could not wake the main loop: {e}");
        }
    }

    /// A failed save or write, for the app to tell the user.
    fn error(&self, message: String) {
        log::error!("{message}");
        self.push(InputEvent::WriteFailed { message });
    }

    /// Reads a file on a worker thread and delivers it.
    fn read_later(&self, path: PathBuf, purpose: FilePurpose) {
        let inbox = self.clone();
        let ticket = LATEST.fetch_add(1, Ordering::SeqCst) + 1;
        std::thread::spawn(move || {
            let name = file_name(&path);
            let read = if App::can_open(purpose, &name) {
                read(&path)
            } else {
                Err(std::io::Error::other("not a file SplatStudy opens"))
            };
            if LATEST.load(Ordering::SeqCst) != ticket {
                log::info!("{} was replaced by a newer file", path.display());
                return;
            }
            let bytes = match read {
                Ok(bytes) => {
                    log::info!("read {}: {} bytes", path.display(), bytes.len());
                    bytes
                }
                Err(e) => {
                    log::error!("could not read {}: {e}", path.display());
                    // Empty, so the app says it can't open this file.
                    Arc::from([])
                }
            };
            inbox.push(InputEvent::File {
                purpose,
                name,
                bytes,
            });
        });
    }

    /// Opens a gallery cast on a worker thread, downloading it into
    /// `folder` first unless it is there already, and delivers it; empty
    /// if it couldn't be had, so the app says the download failed.
    fn cast_later(&self, folder: PathBuf, sample: &'static Sample, purpose: FilePurpose) {
        let inbox = self.clone();
        let ticket = purpose
            .supersedes()
            .then(|| LATEST.fetch_add(1, Ordering::SeqCst) + 1);
        std::thread::spawn(move || {
            let cast = gallery::cast(&folder, sample).and_then(|path| read(&path));
            // Kept for next time even when a newer file took its place.
            if ticket.is_some_and(|t| LATEST.load(Ordering::SeqCst) != t) {
                log::info!("{} was replaced by a newer file", sample.file);
                return;
            }
            let bytes = match cast {
                Ok(bytes) => {
                    log::info!("opened {}: {} bytes", sample.file, bytes.len());
                    bytes
                }
                Err(e) => {
                    log::error!("could not download {}: {e}", sample.file);
                    Arc::from([])
                }
            };
            inbox.push(InputEvent::File {
                purpose,
                name: sample.file.to_owned(),
                bytes,
            });
        });
    }
}

pub struct Files {
    inbox: Inbox,
    /// The folder picked last, for `WriteFile`.
    folder: Option<PathBuf>,
    /// A write into the folder failed: the rest are skipped, with one message.
    write_failed: bool,
}

impl Files {
    pub fn new(events: &EventSubsystem) -> Self {
        let wake = match events.register_custom_event::<Wake>() {
            Ok(()) => Some(Arc::new(events.event_sender())),
            Err(e) => {
                log::warn!("no wake-up event: {e}");
                None
            }
        };
        Self {
            inbox: Inbox {
                events: Arc::default(),
                wake,
            },
            folder: None,
            write_failed: false,
        }
    }

    /// A file dropped on the window; the app goes by its extension.
    pub fn dropped(&self, path: &str) {
        self.inbox.read_later(PathBuf::from(path), FilePurpose::Any);
    }

    /// Everything that arrived since the last call, for the app.
    pub fn take(&mut self) -> Vec<InputEvent> {
        let events = match self.inbox.events.lock() {
            Ok(mut events) => std::mem::take(&mut *events),
            Err(_) => return Vec::new(),
        };
        for event in &events {
            if let InputEvent::Folder { path } = event {
                // An empty path: the picker was cancelled.
                self.folder = (!path.is_empty()).then(|| PathBuf::from(path));
                self.write_failed = false;
            }
        }
        events
    }

    /// Does what the app asked for after a frame.
    pub fn handle(&mut self, request: Request, window: &Window) {
        match request {
            Request::OpenFile {
                purpose,
                extensions,
            } => self.open(purpose, extensions, window),
            Request::SaveFile { name, bytes, .. } => self.save(&name, bytes, window),
            Request::PickFolder => self.pick_folder(window),
            Request::WriteFile { path, bytes } => self.write(&path, &bytes),
            Request::Fetch { url, purpose } => self.fetch(&url, purpose),
        }
    }

    fn open(&self, purpose: FilePurpose, extensions: &[&str], window: &Window) {
        let pattern = extensions.join(";");
        let name: Vec<String> = extensions.iter().map(|e| format!(".{e}")).collect();
        let name = name.join(" ");
        let filters = [DialogFileFilter {
            name: &name,
            pattern: &pattern,
        }];
        let inbox = self.inbox.clone();
        let result = dialog::show_open_file_dialog(
            &filters,
            None::<&Path>,
            false,
            window,
            Box::new(move |result, _| {
                if let Some(path) = chosen(result) {
                    inbox.read_later(path, purpose);
                }
            }),
        );
        if let Err(e) = result {
            log::error!("open dialog: {e}");
        }
    }

    fn save(&self, name: &str, bytes: Vec<u8>, window: &Window) {
        let ext = Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("*")
            .to_owned();
        let label = format!(".{ext}");
        let filters = [DialogFileFilter {
            name: &label,
            pattern: &ext,
        }];
        // SDL never frees a dialog's callback, so the bytes are taken out of
        // it rather than kept inside.
        let bytes = Mutex::new(Some(bytes));
        let wanted = ext.clone();
        let inbox = self.inbox.clone();
        let result = dialog::show_save_file_dialog(
            &filters,
            Some(name),
            window,
            Box::new(move |result, _| {
                // Taken first, so a cancel frees them too.
                let Some(bytes) = bytes.lock().ok().and_then(|mut b| b.take()) else {
                    return;
                };
                let Some(mut path) = chosen(result) else {
                    return;
                };
                if path.extension().is_none() && wanted != "*" {
                    path.set_extension(&wanted);
                }
                match std::fs::write(&path, bytes) {
                    Ok(()) => log::info!("saved {}", path.display()),
                    Err(e) => inbox.error(format!("{}: {e}", path.display())),
                }
            }),
        );
        if let Err(e) = result {
            log::error!("save dialog: {e}");
        }
    }

    fn pick_folder(&self, window: &Window) {
        let inbox = self.inbox.clone();
        dialog::show_open_folder_dialog(
            None::<&Path>,
            false,
            window,
            Box::new(move |result, _| {
                // An empty path tells the app the picker was cancelled.
                let path = chosen(result)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                inbox.push(InputEvent::Folder { path });
            }),
        );
    }

    /// Writes into the picked folder; an absolute path is written as is.
    fn write(&mut self, path: &str, bytes: &[u8]) {
        if self.write_failed {
            return;
        }
        let path = Path::new(path);
        let target = if path.is_absolute() {
            path.to_owned()
        } else if let Some(folder) = &self.folder {
            folder.join(path)
        } else {
            log::error!("no folder picked to write {} into", path.display());
            return;
        };
        if let Err(e) = std::fs::write(&target, bytes) {
            self.write_failed = true;
            self.inbox.error(format!("{}: {e}", target.display()));
        }
    }

    /// The gallery's samples, by their path next to the web page: the
    /// thumbnails are built in, the casts downloaded once and kept (see
    /// `gallery`). Nothing else is fetched.
    fn fetch(&self, url: &str, purpose: FilePurpose) {
        match gallery::wanted(url) {
            Some(Wanted::Thumbnail(sample)) => self.inbox.push(InputEvent::File {
                purpose,
                name: format!("{}.png", sample.id),
                bytes: gallery::thumbnail(sample),
            }),
            Some(Wanted::Cast(sample)) => {
                // Without a cache folder, kept in the temporary one.
                let folder = gallery::folder()
                    .unwrap_or_else(|| std::env::temp_dir().join("splatstudy-samples"));
                self.inbox.cast_later(folder, sample, purpose);
            }
            None => {
                log::error!("{url} is not in the gallery: not fetched");
                let name = url.rsplit('/').next().unwrap_or(url).to_owned();
                self.inbox.push(InputEvent::File {
                    purpose,
                    name,
                    bytes: Arc::from([]),
                });
            }
        }
    }
}

/// The single path a dialog returned, or `None` when it was cancelled.
fn chosen(result: Result<Vec<PathBuf>, DialogError>) -> Option<PathBuf> {
    match result {
        Ok(paths) => paths.into_iter().next(),
        Err(DialogError::Canceled) => None,
        Err(e) => {
            log::error!("dialog: {e}");
            None
        }
    }
}

/// Reads a whole file straight into a shared buffer, without a second copy.
/// No memory for it is an error, not an abort.
pub fn read(path: &Path) -> std::io::Result<Arc<[u8]>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len > MAX_FILE {
        return Err(std::io::Error::other(format!("{len} bytes is too large")));
    }
    let len = usize::try_from(len).map_err(std::io::Error::other)?;
    // Allocating can't fail without aborting, but reserving can: the room
    // is reserved (and given back) first, so the buffer then fits in it.
    Vec::<u8>::new()
        .try_reserve_exact(len.saturating_add(64))
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::OutOfMemory))?;
    let mut bytes: Arc<[u8]> = std::iter::repeat_n(0, len).collect();
    file.read_exact(Arc::get_mut(&mut bytes).expect("new buffer"))?;
    Ok(bytes)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
