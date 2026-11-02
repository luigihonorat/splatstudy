// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! An orbit camera: it circles a target point, like walking around a cast
//! on its stand. Y is up.

use glam::{Mat4, Vec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitCamera {
    pub target: Vec3,
    /// Rotation around the vertical axis, radians; 0 looks along -Z (front).
    pub yaw: f32,
    /// Elevation, radians; positive looks down from above.
    pub pitch: f32,
    pub distance: f32,
    /// Vertical field of view, radians.
    pub fov_y: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.12,
            distance: 5.4,
            fov_y: 35f32.to_radians(),
        }
    }
}

const PITCH_LIMIT: f32 = 1.55;

impl OrbitCamera {
    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + self.distance * Vec3::new(sy * cp, sp, cy * cp)
    }

    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.eye(), self.target, Vec3::Y)
    }

    /// Perspective with wgpu's 0..1 depth range.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        let near = (self.distance * 0.01).max(1e-3);
        glam::camera::rh::proj::directx::perspective(
            self.fov_y,
            aspect.max(1e-3),
            near,
            self.distance * 100.0,
        )
    }

    /// Turns around the target; deltas in points, as dragged on screen.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        const SPEED: f32 = 0.008;
        self.yaw -= dx * SPEED;
        self.pitch = (self.pitch + dy * SPEED).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Slides the target so the surface follows the pointer; `view_height`
    /// is the viewport height in points.
    pub fn pan(&mut self, dx: f32, dy: f32, view_height: f32) {
        let per_point = 2.0 * self.distance * (self.fov_y / 2.0).tan() / view_height.max(1.0);
        let view = self.view();
        let right = view.row(0).truncate();
        let up = view.row(1).truncate();
        self.target += (-dx * right + dy * up) * per_point;
    }

    /// The ray through a point of the view, given in normalised device
    /// coordinates (-1..1, Y up): origin on the near plane and unit direction.
    pub fn ray(&self, ndc: glam::Vec2, aspect: f32) -> (Vec3, Vec3) {
        let inverse = (self.projection(aspect) * self.view()).inverse();
        let near = inverse.project_point3(ndc.extend(0.0));
        let far = inverse.project_point3(ndc.extend(1.0));
        (near, (far - near).normalize())
    }

    /// World units per screen point at `point`'s depth, in a view
    /// `view_height` points tall.
    pub fn world_per_point(&self, point: Vec3, view_height: f32) -> f32 {
        let forward = (self.target - self.eye()).normalize();
        let depth = (point - self.eye()).dot(forward).max(1e-4);
        2.0 * depth * (self.fov_y / 2.0).tan() / view_height.max(1.0)
    }

    /// The camera's right and up directions in world space.
    pub fn right_up(&self) -> (Vec3, Vec3) {
        let view = self.view();
        (view.row(0).truncate(), view.row(1).truncate())
    }

    /// Moves closer (factor < 1) or further (factor > 1).
    pub fn dolly(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(0.05, 500.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_view_looks_at_the_target_from_the_front() {
        let cam = OrbitCamera::default();
        let target_in_view = cam.view().transform_point3(cam.target);
        assert!(target_in_view.x.abs() < 1e-5 && target_in_view.y.abs() < 1e-5);
        assert!((target_in_view.z + cam.distance).abs() < 1e-4);
        assert!(cam.eye().z > 0.0, "front view looks along -Z");
    }

    #[test]
    fn pitch_stays_short_of_the_poles_and_dolly_is_bounded() {
        let mut cam = OrbitCamera::default();
        cam.orbit(0.0, 10_000.0);
        assert!(cam.pitch <= PITCH_LIMIT);
        cam.dolly(1e-9);
        assert!(cam.distance >= 0.05);
    }

    #[test]
    fn the_centre_ray_points_at_the_target() {
        let cam = OrbitCamera::default();
        let (origin, dir) = cam.ray(glam::Vec2::ZERO, 1.5);
        let to_target = (cam.target - origin).normalize();
        assert!(dir.dot(to_target) > 0.9999);
    }

    #[test]
    fn panning_keeps_the_distance() {
        let mut cam = OrbitCamera::default();
        let before = cam.eye().distance(cam.target);
        cam.pan(40.0, -25.0, 800.0);
        assert!((cam.eye().distance(cam.target) - before).abs() < 1e-4);
        assert!(cam.target.length() > 0.0);
    }
}
