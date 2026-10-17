//! Gamepad support (port option): the pad drives the same input stream as mouse and keyboard.
//!
//! | Pad | Acts as |
//! |---|---|
//! | left stick | mouse cursor |
//! | right stick | smooth camera look in nodes |
//! | A / Cross | left click |
//! | B / Circle | right click (inventory, back) |
//! | X / Square (hold) | TAB: hotspot highlight |
//! | Y / Triangle (hold) | H: message history |
//! | D-pad, LB/RB | turn and look in 90° steps |
//! | Start | Escape (menu, skip) |

use std::time::Instant;

use gilrs::{Axis, Button as PadButton, EventType, Gilrs};
use winit::keyboard::KeyCode;

use crate::platform::{Button, Input, PadAction};

const DEAD_ZONE: f32 = 0.2;

pub struct Gamepad {
    gilrs: Option<Gilrs>,
    pub enabled: bool,
    last: Instant,
}

impl Gamepad {
    pub fn new() -> Gamepad {
        let gilrs = match Gilrs::new() {
            Ok(g) => {
                for (_, pad) in g.gamepads() {
                    log::info!("gamepad: {}", pad.name());
                }
                Some(g)
            }
            Err(e) => {
                log::warn!("gamepad support unavailable: {e}");
                None
            }
        };
        Gamepad { gilrs, enabled: true, last: Instant::now() }
    }

    /// Translates pad events since the last call into input events. `cursor` is the pointer
    /// position in window pixels and `size` the window size; returns the new cursor position
    /// when the left stick moved it.
    pub fn poll(&mut self, out: &mut Vec<Input>, cursor: (f32, f32), size: (f32, f32)) -> Option<(f32, f32)> {
        let dt = self.last.elapsed().as_secs_f32().min(0.1);
        self.last = Instant::now();
        let gilrs = self.gilrs.as_mut()?;
        let mut active = None;
        while let Some(ev) = gilrs.next_event() {
            match ev.event {
                EventType::Connected => log::info!("gamepad connected: {}", gilrs.gamepad(ev.id).name()),
                EventType::Disconnected => log::info!("gamepad disconnected"),
                _ => {}
            }
            if !self.enabled {
                continue;
            }
            let (down, b) = match ev.event {
                EventType::ButtonPressed(b, _) => (true, b),
                EventType::ButtonReleased(b, _) => (false, b),
                _ => continue,
            };
            active = Some(ev.id);
            let key = |k: KeyCode| if down { Input::Key(k) } else { Input::KeyUp(k) };
            let click = |b: Button| if down { Input::Press(b) } else { Input::Release(b) };
            let pad = |a: PadAction| down.then_some(Input::Pad(a));
            let input = match b {
                PadButton::South => Some(click(Button::Left)),
                PadButton::East => Some(click(Button::Right)),
                PadButton::West => Some(key(KeyCode::Tab)),
                PadButton::North => Some(key(KeyCode::KeyH)),
                PadButton::Start => Some(key(KeyCode::Escape)),
                PadButton::DPadLeft | PadButton::LeftTrigger => pad(PadAction::TurnLeft),
                PadButton::DPadRight | PadButton::RightTrigger => pad(PadAction::TurnRight),
                PadButton::DPadUp => pad(PadAction::LookUp),
                PadButton::DPadDown => pad(PadAction::LookDown),
                _ => None,
            };
            out.extend(input);
        }
        if !self.enabled {
            return None;
        }
        // Sticks of every connected pad (usually one).
        let (mut lx, mut ly, mut rx, mut ry) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for (id, pad) in gilrs.gamepads() {
            if active.is_some() && active != Some(id) {
                continue;
            }
            let v = |a: Axis| {
                let x = pad.value(a);
                if x.abs() < DEAD_ZONE {
                    0.0
                } else {
                    // Rescale past the dead zone, squared for fine control near the centre.
                    let t = (x.abs() - DEAD_ZONE) / (1.0 - DEAD_ZONE);
                    t * t * x.signum()
                }
            };
            lx += v(Axis::LeftStickX);
            ly += v(Axis::LeftStickY);
            rx += v(Axis::RightStickX);
            ry += v(Axis::RightStickY);
        }
        if rx != 0.0 || ry != 0.0 {
            // Radians per second at full deflection.
            out.push(Input::PadLook { dx: rx * 2.2 * dt, dy: ry * 1.6 * dt });
        }
        if lx == 0.0 && ly == 0.0 {
            return None;
        }
        // A full deflection crosses the window height in about 0.8 s.
        let speed = size.1 * 1.25;
        let x = (cursor.0 + lx * speed * dt).clamp(0.0, size.0 - 1.0);
        let y = (cursor.1 - ly * speed * dt).clamp(0.0, size.1 - 1.0);
        out.push(Input::MouseMove { x, y });
        Some((x, y))
    }
}

impl Default for Gamepad {
    fn default() -> Self {
        Self::new()
    }
}
