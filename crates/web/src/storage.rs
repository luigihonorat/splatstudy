// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Storage in the browser: autosave in IndexedDB and preferences in
//! localStorage.
//!
//! localStorage can be missing or throw (private windows, blocked cookies),
//! so every access here fails quietly and the app runs as on a first visit.

use web_sys::Storage;

/// Set after the first visit.
const SEEN: &str = "splatstudy.seen";

fn local() -> Option<Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

pub fn get(key: &str) -> Option<String> {
    local()?.get_item(key).ok().flatten()
}

pub fn set(key: &str, value: &str) {
    if let Some(storage) = local()
        && storage.set_item(key, value).is_err()
    {
        log::warn!("could not store {key}");
    }
}

/// True on the first visit to the page, and from then on false.
pub fn first_run() -> bool {
    let first = get(SEEN).is_none();
    if first {
        set(SEEN, "1");
    }
    first
}
