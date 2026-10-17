//! Text state: the feedback/subtitle line in the bottom bar, the clickable text list (dialogue
//! choices and menu confirmations) and the journal.

use std::time::Instant;

/// Bottom-bar feedback text (`core_Feedback` 0x40e240, drawn by `render_Feedback` 0x40ad10).
#[derive(Debug)]
pub struct Feedback {
    /// Lines bottom-up: `lines[0]` is drawn lowest and holds the end of the text.
    pub lines: Vec<String>,
    pub start: Instant,
    /// Seconds the text stays fully visible: `100 / txt_speed + len / 16`.
    pub duration: f32,
    /// 1 = invisible, 0 = opaque (the original's `DAT_0048bbcc`).
    pub fade: f32,
    /// Glow pass intensity (`DAT_0048bbd0`) for SC_GLOW-style text.
    pub glow: f32,
    fresh: bool,
}

impl Default for Feedback {
    fn default() -> Self {
        Feedback { lines: Vec::new(), start: Instant::now(), duration: 0.0, fade: 1.0, glow: 0.0, fresh: false }
    }
}

impl Feedback {
    /// Splits `text` into 1–4 lines by length and starts showing it.
    pub fn set(&mut self, text: &str, txt_speed: i32) {
        let bytes: Vec<u8> = text.bytes().collect();
        let len = bytes.len();
        if len > 399 {
            return;
        }
        let (per_line, last) = match len {
            0..=100 => (100, 0),
            101..=200 => (len / 2, 1),
            201..=300 => (len / 3, 2),
            _ => (len / 4, 3),
        };
        // Lines are produced top-down into slots last, last-1, ...; a break happens at the
        // first space once a line has at least `per_line - 1` characters. '|' is a
        // non-breaking space.
        let mut slots: Vec<String> = vec![String::new(); last + 1];
        let mut slot = last as isize;
        let mut seg_start = 0;
        let mut run = 0;
        for i in 0..len {
            let c = bytes[i];
            if c == b' ' && run >= per_line as isize - 1 && slot > 0 {
                slots[slot as usize] = clean(&bytes[seg_start..i]);
                slot -= 1;
                seg_start = i + 1;
                run = 0;
            }
            run += 1;
        }
        if slot >= 0 {
            slots[slot as usize] = clean(&bytes[seg_start..]);
        }
        self.lines = if len == 0 { Vec::new() } else { slots };
        let len_for_time = if len < 17 { 32 } else { len };
        self.duration = (100 / txt_speed.max(1)) as f32 + (len_for_time / 16) as f32;
        self.start = Instant::now();
        self.fresh = true;
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Advances the fade by one frame (`speed` = 60 fps-normalised frame factor). Returns the
    /// lines to draw with their baseline height above the window bottom (virtual px) and alpha.
    pub fn update(&mut self, txt_speed: i32, glow_enabled: bool, speed: f32) -> Vec<(String, f32, f32, f32)> {
        if self.fresh {
            self.fresh = false;
            self.fade = 1.0;
            if glow_enabled {
                self.glow = 0.5;
            }
        }
        if self.lines.is_empty() {
            return Vec::new();
        }
        let fade_in = txt_speed as f32 * 0.001;
        let fade_out = txt_speed as f32 * 0.0001;
        let elapsed = self.start.elapsed().as_secs_f32();
        let (first, count, base) = match self.lines.len() {
            1 => (0, 1, 24.0),
            2 => (0, 2, 16.0),
            3 => (0, 3, 8.0),
            _ if elapsed < self.duration * 0.5 => (1, 4, -8.0),
            _ => (0, 3, 8.0),
        };
        let mut out = Vec::new();
        for i in first..count {
            if elapsed <= self.duration {
                if self.fade > 0.0 {
                    self.fade -= fade_in * speed;
                }
            } else {
                self.fade += fade_out * speed;
            }
            let mut glow = 0.0;
            if glow_enabled {
                let g = fade_in * speed;
                if elapsed <= 0.25 {
                    if self.glow > 0.0 {
                        self.glow += g;
                    }
                } else {
                    self.glow -= g;
                }
                self.glow = self.glow.min(0.5);
                glow = self.glow * 2.0;
            }
            let y = base + 16.0 * i as f32;
            out.push((self.lines[i].clone(), y, (1.0 - self.fade).clamp(0.0, 1.0), glow.max(0.0)));
        }
        if self.fade >= 1.0 && elapsed > self.duration {
            self.fade = 1.0;
            self.lines.clear();
        }
        out
    }
}

fn clean(b: &[u8]) -> String {
    b.iter().map(|&c| if c == b'|' { ' ' } else { c as char }).collect()
}

/// Text list shown above the bottom of the screen (`textlist_Add` 0x40e5e0): choices of a
/// dialogue (`scChoiceGo`) or the menu's question with "yes"/"no".
#[derive(Debug, Default)]
pub struct TextList {
    pub items: Vec<String>,
    pub hover: Option<usize>,
}

impl TextList {
    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn add(&mut self, s: &str) {
        self.items.push(s.to_owned());
    }
}

/// Journal: wrapped lines (at most 40 chars, 1000 lines) and one "seen" flag per entry id so an
/// entry is added once (`journal_Add` 0x4189b0, `journal_AddText` 0x40e470).
#[derive(Debug)]
pub struct Journal {
    pub lines: Vec<String>,
    pub first: usize,
    pub seen: Vec<bool>,
    pub image: String,
    pub font: String,
    pub sound_open: String,
    pub sound_turn: String,
    pub sound_close: String,
}

pub const JOURNAL_SPREAD: usize = 76;
const JOURNAL_WRAP: usize = 30;

impl Default for Journal {
    fn default() -> Self {
        Journal {
            lines: Vec::new(),
            first: 0,
            seen: vec![false; 1000],
            image: String::new(),
            font: String::new(),
            sound_open: String::new(),
            sound_turn: String::new(),
            sound_close: String::new(),
        }
    }
}

impl Journal {
    pub fn clear(&mut self) {
        self.lines.clear();
        self.first = 0;
        self.seen.iter_mut().for_each(|s| *s = false);
    }

