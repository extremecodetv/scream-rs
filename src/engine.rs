//! Engine core: main loop, state machine, transitions, input and the script-facing operations.
//!
//! Control flow follows scream.exe. The whole game runs inside the Lua call of the game script:
//! `scRun()` enters the main loop and only returns on quit. Blocking script calls (`scSleep`,
//! voiced lines, cutscenes, smooth `scLookAt`) run nested frame loops — while they wait, input is
//! drained and ignored except Escape, which skips. Lua callbacks (clicks, timers, room entry)
//! are invoked by name and may nest further; no `RefCell` borrow is held across a Lua call.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use mlua::Lua;
use winit::keyboard::KeyCode;

use crate::audio::Audio;
use crate::camera::{Camera, Edge, Pointer};
use crate::config::Settings;
use crate::gfx::{self, slot, Gfx, Image};
use crate::platform::{Button, Input, Platform};
use crate::res::{Kind, Resources};
use crate::sc;
use crate::script::{self, Args, Ret};
use crate::ui::{Feedback, Journal, TextList, JOURNAL_SPREAD};
use crate::video::Video;
use crate::world::{Id, World};

/// What the cursor shows next to the arrow while an item is involved (`g_cursor_mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorMode {
    Normal,
    /// 0xc9: item just dropped.
    Released,
    /// 0xca: carrying an item.
    Holding,
    /// 0xcb: carrying an item over something that reacts.
    Target,
}

#[derive(Debug)]
pub struct Flags {
    /// `scLock`: input and camera frozen, no cursor (`g_lock`).
    pub lock: bool,
    /// Wait cursor shown (`g_waiting`).
    pub waiting: bool,
    /// Camera may move (`g_motion`); `scStill` / sleeping clear it.
    pub motion: bool,
    /// Kind of the last `scSwitch` (`g_last_switch`).
    pub last_switch: i32,
    pub quit: bool,
    pub initialized: bool,
}

#[derive(Debug, Default)]
pub struct Fade {
    pub active: bool,
    /// 0 → old frame fully covering, 1 → finished.
    pub alpha: f32,
}

#[derive(Debug, Default)]
pub struct Hints {
    pub active: bool,
    pub open_pending: bool,
    pub grab_pending: bool,
    pub close_pending: bool,
    pub msg_open: String,
    pub msg_grab: String,
    pub msg_close: String,
    pub item: String,
}

#[derive(Debug, Default)]
pub struct MenuState {
    /// logo, back, new, settings, load, save, quit
    pub textures: [String; 7],
    pub new_game: [String; 2],
    pub intro: Option<String>,
    pub outro: Option<String>,
    pub background: Option<String>,
    pub music: Option<String>,
    pub save_func: bool,
    pub enabled: bool,
    /// `scAbort` was used: intro and outro are not played again (`+0x24c`).
    pub aborted: bool,
    /// A game is in progress (menu offers "back" and "save").
    pub started: bool,
    /// 0 main, 1 load, 2 save, 4 settings.
    pub mode: i32,
    pub saved_state: i32,
    /// Labels of save slots 0..10 ("N. text"); slot 0 is the quicksave.
    pub slot_labels: [String; 11],
    pub slot_exists: [bool; 12],
    pub hover: Option<usize>,
    pub disabled: [bool; 7],
    /// Yes/no or game-select question in progress: (item it confirms, answer).
    pub confirm: Option<usize>,
    pub answer_first: bool,
    pub answer_second: bool,
    pub saved_cloud: i32,
}

#[derive(Debug, Default)]
pub struct Subtitles {
    pub lines: Vec<(f32, f32, String)>,
    pub start: Option<Instant>,
    pub shown: Option<usize>,
}

/// Effect clocks (lightning in the rain, candle/lamp flicker, cloud scroll).
#[derive(Debug)]
pub struct FxClock {
    pub storm_start: Instant,
    pub candle_start: Instant,
    pub cloud_offset: f32,
    pub rain_offset: f32,
    pub sky_scroll: f32,
    pub dream_fov_dir: i32,
}

impl Default for FxClock {
    fn default() -> Self {
        FxClock {
            storm_start: Instant::now(),
            candle_start: Instant::now(),
            cloud_offset: 0.0,
            rain_offset: 0.0,
            sky_scroll: 0.0,
            dream_fov_dir: 1,
        }
    }
}

/// Mouse state: the pointer used for UI and picking, in window pixels.
#[derive(Debug, Default, Clone, Copy)]
pub struct Mouse {
    pub x: f32,
    pub y: f32,
    pub grabbed: bool,
}

pub struct Engine {
    pub lua: Lua,
    pub game_dir: PathBuf,
    pub user_dir: PathBuf,
    pub cfg: RefCell<Settings>,
    pub world: RefCell<World>,
    pub res: RefCell<Resources>,
    pub gfx: RefCell<Gfx>,
    pub platform: RefCell<Platform>,
    pub audio: Audio,
    pub flags: RefCell<Flags>,
    pub cam: RefCell<Camera>,
    pub fade: RefCell<Fade>,
    pub feedback: RefCell<Feedback>,
    pub textlist: RefCell<TextList>,
    pub journal: RefCell<Journal>,
    pub hints_state: RefCell<Hints>,
    pub menu: RefCell<MenuState>,
    pub subtitles: RefCell<Subtitles>,
    pub fx: RefCell<FxClock>,
    pub mouse: Cell<Mouse>,
    pub cursor: Cell<(u32, CursorMode)>,
    pub font: RefCell<String>,
    pub video: RefCell<Option<Video>>,
    /// Frame time factor normalised to 60 fps (`cfg_speed`).
    pub speed: Cell<f32>,
    /// Current field of view (the dream effect sways it).
    pub fov: Cell<f32>,
    pub dome: RefCell<crate::render::Dome>,
    pub amb: RefCell<crate::render::Ambience>,
    last_frame: Cell<Instant>,
    timer_clock: Cell<Instant>,
    /// Ambience colour presets (sky, cloud, screen) and their saved copies.
    pub ambience: Cell<[i32; 3]>,
    pub options: Options,
    /// Held keys of port options: TAB shows hotspots, H the subtitle history.
    pub held_keys: Cell<HeldKeys>,
    /// Recent subtitles and messages (port option "history on H").
    pub history: RefCell<std::collections::VecDeque<String>>,
    /// Room of the last autosave and when it happened.
    pub autosave: Cell<(Id, Instant)>,
    /// True until the first new game has saved the pristine world (`DAT_0048bc64`).
    pub new_game_snapshot: Cell<bool>,
}

pub const AUTOSAVE_SLOT: usize = 12;

#[derive(Debug, Default, Clone, Copy)]
pub struct HeldKeys {
    pub hotspots: bool,
    pub history: bool,
}

/// Command-line options for testing.
#[derive(Debug, Default, Clone)]
pub struct Options {
    pub skip_videos: bool,
    pub shot: Option<(PathBuf, u32)>,
    pub frames: Cell<u32>,
    /// Scripted input for tests: (frame, event) with positions in virtual 1024×768 pixels.
    pub inputs: Vec<(u32, TestInput)>,
    /// Lua code run once when the main loop starts.
    pub exec: Option<String>,
    /// Several screenshots: (frame, file).
    pub shots: Vec<(u32, PathBuf)>,
    /// Script to run instead of the configured one (path without extension).
    pub script: Option<String>,
    /// Windowed size and window position overrides.
    pub windowed: Option<(u32, u32)>,
    pub position: Option<(i32, i32)>,
    /// Port option overrides (`--opt KEY VALUE`), as in scream.cfg.
    pub opts: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub enum TestInput {
    Click(f32, f32),
    RightClick(f32, f32),
    Move(f32, f32),
    Key(KeyCode),
    KeyUp(KeyCode),
    Look(f32, f32),
}

impl Engine {
    pub fn new(game_dir: PathBuf, options: Options) -> anyhow::Result<Rc<Engine>> {
        let user_dir = Settings::user_dir();
        let cfg = Settings::load(&user_dir.join("scream.cfg"));
        let mut cfg = cfg;
        for (k, v) in &options.opts {
            if !cfg.port.apply(k, v) {
                log::warn!("--opt: unknown port option {k}");
            }
        }
        // The engine settles the script name from the shipped cfg when the user has none.
        if cfg.script == "scream" {
            if let Ok(text) = std::fs::read_to_string(game_dir.join("scream.cfg")) {
                let mut shipped = Settings::default();
                shipped.apply_cfg(&text);
                cfg.script = shipped.script;
            }
        }
        let (w, h) = match (options.windowed, cfg.port.resolution) {
            (Some(size), _) => size,
            (None, (0, 0)) => (1280, 720),
            (None, size) => size,
        };
        let platform = Platform::new("Scratches: Director's Cut", w, h, false, options.position)?;
        if options.windowed.is_none() {
            platform.apply_display(cfg.port.display, cfg.port.resolution);
        }
        let gfx = Gfx::new(platform.window(), cfg.vsync)?;
        let lua = unsafe { Lua::unsafe_new() };
        let engine = Rc::new_cyclic(|weak: &Weak<Engine>| {
            if let Err(e) = script::register(&lua, weak.clone()) {
                log::error!("registering sc* functions failed: {e}");
            }
            Engine {
                lua,
                res: RefCell::new(Resources::new(game_dir.clone())),
                game_dir,
                user_dir,
                cfg: RefCell::new(cfg),
                world: RefCell::new(World::new()),
                gfx: RefCell::new(gfx),
                platform: RefCell::new(platform),
                audio: Audio::new(),
                flags: RefCell::new(Flags {
                    lock: false,
                    waiting: false,
                    motion: true,
                    last_switch: 0,
                    quit: false,
                    initialized: false,
                }),
                cam: RefCell::new(Camera::default()),
                fade: RefCell::new(Fade::default()),
                feedback: RefCell::new(Feedback::default()),
                textlist: RefCell::new(TextList::default()),
                journal: RefCell::new(Journal::default()),
                hints_state: RefCell::new(Hints::default()),
                menu: RefCell::new(MenuState::default()),
                subtitles: RefCell::new(Subtitles::default()),
                fx: RefCell::new(FxClock::default()),
                mouse: Cell::new(Mouse::default()),
                cursor: Cell::new((slot::CURSOR_NORMAL, CursorMode::Normal)),
                font: RefCell::new(String::new()),
                video: RefCell::new(None),
                speed: Cell::new(1.0),
                fov: Cell::new(45.0),
                dome: RefCell::new(crate::render::Dome::new()),
                amb: RefCell::new(crate::render::Ambience::default()),
                last_frame: Cell::new(Instant::now()),
                timer_clock: Cell::new(Instant::now()),
                ambience: Cell::new([sc::MORNING_PRESET; 3]),
                held_keys: Cell::new(HeldKeys::default()),
                history: RefCell::new(Default::default()),
                autosave: Cell::new((0, Instant::now())),
                new_game_snapshot: Cell::new(true),
                options,
            }
        });
        engine.apply_volumes();
        engine.gfx.borrow_mut().layout.view = engine.cfg.borrow().port.node_view;
        engine.platform.borrow_mut().pad.enabled = engine.cfg.borrow().port.gamepad;
        engine.gfx.borrow_mut().fsr = engine.cfg.borrow().port.filter == crate::options::Filter::Fsr;
        Ok(engine)
    }

    // ------------------------------------------------------------------ boot

    /// `scream_Main`: runs `scream.sc` (constants) and the game script. Returns when the game
    /// script finishes (after `scRun`'s loop ends).
    pub fn boot(self: &Rc<Self>) -> anyhow::Result<()> {
        self.register_test_api()?;
        let script = self.options.script.clone().unwrap_or_else(|| self.cfg.borrow().script.clone());
        for name in ["scream".to_owned(), script] {
            // Like `scream_Main`: `<name>.sc`, or the Lua source `<name>.lua`.
            let base = if std::path::Path::new(&name).is_absolute() { PathBuf::from(&name) } else { self.game_dir.join(&name) };
            let sc = base.with_extension("sc");
            let lua_src = base.with_extension("lua");
            let (chunk, mode, label) = if sc.is_file() {
                let bytes = std::fs::read(&sc)?;
                (crate::bytecode::adapt(&bytes)?, mlua::ChunkMode::Binary, sc.display().to_string())
            } else if lua_src.is_file() {
                (std::fs::read(&lua_src)?, mlua::ChunkMode::Text, lua_src.display().to_string())
            } else {
                anyhow::bail!("Script file not found: {}", sc.display());
            };
            log::info!("Loading script '{label}'...");
            if let Err(e) = self.lua.load(&chunk).set_name(&label).set_mode(mode).exec() {
                if self.flags.borrow().quit {
                    break;
                }
                return Err(anyhow::anyhow!("{label}: {e}"));
            }
            if self.flags.borrow().quit {
                break;
            }
        }
        Ok(())
    }

