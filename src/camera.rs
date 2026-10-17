//! Node camera (`cam_Update` 0x407cf0, `cam_EdgeZone` 0x407c10, `core_LookAt` 0x412960).
//!
//! The view direction is `(sin yaw, look_y, -cos yaw)` from the cube centre — `look_y` is a
//! height, not an angle, so the effective pitch is `atan(look_y)`. Yaw wraps in [0, 6.28].

use glam::{Mat4, Vec3};

use crate::sc;

#[derive(Debug, Clone)]
pub struct Camera {
    pub yaw: f32,
    pub look_y: f32,
    /// Acceleration ramp 0..1 (`DAT_0048bbb4`), +0.025 per moving frame.
    accel: f32,
    moving: bool,
    /// Pending mouse-look offset in virtual pixels (alternate control).
    look_dx: f32,
    look_dy: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { yaw: std::f32::consts::PI, look_y: 0.0, accel: 0.0, moving: false, look_dx: 0.0, look_dy: 0.0 }
    }
}

/// Edge-scroll direction under the cursor (`cam_EdgeZone`), used for cursors in slideshow mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    None,
    Left,
    Right,
    Up,
    Down,
}

/// Inputs to one camera update, all in virtual canvas pixels except `width`.
pub struct Pointer {
    pub x: f32,
    pub y: f32,
    /// Virtual width of the window (wider than 1024 on widescreen).
    pub width: f32,
}

const TWO_PI: f32 = 6.28;

impl Camera {
    pub fn view(&self) -> Mat4 {
        let dir = Vec3::new(self.yaw.sin(), self.look_y, -self.yaw.cos());
        Mat4::look_at_rh(Vec3::ZERO, dir, Vec3::Y)
    }

    /// `gluPerspective(fov, aspect, 0.1, 1000)` mapped to wgpu's 0..1 depth range.
    pub fn projection(fov_deg: f32, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(fov_deg.to_radians(), aspect, 0.1, 1000.0)
    }

    pub fn set(&mut self, yaw: f32, look_y: f32) {
        self.yaw = yaw;
        self.look_y = look_y;
    }

    pub fn reset_motion(&mut self) {
        self.accel = 0.0;
        self.look_dx = 0.0;
        self.look_dy = 0.0;
    }

    /// Mouse motion while grabbed (alternate control), in virtual pixels.
    pub fn add_look(&mut self, dx: f32, dy: f32) {
        self.look_dx += dx;
        self.look_dy += dy;
    }

    fn zones(p: &Pointer) -> (f32, f32, f32, f32) {
        let mx = p.width * 0.15;
        let my = (768.0 - 192.0) * 0.15;
        (mx, 64.0 + my, p.width - mx, 640.0 - my)
    }

    /// `cam_EdgeZone`: which edge the cursor is in (65 px beyond the scroll zone).
    pub fn edge(&self, p: &Pointer) -> Edge {
        let (l, t, r, b) = Self::zones(p);
        let mut e = Edge::None;
        if p.x < l - 65.0 {
            e = Edge::Left;
        } else if p.x > r + 65.0 {
            e = Edge::Right;
        }
        if p.y < t - 65.0 && self.look_y <= 1.0 {
            e = Edge::Up;
        } else if p.y > b + 65.0 && self.look_y >= -1.0 {
            e = Edge::Down;
        }
        e
    }

    /// Slideshow-mode click: jumps one step towards the clicked edge.
    pub fn step(&mut self, edge: Edge) {
        match edge {
            Edge::Left => self.yaw -= 0.7855,
            Edge::Right => self.yaw += 0.7855,
            Edge::Up => self.look_y = if self.look_y < -0.5 { 0.0 } else { 1.0 },
            Edge::Down => self.look_y = if self.look_y > 0.5 { 0.0 } else { -1.0 },
            Edge::None => {}
        }
        self.wrap();
    }

    fn wrap(&mut self) {
        if self.yaw > TWO_PI {
            self.yaw = 0.0;
        } else if self.yaw < 0.0 {
            self.yaw = TWO_PI;
        }
    }

