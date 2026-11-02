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
// The scene and the blob it names in one go: other blobs are dropped, so
// only the file the latest scene needs stays. When the page is leaving,
// the write starts before this returns and is committed at once, leaving
// the clean-up to the next save: a page being reloaded doesn't wait.
export function idb_put_scene(bytes, keep, leaving) {
  const copy = bytes.slice();
  const write = d => {
    const tx = d.transaction(['scene', 'blobs'], 'readwrite');
    tx.objectStore('scene').put(copy, 'current');
    if (leaving) {
      if (tx.commit) tx.commit();
    } else {
      const blobs = tx.objectStore('blobs');
      blobs.getAllKeys().onsuccess = e => {
        for (const k of e.target.result) if (k !== keep) blobs.delete(k);
      };
    }
    return finished(tx);
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

/// Opens the last session's autosave in `app`, unless the URL has
/// `?fresh`. Returns the URL options still to apply: without `sample=`
/// when the session came back with its own reference, which the sample
/// would replace.
pub async fn restore(app: &mut App, search: &str) -> String {
    if crate::query_value(search, "fresh").is_some() {
        return search.to_owned();
    }
    let scene = match idb_scene().await {
        Ok(v) => match v.dyn_into::<Uint8Array>() {
            Ok(view) => view.to_vec(),
            Err(_) => return search.to_owned(),
        },
        Err(e) => {
            log::warn!("no autosave to restore: {}", error_name(&e));
            return search.to_owned();
        }
    };
    let key = scene::reference_key(&scene).ok().flatten();
    let reference = match key {
        Some(key) => match idb_blob(&key.name()).await {
            Ok(v) => v.dyn_into::<Uint8Array>().ok().map(|view| to_arc(&view)),
            Err(e) => {
                log::warn!("autosave's reference: {}", error_name(&e));
                None
            }
        },
        None => None,
    };
    let with_reference = reference.is_some();
    if !app.restore_autosave(&scene, reference) {
        return search.to_owned();
    }
    if with_reference {
        // Known to be stored: a save as the page closes needn't ask.
        RESTORED_BLOB.set(key.map(|key| key.name()));
    }
    log::info!("restored the autosave ({} bytes)", scene.len());
    if !with_reference {
        return search.to_owned();
    }
    let kept: Vec<&str> = search
        .trim_start_matches('?')
        .split('&')
        .filter(|pair| !pair.is_empty() && !pair.starts_with("sample="))
        .collect();
    kept.join("&")
}

thread_local! {
    /// The reference file the restored autosave named, for the autosave.
    static RESTORED_BLOB: Cell<Option<String>> = const { Cell::new(None) };
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
    /// The revision stored, or restored.
    stored: u64,
    /// A save is being written.
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
                stored: revision,
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
        let revision = app.scene_revision();
        let t = now();
        if revision != st.seen {
            st.seen = revision;
            st.changed_at = t;
        }
        let quiet = leaving || t - st.changed_at >= QUIET;
        if !st.on || st.writing || revision == st.stored || !quiet {
            return;
        }
        if !app.autosave_ready(leaving) {
            return;
        }
        let bytes = app.autosave_bytes();
        st.writing = true;
        drop(st);
        let this = self.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = this.write(bytes, leaving).await;
            let mut st = this.state.borrow_mut();
            st.writing = false;
            match result {
                Ok(()) => st.stored = revision,
                Err(trouble) => {
                    st.on = false;
                    st.trouble = Some(trouble);
                }
            }
        });
    }

    /// Stores a save, or clears the stores when there is nothing to keep.
    async fn write(&self, bytes: Option<Bytes>, leaving: bool) -> Result<(), Trouble> {
        let Some(save) = bytes else {
            idb_clear().await.map_err(trouble)?;
            self.state.borrow_mut().stored_blob = None;
            log::info!("autosave cleared");
            return Ok(());
        };
        if save.sculpted && !std::mem::replace(&mut self.state.borrow_mut().asked_persist, true) {
            storage_persist();
        }
        let keep = match &save.reference {
            Some((key, file)) => self.store_blob(*key, file).await?,
            None => None,
        };
        let start = now();
        idb_put_scene(&save.scene, keep.clone(), leaving)
            .await
            .map_err(trouble)?;
        log::info!(
            "autosaved {} bytes in {:.0} ms",
            save.scene.len(),
            (now() - start) * 1000.0
        );
        self.state.borrow_mut().stored_blob = keep;
        Ok(())
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
