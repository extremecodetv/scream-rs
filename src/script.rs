//! The `sc*` Lua API (scream.exe wrappers 0x404df0–0x406040, registered in `scream_Main`).
//!
//! Wrappers read arguments with `lua_tonumber` (rounded with FRNDINT) and `lua_tolstring`
//! (nil → NULL, which callees treat as an empty string) and forward to the engine.

use std::rc::{Rc, Weak};

use mlua::{Lua, MultiValue, Value};

use crate::engine::Engine;

/// Argument accessor mirroring the Lua C API conversions the engine relies on.
pub struct Args(Vec<Value>);

impl Args {
    /// `ROUND(lua_tonumber(L, i))`, 1-based like the C API.
    pub fn int(&self, i: usize) -> i32 {
        self.num(i).round_ties_even() as i32
    }

    pub fn num(&self, i: usize) -> f64 {
        match self.0.get(i - 1) {
            Some(Value::Integer(n)) => *n as f64,
            Some(Value::Number(n)) => *n,
            Some(Value::String(s)) => s.to_str().ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0.0),
            _ => 0.0,
        }
    }

    /// `lua_tolstring(L, i)`, with NULL mapped to "".
    pub fn str(&self, i: usize) -> String {
        self.opt_str(i).unwrap_or_default()
    }

    pub fn opt_str(&self, i: usize) -> Option<String> {
        match self.0.get(i - 1) {
            Some(Value::String(s)) => Some(s.to_string_lossy().to_string()),
            Some(Value::Integer(n)) => Some(n.to_string()),
            Some(Value::Number(n)) => Some(lua_number_string(*n)),
            _ => None,
        }
    }
}

