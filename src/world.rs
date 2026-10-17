//! The game world built by the script: rooms, nodes, zooms, objects, items, dialogue scripts,
//! choice sets and music tracks, plus the "bound object" registers the script API works through.
//!
//! Mirrors scream.exe's static world (0x49fc70…): every collection is 1-based (id 0 = none),
//! objects are referenced by id, and lookups by name follow `world_RetrieveID` (0x403ec0).

use crate::sc;

pub type Id = usize;

#[derive(Clone, Debug, Default)]
pub struct Timer {
    pub id: i32,
    pub interval: i32,
    pub elapsed: i32,
    pub enabled: bool,
    pub func: String,
}

#[derive(Clone, Debug, Default)]
pub struct Room {
    pub name: String,
    pub on_enter: String,
    pub on_exit: String,
    /// Stereo ambience (`scAudio(SC_STEREO, file)` while describing the room).
    pub ambience: String,
    pub nodes: Vec<Id>,
    pub cur_node: Id,
    /// Node entered when switching to the room (first node, then the last one visited).
    pub entry_node: Id,
    pub zoom_stack: Vec<Id>,
    pub timers: Vec<Timer>,
}

#[derive(Clone, Debug, Default)]
pub struct Node {
    pub name: String,
    /// Face textures in SC_FRONT..SC_DOWN order (index = face - 1).
    pub faces: [String; 6],
    /// Per-face masks derived from the texture name (`<base>_msk.tga`).
    pub masks: [String; 6],
    pub objects: Vec<Id>,
    pub footstep: String,
}

#[derive(Clone, Debug, Default)]
pub struct Zoom {
    pub name: String,
    pub texture: String,
    pub mask: String,
    pub objects: Vec<Id>,
    pub enter_sound: String,
    pub exit_sound: String,
}

#[derive(Clone, Debug, Default)]
pub struct Reaction {
    pub item: Id,
    pub kind: i32,
    /// Feedback text, or the Lua function name for SC_CUSTOM.
    pub text: String,
    pub audio: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Object {
    pub name: String,
    pub enabled: bool,
    pub rect: Rect,
    /// SC_FRONT..SC_DOWN for node hotspots; zoom hotspots keep the face they were placed with.
    pub face: i32,
    /// Cursor kind on hover (SC_NORMAL, SC_LOOK, ...).
    pub hover: i32,
    /// Click action: SC_FEEDBACK, SC_CUSTOM, SC_FEEDBACK_EX, SC_ZOOMOUT/ZOOMBACK or an object type.
    pub action: i32,
    /// Feedback text, Lua function name or switch target name, depending on `action`.
    pub target: String,
    pub audio: String,
    pub reactions: Vec<Reaction>,
}

#[derive(Clone, Debug, Default)]
pub struct Overlay {
    pub name: String,
    pub texture: String,
    pub open_sound: String,
    pub close_sound: String,
}

#[derive(Clone, Debug, Default)]
pub struct Item {
    pub name: String,
    pub icon: String,
    pub title: String,
    /// Said when the item is used on something that has no reaction to it.
    pub refusal: String,
    pub refusal_audio: String,
    /// Reactions when another item is used on this one (inventory combinations).
    pub reactions: Vec<Reaction>,
}

#[derive(Clone, Debug, Default)]
pub struct ScriptLine {
    pub audio: String,
    pub text: String,
    /// 1 = only played with hints enabled.
    pub flag: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Script {
    pub name: String,
    pub lines: Vec<ScriptLine>,
}

#[derive(Clone, Debug, Default)]
pub struct ChoiceOption {
    pub text: String,
    pub func: String,
}

#[derive(Clone, Debug, Default)]
pub struct Choice {
    pub name: String,
    pub options: Vec<ChoiceOption>,
    /// Index passed to the handler (`+0x38`), set when an option is clicked.
    pub selected: i32,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub name: String,
    pub front: String,
    pub rear_left: String,
    pub rear_right: String,
    pub surround: bool,
    /// 0..10 (SC_VOLUME).
    pub volume: i32,
    pub fade_in: i32,
    pub fade_out: i32,
    pub looped: bool,
}

impl Default for Track {
    fn default() -> Self {
        Track {
            name: String::new(),
            front: String::new(),
            rear_left: String::new(),
            rear_right: String::new(),
            surround: false,
            volume: 10,
            fade_in: 1,
            fade_out: 1,
            looped: false,
        }
    }
}

/// Fixed capacities of the original static arrays; exceeding one is a fatal script error there.
const MAX_ROOMS: usize = 58;
const MAX_NODES: usize = 214;
const MAX_ZOOMS: usize = 376;
const MAX_OBJECTS: usize = 2298;
const MAX_ITEMS: usize = 59;
const MAX_SCRIPTS: usize = 51;
const MAX_CHOICES: usize = 15;
const MAX_TRACKS: usize = 38;
const MAX_TIMERS: usize = 31;
const MAX_REACTIONS: usize = 63;
const MAX_SCRIPT_LINES: usize = 21;
const MAX_CHOICE_OPTIONS: usize = 16;

#[derive(Debug)]
pub struct WorldError(pub String);

impl std::fmt::Display for WorldError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WorldError {}

pub type Result<T> = std::result::Result<T, WorldError>;

fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(WorldError(msg.into()))
}

/// Collections are padded with a dummy element at index 0 so ids index them directly.
fn padded<T: Default>() -> Vec<T> {
    vec![T::default()]
}

pub struct World {
    pub rooms: Vec<Room>,
    pub nodes: Vec<Node>,
    pub zooms: Vec<Zoom>,
    pub objects: Vec<Object>,
    pub overlays: Vec<Overlay>,
    pub items: Vec<Item>,
    pub scripts: Vec<Script>,
    pub choices: Vec<Choice>,
    pub tracks: Vec<Track>,
    /// Bound object per type, index = type - SC_ROOM.
    binds: [Id; 9],
    pub state: i32,
    pub prev_state: i32,
    /// Construction nesting: SC_WORLD → SC_ROOM → SC_NODE / SC_ZOOM.
    level: i32,
    pub cur_room: Id,
    /// Inventory slots 1.. (index 0 unused), each holding an item id.
    pub inventory: Vec<Id>,
    pub held: Id,
    pub global_timers: Vec<Timer>,
}

impl Default for World {
    fn default() -> Self {
        World::new()
    }
}

impl World {
    /// `world_Reset` (0x402280).
    pub fn new() -> World {
        World {
            rooms: padded(),
            nodes: padded(),
            zooms: padded(),
            objects: padded(),
            overlays: padded(),
            items: padded(),
            scripts: padded(),
            choices: padded(),
            tracks: padded(),
            binds: [0; 9],
            state: 0,
            prev_state: 0,
            level: sc::WORLD,
            cur_room: 0,
            inventory: vec![0],
            held: 0,
            global_timers: Vec::new(),
        }
    }

