//! Frame composition (`frame_Render` 0x416db0 and the `render_*` functions).
//!
//! Drawing order, blend modes and colours follow the original. Coordinates of 2D elements are
//! the original's 1024×768 layout converted to a top-left origin (`y = 768 - y_gl`).

use glam::{Mat4, Vec3};

use crate::camera::Camera;
use crate::engine::{CursorMode, Engine};
use crate::gfx::{slot, Blend, Factor, Gfx, BAR, VIRTUAL_H, VIRTUAL_W};
use crate::sc;

const WHITE: [f32; 4] = [1.0; 4];

/// Ambience presets → (sky base, sky gradient tint), cloud tint, screen tint (rgb, a).
fn sky_colors(preset: i32) -> ([f32; 3], [f32; 3]) {
    match preset {
        sc::DAWN_PRESET => ([1.0, 1.0, 0.75], [0.8, 0.05, 0.05]),
        sc::MORNING_PRESET => ([0.9, 0.9, 0.9], [0.55, 0.65, 0.8]),
        sc::DUSK_PRESET => ([0.45, 0.5, 0.6], [1.0, 0.0, 0.0]),
        _ => ([0.7, 0.7, 0.9], [0.15, 0.15, 0.8]),
    }
}

fn cloud_color(preset: i32) -> [f32; 3] {
    match preset {
        sc::DAWN_PRESET => [1.0, 0.7, 0.55],
        sc::MORNING_PRESET => [0.75, 0.75, 0.75],
        sc::DUSK_PRESET => [0.75, 0.55, 0.45],
        _ => [0.3, 0.3, 0.3],
    }
}

fn screen_color(preset: i32) -> [f32; 4] {
    match preset {
        sc::DAWN_PRESET => [1.0, 0.55, 0.55, 0.025],
        sc::MORNING_PRESET => [1.0, 1.0, 1.0, 0.0],
        sc::DUSK_PRESET => [1.0, 0.25, 0.25, 0.02],
        _ => [0.0, 0.0, 0.15, 0.15],
    }
}

/// Animated ambience colours: each value walks towards its preset (`FUN_0040cbf0` & co.).
#[derive(Debug, Clone)]
pub struct Ambience {
    pub sky_base: [f32; 3],
    pub sky_tint: [f32; 3],
    pub cloud: [f32; 3],
    pub screen: [f32; 4],
    initialised: bool,
    /// Cloud layer alphas and their cross-fade state machine (SKY_DETAIL 2).
    pub layer_a: f32,
    pub layer_b: f32,
    phase: i32,
    pause: f32,
    pub cloud_scroll: f32,
    pub fog_scroll: f32,
    pub rain_scroll: f32,
    pub gradient_slide: f32,
}

impl Default for Ambience {
    fn default() -> Self {
        Ambience {
            sky_base: [0.0; 3],
            sky_tint: [0.0; 3],
            cloud: [0.0; 3],
            screen: [0.0; 4],
            initialised: false,
            layer_a: 1.0,
            layer_b: 0.0,
            phase: 0,
            pause: 0.0,
            cloud_scroll: 0.0,
            fog_scroll: 0.0,
            rain_scroll: 0.0,
            gradient_slide: 0.0,
        }
    }
}

fn approach(v: &mut f32, target: f32, step: f32) {
    if (*v - target).abs() <= step {
        *v = target;
    } else if *v < target {
        *v += step;
    } else {
        *v -= step;
    }
}

impl Ambience {
    /// Moves the colours one frame towards the presets (`instant` snaps, like `scSetAmbience(..., 1)`).
    pub fn update(&mut self, presets: [i32; 3], instant: bool) {
        let (base, tint) = sky_colors(presets[0]);
        let cloud = cloud_color(presets[1]);
        let screen = screen_color(presets[2]);
        if instant || !self.initialised {
            self.sky_base = base;
            self.sky_tint = tint;
            self.cloud = cloud;
            self.screen = screen;
            self.initialised = true;
            return;
        }
        for i in 0..3 {
            approach(&mut self.sky_base[i], base[i], 0.0001);
            approach(&mut self.sky_tint[i], tint[i], 0.0001);
            approach(&mut self.cloud[i], cloud[i], 0.0001);
        }
        for i in 0..4 {
            approach(&mut self.screen[i], screen[i], 0.001);
        }
    }

    /// `FUN_0040da60` timeline: cross-fades two cloud layers.
    fn step_layers(&mut self, detail: i32) {
        if detail != 2 {
            return;
        }
        match self.phase {
            0 => {
                self.layer_b += 0.0005;
                if self.layer_b >= 0.5 {
                    self.phase = 1;
                }
            }
            1 => {
                self.layer_a -= 0.0005;
                self.layer_b += 0.0005;
                if self.layer_b >= 1.0 {
                    self.phase = 3;
                }
            }
            3 => {
                self.pause += 0.001;
                if self.pause >= 1.0 {
                    self.pause = 0.0;
                    self.phase = 4;
                }
            }
            4 => {
                self.layer_a += 0.0005;
                if self.layer_a >= 0.5 {
                    self.phase = 5;
                }
            }
            5 => {
                self.layer_b -= 0.0005;
                self.layer_a += 0.0005;
                if self.layer_a >= 1.0 {
                    self.phase = 6;
                }
            }
            _ => {
                self.pause += 0.001;
                if self.pause >= 1.0 {
                    self.pause = 0.0;
                    self.phase = 0;
                }
            }
        }
    }
}

/// Sky dome (`FUN_00418af0`): radius 150, 5°×5° steps, texture tiling 12×4, as a triangle list.
pub struct Dome {
    pub tris: Vec<(Vec3, (f32, f32))>,
}

