//! Lists archive statistics and prints headers of selected resources.
use scream::res::Archive;

fn main() -> anyhow::Result<()> {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or(r"D:\SteamLibrary\steamapps\common\Scratches".into()));
    for arc in ["scream.res", "scratches.res"] {
        let mut a = Archive::open(&dir.join(arc))?;
        println!("{arc}: {} entries", a.len());
        let mut names: Vec<String> = a.names().map(String::from).collect();
        names.sort();
        let want = std::env::args().skip(2).collect::<Vec<_>>();
        for n in &names {
            if !want.iter().any(|w| n.contains(w.as_str())) { continue; }
            let d = a.read(n).unwrap();
            if n.ends_with(".tga") {
                println!("  {n}: type {} {}x{} bpp {} desc {:#x} idlen {}", d[2], u16::from_le_bytes([d[12],d[13]]), u16::from_le_bytes([d[14],d[15]]), d[16], d[17], d[0]);
            } else if n.ends_with(".ogg") {
                let p = d.windows(7).position(|w| w == b"\x01vorbis").unwrap();
                println!("  {n}: ch {} rate {} dur {:.2}s theora {}", d[p+11], u32::from_le_bytes(d[p+12..p+16].try_into().unwrap()), scream::audio::ogg_duration(&d), d.windows(7).any(|w| w == b"\x80theora"));
            } else { println!("  {n}: {} bytes", d.len()); }
        }
    }
    Ok(())
}
