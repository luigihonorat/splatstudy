// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Files in the browser: drag and drop, the file picker, downloads and
//! fetching shipped samples, for the core's file events and requests.
//!
//! Files are read in slices straight into one shared buffer, so a splat of
//! several hundred MB is copied once into wasm memory, and the page keeps
//! handling events between slices.

use std::cell::Cell;
use std::mem::MaybeUninit;
use std::rc::Rc;
use std::sync::Arc;

use js_sys::{Array, Uint8Array};
use splatstudy_core::{App, FilePurpose, InputEvent, Request};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, BlobPropertyBag, Document, DragEvent, File, HtmlAnchorElement, HtmlInputElement,
    Response, Url,
};

/// Hands a file event to the app. Never called while the shell is borrowed.
pub type Deliver = Rc<dyn Fn(InputEvent)>;

/// How much of a file is read per step.
const SLICE: usize = 16 << 20;
/// How long a download's object URL is kept before it is revoked, in ms.
const REVOKE_AFTER_MS: i32 = 60_000;
/// Larger files aren't read: the buffer alone could exhaust the page's
/// memory, and no reference or scene is that big.
const MAX_FILE: usize = 1 << 30;

thread_local! {
    /// The newest read or fetch. A slow read that finishes after a newer
    /// one has started is dropped instead of replacing it.
    static LATEST: Cell<u64> = const { Cell::new(0) };
}

/// Starts a read, making every older one stale.
fn ticket() -> u64 {
    LATEST.with(|l| {
        l.set(l.get() + 1);
        l.get()
    })
}

fn is_latest(ticket: u64) -> bool {
    LATEST.with(|l| l.get() == ticket)
}

/// A buffer of `len` bytes to fill, or `None` when there is no memory for
/// it. Allocating can't fail without aborting the page, but reserving can:
/// the room is reserved (and given back) first, so the buffer then fits in
/// it.
pub fn buffer(len: usize) -> Option<Arc<[MaybeUninit<u8>]>> {
    // Room for the shared buffer's counts too.
    Vec::<u8>::new()
        .try_reserve_exact(len.checked_add(64)?)
        .ok()?;
    Some(Arc::new_uninit_slice(len))
}

pub struct Files {
    document: Document,
    /// A hidden `<input type=file>`, reused for every open request.
    picker: HtmlInputElement,
    /// The purpose of the open request the picker is answering.
    purpose: Rc<Cell<FilePurpose>>,
    deliver: Deliver,
}

impl Files {
    /// Creates the picker and starts accepting files dropped on the window.
    pub fn new(document: &Document, deliver: Deliver) -> Result<Self, JsValue> {
        let window: web_sys::EventTarget = web_sys::window().ok_or("no window")?.into();
        // The page must claim a drag for the drop to reach it; otherwise the
        // browser opens the file in place of the app.
        for name in ["dragenter", "dragover"] {
            crate::on(&window, name, false, |e: DragEvent| {
                e.prevent_default();
                if let Some(data) = e.data_transfer() {
                    data.set_drop_effect("copy");
                }
            })?;
        }
        let d = deliver.clone();
        crate::on(&window, "drop", false, move |e: DragEvent| {
            e.prevent_default();
            // The list is only readable during the event; the reads happen later.
            let files = e
                .data_transfer()
                .and_then(|data| data.files())
                .map(|list| (0..list.length()).filter_map(|i| list.get(i)).collect())
                .unwrap_or_default();
            read_all(files, FilePurpose::Any, d.clone());
        })?;

        let picker: HtmlInputElement = document.create_element("input")?.dyn_into()?;
        picker.set_type("file");
        picker.style().set_property("display", "none")?;
        document.body().ok_or("no body")?.append_child(&picker)?;
        let purpose = Rc::new(Cell::new(FilePurpose::Any));
        let (p, d, input) = (purpose.clone(), deliver.clone(), picker.clone());
        crate::on(&picker, "change", true, move |_: web_sys::Event| {
            let file = input.files().and_then(|list| list.get(0));
            // Cleared, so picking the same file again still fires `change`.
            input.set_value("");
            read_all(file.into_iter().collect(), p.get(), d.clone());
        })?;

        Ok(Self {
            document: document.clone(),
            picker,
            purpose,
            deliver,
        })
    }

    /// Does what the app asked for after a frame.
    pub fn handle(&self, request: Request) {
        let result = match request {
            Request::OpenFile {
                purpose,
                extensions,
            } => self.open(purpose, extensions),
            Request::SaveFile { name, mime, bytes } => self.download(&name, mime, &bytes),
            Request::Fetch { url, purpose } => {
                fetch(url, purpose, self.deliver.clone());
                Ok(())
            }
            Request::PickFolder => {
                log::warn!("picking a folder is desktop only; ignored on the web");
                Ok(())
            }
            Request::WriteFile { path, .. } => {
                log::warn!("writing {path} is desktop only; ignored on the web");
                Ok(())
            }
        };
        if let Err(err) = result {
            log::error!("file request failed: {err:?}");
        }
    }

    /// Opens the browser's file picker. Browsers allow this only shortly
    /// after a click or a key press, which is when menus ask for it.
    fn open(&self, purpose: FilePurpose, extensions: &[&str]) -> Result<(), JsValue> {
        self.purpose.set(purpose);
        let accept: Vec<String> = extensions.iter().map(|e| format!(".{e}")).collect();
        self.picker.set_accept(&accept.join(","));
        self.picker.click();
        Ok(())
    }