impl Dome {
    pub fn new() -> Dome {
        let (r, dt, dp, ht, vt) = (150.0f32, 5.0f32, 5.0f32, 12.0f32, 4.0f32);
        let d2r = std::f32::consts::PI / 180.0;
        let vertex = |phi: f32, theta: f32| {
            let p = Vec3::new(r * (phi * d2r).sin() * (theta * d2r).cos(), r * (phi * d2r).sin() * (theta * d2r).sin(), r * (phi * d2r).cos());
            let n = p.normalize();
            let u = n.x.atan2(n.z) / std::f32::consts::TAU * ht + 0.5;
            let v = n.y.asin() / std::f32::consts::PI * vt + 0.5;
            (p, (u, v))
        };
        let mut tris = Vec::new();
        let mut phi = 0.0;
        while phi <= 90.0 - dp {
            let mut theta = 0.0;
            while theta <= 360.0 - dt {
                let mut q = [vertex(phi, theta), vertex(phi + dp, theta), vertex(phi, theta + dt), vertex(phi + dp, theta + dt)];
                // Fix texture seams like the original's post-pass.
                for i in 0..4 {
                    for j in 0..4 {
                        if q[j].1 .0 - q[i].1 .0 > 0.9 {
                            q[i].1 .0 += 1.0;
                        }
                        if q[j].1 .1 - q[i].1 .1 > 0.8 {
                            q[i].1 .1 += 1.0;
                        }
                    }
                }
                tris.extend_from_slice(&[q[0], q[1], q[2], q[2], q[1], q[3]]);
                theta += dt;
            }
            phi += dp;
        }
        Dome { tris }
    }
}

impl Engine {
    fn scene_matrices(&self) -> Mat4 {
        let g = self.gfx.borrow();
        let fov = self.fov.get();
        let (aspect, fov) = if g.content.is_some() {
            (VIRTUAL_W / (VIRTUAL_H - 2.0 * BAR), fov)
        } else {
            let (_, _, w, h) = g.layout.scene_rect();
            (w / h.max(1.0), g.layout.scene_fov(fov))
        };
        Camera::projection(fov, aspect) * self.cam.borrow().view()
    }

    /// Canvas mapping of a flat picture covering `rect` under a port scaling option.
    fn flat_affine(&self, rect: [f32; 4], mode: crate::options::FlatScale) -> crate::gfx::Affine {
        let l = self.gfx.borrow().layout;
        l.fit(rect, mode).unwrap_or(l.canvas())
    }

    fn hotspots_visible(&self) -> bool {
        self.cfg.borrow().show_hotspots || self.held_keys.get().hotspots && !self.flags.borrow().lock
    }

    /// One frame of everything on screen; `present` shows it (otherwise it only feeds a capture).
    pub fn render_frame(&self, present: bool) {
        self.begin_frame();
        let state = self.world.borrow().state;
        let (lock, waiting) = {
            let f = self.flags.borrow();
            (f.lock, f.waiting)
        };
        let cursor_visible = !lock || waiting;
        match state {
            sc::ST_VIDEO => {
                self.draw_video();
                self.draw_feedback();
            }
            sc::ST_ZOOM => {
                self.draw_zoom();
                self.draw_feedback();
                if cursor_visible {
                    self.draw_cursor();
                }
            }
            sc::ST_INV => {
                self.draw_inventory();
                self.draw_feedback();
                if cursor_visible {
                    self.draw_cursor();
                }
            }
            sc::ST_OVERLAY => {
                self.draw_under(self.world.borrow().prev_state);
                self.draw_overlay();
                self.draw_feedback();
                if !lock {
                    self.draw_cursor();
                }
            }
            sc::ST_MENU => {
                self.draw_menu_screen();
                self.draw_feedback();
                if cursor_visible {
                    self.draw_cursor();
                }
            }
            sc::ST_CHOICE => {
                let prev = self.world.borrow().prev_state;
                self.draw_under(prev);
                self.draw_textlist(prev == sc::ST_MENU);
                self.draw_cursor();
            }
            sc::ST_BOOK => {
                self.draw_band_image(slot::LOGO);
                self.draw_journal();
                self.draw_feedback();
                if cursor_visible {
                    self.draw_cursor();
                }
            }
            _ => {
                self.update_camera();
                self.draw_node();
                self.draw_feedback();
                if cursor_visible {
                    self.draw_cursor();
                }
            }
        }
        self.draw_history();
        let gamma = [0.0, 0.33, 0.66, 1.0][self.cfg.borrow().gamma.clamp(0, 3) as usize];
        let mut g = self.gfx.borrow_mut();
        g.gamma = gamma;
        g.flush(present);
    }

    /// `render_BeginFrame` (0x407af0): the dream effect sways the field of view.
    fn begin_frame(&self) {
        let dream = self.cfg.borrow().fx.dream;
        let base = self.cfg.borrow().fov;
        if !dream {
            self.fov.set(base);
            return;
        }
        let mut fx = self.fx.borrow_mut();
        let mut fov = self.fov.get();
        if fx.dream_fov_dir == 1 {
            fov += 0.005;
            if fov > 55.0 {
                fx.dream_fov_dir = 2;
            }
        } else {
            fov -= 0.005;
            if fov < 35.0 {
                fx.dream_fov_dir = 1;
            }
        }
        self.fov.set(fov);
    }

    fn update_camera(&self) {
        let (motion, lock) = {
            let f = self.flags.borrow();
            (f.motion, f.lock)
        };
        let quick = self.cfg.borrow().quickmode;
        if !motion || quick || lock {
            return;
        }
        let p = self.pointer();
        let (alt, cam_speed) = {
            let c = self.cfg.borrow();
            (c.alt_control, c.cam_speed)
        };
        let speed = self.speed.get();
        self.cam.borrow_mut().update(&p, alt && self.mouse.get().grabbed, cam_speed, speed);
    }

