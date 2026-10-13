//! Type 1 program subsetting: filter a font's charstrings to the ones the
//! export actually shows, and re-emit a valid PFA (Layer B, Task 8).
//!
//! Layer A embedded `/FontFile` programs verbatim, one full copy per slice.
//! Real programs carry hundreds of charstrings while a page shows a handful,
//! so this module eexec-decrypts the private section, drops the charstrings
//! no slice references, and re-encrypts what remains. The output is a PFA in
//! exactly the shape Layer A gs-verified (10.08.0, build 2026-09-08, this
//! repo's oracle):
//!
//! - the cleartext header verbatim, through the `eexec` token;
//! - the re-encrypted private section as **one unbroken hex line** — gs's
//!   resync after each charstring `readstring` mis-handles newlines inside
//!   the hex (verified in Layer A: the same bytes wrapped fail to load);
//! - each charstring's definer (`ND`) **immediately** after its data — the
//!   same resync mis-reads even one space;
//! - the classic trailer: eight lines of 64 zeros and `cleartomark`.
//!
//! What is *not* re-encrypted: charstring ciphertext rides verbatim, so the
//! seed-4330 layer (and the private dict's `/lenIV`, default 4, which
//! parameterises it) needs no re-reading — the `/lenIV` line stays where the
//! font wrote it, in the verbatim prefix. `Subrs`, `OtherSubrs` and every
//! other private entry survive the same way, in whatever position the font
//! put them (prefix or suffix of the charstring block), so a kept charstring
//! still finds its subroutines. The tokens the font used to read and define
//! charstrings (`RD`/`-|`, `ND`/`|-`) are captured from the first entry and
//! re-used verbatim, and the block's shape between the entry count and
//! `begin` (` dict dup begin`, however the font spelled it) is copied, not
//! normalised — the tail's operand-stack arithmetic depends on it.
//!
//! Inputs are the three serialisations a `/FontFile` stream carries: PFA
//! with hex eexec (this repo's fixtures), PFA with binary eexec, and PFB
//! (`0x80`-segmented) — the same set skrifa's `Type1Font` accepts, which is
//! what gates the strategy arm that calls here. The ASCII zeros trailer
//! after the private section is discarded (its bytes decrypt as garbage; the
//! re-emission writes a fresh trailer).
//!
//! `None` on any parse failure — a program that cannot be split and
//! re-assembled safely embeds verbatim instead (Layer A behaviour: correct
//! output, only larger, no note). The same fallback covers `seac`, the
//! standard-encoding accent composition Adobe-era Type 1 fonts build
//! é/ü/à from: a composing charstring references its base and accent
//! glyphs *by name*, so a subset that drops them would break the glyph in
//! the consumer with no note. Any kept charstring whose byte shape looks
//! like `seac` (see [`may_compose_accents`]) declines the whole subset.
//! True retention — resolving bchar/achar through StandardEncoding into the
//! kept set — is the future refinement.
//!
//! Hostile inputs follow AGENTS §4: every offset and length is validated
//! against the input with `get()` and checked arithmetic before use, an
//! entry claiming more bytes than the section carries declines (never
//! allocates by the claimed count), and the byte scans are linear over
//! input-bounded ranges. No panics, no `unsafe`.

/// The eexec layer's cipher seed (Adobe Type 1 Font Format, § 7).
const EEXEC_SEED: u32 = 55665;

/// The charstring layer's cipher seed — the same cipher over the bytes each
/// `/name <n> RD` entry carries (Adobe Type 1 Font Format, § 6).
const CHARSTRING_SEED: u32 = 4330;

/// The largest `/lenIV` a real font carries: the value counts the random
/// bytes prefixing every charstring, the format's default is 4, and no real
/// font exceeds a handful. Anything larger means the charstring bytes cannot
/// be judged, so the subset declines.
const MAX_LEN_IV: usize = 32;

/// The classic PFA trailer this module re-emits: eight lines of 64 zeros
/// (512 `0` characters, which consumers skip as noise while their eexec
/// reader resynchronises) followed by `cleartomark`.
const TRAILER_ZEROS_LINES: usize = 8;
const TRAILER_ZEROS_WIDTH: usize = 64;

