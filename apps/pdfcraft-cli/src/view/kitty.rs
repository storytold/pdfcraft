//! Kitty graphics protocol escape codes (<https://sw.kovidgoyal.net/kitty/graphics-protocol/>).
//!
//! Pure: every function appends bytes to a buffer and the caller writes them out. Images go as
//! PNG (`f=100`) inside the escape codes themselves (`t=d`), which works over SSH too, or, for a
//! terminal on this machine, as raw pixels in a temporary file whose path the escape code names
//! (`t=t`). The terminal is told not to answer (`q=2`), so nothing lands on the user's command
//! line; only the pager's placements have errors answered (`q=1`), which the pager reads among
//! the keys.

use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use base64::Engine as _;

/// The largest base64 payload one escape code may carry. Every chunk but the last must be a
/// multiple of 4 bytes long, which this is.
const CHUNK: usize = 4096;

/// Where the escape codes go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wrap {
    /// Straight to the terminal.
    None,
    /// Through tmux: each code inside tmux's passthrough sequence, `ESC P tmux; … ESC \`, with
    /// every ESC in it doubled (tmux FAQ). tmux passes it on when the pane allows passthrough.
    Tmux,
}

/// Transmit a PNG and show it at the cursor over `cols` × `rows` cells, in one go (`a=T`). The
/// cursor ends after the image. The image gets no id: nothing refers to it again.
pub(crate) fn show_png(out: &mut Vec<u8>, png: &[u8], cols: u16, rows: u16) {
    chunked(out, &format!("a=T,f=100,t=d,q=2,c={cols},r={rows}"), png, Wrap::None);
}

/// Transmit a PNG as image `id` without showing it (`a=t`); placements show it. Transmitting an
/// id again replaces the image and deletes its placements.
pub(crate) fn transmit(out: &mut Vec<u8>, id: u32, png: &[u8], wrap: Wrap) {
    chunked(out, &format!("a=t,f=100,t=d,q=2,i={id}"), png, wrap);
}

/// Transmit image `id` from a temporary file of `width` × `height` straight RGBA pixels (`t=t`,
/// `f=32`), without showing it: the terminal reads the file, then deletes it. Only the path goes
/// in the escape code. For a terminal on this machine (`files.rs`).
pub(crate) fn transmit_file(out: &mut Vec<u8>, id: u32, path: &Path, width: u32, height: u32, wrap: Wrap) {
    let path = base64::engine::general_purpose::STANDARD.encode(path.as_os_str().as_bytes());
    command(out, &format!("a=t,f=32,t=t,s={width},v={height},q=2,i={id}"), path.as_bytes(), wrap);
}

/// A rectangle of an image's pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rect {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) w: u32,
    pub(crate) h: u32,
}

/// Show image `id` at the cursor over `cols` × `rows` cells, as its placement `placement`: all of
/// it, or the `src` rectangle of its pixels. Placing it again under the same placement moves it
/// instead of adding another; under two, it shows twice. The cursor stays where it is (`C=1`), so
/// drawing at the bottom never scrolls the screen. Errors are answered (`q=1`): the terminal says
/// when it no longer has the image ([`super::keys::Key::Missing`]).
pub(crate) fn place(out: &mut Vec<u8>, id: u32, placement: u32, cols: u16, rows: u16, src: Option<Rect>, wrap: Wrap) {
    let mut keys = format!("a=p,q=1,i={id},p={placement},C=1,c={cols},r={rows}");
    if let Some(r) = src {
        keys.push_str(&format!(",x={},y={},w={},h={}", r.x, r.y, r.w, r.h));
    }
    command(out, &keys, b"", wrap);
}

/// Delete image `id` and free its data (`d=I`).
pub(crate) fn delete(out: &mut Vec<u8>, id: u32, wrap: Wrap) {
    command(out, &format!("a=d,d=I,q=2,i={id}"), b"", wrap);
}

/// Take placement `placement` of image `id` off the screen but keep the image's data (`d=i`), so
/// placing it again needs no new transmission.
pub(crate) fn hide(out: &mut Vec<u8>, id: u32, placement: u32, wrap: Wrap) {
    command(out, &format!("a=d,d=i,q=2,i={id},p={placement}"), b"", wrap);
}

