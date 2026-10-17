//! crop <in.png> <out.png> <x> <y> <w> <h> [scale]
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let img = image::open(&a[1]).expect("open").to_rgba8();
    let n = |i: usize| a[i].parse::<u32>().unwrap();
    let c = image::imageops::crop_imm(&img, n(3), n(4), n(5), n(6)).to_image();
    let s: f32 = a.get(7).map_or(1.0, |v| v.parse().unwrap());
    let c = image::imageops::resize(&c, (c.width() as f32 * s) as u32, (c.height() as f32 * s) as u32, image::imageops::FilterType::Triangle);
    c.save(&a[2]).expect("save");
}