    fn bind_slot(kind: i32) -> Result<usize> {
        if (sc::ROOM..=sc::TRACK).contains(&kind) {
            Ok((kind - sc::ROOM) as usize)
        } else {
            err(format!("invalid bind {kind}"))
        }
    }

    pub fn bound(&self, kind: i32) -> Id {
        Self::bind_slot(kind).map(|s| self.binds[s]).unwrap_or(0)
    }

    /// `world_GetBind`: the bound id, or an error when nothing is bound.
    pub fn bound_id(&self, kind: i32) -> Result<Id> {
        match self.bound(kind) {
            0 => err(format!("type {kind} not bound")),
            id => Ok(id),
        }
    }

    pub fn set_bind(&mut self, kind: i32, id: Id) -> Result<()> {
        let s = Self::bind_slot(kind)?;
        if id >= self.count(kind) {
            return err(format!("invalid id {id} for type {kind}"));
        }
        self.binds[s] = id;
        Ok(())
    }

    fn count(&self, kind: i32) -> usize {
        match kind {
            sc::ROOM => self.rooms.len(),
            sc::NODE => self.nodes.len(),
            sc::ZOOM => self.zooms.len(),
            sc::OBJECT => self.objects.len(),
            sc::OVERLAY => self.overlays.len(),
            sc::ITEM => self.items.len(),
            sc::SCRIPT => self.scripts.len(),
            sc::CHOICE => self.choices.len(),
            sc::TRACK => self.tracks.len(),
            _ => 0,
        }
    }