/// A virtual placement of image `id` over `cols` × `rows` cells (`U=1`): it shows wherever
/// [`placeholder_row`] cells for it are drawn.
pub(crate) fn place_virtual(out: &mut Vec<u8>, id: u32, cols: u16, rows: u16, wrap: Wrap) {
    command(out, &format!("a=p,U=1,q=2,i={id},c={cols},r={rows}"), b"", wrap);
}

/// The character every placeholder cell holds.
const PLACEHOLDER: char = '\u{10EEEE}';

/// Row and column numbers as combining marks: entry n stands for n. This is the protocol's
/// `rowcolumn-diacritics.txt`: the combining marks of class 230 in Unicode 6.0.0 that have no
/// decomposition mapping, less a few that normalization may fuse with the character before them.
#[rustfmt::skip]
const DIACRITICS: [char; 297] = [
    '\u{0305}', '\u{030D}', '\u{030E}', '\u{0310}', '\u{0312}', '\u{033D}', '\u{033E}', '\u{033F}', '\u{0346}', '\u{034A}',
    '\u{034B}', '\u{034C}', '\u{0350}', '\u{0351}', '\u{0352}', '\u{0357}', '\u{035B}', '\u{0363}', '\u{0364}', '\u{0365}',
    '\u{0366}', '\u{0367}', '\u{0368}', '\u{0369}', '\u{036A}', '\u{036B}', '\u{036C}', '\u{036D}', '\u{036E}', '\u{036F}',
    '\u{0483}', '\u{0484}', '\u{0485}', '\u{0486}', '\u{0487}', '\u{0592}', '\u{0593}', '\u{0594}', '\u{0595}', '\u{0597}',
    '\u{0598}', '\u{0599}', '\u{059C}', '\u{059D}', '\u{059E}', '\u{059F}', '\u{05A0}', '\u{05A1}', '\u{05A8}', '\u{05A9}',
    '\u{05AB}', '\u{05AC}', '\u{05AF}', '\u{05C4}', '\u{0610}', '\u{0611}', '\u{0612}', '\u{0613}', '\u{0614}', '\u{0615}',
    '\u{0616}', '\u{0617}', '\u{0657}', '\u{0658}', '\u{0659}', '\u{065A}', '\u{065B}', '\u{065D}', '\u{065E}', '\u{06D6}',
    '\u{06D7}', '\u{06D8}', '\u{06D9}', '\u{06DA}', '\u{06DB}', '\u{06DC}', '\u{06DF}', '\u{06E0}', '\u{06E1}', '\u{06E2}',
    '\u{06E4}', '\u{06E7}', '\u{06E8}', '\u{06EB}', '\u{06EC}', '\u{0730}', '\u{0732}', '\u{0733}', '\u{0735}', '\u{0736}',
    '\u{073A}', '\u{073D}', '\u{073F}', '\u{0740}', '\u{0741}', '\u{0743}', '\u{0745}', '\u{0747}', '\u{0749}', '\u{074A}',
    '\u{07EB}', '\u{07EC}', '\u{07ED}', '\u{07EE}', '\u{07EF}', '\u{07F0}', '\u{07F1}', '\u{07F3}', '\u{0816}', '\u{0817}',
    '\u{0818}', '\u{0819}', '\u{081B}', '\u{081C}', '\u{081D}', '\u{081E}', '\u{081F}', '\u{0820}', '\u{0821}', '\u{0822}',
    '\u{0823}', '\u{0825}', '\u{0826}', '\u{0827}', '\u{0829}', '\u{082A}', '\u{082B}', '\u{082C}', '\u{082D}', '\u{0951}',
    '\u{0953}', '\u{0954}', '\u{0F82}', '\u{0F83}', '\u{0F86}', '\u{0F87}', '\u{135D}', '\u{135E}', '\u{135F}', '\u{17DD}',
    '\u{193A}', '\u{1A17}', '\u{1A75}', '\u{1A76}', '\u{1A77}', '\u{1A78}', '\u{1A79}', '\u{1A7A}', '\u{1A7B}', '\u{1A7C}',
    '\u{1B6B}', '\u{1B6D}', '\u{1B6E}', '\u{1B6F}', '\u{1B70}', '\u{1B71}', '\u{1B72}', '\u{1B73}', '\u{1CD0}', '\u{1CD1}',
    '\u{1CD2}', '\u{1CDA}', '\u{1CDB}', '\u{1CE0}', '\u{1DC0}', '\u{1DC1}', '\u{1DC3}', '\u{1DC4}', '\u{1DC5}', '\u{1DC6}',
    '\u{1DC7}', '\u{1DC8}', '\u{1DC9}', '\u{1DCB}', '\u{1DCC}', '\u{1DD1}', '\u{1DD2}', '\u{1DD3}', '\u{1DD4}', '\u{1DD5}',
    '\u{1DD6}', '\u{1DD7}', '\u{1DD8}', '\u{1DD9}', '\u{1DDA}', '\u{1DDB}', '\u{1DDC}', '\u{1DDD}', '\u{1DDE}', '\u{1DDF}',
    '\u{1DE0}', '\u{1DE1}', '\u{1DE2}', '\u{1DE3}', '\u{1DE4}', '\u{1DE5}', '\u{1DE6}', '\u{1DFE}', '\u{20D0}', '\u{20D1}',
    '\u{20D4}', '\u{20D5}', '\u{20D6}', '\u{20D7}', '\u{20DB}', '\u{20DC}', '\u{20E1}', '\u{20E7}', '\u{20E9}', '\u{20F0}',
    '\u{2CEF}', '\u{2CF0}', '\u{2CF1}', '\u{2DE0}', '\u{2DE1}', '\u{2DE2}', '\u{2DE3}', '\u{2DE4}', '\u{2DE5}', '\u{2DE6}',
    '\u{2DE7}', '\u{2DE8}', '\u{2DE9}', '\u{2DEA}', '\u{2DEB}', '\u{2DEC}', '\u{2DED}', '\u{2DEE}', '\u{2DEF}', '\u{2DF0}',
    '\u{2DF1}', '\u{2DF2}', '\u{2DF3}', '\u{2DF4}', '\u{2DF5}', '\u{2DF6}', '\u{2DF7}', '\u{2DF8}', '\u{2DF9}', '\u{2DFA}',
    '\u{2DFB}', '\u{2DFC}', '\u{2DFD}', '\u{2DFE}', '\u{2DFF}', '\u{A66F}', '\u{A67C}', '\u{A67D}', '\u{A6F0}', '\u{A6F1}',
    '\u{A8E0}', '\u{A8E1}', '\u{A8E2}', '\u{A8E3}', '\u{A8E4}', '\u{A8E5}', '\u{A8E6}', '\u{A8E7}', '\u{A8E8}', '\u{A8E9}',
    '\u{A8EA}', '\u{A8EB}', '\u{A8EC}', '\u{A8ED}', '\u{A8EE}', '\u{A8EF}', '\u{A8F0}', '\u{A8F1}', '\u{AAB0}', '\u{AAB2}',
    '\u{AAB3}', '\u{AAB7}', '\u{AAB8}', '\u{AABE}', '\u{AABF}', '\u{AAC1}', '\u{FE20}', '\u{FE21}', '\u{FE22}', '\u{FE23}',
    '\u{FE24}', '\u{FE25}', '\u{FE26}', '\u{10A0F}', '\u{10A38}', '\u{1D185}', '\u{1D186}', '\u{1D187}', '\u{1D188}', '\u{1D189}',
    '\u{1D1AA}', '\u{1D1AB}', '\u{1D1AC}', '\u{1D1AD}', '\u{1D242}', '\u{1D243}', '\u{1D244}',
];