/// Filter a Type 1 program to `used` glyph names: eexec-decrypt the private
/// section, drop unused charstrings, re-emit a valid PFA. `None` on any parse
/// failure (caller embeds the original program verbatim — Layer A behaviour).
///
/// `.notdef` is always kept; every other charstring survives exactly when its
/// name is listed in `used`. A program whose kept set is empty declines, as
/// does one whose private section cannot be split into the prefix / entries /
/// tail shape above — both fall back to the verbatim embed.
pub(crate) fn subset_type1(program: &[u8], used: &[&str]) -> Option<Vec<u8>> {
    let (header, cipher) = split_program(program)?;
    let plain = eexec_decrypt(EEXEC_SEED, &cipher);
    let split = split_private(&plain, used)?;
    // A kept charstring that composes accents references other glyphs by
    // name; dropping those would break it silently, so any seac-shaped one
    // declines the whole subset (verbatim fallback — correct, only larger).
    let len_iv = len_iv(&plain);
    if len_iv > MAX_LEN_IV || may_compose_accents(&split.kept, len_iv) {
        return None;
    }
    // Reassemble the plaintext: prefix verbatim, the charstring block with
    // only the kept entries in their original order, tail verbatim through
    // `closefile`. A fully-used program re-emits byte-identically (the test
    // pins that), which is what keeps Layer A's verbatim-embed gate true.
    let mut new_plain = Vec::with_capacity(plain.len());
    new_plain.extend_from_slice(split.prefix);
    new_plain.extend_from_slice(b"/CharStrings ");
    new_plain.extend_from_slice(split.kept.len().to_string().as_bytes());
    new_plain.extend_from_slice(split.head);
    new_plain.push(b'\n');
    for charstring in &split.kept {
        new_plain.push(b'/');
        new_plain.extend_from_slice(charstring.name);
        new_plain.extend_from_slice(format!(" {} {} ", charstring.data.len(), String::from_utf8_lossy(split.reader)).as_bytes());
        new_plain.extend_from_slice(charstring.data);
        new_plain.extend_from_slice(split.definer);
        new_plain.push(b'\n');
    }
    new_plain.extend_from_slice(split.suffix);
    new_plain.push(b'\n');
    // …and wrap it in the gs-verified shape.
    let mut out = header;
    out.push(b'\n');
    out.extend_from_slice(one_line_hex(&eexec_encrypt(EEXEC_SEED, &new_plain)).as_bytes());
    out.push(b'\n');
    for _ in 0..TRAILER_ZEROS_LINES {
        out.extend(std::iter::repeat_n(b'0', TRAILER_ZEROS_WIDTH));
        out.push(b'\n');
    }
    out.extend_from_slice(b"cleartomark\n");
    Some(out)
}

/// One decrypted private section, split where the subset cuts.
struct PrivateSplit<'a> {
    /// Everything before the `/CharStrings` token, verbatim: the four random
    /// eexec bytes, the private dict's own definitions (`/RD`, `/ND`,
    /// `/lenIV`, `Subrs`…) and whatever introduced the charstring block
    /// (`dup /Private … begin`, `2 index `…).
    prefix: &'a [u8],
    /// From just after the original entry count to just after the `begin`
    /// token (` dict dup begin`, however the font spelled it): re-emitted
    /// verbatim so the operand-stack shape the tail depends on survives.
    head: &'a [u8],
    /// The charstrings to keep, in their original order (`.notdef` first iff
    /// the font wrote it first — order is the font's, not ours).
    kept: Vec<Charstring<'a>>,
    /// From the `end` that closes the charstring dict through the
    /// `closefile` that ends the eexec section, verbatim.
    suffix: &'a [u8],
    /// The token that precedes charstring data (`RD` or `-|`).
    reader: &'a [u8],
    /// The token that follows it (`ND`, `|-`…), re-emitted immediately after
    /// the data (the gs resync quirk).
    definer: &'a [u8],
}

/// One charstring: its name and its still-encrypted data (seed 4330, `/lenIV`
/// — copied verbatim, never re-encrypted).
struct Charstring<'a> {
    name: &'a [u8],
    data: &'a [u8],
}

/// The eexec cipher, encrypting: `plain ^ (seed >> 8)`, the seed advancing
/// by the *cipher* byte (Adobe Type 1 Font Format § 7 — the encryptor's
/// recurrence emits the byte it just produced).
fn eexec_encrypt(seed: u32, data: &[u8]) -> Vec<u8> {
    let mut seed = seed;
    data.iter()
        .map(|&plain| {
            let cipher = plain ^ ((seed >> 8) as u8);
            seed = (u32::from(cipher).wrapping_add(seed)).wrapping_mul(52845).wrapping_add(22719) & 0xFFFF;
            cipher
        })
        .collect()
}

/// The eexec cipher, decrypting: the same XOR, but the seed advances by the
/// *cipher* byte it consumed (the input here) — the recurrence the spec's
/// decryptor runs. The two directions are mutual inverses, but not the same
/// function: the seed tracks ciphertext in both, which is the output when
/// encrypting and the input when decrypting.
fn eexec_decrypt(seed: u32, data: &[u8]) -> Vec<u8> {
    let mut seed = seed;
    data.iter()
        .map(|&cipher| {
            let plain = cipher ^ ((seed >> 8) as u8);
            seed = (u32::from(cipher).wrapping_add(seed)).wrapping_mul(52845).wrapping_add(22719) & 0xFFFF;
            plain
        })
        .collect()
}

/// The private dict's `/lenIV`: how many random bytes prefix every
/// charstring, which the seac scan decrypts past. The format's default is
/// 4; the last definition wins, matching consumers that read the dict
/// sequentially. A negative value (charstrings not encrypted) reads as no
/// integer here, so the previous value stands — a skip that can only cost
/// size, never correctness.
fn len_iv(plain: &[u8]) -> usize {
    const NAME: &[u8] = b"lenIV";
    let mut value = 4usize;
    let mut from = 0usize;
    while let Some(at) = find_sub(plain, NAME, from)
        && at > 0
        && plain.get(at - 1) == Some(&b'/')
    {
        if let Some((n, _)) = read_count(plain, skip_ws(plain, at + NAME.len())) {
            value = n;
        }
        from = at + 1;
    }
    value
}

