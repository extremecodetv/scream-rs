//! Loads the original compiled scripts with stub `sc*` functions and reports what they call.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

fn main() -> mlua::Result<()> {
    let dir = std::env::args().nth(1).unwrap_or(r"D:\SteamLibrary\steamapps\common\Scratches".into());
    let lua = unsafe { mlua::Lua::unsafe_new() };
    let calls: Arc<Mutex<BTreeMap<String, usize>>> = Default::default();
    let names = ["scInit","scRun","scTerminate","scCreate","scBegin","scEnd","scTexture","scPlace","scOnClick","scOnHover",
        "scFootstep","scBind","scSet","scGet","scCursor","scFont","scMenuSet","scMenuSetEx","scMenuSetSaveFunc","scHintsSet",
        "scScriptAdd","scInventorySet","scInventoryReaction","scReaction","scAudio","scTrackSet","scTrackSet3D","scTrackSetEx",
        "scChoiceAdd","scDisable","scEnable","scOnEnter","scOnExit","scFeedback","scTextureUpdate","scOverlay","scJournalSet"];
    for n in names {
        let c = calls.clone();
        let name = n.to_string();
        lua.globals().set(n, lua.create_function(move |_, _: mlua::MultiValue| {
            *c.lock().unwrap().entry(name.clone()).or_default() += 1;
            Ok(0)
        })?)?;
    }
    for f in ["scream.sc", "scratches.sc"] {
        let bytes = std::fs::read(std::path::Path::new(&dir).join(f)).expect("read script");
        let bytes = scream::bytecode::adapt(&bytes).expect("adapt");
        lua.load(&bytes).set_name(f).set_mode(mlua::ChunkMode::Binary).exec()?;
        println!("{f}: ok");
    }
    for (k, v) in calls.lock().unwrap().iter() { println!("{k:24} {v}"); }
    Ok(())
}