    /// The scene under a menu-like state (overlay, choice list).
    fn draw_under(&self, prev: i32) {
        match prev {
            sc::ST_NODE => self.draw_node(),
            sc::ST_ZOOM => self.draw_zoom(),
            sc::ST_INV => self.draw_inventory(),
            sc::ST_MENU => self.draw_menu_screen(),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ 3D

    fn cube_face(&self, g: &mut Gfx, vp: &Mat4, tex: u32, face: usize, blend: Blend, color: [f32; 4], v_shift: f32) {
        let s = 1.0f32;
        let (w, h) = g.size(tex).unwrap_or((2, 2));
        let (lo_u, hi_u) = (0.5 / w as f32, 1.0 - 0.5 / w as f32);
        let (lo_v, hi_v) = (0.5 / h as f32 + v_shift, 1.0 - 0.5 / h as f32);
        // Corners in screen order (top-left, top-right, bottom-right, bottom-left) as seen from
        // inside the cube; FRONT is +z, LEFT is +x, as in `render_Cube` / `hotspot_Emit`.
        let pts = match face {
            0 => [Vec3::new(s, s, s), Vec3::new(-s, s, s), Vec3::new(-s, -s, s), Vec3::new(s, -s, s)],
            1 => [Vec3::new(-s, s, -s), Vec3::new(s, s, -s), Vec3::new(s, -s, -s), Vec3::new(-s, -s, -s)],
            2 => [Vec3::new(s, s, -s), Vec3::new(s, s, s), Vec3::new(s, -s, s), Vec3::new(s, -s, -s)],
            3 => [Vec3::new(-s, s, s), Vec3::new(-s, s, -s), Vec3::new(-s, -s, -s), Vec3::new(-s, -s, s)],
            4 => [Vec3::new(s, s, -s), Vec3::new(-s, s, -s), Vec3::new(-s, s, s), Vec3::new(s, s, s)],
            _ => [Vec3::new(s, -s, s), Vec3::new(-s, -s, s), Vec3::new(-s, -s, -s), Vec3::new(s, -s, -s)],
        };
        g.quad_3d(tex, blend, color, vp, pts, [(lo_u, lo_v), (hi_u, lo_v), (hi_u, hi_v), (lo_u, hi_v)]);
    }

    /// `render_Cube(base)`: six faces from consecutive slots (faces, masks), or a solid colour.
    fn draw_cube(&self, base: Option<u32>, blend: Blend, color: [f32; 4]) {
        let vp = self.scene_matrices();
        let mut g = self.gfx.borrow_mut();
        for face in 0..6 {
            let tex = base.map_or(slot::WHITE, |b| b + face as u32);
            self.cube_face(&mut g, &vp, tex, face, blend, color, 0.0);
        }
    }

    fn draw_gradient_cube(&self, blend: Blend, color: [f32; 4], shift: f32) {
        let vp = self.scene_matrices();
        let mut g = self.gfx.borrow_mut();
        for face in 0..6 {
            let tex = match face {
                5 => slot::WHITE,
                4 => slot::WHITE,
                _ => slot::GRADIENT,
            };
            let c = if face == 4 { [0.0, 0.0, 0.0, 0.0] } else { color };
            self.cube_face(&mut g, &vp, tex, face, blend, c, shift);
        }
    }

    /// Cloud/fog/rain dome with an optional rotation.
    fn draw_dome(&self, tex: u32, blend: Blend, color: [f32; 4], model: Mat4, uv_scale: f32, v_offset: f32) {
        let vp = self.scene_matrices() * model;
        let dome = self.dome.borrow();
        let mut g = self.gfx.borrow_mut();
        for t in dome.tris.chunks_exact(6) {
            // Two triangles of a quad: emit as one quad (a, b, d, c) to reuse the quad batcher.
            let (a, b, c, d) = (t[0], t[1], t[2], t[5]);
            let uv = |p: (f32, f32)| (p.0 * uv_scale, p.1 * uv_scale + v_offset);
            g.quad_3d(tex, blend, color, &vp, [a.0, b.0, d.0, c.0], [uv(a.1), uv(b.1), uv(d.1), uv(c.1)]);
        }
    }

    /// Sky, clouds, fog or rain behind the node, then the masks that let it through.
    fn draw_sky(&self, zoom: bool) {
        let (detail, fx, presets) = {
            let c = self.cfg.borrow();
            (c.sky_detail, c.fx.clone(), self.ambience.get())
        };
        let speed = self.speed.get();
        let sky_speed = self.cfg.borrow().sky_speed;
        if detail == -1 {
            return;
        }
        let mut amb = self.amb.borrow_mut();
        amb.update(presets, false);
        let clouds = Mat4::from_translation(Vec3::new(0.0, -100.0, 0.0)) * Mat4::from_rotation_x(270f32.to_radians());
        let mask_blend;
        if !fx.sky {
            if fx.fog {
                drop(amb);
                self.draw_cube(None, Blend::ADD, [0.25, 0.25, 0.25, 1.0]);
                let mut amb = self.amb.borrow_mut();
                amb.fog_scroll += speed * 0.00005 * (sky_speed / 10) as f32;
                let off = amb.fog_scroll;
                drop(amb);
                if detail > 0 {
                    self.draw_dome(slot::CLOUDS1, Blend::ALPHA, [1.0, 1.0, 1.0, 0.5], clouds, 1.0, off);
                }
                if detail == 2 {
                    self.draw_dome(slot::CLOUDS1, Blend::ALPHA, [1.0, 1.0, 1.0, 0.5], clouds, 1.0, off);
                }
                mask_blend = Blend(Factor::SrcColor, Factor::One);
            } else if fx.rain {
                drop(amb);
                self.draw_cube(None, Blend::ADD, WHITE);
                self.draw_rain();
                mask_blend = Blend(Factor::DstColor, Factor::One);
            } else {
                return;
            }
        } else {
            let (base, tint, cloud, a, b) = (amb.sky_base, amb.sky_tint, amb.cloud, amb.layer_a, amb.layer_b);
            amb.gradient_slide = (amb.gradient_slide + 0.001).min(0.25);
            let slide = amb.gradient_slide;
            amb.cloud_scroll += speed * 0.00005 * (sky_speed / 10) as f32;
            let off = amb.cloud_scroll;
            amb.step_layers(detail);
            drop(amb);
            self.draw_cube(None, Blend::ADD, [base[0], base[1], base[2], 1.0]);
            if detail == 2 {
                self.draw_gradient_cube(Blend::ALPHA, [tint[0], tint[1], tint[2], 1.0], slide);
            }
            if detail >= 1 {
                self.draw_dome(slot::CLOUDS1, Blend::ALPHA, [cloud[0], cloud[1], cloud[2], a], clouds, 1.0, off);
                if detail == 2 {
                    let c2 = [cloud[0] - 0.06, cloud[1] - 0.06, cloud[2] - 0.06, b];
                    self.draw_dome(slot::CLOUDS2, Blend::ALPHA, c2, clouds, 1.0, off);
                }
            }
            mask_blend = Blend(Factor::SrcColor, Factor::One);
        }
        if zoom {
            self.draw_band(slot::ZOOM_MASK, mask_blend, WHITE);
        } else {
            self.draw_cube(Some(slot::MASK), mask_blend, WHITE);
        }
    }

    /// Two rain hemispheres (`FUN_0040deb0`).
    fn draw_rain(&self) {
        let speed = self.speed.get();
        let off = {
            let mut a = self.amb.borrow_mut();
            a.rain_scroll += speed * 0.035;
            a.rain_scroll
        };
        let r1 = Mat4::from_axis_angle(Vec3::Y, 270f32.to_radians());
        let r2 = Mat4::from_axis_angle(-Vec3::Y, 270f32.to_radians());
        let c = [1.0, 1.0, 1.0, 0.5];
        self.draw_dome(slot::RAIN, Blend(Factor::DstColor, Factor::OneMinusSrcAlpha), c, r1, 2.0, off);
        self.draw_dome(slot::RAIN, Blend(Factor::One, Factor::OneMinusSrcAlpha), c, r2, 2.0, off);
    }

    /// `render_Node` (0x4093b0).
    fn draw_node(&self) {
        let sky_active = {
            let c = self.cfg.borrow();
            c.sky_detail != -1 && (c.fx.sky || c.fx.fog || c.fx.rain)
        };
        self.draw_sky(false);
        let main_blend = if sky_active { Blend::MULTIPLY } else { Blend::ADD };
        self.draw_effects_cube(main_blend);
        if self.hotspots_visible() {
            self.draw_hotspots();
        }
        self.draw_fade();
    }

    /// `render_Effects(0)`: the node faces plus night, lightning, screen tint and candle light.
    fn draw_effects_cube(&self, main_blend: Blend) {
        let fx = self.cfg.borrow().fx.clone();
        let faces = Some(slot::FACE);
        if fx.night {
            self.draw_cube(faces, main_blend, WHITE);
            self.draw_cube(faces, Blend::ALPHA, [0.1, 0.1, 0.5, 0.5]);
        } else if !fx.rain || fx.candle {
            self.draw_cube(faces, main_blend, WHITE);
            let s = self.amb.borrow().screen;
            if s[3] > 0.0 {
                self.draw_cube(Some(slot::MASK), Blend::ALPHA, s);
            }
        } else {
            self.draw_cube(faces, main_blend, WHITE);
            match self.storm_phase() {
                0 => self.draw_cube(faces, Blend::ALPHA, [0.5, 0.5, 0.5, 0.5]),
                1 => {
                    self.draw_cube(faces, Blend::ADD, [0.25, 0.25, 0.25, 0.25]);
                    self.draw_cube(None, Blend::ALPHA, [1.0, 1.0, 1.0, 0.05]);
                }
                2 => {
                    self.draw_cube(faces, Blend::ADD, [0.5, 0.5, 0.5, 0.5]);
                    self.draw_cube(None, Blend::ALPHA, [1.0, 1.0, 1.0, 0.15]);
                }
                _ => {
                    self.draw_cube(faces, Blend::ADD, [0.75, 0.75, 0.75, 0.75]);
                    self.draw_cube(None, Blend::ALPHA, [1.0, 1.0, 1.0, 0.2]);
                }
            }
        }
        if fx.candle || fx.lamp {
            let (a_glow, a_light) = self.candle_alphas(fx.lamp);
            self.draw_cube(None, Blend::ALPHA_ADD, [1.0, 0.75, 0.5, a_glow]);
            self.draw_cube(faces, Blend::ALPHA_ADD, [1.0, 0.75, 0.5, a_light]);
        }
    }

    /// Lightning timeline of the storm (`render_Effects`, rain branch), 30.5 s period.
    fn storm_phase(&self) -> i32 {
        let mut fx = self.fx.borrow_mut();
        let t = fx.storm_start.elapsed().as_secs_f64();
        let marks: [(f64, i32); 12] = [
            (10.0, 1),
            (10.2, 0),
            (10.4, 2),
            (10.6, 0),
            (15.0, 1),
            (15.5, 0),
            (20.0, 1),
            (20.1, 0),
            (20.2, 1),
            (20.4, 0),
            (30.0, 3),
            (30.5, 0),
        ];
        let mut phase = 0;
        for (at, p) in marks {
            if t >= at {
                phase = p;
            }
        }
        if t >= 30.5 {
            fx.storm_start = std::time::Instant::now();
        }
        phase
    }

    /// Candle (or lamp, dimmer) flicker: (overlay alpha, light alpha), 2.5 s period.
    fn candle_alphas(&self, lamp: bool) -> (f32, f32) {
        let mut fx = self.fx.borrow_mut();
        let l = if lamp { 1.0 } else { 0.0 };
        let base = l * 0.25;
        let t = fx.candle_start.elapsed().as_secs_f32();
        let marks: [(f32, i32); 21] = [
            (0.1, 1),
            (0.2, 0),
            (0.3, 1),
            (0.4, 0),
            (0.75, 1),
            (0.85, 0),
            (0.9, 1),
            (1.0, 2),
            (1.1, 1),
            (1.2, 0),
            (1.3, 1),
            (1.4, 0),
            (1.8, 1),
            (1.9, 0),
            (2.0, 1),
            (2.15, 0),
            (2.2, 1),
            (2.3, 2),
            (2.4, 1),
            (2.5, 0),
            (f32::MAX, 0),
        ];
        let mut state = 0;
        for (at, s) in marks {
            if t >= base + at {
                state = s;
            }
        }
        if t >= base + 2.5 {
            fx.candle_start = std::time::Instant::now();
        }
        match state {
            2 => (0.075 - l * 0.055, 0.75 - l * 0.55),
            1 => (0.05 - l * 0.035, 0.5 - l * 0.35),
            _ => (0.01 - l * 0.05, 0.1 - l * 0.5),
        }
    }

    /// Debug view of hotspot rectangles (`SC_SHOW_HOTSPOTS`).
    fn draw_hotspots(&self) {
        let vp = self.scene_matrices();
        let objects: Vec<_> = {
            let w = self.world.borrow();
            let n = &w.nodes[w.cur_node()];
            n.objects.iter().map(|&o| w.objects[o].clone()).filter(|o| o.enabled).collect()
        };
        let mut g = self.gfx.borrow_mut();
        let (fill, edge) = hotspot_colors();
        for o in objects {
            let r = o.rect;
            let (x0, y0, x1, y1) = (r.x as f32, r.y as f32, (r.x + r.w) as f32, (r.y + r.h) as f32);
            let mut rect = |x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 4]| {
                let pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| face_point(o.face, x, y) / 512.0);
                g.quad_3d(slot::WHITE, Blend::ADD, c, &vp, pts, [(0.0, 0.0); 4]);
            };
            rect(x0, y0, x1, y1, fill);
            let t = 3.0;
            rect(x0, y0, x1, y0 + t, edge);
            rect(x0, y1 - t, x1, y1, edge);
            rect(x0, y0 + t, x0 + t, y1 - t, edge);
            rect(x1 - t, y0 + t, x1, y1 - t, edge);
        }
    }