    /// Saves bytes as a download: a Blob, an object URL and a clicked link.
    fn download(&self, name: &str, mime: &str, bytes: &[u8]) -> Result<(), JsValue> {
        let options = BlobPropertyBag::new();
        options.set_type(mime);
        // SAFETY: the view points into wasm memory, which is only invalid
        // after the memory grows. Nothing allocates between here and the
        // Blob constructor, which copies the bytes before it returns.
        let blob = unsafe {
            let view = Uint8Array::view(bytes);
            Blob::new_with_u8_array_sequence_and_options(&Array::of1(&view), &options)?
        };
        let url = Url::create_object_url_with_blob(&blob)?;
        let link: HtmlAnchorElement = self.document.create_element("a")?.dyn_into()?;
        link.set_href(&url);
        link.set_download(name);
        // Some browsers only follow links that are in the document.
        let body = self.document.body().ok_or("no body")?;
        body.append_child(&link)?;
        link.click();
        link.remove();
        // Revoking right away can cancel the download in some browsers.
        let revoke = Closure::once_into_js(move || {
            let _ = Url::revoke_object_url(&url);
        });
        web_sys::window()
            .ok_or("no window")?
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                revoke.unchecked_ref(),
                REVOKE_AFTER_MS,
            )?;
        Ok(())
    }
}

/// Reads files one after the other and hands each to the app.
fn read_all(files: Vec<File>, purpose: FilePurpose, deliver: Deliver) {
    if files.is_empty() {
        return;
    }
    let ticket = ticket();
    wasm_bindgen_futures::spawn_local(async move {
        for file in files {
            let name = file.name();
            if !App::can_open(purpose, &name) {
                // Not read at all: the app says it can't open it.
                deliver(InputEvent::File {
                    purpose,
                    name,
                    bytes: Arc::from([]),
                });
                continue;
            }
            let start = js_sys::Date::now();
            let read = read_blob(&file).await;
            if !is_latest(ticket) {
                log::info!("{name} was replaced by a newer file before it was read");
                return;
            }
            match read {
                Ok(bytes) => {
                    log::info!(
                        "read {name}: {} bytes in {} ms",
                        bytes.len(),
                        js_sys::Date::now() - start
                    );
                    deliver(InputEvent::File {
                        purpose,
                        name,
                        bytes,
                    });
                }
                Err(err) => {
                    log::error!("could not read {name}: {err:?}");
                    // Empty, so the app says it can't open this file.
                    deliver(InputEvent::File {
                        purpose,
                        name,
                        bytes: Arc::from([]),
                    });
                }
            }
        }
    });
}

/// Fetches a file shipped next to the page and hands it to the app, empty
/// if it couldn't be fetched, so the app can say so. Thumbnails come many
/// at once and don't make each other, or a sample on its way, stale.
fn fetch(url: String, purpose: FilePurpose, deliver: Deliver) {
    let ticket = purpose.supersedes().then(ticket);
    wasm_bindgen_futures::spawn_local(async move {
        let fetched = fetch_bytes(&url).await;
        if ticket.is_some_and(|t| !is_latest(t)) {
            return;
        }
        let bytes = fetched.unwrap_or_else(|err| {
            log::error!("could not fetch {url}: {err:?}");
            Arc::from([])
        });
        deliver(InputEvent::File {
            purpose,
            name: file_name(&url).to_owned(),
            bytes,
        });
    });
}

async fn fetch_bytes(url: &str) -> Result<Arc<[u8]>, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    // A relative URL resolves against the page, so samples sit next to it.
    let response: Response = JsFuture::from(window.fetch_with_str(url))
        .await?
        .dyn_into()?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()).into());
    }
    let body = JsFuture::from(response.array_buffer()?).await?;
    let view = Uint8Array::new(&body);
    if view.length() as usize > MAX_FILE {
        return Err("file too large".into());
    }
    let mut bytes = buffer(view.length() as usize).ok_or("no memory for the file")?;
    view.copy_to_uninit(Arc::get_mut(&mut bytes).expect("new buffer"));
    // SAFETY: `copy_to_uninit` filled the whole buffer.
    Ok(unsafe { bytes.assume_init() })
}

/// Reads a file or blob slice by slice into one buffer. Each slice is a
/// separate await, so the page stays responsive and the browser never holds
/// a second full copy of a large file. The buffer isn't zeroed first: on a
/// large file that alone stalls the page.
async fn read_blob(blob: &Blob) -> Result<Arc<[u8]>, JsValue> {
    let size = blob.size() as usize;
    if size > MAX_FILE {
        return Err(format!("{size} bytes is more than the {MAX_FILE} allowed").into());
    }
    let mut bytes = buffer(size).ok_or("no memory for the file")?;
    let out = Arc::get_mut(&mut bytes).expect("new buffer");
    let mut start = 0;
    while start < size {
        let end = (start + SLICE).min(size);
        let part = blob.slice_with_f64_and_f64(start as f64, end as f64)?;
        let buffer = JsFuture::from(part.array_buffer()).await?;
        let view = Uint8Array::new(&buffer);
        if view.length() as usize != end - start {
            return Err("the file changed while it was read".into());
        }
        view.copy_to_uninit(&mut out[start..end]);
        start = end;
    }
    // SAFETY: the slices cover 0..size without gaps, and each was filled.
    Ok(unsafe { bytes.assume_init() })
}

/// The last path segment of a URL, without its query or fragment.
fn file_name(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/').next().unwrap_or(path)
}
