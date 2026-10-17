//! Lua 5.1 precompiled chunk re-encoder.
//!
//! The game's `.sc` files are stock Lua 5.1 bytecode produced by a 32-bit `luac`
//! (`size_t` = 4). A 64-bit Lua refuses them in `luaU_undump` because the header records the
//! `size_t` width. Only string lengths use `size_t`, so the chunk is rewritten with the host's
//! width; code, constants and debug info are copied unchanged.

use anyhow::{bail, Result};

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    size_t: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.data.len() {
            bail!("truncated chunk at {}", self.pos);
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    fn int(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    fn size(&mut self) -> Result<u64> {
        let b = self.bytes(self.size_t)?;
        let mut v = [0u8; 8];
        v[..b.len()].copy_from_slice(b);
        Ok(u64::from_le_bytes(v))
    }
}

struct Writer {
    out: Vec<u8>,
    size_t: usize,
}

impl Writer {
    fn size(&mut self, v: u64) {
        self.out.extend_from_slice(&v.to_le_bytes()[..self.size_t]);
    }
}

fn string(r: &mut Reader, w: &mut Writer) -> Result<()> {
    let len = r.size()?;
    w.size(len);
    w.out.extend_from_slice(r.bytes(len as usize)?);
    Ok(())
}

fn copy_ints(r: &mut Reader, w: &mut Writer, per_item: usize) -> Result<usize> {
    let n = r.int()?;
    w.out.extend_from_slice(&n.to_le_bytes());
    w.out.extend_from_slice(r.bytes(n as usize * per_item)?);
    Ok(n as usize)
}

fn function(r: &mut Reader, w: &mut Writer) -> Result<()> {
    string(r, w)?; // source
    w.out.extend_from_slice(r.bytes(4 + 4 + 4)?); // linedefined, lastlinedefined, 4 bytes of counts
    copy_ints(r, w, 4)?; // code
    let nk = r.int()?;
    w.out.extend_from_slice(&nk.to_le_bytes());
    for _ in 0..nk {
        let t = r.byte()?;
        w.out.push(t);
        match t {
            0 => {}
            1 => w.out.push(r.byte()?),
            3 => w.out.extend_from_slice(r.bytes(8)?),
            4 => string(r, w)?,
            t => bail!("bad constant type {t}"),
        }
    }
    let np = r.int()?;
    w.out.extend_from_slice(&np.to_le_bytes());
    for _ in 0..np {
        function(r, w)?;
    }
    copy_ints(r, w, 4)?; // lineinfo
    let nloc = r.int()?;
    w.out.extend_from_slice(&nloc.to_le_bytes());
    for _ in 0..nloc {
        string(r, w)?;
        w.out.extend_from_slice(r.bytes(8)?); // startpc, endpc
    }
    let nup = r.int()?;
    w.out.extend_from_slice(&nup.to_le_bytes());
    for _ in 0..nup {
        string(r, w)?;
    }
    Ok(())
}

/// Returns `chunk` re-encoded for this platform's `size_t`, or the input when it already matches.
pub fn adapt(chunk: &[u8]) -> Result<Vec<u8>> {
    if chunk.len() < 12 || &chunk[..5] != b"\x1bLuaQ" {
        bail!("not a Lua 5.1 chunk");
    }
    let (int_size, size_t, insn, num, integral) = (chunk[7], chunk[8], chunk[9], chunk[10], chunk[11]);
    if chunk[6] != 1 || int_size != 4 || insn != 4 || num != 8 || integral != 0 {
        bail!("unsupported Lua 5.1 chunk layout");
    }
    let host = std::mem::size_of::<usize>();
    if size_t as usize == host {
        return Ok(chunk.to_vec());
    }
    let mut header = chunk[..12].to_vec();
    header[8] = host as u8;
    let mut r = Reader { data: chunk, pos: 12, size_t: size_t as usize };
    let mut w = Writer { out: header, size_t: host };
    function(&mut r, &mut w)?;
    if r.pos != chunk.len() {
        bail!("trailing data after chunk ({} of {} bytes)", r.pos, chunk.len());
    }
    Ok(w.out)
}
