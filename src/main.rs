// Release builds are GUI programs on Windows, so no console window opens with the game;
// `--console` brings one back for the log. Debug builds keep theirs for `cargo run`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use scream::engine::{Engine, Options};

fn usage() -> ! {
    eprintln!(
        "usage: scream-rs [options]\n\
         \n  --game <dir>               folder of Scratches: Director's Cut (scratches.sc, scratches.res)\
         \n  --console                  show a console window with the log\
         \n  --windowed <W>x<H>         start in a window of this size\
         \n  --pos <x> <y>              window position\
         \n  --opt <KEY> <VALUE>        port option as in scream.cfg (e.g. --opt VIEW FULL)\
         \n  --skip-videos              do not play cutscenes\
         \n  --script <path>            run this script instead of the configured one (no extension)\
         \n  --exec \"<lua>\"             run Lua when the main loop starts\
         \n  --input \"<frame>:<kind>:<arg>;...\"\
         \n                             scripted input: click, rclick, move, look, key, keyup\
         \n  --snap <file.png> <frame>  screenshot at a frame (repeatable)\
         \n  --shot <file.png> <frame>  screenshot and quit"
    );
    std::process::exit(2);
}

fn find_game_dir() -> Option<PathBuf> {
    let candidates = [
        std::env::current_dir().ok(),
        std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())),
        Some(PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Scratches")),
        Some(PathBuf::from(r"D:\SteamLibrary\steamapps\common\Scratches")),
    ];
    candidates.into_iter().flatten().find(|d| d.join("scratches.sc").is_file() || d.join("scream.sc").is_file())
}

/// Gives a GUI-subsystem process a console: the one of the shell it was started from, or a new
/// window when started from Explorer.
#[cfg(windows)]
fn open_console() {
    use windows_sys::Win32::System::Console::{AllocConsole, AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            AllocConsole();
        }
    }
}

#[cfg(not(windows))]
fn open_console() {}

/// Reports an error that stops the game before it has a window. Without a console (a Windows
/// release build) stderr goes nowhere, so it is shown in a message box too.
fn fatal(msg: &str) -> ! {
    eprintln!("{msg}");
    #[cfg(all(windows, not(debug_assertions)))]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (text, caption) = (wide(msg), wide("scream-rs"));
        unsafe {
            MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONERROR);
        }
    }
    std::process::exit(1);
}

fn main() {
    if std::env::args().skip(1).any(|a| a == "--console") {
        open_console();
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_hal=warn,wgpu_core=warn,naga=warn")).init();
    let mut game: Option<PathBuf> = None;
    let mut options = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--game" => game = Some(args.next().unwrap_or_else(|| usage()).into()),
            "--console" => {}
            "--skip-videos" => options.skip_videos = true,
            "--input" => options.inputs = scream::engine::parse_inputs(&args.next().unwrap_or_else(|| usage())),
            "--exec" => options.exec = Some(args.next().unwrap_or_else(|| usage())),
            "--script" => options.script = Some(args.next().unwrap_or_else(|| usage())),
            "--windowed" => {
                let s = args.next().unwrap_or_else(|| usage());
                let (w, h) = s.split_once('x').unwrap_or_else(|| usage());
                options.windowed = Some((w.parse().unwrap_or(1024), h.parse().unwrap_or(768)));
            }
            "--pos" => {
                let x = args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage());
                let y = args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage());
                options.position = Some((x, y));
            }
            "--snap" => {
                let file = args.next().unwrap_or_else(|| usage());
                let frame = args.next().and_then(|f| f.parse().ok()).unwrap_or_else(|| usage());
                options.shots.push((frame, file.into()));
            }
            "--shot" => {
                let file = args.next().unwrap_or_else(|| usage());
                let frame = args.next().and_then(|f| f.parse().ok()).unwrap_or_else(|| usage());
                options.shot = Some((file.into(), frame));
            }
            "--opt" => {
                let k = args.next().unwrap_or_else(|| usage());
                let v = args.next().unwrap_or_else(|| usage());
                options.opts.push((k, v));
            }
            "-h" | "--help" => usage(),
            _ => usage(),
        }
    }
    let Some(game) = game.or_else(find_game_dir) else {
        fatal("Scratches game folder not found; pass --game <dir>");
    };
    let engine = match Engine::new(game, options) {
        Ok(e) => e,
        Err(e) => fatal(&format!("startup failed: {e:#}")),
    };
    if let Err(e) = engine.boot() {
        log::error!("{e:#}");
    }
}
