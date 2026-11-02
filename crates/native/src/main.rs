// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The desktop shell: an SDL3 window (SDL has pen pressure on Windows, macOS
//! and Linux), a wgpu surface, and SDL events translated into core input.
//! Everything SDL-specific lives in this file, so a later move to winit only
//! replaces it.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod files;
mod text;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use sdl3::event::{Event, WindowEvent};
use sdl3::keyboard::{Keycode, Mod};
use sdl3::mouse::{Cursor, MouseButton, MouseWheelDirection, SystemCursor};
use sdl3::pen::PenAxis;
use sdl3::video::Window;
use splatstudy_core::{
    App, Button, CursorIcon, InputEvent, Key, Lang, Modifiers, PointerEvent, PointerKind,
    PointerPhase,
};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    if let Err(err) = run() {
        log::error!("{err}");
        let _ = sdl3::messagebox::show_simple_message_box(
            sdl3::messagebox::MessageBoxFlag::ERROR,
            "splatstudy",
            &err,
            None,
        );
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    // A pen should arrive only as pen events, not also as mouse or touch.
    sdl3::hint::set("SDL_PEN_MOUSE_EVENTS", "0");
    sdl3::hint::set("SDL_PEN_TOUCH_EVENTS", "0");

    let sdl = sdl3::init().map_err(|e| e.to_string())?;
    let video = sdl.video().map_err(|e| e.to_string())?;
    let window = video
        .window("splatstudy", 1440, 900)
        .position_centered()
        .resizable()
        .high_pixel_density()
        .metal_view()
        .build()
        .map_err(|e| e.to_string())?;

    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let surface = surface::create(&instance, &window)?;
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        ..Default::default()
    }))
    .map_err(|e| format!("no suitable GPU: {e}"))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("splatstudy"),
        required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .map_err(|e| format!("GPU device: {e}"))?;

    let caps = surface.get_capabilities(&adapter);
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| !f.is_srgb())
        .unwrap_or(caps.formats[0]);
    let (w, h) = window.size_in_pixels();
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: w.max(1),
        height: h.max(1),
        present_mode: wgpu::PresentMode::AutoVsync,
        desired_maximum_frame_latency: 2,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        color_space: Default::default(),
    };
    surface.configure(&device, &config);

    let lang = sys_locale_lang();
    let mut app = App::new(device.clone(), queue.clone(), format, lang);
    app.set_debug(std::env::args().any(|a| a == "--debug"));
    // `--sample=<id>` and `--view=<names>`, as `?sample=` and `?view=` on the web.
    let options: Vec<String> = std::env::args()
        .skip(1)
        .filter_map(|a| a.strip_prefix("--").map(str::to_owned))
        .collect();
    app.apply_url_options(&options.join("&"));
    app.set_first_run(config::first_run());
    if let Some(saved) = config::load_preferences() {
        app.apply_preferences(saved.trim());
    }
    let mut preferences = app.preferences();

    let event_subsystem = sdl.event().map_err(|e| e.to_string())?;
    let mut files = files::Files::new(&event_subsystem);
    let mut text = text::TextInput::default();

    let mut events = sdl.event_pump().map_err(|e| e.to_string())?;
    let mut input = Translator::default();
    let mut cursors = Cursors::default();
    let start = Instant::now();
    let mut redraw_at: Option<Instant> = None;

    loop {
        // Sleep until an event arrives or the app's next redraw is due.
        let timeout = match redraw_at {
            Some(t) => t.saturating_duration_since(Instant::now()),
            None if app.needs_frame() => Duration::ZERO,
            None => Duration::from_millis(500),
        };
        let first = if timeout.is_zero() {
            events.poll_event()
        } else {
            events.wait_event_timeout(timeout)
        };
        for event in first.into_iter().chain(events.poll_iter()) {
            match event {
                Event::Quit { .. } => return Ok(()),
                Event::Window {
                    win_event: WindowEvent::PixelSizeChanged(..),
                    ..
                } => {
                    let (w, h) = window.size_in_pixels();
                    config.width = w.max(1);
                    config.height = h.max(1);
                    surface.configure(&device, &config);
                    app.push_event(InputEvent::Focus(true));
                }
                Event::DropFile { filename, .. } => files.dropped(&filename),
                other => {
                    for e in input.translate(other, &window) {
                        app.push_event(e);
                    }
                }
            }
        }

        // Dialogs and file reads finish on other threads.
        for e in files.take() {
            app.push_event(e);
        }

        let due = redraw_at.is_some_and(|t| Instant::now() >= t);
        if !app.needs_frame() && !due {
            continue;
        }
        let texture = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                surface.configure(&device, &config);
                continue;
            }
            _ => {
                redraw_at = Some(Instant::now() + Duration::from_millis(16));
                continue;
            }
        };
        let view = texture.texture.create_view(&Default::default());
        let ppp = window.display_scale();
        let out = app.frame(
            start.elapsed().as_secs_f64(),
            &view,
            [config.width, config.height],
            ppp,
        );
        queue.present(texture);
        let now_preferences = app.preferences();
        if now_preferences != preferences {
            config::save_preferences(&now_preferences);
            preferences = now_preferences;
        }
        redraw_at = out
            .repaint_after
            .map(|s| Instant::now() + Duration::from_secs_f64(s));
        cursors.set(out.cursor);
        for request in out.requests {
            files.handle(request, &window);
        }
        text.update(app.text_field().map(|f| f.rect), &window, &video);
        if let Some(url) = out.open_url
            && let Err(e) = sdl3::url::open_url(&url)
        {
            eprintln!("could not open {url}: {e}");
        }
        if out.quit {
            return Ok(());
        }
    }
}