    /// Extra globals used by the comparison harness (`difftest/harness.lua`).
    fn register_test_api(self: &Rc<Self>) -> anyhow::Result<()> {
        let g = self.lua.globals();
        let err = |e: mlua::Error| anyhow::anyhow!("{e}");
        g.set("__scream_rs", true).map_err(err)?;
        let weak = Rc::downgrade(self);
        let snap = self
            .lua
            .create_function(move |_, path: String| {
                if let Some(e) = weak.upgrade() {
                    e.render_frame(true);
                    let img = e.gfx.borrow_mut().read_frame();
                    if let Some(buf) = image::RgbaImage::from_raw(img.width, img.height, img.rgba) {
                        let _ = buf.save(&path);
                    }
                }
                Ok(())
            })
            .map_err(err)?;
        g.set("__snap", snap).map_err(err)?;
        let weak = Rc::downgrade(self);
        let dofile_sc = self
            .lua
            .create_function(move |lua, name: String| {
                let Some(e) = weak.upgrade() else { return Ok(()) };
                let path = e.game_dir.join(&name);
                let bytes = std::fs::read(&path).map_err(mlua::Error::external)?;
                let chunk = crate::bytecode::adapt(&bytes).map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;
                lua.load(&chunk).set_name(&name).set_mode(mlua::ChunkMode::Binary).exec()
            })
            .map_err(err)?;
        g.set("__dofile_sc", dofile_sc).map_err(err)?;
        let weak = Rc::downgrade(self);
        let post = self
            .lua
            .create_function(move |_, (cmd, a, b): (String, Option<f32>, Option<f32>)| {
                if let Some(e) = weak.upgrade() {
                    e.post_test_input(&cmd, a.unwrap_or(0.0), b.unwrap_or(0.0));
                }
                Ok(())
            })
            .map_err(err)?;
        g.set("__post", post).map_err(err)?;
        Ok(())
    }

    /// `scInit` → `core_Init` (0x410560): default resources, the built-in "black" zoom, then
    /// the game archive.
    pub fn init(&self) {
        if self.flags.borrow().initialized {
            return;
        }
        log::info!("Initializing...");
        {
            let mut res = self.res.borrow_mut();
            if let Err(e) = res.open_archive("scream.res") {
                log::warn!("{e}");
            }
        }
        self.load_tex(slot::LOGO, "sc_logo.tga");
        self.load_tex(slot::FONT, "font_def.tga");
        self.load_tex(slot::FONT_DEFAULT, "font_def.tga");
        for s in [slot::CURSOR_NORMAL, slot::CURSOR_FORWARD, slot::CURSOR_LOOK, slot::CURSOR_USE, slot::CURSOR_BACK] {
            self.load_tex(s, "cursor_def.tga");
        }
        self.load_tex(slot::CURSOR_WAIT, "wait_def.tga");
        self.load_tex(slot::RAIN, "rain_def.tga");
        {
            let mut w = self.world.borrow_mut();
            let _ = w.create(sc::ZOOM, "black");
            let _ = w.set_texture(sc::ZOOM, "black.jpg");
        }
        let (resource, file, splash) = {
            let c = self.cfg.borrow();
            (c.resource, c.res_file.clone(), c.splash.clone())
        };
        {
            let mut res = self.res.borrow_mut();
            if resource {
                if let Err(e) = res.open_archive(&file) {
                    log::error!("{e}");
                    self.flags.borrow_mut().quit = true;
                }
            } else {
                res.close_archive();
            }
        }
        if splash != "sc_logo.tga" {
            self.load_tex(slot::LOGO, &splash);
        }
        self.render_splash();
        self.load_tex(slot::CLOUDS1, "clouds1.tga");
        self.load_tex(slot::CLOUDS2, "clouds2.tga");
        self.load_tex(slot::GRADIENT, "gradient.tga");
        self.flags.borrow_mut().initialized = true;
    }

    /// `scRun` → `core_Run`: shows the menu (or starts exploring) and runs the main loop.
    pub fn run(&self) {
        let showmenu = self.cfg.borrow().showmenu;
        {
            let (w, h) = {
                let g = self.gfx.borrow();
                (g.layout.width, g.layout.height)
            };
            self.mouse.set(Mouse { x: w / 2.0, y: h / 2.0, grabbed: false });
        }
        if showmenu {
            self.menu_show();
        } else {
            self.world.borrow_mut().set_state(sc::ST_NODE);
        }
        log::info!("Entering main loop...");
        if let Some(code) = self.options.exec.clone() {
            if let Err(e) = self.lua.load(&code).set_name("--exec").exec() {
                log::error!("--exec: {e}");
            }
        }
        while !self.quit_requested() {
            let events = self.platform.borrow_mut().pump();
            for ev in events {
                self.dispatch(ev);
                if self.quit_requested() {
                    break;
                }
            }
            self.tick();
            self.autosave_check();
        }
    }

    pub fn quit_requested(&self) -> bool {
        self.flags.borrow().quit
    }

    /// `scTerminate`: fade to black, save settings, quit.
    pub fn terminate(&self) {
        if !self.quit_requested() {
            self.switch(sc::ZOOM, "black");
            self.sleep(1.0);
        }
        let cfg = self.cfg.borrow();
        // Test runs (overrides, scripted screenshots) leave the player's settings alone.
        let testing = !self.options.opts.is_empty() || self.options.shot.is_some() || !self.options.inputs.is_empty();
        if !testing {
            if let Err(e) = cfg.save(&self.user_dir.join("scream.cfg")) {
                log::warn!("saving settings: {e}");
            }
        }
        self.flags.borrow_mut().quit = true;
    }

    /// Escape from the game script: abort the Lua chunk by raising an error once quit is set.
    pub fn check_quit(&self) -> mlua::Result<()> {
        if self.quit_requested() {
            Err(mlua::Error::RuntimeError("quit".into()))
        } else {
            Ok(())
        }
    }

    // ------------------------------------------------------------------ helpers

    pub fn call(&self, func: &str) {
        script::call_global(&self.lua, func);
    }

    /// Runs a world mutation from a script binding, logging errors like the original's
    /// fatal-error dump (but without halting).
    pub fn world_op(&self, what: &str, f: impl FnOnce(&mut World) -> crate::world::Result<()>) {
        if let Err(e) = f(&mut self.world.borrow_mut()) {
            log::error!("{what}: {e}");
        }
    }

    pub fn read_res(&self, name: &str) -> Option<Vec<u8>> {
        if name.is_empty() {
            return None;
        }
        self.res.borrow_mut().read(name)
    }

    /// `tex_Load` (0x407730).
    pub fn load_tex(&self, s: u32, file: &str) {
        let kind = self.res.borrow().kind(file);
        if !matches!(kind, Kind::Jpeg | Kind::Tga) {
            // The original returns early and keeps whatever the slot held.
            return;
        }
        let img = self.read_res(file).and_then(|d| crate::imgload::decode(kind, &d));
        let mut gfx = self.gfx.borrow_mut();
        match img {
            Some(img) => gfx.upload(s, &img),
            None if file == "black.jpg" => gfx.upload(s, &Image::solid(16, 16, [0, 0, 0, 255])),
            None => {
                log::debug!("LoadTexture: texture {file} not loaded");
                gfx.clear_slot(s);
            }
        }
    }

    pub fn set_cursor(&self, s: u32) {
        let (_, mode) = self.cursor.get();
        self.cursor.set((s, mode));
    }

    /// `cursor_Set` (0x40b7a0): 0xc9..0xcb select the item-cursor mode, anything else the image.
    fn set_cursor_mode(&self, mode: CursorMode) {
        let (s, _) = self.cursor.get();
        self.cursor.set((s, mode));
    }

    // ------------------------------------------------------------------ frame loop

    /// `frame_Tick` (0x412340): hover, render, subtitles, and once per second the timers.
    pub fn tick(&self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame.get()).as_secs_f32().min(0.25);
        self.last_frame.set(now);
        self.speed.set((dt * 60.0).clamp(0.05, 10.0) * self.cfg.borrow().speed);
        self.update_grab();
        let m = self.mouse.get();
        self.node_click(m.x, m.y, false);
        self.render_frame(true);
        self.subtitle_update();
        if now.duration_since(self.timer_clock.get()) >= Duration::from_secs(1) {
            self.timer_clock.set(now);
            self.run_timers();
        }
        self.options.frames.set(self.options.frames.get() + 1);
        self.test_hooks();
        if let Some((path, frame)) = &self.options.shot {
            if self.options.frames.get() == *frame {
                let img = self.gfx.borrow_mut().read_frame();
                if let Some(buf) = image::RgbaImage::from_raw(img.width, img.height, img.rgba) {
                    let _ = buf.save(path);
                    log::info!("screenshot written to {}", path.display());
                }
                self.flags.borrow_mut().quit = true;
            }
        }
    }

    fn run_timers(&self) {
        let mut due = Vec::new();
        {
            let mut w = self.world.borrow_mut();
            let mut step = |timers: &mut Vec<crate::world::Timer>| {
                for t in timers.iter_mut().filter(|t| t.enabled) {
                    t.elapsed += 1;
                    if t.elapsed >= t.interval {
                        t.elapsed = 0;
                        t.enabled = false;
                        due.push(t.func.clone());
                    }
                }
            };
            if w.state == sc::ST_NODE {
                step(&mut w.global_timers);
            }
            let room = w.cur_room;
            step(&mut w.rooms[room].timers);
        }
        for f in due {
            log::debug!("timer -> {f}");
            self.call(&f);
            self.rebind_current();
        }
    }

    /// Mouse-look is active only while exploring a node with alternate control.
    fn update_grab(&self) {
        let want = {
            let st = self.world.borrow().state;
            let alt = self.cfg.borrow().alt_control;
            let f = self.flags.borrow();
            alt && st == sc::ST_NODE && !f.lock && f.motion
        };
        let mut m = self.mouse.get();
        if want != m.grabbed {
            self.platform.borrow_mut().set_grab(want);
            self.cam.borrow_mut().reset_motion();
            m.grabbed = want;
        }
        if want {
            let (x, y, w, h) = self.gfx.borrow().layout.scene_rect();
            m.x = x + w / 2.0;
            m.y = y + h / 2.0;
        }
        self.mouse.set(m);
    }

    fn dispatch(&self, ev: Input) {
        match ev {
            Input::Quit => self.flags.borrow_mut().quit = true,
            Input::Resize(w, h) => self.gfx.borrow_mut().resize(w, h),
            Input::MouseMove { x, y } => {
                let mut m = self.mouse.get();
                if !m.grabbed {
                    m.x = x;
                    m.y = y;
                    self.mouse.set(m);
                    self.input_mouse(0, 0);
                }
            }
            Input::MouseDelta { dx, dy } => {
                if self.mouse.get().grabbed && self.flags.borrow().motion {
                    let s = self.gfx.borrow().layout.scale;
                    let (sens, invert) = {
                        let p = &self.cfg.borrow().port;
                        (p.mouse_sens as f32 / 5.0, p.invert_y)
                    };
                    let dy = if invert { -dy } else { dy };
                    self.cam.borrow_mut().add_look(dx / s * sens, dy / s * sens);
                }
            }
            Input::Press(Button::Left) => self.input_mouse(1, 1),
            Input::Release(Button::Left) => self.input_mouse(1, 2),
            Input::Press(Button::Right) => self.input_mouse(2, 1),
            Input::Release(Button::Right) => self.input_mouse(2, 2),
            Input::Key(k) => {
                if !self.held_key(k, true) {
                    self.input_key(k)
                }
            }
            Input::KeyUp(k) => {
                self.held_key(k, false);
            }
            Input::Pad(a) => {
                use crate::platform::PadAction;
                self.modern_key(match a {
                    PadAction::TurnLeft => KeyCode::ArrowLeft,
                    PadAction::TurnRight => KeyCode::ArrowRight,
                    PadAction::LookUp => KeyCode::ArrowUp,
                    PadAction::LookDown => KeyCode::ArrowDown,
                });
            }
            Input::PadLook { dx, dy } => {
                let ok = {
                    let f = self.flags.borrow();
                    f.motion && !f.lock && self.world.borrow().state == sc::ST_NODE
                };
                if ok {
                    let dy = if self.cfg.borrow().port.invert_y { -dy } else { dy };
                    let mut c = self.cam.borrow_mut();
                    let (yaw, look) = (c.yaw + dx, (c.look_y + dy).clamp(-2.0, 1.75));
                    c.set(yaw, look);
                }
            }
        }
    }

