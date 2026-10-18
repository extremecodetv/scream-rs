# Changelog

## Unreleased

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