/// Whether any kept charstring may compose accents — `seac`, the
/// standard-encoding accent construction Adobe-era Type 1 fonts build
/// é/ü/à from. A composing charstring references its base and accent
/// glyphs *by name* (through StandardEncoding), so a subset that drops
/// them would break the glyph in the consumer with no note: any seac shape
/// declines the whole subset.
///
/// The judgement is byte-shaped, not interpreted — charstrings are
/// tokenized into numbers and operators (no stack semantics), and three
/// shapes fire:
///
/// - the explicit `seac` escape operator, bytes `12 6` (no other escaped
///   operator uses sub-code 6, so an operator token `12 6` can only be it);
/// - an `endchar` (`14`) anywhere **but** the final byte — operands-then-
///   `endchar` mid-string, the shape a composing charstring takes when more
///   follows it;
/// - four or more operands sitting on the **final** `endchar` — seac's
///   implicit spelling, `adx ady bchar achar endchar` (a legal charstring
///   otherwise ends with at most one width operand, its metrics consumed by
///   `hsbw`/`sbw` long before).
///
/// A number payload that happens to hold these bytes fires too; that only
/// costs size, and a seac never slips through. The operands-before-endchar
/// count is seac's own arity (4, or 5 with the sidebearing) — two or three
/// trailing operands belong to malformed charstrings the consumer errors on
/// regardless of subsetting.
fn may_compose_accents(kept: &[Charstring<'_>], len_iv: usize) -> bool {
    kept.iter().any(|charstring| {
        let plain = eexec_decrypt(CHARSTRING_SEED, charstring.data);
        plain.get(len_iv..).is_some_and(charstring_may_seac)
    })
}

/// One decrypted charstring's byte shape, judged for `seac` (see
/// [`may_compose_accents`] for the three signals and why each is sound).
fn charstring_may_seac(body: &[u8]) -> bool {
    let mut explicit = false;
    let mut early_endchar = false;
    let mut into_endchar = 0usize;
    let mut at = 0usize;
    let mut operands = 0usize; // numbers since the last operator token
    while let Some(&byte) = body.get(at) {
        let size = match byte {
            12 => 2, // an escaped operator; its sub-code rides along
            247..=250 => 2,
            251..=254 => 3,
            255 => 5,
            _ => 1,
        };
        match byte {
            12 if body.get(at + 1) == Some(&6) => explicit = true, // `seac`
            14 if at + size == body.len() => into_endchar = operands,
            14 => early_endchar = true,
            32..=255 => operands = operands.saturating_add(1),
            _ => operands = 0,
        }
        at += size;
    }
    explicit || early_endchar || into_endchar >= 4
}

/// Split a program into its cleartext header (ending at the `eexec` token)
/// and its encrypted private section. PFB inputs are unsegmented first; PFA
/// inputs carry the section as hex (a run of hex digits and whitespace) or
/// raw binary, the same discrimination skrifa makes (first four bytes all
/// hex digits → hex). `None` when no `eexec` token bounds the header.
fn split_program(program: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    if program.get(..2) == Some(&[0x80, 0x01]) {
        return split_pfb(program);
    }
    let eexec_end = find_eexec(program)?;
    let after = program.get(eexec_end..)?;
    let data = after.get(whitespace_run(after)..)?;
    let cipher =
        if data.len() >= 4 && data.get(..4).is_some_and(|first| first.iter().all(|b| b.is_ascii_hexdigit())) { hex_run(data) } else { data.to_vec() };
    Some((program.get(..eexec_end)?.to_vec(), cipher))
}

/// A PFB program (`0x80 0x01`/`0x80 0x02`-tagged segments, little-endian
/// lengths): the leading text segments concatenated are the cleartext, the
/// following binary segments concatenated are the private section, and
/// anything after the binary run (the ASCII trailer, the `0x80 0x03` EOF
/// marker) is dropped.
fn split_pfb(program: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut cleartext = Vec::new();
    let mut cipher = Vec::new();
    let mut at = 0usize;
    let mut in_text = true;
    while let Some((tag, len)) = pfb_segment(program, at) {
        if tag == 0x8003 {
            break;
        }
        let start = at.checked_add(6)?;
        let end = start.checked_add(len)?;
        let bytes = program.get(start..end)?;
        match tag {
            0x8001 if in_text => cleartext.extend_from_slice(bytes),
            0x8002 => {
                in_text = false;
                cipher.extend_from_slice(bytes);
            }
            _ => {}
        }
        at = end;
    }
    let eexec_end = find_eexec(&cleartext)?;
    Some((cleartext.get(..eexec_end)?.to_vec(), cipher))
}

/// The PFB segment header at `at`: its tag and length, `None` at end of data
/// or for a tag that is not a segment marker.
fn pfb_segment(data: &[u8], at: usize) -> Option<(u16, usize)> {
    let header = data.get(at..at.checked_add(6)?)?;
    let tag = u16::from_be_bytes([*header.first()?, *header.get(1)?]);
    if matches!(tag, 0x8001..=0x8003) {
        let len = usize::try_from(u32::from_le_bytes(header.get(2..6)?.try_into().ok()?)).ok()?;
        Some((tag, len))
    } else {
        None
    }
}

/// The end of the `eexec` token that bounds a program's cleartext header, or
/// `None`. The token must stand alone (whitespace on both sides) and must not
/// sit in a comment (no `%` earlier on its line) — the two places a font's
/// header can mention "eexec" without meaning it.
fn find_eexec(data: &[u8]) -> Option<usize> {
    let mut from = 0usize;
    while let Some(at) = find_sub(data, b"eexec", from) {
        let before_ok = at > 0 && is_ps_whitespace(*data.get(at - 1)?);
        let after_ok = data.get(at + 5).is_some_and(|&b| is_ps_whitespace(b));
        let line_start = data.get(..at)?.iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1);
        let commented = data.get(line_start..at)?.contains(&b'%');
        if before_ok && after_ok && !commented {
            return Some(at + 5);
        }
        from = at + 1;
    }
    None
}

