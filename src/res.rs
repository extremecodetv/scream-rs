//! `.res` archives (`scream.res`, `scratches.res`).
//!
//! Layout (scream.exe `res_Open` 0x40ea90): a 0x104-byte header — magic `"SCream resource file"`
//! and the entry count as u32 at 0x100 — followed by 0x5c-byte entries
//! `{ name[0x50], type u32, offset u32, size u32 }`. Payloads are stored uncompressed.
//! The engine keeps one archive open at a time and resolves names by exact, case-sensitive match.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Resource kind as stored in the archive (and inferred from the extension for loose files).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Jpeg,
    Tga,
    Ogg,
    Srt,
    Other,
}

impl Kind {
    fn from_code(code: u32) -> Kind {
        match code {
            1 => Kind::Jpeg,
            2 => Kind::Tga,
            3 => Kind::Ogg,
            4 => Kind::Srt,
            _ => Kind::Other,
        }
    }

    /// Same substring test the engine uses (`strstr(name, ".jpg")`, ...).
    pub fn from_name(name: &str) -> Kind {
        if name.contains(".jpg") {
            Kind::Jpeg
        } else if name.contains(".tga") {
            Kind::Tga
        } else if name.contains(".ogg") {
            Kind::Ogg
        } else if name.contains(".srt") {
            Kind::Srt
        } else {
            Kind::Other
        }
    }
}

#[derive(Clone, Debug)]
struct Entry {
    kind: Kind,
    offset: u64,
    size: u64,
}

pub struct Archive {
    path: PathBuf,
    file: File,
    entries: HashMap<String, Entry>,
}

const HEADER_SIZE: usize = 0x104;
const ENTRY_SIZE: usize = 0x5c;
const MAGIC: &[u8] = b"SCream resource file";

impl Archive {
    pub fn open(path: &Path) -> Result<Archive> {
        let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let mut header = [0u8; HEADER_SIZE];
        file.read_exact(&mut header)?;
        if !header.starts_with(MAGIC) {
            bail!("{} is not a SCream resource file", path.display());
        }
        let count = u32::from_le_bytes(header[0x100..0x104].try_into().unwrap()) as usize;
        let mut table = vec![0u8; count * ENTRY_SIZE];
        file.read_exact(&mut table)?;
        let mut entries = HashMap::with_capacity(count);
        for raw in table.chunks_exact(ENTRY_SIZE) {
            let name_len = raw[..0x50].iter().position(|&b| b == 0).unwrap_or(0x50);
            let name = String::from_utf8_lossy(&raw[..name_len]).into_owned();
            let u32_at = |o: usize| u32::from_le_bytes(raw[o..o + 4].try_into().unwrap());
            entries.insert(
                name,
                Entry { kind: Kind::from_code(u32_at(0x50)), offset: u32_at(0x54) as u64, size: u32_at(0x58) as u64 },
            );
        }
        Ok(Archive { path: path.to_owned(), file, entries })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub fn kind(&self, name: &str) -> Option<Kind> {
        self.entries.get(name).map(|e| e.kind)
    }

    pub fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        let entry = self.entries.get(name)?.clone();
        let mut data = vec![0u8; entry.size as usize];
        self.file.seek(SeekFrom::Start(entry.offset)).ok()?;
        self.file.read_exact(&mut data).ok()?;
        Some(data)
    }
}

/// Resource access as the engine sees it: one active archive (`res_SetFile` / `res_Open`), or
/// loose files under the resource path when archives are disabled (`SC_RESOURCE` off).
pub struct Resources {
    root: PathBuf,
    archive: Option<Archive>,
}

impl Resources {
    pub fn new(root: PathBuf) -> Resources {
        Resources { root, archive: None }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Replaces the active archive. Mirrors `res_Close` + `res_Open`.
    pub fn open_archive(&mut self, file_name: &str) -> Result<()> {
        self.archive = None;
        self.archive = Some(Archive::open(&self.root.join(file_name))?);
        Ok(())
    }

    pub fn close_archive(&mut self) {
        self.archive = None;
    }

    pub fn exists(&self, name: &str) -> bool {
        match &self.archive {
            Some(a) => a.contains(name),
            None => self.root.join(name).is_file(),
        }
    }

    /// Reads a whole resource. Returns `None` when it does not exist, like `res_Find` returning 0.
    pub fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        match &mut self.archive {
            Some(a) => a.read(name),
            None => std::fs::read(self.root.join(name)).ok(),
        }
    }

    pub fn kind(&self, name: &str) -> Kind {
        self.archive.as_ref().and_then(|a| a.kind(name)).unwrap_or_else(|| Kind::from_name(name))
    }

    /// Plain file next to the game (subtitles, configs) — the engine opens these with `fopen`.
    pub fn loose_path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}
