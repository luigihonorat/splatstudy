// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The browser shell: a full-window canvas, WebGPU through wgpu, and the DOM's
//! Pointer Events (with pen pressure) translated into core input events.

mod files;
mod ime;
mod storage;
mod video;

use std::cell::RefCell;
use std::rc::Rc;

use splatstudy_core::{
    App, Button, CursorIcon, InputEvent, Key, Lang, Modifiers, PointerEvent, PointerKind,
    PointerPhase, Request,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{
    HtmlCanvasElement, KeyboardEvent, MouseEvent, PointerEvent as DomPointer, WheelEvent,
};

const CANVAS_ID: &str = "splatstudy";

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    wasm_bindgen_futures::spawn_local(async {
        match run().await {
            Ok(()) => {}
            // The page already shows the friendly WebGPU message.
            Err(err) if err.is_undefined() => {}
            Err(err) => {
                log::error!("splatstudy failed to start: {err:?}");
                show_error(&format!("{err:?}"));
            }
        }
    });
}

/// The browser has WebGPU but no usable adapter or device: shows the page's
/// "this browser can't use WebGPU" message, in the page's language, instead
/// of a raw error. Returns `undefined` when the message is on screen.
fn no_gpu(detail: &str) -> JsValue {
    log::error!("WebGPU is not usable: {detail}");
    let shown = web_sys::window()
        .and_then(|w| js_sys::Reflect::get(&w, &"splatstudyUnsupported".into()).ok())
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .is_some_and(|f| f.call0(&JsValue::NULL).is_ok());
    if shown {
        JsValue::UNDEFINED
    } else {
        detail.into()
    }
}

struct Shell {
    app: App,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    canvas: HtmlCanvasElement,
    modifiers: Modifiers,
    /// Seconds (performance clock) at which the app asked to be redrawn.
    redraw_at: Option<f64>,
    cursor: CursorIcon,
    /// The preferences as last stored.
    preferences: String,
}

async fn run() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let canvas: HtmlCanvasElement = document
        .get_element_by_id(CANVAS_ID)
        .ok_or("no canvas")?
        .dyn_into()?;

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::BROWSER_WEBGPU,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
        .map_err(|e| format!("surface: {e}"))?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        })
        .await
        .map_err(|e| no_gpu(&format!("adapter: {e}")))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("splatstudy"),
            required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
            ..Default::default()
        })
        .await
        .map_err(|e| no_gpu(&format!("device: {e}")))?;

    let caps = surface.get_capabilities(&adapter);
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| !f.is_srgb())
        .unwrap_or(caps.formats[0]);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: 1,
        height: 1,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        color_space: Default::default(),
    };

    // `?lang=fr` overrides the browser's language; `?debug` shows pen data;
    // the app reads the rest (`?sample=`, `?view=`).
    let search = window.location().search().unwrap_or_default();
    let lang = query_value(&search, "lang")
        .or_else(|| window.navigator().language())
        .map(|t| Lang::from_tag(&t))
        .unwrap_or_default();
    let mut app = App::new(device.clone(), queue.clone(), format, lang);
    app.set_debug(query_value(&search, "debug").is_some());
    app.set_mac(is_mac());
    app.set_video_encoder(Box::new(video::WebVideo::new()));
    // Saved preferences beat the browser's language; `?lang=` beats both.
    if let Some(saved) = storage::get(storage::PREFERENCES) {
        app.apply_preferences(&saved);
    }
    if let Some(tag) = query_value(&search, "lang") {
        app.apply_preferences(&format!("lang={tag}"));
    }
    app.apply_url_options(&search);
    app.set_first_run(storage::first_run());
    let preferences = app.preferences();

    let shell = Rc::new(RefCell::new(Shell {
        app,
        surface,
        config,
        device,
        queue,
        canvas: canvas.clone(),
        modifiers: Modifiers::default(),
        redraw_at: None,
        cursor: CursorIcon::Default,
        preferences,
    }));
    listen(&shell, &canvas)?;
    // Files and text input deliver from their own callbacks. One can fire
    // while the shell is borrowed (moving the focus commits an IME word
    // synchronously), so such events wait for the next frame instead.
    let pending: Rc<RefCell<Vec<InputEvent>>> = Rc::default();
    let deliver: files::Deliver = {
        let s = shell.clone();
        let p = pending.clone();
        Rc::new(move |e| match s.try_borrow_mut() {
            Ok(mut shell) => shell.app.push_event(e),
            Err(_) => p.borrow_mut().push(e),
        })
    };
    let files = files::Files::new(&document, deliver.clone())?;
    let text = ime::TextInput::new(&document, &canvas, deliver)?;
    animate(shell, files, text, pending);
    let _ = canvas.focus();
    Ok(())
}