/// The decrypted private section split for subsetting. See [`PrivateSplit`]
/// for what is kept verbatim and why. `None` when the section does not carry
/// the standard shape — `/CharStrings <n> dict [dup] begin`, entries, a
/// closing `end`, a tail with `definefont` and `closefile` — or when nothing
/// would be kept (`.notdef` is always kept, so that means the font has none
/// and `used` matches none of its names).
fn split_private<'a>(plain: &'a [u8], used: &[&str]) -> Option<PrivateSplit<'a>> {
    const CS: &[u8] = b"CharStrings";
    let cs = find_name(plain, CS)?;
    let pos = skip_ws(plain, cs + 1 + CS.len());
    let (_count, count_end) = read_count(plain, pos)?;
    // Through `begin` verbatim: the tokens between the count and the block's
    // opening are the font's own (` dict dup begin`), and the tail's stack
    // arithmetic depends on their exact shape.
    let after_dict = expect_token(plain, skip_ws(plain, count_end), b"dict")?;
    let after_dup = skip_ws(plain, after_dict);
    let begin_end = if token_at(plain, after_dup, b"dup") {
        expect_token(plain, skip_ws(plain, after_dup + 3), b"begin")?
    } else {
        expect_token(plain, after_dup, b"begin")?
    };
    let head = plain.get(count_end..begin_end)?;
    // The entries. Each is `/name <len> RD|-| <exactly len bytes> ND|-|`; the
    // reader and definer tokens must match the first entry's (fonts are
    // consistent within a block; a mixed one declines rather than guessing).
    let mut kept: Vec<Charstring<'a>> = Vec::new();
    let mut reader: Option<&'a [u8]> = None;
    let mut definer: Option<&'a [u8]> = None;
    let mut pos = begin_end;
    let block_end;
    loop {
        let ws = skip_ws(plain, pos);
        if token_at(plain, ws, b"end") {
            block_end = ws;
            break;
        }
        if plain.get(ws) != Some(&b'/') {
            return None;
        }
        let name_start = ws + 1;
        let name_end = name_start + token_len(plain.get(name_start..)?);
        let name = plain.get(name_start..name_end)?;
        if name.is_empty() {
            return None;
        }
        let (len, len_end) = read_count(plain, skip_ws(plain, name_end))?;
        let read_start = skip_ws(plain, len_end);
        let read_end = read_start + token_len(plain.get(read_start..)?);
        let token = plain.get(read_start..read_end)?;
        if token != b"RD" && token != b"-|" {
            return None;
        }
        if reader.is_none() {
            reader = Some(token);
        }
        if reader != Some(token) {
            return None;
        }
        // Exactly one separator byte before the data (the convention every
        // consumer's resync assumes; FreeType skips one byte unconditionally).
        if plain.get(read_end) != Some(&b' ') {
            return None;
        }
        let data_start = read_end + 1;
        let data_end = data_start.checked_add(len)?;
        let data = plain.get(data_start..data_end)?;
        // The definer may be separated on input (consumers tolerate it); the
        // re-emission writes it tight — the gs quirk.
        let define_start = skip_ws(plain, data_end);
        let define_end = define_start + token_len(plain.get(define_start..)?);
        let define_token = plain.get(define_start..define_end)?;
        if define_token.is_empty() || define_token == b"end" {
            return None;
        }
        if definer.is_none() {
            definer = Some(define_token);
        }
        if definer != Some(define_token) {
            return None;
        }
        if name == b".notdef" || used.iter().any(|wanted| wanted.as_bytes() == name) {
            kept.push(Charstring { name, data });
        }
        pos = define_end;
    }
    if kept.is_empty() {
        return None;
    }
    // The tail has to close the eexec section and leave the font defined —
    // `findfont` depends on the `definefont` the tail carries.
    let tail = plain.get(block_end..)?;
    let close = find_token(tail, b"closefile")? + block_end;
    let close_end = close + b"closefile".len();
    let suffix = plain.get(block_end..close_end)?;
    find_sub(suffix, b"definefont", 0)?;
    Some(PrivateSplit { prefix: plain.get(..cs)?, head, kept, suffix, reader: reader?, definer: definer? })
}