    /// Adds entry `id` once; entries are separated by an empty line.
    pub fn add(&mut self, id: usize, text: &str) {
        if self.seen.get(id).copied().unwrap_or(true) {
            return;
        }
        self.seen[id] = true;
        if !self.lines.is_empty() {
            self.lines.push(String::new());
        }
        let bytes = text.as_bytes();
        let mut start = 0;
        let mut run = 0;
        for i in 0..bytes.len() {
            if bytes[i] == b' ' && run > JOURNAL_WRAP {
                self.lines.push(clean(&bytes[start..i]));
                start = i + 1;
                run = 0;
            }
            run += 1;
        }
        self.lines.push(clean(&bytes[start..]));
        self.lines.truncate(1000);
    }

    /// `journal_Scroll`: moves by a spread; false when already at either end.
    pub fn scroll(&mut self, delta: isize) -> bool {
        let next = self.first as isize + delta;
        if next < 0 {
            self.first = 0;
            false
        } else if next as usize > self.lines.len() {
            self.first = (self.lines.len() / JOURNAL_SPREAD) * JOURNAL_SPREAD;
            false
        } else {
            self.first = next as usize;
            true
        }
    }

    /// Lines of the visible spread with their virtual position (x, y).
    pub fn visible(&self) -> Vec<(f32, f32, &str)> {
        let mut out = Vec::new();
        let (mut x, mut y) = (32.0, 100.0);
        for i in 0..JOURNAL_SPREAD {
            if let Some(l) = self.lines.get(self.first + i) {
                out.push((x, y, l.as_str()));
            }
            y += 16.0;
            if y >= 704.0 {
                y = 100.0;
                x += 480.0;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_splits_long_text_bottom_up() {
        let mut f = Feedback::default();
        let text = "aaaa ".repeat(30); // 150 chars -> 2 lines
        f.set(text.trim_end(), 50);
        assert_eq!(f.lines.len(), 2);
        assert!(f.lines[1].len() >= 74, "{:?}", f.lines);
        assert_eq!(f.duration, 2.0 + 9.0);
        f.set("short|text", 50);
        assert_eq!(f.lines, vec!["short text".to_string()]);
        assert_eq!(f.duration, 2.0 + 2.0);
    }

    #[test]
    fn journal_wraps_and_dedupes() {
        let mut j = Journal::default();
        j.add(3, "one two three four five six seven eight nine ten");
        j.add(3, "again");
        j.add(4, "x");
        assert_eq!(j.lines.len(), 4);
        assert_eq!(j.lines[2], "");
        assert!(j.lines[0].len() > 30);
        assert!(!j.scroll(-76));
        assert!(!j.scroll(76));
    }
}
