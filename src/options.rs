//! Port-only options (not in the original engine). Every one is switchable from the setup menu
//! ("port options" pages) and stored in `scream.cfg`; the original behaviour is always one of
//! the choices.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    Windowed,
    Borderless,
    Exclusive,
}

/// How the 3D panorama uses the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NodeView {
    /// Full window width, original black bars top and bottom (Hor+).
    #[default]
    Wide,
    /// The original 4:3 picture.
    Classic,
    /// Whole window, no bars; subtitles are drawn over the picture.
    Full,
}

/// How flat pictures (zooms, menu background, journal) and videos are scaled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlatScale {
    /// Inside the original 1024×768 layout.
    Original,
    /// As large as fits the window between the bars, aspect kept.
    Fit,
    /// Covers the area between the bars, cropping the edges.
    Fill,
    /// Stretched to the area between the bars.
    Stretch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Bilinear,
    Fsr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keys {
    /// The original bindings (Left/A turn left, Up/D turn right, Right/W look up, Down/S look down).
    Original,
    /// Arrows and WASD as expected: left/right turn, up/down look.
    Modern,
}

#[derive(Clone, Debug)]
pub struct PortOptions {
    pub display: Display,
    /// Window or exclusive-mode size; (0, 0) = desktop size.
    pub resolution: (u32, u32),
    pub node_view: NodeView,
    pub zoom_scale: FlatScale,
    pub video_scale: FlatScale,
    pub filter: Filter,
    pub keys: Keys,
    /// Mouse-look sensitivity 1..10 (5 = the original).
    pub mouse_sens: i32,
    pub invert_y: bool,
    /// Gamepad input (see `gamepad.rs` for the mapping).
    pub gamepad: bool,
    /// Hold Tab to see every hotspot of the current view.
    pub hotspot_key: bool,
    /// Left click and Space skip dialogue lines and cutscenes (the original: Escape only).
    pub click_skip: bool,
    pub autosave: bool,
    pub large_subtitles: bool,
    pub subtitle_box: bool,
    /// Hold H to read the last lines of dialogue.
    pub history: bool,
    pub vol_master: i32,
    pub vol_music: i32,
    pub vol_fx: i32,
    pub vol_voice: i32,
}

impl Default for PortOptions {
    fn default() -> Self {
        PortOptions {
            display: Display::Borderless,
            resolution: (0, 0),
            node_view: NodeView::Wide,
            zoom_scale: FlatScale::Original,
            video_scale: FlatScale::Original,
            filter: Filter::Bilinear,
            keys: Keys::Modern,
            mouse_sens: 5,
            invert_y: false,
            gamepad: true,
            hotspot_key: true,
            click_skip: true,
            autosave: true,
            large_subtitles: false,
            subtitle_box: false,
            history: true,
            vol_master: 10,
            vol_music: 10,
            vol_fx: 10,
            vol_voice: 10,
        }
    }
}

/// Rows of the two "port options" pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Display,
    Resolution,
    NodeView,
    ZoomScale,
    VideoScale,
    Filter,
    Keys,
    MouseSens,
    InvertY,
    Gamepad,
    HotspotKey,
    ClickSkip,
    Autosave,
    SubtitleSize,
    SubtitleBox,
    History,
    VolMaster,
    VolMusic,
    VolFx,
    VolVoice,
}

pub const PAGES: [&[Row]; 2] = [
    &[Row::Display, Row::Resolution, Row::NodeView, Row::ZoomScale, Row::VideoScale, Row::Filter, Row::Keys, Row::MouseSens, Row::InvertY, Row::Gamepad],
    &[
        Row::HotspotKey,
        Row::ClickSkip,
        Row::Autosave,
        Row::SubtitleSize,
        Row::SubtitleBox,
        Row::History,
        Row::VolMaster,
        Row::VolMusic,
        Row::VolFx,
        Row::VolVoice,
    ],
];

fn cycle<T: Copy + PartialEq>(all: &[T], cur: T, dir: i32) -> T {
    let i = all.iter().position(|&v| v == cur).unwrap_or(0) as i32;
    let n = all.len() as i32;
    all[((i + dir).rem_euclid(n)) as usize]
}

fn onoff(b: bool) -> &'static str {
    if b {
        "ON"
    } else {
        "OFF"
    }
}

fn scale_name(s: FlatScale) -> &'static str {
    match s {
        FlatScale::Original => "ORIGINAL",
        FlatScale::Fit => "FIT",
        FlatScale::Fill => "FILL",
        FlatScale::Stretch => "STRETCH",
    }
}

const SCALES: [FlatScale; 4] = [FlatScale::Original, FlatScale::Fit, FlatScale::Fill, FlatScale::Stretch];

