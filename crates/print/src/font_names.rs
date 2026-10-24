//! Embedded TrueType and OpenType programs renamed before a print-ready PDF goes to the Windows
//! spooler, so no installed font answers to their names (#3132).
//!
//! A subset program numbers its glyphs anew: ReportLab's Arial subsets draw a space with glyph 11
//! (Arial Bold: glyph 8), where the installed Arial's space is glyph 3. Windows' print path
//! (Windows.Data.Pdf drawing into Direct2D's print control, through craft-print) keeps the
//! subset's glyph numbers but draws them with the installed font of the same name, so every letter
//! prints as the installed glyph with that number: `Revisión de clientes,` in the bold subset comes
//! out as `Wj{nxn s%ij%hqnjsyjx1`, its `z` as `Ä`. Both Brother queues and the Adobe PDF printer
//! show it; PdfCraft's renderer, CUPS and Chrome on Windows draw the embedded program and are right.
//!
//! [`distinct`] renames each program to `PdfCraftPrint<n>`: the identity records of its `name`
//! table, `/FontName` of its descriptors and `/BaseFont` of the fonts drawn with it (a subset keeps
//! its tag, `AAAAAA+PdfCraftPrint1`). Glyphs, glyph numbers, cmaps, encodings, widths and
//! `/ToUnicode` are not touched, so pages draw and extract as before. A program that can't be
//! read and rewritten keeps its old name everywhere.

use std::collections::BTreeMap;
use std::sync::Arc;

use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};

use crate::PrintError;

/// Whether [`crate::spool`] renames the programs before spooling: on Windows only.
pub const RENAME_FOR_SPOOLER: bool = cfg!(windows);

/// Decoded size past which a program is left as it is (a whole CJK font is about 20 MB).
const MAX_PROGRAM: usize = 64 << 20;

/// Decoded bytes the whole renaming may read, across every program. A document's fonts rarely
/// come to 100 MB; a file of many distinct compressed bombs is read only this far.
const MAX_DECODED_IN_ALL: usize = 512 << 20;

/// More tables than any font has; a larger count is a damaged directory.
const MAX_TABLES: usize = 256;

/// The `name` records that say which font this is: family, unique ID, full name, PostScript name,
/// typographic family, Mac compatible full name, PostScript CID name, WWS family and the
/// variations PostScript name prefix.
const IDENTITY: [u16; 9] = [1, 3, 4, 6, 16, 18, 20, 21, 25];

/// The font subtypes whose descriptor can hold a TrueType or OpenType program.
const DRAWN_WITH_A_PROGRAM: [&[u8]; 5] = [b"TrueType", b"Type1", b"MMType1", b"CIDFontType0", b"CIDFontType2"];

/// An embedded program and what it will be called.
struct Program {
    name: String,
    /// A `/FontFile2` program, whose `/Length1` is the decoded length.
    true_type: bool,
}

/// The fonts drawn with embedded TrueType or OpenType programs, found by scanning every object.
#[derive(Default)]
struct Found {
    /// Each program, numbered in object order.
    programs: BTreeMap<ObjRef, Program>,
    /// Simple fonts and CIDFonts, with their program.
    fonts: BTreeMap<ObjRef, ObjRef>,
    /// Indirect font descriptors, with their program.
    descriptors: BTreeMap<ObjRef, ObjRef>,
    /// Type 0 fonts whose CIDFont is an object of its own, with that CIDFont.
    type0: BTreeMap<ObjRef, ObjRef>,
    /// Type 0 fonts whose CIDFont is written inline, with the array object holding it when
    /// `/DescendantFonts` names one, and the CIDFont's program.
    inline: BTreeMap<ObjRef, (Option<ObjRef>, ObjRef)>,
}

impl Found {
    /// Note the program `font` draws with, and its descriptor when that is an object of its own;
    /// the program, if it has one to rename.
    fn add(&mut self, doc: &Document, font: &Dict) -> Option<ObjRef> {
        if !font.name(b"Subtype").is_some_and(|s| DRAWN_WITH_A_PROGRAM.contains(&s)) {
            return None;
        }
        let descriptor = font.get(b"FontDescriptor")?;
        let (program, true_type) = doc.dict(descriptor).and_then(|d| program(doc, &d))?;
        let number = self.programs.len() + 1;
        self.programs.entry(program).or_insert_with(|| Program { name: format!("PdfCraftPrint{number}"), true_type });
        if let Object::Ref(descriptor) = descriptor {
            self.descriptors.insert(*descriptor, program);
        }
        Some(program)
    }
}