    pub fn name_of(&self, kind: i32, id: Id) -> &str {
        let name = match kind {
            sc::ROOM => self.rooms.get(id).map(|o| &o.name),
            sc::NODE => self.nodes.get(id).map(|o| &o.name),
            sc::ZOOM => self.zooms.get(id).map(|o| &o.name),
            sc::OBJECT => self.objects.get(id).map(|o| &o.name),
            sc::OVERLAY => self.overlays.get(id).map(|o| &o.name),
            sc::ITEM => self.items.get(id).map(|o| &o.name),
            sc::SCRIPT => self.scripts.get(id).map(|o| &o.name),
            sc::CHOICE => self.choices.get(id).map(|o| &o.name),
            sc::TRACK => self.tracks.get(id).map(|o| &o.name),
            _ => None,
        };
        name.map(String::as_str).unwrap_or("")
    }

    /// `scCreate(type, name)`: appends a new object and binds it.
    pub fn create(&mut self, kind: i32, name: &str) -> Result<Id> {
        if name.len() > 47 {
            return err(format!("{name}: filename too long"));
        }
        let name = name.to_owned();
        let (len, max) = match kind {
            sc::ROOM => {
                self.rooms.push(Room { name, ..Default::default() });
                (self.rooms.len(), MAX_ROOMS)
            }
            sc::NODE => {
                self.nodes.push(Node { name, ..Default::default() });
                (self.nodes.len(), MAX_NODES)
            }
            sc::ZOOM => {
                self.zooms.push(Zoom { name, ..Default::default() });
                (self.zooms.len(), MAX_ZOOMS)
            }
            sc::OBJECT => {
                self.objects.push(Object { name, ..Default::default() });
                (self.objects.len(), MAX_OBJECTS)
            }
            sc::OVERLAY => {
                self.overlays.push(Overlay { name, ..Default::default() });
                (self.overlays.len(), 1)
            }
            sc::ITEM => {
                self.items.push(Item { name, ..Default::default() });
                (self.items.len(), MAX_ITEMS)
            }
            sc::SCRIPT => {
                self.scripts.push(Script { name, ..Default::default() });
                (self.scripts.len(), MAX_SCRIPTS)
            }
            sc::CHOICE => {
                self.choices.push(Choice { name, ..Default::default() });
                (self.choices.len(), MAX_CHOICES)
            }
            sc::TRACK => {
                self.tracks.push(Track { name, ..Default::default() });
                (self.tracks.len(), MAX_TRACKS)
            }
            _ => return err(format!("Create: invalid type {kind}")),
        };
        let id = len - 1;
        if id > max {
            log::warn!("Create: too many objects of type {kind} ({id} > {max})");
        }
        if kind == sc::ROOM {
            // The room being described becomes the current one, like the original.
            self.cur_room = id;
        }
        self.binds[Self::bind_slot(kind)?] = id;
        Ok(id)
    }

    /// `world_RetrieveID` (0x403ec0): name lookup with the original scoping rules.
    pub fn find(&self, kind: i32, name: &str) -> Result<Id> {
        let found = match kind {
            sc::ROOM => position(&self.rooms, |r| r.name == name),
            sc::NODE => {
                // Only nodes of the bound room match.
                let room = self.rooms.get(self.bound(sc::ROOM));
                room.and_then(|r| r.nodes.iter().copied().find(|&n| self.nodes[n].name == name))
            }
            sc::ZOOM => position(&self.zooms, |z| z.name == name),
            sc::OBJECT => {
                // Objects of the bound node while exploring, of the bound zoom inside a zoom.
                let scope: &[Id] = match self.state {
                    sc::ST_NODE => self.nodes.get(self.bound(sc::NODE)).map_or(&[], |n| &n.objects),
                    sc::ST_ZOOM => self.zooms.get(self.bound(sc::ZOOM)).map_or(&[], |z| &z.objects),
                    _ => &[],
                };
                scope.iter().copied().find(|&o| self.objects[o].name == name)
            }
            sc::OVERLAY => position(&self.overlays, |o| o.name == name),
            sc::ITEM => position(&self.items, |o| o.name == name),
            sc::SCRIPT => position(&self.scripts, |o| o.name == name),
            sc::CHOICE => position(&self.choices, |o| o.name == name),
            sc::TRACK => position(&self.tracks, |o| o.name == name),
            _ => return err(format!("RetrieveID: invalid type {kind}")),
        };
        match found {
            Some(id) => Ok(id),
            None => err(format!("RetrieveID: ID not found for '{name}' (type {kind})")),
        }
    }

    /// `scBind(type, name)`.
    pub fn bind(&mut self, kind: i32, name: &str) -> Result<Id> {
        let id = self.find(kind, name)?;
        self.set_bind(kind, id)?;
        Ok(id)
    }

