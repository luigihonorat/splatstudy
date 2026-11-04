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
    /// Orthographic: parallel lines stay parallel, and the form keeps its
    /// size whatever its depth. The view is as tall as the perspective's
    /// at the target, so switching keeps the target's scale, and dollying
    /// still zooms.
    pub ortho: bool,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.12,
            distance: 5.4,
            fov_y: 35f32.to_radians(),
            ortho: false,
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

    /// Half the view's height at the target, in world units: what an
    /// orthographic view shows above and below its centre.
    pub fn half_height(&self) -> f32 {
        self.distance * (self.fov_y / 2.0).tan()
    }

    /// Perspective or orthographic, with wgpu's 0..1 depth range. The
    /// orthographic view reaches as far behind the eye as in front of it,
    /// so zooming in never cuts the form off.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        let aspect = aspect.max(1e-3);
        let far = self.distance * 100.0;
        if self.ortho {
            let h = self.half_height();
            let w = h * aspect;
            glam::camera::rh::proj::directx::orthographic(-w, w, -h, h, -far, far)
        } else {
            let near = (self.distance * 0.01).max(1e-3);
            glam::camera::rh::proj::directx::perspective(self.fov_y, aspect, near, far)
        }
    }

    /// Turns around the target; deltas in points, as dragged on screen.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        const SPEED: f32 = 0.008;
        self.yaw -= dx * SPEED;
        self.pitch = (self.pitch + dy * SPEED).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Looks at the target from a fixed direction, keeping the distance.
    pub fn look_from(&mut self, yaw: f32, pitch: f32) {
        self.yaw = yaw;
        self.pitch = pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Centres a sphere of `radius` around `centre` so it fills the view's
    /// height with a margin, keeping the direction.
    pub fn frame(&mut self, centre: Vec3, radius: f32) {
        self.target = centre;
        self.distance = 1.25 * radius.max(1e-3) / (self.fov_y / 2.0).sin();
    }

    /// Slides the target so the surface follows the pointer; `view_height`
    /// is the viewport height in points.
    pub fn pan(&mut self, dx: f32, dy: f32, view_height: f32) {
        let per_point = 2.0 * self.half_height() / view_height.max(1.0);
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
        if self.ortho {
            return 2.0 * self.half_height() / view_height.max(1.0);
        }
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
    fn viewpoints_and_framing() {
        let mut cam = OrbitCamera::default();
        cam.look_from(std::f32::consts::FRAC_PI_2, 0.0);
        assert!(
            cam.eye().x > 0.0 && cam.eye().z.abs() < 1e-4,
            "right view from +X"
        );
        cam.look_from(0.0, 10.0);
        assert!(cam.pitch <= PITCH_LIMIT);

        let centre = Vec3::new(0.5, 1.0, -0.3);
        cam.frame(centre, 2.0);
        assert_eq!(cam.target, centre);
        // The sphere's silhouette fits inside the vertical field of view.
        let half_angle = (2.0 / cam.distance).asin();
        assert!(half_angle < cam.fov_y / 2.0);
    }

    #[test]
    fn ortho_rays_are_parallel_and_hit_where_the_point_is_drawn() {
        let mut cam = OrbitCamera::default();
        cam.orbit(30.0, -20.0);
        cam.ortho = true;
        let aspect = 1.5;
        let to_clip = cam.projection(aspect) * cam.view();
        let forward = (cam.target - cam.eye()).normalize();
        for point in [
            Vec3::ZERO,
            Vec3::new(0.4, -0.3, 0.2),
            Vec3::new(-0.9, 0.5, -0.6),
        ] {
            let ndc = to_clip.project_point3(point);
            let (origin, dir) = cam.ray(ndc.truncate(), aspect);
            assert!(dir.dot(forward) > 0.9999, "parallel to the view");
            // The ray passes through the point.
            let along = (point - origin).dot(dir);
            assert!((origin + dir * along).distance(point) < 1e-3, "{point}");
            assert!(along > 0.0, "in front of the ray's origin");
        }
        // Even a point behind the eye is drawn and picked.
        let behind = cam.eye() + (cam.eye() - cam.target) * 0.5;
        let ndc = to_clip.project_point3(behind);
        assert!((0.0..=1.0).contains(&ndc.z), "{ndc}");
    }

    #[test]
    fn ortho_keeps_the_scale_at_the_target_whatever_the_depth() {
        let persp = OrbitCamera::default();
        let ortho = OrbitCamera {
            ortho: true,
            ..persp
        };
        let height = 600.0;
        let at_target = persp.world_per_point(persp.target, height);
        assert!((ortho.world_per_point(persp.target, height) - at_target).abs() < 1e-6);
        let near = persp.target + Vec3::Z;
        assert!(persp.world_per_point(near, height) < at_target);
        assert_eq!(
            ortho.world_per_point(near, height),
            ortho.world_per_point(persp.target, height)
        );
        // A point at the target's depth lands at the same place in both.
        let (right, up) = persp.right_up();
        let p = persp.target + right * 0.5 + up * 0.3;
        let seen = |cam: &OrbitCamera| (cam.projection(1.2) * cam.view()).project_point3(p);
        assert!((seen(&persp).truncate() - seen(&ortho).truncate()).length() < 1e-4);
        // Dollying out shows more.
        let mut far = ortho;
        far.dolly(2.0);
        assert!((far.world_per_point(p, height) / at_target - 2.0).abs() < 1e-4);
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