/// `pdf` with every embedded TrueType or OpenType program renamed; `None` when it has none that
/// can be.
pub fn distinct(pdf: &[u8]) -> Result<Option<Vec<u8>>, PrintError> {
    distinct_within(pdf, &mut DecodeBudget::new(MAX_DECODED_IN_ALL, 0))
}

/// [`distinct`], decoding the programs against `budget`: once it is spent, the programs not yet
/// read keep their names, as one that can't be read does.
pub(crate) fn distinct_within(pdf: &[u8], budget: &mut DecodeBudget) -> Result<Option<Vec<u8>>, PrintError> {
    let mut doc = Document::open(Arc::new(pdf.to_vec()))?;
    let mut found = find(&doc);
    // A program that can't be rewritten keeps its names in the PDF too: a viewer that can't load
    // it either then still picks its substitute by the original name. The lookups below skip it.
    found.programs.retain(|program, about| rename_program(&mut doc, *program, about, budget));
    if found.programs.is_empty() {
        return Ok(None);
    }
    for (descriptor, program) in &found.descriptors {
        let Some(about) = found.programs.get(program) else { continue };
        doc.update_dict(*descriptor, |d| rename(d, b"FontName", &about.name))?;
    }
    for (font, program) in &found.fonts {
        let Some(about) = found.programs.get(program) else { continue };
        doc.update_dict(*font, |d| rename_font(d, &about.name))?;
    }
    // A Type 0 font is named after its CIDFont; renaming only the CIDFont would leave the old
    // name on the font the page uses.
    for (font, cid) in &found.type0 {
        let Some(about) = found.fonts.get(cid).and_then(|p| found.programs.get(p)) else { continue };
        doc.update_dict(*font, |d| rename(d, b"BaseFont", &about.name))?;
    }
    for (font, (array, program)) in &found.inline {
        let Some(about) = found.programs.get(program) else { continue };
        doc.update_dict(*font, |d| {
            rename(d, b"BaseFont", &about.name);
            if let Some(Object::Array(descendants)) = d.get_mut(b"DescendantFonts") {
                rename_first(descendants, &about.name);
            }
        })?;
        if let Some(array) = array {
            let Object::Array(mut descendants) = doc.get(*array).as_ref().clone() else { continue };
            rename_first(&mut descendants, &about.name);
            doc.set(*array, Object::Array(descendants));
        }
    }
    Ok(Some(write_full(&doc, &SaveOptions::default())?))
}

fn find(doc: &Document) -> Found {
    let mut found = Found::default();
    for (reference, object) in doc.scan_objects() {
        let Object::Dict(font) = object.as_ref() else { continue };
        if font.name(b"Subtype") == Some(b"Type0") {
            find_descendant(doc, reference, font, &mut found);
        } else if let Some(program) = found.add(doc, font) {
            found.fonts.insert(reference, program);
        }
    }
    found
}

/// Note the CIDFont a Type 0 font is named after: an object of its own, renamed when the scan
/// reaches it, or written inline, which no scan reaches, so its program is noted here.
fn find_descendant(doc: &Document, font: ObjRef, d: &Dict, found: &mut Found) {
    let Some(descendants) = d.get(b"DescendantFonts") else { return };
    let array = match descendants {
        Object::Ref(array) => Some(*array),
        _ => None,
    };
    let descendants = doc.resolve(descendants);
    match descendants.as_array().and_then(|a| a.first()) {
        Some(Object::Ref(cid)) => {
            found.type0.insert(font, *cid);
        }
        Some(Object::Dict(cid)) => {
            if let Some(program) = found.add(doc, cid) {
                found.inline.insert(font, (array, program));
            }
        }
        _ => {}
    }
}

/// Rename a simple font or CIDFont, and its descriptor when that is written inline.
fn rename_font(d: &mut Dict, name: &str) {
    rename(d, b"BaseFont", name);
    if let Some(Object::Dict(descriptor)) = d.get_mut(b"FontDescriptor") {
        rename(descriptor, b"FontName", name);
    }
}