    /// Drains input inside a blocking wait (`window_Pump` 0x419fb0). Returns true when Escape
    /// was pressed, which skips the wait.
    pub fn pump_skip(&self) -> bool {
        let events = self.platform.borrow_mut().pump();
        let mut skip = false;
        for ev in events {
            match ev {
                Input::Quit => {
                    self.flags.borrow_mut().quit = true;
                    skip = true;
                }
                Input::Resize(w, h) => self.gfx.borrow_mut().resize(w, h),
                Input::Key(KeyCode::Escape) => skip = true,
                // Port option: a click or Space skips the wait like Escape.
                Input::Release(Button::Left) | Input::Key(KeyCode::Space) if self.cfg.borrow().port.click_skip => skip = true,
                Input::Key(k) => {
                    self.held_key(k, true);
                }
                Input::KeyUp(k) => {
                    self.held_key(k, false);
                }
                Input::MouseMove { x, y } => {
                    // The original drops these too, but keeping the pointer current avoids a
                    // jump when the wait ends.
                    let mut m = self.mouse.get();
                    if !m.grabbed {
                        m.x = x;
                        m.y = y;
                        self.mouse.set(m);
                    }
                }
                _ => {}
            }
        }
        skip
    }

    /// `core_Sleep` (0x4127a0).
    pub fn sleep(&self, secs: f32) {
        self.wait_cursor(true);
        self.flags.borrow_mut().motion = false;
        let start = Instant::now();
        while start.elapsed().as_secs_f32() < secs && !self.quit_requested() {
            self.tick();
            if self.pump_skip() {
                for ch in 14..18 {
                    if self.audio.is_playing(ch) {
                        self.audio.stop(ch);
                    }
                }
                break;
            }
        }
        self.flags.borrow_mut().motion = true;
        self.wait_cursor(false);
    }

    /// `core_Wait` / `core_Go`: shows or hides the wait cursor (when enabled).
    pub fn wait_cursor(&self, on: bool) {
        if !self.cfg.borrow().wait_cursor {
            return;
        }
        self.flags.borrow_mut().waiting = on;
        let held = self.world.borrow().held;
        let idle = if held != 0 { slot::ICON + held as u32 } else { slot::CURSOR_NORMAL };
        self.set_cursor(if on { slot::CURSOR_WAIT } else { idle });
    }

    /// `fx_CaptureFade` (0x407990): renders the current state and cross-fades from it.
    pub fn capture_fade(&self, quick: bool) {
        // The original brackets every capture with `lock = 1 ... lock = 0`, so a capture also
        // releases an `scLock` held by the script.
        self.flags.borrow_mut().lock = true;
        self.gfx.borrow_mut().request_capture();
        self.render_frame(false);
        self.flags.borrow_mut().lock = false;
        let mut f = self.fade.borrow_mut();
        f.active = true;
        f.alpha = if quick { 0.75 } else { 0.0 };
    }

    /// `world_RebindCurrent` (0x4124e0): after a Lua handler, binds the current room, node and
    /// zoom again so scripts keep working on what is on screen.
    pub fn rebind_current(&self) {
        let mut w = self.world.borrow_mut();
        let room = w.cur_room;
        let node = w.cur_node();
        let zoom = w.cur_zoom();
        let _ = w.set_bind(sc::ROOM, room);
        let _ = w.set_bind(sc::NODE, node);
        let _ = w.set_bind(sc::ZOOM, zoom);
    }

    // ------------------------------------------------------------------ switching

    /// `core_Switch` (0x415c60).
    pub fn switch(&self, kind: i32, name: &str) {
        if kind != sc::ZOOMOUT && kind != sc::ZOOMBACK {
            let r = self.world.borrow_mut().bind(kind, name);
            if let Err(e) = r {
                log::error!("Switch: {e}");
                return;
            }
        }
        match kind {
            sc::ZOOMBACK => {
                log::debug!("Zooming back...");
                let exit = {
                    let mut w = self.world.borrow_mut();
                    if w.room().zoom_stack.is_empty() {
                        return;
                    }
                    w.room_mut().zoom_stack.clear();
                    let z = w.bound(sc::ZOOM);
                    w.zooms.get(z).map(|z| z.exit_sound.clone()).unwrap_or_default()
                };
                self.fx_play(&exit);
                self.capture_fade(false);
                self.world.borrow_mut().set_state(sc::ST_NODE);
                self.flags.borrow_mut().last_switch = sc::NODE;
            }
            sc::ZOOMOUT => {
                log::debug!("Zooming out...");
                let (remaining, exit) = {
                    let mut w = self.world.borrow_mut();
                    if w.room().zoom_stack.is_empty() {
                        return;
                    }
                    w.room_mut().zoom_stack.pop();
                    let z = w.bound(sc::ZOOM);
                    (w.room().zoom_stack.len(), w.zooms.get(z).map(|z| z.exit_sound.clone()).unwrap_or_default())
                };
                if !exit.is_empty() {
                    self.fx_play_ex(sc::CH_FX, &exit, false, false);
                }
                if remaining == 0 {
                    self.capture_fade(false);
                    self.world.borrow_mut().set_state(sc::ST_NODE);
                    self.flags.borrow_mut().last_switch = sc::NODE;
                } else {
                    self.flags.borrow_mut().last_switch = sc::ZOOMOUT;
                    let top = {
                        let w = self.world.borrow();
                        w.zooms[w.cur_zoom()].name.clone()
                    };
                    self.switch(sc::ZOOM, &top);
                }
            }
            sc::ROOM => {
                log::debug!("Switching to ROOM '{name}'");
                let (entry, on_enter) = {
                    let mut w = self.world.borrow_mut();
                    w.cur_room = w.bound(sc::ROOM);
                    let r = w.room();
                    (w.nodes.get(r.entry_node).map(|n| n.name.clone()).unwrap_or_default(), r.on_enter.clone())
                };
                self.switch(sc::NODE, &entry);
                self.call(&on_enter);
                self.flags.borrow_mut().last_switch = sc::ROOM;
            }
            sc::OVERLAY => {
                log::debug!("Switching to OVERLAY '{name}'");
                let (tex, open) = {
                    let w = self.world.borrow();
                    let o = &w.overlays[w.bound(sc::OVERLAY)];
                    (o.texture.clone(), o.open_sound.clone())
                };
                self.load_tex(slot::OVERLAY, &tex);
                self.capture_fade(false);
                self.world.borrow_mut().set_state(sc::ST_OVERLAY);
                self.fx_play(&open);
                self.flags.borrow_mut().last_switch = sc::OVERLAY;
            }
            sc::ZOOM => {
                log::debug!("Switching to ZOOM '{name}'");
                let reenter = self.flags.borrow().last_switch == sc::ZOOMOUT;
                let (tex, mask, enter) = {
                    let mut w = self.world.borrow_mut();
                    let z = w.bound(sc::ZOOM);
                    if !reenter {
                        w.room_mut().zoom_stack.push(z);
                    }
                    let zz = &w.zooms[z];
                    (zz.texture.clone(), zz.mask.clone(), zz.enter_sound.clone())
                };
                self.capture_fade(false);
                self.load_tex(slot::ZOOM, &tex);
                self.load_tex(slot::ZOOM_MASK, &mask);
                self.world.borrow_mut().set_state(sc::ST_ZOOM);
                if !reenter && !enter.is_empty() {
                    self.fx_play_ex(sc::CH_FX, &enter, false, false);
                }
                self.flags.borrow_mut().last_switch = sc::ZOOM;
            }
            sc::NODE => {
                log::debug!("Switching to NODE '{name}'");
                let node = self.world.borrow().bound(sc::NODE);
                if let Err(e) = self.world.borrow_mut().enter_node(node) {
                    log::error!("{e}");
                    return;
                }
                self.wait_cursor(true);
                self.render_frame(true);
                self.wait_cursor(false);
                self.capture_fade(false);
                let (faces, masks, step) = {
                    let w = self.world.borrow();
                    let n = &w.nodes[node];
                    (n.faces.clone(), n.masks.clone(), n.footstep.clone())
                };
                for i in 0..6 {
                    self.load_tex(slot::FACE + i as u32, &faces[i]);
                    self.load_tex(slot::MASK + i as u32, &masks[i]);
                }
                {
                    let mut w = self.world.borrow_mut();
                    w.room_mut().entry_node = node;
                    w.set_state(sc::ST_NODE);
                }
                if !step.is_empty() {
                    self.fx_play_ex(sc::CH_STEP, &step, false, false);
                }
                self.flags.borrow_mut().last_switch = sc::NODE;
            }
            _ => log::warn!("Switch: unsupported kind {kind}"),
        }
    }

    /// `scTextureUpdate(face, file, now)`: changes a face (and its mask) of the bound node or
    /// the bound zoom/overlay; with `now` the change is shown immediately with a cross-fade.
    pub fn texture_update(&self, target: i32, file: &str, now: bool) {
        if now {
            self.capture_fade(false);
        }
        self.world_op("scTextureUpdate", |w| w.set_texture(target, file));
        if !now {
            return;
        }
        let w = self.world.borrow();
        let (tex, mask, s, ms) = match target {
            sc::FRONT..=sc::DOWN => {
                let n = &w.nodes[w.bound(sc::NODE)];
                let i = (target - 1) as usize;
                (n.faces[i].clone(), n.masks[i].clone(), slot::FACE + i as u32, Some(slot::MASK + i as u32))
            }
            sc::ZOOM => {
                let z = &w.zooms[w.bound(sc::ZOOM)];
                (z.texture.clone(), z.mask.clone(), slot::ZOOM, Some(slot::ZOOM_MASK))
            }
            sc::OVERLAY => (w.overlays[w.bound(sc::OVERLAY)].texture.clone(), String::new(), slot::OVERLAY, None),
            _ => return,
        };
        drop(w);
        self.load_tex(s, &tex);
        if let Some(ms) = ms {
            self.load_tex(ms, &mask);
        }
    }

    // ------------------------------------------------------------------ object setup helpers

    pub fn on_click(&self, kind: i32, a: &str, b: &str) {
        self.world_op("scOnClick", |w| {
            let o = w.object_mut()?;
            o.action = kind;
            match kind {
                sc::FEEDBACK | sc::CUSTOM => o.target = a.into(),
                sc::FEEDBACK_EX => {
                    o.target = a.into();
                    o.audio = b.into();
                }
                sc::ZOOMOUT | sc::ZOOMBACK | sc::ROOM | sc::NODE | sc::ZOOM | sc::OVERLAY => o.target = a.into(),
                _ => {}
            }
            Ok(())
        });
    }

    /// `core_Audio`: room ambience (SC_STEREO), zoom enter/exit sounds.
    pub fn attach_audio(&self, event: i32, file: &str) {
        self.world_op("scAudio", |w| {
            match event {
                sc::STEREO => {
                    let r = w.cur_room;
                    w.rooms[r].ambience = file.into();
                }
                sc::ZOOM => {
                    let z = w.bound_id(sc::ZOOM)?;
                    w.zooms[z].enter_sound = file.into();
                }
                sc::ZOOMOUT | sc::ZOOMBACK => {
                    let z = w.bound_id(sc::ZOOM)?;
                    w.zooms[z].exit_sound = file.into();
                }
                _ => {}
            }
            Ok(())
        });
    }

    pub fn inventory_set(&self, icon: &str, title: &str, refusal: &str, audio: &str) {
        let id = {
            let mut w = self.world.borrow_mut();
            if let Err(e) = w.set_item(icon, title, refusal, audio) {
                log::error!("scInventorySet: {e}");
                return;
            }
            w.bound(sc::ITEM)
        };
        self.load_tex(slot::ICON + id as u32, icon);
    }

    /// `scCursor(kind, file)`.
    pub fn set_cursor_image(&self, kind: i32, file: &str) {
        let s = match kind {
            sc::LEFT => slot::CURSOR_LEFT,
            sc::RIGHT => slot::CURSOR_RIGHT,
            sc::UP => slot::CURSOR_UP,
            sc::DOWN => slot::CURSOR_DOWN,
            sc::NORMAL => slot::CURSOR_NORMAL,
            sc::FORWARD => slot::CURSOR_FORWARD,
            sc::LOOK => slot::CURSOR_LOOK,
            sc::USE => slot::CURSOR_USE,
            sc::WAIT => slot::CURSOR_WAIT,
            sc::MENU => slot::CURSOR_MENU,
            sc::ZOOMOUT | sc::ZOOMBACK => slot::CURSOR_BACK,
            _ => return,
        };
        self.load_tex(s, file);
    }