    /// `core_Begin` (0x414b20): rooms at world level; nodes and zooms inside rooms. A node is
    /// attached to the bound room when its description begins.
    pub fn begin(&mut self, kind: i32) -> Result<()> {
        match kind {
            sc::ROOM if self.level == sc::WORLD => self.level = sc::ROOM,
            sc::NODE if self.level == sc::ROOM => {
                let node = self.bound_id(sc::NODE)?;
                let room = self.bound_id(sc::ROOM)?;
                let r = &mut self.rooms[room];
                r.nodes.push(node);
                if r.nodes.len() == 1 {
                    r.entry_node = node;
                }
                self.level = sc::NODE;
            }
            sc::ZOOM if self.level == sc::ROOM => self.level = sc::ZOOM,
            _ => return err(format!("scBegin: invalid begin {kind} at level {}", self.level)),
        }
        Ok(())
    }

    /// `core_End` (0x414be0).
    pub fn end(&mut self, kind: i32) -> Result<()> {
        if kind != self.level {
            return err(format!("scEnd: invalid call {kind} at level {}", self.level));
        }
        self.level = match kind {
            sc::NODE | sc::ZOOM => sc::ROOM,
            _ => sc::WORLD,
        };
        Ok(())
    }

    pub fn level(&self) -> i32 {
        self.level
    }

    /// `core_Place` (0x415980): positions the bound object and attaches it to the node or zoom
    /// being described. Zoom coordinates are stored 64 px lower (below the top letterbox bar).
    pub fn place(&mut self, x: i32, y: i32, w: i32, h: i32, face: i32) -> Result<()> {
        let in_zoom = self.level == sc::ZOOM;
        let id = self.bound_id(sc::OBJECT)?;
        let o = &mut self.objects[id];
        o.enabled = true;
        o.rect = Rect { x, y: if in_zoom { y + 64 } else { y }, w, h };
        o.face = face;
        if in_zoom {
            let z = self.bound_id(sc::ZOOM)?;
            self.zooms[z].objects.push(id);
        } else {
            let n = self.bound_id(sc::NODE)?;
            self.nodes[n].objects.push(id);
        }
        Ok(())
    }

    /// `core_Texture` (0x414d40): describes a node face, the bound zoom or the bound overlay.
    pub fn set_texture(&mut self, target: i32, file: &str) -> Result<()> {
        match target {
            sc::FRONT..=sc::DOWN => {
                let n = self.bound_id(sc::NODE)?;
                let i = (target - 1) as usize;
                self.nodes[n].faces[i] = file.to_owned();
                self.nodes[n].masks[i] = mask_name(file);
            }
            sc::ZOOM => {
                let z = self.bound_id(sc::ZOOM)?;
                self.zooms[z].texture = file.to_owned();
                self.zooms[z].mask = mask_name(file);
            }
            sc::OVERLAY => {
                let o = self.bound_id(sc::OVERLAY)?;
                self.overlays[o].texture = file.to_owned();
            }
            _ => {}
        }
        Ok(())
    }

    pub fn object_mut(&mut self) -> Result<&mut Object> {
        let id = self.bound_id(sc::OBJECT)?;
        Ok(&mut self.objects[id])
    }

    /// `core_Reaction` (0x412c20): reaction of the bound object to an item.
    pub fn add_reaction(&mut self, item: &str, kind: i32, text: &str, audio: &str) -> Result<()> {
        let item = self.find(sc::ITEM, item)?;
        let o = self.object_mut()?;
        if o.reactions.len() >= MAX_REACTIONS {
            return err("too many reactions");
        }
        o.reactions.push(Reaction { item, kind, text: text.into(), audio: audio.into() });
        Ok(())
    }

    /// `core_InventoryReaction` (0x412e10): registered on both items so combining works either way.
    pub fn add_item_reaction(&mut self, other: &str, kind: i32, text: &str, audio: &str) -> Result<()> {
        let this = self.bound_id(sc::ITEM)?;
        let other = self.find(sc::ITEM, other)?;
        let r = |item| Reaction { item, kind, text: text.into(), audio: audio.into() };
        self.items[this].reactions.push(r(other));
        self.items[other].reactions.push(r(this));
        Ok(())
    }