impl PortOptions {
    pub fn label(row: Row) -> &'static str {
        match row {
            Row::Display => "display",
            Row::Resolution => "resolution",
            Row::NodeView => "view",
            Row::ZoomScale => "close-ups",
            Row::VideoScale => "videos",
            Row::Filter => "texture filter",
            Row::Keys => "keys",
            Row::MouseSens => "mouse speed",
            Row::InvertY => "invert mouse",
            Row::Gamepad => "gamepad",
            Row::HotspotKey => "hotspots on TAB",
            Row::ClickSkip => "click to skip",
            Row::Autosave => "autosave",
            Row::SubtitleSize => "subtitles",
            Row::SubtitleBox => "subtitle backdrop",
            Row::History => "history on H",
            Row::VolMaster => "volume",
            Row::VolMusic => "music volume",
            Row::VolFx => "effects volume",
            Row::VolVoice => "voice volume",
        }
    }

    pub fn value(&self, row: Row) -> String {
        match row {
            Row::Display => match self.display {
                Display::Windowed => "WINDOW",
                Display::Borderless => "FULLSCREEN",
                Display::Exclusive => "EXCLUSIVE",
            }
            .into(),
            Row::Resolution => match self.resolution {
                (0, 0) => "DESKTOP".into(),
                (w, h) => format!("{w}x{h}"),
            },
            Row::NodeView => match self.node_view {
                NodeView::Wide => "WIDE",
                NodeView::Classic => "4:3",
                NodeView::Full => "NO BARS",
            }
            .into(),
            Row::ZoomScale => scale_name(self.zoom_scale).into(),
            Row::VideoScale => scale_name(self.video_scale).into(),
            Row::Filter => match self.filter {
                Filter::Bilinear => "ORIGINAL",
                Filter::Fsr => "FSR 1",
            }
            .into(),
            Row::Keys => match self.keys {
                Keys::Original => "ORIGINAL",
                Keys::Modern => "MODERN",
            }
            .into(),
            Row::MouseSens => self.mouse_sens.to_string(),
            Row::InvertY => onoff(self.invert_y).into(),
            Row::Gamepad => onoff(self.gamepad).into(),
            Row::HotspotKey => onoff(self.hotspot_key).into(),
            Row::ClickSkip => onoff(self.click_skip).into(),
            Row::Autosave => onoff(self.autosave).into(),
            Row::SubtitleSize => if self.large_subtitles { "LARGE" } else { "ORIGINAL" }.into(),
            Row::SubtitleBox => onoff(self.subtitle_box).into(),
            Row::History => onoff(self.history).into(),
            Row::VolMaster => self.vol_master.to_string(),
            Row::VolMusic => self.vol_music.to_string(),
            Row::VolFx => self.vol_fx.to_string(),
            Row::VolVoice => self.vol_voice.to_string(),
        }
    }

    /// Steps a row's value; `resolutions` lists the sizes the monitor offers.
    pub fn step(&mut self, row: Row, dir: i32, resolutions: &[(u32, u32)]) {
        let step_int = |v: &mut i32, lo: i32, hi: i32| *v = (*v - lo + dir).rem_euclid(hi - lo + 1) + lo;
        match row {
            Row::Display => self.display = cycle(&[Display::Windowed, Display::Borderless, Display::Exclusive], self.display, dir),
            Row::Resolution => {
                let mut all = vec![(0, 0)];
                all.extend_from_slice(resolutions);
                self.resolution = cycle(&all, self.resolution, dir);
            }
            Row::NodeView => self.node_view = cycle(&[NodeView::Wide, NodeView::Classic, NodeView::Full], self.node_view, dir),
            Row::ZoomScale => self.zoom_scale = cycle(&SCALES, self.zoom_scale, dir),
            Row::VideoScale => self.video_scale = cycle(&SCALES, self.video_scale, dir),
            Row::Filter => self.filter = cycle(&[Filter::Bilinear, Filter::Fsr], self.filter, dir),
            Row::Keys => self.keys = cycle(&[Keys::Original, Keys::Modern], self.keys, dir),
            Row::MouseSens => step_int(&mut self.mouse_sens, 1, 10),
            Row::InvertY => self.invert_y = !self.invert_y,
            Row::Gamepad => self.gamepad = !self.gamepad,
            Row::HotspotKey => self.hotspot_key = !self.hotspot_key,
            Row::ClickSkip => self.click_skip = !self.click_skip,
            Row::Autosave => self.autosave = !self.autosave,
            Row::SubtitleSize => self.large_subtitles = !self.large_subtitles,
            Row::SubtitleBox => self.subtitle_box = !self.subtitle_box,
            Row::History => self.history = !self.history,
            Row::VolMaster => step_int(&mut self.vol_master, 0, 10),
            Row::VolMusic => step_int(&mut self.vol_music, 0, 10),
            Row::VolFx => step_int(&mut self.vol_fx, 0, 10),
            Row::VolVoice => step_int(&mut self.vol_voice, 0, 10),
        }
    }

    /// Reads one `KEY value` pair from scream.cfg; false when the key is not a port option.
    pub fn apply(&mut self, key: &str, val: &str) -> bool {
        let int = || val.parse::<i32>().unwrap_or(0);
        let on = || val != "0";
        let scale = || match val {
            "FIT" => FlatScale::Fit,
            "FILL" => FlatScale::Fill,
            "STRETCH" => FlatScale::Stretch,
            _ => FlatScale::Original,
        };
        match key {
            "DISPLAY" => {
                self.display = match val {
                    "WINDOW" => Display::Windowed,
                    "EXCLUSIVE" => Display::Exclusive,
                    _ => Display::Borderless,
                }
            }
            "RESOLUTION" => {
                self.resolution = val
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    .unwrap_or((0, 0))
            }
            "VIEW" => {
                self.node_view = match val {
                    "CLASSIC" => NodeView::Classic,
                    "FULL" => NodeView::Full,
                    _ => NodeView::Wide,
                }
            }
            "ZOOM_SCALE" => self.zoom_scale = scale(),
            "VIDEO_SCALE" => self.video_scale = scale(),
            "FILTER" => self.filter = if val == "FSR" { Filter::Fsr } else { Filter::Bilinear },
            "KEYS" => self.keys = if val == "ORIGINAL" { Keys::Original } else { Keys::Modern },
            "MOUSE_SPEED" => self.mouse_sens = int().clamp(1, 10),
            "INVERT_Y" => self.invert_y = on(),
            "GAMEPAD" => self.gamepad = on(),
            "HOTSPOT_KEY" => self.hotspot_key = on(),
            "CLICK_SKIP" => self.click_skip = on(),
            "AUTOSAVE" => self.autosave = on(),
            "LARGE_SUBTITLES" => self.large_subtitles = on(),
            "SUBTITLE_BOX" => self.subtitle_box = on(),
            "HISTORY" => self.history = on(),
            "VOLUME" => self.vol_master = int().clamp(0, 10),
            "VOLUME_MUSIC" => self.vol_music = int().clamp(0, 10),
            "VOLUME_FX" => self.vol_fx = int().clamp(0, 10),
            "VOLUME_VOICE" => self.vol_voice = int().clamp(0, 10),
            _ => return false,
        }
        true
    }

    pub fn to_cfg(&self) -> String {
        let b = |v: bool| v as i32;
        let scale = |s: FlatScale| scale_name(s);
        format!(
            "DISPLAY {}\nRESOLUTION {}x{}\nVIEW {}\nZOOM_SCALE {}\nVIDEO_SCALE {}\nFILTER {}\nKEYS {}\nMOUSE_SPEED {}\n\
             INVERT_Y {}\nGAMEPAD {}\nHOTSPOT_KEY {}\nCLICK_SKIP {}\nAUTOSAVE {}\nLARGE_SUBTITLES {}\nSUBTITLE_BOX {}\nHISTORY {}\n\
             VOLUME {}\nVOLUME_MUSIC {}\nVOLUME_FX {}\nVOLUME_VOICE {}\n",
            match self.display {
                Display::Windowed => "WINDOW",
                Display::Borderless => "FULLSCREEN",
                Display::Exclusive => "EXCLUSIVE",
            },
            self.resolution.0,
            self.resolution.1,
            match self.node_view {
                NodeView::Wide => "WIDE",
                NodeView::Classic => "CLASSIC",
                NodeView::Full => "FULL",
            },
            scale(self.zoom_scale),
            scale(self.video_scale),
            match self.filter {
                Filter::Bilinear => "BILINEAR",
                Filter::Fsr => "FSR",
            },
            match self.keys {
                Keys::Original => "ORIGINAL",
                Keys::Modern => "MODERN",
            },
            self.mouse_sens,
            b(self.invert_y),
            b(self.gamepad),
            b(self.hotspot_key),
            b(self.click_skip),
            b(self.autosave),
            b(self.large_subtitles),
            b(self.subtitle_box),
            b(self.history),
            self.vol_master,
            self.vol_music,
            self.vol_fx,
            self.vol_voice,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfg_roundtrip() {
        let mut o = PortOptions::default();
        o.display = Display::Windowed;
        o.resolution = (1920, 1080);
        o.zoom_scale = FlatScale::Fill;
        o.vol_music = 3;
        let mut p = PortOptions::default();
        for line in o.to_cfg().lines() {
            let (k, v) = line.split_once(' ').unwrap();
            assert!(p.apply(k, v), "{k}");
        }
        assert_eq!(p.resolution, (1920, 1080));
        assert_eq!(p.zoom_scale, FlatScale::Fill);
        assert_eq!(p.vol_music, 3);
        assert_eq!(p.display, Display::Windowed);
    }

    #[test]
    fn stepping_wraps() {
        let mut o = PortOptions::default();
        o.step(Row::VolMaster, 1, &[]);
        assert_eq!(o.vol_master, 0);
        o.step(Row::Resolution, 1, &[(1280, 720)]);
        assert_eq!(o.resolution, (1280, 720));
        o.step(Row::Resolution, 1, &[(1280, 720)]);
        assert_eq!(o.resolution, (0, 0));
    }
}