    pub fn set_font(&self, file: &str) {
        *self.font.borrow_mut() = file.into();
        self.load_tex(slot::FONT, file);
    }

    /// `scEnable` / `scDisable`.
    pub fn enable(&self, key: i32, on: bool) {
        if key == sc::OBJECT {
            self.world_op("scEnable", |w| {
                w.object_mut()?.enabled = on;
                Ok(())
            });
            return;
        }
        if !self.cfg.borrow_mut().set_flag(key, on) {
            log::debug!("scEnable: unknown key {key}");
        }
    }

    /// `scSet`.
    pub fn set(&self, key: i32, a: &Args) {
        let mut cfg = self.cfg.borrow_mut();
        match key {
            sc::K_LANGUAGE | sc::K_SPLASH | sc::K_RES_PATH | sc::K_RES_FILE => cfg.set_str(key, &a.str(2)),
            _ => cfg.set_num(key, a.int(2)),
        }
    }

    /// `scGet`: current room/node names, language, or a number.
    pub fn get(&self, key: i32) -> Ret {
        match key {
            sc::ROOM => {
                let w = self.world.borrow();
                Ret::Str(w.room().name.clone())
            }
            sc::NODE => {
                let w = self.world.borrow();
                Ret::Str(w.nodes.get(w.cur_node()).map(|n| n.name.clone()).unwrap_or_default())
            }
            sc::K_LANGUAGE => Ret::Str(self.cfg.borrow().language.clone()),
            _ => Ret::Num(self.cfg.borrow().get_num(key) as f64),
        }
    }

    pub fn load_settings(&self, file: &str) {
        let path = self.user_dir.join(file);
        if let Ok(text) = std::fs::read_to_string(path) {
            self.cfg.borrow_mut().apply_cfg(&text);
        }
    }

    /// `core_SetAmbience`: colour presets of sky, clouds and screen tint.
    pub fn set_ambience(&self, which: i32, preset: i32, _smooth: i32) {
        let mut a = self.ambience.get();
        match which {
            sc::K_SKY_COLOR => a[0] = preset,
            sc::K_CLOUD_COLOR => a[1] = preset,
            sc::K_SCREEN_COLOR => a[2] = preset,
            _ => return,
        }
        self.ambience.set(a);
    }

    pub fn feedback(&self, text: &str) {
        {
            let clean = text.replace('|', " ");
            let mut h = self.history.borrow_mut();
            if !clean.trim().is_empty() && h.back() != Some(&clean) {
                if h.len() >= 200 {
                    h.pop_front();
                }
                h.push_back(clean);
            }
        }
        let speed = self.cfg.borrow().txt_speed;
        self.feedback.borrow_mut().set(text, speed);
    }

    // ------------------------------------------------------------------ hints

    /// `core_Hints` (0x417460).
    pub fn hints(&self, event: i32) {
        let hints_on = self.cfg.borrow().hints;
        let mut msg = None;
        {
            let mut h = self.hints_state.borrow_mut();
            match event {
                sc::HINTS_INACTIVE => {
                    h.active = false;
                    return;
                }
                sc::HINTS_ACTIVE => h.active = true,
                _ if !h.active => return,
                _ => {}
            }
            if !hints_on {
                return;
            }
            let held_name = {
                let w = self.world.borrow();
                (w.held != 0).then(|| w.items[w.held].name.clone())
            };
            match event {
                sc::HINTS_OPEN if h.open_pending => {
                    h.open_pending = false;
                    msg = Some(h.msg_open.clone());
                }
                sc::HINTS_GRAB if h.grab_pending && held_name.as_deref() == Some(&h.item) => {
                    h.grab_pending = false;
                    msg = Some(h.msg_grab.clone());
                }
                sc::HINTS_CLOSE if h.close_pending && held_name.as_deref() == Some(&h.item) => {
                    h.close_pending = false;
                    msg = Some(h.msg_close.clone());
                }
                sc::HINTS_RESET => {
                    h.open_pending = true;
                    h.grab_pending = true;
                    h.close_pending = true;
                }
                _ => {}
            }
        }
        if let Some(m) = msg.filter(|m| !m.is_empty()) {
            self.feedback(&m);
        }
    }