    /// `scInventorySet(icon, title, refusal, audio)` on the bound item.
    pub fn set_item(&mut self, icon: &str, title: &str, refusal: &str, audio: &str) -> Result<()> {
        let id = self.bound_id(sc::ITEM)?;
        let it = &mut self.items[id];
        it.icon = icon.into();
        it.title = title.into();
        it.refusal = refusal.into();
        it.refusal_audio = audio.into();
        Ok(())
    }

    /// `scInventoryAdd`: appends the bound item to the inventory.
    pub fn inventory_add(&mut self) -> Result<()> {
        let id = self.bound_id(sc::ITEM)?;
        self.inventory.push(id);
        Ok(())
    }

    /// `scInventoryRemove`: removes the bound item (the one in hand when used from a reaction).
    pub fn inventory_remove(&mut self) -> Result<()> {
        let id = self.bound_id(sc::ITEM)?;
        if let Some(pos) = self.inventory.iter().skip(1).position(|&i| i == id) {
            self.inventory.remove(pos + 1);
        }
        if self.held == id {
            self.held = 0;
        }
        Ok(())
    }

    pub fn slot(&self, slot: usize) -> Id {
        self.inventory.get(slot).copied().unwrap_or(0)
    }

    /// `scScriptAdd(ogg, text, flag)` on the bound script.
    pub fn script_add(&mut self, audio: &str, text: &str, flag: i32) -> Result<()> {
        let id = self.bound_id(sc::SCRIPT)?;
        let s = &mut self.scripts[id];
        if s.lines.len() >= MAX_SCRIPT_LINES {
            return err("ScriptAdd: too many lines");
        }
        s.lines.push(ScriptLine { audio: audio.into(), text: text.into(), flag });
        Ok(())
    }

    /// `scChoiceAdd(text, fn)` on the bound choice set.
    pub fn choice_add(&mut self, text: &str, func: &str) -> Result<()> {
        let id = self.bound_id(sc::CHOICE)?;
        let c = &mut self.choices[id];
        if c.options.len() < MAX_CHOICE_OPTIONS {
            c.options.push(ChoiceOption { text: text.into(), func: func.into() });
        }
        Ok(())
    }

    /// `scChoiceRemove`: drops the option chosen last (`+0x38`).
    pub fn choice_remove(&mut self) -> Result<()> {
        let id = self.bound_id(sc::CHOICE)?;
        let c = &mut self.choices[id];
        let i = c.selected.max(0) as usize;
        if i < c.options.len() {
            c.options.remove(i);
        }
        Ok(())
    }

    pub fn choice_clear(&mut self) -> Result<()> {
        let id = self.bound_id(sc::CHOICE)?;
        self.choices[id].options.clear();
        Ok(())
    }

    /// `world_SetState` (0x402cf0): STLAST swaps back; STOVERLAY keeps the previous state.
    pub fn set_state(&mut self, state: i32) {
        if state == sc::ST_LAST {
            std::mem::swap(&mut self.state, &mut self.prev_state);
            return;
        }
        if state != sc::ST_OVERLAY {
            self.prev_state = self.state;
        }
        self.state = state;
    }

    pub fn room(&self) -> &Room {
        &self.rooms[self.cur_room]
    }

    pub fn room_mut(&mut self) -> &mut Room {
        &mut self.rooms[self.cur_room]
    }

    pub fn cur_node(&self) -> Id {
        self.rooms.get(self.cur_room).map_or(0, |r| r.cur_node)
    }

    pub fn cur_zoom(&self) -> Id {
        self.rooms.get(self.cur_room).and_then(|r| r.zoom_stack.last().copied()).unwrap_or(0)
    }

    /// `world_Switch(SC_NODE)`: the node must belong to the current room.
    pub fn enter_node(&mut self, node: Id) -> Result<()> {
        let room = &mut self.rooms[self.cur_room];
        if !room.nodes.contains(&node) {
            return err(format!("Switch: node {node} not in room"));
        }
        room.cur_node = node;
        Ok(())
    }

