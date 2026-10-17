//! Streaming audio channels, a software replacement for the engine's OpenAL layer
//! (scream.exe 0x401040–0x402270).
//!
//! The original owns 17 sources (`g_channels[1..17]`). Each holds one Ogg/Vorbis file decoded
//! from memory, a gain and a target gain, fade-in/fade-out step counts and a loop flag. A
//! background thread calls `snd_UpdateAll` in a `Sleep(1)` loop; every update moves a fading gain
//! by `0.0025 / steps`, so a fade of N steps takes 400·N updates. We run updates every 2.5 ms of
//! output time, which makes one step one second — the scale the engine itself assumes when it
//! compares fade steps with remaining track time.
//!
//! Sources are listener-relative with rolloff 0: channel 2 sits front-left, channel 3
//! front-right (the `-RL` / `-RR` halves of surround tracks), everything else is centred.

use std::io::Cursor;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use lewton::inside_ogg::OggStreamReader;

pub const CHANNELS: usize = 18;
const GAIN_STEP: f32 = 0.0025;
const TICK_SECONDS: f32 = 0.0025;

#[derive(Clone, Debug, Default)]
pub struct ChannelInfo {
    pub file: String,
    pub loaded: bool,
    pub playing: bool,
    pub gain: f32,
    pub target_gain: f32,
    pub fade_in: i32,
    pub fade_out: i32,
    pub looped: bool,
}

struct Stream {
    data: Arc<Vec<u8>>,
    reader: OggStreamReader<Cursor<ArcBytes>>,
    rate: u32,
    channels: usize,
    /// Decoded interleaved samples not consumed yet.
    pcm: Vec<f32>,
    pcm_pos: usize,
    /// Fractional frame position for resampling, relative to `pcm_pos`.
    frac: f64,
    eof: bool,
}

#[derive(Clone)]
struct ArcBytes(Arc<Vec<u8>>);

impl AsRef<[u8]> for ArcBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Stream {
    fn open(data: Arc<Vec<u8>>) -> Option<Stream> {
        let reader = OggStreamReader::new(Cursor::new(ArcBytes(data.clone()))).ok()?;
        let rate = reader.ident_hdr.audio_sample_rate;
        let channels = reader.ident_hdr.audio_channels as usize;
        Some(Stream { data, reader, rate, channels, pcm: Vec::new(), pcm_pos: 0, frac: 0.0, eof: false })
    }

    fn rewind(&mut self) -> bool {
        match Stream::open(self.data.clone()) {
            Some(s) => {
                *self = s;
                true
            }
            None => false,
        }
    }

    /// Ensures at least two frames are buffered (for interpolation); false at end of stream.
    fn fill(&mut self) -> bool {
        while (self.pcm.len() - self.pcm_pos) / self.channels < 2 && !self.eof {
            match self.reader.read_dec_packet_itl() {
                Ok(Some(samples)) => {
                    self.pcm.drain(..self.pcm_pos);
                    self.pcm_pos = 0;
                    self.pcm.extend(samples.iter().map(|&s| s as f32 / 32768.0));
                }
                _ => self.eof = true,
            }
        }
        self.pcm.len() - self.pcm_pos >= self.channels
    }

    /// Next output frame as (left, right), resampled to `out_rate`.
    fn next(&mut self, out_rate: u32) -> Option<(f32, f32)> {
        if !self.fill() {
            return None;
        }
        let c = self.channels;
        let a = &self.pcm[self.pcm_pos..];
        let have_next = a.len() >= 2 * c;
        let t = self.frac as f32;
        let sample = |ch: usize| {
            let s0 = a[ch.min(c - 1)];
            if have_next {
                let s1 = a[c + ch.min(c - 1)];
                s0 + (s1 - s0) * t
            } else {
                s0
            }
        };
        let frame = (sample(0), sample(1));
        self.frac += self.rate as f64 / out_rate as f64;
        while self.frac >= 1.0 {
            self.frac -= 1.0;
            self.pcm_pos += c;
            if self.pcm.len() - self.pcm_pos < c && !self.fill() {
                break;
            }
        }
        Some(frame)
    }
}

struct Channel {
    info: ChannelInfo,
    stream: Option<Stream>,
    duration: f32,
    /// Seconds played since the last (re)start.
    time: f64,
    pan: f32,
}

impl Channel {
    fn new() -> Channel {
        Channel {
            info: ChannelInfo { gain: 1.0, target_gain: 1.0, ..Default::default() },
            stream: None,
            duration: 0.0,
            time: 0.0,
            pan: 0.0,
        }
    }

    /// One `snd_UpdateChannel` gain step.
    fn tick(&mut self) {
        if !self.info.playing {
            return;
        }
        if self.info.fade_out != 0 {
            let step = GAIN_STEP / self.info.fade_out as f32;
            self.info.gain = if self.info.gain < step { 0.0 } else { self.info.gain - step };
        }
        if self.info.fade_in != 0 {
            let step = GAIN_STEP / self.info.fade_in as f32;
            self.info.gain = if self.info.target_gain < self.info.gain {
                self.info.target_gain
            } else {
                self.info.gain + step
            };
        }
    }
}

