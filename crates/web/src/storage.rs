// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Storage in the browser: autosave in IndexedDB and preferences in
//! localStorage.
//!
//! localStorage can be missing or throw (private windows, blocked cookies),
//! so every access here fails quietly and the app runs as on a first visit.
//!
//! Autosave keeps two object stores in the `splatstudy` database: `scene`
//! holds the current scene under one key, its reference named by a content
//! key, and `blobs` holds that reference file under its key, so a big file
//! is written once rather than with every save. The core makes the bytes
//! and says when it is ready (see `splatstudy_core::scene::autosave`); this
//! decides when to save, and stores.
//!
//! Several tabs share the stores. The last one to save wins, but none
//! loses its work quietly: each tells the others when it wrote, and they
//! then save again at their next change, or as they close.
//!
//! A restore that crashes the tab would crash it again on every reload, so
//! a flag in localStorage marks a restore under way until its first frame
//! is drawn. Found at start, the restore is only offered.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use js_sys::Uint8Array;
use splatstudy_core::App;
use splatstudy_core::scene::autosave::{Autosave as Bytes, Trouble};
use splatstudy_core::scene::{self, BlobKey};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::Storage;

/// The app's preferences string (language, theme, navigation).
pub const PREFERENCES: &str = "splatstudy.preferences";
/// Set after the first visit.
const SEEN: &str = "splatstudy.seen";
/// Set while an autosave is being restored, until a frame is drawn.
const RESTORING: &str = "splatstudy.restoring";

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

