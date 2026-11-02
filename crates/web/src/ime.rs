// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Text input in the browser beyond single key presses: IME composition
//! (Japanese, Chinese, Korean), paste, and virtual keyboards.
//!
//! An IME only works in an editable element, never on a canvas. So while a
//! text field in the app is being edited, a small `<input>` takes the focus
//! and sits just under that field. While a composition is going on it is
//! visible and shows the text being composed, with the browser's own
//! underline, and the IME puts its candidate list next to it. The committed
//! text goes to the app as [`InputEvent::Text`]; the input is then emptied
//! and turns invisible again.
//!
//! Plain key presses still go through the window's `keydown` listener in
//! `lib.rs`, which skips keys that belong to a composition ([`is_ime_key`]).

use std::cell::Cell;
use std::rc::Rc;

use splatstudy_core::InputEvent;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{
    ClipboardEvent, CompositionEvent, Document, HtmlCanvasElement, HtmlInputElement, KeyboardEvent,
};

use crate::files::Deliver;

/// The height of the composition line under a field, in CSS pixels.
const LINE_HEIGHT: f32 = 26.0;

/// True for a key press that belongs to an IME: during a composition, or the
/// key that starts or ends one (reported as key code 229 by every browser).
pub fn is_ime_key(e: &KeyboardEvent) -> bool {
    e.is_composing() || e.key_code() == 229
}

/// Where the app's text field is and the colours to draw the composition
/// with, as the core reports them after a frame.
#[derive(Clone, Copy, PartialEq)]
pub struct Field {
    pub rect: [f32; 4],
    pub text: [u8; 3],
    pub background: [u8; 3],
    pub line: [u8; 3],
}

pub struct TextInput {
    input: HtmlInputElement,
    canvas: HtmlCanvasElement,
    /// The field the input was last placed under.
    placed: Cell<Option<Field>>,
}

impl TextInput {
    pub fn new(
        document: &Document,
        canvas: &HtmlCanvasElement,
        deliver: Deliver,
    ) -> Result<Self, JsValue> {
        let input: HtmlInputElement = document.create_element("input")?.dyn_into()?;
        input.set_type("text");
        for (name, value) in [
            ("autocomplete", "off"),
            ("autocorrect", "off"),
            ("autocapitalize", "off"),
            ("spellcheck", "false"),
            ("aria-hidden", "true"),
            ("tabindex", "-1"),
        ] {
            input.set_attribute(name, value)?;
        }
        let style = input.style();
        for (name, value) in [
            ("position", "fixed"),
            ("left", "0"),
            ("top", "0"),
            ("width", "1px"),
            ("height", "1px"),
            ("margin", "0"),
            ("padding", "0 6px"),
            ("box-sizing", "border-box"),
            ("border", "none"),
            ("outline", "none"),
            ("font", "300 14px system-ui, sans-serif"),
            // Focusable but unseen until a composition starts.
            ("opacity", "0"),
            ("pointer-events", "none"),
        ] {
            style.set_property(name, value)?;
        }
        document.body().ok_or("no body")?.append_child(&input)?;

        let composing = Rc::new(Cell::new(false));
        // Whether the last key press went to the IME rather than the app.
        let ime_key = Rc::new(Cell::new(false));

        let k = ime_key.clone();
        crate::on(&input, "keydown", true, move |e: KeyboardEvent| {
            k.set(is_ime_key(&e));
        })?;
        let (c, i) = (composing.clone(), input.clone());
        crate::on(
            &input,
            "compositionstart",
            true,
            move |_: CompositionEvent| {
                c.set(true);
                let _ = i.style().set_property("opacity", "1");
            },
        )?;
        let (c, i, d) = (composing.clone(), input.clone(), deliver.clone());
        crate::on(
            &input,
            "compositionend",
            true,
            move |e: CompositionEvent| {
                c.set(false);
                let _ = i.style().set_property("opacity", "0");
                i.set_value("");
                if let Some(text) = e.data().filter(|t| !t.is_empty()) {
                    d(InputEvent::Text(text));
                }
            },
        )?;
        // Text that arrives without a key press the app could see: virtual
        // keyboards report every key as 229 and insert through `input`.
        // Keys the app already got as `keydown` are dropped here, so nothing
        // is typed twice.
        let (c, i, d) = (composing, input.clone(), deliver.clone());
        crate::on(&input, "input", true, move |_: web_sys::Event| {
            if c.get() {
                return;
            }
            let text = i.value();
            i.set_value("");
            if ime_key.get() && !text.is_empty() {
                d(InputEvent::Text(text));
            }
        })?;
        let d = deliver;
        crate::on(&input, "paste", false, move |e: ClipboardEvent| {
            e.prevent_default();
            let text = e
                .clipboard_data()
                .and_then(|data| data.get_data("text").ok());
            if let Some(text) = text.filter(|t| !t.is_empty()) {
                // A single-line field: line breaks become spaces.
                d(InputEvent::Text(text.replace(['\r', '\n'], " ")));
            }
        })?;

        Ok(Self {
            input,
            canvas: canvas.clone(),
            placed: Cell::new(None),
        })
    }

    /// After a frame: follows the app's text field with the focus, or gives
    /// the focus back to the canvas when no field is being edited.
    pub fn update(&self, field: Option<Field>) {
        let document = self.input.owner_document();
        let active = document.as_ref().and_then(Document::active_element);
        let focused = active.is_some_and(|a| a == **self.input);
        match field {
            Some(field) => {
                if self.placed.get() != Some(field) {
                    self.place(&field);
                    self.placed.set(Some(field));
                }
                if !focused {
                    let _ = self.input.focus();
                }
            }
            None => {
                if focused {
                    let _ = self.canvas.focus();
                }
            }
        }
    }

    /// Puts the input just under the field, as wide as the field.
    fn place(&self, field: &Field) {
        let [x, y, w, h] = field.rect;
        let css = |c: [u8; 3]| format!("rgb({},{},{})", c[0], c[1], c[2]);
        let style = self.input.style();
        for (name, value) in [
            ("left", format!("{x}px")),
            ("top", format!("{}px", y + h + 2.0)),
            ("width", format!("{}px", w.max(160.0))),
            ("height", format!("{LINE_HEIGHT}px")),
            ("color", css(field.text)),
            ("background", css(field.background)),
            ("border-bottom", format!("1px solid {}", css(field.line))),
        ] {
            let _ = style.set_property(name, &value);
        }
    }
}
