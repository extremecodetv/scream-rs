//! pix <png> x y ...  — prints RGBA at points
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let img = image::open(&a[1]).expect("open").to_rgba8();
    println!("{}x{}", img.width(), img.height());
    for p in a[2..].chunks(2) {
        let (x, y) = (p[0].parse().unwrap(), p[1].parse().unwrap());
        println!("({x},{y}) = {:?}", img.get_pixel(x, y).0);
    }
}
