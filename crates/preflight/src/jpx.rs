//! PDF/A-2 and -3 6.2.8.3: JPEG 2000 images (`JPXDecode`, ISO 32000-1 7.4.9). The data has 1, 3 or
//! 4 colour channels (an opacity channel isn't one); of several colour specifications (`colr`
//! boxes) exactly one is marked as the best (`APPROX` 1); the one used has the method (`METH`) 1, 2
//! or 3 and, when enumerated, isn't colour space 19 (CIEJab); and the bit depth is 1 to 38, the
//! same for every channel.
//!
//! An image whose dictionary has `/ColorSpace` isn't checked: that colour space then decides how the
//! samples read, and the data's own colour specifications are ignored (ISO 32000-2 7.4.9).
//!
//! The values are read from the JP2 header box (ITU-T T.800 Annex I: `ihdr`, `bpcc` and `colr`,
//! I.5.3, with `cdef` and `pclr` for the channels), or, for data that is a bare codestream
//! without one, from its `SIZ` marker segment (A.5.1), which has no colour specifications. Only
//! the headers are read, never the image.

use pdfcraft_cos::{DecodeBudget, Dict, Document, Object, Stream};

/// How many boxes are read before the header is given up on, at the top level and in `jp2h`.
const MAX_BOXES: usize = 64;
/// The most bytes read from the data of an image whose `JPXDecode` follows other filters.
const MAX_PREFIX_DECODED: usize = 16 << 20;

/// What a JPEG 2000 image breaks of 6.2.8.3.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Jpx {
    /// The number of colour channels, when it isn't 1, 3 or 4.
    pub(crate) channels: Option<u16>,
    /// There are several colour specifications, and not exactly one has `APPROX` 1.
    pub(crate) best: bool,
    /// The method of the colour specification used, when it isn't 1, 2 or 3.
    pub(crate) method: Option<u8>,
    /// The colour specification used is the enumerated colour space 19 (CIEJab).
    pub(crate) cie_jab: bool,
    /// The bit depth is outside 1 to 38, or differs between channels.
    pub(crate) depth: bool,
}

/// One `colr` box: its method, its `APPROX` field and, for method 1, its enumerated colour space.
struct Colour {
    method: u8,
    approx: u8,
    enumerated: Option<u32>,
}

/// The boxes of `data`, as (type, contents), at most [`MAX_BOXES`]. A box whose length runs past
/// the data ends the list; a length of 0 runs to the end (T.800 I.4).
fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while out.len() < MAX_BOXES {
        let Some(head) = data.get(at..at.saturating_add(8)) else { break };
        let (Some(len), Some(kind)) = (head.first_chunk::<4>().map(|l| u32::from_be_bytes(*l)), head.get(4..8).and_then(|k| k.first_chunk::<4>()))
        else {
            break;
        };
        let (start, end) = match len {
            0 => (at.saturating_add(8), data.len()),
            1 => {
                let Some(long) =
                    data.get(at.saturating_add(8)..at.saturating_add(16)).and_then(|l| l.first_chunk::<8>()).map(|l| u64::from_be_bytes(*l))
                else {
                    break;
                };
                let Ok(long) = usize::try_from(long) else { break };
                (at.saturating_add(16), at.saturating_add(long))
            }
            len => {
                let Ok(len) = usize::try_from(len) else { break };
                (at.saturating_add(8), at.saturating_add(len))
            }
        };
        let Some(contents) = data.get(start..end).filter(|_| end >= start) else { break };
        out.push((*kind, contents));
        if end <= at {
            break;
        }
        at = end;
    }
    out
}