/// Lua 5.1 `%.14g` formatting of numbers converted to strings.
fn lua_number_string(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

pub enum Ret {
    None,
    Num(f64),
    Str(String),
}

type Binding = fn(&Engine, &Args) -> Ret;

/// Registers all `sc*` functions as globals. Callbacks hold a weak reference to the engine,
/// which owns the Lua state.
pub fn register(lua: &Lua, engine: Weak<Engine>) -> mlua::Result<()> {
    let table: &[(&str, Binding)] = &[
        ("scAbort", |e, _| {
            e.abort();
            Ret::None
        }),
        ("scAttach", |_, _| Ret::None),
        ("scAudio", |e, a| {
            e.attach_audio(a.int(1), &a.str(2));
            Ret::None
        }),
        ("scBegin", |e, a| {
            e.world_op("scBegin", |w| w.begin(a.int(1)));
            Ret::None
        }),
        ("scBind", |e, a| {
            e.world_op("scBind", |w| w.bind(a.int(1), &a.str(2)).map(|_| ()));
            Ret::None
        }),
        ("scChoiceAdd", |e, a| {
            e.world_op("scChoiceAdd", |w| w.choice_add(&a.str(1), &a.str(2)));
            Ret::None
        }),
        ("scChoiceClear", |e, _| {
            e.world_op("scChoiceClear", |w| w.choice_clear());
            Ret::None
        }),
        ("scChoiceGo", |e, a| {
            e.choice_go(&a.str(1));
            Ret::None
        }),
        ("scChoiceRemove", |e, _| {
            e.world_op("scChoiceRemove", |w| w.choice_remove());
            Ret::None
        }),
        ("scCreate", |e, a| {
            e.world_op("scCreate", |w| w.create(a.int(1), &a.str(2)).map(|_| ()));
            Ret::None
        }),
        ("scCursor", |e, a| {
            e.set_cursor_image(a.int(1), &a.str(2));
            Ret::None
        }),
        ("scCutscene", |e, a| {
            e.cutscene(&a.str(1));
            Ret::None
        }),
        ("scDisable", |e, a| {
            e.enable(a.int(1), false);
            Ret::None
        }),
        ("scEnable", |e, a| {
            e.enable(a.int(1), true);
            Ret::None
        }),
        ("scEnd", |e, a| {
            e.world_op("scEnd", |w| w.end(a.int(1)));
            Ret::None
        }),
        ("scFeedback", |e, a| {
            e.feedback(&a.str(1));
            Ret::None
        }),
        ("scFeedbackEx", |e, a| {
            e.feedback(&a.str(1));
            e.fx_play_ex(crate::sc::CH_DIALOGUE, &a.str(2), false, false);
            Ret::None
        }),
        ("scFont", |e, a| {
            e.set_font(&a.str(1));
            Ret::None
        }),
        ("scFootstep", |e, a| {
            e.world_op("scFootstep", |w| {
                let n = w.bound_id(crate::sc::NODE)?;
                w.nodes[n].footstep = a.str(1);
                Ok(())
            });
            Ret::None
        }),
        ("scFXPlay", |e, a| {
            e.fx_play(&a.str(1));
            Ret::None
        }),
        ("scFXPlayEx", |e, a| {
            e.fx_play_ex(a.int(1).max(0) as usize, &a.str(2), a.int(3) != 0, a.int(4) != 0);
            Ret::None
        }),
        ("scFXSet", |e, a| {
            e.fx_set(a.int(1).max(0) as usize, a.int(2), a.int(3));
            Ret::None
        }),
        ("scFXStop", |e, a| {
            e.audio.release(a.int(1).max(0) as usize);
            Ret::None
        }),
        ("scGet", |e, a| e.get(a.int(1))),
        ("scGo", |e, _| {
            e.wait_cursor(false);
            Ret::None
        }),
        ("scHints", |e, a| {
            e.hints(a.int(1));
            Ret::None
        }),
        ("scHintsSet", |e, a| {
            e.hints_set(a.int(1), &a.str(2));
            Ret::None
        }),
        ("scInit", |e, _| {
            e.init();
            Ret::None
        }),
        ("scInventoryAdd", |e, _| {
            e.world_op("scInventoryAdd", |w| w.inventory_add());
            Ret::None
        }),
        ("scInventoryReaction", |e, a| {
            e.world_op("scInventoryReaction", |w| w.add_item_reaction(&a.str(1), a.int(2), &a.str(3), &a.str(4)));
            Ret::None
        }),
        ("scInventoryRemove", |e, _| {
            e.world_op("scInventoryRemove", |w| w.inventory_remove());
            Ret::None
        }),
        ("scInventorySet", |e, a| {
            e.inventory_set(&a.str(1), &a.str(2), &a.str(3), &a.str(4));
            Ret::None
        }),
        ("scJournalAdd", |e, a| {
            e.journal.borrow_mut().add(a.int(1).max(0) as usize, &a.str(2));
            Ret::None
        }),
        ("scJournalClear", |e, _| {
            e.journal.borrow_mut().clear();
            Ret::None
        }),
        ("scJournalSet", |e, a| {
            let mut j = e.journal.borrow_mut();
            j.image = a.str(1);
            j.font = a.str(2);
            j.sound_open = a.str(3);
            j.sound_turn = a.str(4);
            j.sound_close = a.str(5);
            Ret::None
        }),
        ("scJournalShow", |e, _| {
            e.journal_show();
            Ret::None
        }),
        ("scLoadSettings", |e, a| {
            e.load_settings(&a.str(1));
            Ret::None
        }),
        ("scLoadSubtitle", |e, a| {
            e.subtitle_load(&a.str(1));
            Ret::None
        }),
        ("scLock", |e, _| {
            e.flags.borrow_mut().lock = true;
            Ret::None
        }),
        ("scLookAt", |e, a| {
            e.look_at(a.int(1), a.int(2));
            Ret::None
        }),
        ("scMenuSet", |e, a| {
            e.menu_set([&a.str(1), &a.str(2), &a.str(3), &a.str(4), &a.str(5), &a.str(6), &a.str(7)], &a.str(8), &a.str(9));
            Ret::None
        }),
        ("scMenuSetEx", |e, a| {
            e.menu_set_ex(a.int(1), &a.str(2));
            Ret::None
        }),
        ("scMenuSetSaveFunc", |e, _| {
            e.menu.borrow_mut().save_func = true;
            Ret::None
        }),
        ("scMenuShow", |e, _| {
            e.menu_show();
            Ret::None
        }),
        ("scMotion", |e, _| {
            e.flags.borrow_mut().motion = true;
            Ret::None
        }),
        ("scMusicFadeIn", |_, _| Ret::None),
        ("scMusicFadeOut", |_, _| Ret::None),
        ("scMusicPlay", |_, _| Ret::None),
        ("scMusicStop", |_, _| Ret::None),
        ("scOnClick", |e, a| {
            e.on_click(a.int(1), &a.str(2), &a.str(3));
            Ret::None
        }),
        ("scOnEnter", |e, a| {
            e.world_op("scOnEnter", |w| {
                let r = w.bound_id(crate::sc::ROOM)?;
                w.rooms[r].on_enter = a.str(1);
                Ok(())
            });
            Ret::None
        }),
        ("scOnExit", |e, a| {
            e.world_op("scOnExit", |w| {
                let r = w.bound_id(crate::sc::ROOM)?;
                w.rooms[r].on_exit = a.str(1);
                Ok(())
            });
            Ret::None
        }),
        ("scOnHover", |e, a| {
            e.world_op("scOnHover", |w| {
                w.object_mut()?.hover = a.int(1);
                Ok(())
            });
            Ret::None
        }),
        ("scOverlay", |e, a| {
            e.world_op("scOverlay", |w| {
                let o = w.bound_id(crate::sc::OVERLAY)?;
                let ov = &mut w.overlays[o];
                ov.texture = a.str(1);
                ov.open_sound = a.str(2);
                ov.close_sound = a.str(3);
                Ok(())
            });
            Ret::None
        }),
        ("scPlace", |e, a| {
            e.world_op("scPlace", |w| w.place(a.int(1), a.int(2), a.int(3), a.int(4), a.int(5)));
            Ret::None
        }),
        ("scPlaylistClear", |_, _| Ret::None),
        ("scPlaylistFirst", |_, _| Ret::None),
        ("scPlaylistLast", |_, _| Ret::None),
        ("scPlaylistNext", |_, _| Ret::None),
        ("scPlaylistPrevious", |_, _| Ret::None),
        ("scPlaylistQueue", |_, _| Ret::None),
        ("scPlaylistStart", |_, _| Ret::None),
        ("scPlaylistStop", |_, _| Ret::None),
        ("scReaction", |e, a| {
            e.world_op("scReaction", |w| w.add_reaction(&a.str(1), a.int(2), &a.str(3), &a.str(4)));
            Ret::None
        }),
        ("scRun", |e, _| {
            e.run();
            Ret::None
        }),
        ("scScriptAdd", |e, a| {
            e.world_op("scScriptAdd", |w| w.script_add(&a.str(1), &a.str(2), a.int(3)));
            Ret::None
        }),
        ("scScriptGo", |e, a| {
            e.script_go(&a.str(1));
            Ret::None
        }),
        ("scSet", |e, a| {
            e.set(a.int(1), &a);
            Ret::None
        }),
        ("scSetAmbience", |e, a| {
            e.set_ambience(a.int(1), a.int(2), a.int(3));
            Ret::None
        }),
        ("scSetLastVisited", |e, a| {
            e.world_op("scSetLastVisited", |w| {
                let r = w.bound_id(crate::sc::ROOM)?;
                let n = w.find(crate::sc::NODE, &a.str(1))?;
                w.rooms[r].entry_node = n;
                Ok(())
            });
            Ret::None
        }),
        ("scSleep", |e, a| {
            e.sleep(a.num(1) as f32);
            Ret::None
        }),
        ("scStill", |e, _| {
            e.flags.borrow_mut().motion = false;
            Ret::None
        }),
        ("scSwitch", |e, a| {
            e.switch(a.int(1), &a.str(2));
            Ret::None
        }),
        ("scTerminate", |e, _| {
            e.terminate();
            Ret::None
        }),
        ("scTexture", |e, a| {
            e.world_op("scTexture", |w| w.set_texture(a.int(1), &a.str(2)));
            Ret::None
        }),
        ("scTextureUpdate", |e, a| {
            e.texture_update(a.int(1), &a.str(2), a.int(3) != 0);
            Ret::None
        }),
        ("scTimerCreate", |e, a| {
            e.world_op("scTimerCreate", |w| w.create_timer(false, a.int(1), a.int(2), &a.str(3)));
            Ret::None
        }),
        ("scTimerCreateGlobal", |e, a| {
            e.world_op("scTimerCreateGlobal", |w| w.create_timer(true, a.int(1), a.int(2), &a.str(3)));
            Ret::None
        }),
        ("scTrackFadeOut", |e, a| {
            e.track_fade_out(a.int(1));
            Ret::None
        }),
        ("scTrackPlay", |e, a| {
            e.track_play(&a.str(1));
            Ret::None
        }),
        ("scTrackSet", |e, a| {
            e.world_op("scTrackSet", |w| {
                let t = w.bound_id(crate::sc::TRACK)?;
                let tr = &mut w.tracks[t];
                tr.front = a.str(1);
                tr.surround = false;
                tr.looped = a.int(2) != 0;
                Ok(())
            });
            Ret::None
        }),
        ("scTrackSet3D", |e, a| {
            e.world_op("scTrackSet3D", |w| {
                let t = w.bound_id(crate::sc::TRACK)?;
                let tr = &mut w.tracks[t];
                tr.front = a.str(1);
                tr.rear_left = a.str(2);
                tr.rear_right = a.str(3);
                tr.surround = true;
                tr.looped = a.int(4) != 0;
                Ok(())
            });
            Ret::None
        }),
        ("scTrackSetEx", |e, a| {
            e.world_op("scTrackSetEx", |w| {
                let t = w.bound_id(crate::sc::TRACK)?;
                let tr = &mut w.tracks[t];
                let v = a.int(2);
                match a.int(1) {
                    crate::sc::VOLUME => tr.volume = v,
                    crate::sc::FADEIN => tr.fade_in = v,
                    crate::sc::FADEOUT => tr.fade_out = v,
                    _ => {}
                }
                Ok(())
            });
            Ret::None
        }),
        ("scTrackStop", |e, _| {
            e.track_stop();
            Ret::None
        }),
        ("scUnloadSubtitle", |e, _| {
            e.subtitle_unload();
            Ret::None
        }),
        ("scUnlock", |e, _| {
            e.flags.borrow_mut().lock = false;
            Ret::None
        }),
        ("scWait", |e, _| {
            e.wait_cursor(true);
            Ret::None
        }),
    ];
    let globals = lua.globals();
    for &(name, f) in table {
        let engine = engine.clone();
        let func = lua.create_function(move |lua, args: MultiValue| {
            let Some(e) = engine.upgrade() else { return Ok(Value::Nil) };
            let args = Args(args.into_vec());
            Ok(match f(&e, &args) {
                Ret::None => Value::Integer(0),
                Ret::Num(n) => Value::Number(n),
                Ret::Str(s) => Value::String(lua.create_string(&s)?),
            })
        })?;
        globals.set(name, func)?;
    }
    Ok(())
}

/// Calls a global Lua function by name, as `lua_CallGlobal` (0x404c40) does. Errors are logged
/// and swallowed like the original's "Runtime error" log line.
pub fn call_global(lua: &Lua, name: &str) {
    if name.is_empty() {
        return;
    }
    match lua.globals().get::<Value>(name) {
        Ok(Value::Function(f)) => {
            log::debug!("lua: {name}()");
            if let Err(e) = f.call::<()>(()) {
                log::error!("lua: {name}: {e}");
            }
        }
        _ => log::warn!("lua: no function '{name}'"),
    }
}

/// Calls a global function returning a string (the menu save function).
pub fn call_global_str(lua: &Lua, name: &str) -> Option<String> {
    let f: mlua::Function = lua.globals().get(name).ok()?;
    match f.call::<Value>(()) {
        Ok(Value::String(s)) => Some(s.to_string_lossy().to_string()),
        Ok(_) => None,
        Err(e) => {
            log::error!("lua: {name}: {e}");
            None
        }
    }
}

pub type EngineRef = Rc<Engine>;
