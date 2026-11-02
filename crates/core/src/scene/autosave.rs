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
//! 2. It watches `App::scene_revision` after every frame. About two
//!    seconds after the last change, when `App::autosave_ready(false)`
//!    says no stroke or export is under way, it takes
//!    `App::autosave_bytes` and stores them; when the page or window is
//!    going away it does the same with `autosave_ready(true)` at once.
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