/// What the JPEG 2000 `data` (a JP2 file or a bare codestream) breaks of 6.2.8.3, or `None` when
/// its header can't be read.
pub(crate) fn check_data(data: &[u8]) -> Option<Jpx> {
    if data.starts_with(&[0xFF, 0x4F, 0xFF, 0x51]) {
        return codestream(data);
    }
    let top = boxes(data);
    let header = top.iter().find(|(kind, _)| kind == b"jp2h").map(|(_, c)| boxes(c))?;
    let ihdr = header.iter().find(|(kind, _)| kind == b"ihdr").map(|(_, c)| *c)?;
    let components = usize::from(u16::from_be_bytes(*ihdr.get(8..10)?.first_chunk::<2>()?));
    let bpc = *ihdr.get(10)?;
    // T.800 I.5.3.1: 255 means the components' depths differ, and the `bpcc` box gives each one's.
    let depths: Vec<u8> = if bpc == 0xFF {
        header.iter().find(|(kind, _)| kind == b"bpcc").map(|(_, c)| c.get(..components).unwrap_or(c).to_vec()).unwrap_or_default()
    } else {
        vec![bpc; components.min(MAX_CHANNELS)]
    };
    let (channels, unreadable) = channels(&header, &depths, components);
    let colour: Vec<u8> = channels.iter().filter(|c| c.colour).map(|c| c.depth).collect();
    let count = u16::try_from(colour.len()).unwrap_or(u16::MAX);
    let mut out = Jpx { channels: (!matches!(count, 1 | 3 | 4)).then_some(count), ..Jpx::default() };
    // Every channel's depth is within range, and the colour channels' are the same.
    out.depth = unreadable
        || (bpc == 0xFF && depths.len() != components)
        || channels.iter().any(|c| !in_range(c.depth))
        || colour.iter().any(|d| colour.first().is_some_and(|first| (first & 0x7F) != (d & 0x7F)));
    let colours: Vec<Colour> = header
        .iter()
        .filter(|(kind, _)| kind == b"colr")
        .filter_map(|(_, c)| {
            let method = *c.first()?;
            let approx = *c.get(2)?;
            let enumerated = (method == 1).then(|| c.get(3..7).and_then(|e| e.first_chunk::<4>()).map(|e| u32::from_be_bytes(*e))).flatten();
            Some(Colour { method, approx, enumerated })
        })
        .collect();
    let best: Vec<&Colour> = colours.iter().filter(|c| c.approx == 1).collect();
    out.best = colours.len() > 1 && best.len() != 1;
    // The specification used: the only one, or the one marked as the best.
    let used = match (colours.as_slice(), best.as_slice()) {
        ([only], _) => Some(only),
        (_, [best]) => Some(*best),
        _ => None,
    };
    if let Some(used) = used {
        out.method = (!matches!(used.method, 1..=3)).then_some(used.method);
        out.cie_jab = used.method == 1 && used.enumerated == Some(19);
    }
    Some(out)
}

/// The most channels read from a header.
const MAX_CHANNELS: usize = 16384;

/// One channel the decoded image has: its depth, as `bpcc` writes it, and whether it holds colour.
struct Channel {
    depth: u8,
    colour: bool,
}

/// The channels the header box's contents `header` describe, from the components' depths
/// `depths`, and whether a box that says how they are made couldn't be read.
///
/// - The component mapping (`cmap`, T.800 I.5.3.5) makes one channel of each four-byte entry: a
///   component directly, at its own depth, or through a column of the palette (`pclr`, I.5.3.4),
///   at that column's depth. Without one, a palette's columns are the channels, and otherwise each
///   component is one.
/// - The channel definitions (`cdef`, I.5.3.6) say which hold colour: type 0, and not opacity. A
///   channel they leave out doesn't. Without them, every channel holds colour.
fn channels(header: &[([u8; 4], &[u8])], depths: &[u8], components: usize) -> (Vec<Channel>, bool) {
    let find = |name: &[u8; 4]| header.iter().find(|(kind, _)| kind == name).map(|(_, c)| *c);
    // The palette's column depths: NE (2 bytes), NPC (1), then NPC depths.
    let palette: Option<&[u8]> = find(b"pclr").and_then(|p| p.get(3..3usize.saturating_add(usize::from(*p.get(2)?))));
    let mut unreadable = false;
    let mut out: Vec<Channel> = match (find(b"cmap"), palette) {
        (Some(cmap), _) => cmap
            .as_chunks::<4>()
            .0
            .iter()
            .take(MAX_CHANNELS)
            .map(|[c0, c1, mtyp, pcol]| {
                let component = usize::from(u16::from_be_bytes([*c0, *c1]));
                let depth = if *mtyp == 1 { palette.and_then(|p| p.get(usize::from(*pcol))) } else { depths.get(component) };
                unreadable |= depth.is_none();
                Channel { depth: depth.copied().unwrap_or(0), colour: true }
            })
            .collect(),
        (None, Some(palette)) => palette.iter().map(|&depth| Channel { depth, colour: true }).collect(),
        (None, None) => depths.iter().take(components.min(MAX_CHANNELS)).map(|&depth| Channel { depth, colour: true }).collect(),
    };
    if let Some(cdef) = find(b"cdef")
        && let Some(n) = cdef.first_chunk::<2>().map(|n| usize::from(u16::from_be_bytes(*n)))
    {
        for channel in &mut out {
            channel.colour = false;
        }
        // Each definition is a channel, a type and an association, two bytes each.
        for definition in cdef.get(2..).unwrap_or_default().as_chunks::<6>().0.iter().take(n) {
            let [c0, c1, t0, t1, _, _] = *definition;
            if let Some(channel) = out.get_mut(usize::from(u16::from_be_bytes([c0, c1]))) {
                channel.colour = [t0, t1] == [0, 0];
            }
        }
    }
    (out, unreadable)
}