    /// Zoom hotspots (800-wide coordinates scaled by 1.28, like `pick`).
    fn draw_zoom_hotspots(&self) {
        let rects: Vec<_> = {
            let w = self.world.borrow();
            w.zooms
                .get(w.cur_zoom())
                .map(|z| z.objects.iter().map(|&o| &w.objects[o]).filter(|o| o.enabled).map(|o| o.rect).collect())
                .unwrap_or_default()
        };
        let mut g = self.gfx.borrow_mut();
        let (fill, edge) = hotspot_colors();
        let uv = [0.0, 0.0, 1.0, 1.0];
        for r in rects {
            let k = 1.28;
            let (x0, y0, x1, y1) = (r.x as f32 * k, r.y as f32 * k, (r.x + r.w) as f32 * k, (r.y + r.h) as f32 * k);
            let t = 2.0;
            g.quad(slot::WHITE, Blend::ADD, fill, [x0, y0, x1, y1], uv);
            g.quad(slot::WHITE, Blend::ADD, edge, [x0, y0, x1, y0 + t], uv);
            g.quad(slot::WHITE, Blend::ADD, edge, [x0, y1 - t, x1, y1], uv);
            g.quad(slot::WHITE, Blend::ADD, edge, [x0, y0 + t, x0 + t, y1 - t], uv);
            g.quad(slot::WHITE, Blend::ADD, edge, [x1 - t, y0 + t, x1, y1 - t], uv);
        }
    }

