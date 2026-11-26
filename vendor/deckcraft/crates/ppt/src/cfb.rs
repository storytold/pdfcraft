//! Reading Compound File Binary containers ([MS-CFB] §2.2–2.6), the file system a `.ppt` lives in:
//! fixed-size sectors chained by a FAT, a directory of 128-byte entries arranged as one sibling
//! tree per storage, and a mini stream (with its own mini FAT) for streams under 4096 bytes.
//!
//! Ported from FilmCraft's clean-room `filmcraft-cfb` reader (same owner and licence), with every
//! read bounds-checked through `get` and every chain bounded by the file's sector count, so a
//! damaged file gives an error, never a panic or an endless loop.

use std::cmp::Ordering;

const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const MAXREGSECT: u32 = 0xFFFF_FFFA;
const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const FREESECT: u32 = 0xFFFF_FFFF;
const NOSTREAM: u32 = 0xFFFF_FFFF;
const MINI_SECTOR_SIZE: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CfbError {
    #[error("not a compound file")]
    NotCfb,
    #[error("damaged compound file: {0}")]
    Invalid(String),
    #[error("no stream {0:?}")]
    NotFound(String),
}

type Result<T> = std::result::Result<T, CfbError>;

fn invalid(s: impl Into<String>) -> CfbError {
    CfbError::Invalid(s.into())
}

/// Whether `bytes` starts with the compound file signature.
pub fn sniff(bytes: &[u8]) -> bool {
    bytes.starts_with(&SIGNATURE)
}

pub(crate) fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([*s.first()?, *s.get(1)?]))
}

pub(crate) fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?]))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from(u32_at(b, at)?) | (u64::from(u32_at(b, at + 4)?) << 32))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Root,
    Storage,
    Stream,
    Unknown,
}

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    kind: Kind,
    start: u32,
    size: u64,
    children: Vec<usize>,
    left: u32,
    right: u32,
    child: u32,
}

/// A parsed compound file over borrowed bytes.
pub struct CompoundFile<'a> {
    data: &'a [u8],
    sector_size: usize,
    mini_cutoff: u64,
    fat: Vec<u32>,
    minifat: Vec<u32>,
    mini_stream: Vec<u8>,
    entries: Vec<Entry>,
}

impl<'a> CompoundFile<'a> {
    /// Parse the header, FAT, mini FAT, directory and mini stream of `data`.
    pub fn open(data: &'a [u8]) -> Result<CompoundFile<'a>> {
        if data.len() < 512 || !sniff(data) || u16_at(data, 28) != Some(0xFFFE) {
            return Err(CfbError::NotCfb);
        }
        let h = |at| u32_at(data, at).ok_or(CfbError::NotCfb);
        let version = u16_at(data, 26).ok_or(CfbError::NotCfb)?;
        let shift = u16_at(data, 30).ok_or(CfbError::NotCfb)?;
        if shift != 9 && shift != 12 {
            return Err(invalid(format!("sector shift {shift}")));
        }
        let sector_size = 1usize << shift;
        let n_fat = h(44)? as usize;
        let first_dir = h(48)?;
        let mini_cutoff = u64::from(h(56)?);
        let first_minifat = h(60)?;
        let first_difat = h(68)?;
        let max_sectors = data.len() / sector_size + 1;
        if n_fat > max_sectors {
            return Err(invalid(format!("{n_fat} FAT sectors in a {}-byte file", data.len())));
        }
        let mut cf =
            CompoundFile { data, sector_size, mini_cutoff, fat: Vec::new(), minifat: Vec::new(), mini_stream: Vec::new(), entries: Vec::new() };
        // FAT sector numbers: 109 in the header, then the DIFAT chain.
        let mut fat_sectors: Vec<u32> = (0..109).filter_map(|i| u32_at(data, 76 + i * 4)).filter(|&s| s <= MAXREGSECT).collect();
        let mut d = first_difat;
        let mut seen = 0usize;
        let per = sector_size / 4 - 1;
        while d <= MAXREGSECT && fat_sectors.len() < n_fat {
            seen += 1;
            if seen > max_sectors {
                return Err(invalid("DIFAT chain loops"));
            }
            let s = cf.sector(d)?;
            fat_sectors.extend((0..per).filter_map(|i| u32_at(s, i * 4)).filter(|&v| v <= MAXREGSECT));
            d = u32_at(s, per * 4).ok_or_else(|| invalid("short DIFAT sector"))?;
        }
        fat_sectors.truncate(n_fat);
        let mut fat = Vec::with_capacity(fat_sectors.len().saturating_mul(sector_size / 4));
        for s in fat_sectors {
            let b = cf.sector(s)?;
            fat.extend((0..sector_size / 4).filter_map(|i| u32_at(b, i * 4)));
        }
        cf.fat = fat;
        let dir = cf.read_chain(first_dir, None)?;
        for e in dir.as_chunks::<128>().0 {
            cf.entries.push(parse_entry(e, version).ok_or_else(|| invalid("short directory entry"))?);
        }
        match cf.entries.first() {
            Some(r) if r.kind == Kind::Root => {}
            _ => return Err(invalid("the first directory entry is not the root")),
        }
        if first_minifat <= MAXREGSECT {
            let mf = cf.read_chain(first_minifat, None)?;
            cf.minifat = mf.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
        }
        let (root_start, root_size) = cf.entries.first().map(|r| (r.start, r.size)).unwrap_or((ENDOFCHAIN, 0));
        if root_start <= MAXREGSECT && root_size > 0 {
            let size = usize::try_from(root_size.min(data.len() as u64)).unwrap_or(0);
            cf.mini_stream = cf.read_chain(root_start, Some(size))?;
        }
        cf.build_tree()?;
        Ok(cf)
    }

    fn sector(&self, n: u32) -> Result<&'a [u8]> {
        if n > MAXREGSECT {
            return Err(invalid(format!("bad sector {n:#x}")));
        }
        let start = (n as usize).checked_add(1).and_then(|k| k.checked_mul(self.sector_size)).ok_or_else(|| invalid("sector number overflow"))?;
        let end = start.checked_add(self.sector_size).ok_or_else(|| invalid("sector number overflow"))?;
        self.data.get(start..end).ok_or_else(|| invalid(format!("sector {n} is outside the file")))
    }