/// A component's bit depth, as `ihdr`, `bpcc` and `SIZ` write it (the depth less one, with the
/// top bit for signed samples), within 1 to 38.
fn in_range(written: u8) -> bool {
    (1..=38).contains(&((written & 0x7F).saturating_add(1)))
}

/// A bare codestream: the `SIZ` segment right after `SOC` gives the components (`Csiz`) and each
/// one's depth (`Ssiz`, T.800 A.5.1).
fn codestream(data: &[u8]) -> Option<Jpx> {
    // SOC (2), SIZ marker (2), Lsiz (2), Rsiz (2), eight 4-byte sizes (32): Csiz at 40.
    let components = u16::from_be_bytes(*data.get(40..42)?.first_chunk::<2>()?);
    let depths: Vec<u8> = (0..usize::from(components)).map_while(|i| data.get(i.checked_mul(3)?.checked_add(42)?).copied()).collect();
    let depth = depths.len() != usize::from(components) || depths.iter().any(|d| d != depths.first().unwrap_or(d) || !in_range(*d));
    Some(Jpx { channels: (!matches!(components, 1 | 3 | 4)).then_some(components), depth, ..Jpx::default() })
}

/// The names of a stream's filters: one name or an array of them, each given directly or by
/// reference.
fn filter_names(doc: &Document, dict: &Dict) -> Vec<Vec<u8>> {
    let Some(filter) = dict.get(b"Filter").map(|f| doc.resolve(f)) else { return Vec::new() };
    match &*filter {
        Object::Name(n) => vec![n.clone()],
        Object::Array(a) => a.iter().filter_map(|f| doc.resolve(f).as_name().map(<[u8]>::to_vec)).collect(),
        _ => Vec::new(),
    }
}

/// What an image XObject `s` breaks of 6.2.8.3, when its last filter is `JPXDecode` and its data
/// has a header to read. Filters before `JPXDecode` are undone first, within `budget`.
pub(crate) fn check(doc: &Document, s: &Stream, budget: &mut DecodeBudget) -> Option<Jpx> {
    let mut filters = filter_names(doc, &s.dict);
    if filters.pop().as_deref() != Some(b"JPXDecode".as_slice()) {
        return None;
    }
    if filters.is_empty() {
        return check_data(&s.raw);
    }
    // The stages before `JPXDecode`, with their parameters.
    let mut dict = s.dict.clone();
    dict.set(b"Filter".to_vec(), Object::Array(filters.iter().map(|f| Object::Name(f.clone())).collect()));
    if let Some(parms) = s.dict.get(b"DecodeParms").map(|p| doc.resolve(p))
        && let Object::Array(parms) = &*parms
    {
        dict.set(b"DecodeParms".to_vec(), Object::Array(parms.iter().take(filters.len()).cloned().collect()));
    }
    let data = budget.decode_within(&Stream::from_raw(dict, s.raw.to_vec()), MAX_PREFIX_DECODED).ok()?;
    check_data(&data)
}