struct Mixer {
    channels: Vec<Channel>,
    out_rate: u32,
    tick_frames: u32,
    tick_counter: u32,
    master: f32,
    /// Per-channel volume from the options (music / effects / voice groups).
    volumes: [f32; CHANNELS],
}

impl Mixer {
    fn render(&mut self, out: &mut [f32], out_channels: usize) {
        for frame in out.chunks_mut(out_channels) {
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for (idx, ch) in self.channels.iter_mut().enumerate().skip(1) {
                let vol = self.volumes[idx];
                if !ch.info.playing {
                    continue;
                }
                let Some(stream) = ch.stream.as_mut() else { continue };
                let mono = stream.channels == 1;
                let next = match stream.next(self.out_rate) {
                    Some(f) => Some(f),
                    None if ch.info.looped && stream.rewind() => {
                        ch.time = 0.0;
                        stream.next(self.out_rate)
                    }
                    None => None,
                };
                let Some((a, b)) = next else {
                    // "Auto stopping channel": the stream ran out and does not loop.
                    ch.info.playing = false;
                    continue;
                };
                ch.time += 1.0 / self.out_rate as f64;
                let g = ch.info.gain.clamp(0.0, 1.0) * vol;
                if mono && ch.pan != 0.0 {
                    let theta = (ch.pan + 1.0) * std::f32::consts::FRAC_PI_4;
                    l += a * g * theta.cos();
                    r += a * g * theta.sin();
                } else {
                    l += a * g;
                    r += b * g;
                }
            }
            l = (l * self.master).clamp(-1.0, 1.0);
            r = (r * self.master).clamp(-1.0, 1.0);
            match frame.len() {
                1 => frame[0] = (l + r) * 0.5,
                _ => {
                    frame[0] = l;
                    frame[1] = r;
                    for s in &mut frame[2..] {
                        *s = 0.0;
                    }
                }
            }
            self.tick_counter += 1;
            if self.tick_counter >= self.tick_frames {
                self.tick_counter = 0;
                for ch in &mut self.channels {
                    ch.tick();
                }
            }
        }
    }
}

/// Audio output plus the 17 engine channels. All methods lock the mixer briefly, so they are
/// cheap to call from the game thread.
pub struct Audio {
    mixer: Arc<Mutex<Mixer>>,
    _stream: Option<cpal::Stream>,
}

impl Audio {
    pub fn new() -> Audio {
        let mixer = Arc::new(Mutex::new(Mixer {
            channels: (0..CHANNELS).map(|_| Channel::new()).collect(),
            out_rate: 44100,
            tick_frames: (44100.0 * TICK_SECONDS) as u32,
            tick_counter: 0,
            master: 1.0,
            volumes: [1.0; CHANNELS],
        }));
        let stream = match Self::open_output(mixer.clone()) {
            Ok(s) => Some(s),
            Err(e) => {
                log::warn!("audio disabled: {e}");
                None
            }
        };
        Audio { mixer, _stream: stream }
    }

