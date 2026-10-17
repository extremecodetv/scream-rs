//! Image decoding with the engine's conventions.
//!
//! `img_LoadTGA` (0x404470) reads TGA rows in file order and ignores the origin bit, so the
//! first stored row ends up at texture v = 0 (the top of every quad). Only type 2
//! (uncompressed true-colour, 24/32 bpp) and types 10/11 (RLE colour / grayscale) are accepted.
//! Grayscale data is used as GL_LUMINANCE: (L, L, L, 1). JPEGs go through libjpeg top-down.

use crate::gfx::Image;
use crate::res::Kind;

pub fn decode(kind: Kind, data: &[u8]) -> Option<Image> {
    match kind {
        Kind::Tga => decode_tga(data),
        Kind::Jpeg => decode_jpeg(data),
        _ => None,
    }
}

fn decode_jpeg(data: &[u8]) -> Option<Image> {
    let img = image::load_from_memory_with_format(data, image::ImageFormat::Jpeg).ok()?.to_rgba8();
    Some(Image { width: img.width(), height: img.height(), rgba: img.into_raw() })
}

fn expand(px: &[u8], out: &mut Vec<u8>) {
    match px.len() {
        1 => out.extend_from_slice(&[px[0], px[0], px[0], 255]),
        3 => out.extend_from_slice(&[px[2], px[1], px[0], 255]),
        _ => out.extend_from_slice(&[px[2], px[1], px[0], px[3]]),
    }
}

pub fn decode_tga(data: &[u8]) -> Option<Image> {
    if data.len() < 18 {
        return None;
    }
    let image_type = data[2];
    let width = u16::from_le_bytes([data[12], data[13]]) as u32;
    let height = u16::from_le_bytes([data[14], data[15]]) as u32;
    let bpp = data[16] as usize / 8;
    if width == 0 || height == 0 || !matches!(bpp, 1 | 3 | 4) {
        return None;
    }
    let body = &data[18..];
    let pixels = (width * height) as usize;
    let mut out = Vec::with_capacity(pixels * 4);
    match image_type {
        2 => {
            if bpp == 1 {
                return None;
            }
            for px in body.chunks_exact(bpp).take(pixels) {
                expand(px, &mut out);
            }
        }
        10 | 11 => {
            let mut p = 0;
            while out.len() < pixels * 4 {
                let head = *body.get(p)?;
                p += 1;
                let n = (head & 0x7f) as usize + 1;
                if head & 0x80 == 0 {
                    for _ in 0..n {
                        expand(body.get(p..p + bpp)?, &mut out);
                        p += bpp;
                    }
                } else {
                    let px = body.get(p..p + bpp)?;
                    p += bpp;
                    for _ in 0..n {
                        expand(px, &mut out);
                    }
                }
            }
            out.truncate(pixels * 4);
        }
        _ => return None,
    }
    if out.len() != pixels * 4 {
        return None;
    }
    Some(Image { width, height, rgba: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rle_grayscale_and_raw_bgr() {
        let mut t = vec![0u8; 18];
        t[2] = 11;
        t[12] = 3;
        t[14] = 1;
        t[16] = 8;
        t.extend_from_slice(&[0x82, 7]); // run of 3 × 7
        let img = decode_tga(&t).unwrap();
        assert_eq!(img.rgba, [7, 7, 7, 255].repeat(3));

        let mut t = vec![0u8; 18];
        t[2] = 2;
        t[12] = 1;
        t[14] = 1;
        t[16] = 24;
        t.extend_from_slice(&[1, 2, 3]);
        assert_eq!(decode_tga(&t).unwrap().rgba, vec![3, 2, 1, 255]);
    }
}
