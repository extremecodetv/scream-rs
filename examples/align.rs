//! align <a.png> <b.png>: best integer shift (dx, dy) of b against a, and scale-about-centre fit.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let x = image::open(&a[1]).unwrap().to_luma8();
    let y = image::open(&a[2]).unwrap().to_luma8();
    let (w, h) = x.dimensions();
    let (y0, y1) = (80i32, 690i32);
    let err = |f: &dyn Fn(i32, i32) -> Option<(i32, i32)>| {
        let mut s = 0f64; let mut n = 0f64;
        for py in (y0..y1).step_by(2) { for px in (40..w as i32 - 40).step_by(2) {
            if let Some((qx, qy)) = f(px, py) {
                if qx >= 0 && qy >= 0 && qx < w as i32 && qy < h as i32 {
                    s += (x.get_pixel(px as u32, py as u32)[0] as f64 - y.get_pixel(qx as u32, qy as u32)[0] as f64).abs(); n += 1.0;
                }
            }
        }}
        s / n
    };
    let mut best = (f64::MAX, 0, 0);
    for dy in -6..=6 { for dx in -6..=6 {
        let e = err(&|px, py| Some((px + dx, py + dy)));
        if e < best.0 { best = (e, dx, dy); }
    }}
    println!("shift: err {:.2} at dx={} dy={}", best.0, best.1, best.2);
    let (cx, cy) = (w as f64 / 2.0, 384.0);
    let mut bs = (f64::MAX, 1.0);
    for i in -20..=20 { let s = 1.0 + i as f64 * 0.005;
        let e = err(&|px, py| Some(((cx + (px as f64 - cx) * s).round() as i32, (cy + (py as f64 - cy) * s).round() as i32)));
        if e < bs.0 { bs = (e, s); }
    }
    println!("scale: err {:.2} at s={:.3}", bs.0, bs.1);
}