/// Adds an event listener that lives as long as the page.
fn on<E: JsCast + 'static>(
    target: &web_sys::EventTarget,
    name: &str,
    passive: bool,
    mut f: impl FnMut(E) + 'static,
) -> Result<(), JsValue> {
    let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |e: web_sys::Event| {
        f(e.unchecked_into());
    });
    let options = web_sys::AddEventListenerOptions::new();
    options.set_passive(passive);
    target.add_event_listener_with_callback_and_add_event_listener_options(
        name,
        closure.as_ref().unchecked_ref(),
        &options,
    )?;
    closure.forget();
    Ok(())
}

fn listen(shell: &Rc<RefCell<Shell>>, canvas: &HtmlCanvasElement) -> Result<(), JsValue> {
    let window: web_sys::EventTarget = web_sys::window().ok_or("no window")?.into();

    for (name, phase) in [
        ("pointerdown", PointerPhase::Down),
        ("pointermove", PointerPhase::Move),
        ("pointerup", PointerPhase::Up),
        ("pointercancel", PointerPhase::Cancel),
    ] {
        let s = shell.clone();
        let c = canvas.clone();
        on(canvas, name, false, move |e: DomPointer| {
            e.prevent_default();
            if phase == PointerPhase::Down {
                let _ = c.set_pointer_capture(e.pointer_id());
                // Before borrowing the shell: taking the focus from the IME
                // commits its word, which is delivered first, as typed.
                let _ = c.focus();
            }
            let mut shell = s.borrow_mut();
            shell.update_modifiers(&e);
            if matches!(phase, PointerPhase::Up | PointerPhase::Cancel) {
                let _ = c.release_pointer_capture(e.pointer_id());
            }
            // A pen reports several samples per frame; the brush wants them all.
            let coalesced = if phase == PointerPhase::Move {
                e.get_coalesced_events()
            } else {
                js_sys::Array::new()
            };
            if coalesced.length() > 1 {
                for sample in coalesced.iter() {
                    let sample: DomPointer = sample.unchecked_into();
                    shell
                        .app
                        .push_event(InputEvent::Pointer(pointer(&sample, phase)));
                }
            } else {
                shell
                    .app
                    .push_event(InputEvent::Pointer(pointer(&e, phase)));
            }
        })?;
    }
    let s = shell.clone();
    on(canvas, "pointerleave", true, move |e: DomPointer| {
        if e.buttons() == 0 {
            s.borrow_mut().app.push_event(InputEvent::PointerLeft);
        }
    })?;
    on(canvas, "contextmenu", false, |e: MouseEvent| {
        e.prevent_default()
    })?;

    let s = shell.clone();
    on(canvas, "wheel", false, move |e: WheelEvent| {
        e.prevent_default();
        let mut shell = s.borrow_mut();
        shell.update_modifiers(&e);
        let scale = match e.delta_mode() {
            WheelEvent::DOM_DELTA_PAGE => 20.0,
            _ => 1.0,
        };
        let delta = [-(e.delta_x() as f32) * scale, -(e.delta_y() as f32) * scale];
        let lines = e.delta_mode() == WheelEvent::DOM_DELTA_LINE;
        shell.app.push_event(InputEvent::Wheel { delta, lines });
    })?;

    for (name, pressed) in [("keydown", true), ("keyup", false)] {
        let s = shell.clone();
        on(&window, name, false, move |e: KeyboardEvent| {
            let mut shell = s.borrow_mut();
            shell.update_key_modifiers(&e);
            // The IME owns these; its committed text arrives from `ime`.
            if ime::is_ime_key(&e) {
                return;
            }
            let name = e.key();
            if let Some(key) = Key::from_name(&name) {
                shell.app.push_event(InputEvent::Key {
                    key,
                    pressed,
                    repeat: e.repeat(),
                });
            }
            // Windows reports AltGr (for @ # € on French and German
            // keyboards) as Ctrl+Alt: that is typing, not a shortcut.
            let altgr = e.get_modifier_state("AltGraph") || (e.ctrl_key() && e.alt_key());
            let shortcut = (e.ctrl_key() || e.meta_key()) && !altgr;
            if pressed && !shortcut && name.chars().count() == 1 {
                shell.app.push_event(InputEvent::Text(name.clone()));
            }
            // Keep the browser from acting on keys the app uses.
            // Cmd/Ctrl+S, +Shift+S and +O are the app's save, save as and
            // open, not the browser's.
            let command = if is_mac() { e.meta_key() } else { e.ctrl_key() };
            let file_key = command && matches!(name.to_ascii_lowercase().as_str(), "s" | "o");
            if matches!(name.as_str(), "Alt" | "Tab" | " " | "F2")
                || (!shortcut && name.starts_with("Arrow"))
                || file_key
            {
                e.prevent_default();
            }
        })?;
    }
    for (name, focused) in [("focus", true), ("blur", false)] {
        let s = shell.clone();
        on(&window, name, true, move |_: web_sys::FocusEvent| {
            s.borrow_mut().app.push_event(InputEvent::Focus(focused));
        })?;
    }
    Ok(())
}

