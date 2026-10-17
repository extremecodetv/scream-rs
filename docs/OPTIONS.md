# Port options

Everything scream-rs adds on top of the original engine is a switchable option. Open
**Setup → port options ...** in the main menu (two pages). Left click cycles a value forward,
right click goes back. Options are stored in `scream.cfg` in the port's settings folder (the
cfg keys are listed below), and every option can be set back to the original behaviour.

## Display

| Option | cfg key | Values | Effect |
|---|---|---|---|
| display | `DISPLAY` | `WINDOW`, `FULLSCREEN`, `EXCLUSIVE` | windowed, borderless fullscreen, or exclusive fullscreen at the monitor's best refresh rate |
| resolution | `RESOLUTION` | `0x0` (desktop) or any mode the monitor reports | window/screen size, applied immediately |
| view | `VIEW` | `WIDE`, `CLASSIC` (4:3), `FULL` (no bars) | panoramas: Hor+ across the whole window width, the original 4:3 picture, or the whole window with no letterbox bars (same scale, more visible above and below) |
| close-ups | `ZOOM_SCALE` | `ORIGINAL`, `FIT`, `FILL`, `STRETCH` | scaling of close-ups and overlays; hotspots and the sky behind zoom masks follow the picture |
| videos | `VIDEO_SCALE` | `ORIGINAL`, `FIT`, `FILL`, `STRETCH` | scaling of cutscenes |
| texture filter | `FILTER` | `BILINEAR`, `FSR` | AMD FidelityFX FSR 1: EASU edge-adaptive upsampling for the photographic pictures (node faces, close-ups, overlays, videos) plus RCAS sharpening of the final frame. Text and cursors are left alone |

## Controls

| Option | cfg key | Values | Effect |
|---|---|---|---|
| keys | `KEYS` | `MODERN`, `ORIGINAL` | `MODERN`: Left/Right turn, Up/Down tilt the view one step, Space levels it (arrows, WASD or numpad 4/6/8/2/5). `ORIGINAL`: the exact original bindings |
| mouse speed | `MOUSE_SPEED` | 1–10 (5 = original) | mouse-look sensitivity in *alternate control* mode |
| invert mouse | `INVERT_Y` | on/off | inverted vertical mouse look (and right stick) |
| gamepad | `GAMEPAD` | on/off | gamepad input, see the mapping below |
| hotspots on TAB | `HOTSPOT_KEY` | on/off | hold TAB to highlight every interactive area of the current view |
| click to skip | `CLICK_SKIP` | on/off | a left click or Space skips a line or a pause, like Escape |

### Gamepad mapping

| Pad (Xbox / PlayStation) | Action |
|---|---|
| Left stick | move the cursor |
| Right stick | look around smoothly in panoramas |
| A / Cross | left click |
| B / Circle | right click (inventory, back) |
| X / Square (hold) | highlight hotspots (TAB) |
| Y / Triangle (hold) | message history (H) |
| D-pad, LB / RB | turn and tilt in steps |
| Start | Escape (menu, skip) |

## Saving

| Option | cfg key | Values | Effect |
|---|---|---|---|
| autosave | `AUTOSAVE` | on/off | saves to slot 12 whenever you enter another room — only from the top-level loop (never in the middle of a script), at most every 20 seconds. F7 and slot "0." in the load menu pick the newer of the quicksave and the autosave (shown as "(auto)") |

## Text

| Option | cfg key | Values | Effect |
|---|---|---|---|
| subtitles | `LARGE_SUBTITLES` | on/off | 1.5× larger subtitles |
| subtitle backdrop | `SUBTITLE_BOX` | on/off | dark box behind subtitles |
| history on H | `HISTORY` | on/off | hold H to read the last subtitles and messages (up to 200) |

## Audio

| Option | cfg key | Values | Effect |
|---|---|---|---|
| volume | `VOLUME` | 0–10 | master volume |
| music volume | `VOLUME_MUSIC` | 0–10 | music channels |
| effects volume | `VOLUME_FX` | 0–10 | sound effects |
| voice volume | `VOLUME_VOICE` | 0–10 | speech |

## Command line

Any option can also be given on the command line for one run, without touching the saved
settings: `scream-rs --opt VIEW FULL --opt FILTER FSR`.