fn remove(key: &str) {
    if let Some(storage) = local() {
        let _ = storage.remove_item(key);
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

// IndexedDB through a few promises. Bytes passed in are views into wasm
// memory, valid only until it grows: each function copies them before its
// first await (a view also clones its whole buffer when stored).
#[wasm_bindgen(inline_js = r#"
let opened = null;
// The connection once open, to start a write without waiting a turn.
let openNow = null;
const done = r => new Promise((ok, fail) => {
  r.onsuccess = () => ok(r.result);
  r.onerror = () => fail(r.error);
});
const finished = tx => new Promise((ok, fail) => {
  tx.oncomplete = () => ok();
  tx.onerror = () => fail(tx.error);
  tx.onabort = () => fail(tx.error || new DOMException('aborted', 'AbortError'));
});
function db() {
  if (!opened) {
    opened = new Promise((ok, fail) => {
      if (typeof indexedDB === 'undefined') throw new DOMException('no IndexedDB', 'NotSupportedError');
      const r = indexedDB.open('splatstudy', 1);
      r.onupgradeneeded = () => {
        r.result.createObjectStore('scene');
        r.result.createObjectStore('blobs');
      };
      r.onsuccess = () => {
        openNow = r.result;
        // Another tab upgrading the database: step aside, open again later.
        openNow.onversionchange = () => { openNow.close(); openNow = null; opened = null; };
        ok(r.result);
      };
      r.onerror = () => fail(r.error);
      r.onblocked = () => fail(new DOMException('blocked by another tab', 'InvalidStateError'));
    });
    opened.catch(() => { opened = null; });
  }
  return opened;
}
export async function idb_scene() {
  const d = await db();
  const v = await done(d.transaction('scene').objectStore('scene').get('current'));
  return v instanceof Uint8Array ? v : null;
}
export async function idb_blob(key) {
  const d = await db();
  const v = await done(d.transaction('blobs').objectStore('blobs').get(key));
  return v instanceof Uint8Array ? v : null;
}
export async function idb_has_blob(key) {
  const d = await db();
  return (await done(d.transaction('blobs').objectStore('blobs').count(key))) > 0;
}
export function idb_put_blob(key, bytes) {
  const copy = bytes.slice();
  return db().then(d => {
    const tx = d.transaction('blobs', 'readwrite');
    tx.objectStore('blobs').put(copy, key);
    return finished(tx);
  });
}
// The scene and the blob it names in one go: the blob `keep` must still be
// there (another tab may have dropped it), or nothing is written and this
// fails with a NotFoundError; other blobs are dropped, so only the file the
// latest scene needs stays. When the page is leaving, the scene is put
// alone and committed at once, leaving the checks and the clean-up to the
// next save: a page being reloaded doesn't wait.
export function idb_put_scene(bytes, keep, leaving) {
  const copy = bytes.slice();
  const write = d => {
    const tx = d.transaction(['scene', 'blobs'], 'readwrite');
    const scene = tx.objectStore('scene');
    const blobs = tx.objectStore('blobs');
    let missing = false;
    if (leaving) {
      scene.put(copy, 'current');
      if (tx.commit) tx.commit();
    } else {
      const put = () => {
        scene.put(copy, 'current');
        blobs.getAllKeys().onsuccess = e => {
          for (const k of e.target.result) if (k !== keep) blobs.delete(k);
        };
      };
      if (keep == null) {
        put();
      } else {
        blobs.count(keep).onsuccess = e => {
          if (e.target.result > 0) {
            put();
          } else {
            missing = true;
            tx.abort();
          }
        };
      }
    }
    return finished(tx).catch(err => {
      throw missing ? new DOMException('the reference file is gone', 'NotFoundError') : err;
    });
  };
  return openNow ? write(openNow) : db().then(write);
}
export function idb_clear() {
  return db().then(d => {
    const tx = d.transaction(['scene', 'blobs'], 'readwrite');
    tx.objectStore('scene').clear();
    tx.objectStore('blobs').clear();
    return finished(tx);
  });
}
// Bytes the page may still store; Infinity when the browser won't say.
export async function storage_room() {
  try {
    const e = await navigator.storage.estimate();
    if (typeof e.quota === 'number' && typeof e.usage === 'number') return e.quota - e.usage;
  } catch (_) {}
  return Infinity;
}
export function storage_persist() {
  try {
    navigator.storage.persist().then(
      granted => console.info('persistent storage ' + (granted ? 'granted' : 'refused')),
      () => {});
  } catch (_) {}
}
// The other tabs of the page, told when this one wrote the stores.
let channel = null;
export function tabs_listen(heard) {
  try {
    channel = new BroadcastChannel('splatstudy.autosave');
    channel.onmessage = () => heard();
  } catch (_) {}
}
export function tabs_tell() {
  try { if (channel) channel.postMessage('written'); } catch (_) {}
}
export function error_name(e) {
  return String((e && e.name) || e);
}
export function page_hidden() {
  return document.visibilityState === 'hidden';
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn idb_scene() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn idb_blob(key: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn idb_has_blob(key: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn idb_put_blob(key: &str, bytes: &[u8]) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn idb_put_scene(
        bytes: &[u8],
        keep: Option<String>,
        leaving: bool,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn idb_clear() -> Result<JsValue, JsValue>;
    async fn storage_room() -> JsValue;
    fn storage_persist();
    fn tabs_listen(heard: &Closure<dyn FnMut()>);
    fn tabs_tell();
    fn error_name(error: &JsValue) -> String;
    fn page_hidden() -> bool;
}

/// Seconds without a change before saving.
const QUIET: f64 = 2.0;
/// How often the revision is looked at, in ms.
const POLL_MS: i32 = 500;
/// Room kept free beyond the file itself when deciding whether it fits.
const ROOM_MARGIN: f64 = 16.0 * 1024.0 * 1024.0;

/// Copies a JS byte array into a buffer of our own.
fn to_arc(view: &Uint8Array) -> Arc<[u8]> {
    let mut bytes = Arc::<[u8]>::new_uninit_slice(view.length() as usize);
    view.copy_to_uninit(Arc::get_mut(&mut bytes).expect("new buffer"));
    // SAFETY: `copy_to_uninit` filled the whole buffer.
    unsafe { bytes.assume_init() }
}

thread_local! {
    /// The reference file the restored autosave named, for the autosave.
    static RESTORED_BLOB: Cell<Option<String>> = const { Cell::new(None) };
    /// A restore was applied; the flag goes once a frame shows it.
    static AWAIT_FRAME: Cell<bool> = const { Cell::new(false) };
}

/// The shell drew a frame: a restore before it didn't crash the tab.
pub fn frame_drawn() {
    if AWAIT_FRAME.take() {
        remove(RESTORING);
    }
}

/// How the last session comes back at start.
pub enum Start {
    /// `?fresh`, or nothing stored.
    Nothing,
    /// A restore was under way when the tab last went: perhaps it crashed
    /// it. The app only offers it.
    Offer,
    /// The stored scene, to restore once a first frame is up.
    Scene(Vec<u8>),
}

/// Reads the last session's scene, unless the URL has `?fresh` or the
/// last restore never finished.
pub async fn begin_restore(search: &str) -> Start {
    if crate::query_value(search, "fresh").is_some() {
        return Start::Nothing;
    }
    let scene = read_scene().await;
    if get(RESTORING).is_some() {
        if scene.is_none() {
            remove(RESTORING);
            return Start::Nothing;
        }
        log::warn!("the last restore didn't finish; offering it instead");
        return Start::Offer;
    }
    match scene {
        Some(scene) => Start::Scene(scene),
        None => Start::Nothing,
    }
}

/// The stored scene, if any.
async fn read_scene() -> Option<Vec<u8>> {
    match idb_scene().await {
        Ok(v) => v.dyn_into::<Uint8Array>().ok().map(|view| view.to_vec()),
        Err(e) => {
            log::warn!("no autosave to restore: {}", error_name(&e));
            None
        }
    }
}

/// The reference file a stored scene names, if it is stored.
async fn read_reference(key: Option<BlobKey>) -> Option<Arc<[u8]>> {
    match idb_blob(&key?.name()).await {
        Ok(v) => v.dyn_into::<Uint8Array>().ok().map(|view| to_arc(&view)),
        Err(e) => {
            log::warn!("autosave's reference: {}", error_name(&e));
            None
        }
    }
}

/// Runs a closure on the app, wherever it is kept.
type Visit<'a> = dyn FnMut(&mut dyn FnMut(&mut App)) + 'a;

/// Opens a stored scene in `app`, with its reference. Flags the restore
/// until a frame is drawn. The reference's key when it came back too, or
/// `None` when the scene didn't read.
async fn restore_scene(scene: &[u8], with_app: &mut Visit<'_>) -> Option<Option<String>> {
    set(RESTORING, "1");
    let key = scene::reference_key(scene).ok().flatten();
    let reference = read_reference(key).await;
    let with_reference = reference.is_some();
    let mut reference = Some(reference);
    let mut restored = false;
    with_app(&mut |app| restored = app.restore_autosave(scene, reference.take().flatten()));
    if !restored {
        remove(RESTORING);
        return None;
    }
    AWAIT_FRAME.set(true);
    log::info!("restored the autosave ({} bytes)", scene.len());
    Some(key.filter(|_| with_reference).map(|key| key.name()))
}

/// Opens the scene [`begin_restore`] read in `app`. Returns the URL
/// options still to apply: without `sample=` when the session came back
/// with its own reference, which the sample would replace.
pub async fn finish_restore(app: &mut App, scene: &[u8], search: &str) -> String {
    let restored = restore_scene(scene, &mut |f| f(app)).await;
    let Some(Some(blob)) = restored else {
        return search.to_owned();
    };
    // Known to be stored: a save as the page closes needn't ask.
    RESTORED_BLOB.set(Some(blob));
    let kept: Vec<&str> = search
        .trim_start_matches('?')
        .split('&')
        .filter(|pair| !pair.is_empty() && !pair.starts_with("sample="))
        .collect();
    kept.join("&")
}

/// Hands the app to a closure; the shell owns it.
pub type WithApp = Rc<dyn Fn(&mut dyn FnMut(&mut App))>;

/// Decides when to save, and remembers what is stored.
struct State {
    /// False once storing failed: the app goes on without autosave.
    on: bool,
    /// The revision last seen, and when it changed (seconds).
    seen: u64,
    changed_at: f64,
    /// The revision stored, or restored; `None` when another tab wrote
    /// since.
    stored: Option<u64>,
    /// Our revision when another tab last wrote: saved again once it
    /// changes, or as the page goes.
    overwritten_at: Option<u64>,
    /// Counts the other tabs' writes heard of.
    heard: u64,
    /// A save, or a restore, is being written or read.
    writing: bool,
    /// The blob key that is stored.
    stored_blob: Option<String>,
    /// A reference that didn't fit; it isn't tried again.
    too_big: Option<BlobKey>,
    /// `navigator.storage.persist()` was asked for.
    asked_persist: bool,
    /// To tell the app, from the next poll.
    trouble: Option<Trouble>,
}

impl State {
    /// True if `revision` isn't what the stores hold.
    fn unsaved(&self, revision: u64, leaving: bool) -> bool {
        match (self.stored, self.overwritten_at) {
            (Some(stored), _) => revision != stored,
            (None, Some(at)) => leaving || revision != at,
            (None, None) => true,
        }
    }

    /// A save of `revision` was written, while `heard` other writes had
    /// been heard of.
    fn saved(&mut self, revision: u64, heard: u64) {
        if self.heard == heard {
            self.stored = Some(revision);
            self.overwritten_at = None;
        } else {
            // Another tab wrote meanwhile, maybe after us: save again as
            // the page goes, or at the next change.
            self.stored = None;
            self.overwritten_at = Some(revision);
        }
    }
}

#[derive(Clone)]
pub struct Autosave {
    state: Rc<RefCell<State>>,
    with_app: WithApp,
}

/// Seconds on the page's clock.
fn now() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now() / 1000.0)
}

impl Autosave {
    /// Starts watching the app: a poll for quiet moments after a change,
    /// and a save when the page is hidden or left.
    pub fn start(with_app: WithApp) -> Result<(), JsValue> {
        let mut revision = 0;
        with_app(&mut |app| revision = app.scene_revision());
        let autosave = Self {
            state: Rc::new(RefCell::new(State {
                on: true,
                seen: revision,
                changed_at: now(),
                stored: Some(revision),
                overwritten_at: None,
                heard: 0,
                writing: false,
                stored_blob: RESTORED_BLOB.take(),
                too_big: None,
                asked_persist: false,
                trouble: None,
            })),
            with_app,
        };
        let window = web_sys::window().ok_or("no window")?;
        let poll = autosave.clone();
        let tick = Closure::<dyn FnMut()>::new(move || poll.poll(false));
        window.set_interval_with_callback_and_timeout_and_arguments_0(
            tick.as_ref().unchecked_ref(),
            POLL_MS,
        )?;
        tick.forget();
        // Another tab wrote: what is stored is no longer ours.
        let state = autosave.state.clone();
        let heard = Closure::<dyn FnMut()>::new(move || {
            let mut st = state.borrow_mut();
            st.heard += 1;
            st.stored = None;
            st.overwritten_at = Some(st.seen);
            st.stored_blob = None;
        });
        tabs_listen(&heard);
        heard.forget();
        let hidden = autosave.clone();
        let document = window.document().ok_or("no document")?;
        crate::on(
            &document,
            "visibilitychange",
            true,
            move |_: web_sys::Event| {
                if page_hidden() {
                    hidden.poll(true);
                }
            },
        )?;
        // `beforeunload` comes while a reload or navigation still waits on
        // the network, early enough for the write to finish; `pagehide`
        // also covers closing the tab and leaving for the back-forward
        // cache. Whichever comes second finds nothing left to save.
        for name in ["beforeunload", "pagehide"] {
            let leaving = autosave.clone();
            crate::on(&window, name, true, move |_: web_sys::Event| {
                leaving.poll(true)
            })?;
        }
        Ok(())
    }

    /// Saves if the scene changed and has been quiet long enough, or at
    /// once when the page is `leaving`.
    fn poll(&self, leaving: bool) {
        let this = self.clone();
        (self.with_app)(&mut |app| this.poll_app(app, leaving));
    }

    fn poll_app(&self, app: &mut App, leaving: bool) {
        let mut st = self.state.borrow_mut();
        if let Some(trouble) = st.trouble.take() {
            app.autosave_trouble(trouble);
        }
        if !st.writing && app.take_restore_request() {
            st.writing = true;
            drop(st);
            return self.restore_later();
        }
        let revision = app.scene_revision();
        let t = now();
        if revision != st.seen {
            st.seen = revision;
            st.changed_at = t;
        }
        let quiet = leaving || t - st.changed_at >= QUIET;
        if !st.on || !st.unsaved(revision, leaving) || !quiet {
            return;
        }
        if !app.autosave_ready(leaving) {
            return;
        }
        if st.writing {
            return;
        }
        let bytes = app.autosave_bytes();
        st.writing = true;
        let heard = st.heard;
        drop(st);
        let this = self.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = this.write(bytes.as_ref(), leaving).await;
            let mut st = this.state.borrow_mut();
            st.writing = false;
            match result {
                Ok(()) => {
                    st.saved(revision, heard);
                    drop(st);
                    remove(RESTORING);
                    tabs_tell();
                }
                Err(trouble) => {
                    st.on = false;
                    st.trouble = Some(trouble);
                }
            }
        });
    }

    /// Restores the stored scene in place of the open one, as the user
    /// asked. `writing` is set: no save runs meanwhile.
    fn restore_later(&self) {
        let this = self.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let restored = match read_scene().await {
                Some(scene) => restore_scene(&scene, &mut |f| (this.with_app)(f)).await,
                None => None,
            };
            let mut revision = None;
            (this.with_app)(&mut |app| {
                if restored.is_none() {
                    app.autosave_gone();
                }
                revision = Some(app.scene_revision());
            });
            let mut st = this.state.borrow_mut();
            st.writing = false;
            if let (Some(blob), Some(revision)) = (restored, revision) {
                st.seen = revision;
                st.stored = Some(revision);
                st.overwritten_at = None;
                st.stored_blob = blob;
            }
        });
    }

    /// Stores a save, or clears the stores when there is nothing to keep.
    async fn write(&self, save: Option<&Bytes>, leaving: bool) -> Result<(), Trouble> {
        let Some(save) = save else {
            idb_clear().await.map_err(trouble)?;
            self.state.borrow_mut().stored_blob = None;
            log::info!("autosave cleared");
            return Ok(());
        };
        if save.sculpted && !std::mem::replace(&mut self.state.borrow_mut().asked_persist, true) {
            storage_persist();
        }
        let start = now();
        let mut keep = self.keep(save).await?;
        let mut put = idb_put_scene(&save.scene, keep.clone(), leaving).await;
        if put
            .as_ref()
            .is_err_and(|e| error_name(e) == "NotFoundError")
        {
            // Another tab dropped the reference: store it again.
            log::info!("autosave: the reference was gone; storing it again");
            self.state.borrow_mut().stored_blob = None;
            keep = self.keep(save).await?;
            put = idb_put_scene(&save.scene, keep.clone(), leaving).await;
        }
        put.map_err(trouble)?;
        log::info!(
            "autosaved {} bytes in {:.0} ms",
            save.scene.len(),
            (now() - start) * 1000.0
        );
        self.state.borrow_mut().stored_blob = keep;
        Ok(())
    }

    /// The blob key a save keeps, its reference stored first if need be.
    async fn keep(&self, save: &Bytes) -> Result<Option<String>, Trouble> {
        match &save.reference {
            Some((key, file)) => self.store_blob(*key, file).await,
            None => Ok(None),
        }
    }

    /// Makes sure the reference file is stored under its key; the key to
    /// keep, or `None` when it can't be stored.
    async fn store_blob(&self, key: BlobKey, file: &[u8]) -> Result<Option<String>, Trouble> {
        let name = key.name();
        let (stored, too_big) = {
            let st = self.state.borrow();
            (
                st.stored_blob.as_deref() == Some(&name),
                st.too_big == Some(key),
            )
        };
        // Taken on trust: the scene's put checks it is still there.
        if stored || idb_has_blob(&name).await.ok().and_then(|v| v.as_bool()) == Some(true) {
            return Ok(Some(name));
        }
        if too_big {
            return Ok(None);
        }
        let room = storage_room().await.as_f64().unwrap_or(f64::INFINITY);
        let fits = room >= file.len() as f64 * 1.1 + ROOM_MARGIN;
        let start = now();
        let stored = if fits {
            idb_put_blob(&name, file).await.map_err(trouble)
        } else {
            Err(Trouble::Full)
        };
        match stored {
            Ok(_) => {
                log::info!(
                    "stored the reference, {} bytes, in {:.0} ms",
                    file.len(),
                    (now() - start) * 1000.0
                );
                Ok(Some(name))
            }
            // The clay is small; it may still fit without the reference.
            Err(Trouble::Full) => {
                log::warn!("no room for the reference ({room} bytes free)");
                let mut st = self.state.borrow_mut();
                st.too_big = Some(key);
                st.trouble = Some(Trouble::ReferenceTooBig);
                Ok(None)
            }
            Err(other) => Err(other),
        }
    }
}

/// What a failed storage call means for autosave.
fn trouble(error: JsValue) -> Trouble {
    let name = error_name(&error);
    log::warn!("autosave failed: {name}");
    if name == "QuotaExceededError" {
        Trouble::Full
    } else {
        Trouble::Unavailable
    }
}