    /// Concatenate a regular sector chain; `limit` stops once that many bytes are read.
    fn read_chain(&self, start: u32, limit: Option<usize>) -> Result<Vec<u8>> {
        // Reserve up to the stream's size; `limit` is checked against the file length by callers,
        // and `min` keeps a bad size from reserving more than the file.
        let mut out = Vec::with_capacity(limit.unwrap_or(0).min(self.data.len()));
        let mut s = start;
        // A chain cannot visit more sectors than the file holds, and must not visit one twice: a
        // loop cut off by `limit` would otherwise return repeated sectors as the stream.
        let mut visited = vec![false; self.data.len() / self.sector_size + 1];
        while s != ENDOFCHAIN {
            match visited.get_mut(s as usize) {
                Some(v) if !*v => *v = true,
                Some(_) => return Err(invalid("sector chain loops")),
                None => return Err(invalid(format!("sector {s} is outside the file"))),
            }
            out.extend_from_slice(self.sector(s)?);
            if limit.is_some_and(|l| out.len() >= l) {
                break;
            }
            s = *self.fat.get(s as usize).ok_or_else(|| invalid(format!("sector {s} has no FAT entry")))?;
            if s == FREESECT {
                return Err(invalid("chain runs into a free sector"));
            }
        }
        if let Some(l) = limit {
            if out.len() < l {
                return Err(invalid("stream is truncated"));
            }
            out.truncate(l);
        }
        Ok(out)
    }

    fn read_mini_chain(&self, start: u32, size: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(size.min(self.mini_stream.len()));
        let mut s = start;
        let mut steps = 0usize;
        while out.len() < size {
            if s > MAXREGSECT {
                return Err(invalid("mini stream chain ends early"));
            }
            steps += 1;
            if steps > self.minifat.len().max(1) {
                return Err(invalid("mini sector chain loops"));
            }
            let at = (s as usize).checked_mul(MINI_SECTOR_SIZE).ok_or_else(|| invalid("mini sector overflow"))?;
            // Checked: on a 32-bit target a sector number near 0x03FF_FFFF puts `at` at the top of usize.
            let end = at.checked_add(MINI_SECTOR_SIZE).ok_or_else(|| invalid("mini sector overflow"))?;
            let b = self.mini_stream.get(at..end).ok_or_else(|| invalid("mini sector outside the mini stream"))?;
            out.extend_from_slice(b);
            s = *self.minifat.get(s as usize).ok_or_else(|| invalid("mini sector has no mini FAT entry"))?;
        }
        out.truncate(size);
        Ok(out)
    }