fn pointer(e: &DomPointer, phase: PointerPhase) -> PointerEvent {
    let kind = match e.pointer_type().as_str() {
        "pen" => PointerKind::Pen,
        "touch" => PointerKind::Touch,
        _ => PointerKind::Mouse,
    };
    let button = match e.button() {
        0 | 5 => Some(Button::Primary),
        1 => Some(Button::Middle),
        2 => Some(Button::Secondary),
        _ => None,
    };
    // A pressed mouse reports 0.5; treat it as full pressure.
    let pressure = match kind {
        PointerKind::Mouse => {
            if e.buttons() != 0 {
                1.0
            } else {
                0.0
            }
        }
        _ => e.pressure(),
    };
    PointerEvent {
        id: e.pointer_id() as u64,
        kind,
        phase,
        pos: [e.client_x() as f32, e.client_y() as f32],
        button: if matches!(phase, PointerPhase::Down | PointerPhase::Up) {
            button
        } else {
            None
        },
        pressure,
        eraser: kind == PointerKind::Pen && (e.buttons() & 32 != 0 || e.button() == 5),
    }
}

impl Shell {
    fn update_modifiers(&mut self, e: &MouseEvent) {
        self.set_modifiers(e.alt_key(), e.ctrl_key(), e.shift_key(), e.meta_key());
    }

    fn update_key_modifiers(&mut self, e: &KeyboardEvent) {
        self.set_modifiers(e.alt_key(), e.ctrl_key(), e.shift_key(), e.meta_key());
    }

    fn set_modifiers(&mut self, alt: bool, ctrl: bool, shift: bool, meta: bool) {
        let mac = is_mac();
        let m = Modifiers {
            alt,
            ctrl,
            shift,
            command: if mac { meta } else { ctrl },
        };
        if m != self.modifiers {
            self.modifiers = m;
            self.app.push_event(InputEvent::Modifiers(m));
        }
    }

    /// Matches the canvas's pixel size to its CSS size and the device pixel ratio.
    fn fit_canvas(&mut self) -> f32 {
        let window = web_sys::window().expect("window");
        let ppp = window.device_pixel_ratio() as f32;
        let w = ((self.canvas.client_width() as f32) * ppp).round().max(1.0) as u32;
        let h = ((self.canvas.client_height() as f32) * ppp)
            .round()
            .max(1.0) as u32;
        let max = self.device.limits().max_texture_dimension_2d;
        let (w, h) = (w.min(max), h.min(max));
        if w != self.config.width || h != self.config.height {
            self.canvas.set_width(w);
            self.canvas.set_height(h);
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
            self.app.push_event(InputEvent::Focus(true));
        }
        ppp
    }