    // ------------------------------------------------------------------ 2D scenes

    /// Cross-fade overlay of the captured previous frame (end of every `render_*`).
    fn draw_fade(&self) {
        let speed = self.speed.get();
        let mov = self.cfg.borrow().mov_speed as f32 * 0.001;
        let alpha = {
            let mut f = self.fade.borrow_mut();
            if !f.active {
                return;
            }
            let a = f.alpha;
            f.alpha += mov * speed;
            if f.alpha >= 1.0 {
                f.alpha = 0.0;
                f.active = false;
            }
            a
        };
        self.gfx.borrow_mut().quad_fullscreen(slot::CAPTURE, Blend::ALPHA, [1.0, 1.0, 1.0, 1.0 - alpha]);
    }

    /// Image in the 3D band area (zooms, menu background, journal): the canvas band 64..704.
    /// Images narrower than 1024 px were made for an 800×600 screen and are scaled by 1.28.
    pub fn band_rect(&self, s: u32) -> [f32; 4] {
        let (w, h) = self.gfx.borrow().size(s).unwrap_or((1024, 640));
        let (sx, sy) = flat_scale(w);
        [0.0, BAR, w as f32 * sx, BAR + h as f32 * sy]
    }

    fn draw_band(&self, s: u32, blend: Blend, color: [f32; 4]) {
        let r = self.band_rect(s);
        self.gfx.borrow_mut().quad(s, blend, color, r, [0.0, 0.0, 1.0, 1.0]);
    }

    fn draw_band_image(&self, s: u32) {
        self.draw_band(s, Blend::ADD, WHITE);
    }

    /// `render_Zoom` (0x40a880): sky through the zoom mask, then the picture multiplied on top.
    fn draw_zoom(&self) {
        let mode = self.cfg.borrow().port.zoom_scale;
        let a = self.flat_affine(self.band_rect(slot::ZOOM), mode);
        self.gfx.borrow_mut().content = Some(a);
        self.draw_zoom_picture();
        if self.hotspots_visible() {
            self.draw_zoom_hotspots();
        }
        self.gfx.borrow_mut().content = None;
        self.draw_fade();
    }

    fn draw_zoom_picture(&self) {
        let (night, has_mask) = {
            let c = self.cfg.borrow();
            (c.fx.night, self.gfx.borrow().loaded(slot::ZOOM_MASK))
        };
        let sky = {
            let c = self.cfg.borrow();
            c.sky_detail != -1 && (c.fx.sky || c.fx.fog || c.fx.rain)
        };
        if !night && has_mask && sky {
            self.draw_sky(true);
            self.draw_band(slot::ZOOM, Blend::MULTIPLY, WHITE);
        } else {
            self.draw_band(slot::ZOOM, Blend::ADD, WHITE);
            if night {
                self.draw_band(slot::ZOOM, Blend::ALPHA, [0.1, 0.1, 0.5, 0.5]);
            }
        }
    }

    fn draw_video(&self) {
        let frame = self.video.borrow_mut().as_mut().and_then(|v| v.poll());
        if let Some(img) = frame {
            self.gfx.borrow_mut().upload(slot::VIDEO, &img);
        }
        let finished = self.video.borrow().as_ref().is_none_or(|v| v.finished());
        if self.gfx.borrow().loaded(slot::VIDEO) {
            let (w, h) = self.gfx.borrow().size(slot::VIDEO).unwrap();
            // `render_Video`: quads of `render_FlatQuad` cases 2 (<800 px), 3 (<1024) and 1.
            let (sx, sy) = if w < 800 {
                (1312.0 / (0.8 * 1024.0), 1389.0 / (0.7465 * 1024.0))
            } else if w < 1024 {
                (1312.0 / 1024.0, 1366.0 / 1024.0)
            } else {
                (1.0, 1.0)
            };
            let r = [0.0, BAR, w as f32 * sx, BAR + h as f32 * sy];
            let a = self.flat_affine(r, self.cfg.borrow().port.video_scale);
            let mut g = self.gfx.borrow_mut();
            g.content = Some(a);
            g.quad(slot::VIDEO, Blend::ADD, WHITE, r, [0.0, 0.0, 1.0, 1.0]);
            g.content = None;
        }
        self.draw_fade();
        if finished {
            // The clip ended: fade from its last frame back to the scene.
            self.gfx.borrow_mut().request_capture();
            {
                let mut f = self.fade.borrow_mut();
                f.active = true;
                f.alpha = 0.0;
            }
            self.world.borrow_mut().set_state(sc::ST_LAST);
        }
    }

