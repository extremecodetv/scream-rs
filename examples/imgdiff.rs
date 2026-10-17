//! imgdiff <orig dir> <port dir> <out dir>
//! For every PNG present in both: mean absolute difference (0..255), and a side-by-side image
//! (original | port | amplified difference). Prints a table sorted by difference.
use image::{GenericImage, Rgba, RgbaImage};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (orig, port, out) = (std::path::Path::new(&a[1]), std::path::Path::new(&a[2]), std::path::Path::new(&a[3]));
    std::fs::create_dir_all(out).unwrap();
    let mut rows = Vec::new();
    for e in std::fs::read_dir(orig).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.ends_with(".png") { continue; }
        let Ok(b) = image::open(port.join(&name)) else { continue };
        let a = image::open(e.path()).unwrap().to_rgba8();
        let b = image::imageops::resize(&b.to_rgba8(), a.width(), a.height(), image::imageops::FilterType::Triangle);
        let (w, h) = a.dimensions();
        let mut diff = RgbaImage::new(w, h);
        let mut sum = 0u64;
        for y in 0..h { for x in 0..w {
            let (p, q) = (a.get_pixel(x, y), b.get_pixel(x, y));
            let d: Vec<u8> = (0..3).map(|i| (p[i] as i32 - q[i] as i32).unsigned_abs() as u8).collect();
            sum += d.iter().map(|&v| v as u64).sum::<u64>();
            diff.put_pixel(x, y, Rgba([d[0].saturating_mul(4), d[1].saturating_mul(4), d[2].saturating_mul(4), 255]));
        }}
        let mean = sum as f64 / (w * h * 3) as f64;
        let mut sbs = RgbaImage::new(w * 3, h);
        sbs.copy_from(&a, 0, 0).unwrap();
        sbs.copy_from(&b, w, 0).unwrap();
        sbs.copy_from(&diff, w * 2, 0).unwrap();
        let small = image::imageops::resize(&sbs, w * 3 / 2, h / 2, image::imageops::FilterType::Triangle);
        small.save(out.join(&name)).unwrap();
        rows.push((mean, name));
    }
    rows.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap());
    let total: f64 = rows.iter().map(|r| r.0).sum::<f64>() / rows.len().max(1) as f64;
    println!("{} pairs, average difference {:.2}", rows.len(), total);
    for (m, n) in &rows { println!("{m:7.2}  {n}"); }
}
