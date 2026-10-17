//! Engine settings: defaults (`config_Defaults` 0x40f3c0), the `scream.cfg` text format
//! (`config_Load` 0x40f600 / `config_Save` 0x40fb20) and `scSet`/`scGet`/`scEnable` keys
//! (`core_SetNum` 0x410080, `core_Enable` 0x40fd00).

use std::path::{Path, PathBuf};

use crate::sc;

#[derive(Clone, Debug)]
pub struct Settings {
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    pub quality: i32,
    /// Global animation speed factor (`_DAT_04df8b88`, always 1.0 in the shipped game).
    pub speed: f32,
    /// Gamma boost level 0..3 (F12).
    pub gamma: i32,
    /// Vertical field of view in degrees.
    pub fov: f32,
    pub log: bool,
    pub fullscreen: bool,
    /// Read assets from the `.res` archive (otherwise loose files).
    pub resource: bool,
    pub vsync: bool,
    pub audio_device: i32,
    pub buffer_size: i32,
    pub num_buffers: i32,
    pub res_path: String,
    pub res_file: String,
    pub surround: bool,
    pub cam_speed: i32,
    pub txt_speed: i32,
    pub mov_speed: i32,
    pub sky_speed: i32,
    pub show_hotspots: bool,
    pub showmenu: bool,
    pub hints: bool,
    pub splash: String,
    pub script: String,
    /// Slideshow mode: the camera jumps by 45° instead of panning.
    pub quickmode: bool,
    pub text_glow: bool,
    pub wait_cursor: bool,
    pub language: String,
    /// "Alternate control" (mouse-look with a centred cursor). On by default.
    pub alt_control: bool,
    pub sky_detail: i32,
    pub debug: bool,
    pub fx: Effects,
    pub port: crate::options::PortOptions,
}

/// Scene effects toggled with `scEnable` / `scDisable`.
#[derive(Clone, Debug, Default)]
pub struct Effects {
    pub sky: bool,
    pub night: bool,
    pub rain: bool,
    pub fog: bool,
    pub glow: bool,
    pub dream: bool,
    pub candle: bool,
    pub lamp: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            width: 1024,
            height: 768,
            depth: 32,
            quality: 3,
            speed: 1.0,
            gamma: 0,
            fov: 45.0,
            log: false,
            fullscreen: true,
            resource: true,
            vsync: true,
            audio_device: 0,
            buffer_size: 0x8000,
            num_buffers: 4,
            res_path: ".\\".into(),
            res_file: "game.res".into(),
            surround: true,
            cam_speed: 50,
            txt_speed: 50,
            mov_speed: 25,
            sky_speed: 15,
            show_hotspots: false,
            showmenu: false,
            hints: false,
            splash: "sc_logo.tga".into(),
            script: "scream".into(),
            quickmode: false,
            text_glow: false,
            wait_cursor: true,
            language: "en".into(),
            alt_control: true,
            sky_detail: 2,
            debug: false,
            fx: Effects { sky: true, ..Default::default() },
            port: crate::options::PortOptions::default(),
        }
    }
}

/// LO/MED/HI presets for speeds, as `core_SetNum` maps them.
fn speed_preset(v: i32) -> i32 {
    match v {
        sc::LO => 25,
        sc::MED => 50,
        sc::HI => 75,
        v => v,
    }
}