    /// Draws a frame if one is needed and returns what the app asked the
    /// shell to do.
    fn frame(&mut self, now: f64) -> Vec<Request> {
        let ppp = self.fit_canvas();
        let due = self.redraw_at.is_some_and(|t| now >= t);
        if !self.app.needs_frame() && !due {
            return Vec::new();
        }
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Vec::new();
            }
            _ => return Vec::new(),
        };
        let view = texture.texture.create_view(&Default::default());
        let size = [self.config.width, self.config.height];
        let out = self.app.frame(now, &view, size, ppp);
        self.queue.present(texture);
        let preferences = self.app.preferences();
        if preferences != self.preferences {
            storage::set(storage::PREFERENCES, &preferences);
            self.preferences = preferences;
        }
        self.redraw_at = out.repaint_after.map(|after| now + after);
        if out.cursor != self.cursor {
            self.cursor = out.cursor;
            let _ = self
                .canvas
                .style()
                .set_property("cursor", css_cursor(out.cursor));
        }
        if let Some(url) = out.open_url
            && let Some(window) = web_sys::window()
        {
            let _ = window.open_with_url_and_target(&url, "_blank");
        }
        out.requests
    }

    /// The app's text field being edited, for the IME.
    fn text_field(&self) -> Option<ime::Field> {
        self.app.text_field().map(|f| ime::Field {
            rect: f.rect,
            text: f.text,
            background: f.background,
            line: f.line,
        })
    }
}

type FrameCallback = Closure<dyn FnMut()>;

/// Runs a frame on every animation frame, then the app's requests, outside
/// the shell's borrow: files and text input hand events back to it.
fn animate(
    shell: Rc<RefCell<Shell>>,
    files: files::Files,
    text: ime::TextInput,
    pending: Rc<RefCell<Vec<InputEvent>>>,
) {
    let callback: Rc<RefCell<Option<FrameCallback>>> = Rc::new(RefCell::new(None));
    let next = callback.clone();
    *callback.borrow_mut() = Some(Closure::new(move || {
        let now = web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now() / 1000.0);
        let queued = std::mem::take(&mut *pending.borrow_mut());
        for event in queued {
            shell.borrow_mut().app.push_event(event);
        }
        let requests = shell.borrow_mut().frame(now);
        for request in requests {
            files.handle(request);
        }
        // Not borrowed while updating: that may move the focus.
        let field = shell.borrow().text_field();
        text.update(field);
        request_frame(next.borrow().as_ref().expect("frame callback"));
    }));
    request_frame(callback.borrow().as_ref().expect("frame callback"));
}

fn request_frame(f: &Closure<dyn FnMut()>) {
    if let Some(window) = web_sys::window() {
        let _ = window.request_animation_frame(f.as_ref().unchecked_ref());
    }
}

/// The value of `key` in a `?a=1&b` query string; `Some("")` for a bare key.
fn query_value(search: &str, key: &str) -> Option<String> {
    search.trim_start_matches('?').split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == key).then(|| v.to_owned())
    })
}

fn is_mac() -> bool {
    web_sys::window()
        .and_then(|w| w.navigator().platform().ok())
        .is_some_and(|p| p.starts_with("Mac") || p.starts_with("iP"))
}

fn css_cursor(cursor: CursorIcon) -> &'static str {
    match cursor {
        CursorIcon::None => "none",
        CursorIcon::PointingHand => "pointer",
        CursorIcon::Crosshair => "crosshair",
        CursorIcon::Text => "text",
        CursorIcon::Grab => "grab",
        CursorIcon::Grabbing => "grabbing",
        CursorIcon::Move | CursorIcon::AllScroll => "move",
        CursorIcon::ResizeHorizontal | CursorIcon::ResizeColumn => "ew-resize",
        CursorIcon::ResizeVertical | CursorIcon::ResizeRow => "ns-resize",
        CursorIcon::NotAllowed | CursorIcon::NoDrop => "not-allowed",
        CursorIcon::Wait => "wait",
        CursorIcon::Progress => "progress",
        CursorIcon::ZoomIn => "zoom-in",
        CursorIcon::ZoomOut => "zoom-out",
        _ => "default",
    }
}

/// Shows a startup failure in the page instead of a blank canvas.
fn show_error(message: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("error"))
    {
        el.set_text_content(Some(message));
        let _ = el.set_attribute("style", "display:block");
    }
}
