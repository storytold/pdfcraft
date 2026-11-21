//! Synthetic Type 1 programs, built in memory: no font file is involved.
//!
//! [`program`] writes a Type 1 font (Adobe Type 1 Font Format, chapters 2, 6 and 7) whose glyphs
//! are polygons, keyed by name, with the built-in encoding `StandardEncoding`: the private
//! dictionary and the charstrings `eexec`- and charstring-encrypted (`lenIV` 4), the encrypted
//! part in hexadecimal. [`Shape`] leaves out what producers sometimes leave out: the `/Subrs`
//! array, or the 512 zeros and `cleartomark` after the encrypted part.

/// What [`program`] writes besides the glyphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    /// Whether the private dictionary has a `/Subrs` array (four subroutines that only return).
    pub subrs: bool,
    /// Whether the encrypted part is followed by 512 zeros and `cleartomark`.
    pub trailer: bool,
}

impl Shape {
    /// A complete program.
    pub const COMPLETE: Self = Self { subrs: true, trailer: true };
}

/// A charstring number (Type 1 Font Format §6.2).
fn number(out: &mut Vec<u8>, v: i32) {
    match v {
        -107..=107 => out.push(u8::try_from(v + 139).unwrap_or(139)),
        108..=1131 => {
            let w = v - 108;
            out.push(u8::try_from(w / 256 + 247).unwrap_or(247));
            out.push(u8::try_from(w % 256).unwrap_or(0));
        }
        -1131..=-108 => {
            let w = -v - 108;
            out.push(u8::try_from(w / 256 + 251).unwrap_or(251));
            out.push(u8::try_from(w % 256).unwrap_or(0));
        }
        _ => {
            out.push(255);
            out.extend_from_slice(&v.to_be_bytes());
        }
    }
}

/// Type 1 encryption (§7.1) of `plain` with key `r`, after `skip` zero bytes.
fn encrypt(plain: &[u8], mut r: u16, skip: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(plain.len().saturating_add(skip));
    for &p in std::iter::repeat_n(&0u8, skip).chain(plain) {
        let c = p ^ u8::try_from(r >> 8).unwrap_or(0);
        r = u16::from(c).wrapping_add(r).wrapping_mul(52845).wrapping_add(22719);
        out.push(c);
    }
    out
}

/// The charstring of a glyph drawing `contours` (each a polygon), 600 units wide.
fn charstring(contours: crate::Outline<'_>) -> Vec<u8> {
    let mut cs = Vec::new();
    number(&mut cs, 0);
    number(&mut cs, 600);
    cs.push(13); // hsbw
    let mut at = (0i32, 0i32);
    for contour in contours {
        for (i, &(x, y)) in contour.iter().enumerate() {
            let (x, y) = (i32::from(x), i32::from(y));
            number(&mut cs, x - at.0);
            number(&mut cs, y - at.1);
            cs.push(if i == 0 { 21 } else { 5 }); // rmoveto, rlineto
            at = (x, y);
        }
        cs.push(9); // closepath
    }
    cs.push(14); // endchar
    encrypt(&cs, 4330, 4)
}

/// A Type 1 program named `Synthetic` whose glyphs are `glyphs` (name and polygons), with a
/// `.notdef` first, as [`Shape`] says.
pub fn program(glyphs: &[(&str, crate::Outline<'_>)], shape: Shape) -> Vec<u8> {
    let mut private: Vec<u8> = b"dup /Private 8 dict dup begin\n/RD{string currentfile exch readstring pop}executeonly def\n/ND{noaccess def}executeonly def\n/NP{noaccess put}executeonly def\n/lenIV 4 def\n".to_vec();
    if shape.subrs {
        private.extend_from_slice(b"/Subrs 4 array\n");
        for n in 0..4 {
            let subr = encrypt(&[11], 4330, 4); // return
            private.extend_from_slice(format!("dup {n} {} RD ", subr.len()).as_bytes());
            private.extend_from_slice(&subr);
            private.extend_from_slice(b" NP\n");
        }
        private.extend_from_slice(b"ND\n");
    }
    private.extend_from_slice(format!("2 index /CharStrings {} dict dup begin\n", glyphs.len().saturating_add(1)).as_bytes());
    for (name, contours) in std::iter::once((".notdef", &[][..])).chain(glyphs.iter().copied()) {
        let cs = charstring(contours);
        private.extend_from_slice(format!("/{name} {} RD ", cs.len()).as_bytes());
        private.extend_from_slice(&cs);
        private.extend_from_slice(b" ND\n");
    }
    private.extend_from_slice(b"end\nend\nreadonly put\nnoaccess put\ndup /FontName get exch definefont pop\nmark currentfile closefile\n");
    let encrypted = encrypt(&private, 55665, 4);
    let mut out = b"%!PS-AdobeFont-1.0: Synthetic 001\n11 dict begin\n/FontName /Synthetic def\n/FontType 1 def\n/PaintType 0 def\n/FontMatrix [0.001 0 0 0.001 0 0] readonly def\n/FontBBox {0 -200 1000 900} readonly def\n/Encoding StandardEncoding def\ncurrentdict end\ncurrentfile eexec\n".to_vec();
    for line in encrypted.chunks(32) {
        for b in line {
            out.extend_from_slice(format!("{b:02x}").as_bytes());
        }
        out.push(b'\n');
    }
    if shape.trailer {
        for _ in 0..8 {
            out.extend_from_slice(&[b'0'; 64]);
            out.push(b'\n');
        }
        out.extend_from_slice(b"cleartomark\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_take_their_shortest_form() {
        let mut out = Vec::new();
        for v in [0, 107, 108, 1131, -108, -1131, 1132] {
            number(&mut out, v);
        }
        assert_eq!(out, [139, 246, 247, 0, 250, 255, 251, 0, 254, 255, 255, 0, 0, 4, 108]);
    }

    #[test]
    fn the_program_is_type1_with_its_parts() {
        let square: &[(i16, i16)] = &[(0, 0), (500, 0), (500, 500), (0, 500)];
        let font = program(&[("a", &[square])], Shape::COMPLETE);
        assert!(font.starts_with(b"%!PS-AdobeFont-1.0"));
        assert!(font.ends_with(b"cleartomark\n"));
        let bare = program(&[("a", &[square])], Shape { subrs: false, trailer: false });
        assert!(!bare.ends_with(b"cleartomark\n") && bare.len() < font.len());
    }
}