    fn build_tree(&mut self) -> Result<()> {
        let n = self.entries.len();
        let mut owned = vec![false; n];
        if let Some(o) = owned.first_mut() {
            *o = true;
        }
        for parent in 0..n {
            let Some(p) = self.entries.get(parent) else { continue };
            if !matches!(p.kind, Kind::Root | Kind::Storage) {
                continue;
            }
            let mut kids = Vec::new();
            // In-order walk of the sibling tree, iterative and cycle-safe.
            let mut stack: Vec<u32> = Vec::new();
            let mut cur = p.child;
            let mut steps = 0;
            while cur != NOSTREAM || !stack.is_empty() {
                while cur != NOSTREAM {
                    let i = cur as usize;
                    match owned.get_mut(i) {
                        Some(o) if !*o => *o = true,
                        _ => return Err(invalid(format!("directory entry {cur} is referenced twice or does not exist"))),
                    }
                    stack.push(cur);
                    cur = self.entries.get(i).map_or(NOSTREAM, |e| e.left);
                    steps += 1;
                    if steps > n {
                        return Err(invalid("directory tree loops"));
                    }
                }
                if let Some(top) = stack.pop() {
                    kids.push(top as usize);
                    cur = self.entries.get(top as usize).map_or(NOSTREAM, |e| e.right);
                }
            }
            if let Some(p) = self.entries.get_mut(parent) {
                p.children = kids;
            }
        }
        Ok(())
    }

    /// Names of the root storage's streams and storages.
    pub fn root_names(&self) -> Vec<&str> {
        self.entries.first().map(|r| r.children.iter().filter_map(|&c| self.entries.get(c)).map(|e| e.name.as_str()).collect()).unwrap_or_default()
    }

    /// Contents of the stream `name` directly under the root (names compare case-insensitively,
    /// [MS-CFB] §2.6.4).
    pub fn read_root_stream(&self, name: &str) -> Result<Vec<u8>> {
        let root = self.entries.first().ok_or_else(|| CfbError::NotFound(name.into()))?;
        let e = root
            .children
            .iter()
            .filter_map(|&c| self.entries.get(c))
            .find(|e| e.kind == Kind::Stream && compare_names(&e.name, name).is_eq())
            .ok_or_else(|| CfbError::NotFound(name.into()))?;
        let size = usize::try_from(e.size).map_err(|_| invalid("stream too large"))?;
        if size == 0 {
            return Ok(Vec::new());
        }
        if e.size > self.data.len() as u64 {
            return Err(invalid(format!("stream {:?} is larger than the file", e.name)));
        }
        if e.size < self.mini_cutoff { self.read_mini_chain(e.start, size) } else { self.read_chain(e.start, Some(size)) }
    }
}

fn upper(u: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&u) {
        return u - 32;
    }
    if u < 0x80 {
        return u;
    }
    match char::from_u32(u32::from(u)) {
        Some(c) => {
            let mut up = c.to_uppercase();
            match (up.next(), up.next()) {
                (Some(x), None) => u16::try_from(u32::from(x)).unwrap_or(u),
                _ => u,
            }
        }
        None => u,
    }
}

/// Sibling order of directory entries ([MS-CFB] §2.6.4): shorter names first, then a code-unit
/// comparison of the upper-cased names.
fn compare_names(a: &str, b: &str) -> Ordering {
    let ua: Vec<u16> = a.encode_utf16().collect();
    let ub: Vec<u16> = b.encode_utf16().collect();
    ua.len().cmp(&ub.len()).then_with(|| ua.iter().map(|&u| upper(u)).cmp(ub.iter().map(|&u| upper(u))))
}

