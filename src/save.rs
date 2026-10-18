//! Save games. The original (`game_Save` 0x411650 / `game_Load` 0x410a60) dumps raw engine
//! memory; this port writes JSON with the same content: the slot label, every numeric Lua
//! global, the settings that change in play (effects, ambience), the journal, camera, the dynamic
//! parts of the world and the audio channels.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::engine::Engine;
use crate::sc;
use crate::world::Timer;

#[derive(Serialize, Deserialize, Default)]
struct TimerSave {
    id: i32,
    interval: i32,
    elapsed: i32,
    enabled: bool,
    func: String,
}

impl From<&Timer> for TimerSave {
    fn from(t: &Timer) -> Self {
        TimerSave { id: t.id, interval: t.interval, elapsed: t.elapsed, enabled: t.enabled, func: t.func.clone() }
    }
}

impl From<&TimerSave> for Timer {
    fn from(t: &TimerSave) -> Self {
        Timer { id: t.id, interval: t.interval, elapsed: t.elapsed, enabled: t.enabled, func: t.func.clone() }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct RoomSave {
    cur_node: usize,
    entry_node: usize,
    zoom_stack: Vec<usize>,
    timers: Vec<TimerSave>,
}

#[derive(Serialize, Deserialize, Default)]
struct ItemSave {
    icon: String,
    title: String,
    refusal: String,
    refusal_audio: String,
}

#[derive(Serialize, Deserialize, Default)]
struct ChannelSave {
    ch: usize,
    file: String,
    loaded: bool,
    playing: bool,
    gain: f32,
    fade_in: i32,
    fade_out: i32,
    looped: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct SaveData {
    label: String,
    globals: Vec<(String, f64)>,
    fx: [bool; 8],
    ambience: [i32; 3],
    hints_active: bool,
    font: String,
    journal_lines: Vec<String>,
    journal_seen: Vec<usize>,
    journal_first: usize,
    yaw: f32,
    look_y: f32,
    state: i32,
    prev_state: i32,
    cur_room: usize,
    global_timers: Vec<TimerSave>,
    rooms: Vec<RoomSave>,
    node_faces: Vec<([String; 6], [String; 6])>,
    zoom_textures: Vec<(String, String)>,
    objects_enabled: Vec<bool>,
    items: Vec<ItemSave>,
    inventory: Vec<usize>,
    channels: Vec<ChannelSave>,
}

/// The label shown in the load/save menu, or `None` when the slot is empty.
pub fn read_label(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let data: SaveData = serde_json::from_str(&text).ok()?;
    Some(data.label)
}

/// Every global holding a number: the script keeps all world state in them (`first_cellar`,
/// `has_knife`, `currentGame`, ...). mlua hands integral Lua 5.1 numbers over as
/// `Value::Integer`, so both variants count.
fn numeric_globals(lua: &mlua::Lua) -> Result<Vec<(String, f64)>> {
    let mut out = Vec::new();
    for pair in lua.globals().pairs::<mlua::Value, mlua::Value>() {
        let (k, v) = pair.map_err(|e| anyhow::anyhow!("{e}"))?;
        let n = match v {
            mlua::Value::Integer(i) => i as f64,
            mlua::Value::Number(n) => n,
            _ => continue,
        };
        if let mlua::Value::String(k) = k {
            out.push((k.to_string_lossy().to_string(), n));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

pub fn save(e: &Engine, path: &Path, label: &str) -> Result<()> {
    let mut d = SaveData { label: label.into(), ..Default::default() };
    d.globals = numeric_globals(&e.lua)?;
    {
        let c = e.cfg.borrow();
        let f = &c.fx;
        d.fx = [f.sky, f.night, f.rain, f.fog, f.glow, f.dream, f.candle, f.lamp];
    }
    d.ambience = e.ambience.get();
    d.hints_active = e.hints_state.borrow().active;
    d.font = e.font.borrow().clone();
    {
        let j = e.journal.borrow();
        d.journal_lines = j.lines.clone();
        d.journal_seen = j.seen.iter().enumerate().filter(|(_, s)| **s).map(|(i, _)| i).collect();
        d.journal_first = j.first;
    }
    {
        let c = e.cam.borrow();
        d.yaw = c.yaw;
        d.look_y = c.look_y;
    }
    {
        let w = e.world.borrow();
        // A save made from a menu or overlay returns to what was underneath.
        d.state = match w.state {
            sc::ST_MENU => e.menu.borrow().saved_state,
            sc::ST_ZOOM => sc::ST_ZOOM,
            _ => sc::ST_NODE,
        };
        d.prev_state = w.prev_state;
        d.cur_room = w.cur_room;
        d.global_timers = w.global_timers.iter().map(Into::into).collect();
        d.rooms = w
            .rooms
            .iter()
            .map(|r| RoomSave {
                cur_node: r.cur_node,
                entry_node: r.entry_node,
                zoom_stack: r.zoom_stack.clone(),
                timers: r.timers.iter().map(Into::into).collect(),
            })
            .collect();
        d.node_faces = w.nodes.iter().map(|n| (n.faces.clone(), n.masks.clone())).collect();
        d.zoom_textures = w.zooms.iter().map(|z| (z.texture.clone(), z.mask.clone())).collect();
        d.objects_enabled = w.objects.iter().map(|o| o.enabled).collect();
        d.items = w
            .items
            .iter()
            .map(|i| ItemSave {
                icon: i.icon.clone(),
                title: i.title.clone(),
                refusal: i.refusal.clone(),
                refusal_audio: i.refusal_audio.clone(),
            })
            .collect();
        d.inventory = w.inventory.clone();
    }
    for ch in 1..crate::audio::CHANNELS {
        let i = e.audio.info(ch);
        d.channels.push(ChannelSave {
            ch,
            file: i.file,
            loaded: i.loaded,
            playing: i.playing,
            gain: i.target_gain,
            fade_in: i.fade_in,
            fade_out: i.fade_out,
            looped: i.looped,
        });
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string(&d)?).with_context(|| path.display().to_string())?;
    Ok(())
}

pub fn load(e: &Engine, path: &Path) -> Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    let d: SaveData = serde_json::from_str(&text)?;
    let err = |e: mlua::Error| anyhow::anyhow!("{e}");
    let g = e.lua.globals();
    // A flag the script created after this save was made did not exist then. Saves from before
    // 0.1.2 carry no globals at all; those keep the running state rather than wiping it.
    if !d.globals.is_empty() {
        let saved: std::collections::HashSet<&str> = d.globals.iter().map(|(k, _)| k.as_str()).collect();
        for (k, _) in numeric_globals(&e.lua)? {
            if !saved.contains(k.as_str()) {
                g.set(k.as_str(), mlua::Value::Nil).map_err(err)?;
            }
        }
    }
    for (k, v) in &d.globals {
        g.set(k.as_str(), *v).map_err(err)?;
    }
    {
        let mut c = e.cfg.borrow_mut();
        let f = &mut c.fx;
        [f.sky, f.night, f.rain, f.fog, f.glow, f.dream, f.candle, f.lamp] = d.fx;
    }
    e.ambience.set(d.ambience);
    e.hints_state.borrow_mut().active = d.hints_active;
    {
        let mut j = e.journal.borrow_mut();
        j.lines = d.journal_lines.clone();
        j.seen.iter_mut().for_each(|s| *s = false);
        for &i in &d.journal_seen {
            if let Some(s) = j.seen.get_mut(i) {
                *s = true;
            }
        }
        j.first = d.journal_first;
    }
    let (room_name, zoom_name) = {
        let mut w = e.world.borrow_mut();
        w.prev_state = d.prev_state;
        w.global_timers = d.global_timers.iter().map(Into::into).collect();
        for (r, s) in w.rooms.iter_mut().zip(&d.rooms) {
            r.cur_node = s.cur_node;
            r.entry_node = s.entry_node;
            r.zoom_stack = s.zoom_stack.clone();
            r.timers = s.timers.iter().map(Into::into).collect();
        }
        for (n, (faces, masks)) in w.nodes.iter_mut().zip(&d.node_faces) {
            n.faces = faces.clone();
            n.masks = masks.clone();
        }
        for (z, (t, m)) in w.zooms.iter_mut().zip(&d.zoom_textures) {
            z.texture = t.clone();
            z.mask = m.clone();
        }
        for (o, en) in w.objects.iter_mut().zip(&d.objects_enabled) {
            o.enabled = *en;
        }
        for (it, s) in w.items.iter_mut().zip(&d.items) {
            it.icon = s.icon.clone();
            it.title = s.title.clone();
            it.refusal = s.refusal.clone();
            it.refusal_audio = s.refusal_audio.clone();
        }
        w.inventory = if d.inventory.is_empty() { vec![0] } else { d.inventory.clone() };
        w.held = 0;
        w.cur_room = d.cur_room.min(w.rooms.len().saturating_sub(1));
        let zoom = if d.state == sc::ST_ZOOM { w.rooms[w.cur_room].zoom_stack.last().copied() } else { None };
        if zoom.is_some() {
            w.rooms.get_mut(d.cur_room).map(|r| r.zoom_stack.pop());
        }
        let room = w.cur_room;
        let _ = w.set_bind(sc::ROOM, room);
        (w.rooms[w.cur_room].name.clone(), zoom.map(|z| w.zooms[z].name.clone()))
    };
    for ch in &d.channels {
        if ch.ch == 8 || ch.ch == 14 {
            continue;
        }
        e.audio.release(ch.ch);
        if ch.loaded {
            e.audio.load(ch.ch, &ch.file, e.read_res(&ch.file));
        }
        e.audio.set_gain(ch.ch, ch.gain);
        if ch.playing {
            e.audio.play(ch.ch, ch.looped);
        }
        if ch.fade_out != 0 {
            e.audio.fade_out(ch.ch, ch.fade_out);
        }
        if ch.fade_in != 0 {
            e.audio.fade_in(ch.ch, ch.fade_in);
        }
    }
    // The new-game snapshot is taken from the menu, before any room was entered.
    if d.cur_room != 0 {
        e.switch(sc::ROOM, &room_name);
    }
    if let Some(z) = zoom_name {
        e.switch(sc::ZOOM, &z);
    }
    e.cam.borrow_mut().set(d.yaw, d.look_y);
    let icons: Vec<(usize, String)> = {
        let w = e.world.borrow();
        w.items.iter().enumerate().skip(1).map(|(i, it)| (i, it.icon.clone())).collect()
    };
    for (i, icon) in icons {
        if !icon.is_empty() {
            e.load_tex(crate::gfx::slot::ICON + i as u32, &icon);
        }
    }
    if !d.font.is_empty() {
        e.set_font(&d.font);
    }
    Ok(())
}