/// Turns SDL events into core events. SDL reports pen pressure in separate
/// axis events, so the last pressure of each pen is remembered here.
#[derive(Default)]
struct Translator {
    pressure: HashMap<u32, f32>,
    eraser: HashMap<u32, bool>,
    modifiers: Modifiers,
}

impl Translator {
    fn translate(&mut self, event: Event, window: &Window) -> Vec<InputEvent> {
        // SDL positions are in window coordinates; the core wants points.
        let scale = window.pixel_density() / window.display_scale();
        let pos = |x: f32, y: f32| [x * scale, y * scale];
        let mut out = Vec::new();
        match event {
            Event::MouseMotion {
                x, y, mousestate, ..
            } => out.push(self.mouse(
                PointerPhase::Move,
                pos(x, y),
                None,
                if mousestate.left() { 1.0 } else { 0.0 },
            )),
            Event::MouseButtonDown {
                x, y, mouse_btn, ..
            } => {
                out.push(self.mouse(PointerPhase::Down, pos(x, y), button(mouse_btn), 1.0));
            }
            Event::MouseButtonUp {
                x, y, mouse_btn, ..
            } => {
                out.push(self.mouse(PointerPhase::Up, pos(x, y), button(mouse_btn), 0.0));
            }
            Event::MouseWheel {
                x, y, direction, ..
            } => {
                let flip = if direction == MouseWheelDirection::Flipped {
                    -1.0
                } else {
                    1.0
                };
                out.push(InputEvent::Wheel {
                    delta: [x * flip, y * flip],
                    lines: true,
                });
            }
            Event::PenDown {
                which,
                x,
                y,
                eraser,
                ..
            } => {
                self.eraser.insert(which, eraser);
                out.push(self.pen(which, PointerPhase::Down, pos(x, y), Some(Button::Primary)));
            }
            Event::PenUp { which, x, y, .. } => {
                out.push(self.pen(which, PointerPhase::Up, pos(x, y), Some(Button::Primary)));
                self.pressure.insert(which, 0.0);
            }
            Event::PenMotion { which, x, y, .. } => {
                out.push(self.pen(which, PointerPhase::Move, pos(x, y), None));
            }
            Event::PenAxis {
                which,
                x,
                y,
                axis: PenAxis::Pressure,
                value,
                ..
            } => {
                self.pressure.insert(which, value);
                out.push(self.pen(which, PointerPhase::Move, pos(x, y), None));
            }
            Event::Window { win_event, .. } => match win_event {
                WindowEvent::MouseLeave => out.push(InputEvent::PointerLeft),
                WindowEvent::FocusGained => out.push(InputEvent::Focus(true)),
                WindowEvent::FocusLost => out.push(InputEvent::Focus(false)),
                WindowEvent::Exposed => out.push(InputEvent::Focus(true)),
                _ => {}
            },
            Event::KeyDown {
                keycode,
                keymod,
                repeat,
                ..
            }
            | Event::KeyUp {
                keycode,
                keymod,
                repeat,
                ..
            } => {
                let pressed = matches!(event, Event::KeyDown { .. });
                self.update_modifiers(keymod, &mut out);
                if let Some(key) = keycode.and_then(key) {
                    out.push(InputEvent::Key {
                        key,
                        pressed,
                        repeat,
                    });
                }
            }
            Event::TextInput { text, .. } => out.push(InputEvent::Text(text)),
            _ => {}
        }
        out
    }

