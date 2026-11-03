// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Opt-in timings for measuring the web build: off unless the page was
//! opened with `?timing`. Then every drawn frame appends
//! `[start ms, frame ms, wasm memory bytes]` to `window.splatstudyTiming`,
//! and slow work outside frames (the autosave) logs its time.

use std::cell::RefCell;

use wasm_bindgen::JsValue;

thread_local! {
    static FRAMES: RefCell<Option<js_sys::Array>> = const { RefCell::new(None) };
}

/// Turns the timings on if the query string has `timing`.
pub fn init(on: bool) {
    if !on {
        return;
    }
    let frames = js_sys::Array::new();
    if let Some(window) = web_sys::window() {
        let _ = js_sys::Reflect::set(&window, &"splatstudyTiming".into(), &frames);
    }
    FRAMES.with(|f| *f.borrow_mut() = Some(frames));
}

pub fn on() -> bool {
    FRAMES.with(|f| f.borrow().is_some())
}

/// Records a frame that started at `start` and took `seconds`.
pub fn frame(start: f64, seconds: f64) {
    FRAMES.with(|f| {
        if let Some(frames) = &*f.borrow() {
            let entry = js_sys::Array::of3(
                &JsValue::from(start * 1000.0),
                &JsValue::from(seconds * 1000.0),
                &JsValue::from(memory_bytes() as f64),
            );
            frames.push(&entry);
        }
    });
}

/// Logs how long `what` took, when timings are on.
pub fn note(what: &str, seconds: f64) {
    if on() {
        log::info!("timing: {what} {:.1} ms", seconds * 1000.0);
    }
}

/// The wasm instance's linear memory: it grows and never shrinks.
fn memory_bytes() -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        core::arch::wasm32::memory_size::<0>() * 65536
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0
    }
}