    /// `render_Overlay` (0x40ab40).
    fn draw_overlay(&self) {
        let (w, h) = self.gfx.borrow().size(slot::OVERLAY).unwrap_or((1024, 640));
        let (sx, sy) = flat_scale(w);
        let r = [0.0, BAR, w as f32 * sx, BAR + h as f32 * sy];
        let a = self.flat_affine(r, self.cfg.borrow().port.zoom_scale);
        let mut g = self.gfx.borrow_mut();
        g.content = Some(a);
        g.quad(slot::OVERLAY, Blend::ALPHA, WHITE, r, [0.0, 0.0, 1.0, 1.0]);
        g.content = None;
        self.draw_fade();
    }

    /// `render_Inventory` (0x409650): the captured scene tinted sepia with the item icons.
    fn draw_inventory(&self) {
        let speed = self.speed.get();
        let mov = self.cfg.borrow().mov_speed as f32 * 0.001;
        let alpha = {
            let mut f = self.fade.borrow_mut();
            let a = f.alpha.min(0.5);
            if f.active {
                f.alpha += mov * speed;
                if f.alpha >= 0.5 {
                    f.alpha = 0.5;
                    f.active = false;
                }
            }
            a
        };
        let held = self.world.borrow().held;
        let slots: Vec<(usize, usize)> = {
            let w = self.world.borrow();
            w.inventory.iter().enumerate().skip(1).map(|(i, &id)| (i, id)).collect()
        };
        let mut g = self.gfx.borrow_mut();
        g.quad_fullscreen(slot::CAPTURE, Blend::ALPHA, [0.9, 0.75, 0.5, 1.0 - alpha]);
        for (i, item) in slots {
            if item == 0 || item == held {
                continue;
            }
            let (col, row) = ((i - 1) % 10, (i - 1) / 10);
            if row >= 7 {
                continue;
            }
            let x = 32.0 + 96.0 * col as f32;
            let y = 66.0 + 96.0 * row as f32;
            g.quad(slot::ICON + item as u32, Blend::ALPHA, [1.0, 1.0, 1.0, (alpha * 2.0).min(1.0)], [x, y, x + 64.0, y + 64.0], [
                0.0, 0.0, 1.0, 1.0,
            ]);
        }
    }

    /// `render_Journal` (0x40b120).
    fn draw_journal(&self) {
        let lines: Vec<(f32, f32, String)> =
            self.journal.borrow().visible().into_iter().map(|(x, y, s)| (x, y, s.to_owned())).collect();
        for (x, y, s) in lines {
            self.draw_text(x, y, &s, [0.0, 0.0, 0.0, 1.0], Blend::ALPHA);
        }
        self.draw_fade();
    }

    /// Menu background, the menu itself and its save slots (`STMENU` branch of `frame_Render`).
    fn draw_menu_screen(&self) {
        let (has_bg, mode, started, quick_exists) = {
            let m = self.menu.borrow();
            (m.background.is_some(), m.mode, m.started, m.slot_exists[11])
        };
        if has_bg {
            self.draw_band_image(slot::LOGO);
        }
        let view = if mode == 0 && !started {
            -1
        } else if mode == 1 && quick_exists {
            3
        } else {
            mode
        };
        self.draw_menu(view);
    }