    fn mouse(
        &self,
        phase: PointerPhase,
        pos: [f32; 2],
        button: Option<Button>,
        pressure: f32,
    ) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            id: 0,
            kind: PointerKind::Mouse,
            phase,
            pos,
            button,
            pressure,
            eraser: false,
        })
    }

    fn pen(
        &self,
        which: u32,
        phase: PointerPhase,
        pos: [f32; 2],
        button: Option<Button>,
    ) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            id: u64::from(which) | 1 << 32,
            kind: PointerKind::Pen,
            phase,
            pos,
            button,
            pressure: self.pressure.get(&which).copied().unwrap_or(0.0),
            eraser: self.eraser.get(&which).copied().unwrap_or(false),
        })
    }

    fn update_modifiers(&mut self, keymod: Mod, out: &mut Vec<InputEvent>) {
        let ctrl = keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD);
        let gui = keymod.intersects(Mod::LGUIMOD | Mod::RGUIMOD);
        let m = Modifiers {
            alt: keymod.intersects(Mod::LALTMOD | Mod::RALTMOD),
            ctrl,
            shift: keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD),
            command: if cfg!(target_os = "macos") { gui } else { ctrl },
        };
        if m != self.modifiers {
            self.modifiers = m;
            out.push(InputEvent::Modifiers(m));
        }
    }
}

fn button(b: MouseButton) -> Option<Button> {
    match b {
        MouseButton::Left => Some(Button::Primary),
        MouseButton::Right => Some(Button::Secondary),
        MouseButton::Middle => Some(Button::Middle),
        _ => None,
    }
}

/// SDL key names → egui key names where they differ.
fn key(code: Keycode) -> Option<Key> {
    let name = code.name();
    let name = match name.as_str() {
        "Left" => "ArrowLeft",
        "Right" => "ArrowRight",
        "Up" => "ArrowUp",
        "Down" => "ArrowDown",
        "Return" => "Enter",
        "Escape" => "Escape",
        other => other,
    };
    Key::from_name(name)
}

/// The UI language from the OS's first preferred locale; Japanese when unknown.
fn sys_locale_lang() -> Lang {
    use sdl3::sys::locale::SDL_GetPreferredLocales;
    use sdl3::sys::stdinc::SDL_free;
    let mut count = 0;
    // SAFETY: SDL returns null or an array of `count` locale pointers, freed
    // with a single SDL_free; `language` is a valid C string when non-null.
    unsafe {
        let locales = SDL_GetPreferredLocales(&mut count);
        if locales.is_null() {
            return Lang::default();
        }
        let first = *locales;
        let lang = if count > 0 && !first.is_null() && !(*first).language.is_null() {
            let tag = std::ffi::CStr::from_ptr((*first).language).to_string_lossy();
            Lang::from_tag(&tag)
        } else {
            Lang::default()
        };
        SDL_free(locales.cast());
        lang
    }
}

#[derive(Default)]
struct Cursors {
    current: Option<CursorIcon>,
    cache: HashMap<SystemCursor, Cursor>,
}

impl Cursors {
    fn set(&mut self, icon: CursorIcon) {
        if self.current == Some(icon) {
            return;
        }
        self.current = Some(icon);
        let system = match icon {
            CursorIcon::PointingHand => SystemCursor::Hand,
            CursorIcon::Crosshair => SystemCursor::Crosshair,
            CursorIcon::Text => SystemCursor::IBeam,
            CursorIcon::Move | CursorIcon::AllScroll | CursorIcon::Grab | CursorIcon::Grabbing => {
                SystemCursor::SizeAll
            }
            CursorIcon::ResizeHorizontal | CursorIcon::ResizeColumn => SystemCursor::SizeWE,
            CursorIcon::ResizeVertical | CursorIcon::ResizeRow => SystemCursor::SizeNS,
            CursorIcon::NotAllowed | CursorIcon::NoDrop => SystemCursor::No,
            CursorIcon::Wait => SystemCursor::Wait,
            CursorIcon::Progress => SystemCursor::WaitArrow,
            _ => SystemCursor::Arrow,
        };
        if !self.cache.contains_key(&system)
            && let Ok(cursor) = Cursor::from_system(system)
        {
            self.cache.insert(system, cursor);
        }
        if let Some(cursor) = self.cache.get(&system) {
            cursor.set();
        }
    }
}

mod surface {
    use sdl3::video::Window;
    use wgpu::rwh::{HasDisplayHandle, HasWindowHandle};

    /// wgpu wants a `Send + Sync` handle source; the window stays on this thread.
    struct SyncWindow<'a>(&'a Window);

    // SAFETY: the surface is only used on the thread that owns the window.
    unsafe impl Send for SyncWindow<'_> {}
    unsafe impl Sync for SyncWindow<'_> {}

    impl HasWindowHandle for SyncWindow<'_> {
        fn window_handle(&self) -> Result<wgpu::rwh::WindowHandle<'_>, wgpu::rwh::HandleError> {
            self.0.window_handle()
        }
    }

    impl HasDisplayHandle for SyncWindow<'_> {
        fn display_handle(&self) -> Result<wgpu::rwh::DisplayHandle<'_>, wgpu::rwh::HandleError> {
            self.0.display_handle()
        }
    }

    pub fn create<'a>(
        instance: &wgpu::Instance,
        window: &'a Window,
    ) -> Result<wgpu::Surface<'a>, String> {
        instance
            .create_surface(SyncWindow(window))
            .map_err(|e| e.to_string())
    }
}
