//! Numeric constants of the SCream script API. The game scripts define the same values as Lua
//! globals in `scream.sc`; the engine only needs them to interpret arguments.

// Object types (`scCreate`, `scBind`, `scSwitch`, ...).
pub const ROOM: i32 = 2000;
pub const NODE: i32 = 2001;
pub const ZOOM: i32 = 2002;
pub const OBJECT: i32 = 2003;
pub const OVERLAY: i32 = 2004;
pub const ITEM: i32 = 2005;
pub const SCRIPT: i32 = 2006;
pub const CHOICE: i32 = 2007;
pub const TRACK: i32 = 2008;

// Tags.
pub const WORLD: i32 = 3000;
pub const MENU: i32 = 3005;
pub const INTRO: i32 = 3006;
pub const IMAGE: i32 = 3008;
pub const OUTRO: i32 = 3012;

// Player states.
pub const ST_NODE: i32 = 4000;
pub const ST_VIDEO: i32 = 4001;
pub const ST_ZOOM: i32 = 4002;
pub const ST_INV: i32 = 4003;
pub const ST_OVERLAY: i32 = 4004;
pub const ST_MENU: i32 = 4005;
pub const ST_LAST: i32 = 4006;
pub const ST_CHOICE: i32 = 4007;
pub const ST_BOOK: i32 = 4008;

// Click actions.
pub const FEEDBACK: i32 = 30;
pub const CUSTOM: i32 = 31;
pub const ZOOMOUT: i32 = 32;
pub const ZOOMBACK: i32 = 33;
pub const FEEDBACK_EX: i32 = 34;

// Audio properties.
pub const STEREO: i32 = 40;
pub const VOLUME: i32 = 41;
pub const FADEIN: i32 = 42;
pub const FADEOUT: i32 = 43;

// Hover cursors.
pub const NORMAL: i32 = 21;
pub const FORWARD: i32 = 22;
pub const LOOK: i32 = 23;
pub const USE: i32 = 24;
pub const WAIT: i32 = 25;

// Cube faces.
pub const FRONT: i32 = 1;
pub const BACK: i32 = 2;
pub const LEFT: i32 = 3;
pub const RIGHT: i32 = 4;
pub const UP: i32 = 5;
pub const DOWN: i32 = 6;

// Audio channels addressed by `scFXPlayEx` / `scFXStop` / `scFXSet`.
pub const CH_STEP: usize = 8;
pub const CH_DIALOGUE: usize = 13;
pub const CH_FX: usize = 14;

// `scLookAt` transitions.
pub const INSTANT: i32 = 1;
pub const SMOOTH: i32 = 2;

// Hint events.
pub const HINTS_INACTIVE: i32 = 0;
pub const HINTS_ACTIVE: i32 = 1;
pub const HINTS_OPEN: i32 = 2;
pub const HINTS_GRAB: i32 = 3;
pub const HINTS_CLOSE: i32 = 4;
pub const HINTS_ITEM: i32 = 5;
pub const HINTS_RESET: i32 = 6;

// Settings keys (`scSet` / `scGet` / `scEnable` / `scDisable`).
pub const K_WIDTH: i32 = 1000;
pub const K_HEIGHT: i32 = 1001;
pub const K_FOV: i32 = 1008;
pub const K_LOG: i32 = 1010;
pub const K_FULLSCREEN: i32 = 1012;
pub const K_RESOURCE: i32 = 1013;
pub const K_RES_PATH: i32 = 1014;
pub const K_RES_FILE: i32 = 1015;
pub const K_CAM_SPEED: i32 = 1016;
pub const K_TXT_SPEED: i32 = 1017;
pub const K_MOV_SPEED: i32 = 1018;
pub const K_SKY_SPEED: i32 = 1019;
pub const K_SHOW_HOTSPOTS: i32 = 1025;
pub const K_SURROUND: i32 = 1026;
pub const K_SHOWMENU: i32 = 1027;
pub const K_SPLASH: i32 = 1028;
pub const K_HINTS: i32 = 1029;
pub const K_SKY: i32 = 1030;
pub const K_NIGHT: i32 = 1031;
pub const K_RAIN: i32 = 1032;
pub const K_FOG: i32 = 1033;
pub const K_GLOW: i32 = 1034;
pub const K_DREAM: i32 = 1035;
pub const K_CANDLE: i32 = 1036;
pub const K_LAMP: i32 = 1037;
pub const K_LANGUAGE: i32 = 1038;
pub const K_DEBUG: i32 = 1040;
pub const K_SKY_COLOR: i32 = 1042;
pub const K_CLOUD_COLOR: i32 = 1043;
pub const K_SCREEN_COLOR: i32 = 1044;

// Quality presets accepted by speed/size settings.
pub const LO: i32 = 101;
pub const MED: i32 = 102;
pub const HI: i32 = 103;

// Ambience presets (`scSetAmbience`).
pub const DAWN_PRESET: i32 = 309;
pub const MORNING_PRESET: i32 = 311;
pub const DUSK_PRESET: i32 = 317;
pub const STORM_PRESET: i32 = 319;
