//! Ogg/Theora cutscenes (`video_Open` 0x41a4c0, `video_Decode` 0x41a390).
//!
//! The original decodes in the frame loop and shows a new frame whenever one frame period has
//! passed. Here a worker thread decodes ahead into a small queue (the Theora header parser
//! recurses deeply, so it gets a large stack) and the game thread picks the frame whose
//! timestamp has been reached. The clips carry no audio; their sound comes from the script.

use std::io::Cursor;
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{anyhow, Result};
use ogg::reading::PacketReader;
use theora_rs::{HeaderParser, OggPacket};

use crate::gfx::Image;

pub struct Frame {
    pub index: u64,
    pub image: Image,
}

pub struct Video {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    rx: Receiver<Frame>,
    stop: Arc<AtomicBool>,
    start: Option<Instant>,
    pending: Option<Frame>,
    finished: bool,
}

impl Video {
    pub fn open(data: Vec<u8>) -> Result<Video> {
        let (tx, rx) = sync_channel::<Frame>(4);
        let (info_tx, info_rx) = sync_channel::<Result<(u32, u32, f64), String>>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        std::thread::Builder::new()
            .name("video".into())
            .stack_size(32 << 20)
            .spawn(move || {
                if let Err(e) = decode(data, tx, &info_tx, &stop2) {
                    let _ = info_tx.try_send(Err(e.to_string()));
                }
            })?;
        let (width, height, fps) = info_rx.recv().map_err(|_| anyhow!("video thread died"))?.map_err(|e| anyhow!(e))?;
        log::info!("video {width}x{height} @ {fps:.2} fps");
        Ok(Video { width, height, fps, rx, stop, start: None, pending: None, finished: false })
    }

    /// Newest frame due now (if it changed), or `None`. `finished()` turns true after the last
    /// frame was shown.
    pub fn poll(&mut self) -> Option<Image> {
        let start = *self.start.get_or_insert_with(Instant::now);
        let due = (start.elapsed().as_secs_f64() * self.fps) as u64;
        let mut shown = None;
        loop {
            let frame = match self.pending.take() {
                Some(f) => f,
                None => match self.rx.try_recv() {
                    Ok(f) => f,
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.finished = true;
                        break;
                    }
                },
            };
            if frame.index > due {
                self.pending = Some(frame);
                break;
            }
            shown = Some(frame.image);
        }
        shown
    }

    pub fn finished(&self) -> bool {
        self.finished && self.pending.is_none()
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn decode(
    data: Vec<u8>,
    tx: std::sync::mpsc::SyncSender<Frame>,
    info_tx: &std::sync::mpsc::SyncSender<Result<(u32, u32, f64), String>>,
    stop: &AtomicBool,
) -> Result<()> {
    let mut reader = PacketReader::new(Cursor::new(data));
    let mut parser = HeaderParser::new();
    let mut serial = None;
    let mut packetno = 0i64;
    let mut decoder = None;
    let mut index = 0u64;
    let mut info = None;
    while !stop.load(Ordering::Relaxed) {
        let Some(pkt) = reader.read_packet()? else { break };
        let bos = pkt.first_in_stream();
        if serial.is_none() {
            if bos && pkt.data.len() > 7 && &pkt.data[1..7] == b"theora" {
                serial = Some(pkt.stream_serial());
            } else {
                continue;
            }
        }
        if Some(pkt.stream_serial()) != serial {
            continue;
        }
        let op = OggPacket {
            b_o_s: bos,
            e_o_s: pkt.last_in_stream(),
            granulepos: pkt.absgp_page() as i64,
            packetno,
            packet: pkt.data,
        };
        packetno += 1;
        if decoder.is_none() {
            parser.push(&op).map_err(|e| anyhow!("theora header: {e:?}"))?;
            if parser.is_ready() {
                let i = &parser.info;
                let fps = if i.fps_numerator > 0 && i.fps_denominator > 0 {
                    i.fps_numerator as f64 / i.fps_denominator as f64
                } else {
                    25.0
                };
                info = Some((i.pic_width.max(1), i.pic_height.max(1), i.pic_x, i.pic_y));
                let _ = info_tx.try_send(Ok((i.pic_width, i.pic_height, fps)));
                decoder = Some(parser.decoder().map_err(|e| anyhow!("theora decoder: {e:?}"))?);
            }
            continue;
        }
        let dec = decoder.as_mut().unwrap();
        if dec.packetin(&op).is_err() {
            continue;
        }
        let Ok(yuv) = dec.ycbcr_out() else { continue };
        let (w, h, px, py) = info.unwrap();
        let image = to_rgba(&yuv, w, h, px, py);
        if tx.send(Frame { index, image }).is_err() {
            break;
        }
        index += 1;
    }
    if decoder.is_none() {
        return Err(anyhow!("no Theora stream"));
    }
    Ok(())
}

/// BT.601 limited-range YUV 4:2:0 → RGBA, as the engine's MMX converter (0x41a950) does.
fn to_rgba(yuv: &theora_rs::YCbCrBuffer, w: u32, h: u32, px: u32, py: u32) -> Image {
    let plane = |p: &theora_rs::ImgPlane, x: i32, y: i32| -> f32 {
        let idx = p.data_offset as isize + y as isize * p.stride as isize + x as isize;
        p.data.get(idx.max(0) as usize).copied().unwrap_or(128) as f32
    };
    let (yp, up, vp) = (&yuv[0], &yuv[1], &yuv[2]);
    let sx = (yp.width / up.width.max(1)).max(1);
    let sy = (yp.height / up.height.max(1)).max(1);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let (fx, fy) = (x + px as i32, y + py as i32);
            let c = (plane(yp, fx, fy) - 16.0) * 1.164;
            let d = plane(up, fx / sx, fy / sy) - 128.0;
            let e = plane(vp, fx / sx, fy / sy) - 128.0;
            let i = ((y as u32 * w + x as u32) * 4) as usize;
            rgba[i] = (c + 1.596 * e).clamp(0.0, 255.0) as u8;
            rgba[i + 1] = (c - 0.392 * d - 0.813 * e).clamp(0.0, 255.0) as u8;
            rgba[i + 2] = (c + 2.017 * d).clamp(0.0, 255.0) as u8;
            rgba[i + 3] = 255;
        }
    }
    Image { width: w, height: h, rgba }
}