    /// `render_Menu(mode)` (0x408440).
    fn draw_menu(&self, mode: i32) {
        let cx = VIRTUAL_W / 2.0;
        let cy = VIRTUAL_H / 2.0;
        let gl = |y: f32| VIRTUAL_H - y;
        let size = |s: u32| self.gfx.borrow().size(s).unwrap_or((128, 128));
        // Logo, 512×512 slot centred near the top.
        let (lw, lh) = size(slot::MENU);
        let ox = (512.0 - lw as f32) / 2.0;
        let oy = (512.0 - lh as f32) / 2.0;
        let x0 = cx - 256.0 + ox;
        let top = gl(cy - oy + 420.0);
        self.gfx.borrow_mut().quad(slot::MENU, Blend::ALPHA, WHITE, [x0, top, x0 + lw as f32, top + lh as f32], [0.0, 0.0, 1.0, 1.0]);
        let (bw, bh) = size(slot::MENU + 2);
        let ix = (128.0 - bw as f32) / 2.0;
        let iy = (112.0 - bh as f32) / 2.0 + 112.0;
        let (hover, disabled, labels, exists) = {
            let m = self.menu.borrow();
            (m.hover, m.disabled, m.slot_labels.clone(), m.slot_exists)
        };
        // Menu textures live in 128×128 slots: the quad spans the slot, the image its top-left.
        let button = |slot_i: u32, x0: f32, y_gl_top: f32, w: f32, h: f32, highlight: bool, dim: bool| {
            let (iw, ih) = size(slot_i);
            let (w, h) = (w * iw as f32 / 128.0, h * ih as f32 / 128.0);
            let rect = [x0, gl(y_gl_top), x0 + w, gl(y_gl_top) + h];
            let mut g = self.gfx.borrow_mut();
            let c = if dim { [0.5, 0.5, 0.5, 1.0] } else { WHITE };
            g.quad(slot_i, Blend::ALPHA, c, rect, [0.0, 0.0, 1.0, 1.0]);
            if highlight && !dim {
                g.quad(slot_i, Blend::ALPHA_ADD, [0.0, 1.0, 1.0, 0.5], rect, [0.0, 0.0, 1.0, 1.0]);
            }
        };
        match mode {
            1..=6 => {
                let title = match mode {
                    2 => slot::MENU + 5,
                    4..=6 => slot::MENU + 3,
                    _ => slot::MENU + 4,
                };
                button(title, cx + ix - 48.0, cy - iy + 108.0, 96.0, 96.0, false, false);
                if mode >= 4 {
                    let rows = if mode == 4 { self.settings_rows() } else { self.port_rows(mode) };
                    for (i, row) in rows.iter().enumerate() {
                        let y = VIRTUAL_H - 276.0 + 16.0 * i as f32 - 16.0;
                        let hl = hover == Some(6 + i);
                        self.draw_text(cx - 145.0, y, row, WHITE, Blend::ALPHA);
                        if hl {
                            self.draw_text(cx - 145.0, y, row, [0.0, 1.0, 1.0, 0.5], Blend::ALPHA_ADD);
                        }
                    }
                } else {
                    let first = if mode == 3 { 0 } else { 1 };
                    for i in first..11 {
                        let y = VIRTUAL_H - 276.0 + 16.0 * i as f32 - 16.0;
                        let label = &labels[i];
                        let usable = mode != 1 && mode != 3 || exists[if i == 0 { 11 } else { i }];
                        let c = if usable { WHITE } else { [0.6, 0.6, 0.6, 1.0] };
                        self.draw_text(cx - 120.0, y, label, c, Blend::ALPHA);
                        if hover == Some(6 + i) {
                            self.draw_text(cx - 120.0, y, label, [0.0, 1.0, 1.0, 0.5], Blend::ALPHA_ADD);
                        }
                    }
                }
                button(slot::MENU + 1, cx + ix - 64.0, cy - iy - 126.0, 128.0, 128.0, hover == Some(0), false);
            }
            _ => {
                let start = if mode == -1 { 1 } else { 0 };
                for k in start..6usize {
                    let s = slot::MENU + 1 + k as u32;
                    let y_top = cy + 64.0 - iy - 36.0 * k as f32;
                    let dim = disabled.get(k).copied().unwrap_or(false);
                    button(s, cx + ix - 56.0, y_top, 112.0, 120.0, hover == Some(k), dim);
                }
            }
        }
        self.draw_fade();
    }

    fn settings_rows(&self) -> Vec<String> {
        let c = self.cfg.borrow();
        let onoff = |b: bool| if b { "      ON" } else { "     OFF" };
        let speed = |v: i32| match v {
            25 => "    SLOW",
            75 => "    FAST",
            _ => "  NORMAL",
        };
        vec![
            format!("hints                {}", onoff(c.hints)),
            format!("surround music       {}", onoff(c.surround)),
            format!("camera speed         {}", speed(c.cam_speed)),
            format!("text speed           {}", speed(c.txt_speed)),
            format!("gamma correction     {}", ["DISABLED", "     LOW", "  MEDIUM", "    HIGH"][c.gamma.clamp(0, 3) as usize]),
            format!("slideshow mode       {}", onoff(c.quickmode)),
            format!("alternate control    {}", if c.alt_control { " ENABLED" } else { "DISABLED" }),
            format!("language             {:>8}", c.language),
            "port options ...".into(),
        ]
    }

    /// Rows of the port options pages (menu modes 5 and 6); the last one flips the page.
    fn port_rows(&self, mode: i32) -> Vec<String> {
        let c = self.cfg.borrow();
        let page = crate::options::PAGES[(mode - 5) as usize];
        let mut rows: Vec<String> = page
            .iter()
            .map(|&r| format!("{:<18}{:>11}", crate::options::PortOptions::label(r), c.port.value(r)))
            .collect();
        rows.push(if mode == 5 { "more ..." } else { "previous page" }.into());
        rows
    }

    /// `render_TextList` (0x40b2e0): dialogue options (left, cyan) or a centred menu question.
    fn draw_textlist(&self, menu: bool) {
        let t = self.textlist.borrow();
        let n = t.items.len();
        let base = if n == 4 { -8.0 } else { 8.0 };
        for (i, s) in t.items.iter().enumerate() {
            let y = VIRTUAL_H - base - 16.0 * i as f32 - 16.0;
            let (x, color) = if menu {
                let x = VIRTUAL_W / 2.0 - (s.chars().count() / 2) as f32 * 10.0 - 4.0;
                (x, if i == n - 1 { WHITE } else { [0.1, 0.75, 0.75, 1.0] })
            } else {
                (16.0, [0.1, 0.75, 0.75, 1.0])
            };
            self.draw_text(x, y, s, color, Blend::ALPHA);
            if t.hover == Some(i) {
                self.draw_text(x, y, s, [0.1, 1.0, 1.0, 1.0], Blend::ALPHA);
            }
        }
    }

    /// `render_Feedback` (0x40ad10): subtitle lines centred in the bottom bar.
    fn draw_feedback(&self) {
        let (txt_speed, glow) = {
            let c = self.cfg.borrow();
            (c.txt_speed, c.text_glow)
        };
        let speed = self.speed.get();
        let lines = self.feedback.borrow_mut().update(txt_speed, glow, speed);
        let (large, boxed) = {
            let p = &self.cfg.borrow().port;
            (p.large_subtitles, p.subtitle_box)
        };
        // Port options: 1.5× text stacked up from the bottom edge, and a dark backdrop.
        let k = if large { 1.5 } else { 1.0 };
        let lowest = lines.iter().map(|l| l.1).fold(f32::MAX, f32::min);
        for (s, base, alpha, glow_a) in lines {
            let width = s.chars().count() as f32 * 10.0 * k;
            let x = if large { (VIRTUAL_W - width) / 2.0 } else { VIRTUAL_W / 2.0 - (s.chars().count() / 2) as f32 * 10.0 + 1.0 };
            let y = if large {
                let rank = ((base - lowest) / 16.0).round();
                VIRTUAL_H - 6.0 - 24.0 * (rank + 1.0)
            } else {
                VIRTUAL_H - base - 16.0
            };
            if boxed && !s.trim().is_empty() {
                let r = [x - 8.0, y - 1.0, x + width + 8.0 * k, y + 16.0 * k + 1.0];
                self.gfx.borrow_mut().quad(slot::WHITE, Blend::ALPHA, [0.0, 0.0, 0.0, 0.65 * alpha], r, [0.0, 0.0, 1.0, 1.0]);
            }
            self.draw_text_scaled(x, y, &s, [1.0, 1.0, 1.0, alpha], Blend::ALPHA, k);
            if glow_a > 0.0 {
                for _ in 0..3 {
                    self.draw_text_scaled(x, y, &s, [1.0, 1.0, 1.0, glow_a], Blend(Factor::SrcColor, Factor::DstColor), k);
                }
            }
        }
    }

