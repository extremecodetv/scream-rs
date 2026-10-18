# Changelog

## Unreleased

### Fixed

- Saves keep the state of the world. The script's flags (`first_cellar`, `has_knife`,
  `currentGame`, ...) hold whole numbers, which were left out of the save, so a loaded game
  replayed old events, e.g. day one's reminder to unpack the luggage on day two. Flags created
  after a save are cleared when it is loaded. Saves made with 0.1.1 or earlier carry no flags
  and should be made again.
- Save slots in the menu are captioned: the location and day from the script, "saved game"
  for a save without a caption, "empty" for a free slot.

### Changed

- Windows release builds start without a console window; `--console` shows one with the log.
  Startup errors are shown in a message box.

## 0.1.1 — 2026-10-18

### Fixed

- Localized versions of the game now start and show their text correctly. Tested with the
  Russian and the Spanish (fan) translations.
  - Compiled scripts with trailing bytes after the Lua chunk load, like in the original
    (the Russian `scratches.sc` ends with a padding byte).
  - `.sc` scripts shipped as plain Lua source load too (the Spanish translation).
  - Script text is kept byte-for-byte instead of being decoded as UTF-8, so 8-bit
    encodings (Windows-1251, Windows-1252) reach the font atlas intact; text centering and
    wrapping count characters, not UTF-8 bytes.
  - Localized `.srt` subtitles are read as bytes, so non-UTF-8 files no longer come up empty.