    pub fn hints_set(&self, event: i32, text: &str) {
        let mut h = self.hints_state.borrow_mut();
        match event {
            sc::HINTS_OPEN => h.msg_open = text.into(),
            sc::HINTS_GRAB => h.msg_grab = text.into(),
            sc::HINTS_CLOSE => h.msg_close = text.into(),
            sc::HINTS_ITEM => h.item = text.into(),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ audio

    /// `core_FXPlay`: first idle channel of 14..17, played once.
    pub fn fx_play(&self, file: &str) {
        if file.is_empty() {
            return;
        }
        for ch in 14..18 {
            if !self.audio.is_playing(ch) {
                if self.audio.is_loaded(ch) {
                    self.audio.unload(ch);
                }
                self.audio.load(ch, file, self.read_res(file));
                self.audio.play(ch, false);
                return;
            }
        }
    }

    /// `core_FXPlayEx(channel, file, wait, loop)`; with `wait` the script blocks until the clip
    /// ends (Escape skips it).
    pub fn fx_play_ex(&self, ch: usize, file: &str, wait: bool, looped: bool) {
        if ch == 0 || ch >= crate::audio::CHANNELS {
            return;
        }
        self.audio.release(ch);
        self.audio.load(ch, file, self.read_res(file));
        self.audio.play(ch, looped);
        if wait {
            while self.audio.is_playing(ch) && !self.quit_requested() {
                self.tick();
                if self.pump_skip() {
                    self.audio.release(ch);
                    return;
                }
            }
        }
    }

    /// `core_FXSet(channel, property, value)`.
    pub fn fx_set(&self, ch: usize, prop: i32, v: i32) {
        match prop {
            sc::VOLUME => self.audio.set_gain(ch, v as f32 * 0.1),
            sc::FADEIN => self.audio.fade_in(ch, v),
            sc::FADEOUT => self.audio.fade_out(ch, v),
            _ => {}
        }
    }

    /// `track_Play` (0x416590): front on channel 1, surround halves on 2 and 3.
    pub fn track_play(&self, name: &str) {
        let track = {
            let mut w = self.world.borrow_mut();
            match w.bind(sc::TRACK, name) {
                Ok(id) => w.tracks[id].clone(),
                Err(e) => {
                    log::error!("scTrackPlay: {e}");
                    return;
                }
            }
        };
        for ch in 1..4 {
            self.audio.release(ch);
        }
        let surround = self.cfg.borrow().surround;
        self.audio.load(1, &track.front, self.read_res(&track.front));
        if surround {
            self.audio.load(2, &track.rear_left, self.read_res(&track.rear_left));
            self.audio.load(3, &track.rear_right, self.read_res(&track.rear_right));
        }
        let gain = track.volume as f32 * 0.1;
        for ch in 1..4 {
            self.audio.set_gain(ch, gain);
            if track.fade_in != 0 {
                self.audio.fade_in(ch, track.fade_in);
            }
        }
        self.audio.play(1, track.looped);
        if surround {
            self.audio.play(2, track.looped);
            self.audio.play(3, track.looped);
        }
    }

    pub fn track_stop(&self) {
        for ch in 1..4 {
            self.audio.release(ch);
        }
    }

    /// `track_FadeOut(steps)`: a fade longer than what remains of the track is shortened to 1.
    pub fn track_fade_out(&self, steps: i32) {
        let steps = if steps < 0 {
            let w = self.world.borrow();
            w.tracks.get(w.bound(sc::TRACK)).map_or(1, |t| t.fade_out)
        } else {
            steps
        };
        let steps = if steps <= self.audio.time(1, 0) { steps } else { 1 };
        for ch in 1..4 {
            self.audio.fade_out(ch, steps);
        }
    }

    // ------------------------------------------------------------------ scripts, choices

    /// `script_Go` (0x415a30): plays the bound dialogue script line by line with subtitles,
    /// ducking the music by 0.25 meanwhile.
    pub fn script_go(&self, name: &str) {
        let lines = {
            let mut w = self.world.borrow_mut();
            match w.bind(sc::SCRIPT, name) {
                Ok(id) => w.scripts[id].lines.clone(),
                Err(e) => {
                    log::error!("scScriptGo: {e}");
                    return;
                }
            }
        };
        for ch in 1..8 {
            let g = self.audio.info(ch).target_gain;
            self.audio.set_gain(ch, g - 0.25);
        }
        let hints = self.cfg.borrow().hints;
        for line in lines {
            if self.quit_requested() {
                break;
            }
            if line.flag != 0 && !hints {
                continue;
            }
            self.fx_play_ex(sc::CH_DIALOGUE, &line.audio, false, false);
            if !line.text.is_empty() {
                self.feedback(&line.text);
            }
            while self.audio.is_playing(sc::CH_DIALOGUE) && !self.quit_requested() {
                self.tick();
                if self.pump_skip() {
                    self.audio.release(sc::CH_DIALOGUE);
                    break;
                }
            }
        }
        for ch in 1..8 {
            let g = self.audio.info(ch).target_gain;
            self.audio.set_gain(ch, g + 0.25);
        }
        self.feedback("");
    }

    /// `choice_Go` (0x415be0): shows the options of a choice set above the bottom bar.
    pub fn choice_go(&self, name: &str) {
        let options = {
            let mut w = self.world.borrow_mut();
            match w.bind(sc::CHOICE, name) {
                Ok(id) => w.choices[id].options.clone(),
                Err(e) => {
                    log::error!("scChoiceGo: {e}");
                    return;
                }
            }
        };
        if options.is_empty() {
            return;
        }
        self.feedback("");
        {
            let mut t = self.textlist.borrow_mut();
            t.clear();
            // Added from the (empty) slot after the last option down to the first, so the
            // first option ends up on top.
            t.add("");
            for o in options.iter().rev() {
                t.add(&o.text);
            }
        }
        self.world.borrow_mut().set_state(sc::ST_CHOICE);
        self.set_cursor(slot::CURSOR_FORWARD);
    }

    // ------------------------------------------------------------------ camera

    /// `core_LookAt(face, mode)`.
    pub fn look_at(&self, face: i32, mode: i32) {
        let Some(target) = Camera::face_target(face) else { return };
        if mode == sc::INSTANT {
            self.cam.borrow_mut().set(target.0, target.1);
            return;
        }
        if mode != sc::SMOOTH {
            return;
        }
        self.wait_cursor(true);
        let target = {
            let mut c = self.cam.borrow_mut();
            c.yaw = c.yaw.rem_euclid(std::f32::consts::TAU);
            if face == sc::BACK && c.yaw >= 3.14 {
                (6.28, target.1)
            } else {
                target
            }
        };
        let lock = std::mem::replace(&mut self.flags.borrow_mut().lock, true);
        let mut done = (false, false);
        while !self.quit_requested() {
            let speed = self.speed.get();
            if self.cam.borrow_mut().approach(target, &mut done, speed) {
                break;
            }
            self.tick();
            self.pump_skip();
        }
        self.flags.borrow_mut().lock = lock;
        self.wait_cursor(false);
    }

    // ------------------------------------------------------------------ cutscenes

    /// `core_Cutscene` (0x412720): plays a Theora clip full screen; Escape skips.
    pub fn cutscene(&self, file: &str) {
        if self.options.skip_videos {
            return;
        }
        let Some(data) = self.read_res(file) else {
            log::warn!("Load: file {file} not loaded");
            return;
        };
        match Video::open(data) {
            Ok(v) => *self.video.borrow_mut() = Some(v),
            Err(e) => {
                log::warn!("{file}: {e}");
                return;
            }
        }
        self.world.borrow_mut().set_state(sc::ST_VIDEO);
        self.gfx.borrow_mut().clear_slot(slot::VIDEO);
        let mut skipped = false;
        while self.world.borrow().state == sc::ST_VIDEO && !self.quit_requested() {
            self.tick();
            if self.pump_skip() {
                skipped = true;
                break;
            }
        }
        if skipped {
            self.capture_fade(false);
            self.world.borrow_mut().set_state(sc::ST_LAST);
            self.audio.release(sc::CH_FX);
        }
        *self.video.borrow_mut() = None;
    }

    // ------------------------------------------------------------------ subtitles (.srt)

    pub fn subtitle_load(&self, file: &str) {
        let path = self.game_dir.join(file);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut s = self.subtitles.borrow_mut();
        s.lines = parse_srt(&text);
        s.start = Some(Instant::now());
        s.shown = None;
    }

    pub fn subtitle_unload(&self) {
        let mut s = self.subtitles.borrow_mut();
        s.lines.clear();
        s.start = None;
    }

    /// `subtitle_Update` (0x418520): shows each .srt cue at its start time.
    fn subtitle_update(&self) {
        let cue = {
            let mut s = self.subtitles.borrow_mut();
            let Some(start) = s.start else { return };
            let t = start.elapsed().as_secs_f32();
            let next = s.shown.map_or(0, |i| i + 1);
            match s.lines.get(next).cloned() {
                Some((from, _, text)) if t >= from => {
                    s.shown = Some(next);
                    Some(text)
                }
                _ => None,
            }
        };
        if let Some(text) = cue {
            self.feedback(&text);
        }
    }

    // ------------------------------------------------------------------ input

    /// `input_Mouse` (0x417600): button 0 = move, 1 = left, 2 = right; action 1 = down, 2 = up.
    fn input_mouse(&self, button: i32, action: i32) {
        if self.flags.borrow().lock {
            return;
        }
        let m = self.mouse.get();
        let state = self.world.borrow().state;
        if button == 1 && action == 2 {
            match state {
                sc::ST_INV => {
                    self.inv_click(true);
                }
                sc::ST_MENU => self.menu_click(true),
                sc::ST_CHOICE => self.choice_click(true),
                sc::ST_BOOK => self.journal_click(true),
                _ => {
                    let quick = self.cfg.borrow().quickmode;
                    if quick && state == sc::ST_NODE {
                        let edge = self.cam.borrow().edge(&self.pointer());
                        if edge != Edge::None {
                            self.capture_fade(true);
                            self.cam.borrow_mut().step(edge);
                            return;
                        }
                    }
                    self.node_click(m.x, m.y, true);
                }
            }
        } else if button == 2 && action == 2 {
            match state {
                sc::ST_MENU => self.menu_step_back(),
                sc::ST_CHOICE => {}
                sc::ST_BOOK => self.journal_close(),
                sc::ST_INV | sc::ST_OVERLAY => {
                    self.capture_fade(false);
                    self.world.borrow_mut().set_state(sc::ST_LAST);
                    self.hints(sc::HINTS_CLOSE);
                }
                _ => {
                    let has_items = self.world.borrow().slot(1) != 0;
                    if has_items {
                        self.hints(sc::HINTS_OPEN);
                        self.capture_fade(false);
                        self.world.borrow_mut().set_state(sc::ST_INV);
                    }
                }
            }
        } else {
            match state {
                sc::ST_INV => {
                    self.inv_click(false);
                }
                sc::ST_MENU => self.menu_click(false),
                sc::ST_CHOICE => self.choice_click(false),
                sc::ST_BOOK => self.journal_click(false),
                _ => {}
            }
        }
    }

    /// `input_Key` (0x417930).
    fn input_key(&self, key: KeyCode) {
        if self.cfg.borrow().port.keys == crate::options::Keys::Modern && self.modern_key(key) {
            return;
        }
        let state = self.world.borrow().state;
        match key {
            KeyCode::Escape => {
                self.release_item();
                match state {
                    sc::ST_MENU | sc::ST_CHOICE => {}
                    sc::ST_INV => {
                        self.capture_fade(false);
                        self.world.borrow_mut().set_state(sc::ST_LAST);
                    }
                    sc::ST_BOOK => self.journal_close(),
                    sc::ST_OVERLAY => {
                        self.capture_fade(false);
                        let close = {
                            let w = self.world.borrow();
                            w.overlays[w.bound(sc::OVERLAY)].close_sound.clone()
                        };
                        self.world.borrow_mut().set_state(sc::ST_LAST);
                        self.fx_play(&close);
                    }
                    _ => {
                        if self.menu.borrow().enabled {
                            self.menu_show();
                        } else {
                            self.terminate();
                        }
                    }
                }
            }
            // The original's bindings, as odd as they are: Left/A turn left, Up/D turn right,
            // Right/W look up, Down/S look down, Space levels the view.
            KeyCode::ArrowLeft | KeyCode::KeyA | KeyCode::Numpad1 => self.cam_key(Some(-1.57), None),
            KeyCode::ArrowUp | KeyCode::KeyD | KeyCode::Numpad4 => self.cam_key(Some(1.57), None),
            KeyCode::ArrowRight | KeyCode::KeyW => self.cam_key(None, Some(1.25)),
            KeyCode::ArrowDown | KeyCode::KeyS => self.cam_key(None, Some(-1.25)),
            KeyCode::Space | KeyCode::Numpad5 => self.cam_key(None, Some(0.0)),
            KeyCode::KeyL => {
                let mut c = self.cfg.borrow_mut();
                c.log = !c.log;
            }
            KeyCode::F1 => {
                let on = {
                    let mut c = self.cfg.borrow_mut();
                    c.hints = !c.hints;
                    c.hints
                };
                self.feedback(if on { "hints ON" } else { "hints OFF" });
            }
            KeyCode::F2 => {
                let on = {
                    let mut c = self.cfg.borrow_mut();
                    c.surround = !c.surround;
                    c.surround
                };
                self.feedback(if on { "surround music ON" } else { "surround music OFF" });
            }
            KeyCode::F5 if state != sc::ST_MENU => {
                self.game_save(11, "quicksave");
                self.feedback("Game saved");
            }
            KeyCode::F7 => {
                let slot = self.quick_slot();
                if self.save_path(slot).is_file() {
                    self.release_item();
                    for ch in 1..18 {
                        self.audio.fade_out(ch, 1);
                    }
                    self.switch(sc::ZOOM, "black");
                    self.sleep(1.0);
                    self.switch(sc::ZOOMOUT, "");
                    for ch in 1..18 {
                        self.audio.release(ch);
                    }
                    self.game_load(slot);
                    self.menu.borrow_mut().started = true;
                    self.feedback("Game loaded");
                }
            }
            KeyCode::F9 => {
                let v = {
                    let mut c = self.cfg.borrow_mut();
                    c.cam_speed = match c.cam_speed {
                        75 => 25,
                        50 => 75,
                        _ => 50,
                    };
                    c.cam_speed
                };
                self.feedback(match v {
                    25 => "camera speed SLOW",
                    75 => "camera speed FAST",
                    _ => "camera speed NORMAL",
                });
            }
            KeyCode::F10 => {
                let v = {
                    let mut c = self.cfg.borrow_mut();
                    c.txt_speed = match c.txt_speed {
                        75 => 25,
                        50 => 75,
                        _ => 50,
                    };
                    c.txt_speed
                };
                self.feedback(match v {
                    25 => "text speed SLOW",
                    75 => "text speed FAST",
                    _ => "text speed NORMAL",
                });
            }
            KeyCode::F11 => {
                let on = {
                    let mut c = self.cfg.borrow_mut();
                    c.fullscreen = !c.fullscreen;
                    c.fullscreen
                };
                self.platform.borrow().set_fullscreen(on);
            }
            KeyCode::F12 => {
                let g = {
                    let mut c = self.cfg.borrow_mut();
                    c.gamma = (c.gamma + 1) % 4;
                    c.gamma
                };
                self.feedback(match g {
                    1 => "gamma correction LOW",
                    2 => "gamma correction NORMAL",
                    3 => "gamma correction HIGH",
                    _ => "gamma correction DISABLED",
                });
            }
            _ => {}
        }
    }

    /// Arrow keys: a quick cross-fade and a 45° turn (or look up/down/level).
    /// Key camera step: a quick cross-fade, then a 90° turn or a fixed view height. Like the
    /// original it works in any state and clears `scLock` (it brackets the capture with lock).
    /// Tracks TAB/H for the hotspot and history overlays; true when the key was consumed.
    fn held_key(&self, k: KeyCode, down: bool) -> bool {
        let p = self.cfg.borrow().port.clone();
        let mut h = self.held_keys.get();
        let used = match k {
            KeyCode::Tab if p.hotspot_key => {
                h.hotspots = down;
                true
            }
            KeyCode::KeyH if p.history => {
                h.history = down;
                true
            }
            _ => false,
        };
        self.held_keys.set(h);
        used
    }

    /// Port option "modern keys": arrows/WASD turn left/right and step the view up/down.
    fn modern_key(&self, k: KeyCode) -> bool {
        let look = self.cam.borrow().look_y;
        let up = if look < -0.3 { 0.0 } else { 1.25 };
        let down = if look > 0.3 { 0.0 } else { -1.25 };
        match k {
            KeyCode::ArrowLeft | KeyCode::KeyA | KeyCode::Numpad4 => self.cam_key(Some(-1.57), None),
            KeyCode::ArrowRight | KeyCode::KeyD | KeyCode::Numpad6 => self.cam_key(Some(1.57), None),
            KeyCode::ArrowUp | KeyCode::KeyW | KeyCode::Numpad8 => self.cam_key(None, Some(up)),
            KeyCode::ArrowDown | KeyCode::KeyS | KeyCode::Numpad2 => self.cam_key(None, Some(down)),
            KeyCode::Space | KeyCode::Numpad5 => self.cam_key(None, Some(0.0)),
            _ => return false,
        }
        true
    }

    fn cam_key(&self, turn: Option<f32>, look: Option<f32>) {
        self.capture_fade(true);
        let mut c = self.cam.borrow_mut();
        let (yaw, look_y) = (c.yaw + turn.unwrap_or(0.0), look.unwrap_or(c.look_y));
        c.set(yaw, look_y);
    }

    pub fn pointer(&self) -> Pointer {
        let m = self.mouse.get();
        let l = self.gfx.borrow().layout;
        let (_, y) = l.to_virtual(m.x, m.y);
        Pointer { x: m.x / l.scale, y, width: l.width / l.scale }
    }

    fn release_item(&self) {
        let held = self.world.borrow().held;
        if held != 0 {
            self.world.borrow_mut().held = 0;
            self.set_cursor(slot::CURSOR_NORMAL);
            self.set_cursor_mode(CursorMode::Released);
        }
    }

    /// Picks the hotspot under the pointer in the current node or zoom (`hotspot_Pick` /
    /// `ui_HitTest(STZOOM)`).
    /// Window ↔ canvas mapping of the current close-up (port option "close-ups").
    fn zoom_affine(&self) -> gfx::Affine {
        let band = self.band_rect(gfx::slot::ZOOM);
        let mode = self.cfg.borrow().port.zoom_scale;
        let l = self.gfx.borrow().layout;
        l.fit(band, mode).unwrap_or(l.canvas())
    }

    pub fn pick(&self, x: f32, y: f32) -> Id {
        let w = self.world.borrow();
        let l = self.gfx.borrow().layout;
        match w.state {
            sc::ST_ZOOM => {
                // Zoom hotspots were authored for an 800-wide canvas; the original scales them by
                // 1.28 into its 1024×768 screen.
                let (vx, vy) = self.zoom_affine().unmap(x, y);
                let z = w.cur_zoom();
                w.zooms.get(z).and_then(|z| {
                    z.objects.iter().copied().find(|&o| {
                        let ob = &w.objects[o];
                        let r = ob.rect;
                        let (x0, y0) = (r.x as f32 * 1.28, r.y as f32 * 1.28);
                        ob.enabled
                            && vx >= x0
                            && vx <= x0 + r.w as f32 * 1.28
                            && vy >= y0
                            && vy <= y0 + r.h as f32 * 1.28
                    })
                })
                .unwrap_or(0)
            }
            sc::ST_NODE => {
                let (sx, sy, sw, sh) = l.scene_rect();
                if x < sx || x > sx + sw || y < sy || y > sy + sh {
                    return 0;
                }
                let fov = l.scene_fov(self.cfg.borrow().fov);
                let proj = Camera::projection(fov, sw / sh);
                let view = self.cam.borrow().view();
                let inv = (proj * view).inverse();
                let ndc_x = (x - sx) / sw * 2.0 - 1.0;
                let ndc_y = 1.0 - (y - sy) / sh * 2.0;
                let far = inv.project_point3(glam::Vec3::new(ndc_x, ndc_y, 1.0));
                let dir = far.normalize();
                let node = &w.nodes[w.cur_node()];
                node.objects
                    .iter()
                    .copied()
                    .find(|&o| {
                        let ob = &w.objects[o];
                        ob.enabled && hotspot_hit(ob.face, ob.rect, dir)
                    })
                    .unwrap_or(0)
            }
            _ => 0,
        }
    }

    /// `node_Click` (0x414610): hover feedback (cursor) and, with `click`, the object action.
    fn node_click(&self, x: f32, y: f32, click: bool) {
        let (lock, waiting) = {
            let f = self.flags.borrow();
            (f.lock, f.waiting)
        };
        if lock || waiting {
            return;
        }
        let state = self.world.borrow().state;
        let holding = self.world.borrow().held != 0;
        let mut hit = 0;
        match state {
            sc::ST_ZOOM => {
                hit = self.pick(x, y);
                // With an item in hand only the cursor mode changes; the image stays the item icon.
                if !holding {
                    self.set_cursor(slot::CURSOR_NORMAL);
                }
                self.set_cursor_mode(if holding { CursorMode::Holding } else { CursorMode::Normal });
            }
            sc::ST_NODE => {
                let quick = self.cfg.borrow().quickmode;
                if quick && !click && !holding {
                    let edge = self.cam.borrow().edge(&self.pointer());
                    let s = match edge {
                        Edge::Left => Some(slot::CURSOR_LEFT),
                        Edge::Right => Some(slot::CURSOR_RIGHT),
                        Edge::Up => Some(slot::CURSOR_UP),
                        Edge::Down => Some(slot::CURSOR_DOWN),
                        Edge::None => None,
                    };
                    if let Some(s) = s {
                        self.set_cursor(s);
                        return;
                    }
                }
                hit = self.pick(x, y);
                // With an item in hand only the cursor mode changes; the image stays the item icon.
                if !holding {
                    self.set_cursor(slot::CURSOR_NORMAL);
                }
                self.set_cursor_mode(if holding { CursorMode::Holding } else { CursorMode::Normal });
            }
            sc::ST_OVERLAY => {
                if !click {
                    return;
                }
                self.capture_fade(false);
                let close = {
                    let w = self.world.borrow();
                    w.overlays[w.bound(sc::OVERLAY)].close_sound.clone()
                };
                self.world.borrow_mut().set_state(sc::ST_LAST);
                self.fx_play(&close);
                return;
            }
            _ => {}
        }
        // The original ignores clicks outside the 3D band (64..704); with the port's scaling
        // options the band is wherever the picture or the scene is drawn.
        let in_band = if state == sc::ST_ZOOM {
            let (_, vy) = self.zoom_affine().unmap(x, y);
            vy > 64.0 && vy <= 703.0
        } else {
            let (_, sy, _, sh) = self.gfx.borrow().layout.scene_rect();
            y > sy && y <= sy + sh
        };
        if !in_band {
            return;
        }
        if hit == 0 {
            if click && holding {
                self.release_item();
            }
            return;
        }
        let obj = {
            let mut w = self.world.borrow_mut();
            let _ = w.set_bind(sc::OBJECT, hit);
            w.objects[hit].clone()
        };
        // Hover cursor.
        if holding {
            if matches!(obj.action, sc::FEEDBACK | sc::CUSTOM | sc::FEEDBACK_EX) {
                self.set_cursor_mode(CursorMode::Target);
            }
        } else {
            let s = match obj.hover {
                sc::NORMAL => Some(slot::CURSOR_NORMAL),
                sc::FORWARD => Some(slot::CURSOR_FORWARD),
                sc::LOOK => Some(slot::CURSOR_LOOK),
                sc::USE => Some(slot::CURSOR_USE),
                sc::ZOOMOUT | sc::ZOOMBACK => Some(slot::CURSOR_BACK),
                _ => None,
            };
            if let Some(s) = s {
                self.set_cursor(s);
            }
        }
        if !click {
            return;
        }
        if !holding {
            log::debug!("Clicked on {}", obj.name);
            match obj.action {
                sc::FEEDBACK => self.feedback(&obj.target),
                sc::FEEDBACK_EX => {
                    self.feedback(&obj.target);
                    self.fx_play_ex(sc::CH_DIALOGUE, &obj.audio, false, false);
                }
                sc::CUSTOM => {
                    self.call(&obj.target);
                    self.rebind_current();
                }
                sc::ZOOMOUT | sc::ZOOMBACK | sc::ROOM | sc::NODE | sc::ZOOM => self.switch(obj.action, &obj.target),
                _ => {}
            }
            return;
        }
        // Using the item in hand on the object.
        if !matches!(obj.action, sc::FEEDBACK | sc::CUSTOM | sc::FEEDBACK_EX) {
            self.release_item();
            return;
        }
        let held = self.world.borrow().held;
        self.release_item();
        let reaction = obj.reactions.iter().find(|r| r.item == held).cloned();
        match reaction {
            Some(r) => self.react(&r),
            None => self.refuse(held),
        }
    }

    fn react(&self, r: &crate::world::Reaction) {
        match r.kind {
            sc::FEEDBACK => self.feedback(&r.text),
            sc::FEEDBACK_EX => {
                self.feedback(&r.text);
                self.fx_play_ex(sc::CH_DIALOGUE, &r.audio, false, false);
            }
            sc::CUSTOM => {
                self.call(&r.text);
                self.rebind_current();
            }
            _ => {}
        }
    }

    /// Default line when an item does not fit (`item.refusal` + its voice).
    fn refuse(&self, item: Id) {
        let (text, audio) = {
            let w = self.world.borrow();
            let it = &w.items[item];
            (it.refusal.clone(), it.refusal_audio.clone())
        };
        self.feedback(&text);
        self.fx_play_ex(sc::CH_DIALOGUE, &audio, false, false);
    }

    /// Inventory grid cell under the pointer: 10 columns × up to 11 rows, 1-based slot.
    pub fn inv_slot_at(&self, x: f32, y: f32) -> usize {
        let (vx, vy) = self.gfx.borrow().layout.to_virtual(x, y);
        let size = 64.0;
        for row in 0..11 {
            let top = 64.0 + 96.0 * row as f32;
            if vy < top || vy > top + size {
                continue;
            }
            for col in 0..10 {
                let left = 32.0 * (col + 1) as f32 + size * col as f32;
                if vx >= left && vx <= left + size {
                    return 1 + col + row * 10;
                }
            }
        }
        0
    }

    /// `inv_Click` (0x4143c0).
    fn inv_click(&self, click: bool) {
        let m = self.mouse.get();
        let slot_i = self.inv_slot_at(m.x, m.y);
        let (held, item) = {
            let w = self.world.borrow();
            (w.held, w.slot(slot_i))
        };
        self.set_cursor_mode(if held != 0 { CursorMode::Holding } else { CursorMode::Normal });
        if slot_i == 0 || item == 0 {
            if click && held != 0 {
                self.release_item();
            }
            return;
        }
        if held == 0 {
            self.set_cursor(slot::CURSOR_USE);
        } else if held != item {
            self.set_cursor_mode(CursorMode::Target);
        }
        let _ = self.world.borrow_mut().set_bind(sc::ITEM, item);
        if !click {
            return;
        }
        if held != 0 {
            if held != item {
                let reaction = {
                    let w = self.world.borrow();
                    w.items[item].reactions.iter().find(|r| r.item == held).cloned()
                };
                self.release_item();
                match reaction {
                    Some(r) => self.react(&r),
                    None => self.refuse(held),
                }
            } else {
                self.release_item();
            }
            return;
        }
        let name = self.world.borrow().items[item].name.clone();
        if name == "journal" {
            self.journal_show();
            return;
        }
        self.world.borrow_mut().held = item;
        self.set_cursor(slot::ICON + item as u32);
        self.set_cursor_mode(CursorMode::Holding);
        self.hints(sc::HINTS_GRAB);
    }

    // ------------------------------------------------------------------ journal

    /// `journal_Show` (0x4189e0).
    pub fn journal_show(&self) {
        self.feedback("");
        let (font, image, open) = {
            let j = self.journal.borrow();
            (j.font.clone(), j.image.clone(), j.sound_open.clone())
        };
        self.load_tex(slot::FONT, &font);
        self.load_tex(slot::LOGO, &image);
        self.capture_fade(false);
        self.world.borrow_mut().set_state(sc::ST_LAST);
        self.world.borrow_mut().set_state(sc::ST_BOOK);
        self.fx_play(&open);
    }

    /// `journal_Close` (0x418a50).
    fn journal_close(&self) {
        self.capture_fade(false);
        self.world.borrow_mut().set_state(sc::ST_LAST);
        let font = self.font.borrow().clone();
        self.load_tex(slot::FONT, &font);
        let close = self.journal.borrow().sound_close.clone();
        self.fx_play(&close);
    }

    /// `journal_Click` (0x414310): left/right half turns the page, bottom bar closes.
    fn journal_click(&self, click: bool) {
        let m = self.mouse.get();
        let (vx, vy) = self.gfx.borrow().layout.to_virtual(m.x, m.y);
        self.set_cursor(if vy < 605.0 { slot::CURSOR_USE } else { slot::CURSOR_BACK });
        if !click {
            return;
        }
        if vy < 605.0 {
            let delta = if vx < 512.0 { -(JOURNAL_SPREAD as isize) } else { JOURNAL_SPREAD as isize };
            if self.journal.borrow_mut().scroll(delta) {
                let turn = self.journal.borrow().sound_turn.clone();
                self.fx_play(&turn);
            }
        } else {
            self.journal_close();
        }
    }

    // ------------------------------------------------------------------ text list clicks

    /// `ui_HitTest(STCHOICE)`: row under the pointer, 1-based from the top.
    fn choice_row(&self) -> usize {
        let m = self.mouse.get();
        let (_, vy) = self.gfx.borrow().layout.to_virtual(m.x, m.y);
        let h = gfx::VIRTUAL_H;
        let prev = self.world.borrow().prev_state;
        if prev == sc::ST_MENU {
            for k in 1..=2 {
                let top = h - 32.0 + 16.0 * (k - 1) as f32;
                if vy >= top && vy <= top + 16.0 {
                    return k;
                }
            }
            return 0;
        }
        let c = {
            let w = self.world.borrow();
            w.choices.get(w.bound(sc::CHOICE)).map_or(0, |c| c.options.len())
        };
        for k in 1..=c + 1 {
            let top = h - 16.0 * c as f32 - 32.0 + 16.0 * (k - 1) as f32;
            if vy >= top && vy <= top + 16.0 {
                return if c == 3 { k - 1 } else { k };
            }
        }
        0
    }

    /// `choice_Click` (0x414210).
    fn choice_click(&self, click: bool) {
        let k = self.choice_row();
        let prev = self.world.borrow().prev_state;
        let count = {
            let w = self.world.borrow();
            w.choices.get(w.bound(sc::CHOICE)).map_or(0, |c| c.options.len())
        };
        {
            let mut t = self.textlist.borrow_mut();
            t.hover = None;
            if k != 0 {
                t.hover = Some(if prev == sc::ST_MENU {
                    if k == 1 {
                        1
                    } else {
                        0
                    }
                } else {
                    count - k + 1
                });
            }
        }
        if k == 0 || !click {
            return;
        }
        self.textlist.borrow_mut().hover = None;
        if prev == sc::ST_MENU {
            {
                let mut m = self.menu.borrow_mut();
                m.answer_first = k == 1;
                m.answer_second = k == 2;
            }
            self.menu_click(true);
            return;
        }
        if k > count {
            return;
        }
        let func = {
            let mut w = self.world.borrow_mut();
            w.set_state(sc::ST_LAST);
            let c = w.bound(sc::CHOICE);
            w.choices[c].selected = (k - 1) as i32;
            w.choices[c].options[k - 1].func.clone()
        };
        self.textlist.borrow_mut().clear();
        self.set_cursor(slot::CURSOR_NORMAL);
        self.call(&func);
        self.rebind_current();
    }

    // ------------------------------------------------------------------ menu

    pub fn menu_set(&self, textures: [&str; 7], new_game_a: &str, new_game_b: &str) {
        {
            let mut m = self.menu.borrow_mut();
            for (i, t) in textures.iter().enumerate() {
                m.textures[i] = (*t).into();
            }
            m.new_game = [new_game_a.into(), new_game_b.into()];
            m.save_func = false;
            m.started = false;
            m.mode = 0;
            m.saved_state = sc::ST_NODE;
            m.enabled = true;
        }
        for (i, t) in textures.iter().enumerate() {
            self.load_tex(slot::MENU + i as u32, t);
        }
        self.cfg.borrow_mut().showmenu = true;
    }

    /// `scMenuSetEx(tag, value)`.
    pub fn menu_set_ex(&self, tag: i32, value: &str) {
        let mut m = self.menu.borrow_mut();
        match tag {
            sc::STEREO => m.music = Some(value.into()),
            sc::INTRO => m.intro = Some(value.into()),
            sc::OUTRO => m.outro = Some(value.into()),
            sc::IMAGE => m.background = Some(value.into()),
            _ => {}
        }
    }

    /// `menu_Show` (0x412fa0).
    pub fn menu_show(&self) {
        let (background, intro, started) = {
            let m = self.menu.borrow();
            (m.background.clone(), m.intro.clone(), m.started)
        };
        if let Some(bg) = &background {
            self.load_tex(slot::LOGO, bg);
        }
        let aborted = self.menu.borrow().aborted;
        if let (Some(intro), false, false) = (&intro, started, aborted) {
            self.call(intro);
        }
        self.refresh_save_slots();
        if started {
            for ch in 1..18 {
                self.audio.fade_out(ch, 1);
            }
        }
        self.capture_fade(false);
        self.switch(sc::ZOOM, "black");
        self.sleep(1.0);
        self.switch(sc::ZOOMOUT, "");
        let st = self.world.borrow().state;
        {
            let mut m = self.menu.borrow_mut();
            m.saved_state = st;
            m.saved_cloud = self.ambience.get()[1];
            m.mode = 0;
            m.disabled[5] = !m.started;
        }
        self.set_cursor(slot::CURSOR_MENU);
        self.world.borrow_mut().set_state(sc::ST_MENU);
        let music = self.menu.borrow().music.clone();
        if let Some(music) = music {
            self.audio.release(7);
            self.audio.load(7, &music, self.read_res(&music));
            self.audio.set_gain(7, 1.0);
            self.audio.play(7, true);
        }
        let mut a = self.ambience.get();
        a[1] = sc::STORM_PRESET;
        self.ambience.set(a);
    }

    fn refresh_save_slots(&self) {
        let mut labels: [String; 11] = Default::default();
        let mut exists = [false; 12];
        let mut any = false;
        for s in 1..12 {
            if let Some(label) = crate::save::read_label(&self.save_path(s)) {
                exists[s] = true;
                any = true;
                let row = if s == 11 { 0 } else { s };
                labels[row] = format!("{row}. {label}");
            }
        }
        // The autosave shares row 0 with the quicksave; the newer one is offered.
        if self.quick_slot() == AUTOSAVE_SLOT {
            if let Some(label) = crate::save::read_label(&self.save_path(AUTOSAVE_SLOT)) {
                exists[11] = true;
                any = true;
                labels[0] = format!("0. {label} (auto)");
            }
        }
        let mut m = self.menu.borrow_mut();
        m.slot_labels = labels;
        m.slot_exists = exists;
        m.disabled[4] = !any;
    }

    /// `menu_Leave` (0x413410): back to the game.
    fn menu_leave(&self) {
        self.audio.fade_out(7, 1);
        self.capture_fade(false);
        self.switch(sc::ZOOM, "black");
        self.sleep(2.0);
        self.switch(sc::ZOOMOUT, "");
        self.audio.release(7);
        let (started, st, cloud) = {
            let m = self.menu.borrow();
            (m.started, m.saved_state, m.saved_cloud)
        };
        if started {
            self.capture_fade(false);
            self.set_cursor(slot::CURSOR_NORMAL);
            self.world.borrow_mut().set_state(st);
            for ch in (1..18).filter(|&c| c != 7) {
                self.audio.fade_in(ch, 1);
            }
        }
        let mut a = self.ambience.get();
        a[1] = cloud;
        self.ambience.set(a);
    }

    /// `menu_Confirm` (0x4132f0): opens a question (returns false) or, when one is open, closes it
    /// and returns whether the first answer was chosen.
    fn menu_confirm(&self, item: usize) -> bool {
        let open = self.menu.borrow().confirm.is_some();
        if !open {
            self.feedback("");
            let started = self.menu.borrow().started;
            let (a, b, q) = match item {
                5 => ("no", "yes", "Quit the game?"),
                1 if !started => ("Scratches: The Last Visit", "Scratches: Director's Cut", "Which game do you want to play?"),
                1 => ("no", "yes", "Start a new game?"),
                _ if self.menu.borrow().mode == 1 => ("no", "yes", "Load a saved game?"),
                _ => ("no", "yes", "Replace saved game?"),
            };
            {
                let mut t = self.textlist.borrow_mut();
                t.clear();
                t.add(a);
                t.add(b);
                t.add(q);
            }
            self.world.borrow_mut().set_state(sc::ST_CHOICE);
            let mut m = self.menu.borrow_mut();
            m.confirm = Some(item);
            m.answer_first = false;
            m.answer_second = false;
            false
        } else {
            self.menu.borrow_mut().confirm = None;
            self.world.borrow_mut().set_state(sc::ST_LAST);
            self.textlist.borrow_mut().clear();
            self.menu.borrow().answer_first
        }
    }

    /// Menu item under the pointer (`ui_HitTest(STMENU)`): 0..5 buttons, 6..16 save rows.
    fn menu_item(&self) -> Option<usize> {
        let m = self.mouse.get();
        let (vx, vy) = self.gfx.borrow().layout.to_virtual(m.x, m.y);
        let (w, h) = (gfx::VIRTUAL_W, gfx::VIRTUAL_H);
        let cx = w / 2.0;
        let menu = self.menu.borrow();
        if menu.mode == 0 {
            for i in 1..=6 {
                let bottom = h - 0x48 as f32 - 36.0 * (i - 1) as f32;
                let top = h - 0x6c as f32 - 36.0 * (i - 1) as f32;
                if vx >= cx - 64.0 && vx <= cx + 64.0 && vy >= top && vy <= bottom {
                    return Some(6 - i);
                }
            }
            return None;
        }
        if menu.mode == 3 || menu.mode > 6 {
            return None;
        }
        // Rows counted from the bottom: row j is menu item 16 - j, i.e. save slot 10 - j, and the
        // top row (j = 10) is the quicksave slot 11.
        for j in 0..11 {
            let top = h - 0x7e as f32 - 16.0 * j as f32;
            let bottom = h - 0x6e as f32 - 16.0 * j as f32;
            if vx >= cx - 256.0 && vx <= cx + 256.0 && vy >= top && vy <= bottom {
                let usable = menu.mode != 1 || if j == 10 { menu.slot_exists[11] } else { menu.slot_exists[10 - j] };
                if usable {
                    return Some(16 - j);
                }
            }
        }
        if vx >= cx - 64.0 && vx <= cx + 64.0 && vy >= h - 0x69 as f32 && vy <= h - 0x46 as f32 {
            return Some(0);
        }
        None
    }

    /// `menu_Click` (0x413970).
    fn menu_click(&self, click: bool) {
        let confirming = self.menu.borrow().confirm;
        let item = match confirming {
            Some(i) => Some(i),
            None => self.menu_item(),
        };
        self.menu.borrow_mut().hover = item;
        if !click {
            return;
        }
        let Some(item) = item else { return };
        let (mode, started) = {
            let m = self.menu.borrow();
            (m.mode, m.started)
        };
        match item {
            0 => {
                self.capture_fade(false);
                if mode == 0 {
                    if started {
                        self.menu_leave();
                    }
                } else {
                    let back = if mode == 5 || mode == 6 { 4 } else { 0 };
                    self.menu.borrow_mut().mode = back;
                }
            }
            1 => {
                let first = self.menu_confirm(1);
                let (answer_second, new_game) = {
                    let m = self.menu.borrow();
                    (m.answer_second, m.new_game.clone())
                };
                if self.menu.borrow().confirm.is_some() {
                    return; // question just opened
                }
                if started {
                    if first {
                        // "Start a new game?" → yes: forget the running game and ask which one.
                        {
                            let mut m = self.menu.borrow_mut();
                            m.started = false;
                            m.disabled[5] = true;
                        }
                        self.menu_confirm(1);
                    }
                    return;
                }
                let func = if first {
                    Some(&new_game[0])
                } else if answer_second {
                    Some(&new_game[1])
                } else {
                    None
                };
                if let Some(func) = func {
                    self.menu_leave();
                    {
                        let mut m = self.menu.borrow_mut();
                        m.started = true;
                    }
                    self.set_cursor(slot::CURSOR_NORMAL);
                    // The first new game snapshots the untouched world into slot 0; every later
                    // one restores it first, so nothing (inventory, flags) leaks between games.
                    if self.new_game_snapshot.replace(false) {
                        self.game_save(0, "new");
                    } else {
                        self.game_load(0);
                    }
                    self.world.borrow_mut().set_state(sc::ST_NODE);
                    let func = func.clone();
                    self.call(&func);
                    self.rebind_current();
                }
            }
            2 => {
                self.capture_fade(false);
                self.menu.borrow_mut().mode = 4;
            }
            3 => {
                if !self.menu.borrow().disabled[4] {
                    self.capture_fade(false);
                    self.menu.borrow_mut().mode = 1;
                }
            }
            4 => {
                if started {
                    self.capture_fade(false);
                    self.menu.borrow_mut().mode = 2;
                }
            }
            5 => {
                if self.menu_confirm(5) {
                    self.audio.fade_out(7, 1);
                    self.capture_fade(false);
                    self.switch(sc::ZOOM, "black");
                    self.sleep(2.0);
                    self.switch(sc::ZOOMOUT, "");
                    self.audio.release(7);
                    let (outro, aborted) = {
                        let m = self.menu.borrow();
                        (m.outro.clone(), m.aborted)
                    };
                    if let (Some(outro), false) = (outro, aborted) {
                        self.call(&outro);
                    }
                    self.terminate();
                }
            }
            6..=16 => {
                let row = item - 6;
                let slot_n = if row == 0 { self.quick_slot() } else { row };
                match mode {
                    1 => {
                        if started && !self.menu_confirm(item) {
                            return;
                        }
                        if !self.save_path(slot_n).is_file() {
                            return;
                        }
                        self.menu_leave();
                        for ch in 1..18 {
                            self.audio.release(ch);
                        }
                        self.game_load(slot_n);
                        {
                            let mut m = self.menu.borrow_mut();
                            m.started = true;
                            m.mode = 0;
                        }
                        self.feedback("Game loaded");
                    }
                    2 if row != 0 => {
                        if self.save_path(slot_n).is_file() && !self.menu_confirm(item) {
                            return;
                        }
                        self.menu_leave();
                        let label = if self.menu.borrow().save_func {
                            script::call_global_str(&self.lua, "save").unwrap_or_default()
                        } else {
                            String::new()
                        };
                        self.game_save(slot_n, &label);
                        self.menu.borrow_mut().mode = 0;
                        self.feedback("Game saved");
                    }
                    4 => self.settings_item(row),
                    5 | 6 => self.port_option(mode, row, 1),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Settings rows (mode 4) reuse the function keys.
    fn settings_item(&self, row: usize) {
        match row {
            0 => self.input_key(KeyCode::F1),
            1 => self.input_key(KeyCode::F2),
            2 => self.input_key(KeyCode::F9),
            3 => self.input_key(KeyCode::F10),
            4 => self.input_key(KeyCode::F12),
            5 => {
                let on = {
                    let mut c = self.cfg.borrow_mut();
                    c.quickmode = !c.quickmode;
                    c.quickmode
                };
                self.feedback(if on { "slideshow mode ON" } else { "slideshow mode OFF" });
            }
            6 => {
                let on = {
                    let mut c = self.cfg.borrow_mut();
                    c.alt_control = !c.alt_control;
                    c.alt_control
                };
                self.feedback(if on { "alternate control ENABLED" } else { "alternate control DISABLED" });
            }
            7 => self.call("language"),
            8 => {
                self.capture_fade(false);
                self.menu.borrow_mut().mode = 5;
            }
            _ => {}
        }
    }

    /// Right click on a port option steps it backwards.
    fn menu_step_back(&self) {
        let mode = self.menu.borrow().mode;
        if let (5 | 6, Some(item @ 6..=16)) = (mode, self.menu_item()) {
            self.port_option(mode, item - 6, -1);
        }
    }

    /// Port options pages (menu modes 5 and 6): a row steps its value, the last row turns the page.
    fn port_option(&self, mode: i32, row: usize, dir: i32) {
        let page = crate::options::PAGES[(mode - 5) as usize];
        if row >= page.len() {
            if row == page.len() {
                self.capture_fade(false);
                self.menu.borrow_mut().mode = if mode == 5 { 6 } else { 5 };
            }
            return;
        }
        let r = page[row];
        let resolutions = self.platform.borrow().resolutions();
        self.cfg.borrow_mut().port.step(r, dir, &resolutions);
        self.apply_option(r);
    }

    /// Side effects of a changed port option.
    pub fn apply_option(&self, r: crate::options::Row) {
        use crate::options::Row;
        let port = self.cfg.borrow().port.clone();
        match r {
            Row::NodeView => self.gfx.borrow_mut().layout.view = port.node_view,
            Row::Gamepad => self.platform.borrow_mut().pad.enabled = port.gamepad,
            Row::Filter => self.gfx.borrow_mut().fsr = port.filter == crate::options::Filter::Fsr,
            Row::Display | Row::Resolution => self.platform.borrow().apply_display(port.display, port.resolution),
            Row::VolMaster | Row::VolMusic | Row::VolFx | Row::VolVoice => self.apply_volumes(),
            _ => {}
        }
    }

    pub fn apply_volumes(&self) {
        let p = self.cfg.borrow().port.clone();
        let k = |v: i32| v as f32 / 10.0;
        let mut g = [k(p.vol_fx); crate::audio::CHANNELS];
        for ch in 1..8 {
            g[ch] = k(p.vol_music);
        }
        g[sc::CH_DIALOGUE] = k(p.vol_voice);
        self.audio.set_volumes(k(p.vol_master), g);
    }

    /// `scAbort`: stop everything and return to the menu.
    pub fn abort(&self) {
        {
            let mut m = self.menu.borrow_mut();
            m.started = false;
            m.aborted = true;
        }
        for ch in 1..18 {
            self.audio.release(ch);
        }
        self.menu_show();
    }

    // ------------------------------------------------------------------ saves

    /// Slot behind "quick load" (F7, menu row 0): the quicksave or the autosave, whichever is newer.
    pub fn quick_slot(&self) -> usize {
        let time = |s| std::fs::metadata(self.save_path(s)).and_then(|m| m.modified()).ok();
        match (time(11), time(AUTOSAVE_SLOT)) {
            (Some(q), Some(a)) if a > q => AUTOSAVE_SLOT,
            (None, Some(_)) => AUTOSAVE_SLOT,
            _ => 11,
        }
    }

    /// Port option: saves to the autosave slot on entering another room, from the top-level
    /// loop only (never inside a script wait) and at most every 20 seconds.
    fn autosave_check(&self) {
        if !self.cfg.borrow().port.autosave || !self.menu.borrow().started {
            return;
        }
        let (state, room) = {
            let w = self.world.borrow();
            (w.state, w.cur_room)
        };
        let busy = {
            let f = self.flags.borrow();
            f.lock || f.waiting
        };
        let (last_room, last_time) = self.autosave.get();
        if state != sc::ST_NODE || busy || room == 0 || room == last_room {
            return;
        }
        if last_room == 0 {
            // First room after starting or loading: nothing new to keep yet.
            self.autosave.set((room, last_time));
            return;
        }
        if last_time.elapsed().as_secs_f32() < 20.0 {
            return;
        }
        let label = if self.menu.borrow().save_func {
            script::call_global_str(&self.lua, "save").unwrap_or_default()
        } else {
            String::new()
        };
        self.game_save(AUTOSAVE_SLOT, &label);
        self.autosave.set((room, Instant::now()));
    }

    pub fn save_path(&self, slot: usize) -> PathBuf {
        self.user_dir.join(format!("scream.{slot:03}"))
    }

    pub fn game_save(&self, slot: usize, label: &str) {
        let path = self.save_path(slot);
        match crate::save::save(self, &path, label) {
            Ok(()) => log::info!("Game saved to {}", path.display()),
            Err(e) => log::error!("Could not save game: {e}"),
        }
    }

    pub fn game_load(&self, slot: usize) {
        let path = self.save_path(slot);
        if let Err(e) = crate::save::load(self, &path) {
            log::error!("Could not load game: {e}");
        }
        self.autosave.set((0, Instant::now()));
    }
}

/// Ray from the cube centre in direction `dir` against an object quad on its face. Geometry of
/// `hotspot_Emit` (0x40b920): faces at ±512 in the pixel space of a 1024² face texture.
pub fn hotspot_hit(face: i32, r: crate::world::Rect, dir: glam::Vec3) -> bool {
    let s = 512.0f32;
    // Face plane: axis index, sign, and the mapping (u, v) -> pixel coordinates.
    let (axis, sign) = match face {
        sc::FRONT => (2, 1.0),
        sc::BACK => (2, -1.0),
        sc::LEFT => (0, 1.0),
        sc::RIGHT => (0, -1.0),
        sc::UP => (1, 1.0),
        sc::DOWN => (1, -1.0),
        _ => return false,
    };
    let d = dir.to_array();
    if d[axis] * sign <= 1e-6 {
        return false;
    }
    let t = s / (d[axis] * sign);
    let p = dir * t;
    let (px, py) = match face {
        sc::FRONT => (s - p.x, s - p.y),
        sc::BACK => (p.x + s, s - p.y),
        sc::LEFT => (p.z + s, s - p.y),
        sc::RIGHT => (s - p.z, s - p.y),
        sc::UP => (s - p.x, p.z + s),
        sc::DOWN => (s - p.x, s - p.z),
        _ => return false,
    };
    px >= r.x as f32 && px <= (r.x + r.w) as f32 && py >= r.y as f32 && py <= (r.y + r.h) as f32
}

/// Minimal .srt parser: (start, end, text) per cue.
fn parse_srt(text: &str) -> Vec<(f32, f32, String)> {
    let parse_time = |t: &str| -> Option<f32> {
        let t = t.trim().replace(',', ".");
        let mut it = t.split(':');
        let h: f32 = it.next()?.parse().ok()?;
        let m: f32 = it.next()?.parse().ok()?;
        let s: f32 = it.next()?.parse().ok()?;
        Some(h * 3600.0 + m * 60.0 + s)
    };
    let mut out = Vec::new();
    for block in text.replace('\r', "").split("\n\n") {
        let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
        let Some(ti) = lines.iter().position(|l| l.contains("-->")) else { continue };
        let mut parts = lines[ti].split("-->");
        let (Some(a), Some(b)) = (parts.next().and_then(parse_time), parts.next().and_then(parse_time)) else { continue };
        out.push((a, b, lines[ti + 1..].join(" ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::Rect;

    #[test]
    fn front_hotspot_centre_hits() {
        let r = Rect { x: 412, y: 412, w: 200, h: 200 };
        assert!(hotspot_hit(sc::FRONT, r, glam::Vec3::Z));
        assert!(!hotspot_hit(sc::BACK, r, glam::Vec3::Z));
        // Looking slightly right on FRONT (towards -x) increases pixel x.
        let right = Rect { x: 600, y: 412, w: 200, h: 200 };
        assert!(hotspot_hit(sc::FRONT, right, glam::Vec3::new(-0.4, 0.0, 1.0)));
    }

    #[test]
    fn srt_cues() {
        let s = "1\n00:00:01,500 --> 00:00:03,000\nHello\nworld\n\n2\n00:01:00,000 --> 00:01:02,000\nBye\n";
        let c = parse_srt(s);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].0, 1.5);
        assert_eq!(c[0].2, "Hello world");
        assert_eq!(c[1].0, 60.0);
    }
}

impl Engine {
    /// Replays `--input` events and writes `--shot` screenshots at their frames.
    fn test_hooks(&self) {
        let frame = self.options.frames.get();
        for (_, ev) in self.options.inputs.iter().filter(|(f, _)| *f == frame) {
            let l = self.gfx.borrow().layout;
            let at = |x: f32, y: f32| {
                let (wx, wy) = l.to_window(x, y);
                let mut m = self.mouse.get();
                m.x = wx;
                m.y = wy;
                self.mouse.set(m);
            };
            log::info!("test input @{frame}: {ev:?}");
            match *ev {
                TestInput::Move(x, y) => {
                    at(x, y);
                    self.input_mouse(0, 0);
                }
                TestInput::Click(x, y) => {
                    at(x, y);
                    self.input_mouse(0, 0);
                    self.input_mouse(1, 1);
                    self.input_mouse(1, 2);
                }
                TestInput::RightClick(x, y) => {
                    at(x, y);
                    self.input_mouse(2, 1);
                    self.input_mouse(2, 2);
                }
                TestInput::Key(k) => {
                    if !self.held_key(k, true) {
                        self.input_key(k)
                    }
                }
                TestInput::KeyUp(k) => {
                    self.held_key(k, false);
                }
                TestInput::Look(yaw, look_y) => self.cam.borrow_mut().set(yaw, look_y),
            }
        }
        for (_, path) in self.options.shots.iter().filter(|(f, _)| *f == frame) {
            let img = self.gfx.borrow_mut().read_frame();
            if let Some(buf) = image::RgbaImage::from_raw(img.width, img.height, img.rgba) {
                let _ = buf.save(path);
                log::info!("screenshot {}", path.display());
            }
        }
    }
}

/// Parses `--input` specs: `frame:click:x,y`, `frame:rclick:x,y`, `frame:move:x,y`,
/// `frame:key:Escape`, `frame:look:yaw,look_y`, separated by `;`.
pub fn parse_inputs(spec: &str) -> Vec<(u32, TestInput)> {
    spec.split(';')
        .filter_map(|item| {
            let mut p = item.trim().splitn(3, ':');
            let frame: u32 = p.next()?.parse().ok()?;
            let kind = p.next()?;
            let arg = p.next().unwrap_or("");
            let xy = || {
                let mut v = arg.split(',').map(|s| s.trim().parse::<f32>().unwrap_or(0.0));
                (v.next().unwrap_or(0.0), v.next().unwrap_or(0.0))
            };
            let ev = match kind {
                "click" => TestInput::Click(xy().0, xy().1),
                "rclick" => TestInput::RightClick(xy().0, xy().1),
                "move" => TestInput::Move(xy().0, xy().1),
                "look" => TestInput::Look(xy().0, xy().1),
                "keyup" => TestInput::KeyUp(match arg {
                    "Tab" => KeyCode::Tab,
                    "H" => KeyCode::KeyH,
                    _ => return None,
                }),
                "key" => TestInput::Key(match arg {
                    "Tab" => KeyCode::Tab,
                    "H" => KeyCode::KeyH,
                    "Space" => KeyCode::Space,
                    "Escape" => KeyCode::Escape,
                    "F1" => KeyCode::F1,
                    "F5" => KeyCode::F5,
                    "F7" => KeyCode::F7,
                    "F12" => KeyCode::F12,
                    "Left" => KeyCode::ArrowLeft,
                    "Right" => KeyCode::ArrowRight,
                    "Up" => KeyCode::ArrowUp,
                    "Down" => KeyCode::ArrowDown,
                    _ => return None,
                }),
                _ => return None,
            };
            Some((frame, ev))
        })
        .collect()
}

impl Engine {
    /// `__post(cmd, x, y)` from the harness: the same commands wincap sends to the original
    /// (`move`, `click`, `rclick` in virtual pixels, `key` with a Windows virtual-key code).
    fn post_test_input(&self, cmd: &str, a: f32, b: f32) {
        let (x, y) = self.gfx.borrow().layout.to_window(a, b);
        let mut p = self.platform.borrow_mut();
        match cmd {
            "move" => p.inject(Input::MouseMove { x, y }),
            "click" => {
                p.inject(Input::MouseMove { x, y });
                p.inject(Input::Press(Button::Left));
                p.inject(Input::Release(Button::Left));
            }
            "rclick" => {
                p.inject(Input::MouseMove { x, y });
                p.inject(Input::Press(Button::Right));
                p.inject(Input::Release(Button::Right));
            }
            "key" => {
                let k = match a as u32 {
                    0x1b => KeyCode::Escape,
                    0x20 => KeyCode::Space,
                    0x25 => KeyCode::ArrowLeft,
                    0x26 => KeyCode::ArrowUp,
                    0x27 => KeyCode::ArrowRight,
                    0x28 => KeyCode::ArrowDown,
                    0x41 => KeyCode::KeyA,
                    0x44 => KeyCode::KeyD,
                    0x53 => KeyCode::KeyS,
                    0x57 => KeyCode::KeyW,
                    0x70 => KeyCode::F1,
                    0x74 => KeyCode::F5,
                    0x76 => KeyCode::F7,
                    _ => return,
                };
                p.inject(Input::Key(k));
            }
            _ => {}
        }
    }
}