    /// One frame of camera motion. Returns whether the camera moved.
    pub fn update(&mut self, p: &Pointer, alt_control: bool, cam_speed: i32, speed: f32) -> bool {
        if self.moving {
            if self.accel < 1.0 {
                self.accel += 0.025;
            }
        } else {
            self.accel = 0.0;
        }
        self.moving = false;
        if alt_control {
            // The original keeps the OS cursor near the centre and turns by its offset each
            // frame while pulling it back by a tenth; the pending offset reproduces that.
            let k = 0.0003 * speed;
            if self.look_dx.abs() > 0.01 {
                self.yaw += self.look_dx * k * 10.0 * 0.1;
                self.moving = true;
            }
            if self.look_dy < 0.0 && self.look_y <= 1.75 || self.look_dy > 0.0 && self.look_y >= -2.0 {
                self.look_y -= self.look_dy * k * 10.0 * 0.1;
                self.moving = self.moving || self.look_dy.abs() > 0.01;
            }
            let keep = 0.9f32.powf(speed);
            self.look_dx *= keep;
            self.look_dy *= keep;
            self.look_y = self.look_y.clamp(-2.0, 1.75);
        } else {
            let (l, t, r, b) = Self::zones(p);
            let f = cam_speed as f32 * 0.001 * self.accel.max(0.025) * speed;
            if p.x < l {
                self.yaw -= (l - p.x) * 0.005 * f;
                self.moving = true;
            } else if p.x > r {
                self.yaw += (p.x - r) * 0.005 * f;
                self.moving = true;
            }
            if p.y < t && self.look_y <= 1.75 {
                self.look_y += (t - p.y) * 0.005 * f;
                self.moving = true;
            } else if p.y > b && self.look_y >= -1.75 {
                self.look_y -= (p.y - b) * 0.005 * f;
                self.moving = true;
            }
        }
        self.wrap();
        self.moving
    }

    /// Target of `scLookAt(face, ...)`.
    pub fn face_target(face: i32) -> Option<(f32, f32)> {
        Some(match face {
            sc::FRONT => (3.14, 0.0),
            sc::BACK => (0.0, 0.0),
            sc::LEFT => (1.57, 0.0),
            sc::RIGHT => (4.71, 0.0),
            sc::UP => (3.14, 6.28),
            sc::DOWN => (3.14, -1.57),
            _ => return None,
        })
    }

    /// One step of the smooth `scLookAt` animation. Returns true when both axes arrived.
    pub fn approach(&mut self, target: (f32, f32), done: &mut (bool, bool), speed: f32) -> bool {
        let (ty, tp) = target;
        let dy = ty - self.yaw;
        let dp = tp - self.look_y;
        let dist = if dy < 0.0 { 1.0 - dy } else { dy };
        let step_y = (dy + 1.0) * speed * 0.01;
        let step_p = (dp + 1.0) * speed * 0.01;
        if !done.0 {
            if dy <= 0.0 {
                self.yaw -= step_y.abs();
            } else {
                self.yaw += step_y;
            }
            if dist <= step_y.abs() || (ty - self.yaw).signum() != dy.signum() {
                self.yaw = ty;
                done.0 = true;
            }
        }
        if !done.1 {
            if tp > self.look_y {
                self.look_y += step_p.abs();
                if self.look_y >= tp {
                    self.look_y = tp;
                    done.1 = true;
                }
            } else if tp < self.look_y {
                self.look_y -= step_p.abs();
                if self.look_y <= tp {
                    self.look_y = tp;
                    done.1 = true;
                }
            } else {
                done.1 = true;
            }
        }
        done.0 && done.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_faces_plus_z() {
        let c = Camera::default();
        let v = c.view();
        // A point straight ahead (on the FRONT face at +z) projects to the view centre (-z eye).
        let p = v.transform_point3(Vec3::new(0.0, 0.0, 512.0));
        assert!(p.x.abs() < 1.0 && p.z < 0.0, "{p:?}");
    }

    #[test]
    fn edge_scrolling_turns_left() {
        let mut c = Camera::default();
        let p = Pointer { x: 0.0, y: 384.0, width: 1024.0 };
        let yaw = c.yaw;
        for _ in 0..10 {
            c.update(&p, false, 50, 1.0);
        }
        assert!(c.yaw < yaw);
    }
}