/// The most rows or columns placeholder cells can number.
pub(crate) const MAX_CELLS: u16 = 297;
const _: () = assert!(DIACRITICS.len() == MAX_CELLS as usize);

/// How placeholder cells name their image: the foreground colour carries the id's low bits, and
/// a third diacritic its high byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IdColor {
    /// 24-bit colour: the low 24 bits.
    Rgb,
    /// The 256-colour palette, where 24-bit colours would arrive changed: the low 8 bits.
    Indexed,
}

impl IdColor {
    /// The largest value the colour itself carries; colour 0 is never used.
    fn max_low(self) -> u32 {
        match self {
            Self::Rgb => 0x00FF_FFFF,
            Self::Indexed => 0xFF,
        }
    }

    /// The `index`th id this colour can name, wrapping after the last: every low value from 1 up,
    /// under each high byte in turn. 65,280 ids in the palette, 4,278,190,080 in 24-bit colour.
    pub(crate) fn id(self, index: u64) -> u32 {
        let low = u64::from(self.max_low());
        let index = index % (low * 256);
        // `index / low` is below 256 and `index % low + 1` at most `low`, so the id fits a u32.
        u32::try_from((index / low) << 24 | (index % low + 1)).unwrap_or(1)
    }
}

/// Row `row` of image `id`'s placeholder cells, `cols` of them, coloured to name the image.
/// `false`, with nothing written, when `row` or `cols` is past [`MAX_CELLS`]. Ids that `color`
/// can't carry ([`IdColor::id`] makes the ones it can) lose bits.
///
/// Only the first cell carries diacritics (row, column 0, and the id's high byte when it has
/// one). The protocol lets the others go bare: a placeholder with none, in the same colour as the
/// cell to its left, takes that cell's row and high byte and the next column. That makes a row
/// about a third the size, which matters inside tmux: it throws output away when the terminal
/// falls behind, and a screenful of fully marked cells was enough to make it do so.
#[must_use]
pub(crate) fn placeholder_row(out: &mut Vec<u8>, id: u32, color: IdColor, row: u16, cols: u16) -> bool {
    let Some(&row_mark) = DIACRITICS.get(usize::from(row)) else { return false };
    if usize::from(cols) > DIACRITICS.len() {
        return false;
    }
    // The high byte goes in a third diacritic, which only ids above 24 bits need.
    let high = DIACRITICS.get(usize::try_from(id >> 24).unwrap_or(0)).copied().filter(|_| id >> 24 != 0);
    let colour = match color {
        IdColor::Rgb => format!("\x1b[38;2;{};{};{}m", (id >> 16) & 0xff, (id >> 8) & 0xff, id & 0xff),
        IdColor::Indexed => format!("\x1b[38;5;{}m", id & 0xff),
    };
    out.extend_from_slice(colour.as_bytes());
    let mut utf8 = [0u8; 4];
    if cols > 0 {
        for c in [PLACEHOLDER, row_mark, DIACRITICS[0]].into_iter().chain(high) {
            out.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
        }
    }
    for _ in 1..cols {
        out.extend_from_slice(PLACEHOLDER.encode_utf8(&mut utf8).as_bytes());
    }
    out.extend_from_slice(b"\x1b[39m");
    true
}

