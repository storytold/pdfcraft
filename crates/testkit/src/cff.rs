//! Synthetic CFF programs, built in memory: no font file is involved.
//!
//! [`cid_keyed`] writes a CID-keyed CFF program (Adobe Technical Note #5176 §18) whose glyphs
//! 1, 2, 3 … carry the CIDs it is given, as a converter that numbers glyphs by their names'
//! standard string ids writes one. Every glyph is empty (`endchar`); a PDF gives the widths,
//! for which [`helvetica_width`] gives a proportional typeface's, and [`standard_string_id`]
//! gives the ids such a converter numbers characters by.

/// A CFF INDEX of `items`, offsets two bytes wide.
fn index(items: &[Vec<u8>]) -> Vec<u8> {
    let count = u16::try_from(items.len()).unwrap_or(u16::MAX);
    let mut out = count.to_be_bytes().to_vec();
    if items.is_empty() {
        return out;
    }
    out.push(2);
    let mut at = 1usize;
    out.extend_from_slice(&u16::try_from(at).unwrap_or(u16::MAX).to_be_bytes());
    for item in items {
        at = at.saturating_add(item.len());
        out.extend_from_slice(&u16::try_from(at).unwrap_or(u16::MAX).to_be_bytes());
    }
    for item in items {
        out.extend_from_slice(item);
    }
    out
}

/// An integer operand in its five-byte form, so offsets don't change a DICT's size.
fn int(v: usize) -> Vec<u8> {
    let mut b = vec![29];
    b.extend_from_slice(&i32::try_from(v).unwrap_or(i32::MAX).to_be_bytes());
    b
}

/// A CID-keyed CFF program (registry `Adobe`, ordering `Identity`) with glyph 0 (`.notdef`, CID
/// 0) and then one empty glyph for each of `cids`, in order (charset format 0, every glyph in
/// font dict 0).
pub fn cid_keyed(cids: &[u16]) -> Vec<u8> {
    let n = cids.len().saturating_add(1);
    let header = vec![1, 0, 4, 1];
    let name = index(&[b"Synthetic".to_vec()]);
    // Custom strings start at SID 391: `Adobe`, `Identity`.
    let strings = index(&[b"Adobe".to_vec(), b"Identity".to_vec()]);
    let global_subrs = index(&[]);
    let charset: Vec<u8> = std::iter::once(0u8).chain(cids.iter().flat_map(|c| c.to_be_bytes())).collect();
    let fd_select: Vec<u8> = std::iter::once(0u8).chain(std::iter::repeat_n(0u8, n)).collect();
    let charstrings = index(&vec![vec![14u8]; n]);
    let private = vec![139, 20];
    let font_dict = |private_at: usize| [int(private.len()), int(private_at), vec![18]].concat();
    let fd_array_len = index(&[font_dict(0)]).len();
    let top = |charset_at: usize, fd_select_at: usize, charstrings_at: usize, fd_array_at: usize| {
        [
            int(391),
            int(392),
            int(0),
            vec![12, 30],
            int(n),
            vec![12, 34],
            int(charset_at),
            vec![15],
            int(charstrings_at),
            vec![17],
            int(fd_array_at),
            vec![12, 36],
            int(fd_select_at),
            vec![12, 37],
        ]
        .concat()
    };
    let top_len = index(&[top(0, 0, 0, 0)]).len();
    let start = header.len() + name.len() + top_len + strings.len() + global_subrs.len();
    let charset_at = start;
    let fd_select_at = charset_at + charset.len();
    let charstrings_at = fd_select_at + fd_select.len();
    let fd_array_at = charstrings_at + charstrings.len();
    let private_at = fd_array_at + fd_array_len;
    let top_index = index(&[top(charset_at, fd_select_at, charstrings_at, fd_array_at)]);
    let fd_array = index(&[font_dict(private_at)]);
    [header, name, top_index, strings, global_subrs, charset, fd_select, charstrings, fd_array, private].concat()
}

/// Helvetica's advance widths (its AFM, thousandths of an em) for letters, the space and a few
/// punctuation marks; 600 for anything else.
pub fn helvetica_width(c: char) -> u32 {
    const LOWER: [u32; 26] =
        [556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500];
    const UPPER: [u32; 26] =
        [667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611];
    match c {
        'a'..='z' => LOWER.get((c as usize).saturating_sub('a' as usize)).copied().unwrap_or(600),
        'A'..='Z' => UPPER.get((c as usize).saturating_sub('A' as usize)).copied().unwrap_or(600),
        ' ' | '.' | ',' | '(' | ')' => 278,
        '\u{2018}' | '\u{2019}' => 222,
        _ => 600,
    }
}

/// The CFF standard string id of a character (TN #5176 Appendix A), for printable ASCII and
/// the curly single quotes: `space` is 1, printable ASCII from `!` follows in order (`A` is 34,
/// `a` 66), the apostrophe is `quoteright` (8) and the opening quote `quoteleft` (65).
pub fn standard_string_id(c: char) -> u16 {
    match c {
        ' ' => 1,
        '\u{2019}' => 8,
        '\u{2018}' => 65,
        _ => u16::try_from(u32::from(c)).unwrap_or(0).saturating_sub(31),
    }
}