    fn open_output(mixer: Arc<Mutex<Mixer>>) -> anyhow::Result<cpal::Stream> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| anyhow::anyhow!("no output device"))?;
        let supported = device.default_output_config()?;
        let config: cpal::StreamConfig = supported.config();
        let out_channels = config.channels as usize;
        {
            let mut m = mixer.lock().unwrap();
            m.out_rate = config.sample_rate.0;
            m.tick_frames = ((config.sample_rate.0 as f32 * TICK_SECONDS) as u32).max(1);
        }
        let err = |e| log::error!("audio stream error: {e}");
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _| mixer.lock().unwrap().render(data, out_channels),
                err,
                None,
            )?,
            cpal::SampleFormat::I16 => {
                let mut buf = Vec::new();
                device.build_output_stream(
                    &config,
                    move |data: &mut [i16], _| {
                        buf.resize(data.len(), 0.0);
                        mixer.lock().unwrap().render(&mut buf, out_channels);
                        for (d, s) in data.iter_mut().zip(&buf) {
                            *d = (s * 32767.0) as i16;
                        }
                    },
                    err,
                    None,
                )?
            }
            f => anyhow::bail!("unsupported sample format {f:?}"),
        };
        stream.play()?;
        Ok(stream)
    }

    fn with<R>(&self, ch: usize, f: impl FnOnce(&mut Channel) -> R) -> Option<R> {
        let mut m = self.mixer.lock().unwrap();
        m.channels.get_mut(ch).filter(|_| ch > 0).map(f)
    }

    /// `snd_Load` (0x401640): decodes the header and prepares the channel without starting it.
    /// Does nothing if the channel is already loaded or the file is empty / missing.
    pub fn load(&self, ch: usize, file: &str, data: Option<Vec<u8>>) {
        if file.is_empty() || self.is_loaded(ch) {
            return;
        }
        let Some(data) = data else {
            log::warn!("Load: file {file} not loaded");
            return;
        };
        let duration = ogg_duration(&data);
        let Some(stream) = Stream::open(Arc::new(data)) else {
            log::warn!("Load: {file} is not a valid Ogg/Vorbis file");
            return;
        };
        self.with(ch, |c| {
            c.stream = Some(stream);
            c.duration = duration;
            c.time = 0.0;
            c.pan = match ch {
                2 => -0.7,
                3 => 0.7,
                _ => 0.0,
            };
            c.info.file = file.to_owned();
            c.info.fade_in = 0;
            c.info.fade_out = 0;
            c.info.looped = false;
            c.info.gain = c.info.target_gain;
            c.info.loaded = true;
            c.info.playing = false;
        });
    }

    /// `snd_Play` (0x4014e0).
    pub fn play(&self, ch: usize, looped: bool) {
        self.with(ch, |c| {
            c.info.looped = looped;
            if c.info.loaded && !c.info.playing {
                c.info.playing = true;
            }
        });
    }

    /// `snd_Stop` (0x401560). The stream stays loaded but restarts from the beginning.
    pub fn stop(&self, ch: usize) {
        self.with(ch, |c| {
            c.info.playing = false;
            if let Some(s) = c.stream.as_mut() {
                s.rewind();
            }
            c.time = 0.0;
        });
    }

    /// `snd_Unload` (0x4019b0).
    pub fn unload(&self, ch: usize) {
        self.with(ch, |c| {
            c.stream = None;
            c.info.loaded = false;
            c.info.playing = false;
            c.info.file.clear();
        });
    }

    /// Stop + unload when loaded (`core_FXStop`).
    pub fn release(&self, ch: usize) {
        if self.is_loaded(ch) {
            self.stop(ch);
            self.unload(ch);
        }
    }

    pub fn is_loaded(&self, ch: usize) -> bool {
        self.with(ch, |c| c.info.loaded).unwrap_or(false)
    }

    pub fn is_playing(&self, ch: usize) -> bool {
        self.with(ch, |c| c.info.playing).unwrap_or(false)
    }

    /// `snd_SetGain` (0x401a80): sets both the gain and the fade target.
    pub fn set_gain(&self, ch: usize, gain: f32) {
        self.with(ch, |c| {
            c.info.gain = gain;
            c.info.target_gain = gain;
        });
    }

    /// `snd_FadeOut` (0x401b00).
    pub fn fade_out(&self, ch: usize, steps: i32) {
        self.with(ch, |c| {
            if c.info.fade_in == 0 {
                c.info.gain = c.info.target_gain;
            }
            c.info.fade_out = steps;
            c.info.fade_in = 0;
        });
    }

    /// `snd_FadeIn` (0x401b50).
    pub fn fade_in(&self, ch: usize, steps: i32) {
        self.with(ch, |c| {
            c.info.fade_in = steps;
            c.info.gain = 0.0;
            c.info.fade_out = 0;
        });
    }

    /// `snd_GetTime` (0x4015f0) in whole seconds: 0 = remaining, 1 = elapsed, 2 = total.
    pub fn time(&self, ch: usize, which: i32) -> i32 {
        self.with(ch, |c| match which {
            0 => (c.duration - c.time as f32).round() as i32,
            1 => c.time.round() as i32,
            _ => c.duration.round() as i32,
        })
        .unwrap_or(0)
    }

    pub fn info(&self, ch: usize) -> ChannelInfo {
        self.with(ch, |c| c.info.clone()).unwrap_or_default()
    }

    pub fn set_master(&self, v: f32) {
        self.mixer.lock().unwrap().master = v;
    }

    pub fn set_volumes(&self, master: f32, per_channel: [f32; CHANNELS]) {
        let mut m = self.mixer.lock().unwrap();
        m.master = master;
        m.volumes = per_channel;
    }
}

impl Default for Audio {
    fn default() -> Self {
        Audio::new()
    }
}

/// Duration in seconds from the granule position of the last Ogg page (what `ov_time_total`
/// reports for single-stream files).
pub fn ogg_duration(data: &[u8]) -> f32 {
    let rate = (|| {
        // Identification header: "\x01vorbis", version u32, channels u8, rate u32.
        let p = data.windows(7).position(|w| w == b"\x01vorbis")?;
        let r = data.get(p + 12..p + 16)?;
        Some(u32::from_le_bytes(r.try_into().ok()?))
    })();
    let Some(rate) = rate.filter(|&r| r > 0) else { return 0.0 };
    let mut pos = data.len().saturating_sub(4);
    while pos > 0 {
        if &data[pos..pos + 4] == b"OggS" && pos + 14 <= data.len() {
            let gp = i64::from_le_bytes(data[pos + 6..pos + 14].try_into().unwrap());
            if gp > 0 {
                return gp as f32 / rate as f32;
            }
        }
        pos -= 1;
    }
    0.0
}