    /// Port option: the subtitle/feedback history while H is held.
    fn draw_history(&self) {
        if !self.held_keys.get().history || !self.cfg.borrow().port.history {
            return;
        }
        // Messages oldest first from the top, a gap between them, the newest in white.
        let msgs: Vec<Vec<String>> = self.history.borrow().iter().map(|t| wrap(t, 96)).collect();
        let rows = 32;
        self.gfx.borrow_mut().quad_fullscreen(slot::WHITE, Blend::ALPHA, [0.0, 0.0, 0.0, 0.85]);
        let mut used = 0;
        let mut first = msgs.len();
        while first > 0 && used + msgs[first - 1].len() <= rows {
            used += msgs[first - 1].len();
            first -= 1;
        }
        let mut y = 24.0;
        for (i, m) in msgs.iter().enumerate().skip(first) {
            let c = if i + 1 == msgs.len() { WHITE } else { [0.7, 0.7, 0.7, 1.0] };
            for l in m {
                self.draw_text(32.0, y, l, c, Blend::ALPHA);
                y += 17.0;
            }
            y += 4.0;
        }
        let shown = &msgs[first..];
        if shown.is_empty() {
            self.draw_text(32.0, VIRTUAL_H / 2.0, "no messages yet", [0.6, 0.6, 0.6, 1.0], Blend::ALPHA);
        }
    }

    /// `text_Draw` (0x40c130): 16×16 cells of a 16×16 glyph atlas, 10 px advance; `(x, y)` is the
    /// top-left of the first cell in virtual pixels.
    pub fn draw_text(&self, x: f32, y: f32, text: &str, color: [f32; 4], blend: Blend) {
        self.draw_text_scaled(x, y, text, color, blend, 1.0);
    }

    fn draw_text_scaled(&self, x: f32, y: f32, text: &str, color: [f32; 4], blend: Blend, k: f32) {
        let mut g = self.gfx.borrow_mut();
        let mut cx = x;
        let cell = 16.0 * k;
        for b in crate::script::encode(text) {
            let code = b as u32;
            let (col, row) = ((code % 16) as f32, (code / 16) as f32);
            let (u0, v0) = (col / 16.0, row / 16.0);
            g.quad(slot::FONT, blend, color, [cx, y, cx + cell, y + cell], [u0, v0, u0 + 1.0 / 16.0, v0 + 1.0 / 16.0]);
            cx += 10.0 * k;
        }
    }

    /// `render_Cursor` (0x40b490): sprite anchored per cursor kind; item icons centred.
    fn draw_cursor(&self) {
        let m = self.mouse.get();
        let (s, mode) = self.cursor.get();
        let l = self.gfx.borrow().layout;
        // Offsets of the sprite's top-left from the hot spot, and its size, in virtual px.
        let (left, top, size) = match s {
            slot::CURSOR_MENU | slot::CURSOR_FORWARD | slot::CURSOR_LOOK => (-8.0, -8.0, 64.0),
            slot::CURSOR_NORMAL | slot::CURSOR_USE | slot::CURSOR_BACK => (-24.0, -24.0, 64.0),
            _ => (-32.0, -32.0, 64.0),
        };
        let k = l.scale;
        let (x, y) = (m.x + left * k, m.y + top * k);
        let rect = [(x, y), (x + size * k, y), (x + size * k, y + size * k), (x, y + size * k)];
        let uv = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let mut g = self.gfx.borrow_mut();
        g.quad_window(s, Blend::ALPHA, WHITE, rect, uv);
        if mode == CursorMode::Target {
            for i in 0..2 {
                let d = (i + 1) as f32 * k;
                let r = [(x - d, y - d), (x + size * k + d, y - d), (x + size * k + d, y + size * k + d), (x - d, y + size * k + d)];
                g.quad_window(s, Blend::ALPHA_ADD, WHITE, r, uv);
            }
        }
    }

    /// `render_Splash` (0x4078d0): the loading picture shown while resources load.
    pub fn render_splash(&self) {
        self.draw_band_image(slot::LOGO);
        self.gfx.borrow_mut().flush(true);
    }
}

/// Pixel `(x, y)` on a node face → point on the cube of half-size 512 (`hotspot_Emit`).
/// Greedy word wrap at `width` characters.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// Hotspot highlight (fill, outline), pulsing gently.
fn hotspot_colors() -> ([f32; 4], [f32; 4]) {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f32() % 60.0);
    let k = 0.75 + 0.25 * (t * 4.0).sin();
    ([0.05 * k, 0.12 * k, 0.13 * k, 1.0], [0.3 * k, 0.65 * k, 0.7 * k, 1.0])
}

pub fn face_point(face: i32, x: f32, y: f32) -> Vec3 {
    let s = 512.0;
    match face {
        sc::FRONT => Vec3::new(s - x, s - y, s),
        sc::BACK => Vec3::new(x - s, s - y, -s),
        sc::LEFT => Vec3::new(s, s - y, x - s),
        sc::RIGHT => Vec3::new(-s, s - y, s - x),
        sc::UP => Vec3::new(s - x, s, y - s),
        _ => Vec3::new(s - x, -s, s - y),
    }
}

/// Scale of band images (`render_FlatQuad`): 1024-wide pictures are drawn 1:1, smaller ones
/// (made for 800×600) go through a 1312×1389 quad over their 1024² texture, i.e. 800×472 → 1024×640.
fn flat_scale(w: u32) -> (f32, f32) {
    if w >= 1024 {
        (1.0, 1.0)
    } else {
        (1312.0 / 1024.0, 1389.0 / 1024.0)
    }
}