impl Settings {
    /// Per-user directory the engine uses for `scream.cfg` and save games.
    pub fn user_dir() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("scream-rs")
    }

    pub fn load(path: &Path) -> Settings {
        let mut s = Settings::default();
        if let Ok(text) = std::fs::read_to_string(path) {
            s.apply_cfg(&text);
        }
        s
    }

    /// Token stream parser equivalent to the `sscanf("%s")` loop in `config_Load`.
    pub fn apply_cfg(&mut self, text: &str) {
        let mut tok = text.split_whitespace();
        while let Some(key) = tok.next() {
            let mut int = || tok.next().and_then(|v| v.parse::<i32>().ok());
            match key {
                "SCRIPT" => self.script = tok.next().unwrap_or_default().to_lowercase(),
                "RESOURCE" => self.res_file = tok.next().unwrap_or_default().into(),
                "PATH" => self.res_path = tok.next().unwrap_or_default().into(),
                "WIDTH" => self.width = int().unwrap_or(self.width),
                "HEIGHT" => self.height = int().unwrap_or(self.height),
                "DEPTH" => self.depth = int().unwrap_or(self.depth),
                "QUALITY" => self.quality = int().unwrap_or(self.quality),
                "FULLSCREEN" => self.fullscreen = int().map_or(self.fullscreen, |v| v != 0),
                "SURROUND" => self.surround = int().map_or(self.surround, |v| v != 0),
                "LOG" => self.log = int().map_or(self.log, |v| v != 0),
                "VSYNC" => self.vsync = int().map_or(self.vsync, |v| v != 0),
                "SKY_DETAIL" => self.sky_detail = int().unwrap_or(self.sky_detail),
                "AUDIO_DEVICE" => self.audio_device = int().unwrap_or(self.audio_device),
                "BUFFER_SIZE" => self.buffer_size = int().unwrap_or(self.buffer_size),
                "NUM_BUFFERS" => self.num_buffers = int().unwrap_or(self.num_buffers),
                "LANGUAGE" => self.language = tok.next().unwrap_or("en").into(),
                "DEBUG" => self.debug = int().map_or(self.debug, |v| v != 0),
                // Port extensions.
                "CAM_SPEED" => self.cam_speed = int().unwrap_or(self.cam_speed),
                "TXT_SPEED" => self.txt_speed = int().unwrap_or(self.txt_speed),
                "GAMMA" => self.gamma = int().unwrap_or(self.gamma).clamp(0, 3),
                "HINTS" => self.hints = int().map_or(self.hints, |v| v != 0),
                "ALT_CONTROL" => self.alt_control = int().map_or(self.alt_control, |v| v != 0),
                "QUICKMODE" => self.quickmode = int().map_or(self.quickmode, |v| v != 0),
                key => {
                    if let Some(v) = tok.next() {
                        self.port.apply(key, v);
                    }
                }
            }
        }
    }

    pub fn to_cfg(&self) -> String {
        let b = |v: bool| v as i32;
        format!(
            "SCRIPT {}\nPATH {}\nWIDTH {}\nHEIGHT {}\nDEPTH {}\nQUALITY {}\nFULLSCREEN {}\nSURROUND {}\n\
             LOG {}\nVSYNC {}\nSKY_DETAIL {}\nAUDIO_DEVICE {}\nBUFFER_SIZE {}\nNUM_BUFFERS {}\nLANGUAGE {}\n\
             CAM_SPEED {}\nTXT_SPEED {}\nGAMMA {}\nHINTS {}\nALT_CONTROL {}\nQUICKMODE {}\n",
            self.script.to_uppercase(),
            self.res_path,
            self.width,
            self.height,
            self.depth,
            self.quality,
            b(self.fullscreen),
            b(self.surround),
            b(self.log),
            b(self.vsync),
            self.sky_detail,
            self.audio_device,
            self.buffer_size,
            self.num_buffers,
            self.language,
            self.cam_speed,
            self.txt_speed,
            self.gamma,
            b(self.hints),
            b(self.alt_control),
            b(self.quickmode),
        ) + &self.port.to_cfg()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_cfg())
    }

    /// `scSet(key, number)`.
    pub fn set_num(&mut self, key: i32, v: i32) {
        match key {
            sc::K_WIDTH => self.width = v,
            sc::K_HEIGHT => self.height = v,
            sc::K_FOV => self.fov = v as f32,
            sc::K_CAM_SPEED => self.cam_speed = speed_preset(v),
            sc::K_TXT_SPEED => self.txt_speed = speed_preset(v),
            sc::K_MOV_SPEED => self.mov_speed = speed_preset(v),
            sc::K_SKY_SPEED => self.sky_speed = speed_preset(v),
            1007 => {
                self.quality = match v {
                    sc::LO => 0,
                    sc::MED => 1,
                    sc::HI => 2,
                    v => v,
                }
            }
            _ => {} // texture/icon/cursor sizes are fixed in the port
        }
    }

    /// `scSet(key, string)` — only four keys take strings.
    pub fn set_str(&mut self, key: i32, v: &str) {
        match key {
            sc::K_RES_PATH => self.res_path = v.into(),
            sc::K_RES_FILE => self.res_file = v.into(),
            sc::K_LANGUAGE => self.language = v.into(),
            sc::K_SPLASH => self.splash = v.into(),
            _ => {}
        }
    }

    /// `scGet(key)` numeric part (`core_GetNum` 0x412900): only these three are readable.
    pub fn get_num(&self, key: i32) -> i32 {
        match key {
            sc::K_HINTS => self.hints as i32,
            sc::K_MOV_SPEED => self.mov_speed,
            sc::K_TXT_SPEED => self.txt_speed,
            _ => 0,
        }
    }

    /// `scEnable` / `scDisable` for setting keys. Returns false for keys handled elsewhere.
    pub fn set_flag(&mut self, key: i32, on: bool) -> bool {
        match key {
            sc::K_FULLSCREEN => self.fullscreen = on,
            sc::K_LOG => self.log = on,
            sc::WAIT => self.wait_cursor = on,
            sc::K_RESOURCE => self.resource = on,
            sc::K_SHOW_HOTSPOTS => self.show_hotspots = on,
            sc::K_SURROUND => self.surround = on,
            sc::K_HINTS => self.hints = on,
            sc::K_SHOWMENU => self.showmenu = on,
            sc::K_SKY => self.fx.sky = on,
            sc::K_NIGHT => self.fx.night = on,
            sc::K_RAIN => self.fx.rain = on,
            sc::K_FOG => self.fx.fog = on,
            sc::K_GLOW => self.fx.glow = on,
            sc::K_DREAM => self.fx.dream = on,
            sc::K_CANDLE => self.fx.candle = on,
            sc::K_LAMP => self.fx.lamp = on,
            sc::K_DEBUG => self.debug = on,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shipped_cfg() {
        let mut s = Settings::default();
        s.apply_cfg("SCRIPT SCRATCHES\nPATH .\\\\\nWIDTH 1024\nHEIGHT 768\nSURROUND 0\nLANGUAGE en\n");
        assert_eq!(s.script, "scratches");
        assert_eq!(s.width, 1024);
        assert!(!s.surround);
        let mut t = Settings::default();
        t.apply_cfg(&s.to_cfg());
        assert_eq!(t.script, "scratches");
        assert!(!t.surround);
    }

    #[test]
    fn speed_presets() {
        let mut s = Settings::default();
        s.set_num(sc::K_CAM_SPEED, sc::HI);
        assert_eq!(s.cam_speed, 75);
        s.set_num(sc::K_MOV_SPEED, 2);
        assert_eq!(s.get_num(sc::K_MOV_SPEED), 2);
    }
}