    /// Timer of the bound room (`timer_Create` 0x412540) or global (`timer_CreateGlobal`).
    /// Re-creating a timer with the same id restarts it.
    pub fn create_timer(&mut self, global: bool, id: i32, interval: i32, func: &str) -> Result<()> {
        let timers = if global {
            &mut self.global_timers
        } else {
            let room = self.bound_id(sc::ROOM)?;
            &mut self.rooms[room].timers
        };
        let t = Timer { id, interval, elapsed: 0, enabled: true, func: func.into() };
        if let Some(slot) = timers.iter_mut().find(|t| t.id == id) {
            *slot = t;
        } else if timers.len() < MAX_TIMERS {
            timers.push(t);
        } else {
            return err("Too many timers!");
        }
        Ok(())
    }
}

fn position<T>(v: &[T], f: impl Fn(&T) -> bool) -> Option<Id> {
    v.iter().enumerate().skip(1).find(|(_, x)| f(x)).map(|(i, _)| i)
}

/// `name.jpg` → `name_msk.tga` (the part before the first dot, as `strtok(name, ".")` does).
pub fn mask_name(texture: &str) -> String {
    let base = texture.split('.').next().unwrap_or(texture);
    format!("{base}_msk.tga")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn garden() -> World {
        let mut w = World::new();
        w.create(sc::ROOM, "garden").unwrap();
        w.begin(sc::ROOM).unwrap();
        w.create(sc::NODE, "n1").unwrap();
        w.begin(sc::NODE).unwrap();
        w.set_texture(sc::FRONT, "garden_n1f.jpg").unwrap();
        w.create(sc::OBJECT, "to_n2").unwrap();
        w.place(10, 20, 30, 40, sc::FRONT).unwrap();
        w.end(sc::NODE).unwrap();
        w.create(sc::ZOOM, "mailbox").unwrap();
        w.begin(sc::ZOOM).unwrap();
        w.set_texture(sc::ZOOM, "hs-mailbox.jpg").unwrap();
        w.create(sc::OBJECT, "open").unwrap();
        w.place(320, 210, 40, 35, sc::FRONT).unwrap();
        w.end(sc::ZOOM).unwrap();
        w.end(sc::ROOM).unwrap();
        w
    }

    #[test]
    fn builds_rooms_nodes_and_zoom_objects() {
        let w = garden();
        assert_eq!(w.rooms[1].nodes, vec![1]);
        assert_eq!(w.rooms[1].entry_node, 1);
        assert_eq!(w.nodes[1].objects, vec![1]);
        assert_eq!(w.nodes[1].masks[0], "garden_n1f_msk.tga");
        assert_eq!(w.zooms[1].objects, vec![2]);
        // Zoom hotspots are shifted below the 64 px bar.
        assert_eq!(w.objects[2].rect, Rect { x: 320, y: 274, w: 40, h: 35 });
        assert_eq!(w.objects[1].rect.y, 20);
        assert_eq!(w.level(), sc::WORLD);
    }

    #[test]
    fn object_lookup_is_scoped_by_state() {
        let mut w = garden();
        assert!(w.find(sc::OBJECT, "to_n2").is_err());
        w.set_state(sc::ST_NODE);
        w.set_bind(sc::NODE, 1).unwrap();
        assert_eq!(w.find(sc::OBJECT, "to_n2").unwrap(), 1);
        assert!(w.find(sc::OBJECT, "open").is_err());
        w.set_state(sc::ST_ZOOM);
        w.set_bind(sc::ZOOM, 1).unwrap();
        assert_eq!(w.find(sc::OBJECT, "open").unwrap(), 2);
    }

    #[test]
    fn state_last_swaps_back() {
        let mut w = World::new();
        w.set_state(sc::ST_NODE);
        w.set_state(sc::ST_INV);
        w.set_state(sc::ST_LAST);
        assert_eq!(w.state, sc::ST_NODE);
        w.set_state(sc::ST_OVERLAY);
        assert_eq!(w.prev_state, sc::ST_INV);
    }

    #[test]
    fn timers_restart_by_id() {
        let mut w = garden();
        w.create_timer(true, 7, 20, "a").unwrap();
        w.global_timers[0].elapsed = 5;
        w.create_timer(true, 7, 30, "b").unwrap();
        assert_eq!(w.global_timers.len(), 1);
        assert_eq!(w.global_timers[0].elapsed, 0);
        assert_eq!(w.global_timers[0].func, "b");
    }

    #[test]
    fn inventory_reactions_are_symmetric() {
        let mut w = World::new();
        w.create(sc::ITEM, "can").unwrap();
        w.create(sc::ITEM, "water").unwrap();
        w.bind(sc::ITEM, "can").unwrap();
        w.add_item_reaction("water", sc::CUSTOM, "fill", "").unwrap();
        assert_eq!(w.items[1].reactions[0].item, 2);
        assert_eq!(w.items[2].reactions[0].item, 1);
    }
}
