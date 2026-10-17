//! Window and input via winit, driven by explicit pumping.
//!
//! The original runs a `PeekMessage` loop and also pumps messages from inside blocking script
//! calls (`scSleep`, `scLookAt(SMOOTH)`, cutscenes, voiced lines). `pump_app_events` gives the
//! same control: [`Platform::pump`] can be called from any nesting depth and returns the input
//! events that arrived since the last call.

use std::sync::Arc;
use std::time::Duration;

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::platform::pump_events::EventLoopExtPumpEvents;
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

/// Gamepad steps of the view (d-pad, shoulder buttons).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadAction {
    TurnLeft,
    TurnRight,
    LookUp,
    LookDown,
}

#[derive(Clone, Debug)]
pub enum Input {
    MouseMove { x: f32, y: f32 },
    /// Relative motion while the cursor is grabbed (alternate control).
    MouseDelta { dx: f32, dy: f32 },
    Press(Button),
    Release(Button),
    Key(KeyCode),
    KeyUp(KeyCode),
    Pad(PadAction),
    /// Right stick: camera turn in radians since the last pump.
    PadLook { dx: f32, dy: f32 },
    Resize(u32, u32),
    Quit,
}

struct Handler {
    window: Option<Arc<Window>>,
    attrs: winit::window::WindowAttributes,
    events: Vec<Input>,
}

impl ApplicationHandler for Handler {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_none() {
            match el.create_window(self.attrs.clone()) {
                Ok(w) => self.window = Some(Arc::new(w)),
                Err(e) => {
                    log::error!("cannot create window: {e}");
                    self.events.push(Input::Quit);
                }
            }
        }
    }

    fn window_event(&mut self, _el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.events.push(Input::Quit),
            WindowEvent::Resized(s) => self.events.push(Input::Resize(s.width, s.height)),
            WindowEvent::CursorMoved { position, .. } => {
                self.events.push(Input::MouseMove { x: position.x as f32, y: position.y as f32 })
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    MouseButton::Left => Button::Left,
                    MouseButton::Right => Button::Right,
                    _ => return,
                };
                self.events.push(match state {
                    ElementState::Pressed => Input::Press(b),
                    ElementState::Released => Input::Release(b),
                });
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(k) = event.physical_key {
                    if event.state == ElementState::Released {
                        self.events.push(Input::KeyUp(k));
                    } else if !event.repeat {
                        self.events.push(Input::Key(k));
                    }
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: winit::event::DeviceId, event: winit::event::DeviceEvent) {
        if let winit::event::DeviceEvent::MouseMotion { delta } = event {
            self.events.push(Input::MouseDelta { dx: delta.0 as f32, dy: delta.1 as f32 });
        }
    }
}

pub struct Platform {
    event_loop: EventLoop<()>,
    handler: Handler,
    grabbed: bool,
    pub pad: crate::gamepad::Gamepad,
    /// Last pointer position in window pixels (the left stick moves it).
    cursor: (f32, f32),
}

impl Platform {
    pub fn new(title: &str, width: u32, height: u32, fullscreen: bool, position: Option<(i32, i32)>) -> anyhow::Result<Platform> {
        let event_loop = EventLoop::new()?;
        let mut attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(PhysicalSize::new(width, height))
            .with_min_inner_size(PhysicalSize::new(320, 240));
        if let Some((x, y)) = position {
            attrs = attrs.with_position(PhysicalPosition::new(x, y));
        }
        if fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let mut p = Platform { event_loop, handler: Handler { window: None, attrs, events: Vec::new() },
            grabbed: false,
            pad: crate::gamepad::Gamepad::new(),
            cursor: (width as f32 / 2.0, height as f32 / 2.0),
        };
        // The window is created on the first `resumed`.
        for _ in 0..100 {
            p.pump_raw(Some(Duration::from_millis(10)));
            if p.handler.window.is_some() {
                break;
            }
        }
        let window = p.handler.window.clone().ok_or_else(|| anyhow::anyhow!("window was not created"))?;
        window.set_cursor_visible(false);
        if !fullscreen {
            // Moving onto a monitor with another DPI rescales the window; keep the exact size.
            for _ in 0..10 {
                let _ = window.request_inner_size(PhysicalSize::new(width, height));
                p.pump_raw(Some(Duration::from_millis(10)));
                if window.inner_size() == PhysicalSize::new(width, height) {
                    break;
                }
            }
            p.handler.events.clear();
        }
        Ok(p)
    }

