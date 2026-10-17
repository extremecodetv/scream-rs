//! Plays a resource on a channel for a few seconds and prints channel state.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut arc = scream::res::Archive::open(std::path::Path::new(r"D:\SteamLibrary\steamapps\common\Scratches\scratches.res")).unwrap();
    let audio = scream::audio::Audio::new();
    let name = a.get(1).cloned().unwrap_or("trck-menu-F.ogg".into());
    audio.load(1, &name, arc.read(&name));
    audio.play(1, false);
    audio.fade_in(1, 2);
    for i in 0..6 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let inf = audio.info(1);
        println!("t={:.1}s playing={} gain={:.3} elapsed={} total={}", (i + 1) as f32 * 0.5, inf.playing, inf.gain, audio.time(1, 1), audio.time(1, 2));
    }
}
