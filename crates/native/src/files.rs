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
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use sdl3::EventSubsystem;
use sdl3::dialog::{self, DialogError, DialogFileFilter};
use sdl3::event::EventSender;
use sdl3::video::Window;
use splatstudy_core::{FilePurpose, InputEvent, Request};

/// The custom event that wakes the main loop.
struct Wake;

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

    /// Reads a file on a worker thread and delivers it.
    fn read_later(&self, path: PathBuf, purpose: FilePurpose) {
        let inbox = self.clone();
        std::thread::spawn(move || match read(&path) {
            Ok(bytes) => {
                log::info!("read {}: {} bytes", path.display(), bytes.len());
                inbox.push(InputEvent::File {
                    purpose,
                    name: file_name(&path),
                    bytes,
                });
            }
            Err(e) => log::error!("could not read {}: {e}", path.display()),
        });
    }
}

pub struct Files {
    inbox: Inbox,
    /// The folder picked last, for `WriteFile`.
    folder: Option<PathBuf>,
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
                self.folder = Some(PathBuf::from(path));
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
        let result = dialog::show_save_file_dialog(
            &filters,
            Some(name),
            window,
            Box::new(move |result, _| {
                let Some(mut path) = chosen(result) else {
                    return;
                };
                let Some(bytes) = bytes.lock().ok().and_then(|mut b| b.take()) else {
                    return;
                };
                if path.extension().is_none() && wanted != "*" {
                    path.set_extension(&wanted);
                }
                match std::fs::write(&path, bytes) {
                    Ok(()) => log::info!("saved {}", path.display()),
                    Err(e) => log::error!("could not save {}: {e}", path.display()),
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
                if let Some(path) = chosen(result) {
                    inbox.push(InputEvent::Folder {
                        path: path.to_string_lossy().into_owned(),
                    });
                }
            }),
        );
    }

    /// Writes into the picked folder; an absolute path is written as is.
    fn write(&self, path: &str, bytes: &[u8]) {
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
            log::error!("could not write {}: {e}", target.display());
        }
    }

    /// Samples ship next to the executable on desktop, if at all.
    fn fetch(&self, url: &str, purpose: FilePurpose) {
        let relative = Path::new(url);
        let plain = relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
        let base = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_owned));
        match base {
            Some(base) if plain => self.inbox.read_later(base.join(relative), purpose),
            _ => log::error!("cannot fetch {url} on desktop"),
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
fn read(path: &Path) -> std::io::Result<Arc<[u8]>> {
    let mut file = File::open(path)?;
    let len = usize::try_from(file.metadata()?.len()).map_err(std::io::Error::other)?;
    let mut bytes: Arc<[u8]> = std::iter::repeat_n(0, len).collect();
    file.read_exact(Arc::get_mut(&mut bytes).expect("new buffer"))?;
    Ok(bytes)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