/// Lowercase hex, no separators: the one unbroken line gs's eexec reader
/// needs (see the module docs).
fn one_line_hex(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(data.len().saturating_mul(2));
    for byte in data {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

/// The hex digits of `data` paired into bytes; whitespace between digits is
/// free, a dangling final nibble is zero-padded (FreeType's tolerance), and
/// the run ends at the first byte that is neither.
fn hex_run(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut high: Option<u8> = None;
    for &byte in data {
        if let Some(nibble) = hex_nibble(byte) {
            match high.take() {
                Some(h) => out.push(h << 4 | nibble),
                None => high = Some(nibble),
            }
        } else if !is_ps_whitespace(byte) {
            break;
        }
    }
    if let Some(h) = high {
        out.push(h << 4);
    }
    out
}

/// The value of a hex digit byte, `None` for anything else.
fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// `name` in `data` as a standalone PostScript literal name (`/name`),
/// returning the position of its `/`. `None` when absent.
fn find_name(data: &[u8], name: &[u8]) -> Option<usize> {
    let mut from = 0usize;
    while let Some(at) = find_sub(data, name, from) {
        if at > 0 && data.get(at - 1) == Some(&b'/') && delimited_after(data, at + name.len()) {
            return Some(at - 1);
        }
        from = at + 1;
    }
    None
}

/// `needle` in `data` as a standalone token, `None` when absent.
fn find_token(data: &[u8], needle: &[u8]) -> Option<usize> {
    let mut from = 0usize;
    while let Some(at) = find_sub(data, needle, from) {
        let starts_token = at == 0 || is_ps_delimiter(*data.get(at - 1)?);
        if starts_token && delimited_after(data, at + needle.len()) {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

/// Whether the byte after a token candidate ends it (whitespace, a special,
/// or the end of data).
fn delimited_after(data: &[u8], at: usize) -> bool {
    data.get(at).is_none_or(|&b| is_ps_delimiter(b))
}

/// `needle` in `data[at..]`, `None` when absent.
fn find_sub(data: &[u8], needle: &[u8], at: usize) -> Option<usize> {
    let tail = data.get(at..)?;
    if needle.is_empty() || tail.len() < needle.len() {
        return None;
    }
    tail.windows(needle.len()).position(|window| window == needle).map(|found| found + at)
}

/// The length of the token starting at `data` (zero when the data starts
/// with a delimiter or is empty).
fn token_len(data: &[u8]) -> usize {
    data.iter().position(|&b| is_ps_delimiter(b)).unwrap_or(data.len())
}

/// Whether `data` starts with `token` standing alone.
fn token_at(data: &[u8], at: usize, token: &[u8]) -> bool {
    data.get(at..).is_some_and(|tail| {
        let matches = tail.get(..token.len()) == Some(token);
        matches && delimited_after(data, at + token.len())
    })
}

/// The position after `token` at `at`, `None` when it is not there standing
/// alone.
fn expect_token(data: &[u8], at: usize, token: &[u8]) -> Option<usize> {
    token_at(data, at, token).then_some(at + token.len())
}

/// The first non-whitespace position at or after `at`.
fn skip_ws(data: &[u8], at: usize) -> usize {
    let mut pos = at;
    while data.get(pos).is_some_and(|&b| is_ps_whitespace(b)) {
        pos += 1;
    }
    pos
}

/// The length of the run of whitespace at the start of `data`.
fn whitespace_run(data: &[u8]) -> usize {
    skip_ws(data, 0)
}

/// A non-negative decimal count at `at`: its value and the position after
/// its digits. Overflow (a hostile claimed length) declines rather than
/// wrapping.
fn read_count(data: &[u8], at: usize) -> Option<(usize, usize)> {
    let mut value: usize = 0;
    let mut pos = at;
    while let Some(&byte) = data.get(pos) {
        if !byte.is_ascii_digit() {
            break;
        }
        value = value.checked_mul(10)?.checked_add(usize::from(byte - b'0'))?;
        pos += 1;
    }
    (pos > at).then_some((value, pos))
}

/// PostScript whitespace, the set FreeType's scanner uses.
fn is_ps_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\n' | b'\r' | b'\t' | 0 | 0x0c)
}

/// The characters that always delimit a PostScript token.
fn is_ps_special(byte: u8) -> bool {
    matches!(byte, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// A token boundary: whitespace or a self-delimiting special.
fn is_ps_delimiter(byte: u8) -> bool {
    is_ps_whitespace(byte) || is_ps_special(byte)
}

#[cfg(test)]
mod tests {
    use skrifa::raw::ps::type1::Type1Font;

    use super::*;

    // ---------------------------------------------------------------- fixtures
    // Mirrors of the fonts.rs/oracle.rs builders (house style: each test
    // module owns its fixtures), parameterised so hostile variants can be
    // forged from the same shape.

    /// Adobe Type 1 encryption (eexec seed 55665, charstring seed 4330) —
    /// the *encryptor* (its seed advances by the byte it emits; the decryptor
    /// [`eexec_decrypt`] advances by the byte it consumes, so the two are not
    /// one function run either way).
    fn t1_encrypt(seed: u32, data: &[u8]) -> Vec<u8> {
        let mut seed = seed;
        data.iter()
            .map(|&plain| {
                let cipher = plain ^ ((seed >> 8) as u8);
                seed = (u32::from(cipher).wrapping_add(seed)).wrapping_mul(52845).wrapping_add(22719) & 0xFFFF;
                cipher
            })
            .collect()
    }

    /// A charstring: the plaintext ops, charstring-encrypted. The fixture
    /// declares `/lenIV 0` (the gs 10.08 FreeType finding — see fonts.rs).
    fn t1_charstring(ops: &[u8]) -> Vec<u8> {
        t1_encrypt(4330, ops)
    }

    /// `0 500 hsbw; 100 hmoveto; 0 400 rlineto; 300 0 rlineto; 0 -400
    /// rlineto; closepath; endchar` — a 300×400 box, `advance` wide.
    fn box_ops(advance: u16) -> Vec<u8> {
        let mut ops = vec![139, 248];
        ops.extend_from_slice(&(advance as i16).to_be_bytes());
        ops.extend_from_slice(&[136, 13, 239, 22, 139, 248, 36, 5, 247, 192, 139, 5, 139, 252, 36, 5, 9, 14]);
        ops
    }

    /// The plaintext private section over `glyphs` (name, plaintext ops):
    /// the private dict, its `/RD` `/ND` `/lenIV 0` definitions, the
    /// charstring dict, and the tail that `definefont`s and closes the file.
    fn t1_private(glyphs: &[(&str, &[u8])]) -> Vec<u8> {
        let mut private: Vec<u8> = Vec::new();
        private.extend_from_slice(
            b"dup /Private 8 dict dup begin\n\
              /RD {string currentfile exch readstring pop} executeonly def\n\
              /ND {noaccess def} executeonly def\n\
              /NP {noaccess put} executeonly def\n\
              /lenIV 0 def\n\
              2 index /CharStrings 3 dict dup begin\n",
        );
        for (name, ops) in glyphs {
            let cipher = t1_charstring(ops);
            private.extend_from_slice(format!("/{name} {} RD ", cipher.len()).as_bytes());
            private.extend_from_slice(&cipher);
            private.extend_from_slice(b"ND\n");
        }
        private.extend_from_slice(
            b"end\nend\nreadonly put\nnoaccess put\n\
              dup /FontName get exch definefont pop\n\
              mark currentfile closefile\n",
        );
        private
    }

    /// A PFA program whose eexec section is `private` (plus the standard four
    /// random bytes), verbatim: the forgery point for structurally broken
    /// private sections.
    fn t1_over_raw(private: &[u8]) -> Vec<u8> {
        let mut plain = vec![0x5a, 0xa5, 0x37, 0xc3];
        plain.extend_from_slice(private);
        let cipher = t1_encrypt(55665, &plain);
        #[rustfmt::skip]
        let cleartext: &str = "%!PS-AdobeFont-1.0: PdfCraftTest 001.001\n\
             11 dict begin\n\
             /FontName /PdfCraftTest def\n\
             /PaintType 0 def\n\
             /FontType 1 def\n\
             /FontMatrix [0.001 0 0 0.001 0 0] readonly def\n\
             /Encoding StandardEncoding def\n\
             /FontBBox [0 -200 700 800] readonly def\n\
             currentdict end\n\
             currentfile eexec\n";
        let mut out = cleartext.as_bytes().to_vec();
        out.extend_from_slice(cipher.iter().map(|b| format!("{b:02x}")).collect::<String>().as_bytes());
        out.push(b'\n');
        for _ in 0..8 {
            out.extend_from_slice(&[b'0'; 64]);
            out.push(b'\n');
        }
        out.extend_from_slice(b"cleartomark\n");
        out
    }

    /// A minimal, real Type 1 font program (PFA style, hex eexec): glyphs
    /// `.notdef`, `A` and `B`, each a 300×400 box outline in a 1000-unit em
    /// (the fonts.rs/oracle.rs fixture, owned here too).
    fn t1_program() -> Vec<u8> {
        let notdef = [139, 248, 136, 13, 14];
        t1_over_raw(&t1_private(&[(".notdef", &notdef), ("A", &box_ops(500)), ("B", &box_ops(700))]))
    }

    /// The glyph names a program parses to, in glyph-id order (the same
    /// skrifa parse production performs).
    fn glyph_names(program: &[u8]) -> Vec<String> {
        let font = Type1Font::new(program).expect("the program parses");
        font.glyph_names().map(|(_, name)| name.to_string()).collect()
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        needle.is_empty() || haystack.windows(needle.len()).any(|window| window == needle)
    }

    // ------------------------------------------------------------------- tests

    /// The brief's Step 1: `t1_program()` filtered to `["A"]` — the eexec
    /// section decrypts, re-encrypts, and the output's `/CharStrings` dict
    /// has exactly `.notdef` and `A`. skrifa parses the output (the same
    /// parse production performs), so the round-trip is proven by
    /// construction; the emitted shape carries the gs-verified quirks (one
    /// unbroken hex line, `ND` tight after the data, the 8×64-zero trailer).
    #[test]
    fn subset_keeps_used_charstrings() {
        let out = subset_type1(&t1_program(), &["A"]).expect("the fixture subsets");
        assert_eq!(glyph_names(&out), [".notdef".to_string(), "A".to_string()], "exactly .notdef and A survive");
        let font = Type1Font::new(&out).expect("the subset parses");
        assert_eq!(font.name(), Some("PdfCraftTest"), "the program keeps its own name");
        // The gs-verified shape, byte-side.
        let text = String::from_utf8(out.clone()).expect("the subset is a pure-ASCII PFA");
        let eexec = text.find("currentfile eexec\n").expect("the cleartext header rides verbatim") + "currentfile eexec\n".len();
        let line_end = eexec + text[eexec..].find('\n').expect("the hex line ends");
        assert!(out[eexec..line_end].iter().all(|b| b.is_ascii_hexdigit()), "one unbroken hex line");
        let mut trailer = String::new();
        for _ in 0..8 {
            trailer.push_str(&"0".repeat(64));
            trailer.push('\n');
        }
        trailer.push_str("cleartomark\n");
        assert!(text.ends_with(&trailer), "the classic zero-line trailer: {text:?}");
        // And the decrypted private section carries A's charstring, not B's,
        // with the definer tight after the data.
        let cipher: Vec<u8> =
            out[eexec..line_end].chunks(2).map(|pair| u8::from_str_radix(&String::from_utf8_lossy(pair), 16).expect("a hex pair")).collect();
        let plain = eexec_decrypt(EEXEC_SEED, &cipher);
        assert!(contains(&plain, b"/CharStrings 2 dict dup begin"), "the block re-emitted over the kept count: {plain:?}");
        assert!(contains(&plain, b"/A "), "A's charstring rides: {plain:?}");
        assert!(!contains(&plain, b"/B "), "B's charstring is dropped: {plain:?}");
        assert!(contains(&plain, b"ND\nend"), "the definer immediately after the data");
    }

    /// A program whose glyphs are all used re-emits byte-identically: every
    /// entry, the count and the tail ride unchanged, and the eexec cipher is
    /// deterministic — which is what keeps Layer A's "program bytes verbatim
    /// in the prolog" gate true for fully-used fonts.
    #[test]
    fn subset_with_every_name_is_byte_identical() {
        let program = t1_program();
        let out = subset_type1(&program, &[".notdef", "A", "B"]).expect("a fully-used program subsets");
        assert_eq!(out, program, "nothing dropped ⇒ identical bytes");
    }

    /// A PFB-wrapped program (the segmentation real `/FontFile` streams
    /// usually carry) subsets like a PFA: the segments are unrolled, the
    /// output is a PFA (pure ASCII, no segment tags), and skrifa parses it
    /// to exactly the kept names.
    #[test]
    fn pfb_program_subsets() {
        let pfa = t1_program();
        // Split the PFA at its eexec line: text(header), binary(cipher),
        // text(trailer) — the segment lengths computed, never counted by hand.
        let mark = pfa.windows(b"currentfile eexec\n".len()).position(|w| w == b"currentfile eexec\n").expect("the marker");
        let hex_start = mark + "currentfile eexec\n".len();
        let hex_end = hex_start + pfa[hex_start..].iter().position(|&b| b == b'\n').expect("the hex line ends");
        let header = &pfa[..hex_start - 1]; // drop the line feed; PFB cleartext ends with \r
        let cipher: Vec<u8> =
            pfa[hex_start..hex_end].chunks(2).map(|pair| u8::from_str_radix(&String::from_utf8_lossy(pair), 16).expect("a hex pair")).collect();
        let trailer = &pfa[hex_end + 1..];
        let mut pfb = Vec::new();
        for (tag, bytes) in [(0x8001u16, header), (0x8002, cipher.as_slice()), (0x8001, trailer)] {
            pfb.push(0x80);
            pfb.push(tag.to_be_bytes()[1]);
            pfb.extend_from_slice(&(bytes.len() as u32 + 1).to_le_bytes());
            pfb.extend_from_slice(bytes);
            pfb.push(b'\r');
        }
        assert!(Type1Font::new(&pfb).is_ok(), "the forged PFB parses (the strategy arm accepts it)");
        let out = subset_type1(&pfb, &["A"]).expect("a PFB program subsets");
        assert_eq!(glyph_names(&out), [".notdef".to_string(), "A".to_string()], "exactly .notdef and A survive");
        assert!(out.iter().all(|b| matches!(*b, b'\n' | b'\r' | b'\t' | 0x20..=0x7e)), "the output is a PFA, not re-segmented");
    }

    /// Broken and hostile programs decline (`None`) rather than emitting a
    /// wrong program: no eexec, garbage private section, an entry claiming
    /// more bytes than the section carries, a tail cut before `closefile`,
    /// no `/CharStrings` block at all — and empty input.
    #[test]
    fn broken_programs_decline() {
        assert!(subset_type1(&[], &["A"]).is_none(), "empty input");
        assert!(subset_type1(b"%!PS-AdobeFont-1.0: X /FontType 1 def\n", &["A"]).is_none(), "no eexec section");
        assert!(subset_type1(&t1_over_raw(b"not a private dict at all"), &["A"]).is_none(), "garbage private section");
        let claiming = b"dup /Private 8 dict dup begin\n\
             /RD {string currentfile exch readstring pop} executeonly def\n\
             /ND {noaccess def} executeonly def\n\
             /lenIV 0 def\n\
             2 index /CharStrings 3 dict dup begin\n\
             /.notdef 4 RD xxxxND\n\
             /A 9999 RD abND\n\
             end\nend\nreadonly put\nnoaccess put\n\
             dup /FontName get exch definefont pop\n\
             mark currentfile closefile\n";
        assert!(subset_type1(&t1_over_raw(claiming), &["A"]).is_none(), "an entry claiming 9999 bytes declines");
        // A tail cut inside `closefile`: the charstrings survive (skrifa
        // still parses — the fonts.rs fallback test drives that half), but
        // the re-emitted program must close its file, so this declines.
        let whole = t1_program();
        let hex_start =
            whole.windows(b"currentfile eexec\n".len()).position(|w| w == b"currentfile eexec\n").expect("the marker") + "currentfile eexec\n".len();
        let hex_end = hex_start + whole[hex_start..].iter().position(|&b| b == b'\n').expect("the hex line ends");
        let mut cut = whole[..hex_end - 30].to_vec();
        cut.extend_from_slice(&whole[hex_end..]);
        assert!(subset_type1(&cut, &["A"]).is_none(), "a tail without closefile declines");
        let no_block = b"dup /Private 8 dict dup begin\n/lenIV 0 def\nend\nend\nreadonly put\nnoaccess put\ndup /FontName get exch definefont pop\nmark currentfile closefile\n";
        assert!(subset_type1(&t1_over_raw(no_block), &["A"]).is_none(), "no /CharStrings block declines");
    }

    /// `seac` (accent composition) declines the whole subset, in every byte
    /// spelling the consumers accept: the explicit `12 6` operator (with and
    /// without a following `endchar` — the Blue Book form ends the charstring
    /// at `seac`), operands sitting on a final `endchar` (the implicit
    /// spelling), and an early `endchar` mid-string. The plain box
    /// charstrings — a bare final `endchar` with an empty stack — must not
    /// trip (fix round 1: a composing glyph references its base and accent
    /// by name, and dropping those broke it silently).
    #[test]
    fn seac_charstrings_decline_the_subset() {
        let notdef = [139, 248, 136, 13, 14];
        // `0 500 hsbw 50 0 bchar achar …`: four operands into seac's spellings.
        let implicit = [139, 248, 136, 13, 189, 139, 204, 209, 14];
        let explicit = [139, 248, 136, 13, 189, 139, 204, 209, 12, 6, 14];
        let terminal = [139, 248, 136, 13, 189, 139, 204, 209, 12, 6];
        let early = [139, 248, 136, 13, 189, 139, 14, 9, 14];
        for accenting in [implicit.as_slice(), explicit.as_slice(), terminal.as_slice(), early.as_slice()] {
            let program = t1_over_raw(&t1_private(&[(".notdef", &notdef), ("A", accenting), ("B", &box_ops(700))]));
            assert!(subset_type1(&program, &["A"]).is_none(), "the seac shape {accenting:?} declines");
        }
        // The plain shapes subset (also proven by every other test in this
        // module, which would fail if the detector over-fired).
        let program = t1_over_raw(&t1_private(&[(".notdef", &notdef), ("A", &box_ops(500)), ("B", &box_ops(700))]));
        assert!(subset_type1(&program, &["A"]).is_some(), "a plain charstring subsets");
        // Three trailing operands (fewer than seac's arity) stay on the
        // legal side: endchar's optional width operand is one, and malformed
        // two/three-operand tails are the consumer's error, not ours.
        let width = [139, 248, 136, 13, 189, 14];
        let program = t1_over_raw(&t1_private(&[(".notdef", &notdef), ("A", &width), ("B", &box_ops(700))]));
        assert!(subset_type1(&program, &["A"]).is_some(), "one width operand into endchar subsets");
        // A hostile /lenIV would push the byte scan past every charstring —
        // the bytes cannot be judged, so the subset declines.
        let private = t1_private(&[(".notdef", &notdef), ("A", &implicit), ("B", &box_ops(700))]);
        let at = private.windows(b"/lenIV 0 def".len()).position(|w| w == b"/lenIV 0 def").expect("the lenIV line");
        let mut hostile = private[..at].to_vec();
        hostile.extend_from_slice(b"/lenIV 9999 def");
        hostile.extend_from_slice(&private[at + "/lenIV 0 def".len()..]);
        assert!(subset_type1(&t1_over_raw(&hostile), &["A"]).is_none(), "an absurd /lenIV declines (the bytes cannot be judged)");
    }
}