/// Rename the CIDFont written inline at the head of `/DescendantFonts`.
fn rename_first(descendants: &mut [Object], name: &str) {
    if let Some(Object::Dict(cid)) = descendants.first_mut() {
        rename_font(cid, name);
    }
}

/// The descriptor's TrueType program (`/FontFile2`), else its OpenType one (`/FontFile3` with
/// `/Subtype /OpenType`), with whether it is TrueType.
fn program(doc: &Document, descriptor: &Dict) -> Option<(ObjRef, bool)> {
    if let Some(Object::Ref(program)) = descriptor.get(b"FontFile2") {
        return Some((*program, true));
    }
    let Some(Object::Ref(program)) = descriptor.get(b"FontFile3") else { return None };
    let open_type = matches!(doc.get(*program).as_ref(), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"OpenType"));
    open_type.then_some((*program, false))
}

/// Set `key` to `name`, keeping the subset tag the old name had.
fn rename(d: &mut Dict, key: &[u8], name: &str) {
    let mut renamed = d.name(key).and_then(subset_tag).map(<[u8]>::to_vec).unwrap_or_default();
    renamed.extend_from_slice(name.as_bytes());
    d.set(key.to_vec(), Object::Name(renamed));
}

/// `ABCDEF+` at the start of a subset font's name (ISO 32000-2 §9.9.2).
fn subset_tag(name: &[u8]) -> Option<&[u8]> {
    let tag = name.get(..7)?;
    let (letters, plus) = tag.split_at(6);
    (letters.iter().all(u8::is_ascii_uppercase) && plus == b"+").then_some(tag)
}