    fn pump_raw(&mut self, timeout: Option<Duration>) {
        let _ = self.event_loop.pump_app_events(timeout, &mut self.handler);
    }

    pub fn window(&self) -> Arc<Window> {
        self.handler.window.clone().expect("window")
    }

    /// Processes pending OS events without blocking and returns the collected input.
    pub fn pump(&mut self) -> Vec<Input> {
        self.pump_raw(Some(Duration::ZERO));
        let mut events = std::mem::take(&mut self.handler.events);
        for ev in &events {
            if let Input::MouseMove { x, y } = ev {
                self.cursor = (*x, *y);
            }
        }
        let size = self.handler.window.as_ref().map_or((1.0, 1.0), |w| {
            let s = w.inner_size();
            (s.width as f32, s.height as f32)
        });
        if let Some(pos) = self.pad.poll(&mut events, self.cursor, size) {
            self.cursor = pos;
            if !self.grabbed {
                // Keep the OS cursor with the pad so the mouse continues from there.
                self.warp(pos.0, pos.1);
            }
        }
        events
    }

    /// Queues a synthetic event, delivered by the next `pump` like real input.
    pub fn inject(&mut self, ev: Input) {
        self.handler.events.push(ev);
    }

    pub fn set_fullscreen(&self, on: bool) {
        self.window().set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
    }

    /// Distinct sizes the current monitor supports, largest first.
    pub fn resolutions(&self) -> Vec<(u32, u32)> {
        let mut v: Vec<(u32, u32)> = self
            .window()
            .current_monitor()
            .map(|m| m.video_modes().map(|vm| (vm.size().width, vm.size().height)).collect())
            .unwrap_or_default();
        v.retain(|&(w, h)| w >= 640 && h >= 480);
        v.sort_by(|a, b| (b.0 * b.1).cmp(&(a.0 * a.1)).then(b.0.cmp(&a.0)));
        v.dedup();
        v
    }

    /// Applies a display mode. `size` (0, 0) means the desktop size.
    pub fn apply_display(&self, display: crate::options::Display, size: (u32, u32)) {
        use crate::options::Display;
        let w = self.window();
        match display {
            Display::Windowed => {
                w.set_fullscreen(None);
                let (sw, sh) = if size == (0, 0) { (1280, 720) } else { size };
                let _ = w.request_inner_size(PhysicalSize::new(sw, sh));
            }
            Display::Borderless => w.set_fullscreen(Some(Fullscreen::Borderless(None))),
            Display::Exclusive => {
                let mode = w.current_monitor().and_then(|m| {
                    let target = if size == (0, 0) { (m.size().width, m.size().height) } else { size };
                    m.video_modes()
                        .filter(|vm| (vm.size().width, vm.size().height) == target)
                        .max_by_key(|vm| (vm.refresh_rate_millihertz(), vm.bit_depth()))
                });
                match mode {
                    Some(m) => w.set_fullscreen(Some(Fullscreen::Exclusive(m))),
                    None => w.set_fullscreen(Some(Fullscreen::Borderless(None))),
                }
            }
        }
    }

    /// Mouse-look mode: the OS cursor is confined and hidden; motion arrives as deltas.
    pub fn set_grab(&mut self, grab: bool) {
        if grab == self.grabbed {
            return;
        }
        let w = self.window();
        let ok = if grab {
            w.set_cursor_grab(CursorGrabMode::Locked).or_else(|_| w.set_cursor_grab(CursorGrabMode::Confined)).is_ok()
        } else {
            w.set_cursor_grab(CursorGrabMode::None).is_ok()
        };
        if ok {
            self.grabbed = grab;
        }
    }

    pub fn warp(&self, x: f32, y: f32) {
        let _ = self.window().set_cursor_position(PhysicalPosition::new(x as f64, y as f64));
    }
}