fn parse_entry(b: &[u8], version: u16) -> Option<Entry> {
    let name_len = usize::from(u16_at(b, 64)?).min(64);
    let units: Vec<u16> = (0..name_len / 2).filter_map(|i| u16_at(b, i * 2)).take_while(|&u| u != 0).collect();
    let kind = match b.get(66)? {
        5 => Kind::Root,
        1 => Kind::Storage,
        2 => Kind::Stream,
        _ => Kind::Unknown,
    };
    let size = u64_at(b, 120)?;
    Some(Entry {
        name: String::from_utf16_lossy(&units),
        kind,
        start: u32_at(b, 116)?,
        // Version 3 files may leave garbage in the high 32 bits (§2.6.3).
        size: if version == 3 { size & 0xFFFF_FFFF } else { size },
        children: Vec::new(),
        left: u32_at(b, 68)?,
        right: u32_at(b, 72)?,
        child: u32_at(b, 76)?,
    })
}

/// Builds small version-3 compound files for tests: every stream goes in regular sectors (the
/// mini stream cutoff is set to 0), one flat directory under the root.
#[cfg(test)]
pub(crate) fn build(streams: &[(&str, &[u8])]) -> Vec<u8> {
    build_with(streams, 9, 0)
}

/// A chain of whole `ss`-byte sectors holding `data`, appended to `sectors` and `fat`; returns its
/// first sector.
#[cfg(test)]
fn push_chain(sectors: &mut Vec<Vec<u8>>, fat: &mut Vec<u32>, ss: usize, data: &[u8]) -> u32 {
    let n = data.len().div_ceil(ss).max(1);
    let first = sectors.len() as u32;
    for k in 0..n {
        let mut s = data.get(k * ss..((k + 1) * ss).min(data.len())).unwrap_or(&[]).to_vec();
        s.resize(ss, 0);
        sectors.push(s);
        fat.push(if k + 1 == n { ENDOFCHAIN } else { first + k as u32 + 1 });
    }
    first
}

