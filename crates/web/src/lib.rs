// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The browser shell: a full-window canvas, WebGPU through wgpu, and the DOM's
//! Pointer Events (with pen pressure) translated into core input events.

use std::cell::RefCell;
use std::rc::Rc;

use splatstudy_core::{
    App, Button, CursorIcon, InputEvent, Key, Lang, Modifiers, PointerEvent, PointerKind,
    PointerPhase,
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
        if let Err(err) = run().await {
            log::error!("splatstudy failed to start: {err:?}");
            show_error(&format!("{err:?}"));
        }
    });
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
        .map_err(|e| format!("adapter: {e}"))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("splatstudy"),
            required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("device: {e}"))?;

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

    // `?lang=fr` overrides the browser's language; `?debug` shows pen data.
    let search = window.location().search().unwrap_or_default();
    let lang = query_value(&search, "lang")
        .or_else(|| window.navigator().language())
        .map(|t| Lang::from_tag(&t))
        .unwrap_or_default();
    let mut app = App::new(device.clone(), queue.clone(), format, lang);
    app.set_debug(query_value(&search, "debug").is_some());

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
    }));
    listen(&shell, &canvas)?;
    animate(shell);
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
            let mut shell = s.borrow_mut();
            shell.update_modifiers(&e);
            match phase {
                PointerPhase::Down => {
                    let _ = c.set_pointer_capture(e.pointer_id());
                    let _ = c.focus();
                }
                PointerPhase::Up | PointerPhase::Cancel => {
                    let _ = c.release_pointer_capture(e.pointer_id());
                }
                PointerPhase::Move => {}
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
            let name = e.key();
            if let Some(key) = Key::from_name(&name) {
                shell.app.push_event(InputEvent::Key {
                    key,
                    pressed,
                    repeat: e.repeat(),
                });
            }
            let shortcut = e.ctrl_key() || e.meta_key();
            if pressed && !shortcut && name.chars().count() == 1 {
                shell.app.push_event(InputEvent::Text(name.clone()));
            }
            // Keep the browser from acting on keys the app uses.
            if matches!(name.as_str(), "Alt" | "Tab" | " " | "F2")
                || (!shortcut && name.starts_with("Arrow"))
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

    fn frame(&mut self, now: f64) {
        let ppp = self.fit_canvas();
        let due = self.redraw_at.is_some_and(|t| now >= t);
        if !self.app.needs_frame() && !due {
            return;
        }
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };
        let view = texture.texture.create_view(&Default::default());
        let size = [self.config.width, self.config.height];
        let out = self.app.frame(now, &view, size, ppp);
        self.queue.present(texture);
        self.redraw_at = out.repaint_after.map(|after| now + after);
        if out.cursor != self.cursor {
            self.cursor = out.cursor;
            let _ = self
                .canvas
                .style()
                .set_property("cursor", css_cursor(out.cursor));
        }
    }
}

type FrameCallback = Closure<dyn FnMut()>;

fn animate(shell: Rc<RefCell<Shell>>) {
    let callback: Rc<RefCell<Option<FrameCallback>>> = Rc::new(RefCell::new(None));
    let next = callback.clone();
    *callback.borrow_mut() = Some(Closure::new(move || {
        let now = web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now() / 1000.0);
        shell.borrow_mut().frame(now);
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
