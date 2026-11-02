// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Text input on the desktop. SDL 3 sends typed text (and opens the IME)
//! only between `SDL_StartTextInput` and `SDL_StopTextInput`, so text input
//! runs while a text field in the app is being edited, with the IME's area
//! set to that field.

use sdl3::VideoSubsystem;
use sdl3::rect::Rect;
use sdl3::video::Window;

#[derive(Default)]
pub struct TextInput {
    /// The field's rectangle in window coordinates while text input runs.
    area: Option<[i32; 4]>,
}

impl TextInput {
    /// After a frame: `field` is the app's text field in points.
    pub fn update(&mut self, field: Option<[f32; 4]>, window: &Window, video: &VideoSubsystem) {
        let input = video.text_input();
        let Some([x, y, w, h]) = field else {
            if self.area.take().is_some() {
                input.stop(window);
            }
            return;
        };
        // Points to SDL's window coordinates.
        let scale = window.display_scale() / window.pixel_density();
        let area = [x, y, w, h].map(|v| (v * scale).round() as i32);
        if self.area == Some(area) {
            return;
        }
        if self.area.is_none() {
            input.start(window);
        }
        self.area = Some(area);
        let [x, y, w, h] = area;
        input.set_rect(window, Rect::new(x, y, w.max(1) as u32, h.max(1) as u32), 0);
    }
}