/// The bytes that base64 to exactly one full chunk.
const RAW_CHUNK: usize = CHUNK / 4 * 3;

/// Send `data` in base64 as one escape code per chunk: `m=1` on all but the last, `m=0` on the
/// last. Only the first carries `keys`; the protocol allows later chunks only `m` and `q`. Each
/// chunk is encoded as it goes, so no copy of the whole payload is ever held.
fn chunked(out: &mut Vec<u8>, keys: &str, data: &[u8], wrap: Wrap) {
    let mut chunks = data.chunks(RAW_CHUNK).peekable();
    if chunks.peek().is_none() {
        command(out, keys, b"", wrap);
        return;
    }
    let mut first = true;
    let mut encoded = String::with_capacity(CHUNK);
    while let Some(chunk) = chunks.next() {
        encoded.clear();
        base64::engine::general_purpose::STANDARD.encode_string(chunk, &mut encoded);
        let more = u8::from(chunks.peek().is_some());
        let control = if first { format!("{keys},m={more}") } else { format!("q=2,m={more}") };
        command(out, &control, encoded.as_bytes(), wrap);
        first = false;
    }
}

/// One escape code: `ESC _G <control> ; <payload> ESC \`, wrapped for tmux when asked. Each
/// chunk gets a wrapper of its own, which keeps every passthrough sequence small.
fn command(out: &mut Vec<u8>, control: &str, payload: &[u8], wrap: Wrap) {
    let mut code = Vec::with_capacity(control.len().saturating_add(payload.len()).saturating_add(6));
    code.extend_from_slice(b"\x1b_G");
    code.extend_from_slice(control.as_bytes());
    if !payload.is_empty() {
        code.push(b';');
        code.extend_from_slice(payload);
    }
    code.extend_from_slice(b"\x1b\\");
    match wrap {
        Wrap::None => out.extend_from_slice(&code),
        Wrap::Tmux => {
            out.extend_from_slice(b"\x1bPtmux;");
            for b in code {
                if b == 0x1b {
                    out.push(0x1b);
                }
                out.push(b);
            }
            out.extend_from_slice(b"\x1b\\");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The escape codes in `bytes`, as (control, payload) pairs. Panics on anything else.
    fn codes(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let text = std::str::from_utf8(bytes).unwrap();
        let mut out = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            let body = rest.strip_prefix("\x1b_G").unwrap_or_else(|| panic!("not an escape code: {rest:?}"));
            let end = body.find("\x1b\\").unwrap();
            let (control, payload) = body[..end].split_once(';').unwrap_or((&body[..end], ""));
            out.push((control.to_string(), payload.as_bytes().to_vec()));
            rest = &body[end + 2..];
        }
        out
    }

    fn decoded(codes: &[(String, Vec<u8>)]) -> Vec<u8> {
        let payload: Vec<u8> = codes.iter().flat_map(|(_, p)| p.clone()).collect();
        base64::engine::general_purpose::STANDARD.decode(payload).unwrap()
    }

    #[test]
    fn a_payload_that_fills_one_chunk_is_one_code() {
        // 3072 bytes are exactly 4096 base64 characters.
        let data = vec![7u8; 3072];
        let mut out = Vec::new();
        show_png(&mut out, &data, 40, 20);
        let codes = codes(&out);
        assert_eq!(codes.len(), 1);
        assert_eq!(codes[0].0, "a=T,f=100,t=d,q=2,c=40,r=20,m=0");
        assert_eq!(codes[0].1.len(), 4096);
        assert_eq!(decoded(&codes), data);
    }

    #[test]
    fn a_longer_payload_continues_with_only_m_and_q() {
        // 3073 bytes are 4100 base64 characters: a full chunk, then 4.
        let data: Vec<u8> = (0..3073u32).map(|i| (i % 251) as u8).collect();
        let mut out = Vec::new();
        show_png(&mut out, &data, 3, 2);
        let codes = codes(&out);
        assert_eq!(codes.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(), ["a=T,f=100,t=d,q=2,c=3,r=2,m=1", "q=2,m=0"]);
        assert_eq!(codes.iter().map(|(_, p)| p.len()).collect::<Vec<_>>(), [4096, 4]);
        assert_eq!(decoded(&codes), data);
    }

    #[test]
    fn every_chunk_but_the_last_is_a_multiple_of_four() {
        for len in [1, 2, 3, 4, 3071, 3072, 3073, 9000, 12_288, 12_289] {
            let data = vec![1u8; len];
            let mut out = Vec::new();
            show_png(&mut out, &data, 1, 1);
            let codes = codes(&out);
            let (last, rest) = codes.split_last().unwrap();
            assert!(last.0.ends_with("m=0"), "{len}: {}", last.0);
            for (control, payload) in rest {
                assert!(control.ends_with("m=1"), "{len}: {control}");
                assert_eq!(payload.len(), CHUNK, "{len}");
            }
            assert_eq!(decoded(&codes), data, "{len}");
        }
    }

    #[test]
    fn a_file_transmission_carries_only_its_path() {
        let path = std::path::Path::new("/tmp/tty-graphics-protocol-pdfcraft-7-0");
        let mut out = Vec::new();
        transmit_file(&mut out, 42, path, 1530, 1980, Wrap::None);
        let codes = codes(&out);
        assert_eq!(codes.len(), 1);
        assert_eq!(codes[0].0, "a=t,f=32,t=t,s=1530,v=1980,q=2,i=42");
        assert_eq!(decoded(&codes), path.as_os_str().as_bytes());
        // Through tmux, the same code wrapped.
        let mut tmux = Vec::new();
        transmit_file(&mut tmux, 42, path, 1530, 1980, Wrap::Tmux);
        assert_eq!(unwrap_tmux(&tmux), out);
    }

    #[test]
    fn an_empty_payload_still_sends_its_keys_once() {
        let mut out = Vec::new();
        show_png(&mut out, b"", 1, 1);
        assert_eq!(out, b"\x1b_Ga=T,f=100,t=d,q=2,c=1,r=1\x1b\\");
    }

    /// The bytes inside tmux passthrough sequences, unwrapped. Panics on anything else.
    fn unwrap_tmux(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            rest = rest.strip_prefix(b"\x1bPtmux;").expect("a passthrough sequence");
            let mut i = 0;
            loop {
                match (rest[i], rest.get(i + 1)) {
                    (0x1b, Some(0x1b)) => {
                        out.push(0x1b);
                        i += 2;
                    }
                    (0x1b, Some(b'\\')) => {
                        i += 2;
                        break;
                    }
                    (0x1b, _) => panic!("a lone ESC inside a passthrough sequence"),
                    (b, _) => {
                        out.push(b);
                        i += 1;
                    }
                }
            }
            rest = &rest[i..];
        }
        out
    }

    #[test]
    fn tmux_gets_each_code_wrapped_with_its_escapes_doubled() {
        let png = vec![9u8; 5000];
        let (mut plain, mut wrapped) = (Vec::new(), Vec::new());
        transmit(&mut plain, 7, &png, Wrap::None);
        transmit(&mut wrapped, 7, &png, Wrap::Tmux);
        assert_eq!(unwrap_tmux(&wrapped), plain);
        // 5000 bytes are two chunks, each in a wrapper of its own.
        assert_eq!(wrapped.windows(7).filter(|w| w == b"\x1bPtmux;").count(), 2);
        let codes = codes(&plain);
        assert_eq!(codes[0].0, "a=t,f=100,t=d,q=2,i=7,m=1");
        assert_eq!(decoded(&codes), png);
    }

    #[test]
    fn a_placement_shows_all_or_part_of_an_image_without_moving_the_cursor() {
        let mut out = Vec::new();
        place(&mut out, 9, 1, 40, 20, None, Wrap::None);
        assert_eq!(out, b"\x1b_Ga=p,q=1,i=9,p=1,C=1,c=40,r=20\x1b\\");
        out.clear();
        place(&mut out, 9, 2, 40, 10, Some(Rect { x: 0, y: 300, w: 800, h: 400 }), Wrap::None);
        assert_eq!(out, b"\x1b_Ga=p,q=1,i=9,p=2,C=1,c=40,r=10,x=0,y=300,w=800,h=400\x1b\\");
        out.clear();
        delete(&mut out, 9, Wrap::Tmux);
        assert_eq!(out, b"\x1bPtmux;\x1b\x1b_Ga=d,d=I,q=2,i=9\x1b\x1b\\\x1b\\");
        out.clear();
        // Lowercase: the placement goes, the data stays for the next time.
        hide(&mut out, 9, 2, Wrap::None);
        assert_eq!(out, b"\x1b_Ga=d,d=i,q=2,i=9,p=2\x1b\\");
    }

    #[test]
    fn a_virtual_placement_names_its_image_and_size() {
        let mut out = Vec::new();
        place_virtual(&mut out, 42, 30, 12, Wrap::None);
        assert_eq!(out, b"\x1b_Ga=p,U=1,q=2,i=42,c=30,r=12\x1b\\");
    }

    #[test]
    fn placeholder_rows_are_the_specification_example_with_what_can_be_inferred_left_out() {
        // The protocol's 2 × 2 example for image 42, in the 256-colour palette, marks every cell:
        // row 0 is 10EEEE 0305 0305, 10EEEE 0305 030D. The second cell's marks say what the
        // protocol infers for a bare cell after one in the same colour (same row, next column),
        // so it goes without.
        let row = |r| {
            let mut out = Vec::new();
            assert!(placeholder_row(&mut out, 42, IdColor::Indexed, r, 2));
            String::from_utf8(out).unwrap()
        };
        assert_eq!(row(0), "\x1b[38;5;42m\u{10EEEE}\u{0305}\u{0305}\u{10EEEE}\x1b[39m");
        assert_eq!(row(1), "\x1b[38;5;42m\u{10EEEE}\u{030D}\u{0305}\u{10EEEE}\x1b[39m");
        // A wide row stays small: 4 bytes a cell after the first.
        let mut wide = Vec::new();
        assert!(placeholder_row(&mut wide, 42, IdColor::Indexed, 3, 200));
        assert_eq!(wide.len(), "\x1b[38;5;42m".len() + 4 + 2 + 2 + 199 * 4 + "\x1b[39m".len());
        let mut none = Vec::new();
        assert!(placeholder_row(&mut none, 42, IdColor::Indexed, 0, 0));
        assert_eq!(none, b"\x1b[38;5;42m\x1b[39m");
    }

    #[test]
    fn a_24_bit_colour_carries_the_low_24_bits() {
        let mut out = Vec::new();
        assert!(placeholder_row(&mut out, 0x12_34_56, IdColor::Rgb, 0, 1));
        assert_eq!(out, "\x1b[38;2;18;52;86m\u{10EEEE}\u{0305}\u{0305}\x1b[39m".as_bytes());
    }

    #[test]
    fn a_third_diacritic_carries_the_high_byte() {
        // The protocol's example: 33554474 = 42 + (2 << 24), its high byte 2 being U+030E.
        let mut out = Vec::new();
        assert!(placeholder_row(&mut out, 33_554_474, IdColor::Indexed, 0, 2));
        // The second cell inherits the high byte from the first.
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b[38;5;42m\u{10EEEE}\u{0305}\u{0305}\u{030E}\u{10EEEE}\x1b[39m");
    }

    #[test]
    fn ids_cover_every_low_value_under_every_high_byte() {
        assert_eq!(IdColor::Indexed.id(0), 1);
        assert_eq!(IdColor::Indexed.id(254), 255);
        assert_eq!(IdColor::Indexed.id(255), (1 << 24) | 1);
        assert_eq!(IdColor::Indexed.id(255 * 256 - 1), (255 << 24) | 255);
        assert_eq!(IdColor::Indexed.id(255 * 256), 1, "wraps");
        assert_eq!(IdColor::Rgb.id(0xFF_FFFF), (1 << 24) | 1);
        assert_eq!(IdColor::Rgb.id(u64::MAX), IdColor::Rgb.id(u64::MAX % (0xFF_FFFF * 256)));
        // Never 0, and the colour part never 0, for any index.
        for index in [0, 1, 254, 255, 256, 65_279, 65_280, 1 << 40, u64::MAX] {
            for colour in [IdColor::Indexed, IdColor::Rgb] {
                let id = colour.id(index);
                assert_ne!(id & colour.max_low(), 0, "{colour:?} {index}: {id:#x}");
            }
        }
        let distinct: std::collections::HashSet<u32> = (0..65_280).map(|i| IdColor::Indexed.id(i)).collect();
        assert_eq!(distinct.len(), 65_280);
    }

    #[test]
    fn placeholders_stop_at_the_end_of_the_table() {
        let mut out = Vec::new();
        assert!(placeholder_row(&mut out, 1, IdColor::Indexed, MAX_CELLS - 1, MAX_CELLS));
        assert_eq!(String::from_utf8(out.clone()).unwrap().chars().filter(|c| *c == PLACEHOLDER).count(), 297);
        out.clear();
        assert!(!placeholder_row(&mut out, 1, IdColor::Indexed, MAX_CELLS, 1));
        assert!(!placeholder_row(&mut out, 1, IdColor::Indexed, 0, MAX_CELLS + 1));
        assert!(out.is_empty());
    }

    #[test]
    fn the_diacritics_are_the_specifications() {
        let distinct: std::collections::HashSet<char> = DIACRITICS.into_iter().collect();
        assert_eq!(distinct.len(), DIACRITICS.len());
        // rowcolumn-diacritics.txt's first two and last entries.
        assert_eq!((DIACRITICS[0], DIACRITICS[1], DIACRITICS[296]), ('\u{0305}', '\u{030D}', '\u{1D244}'));
    }
}