/// Builds a compound file with `1 << shift` byte sectors (9: version 3, 12: version 4). Streams
/// shorter than `mini_cutoff` go in the mini stream. When the FAT outgrows the header's 109
/// slots, the rest are listed in DIFAT sectors. Layout: streams, mini stream, mini FAT,
/// directory, DIFAT, then the FAT last.
#[cfg(test)]
pub(crate) fn build_with(streams: &[(&str, &[u8])], shift: u16, mini_cutoff: u32) -> Vec<u8> {
    let ss = 1usize << shift;
    let per = ss / 4;
    let mut sectors: Vec<Vec<u8>> = Vec::new();
    let mut fat: Vec<u32> = Vec::new();
    let mut mini = Vec::new();
    let mut minifat: Vec<u32> = Vec::new();
    let mut starts = Vec::new();
    for (_, data) in streams {
        if data.is_empty() {
            starts.push(ENDOFCHAIN);
        } else if (data.len() as u64) < u64::from(mini_cutoff) {
            let n = data.len().div_ceil(MINI_SECTOR_SIZE);
            let first = minifat.len() as u32;
            for k in 0..n {
                minifat.push(if k + 1 == n { ENDOFCHAIN } else { first + k as u32 + 1 });
            }
            mini.extend_from_slice(data);
            mini.resize(minifat.len() * MINI_SECTOR_SIZE, 0);
            starts.push(first);
        } else {
            starts.push(push_chain(&mut sectors, &mut fat, ss, data));
        }
    }
    let (root_start, root_size) = if mini.is_empty() { (ENDOFCHAIN, 0) } else { (push_chain(&mut sectors, &mut fat, ss, &mini), mini.len() as u32) };
    let minifat_first = if minifat.is_empty() {
        ENDOFCHAIN
    } else {
        minifat.resize(minifat.len().div_ceil(per) * per, FREESECT);
        let bytes: Vec<u8> = minifat.iter().flat_map(|v| v.to_le_bytes()).collect();
        push_chain(&mut sectors, &mut fat, ss, &bytes)
    };
    let minifat_sectors = (minifat.len() / per) as u32;
    // Directory: root + streams. Root's child is entry 1; siblings are chained as right children in
    // name order (a valid, if unbalanced, tree).
    let mut order: Vec<usize> = (0..streams.len()).collect();
    order.sort_by(|&a, &b| compare_names(streams[a].0, streams[b].0));
    let n_entries = streams.len() + 1;
    let mut dir = vec![0u8; n_entries.div_ceil(ss / 128) * ss];
    let mut put = |idx: usize, name: &str, kind: u8, left: u32, right: u32, child: u32, start: u32, size: u32| {
        let e = idx * 128;
        let units: Vec<u16> = name.encode_utf16().collect();
        for (i, u) in units.iter().enumerate() {
            dir[e + i * 2..e + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        dir[e + 64..e + 66].copy_from_slice(&((units.len() as u16 + 1) * 2).to_le_bytes());
        dir[e + 66] = kind;
        dir[e + 68..e + 72].copy_from_slice(&left.to_le_bytes());
        dir[e + 72..e + 76].copy_from_slice(&right.to_le_bytes());
        dir[e + 76..e + 80].copy_from_slice(&child.to_le_bytes());
        dir[e + 116..e + 120].copy_from_slice(&start.to_le_bytes());
        dir[e + 120..e + 124].copy_from_slice(&size.to_le_bytes());
    };
    let first_child = order.first().map_or(NOSTREAM, |&i| i as u32 + 1);
    put(0, "Root Entry", 5, NOSTREAM, NOSTREAM, first_child, root_start, root_size);
    for (k, &i) in order.iter().enumerate() {
        let right = order.get(k + 1).map_or(NOSTREAM, |&j| j as u32 + 1);
        put(i + 1, streams[i].0, 2, NOSTREAM, right, NOSTREAM, starts[i], streams[i].1.len() as u32);
    }
    let dir_first = push_chain(&mut sectors, &mut fat, ss, &dir);
    // FAT and DIFAT sectors: enough FAT entries for every sector, including their own.
    let difat_for = |n_fat: usize| n_fat.saturating_sub(109).div_ceil(per - 1);
    let mut n_fat = 1;
    while sectors.len() + difat_for(n_fat) + n_fat > n_fat * per {
        n_fat += 1;
    }
    let n_difat = difat_for(n_fat);
    let difat_first = sectors.len() as u32;
    let fat_first = difat_first + n_difat as u32;
    fat.extend(std::iter::repeat_n(0xFFFF_FFFC, n_difat));
    fat.extend(std::iter::repeat_n(0xFFFF_FFFD, n_fat));
    fat.resize(n_fat * per, FREESECT);
    let fat_ids: Vec<u32> = (0..n_fat as u32).map(|k| fat_first + k).collect();
    for d in 0..n_difat {
        let mut v: Vec<u32> = fat_ids.iter().skip(109 + d * (per - 1)).take(per - 1).copied().collect();
        v.resize(per - 1, FREESECT);
        v.push(if d + 1 == n_difat { ENDOFCHAIN } else { difat_first + d as u32 + 1 });
        sectors.push(v.iter().flat_map(|x| x.to_le_bytes()).collect());
    }
    for k in 0..n_fat {
        sectors.push(fat[k * per..(k + 1) * per].iter().flat_map(|v| v.to_le_bytes()).collect());
    }
    let mut out = vec![0u8; ss];
    out[..8].copy_from_slice(&SIGNATURE);
    out[24..26].copy_from_slice(&0x3Eu16.to_le_bytes());
    out[26..28].copy_from_slice(&(if shift == 12 { 4u16 } else { 3 }).to_le_bytes());
    out[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes());
    out[30..32].copy_from_slice(&shift.to_le_bytes());
    out[32..34].copy_from_slice(&6u16.to_le_bytes());
    out[44..48].copy_from_slice(&(n_fat as u32).to_le_bytes());
    out[48..52].copy_from_slice(&dir_first.to_le_bytes());
    out[56..60].copy_from_slice(&mini_cutoff.to_le_bytes());
    out[60..64].copy_from_slice(&minifat_first.to_le_bytes());
    out[64..68].copy_from_slice(&minifat_sectors.to_le_bytes());
    out[68..72].copy_from_slice(&(if n_difat == 0 { ENDOFCHAIN } else { difat_first }).to_le_bytes());
    out[72..76].copy_from_slice(&(n_difat as u32).to_le_bytes());
    for i in 0..109 {
        let v = fat_ids.get(i).copied().unwrap_or(FREESECT);
        out[76 + i * 4..80 + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    for s in sectors {
        out.extend_from_slice(&s);
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn reads_streams_back() {
        let big: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
        let f = build(&[("PowerPoint Document", &big), ("Current User", b"hello"), ("Pictures", &[])]);
        let cf = CompoundFile::open(&f).unwrap();
        assert_eq!(cf.read_root_stream("PowerPoint Document").unwrap(), big);
        assert_eq!(cf.read_root_stream("current user").unwrap(), b"hello");
        assert_eq!(cf.read_root_stream("Pictures").unwrap(), b"");
        assert!(matches!(cf.read_root_stream("Nope"), Err(CfbError::NotFound(_))));
        assert_eq!(cf.root_names().len(), 3);
    }

    #[test]
    fn hostile_files_never_panic() {
        let f = build(&[("PowerPoint Document", &[7u8; 1500]), ("Current User", b"x")]);
        assert!(CompoundFile::open(b"PK\x03\x04").is_err());
        for at in (0..f.len()).step_by(4) {
            for v in [0u32, 1, 3, 0xFFFF_FFFA, 0xFFFF_FFFE, 0xFFFF_FFFF, 0x7FFF_FFFF] {
                let mut c = f.clone();
                c[at..at + 4].copy_from_slice(&v.to_le_bytes());
                if let Ok(cf) = CompoundFile::open(&c) {
                    let _ = cf.read_root_stream("PowerPoint Document");
                }
            }
        }
        for cut in [0usize, 100, 511, 512, 1000, 2000] {
            let _ = CompoundFile::open(&f[..cut.min(f.len())]);
        }
    }

    #[test]
    fn a_looping_chain_stops() {
        let mut f = build(&[("PowerPoint Document", &[1u8; 1500])]);
        // The FAT is the last sector; point sector 1's entry back at sector 0. The stream's size
        // would be reached by going round the loop, but repeated sectors are not the stream.
        let fat_at = f.len() - 512;
        f[fat_at + 4..fat_at + 8].copy_from_slice(&0u32.to_le_bytes());
        let cf = CompoundFile::open(&f).unwrap();
        assert!(cf.read_root_stream("PowerPoint Document").is_err());
        // The directory's chain looping back on itself is an error.
        let mut g = build(&[("PowerPoint Document", &[1u8; 10])]);
        let fat_at = g.len() - 512;
        g[fat_at + 4..fat_at + 8].copy_from_slice(&1u32.to_le_bytes());
        assert!(CompoundFile::open(&g).is_err());
    }

    /// Streams under the cutoff are read from the mini stream, through the mini FAT.
    #[test]
    fn reads_mini_streams() {
        let big: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let small: Vec<u8> = (0..200u8).collect();
        let f = build_with(&[("PowerPoint Document", &big), ("Current User", &small), ("Tiny", b"x")], 9, 4096);
        let cf = CompoundFile::open(&f).unwrap();
        assert!(!cf.minifat.is_empty() && !cf.mini_stream.is_empty(), "the small streams went in the mini stream");
        assert_eq!(cf.read_root_stream("PowerPoint Document").unwrap(), big);
        assert_eq!(cf.read_root_stream("Current User").unwrap(), small);
        assert_eq!(cf.read_root_stream("Tiny").unwrap(), b"x");
    }

    /// Version 4 files use 4096-byte sectors, the header filling the first.
    #[test]
    fn reads_4096_byte_sectors() {
        let big: Vec<u8> = (0..10_000u32).map(|i| (i % 253) as u8).collect();
        let f = build_with(&[("PowerPoint Document", &big), ("Current User", b"hello")], 12, 4096);
        let cf = CompoundFile::open(&f).unwrap();
        assert_eq!(cf.sector_size, 4096);
        assert_eq!(cf.read_root_stream("PowerPoint Document").unwrap(), big);
        assert_eq!(cf.read_root_stream("Current User").unwrap(), b"hello");
    }

    /// Past 109 FAT sectors (about 7 MB at 512-byte sectors), the rest are found through the
    /// DIFAT chain.
    #[test]
    fn reads_fat_sectors_listed_in_the_difat() {
        let big: Vec<u8> = (0..7_500_000u32).map(|i| (i % 249) as u8).collect();
        let f = build_with(&[("PowerPoint Document", &big)], 9, 0);
        assert_ne!(u32_at(&f, 68), Some(ENDOFCHAIN), "the file has a DIFAT chain");
        let cf = CompoundFile::open(&f).unwrap();
        assert_eq!(cf.read_root_stream("PowerPoint Document").unwrap(), big);
    }
}