/// Rewrite the program with its `name` table renamed; whether it was. A program that can't be
/// decoded within `budget` or isn't an sfnt with a readable `name` table stays as it is.
fn rename_program(doc: &mut Document, program: ObjRef, about: &Program, budget: &mut DecodeBudget) -> bool {
    let object = doc.get(program);
    let Object::Stream(stream) = object.as_ref() else { return false };
    let Ok(bytes) = budget.decode_within(stream, MAX_PROGRAM) else { return false };
    let Some(renamed) = renamed_sfnt(&bytes, &about.name) else { return false };
    let mut dict = stream.dict.clone();
    dict.remove(b"DL");
    if about.true_type || dict.get(b"Length1").is_some() {
        dict.set(b"Length1".to_vec(), Object::Int(i64::try_from(renamed.len()).unwrap_or(i64::MAX)));
    }
    doc.set(program, Object::Stream(Stream::flate(dict, &renamed)));
    true
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0_u32, |sum, c| {
        let mut word = [0; 4];
        for (a, b) in word.iter_mut().zip(c) {
            *a = *b;
        }
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// `program` with the identity records of its `name` table saying `name`. The new table goes
/// after the others, as `pdfcraft_fonts` writes its editor cmap, and the directory entry and
/// `head`'s checksum adjustment are brought up to date. `None` when `program` is not a single
/// sfnt font with a `name` table that can be read.
pub(crate) fn renamed_sfnt(program: &[u8], name: &str) -> Option<Vec<u8>> {
    let version = program.get(..4)?;
    if version != [0, 1, 0, 0] && version != b"true" && version != b"OTTO" {
        return None;
    }
    let count = usize::from(u16_at(program, 4)?);
    if count > MAX_TABLES {
        return None;
    }
    let (mut entry, mut head) = (None, None);
    for at in (0..count).map(|i| 12 + i * 16) {
        match program.get(at..at + 4)? {
            b"name" => entry = Some(at),
            b"head" => head = Some(usize::try_from(u32_at(program, at + 8)?).ok()?),
            _ => {}
        }
    }
    let entry = entry?;
    let offset = usize::try_from(u32_at(program, entry + 8)?).ok()?;
    let length = usize::try_from(u32_at(program, entry + 12)?).ok()?;
    let table = renamed_table(program.get(offset..offset.checked_add(length)?)?, name)?;
    let mut out = program.to_vec();
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    let at = u32::try_from(out.len()).ok()?;
    out.get_mut(entry + 4..entry + 8)?.copy_from_slice(&checksum(&table).to_be_bytes());
    out.get_mut(entry + 8..entry + 12)?.copy_from_slice(&at.to_be_bytes());
    out.get_mut(entry + 12..entry + 16)?.copy_from_slice(&u32::try_from(table.len()).ok()?.to_be_bytes());
    out.extend_from_slice(&table);
    if let Some(head) = head {
        let adjustment = head.checked_add(8)?..head.checked_add(12)?;
        out.get_mut(adjustment.clone())?.fill(0);
        let sum = checksum(&out);
        out.get_mut(adjustment)?.copy_from_slice(&0xb1b0afba_u32.wrapping_sub(sum).to_be_bytes());
    }
    Some(out)
}

/// A `name` table (format 0 or 1) with its identity records saying `name`. The records stay in
/// their order, which the format requires to be sorted. An identity record in an encoding that
/// can't spell `name`, or any record whose string lies outside the table, is left out; a broken
/// language tag fails the table, because records refer to the tags by position.
fn renamed_table(table: &[u8], name: &str) -> Option<Vec<u8>> {
    let format = u16_at(table, 0)?;
    if format > 1 {
        return None;
    }
    let count = usize::from(u16_at(table, 2)?);
    let storage = usize::from(u16_at(table, 4)?);
    let string = |length: u16, offset: u16| table.get(storage.checked_add(usize::from(offset))?..)?.get(..usize::from(length));
    let mut records: Vec<([u16; 4], Vec<u8>)> = Vec::new();
    for at in (0..count).map(|i| 6 + i * 12) {
        let field = |i: usize| u16_at(table, at + 2 * i);
        let key = [field(0)?, field(1)?, field(2)?, field(3)?];
        let [platform, encoding, _, id] = key;
        let text = if IDENTITY.contains(&id) { spelled(name, platform, encoding) } else { string(field(4)?, field(5)?).map(<[u8]>::to_vec) };
        if let Some(text) = text {
            records.push((key, text));
        }
    }
    let mut tags = Vec::new();
    if format == 1 {
        let at = 6 + count * 12;
        for i in 0..usize::from(u16_at(table, at)?) {
            let field = |j: usize| u16_at(table, at + 2 + 4 * i + 2 * j);
            tags.push(string(field(0)?, field(1)?)?.to_vec());
        }
    }
    let tag_part = if format == 1 { 2 + tags.len() * 4 } else { 0 };
    let mut out = Vec::new();
    let mut strings = Vec::new();
    let mut push = |out: &mut Vec<u8>, text: &[u8]| -> Option<()> {
        out.extend_from_slice(&u16::try_from(text.len()).ok()?.to_be_bytes());
        out.extend_from_slice(&u16::try_from(strings.len()).ok()?.to_be_bytes());
        strings.extend_from_slice(text);
        Some(())
    };
    out.extend_from_slice(&format.to_be_bytes());
    out.extend_from_slice(&u16::try_from(records.len()).ok()?.to_be_bytes());
    out.extend_from_slice(&u16::try_from(6 + records.len() * 12 + tag_part).ok()?.to_be_bytes());
    for (key, text) in &records {
        key.iter().for_each(|k| out.extend_from_slice(&k.to_be_bytes()));
        push(&mut out, text)?;
    }
    if format == 1 {
        out.extend_from_slice(&u16::try_from(tags.len()).ok()?.to_be_bytes());
        for tag in &tags {
            push(&mut out, tag)?;
        }
    }
    out.extend_from_slice(&strings);
    Some(out)
}

/// `name` (ASCII) as a `name` record in this platform and encoding: UTF-16BE for Unicode and the
/// Windows Unicode and symbol encodings, bytes for Macintosh (every Mac script encoding starts
/// with ASCII); `None` for the legacy Windows CJK encodings and the ISO and custom platforms.
fn spelled(name: &str, platform: u16, encoding: u16) -> Option<Vec<u8>> {
    match (platform, encoding) {
        (0, _) | (3, 0 | 1 | 10) => Some(name.encode_utf16().flat_map(u16::to_be_bytes).collect()),
        (1, _) => Some(name.as_bytes().to_vec()),
        _ => None,
    }
}
