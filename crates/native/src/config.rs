// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! What the desktop app keeps between runs, in the OS's folder for app data
//! (SDL's pref path, such as `%APPDATA%\splatstudy` on Windows).
//! Failing to read or write it is never fatal.

use std::path::PathBuf;

/// The app's own folder for settings, created on first use.
pub fn folder() -> Option<PathBuf> {
    match sdl3::filesystem::get_pref_path("", "splatstudy") {
        Ok(path) => Some(path),
        Err(e) => {
            log::warn!("no folder for settings: {e}");
            None
        }
    }
}

/// True on the first run, and from then on false.
pub fn first_run() -> bool {
    let Some(marker) = folder().map(|f| f.join("seen")) else {
        return true;
    };
    if marker.exists() {
        return false;
    }
    if let Err(e) = std::fs::write(&marker, b"") {
        log::warn!("could not write {}: {e}", marker.display());
    }
    true
}

/// The preferences saved last, if any.
pub fn load_preferences() -> Option<String> {
    std::fs::read_to_string(folder()?.join("preferences")).ok()
}

pub fn save_preferences(preferences: &str) {
    let Some(path) = folder().map(|f| f.join("preferences")) else {
        return;
    };
    if let Err(e) = std::fs::write(&path, preferences) {
        log::warn!("could not write {}: {e}", path.display());
    }
}
