//! Convert spot inks to process colours, for a press without a plate for them. A `Separation`
//! or `DeviceN` space naming the chosen spot inks, whose alternate is `DeviceCMYK`, is replaced
//! wherever it is used by `DeviceCMYK`, with each colour computed by the space's tint transform
//! (ISO 32000-1 §8.6.6.4, §8.6.6.5): the same CMYK the page shows on screen, now printed on the
//! process plates.
//!
//! What is rewritten: `cs`/`CS` and `sc`/`scn`/`SC`/`SCN` in page content, forms, tiling
//! patterns and annotation appearances (as `k`/`K`; `cs` gives the space's initial colour, every
//! tint 1.0, §8.6.6.4); images (as 8-bit `DeviceCMYK`, through their `/Decode`); `Indexed`
//! spaces over a converted space (their lookup table, §8.6.6.3); and axial and radial shadings
//! (sampled at [`SHADING_SEGMENTS`] + 1 points and joined linearly).
//!
//! The space's own array is never rewritten, so anything not reached here (a Type 3 glyph, a
//! soft mask, a refused object) still draws correctly, with the spot.
//!
//! Refused, and listed in [`SpotsConverted::refused`]: a space whose alternate is not
//! `DeviceCMYK` (that is a colour conversion), a `DeviceN` that also names a spot not chosen, a
//! tint transform that can't be read or doesn't map the space's tints to 4 values, an uncoloured
//! pattern or inline image painting in the space, other shading types, images whose data isn't
//! decoded here (picture codecs) or that carry a colour-key mask, and short lookup tables.
//!
//! Overprint: an overprinted spot paint left the process plates alone. As `DeviceCMYK` it
//! replaces them where its components are nonzero, and under overprint mode 0 (and always for
//! images and shadings, §8.6.7) where they are zero too. So every stream that paints a converted
//! colour with overprint on is listed in [`SpotsConverted::overprint_changed`], in either mode.

mod content;
mod samples;
mod tint;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::EditError;
use tint::Func;

pub use samples::SHADING_SEGMENTS;

/// Nesting followed inside one object.
const MAX_NESTING: usize = 24;
/// The most colorants one `DeviceN` space may name (ISO 32000-1 Annex C).
const MAX_COLORANTS: usize = 32;
/// Refusals listed; past it they are counted in a note.
const MAX_REFUSALS: usize = 256;
/// The longest name a PDF reader has to accept (ISO 32000-1 Annex C).
const MAX_INK_NAME: usize = 127;
/// Inks one call may name.
const MAX_INKS: usize = 256;
/// The colorants that are not spots.
const NOT_SPOTS: [&[u8]; 6] = [b"Cyan", b"Magenta", b"Yellow", b"Black", b"All", b"None"];

/// Something that could not be converted, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpotRefusal {
    /// The ink, as text (empty when the refusal concerns a whole stream).
    pub ink: String,
    /// The object it was found in, when known.
    pub object: Option<ObjRef>,
    pub reason: String,
}

/// What [`convert_spots_to_process`] changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpotsConverted {
    /// Distinct colour spaces converted.
    pub spaces: usize,
    /// Colour operators rewritten.
    pub paints: usize,
    pub images: usize,
    /// `Indexed` lookup tables rewritten.
    pub lookups: usize,
    pub shadings: usize,
    /// Objects rewritten.
    pub objects: usize,
    pub refused: Vec<SpotRefusal>,
    /// Streams that paint a converted colour with overprint on, whose result on the process
    /// plates is now different.
    pub overprint_changed: Vec<ObjRef>,
    /// What else the caller should be told.
    pub notes: Vec<String>,
}

impl SpotsConverted {
    /// Colours converted, of every kind.
    #[must_use]
    pub fn converted(&self) -> usize {
        self.paints + self.images + self.lookups + self.shadings
    }

    fn refuse(&mut self, object: Option<ObjRef>, ink: &[u8], reason: String) {
        let r = SpotRefusal { ink: ink_text(ink), object, reason };
        if self.refused.contains(&r) {
            return;
        }
        if self.refused.len() < MAX_REFUSALS {
            self.refused.push(r);
        } else if self.refused.len() == MAX_REFUSALS {
            self.disclose(format!("More than {MAX_REFUSALS} things could not be converted; only the first are listed."));
        }
    }

    fn disclose(&mut self, note: String) {
        if !self.notes.contains(&note) {
            self.notes.push(note);
        }
    }

    fn overprinted(&mut self, at: ObjRef) {
        if !self.overprint_changed.contains(&at) {
            self.overprint_changed.push(at);
        }
    }
}

/// An ink name as text: its UTF-8 as is, and `#XX` for `#`, a space or control byte, and any byte
/// that isn't UTF-8.
fn ink_text(name: &[u8]) -> String {
    let mut out = String::new();
    for chunk in name.utf8_chunks() {
        for c in chunk.valid().chars() {
            if c == '#' || c.is_ascii_whitespace() || c.is_control() {
                let mut buf = [0u8; 4];
                c.encode_utf8(&mut buf).bytes().for_each(|b| out.push_str(&format!("#{b:02X}")));
            } else {
                out.push(c);
            }
        }
        chunk.invalid().iter().for_each(|b| out.push_str(&format!("#{b:02X}")));
    }
    out
}

/// The bytes of an ink name given as text, with `#XX` for a byte; `None` when a `#` isn't
/// followed by two hex digits.
fn ink_bytes(name: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(name.len());
    let mut rest = name.as_bytes();
    while let Some((&b, tail)) = rest.split_first() {
        if b == b'#' {
            let hex = tail.get(..2).filter(|h| h.iter().all(u8::is_ascii_hexdigit)).and_then(|h| std::str::from_utf8(h).ok())?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            rest = tail.get(2..).unwrap_or_default();
        } else {
            out.push(b);
            rest = tail;
        }
    }
    Some(out)
}

/// Why the ink names `inks` can't be converted, or `None` when they can. Names are UTF-8 text
/// with `#` and two hex digits for any other byte and for `#` itself, as in a PDF name.
#[must_use]
pub fn spot_selection_refusal(inks: &[String]) -> Option<String> {
    if inks.len() > MAX_INKS {
        return Some(format!("at most {MAX_INKS} inks can be named at once"));
    }
    for ink in inks {
        let Some(bytes) = ink_bytes(ink) else {
            return Some("a # in an ink name must be followed by two hex digits".into());
        };
        if bytes.is_empty() || bytes.len() > MAX_INK_NAME {
            return Some(format!("an ink name is 1 to {MAX_INK_NAME} bytes"));
        }
        if NOT_SPOTS.contains(&bytes.as_slice()) {
            return Some(format!("{ink} is not a spot ink"));
        }
    }
    None
}

/// Which spot inks convert: every one, or those named.
enum Selection {
    All,
    Only(Vec<Vec<u8>>),
}

impl Selection {
    fn selects(&self, name: &[u8]) -> bool {
        match self {
            Selection::All => true,
            Selection::Only(names) => names.iter().any(|n| n == name),
        }
    }
}

/// A `Separation` or `DeviceN` space, read.
struct InkSpace<'a> {
    names: Vec<Vec<u8>>,
    alternate: &'a Object,
    tint: &'a Object,
    capped: bool,
}

fn ink_space<'a>(doc: &Document, items: &'a [Object]) -> Option<InkSpace<'a>> {
    let head = doc.resolve(items.first()?);
    let (alternate, tint) = (items.get(2)?, items.get(3)?);
    match head.as_name()? {
        b"Separation" => {
            let name = doc.resolve(items.get(1)?).as_name()?.to_vec();
            Some(InkSpace { names: vec![name], alternate, tint, capped: false })
        }
        b"DeviceN" => {
            let list = doc.resolve(items.get(1)?);
            let all = list.as_array()?;
            let names = all.iter().take(MAX_COLORANTS).filter_map(|n| doc.resolve(n).as_name().map(<[u8]>::to_vec)).collect();
            Some(InkSpace { names, alternate, tint, capped: all.len() > MAX_COLORANTS })
        }
        _ => None,
    }
}

fn is_spot(name: &[u8]) -> bool {
    !NOT_SPOTS.contains(&name)
}

/// How one convertible space maps its tints to CMYK.
struct Conversion {
    n: usize,
    tint: Func,
    inks: Vec<Vec<u8>>,
}

impl Conversion {
    /// CMYK for `tints`, each clamped to 0..1; `None` for the wrong number of tints.
    fn cmyk(&self, tints: &[f64]) -> Option<[f64; 4]> {
        if tints.len() != self.n {
            return None;
        }
        let out = self.tint.eval(tints)?;
        let c = |i: usize| out.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0);
        Some([c(0), c(1), c(2), c(3)])
    }

    /// CMYK for the space's initial colour, every tint 1.0 (§8.6.6.4).
    fn initial(&self) -> Option<[f64; 4]> {
        self.cmyk(&vec![1.0; self.n])
    }

    fn ink(&self) -> &[u8] {
        self.inks.first().map_or(b"".as_slice(), Vec::as_slice)
    }
}

/// Which spaces convert, decided once per distinct space.
struct Spaces<'d> {
    doc: &'d Document,
    selection: Selection,
    memo: HashMap<Vec<u8>, Option<usize>>,
    conversions: Vec<Conversion>,
    /// Every ink some space names, for telling the caller about names none defines.
    defined: BTreeSet<Vec<u8>>,
}

impl<'d> Spaces<'d> {
    /// The conversion for colour space `cs`, or `None` when it is not a chosen space or can't be
    /// converted (the refusal recorded once).
    fn lookup(&mut self, cs: &Object, at: Option<ObjRef>, report: &mut SpotsConverted) -> Option<usize> {
        let resolved = self.doc.resolve(cs);
        let items = resolved.as_array()?;
        let mut key = Vec::new();
        pdfcraft_cos::serialize(&resolved, &mut key);
        if let Some(hit) = self.memo.get(&key) {
            return *hit;
        }
        let hit = self.decide(items, cs.as_ref().or(at), report).map(|c| {
            self.conversions.push(c);
            report.spaces += 1;
            self.conversions.len() - 1
        });
        self.memo.insert(key, hit);
        hit
    }

    fn decide(&self, items: &[Object], at: Option<ObjRef>, report: &mut SpotsConverted) -> Option<Conversion> {
        let space = ink_space(self.doc, items)?;
        let spots: Vec<&Vec<u8>> = space.names.iter().filter(|n| is_spot(n)).collect();
        let first = *spots.iter().find(|n| self.selection.selects(n))?;
        if let Some(other) = spots.iter().find(|n| !self.selection.selects(n)) {
            report.refuse(at, first, format!("its colour space also names the spot ink {}, which was not chosen", ink_text(other)));
            return None;
        }
        if space.capped {
            report.refuse(at, first, format!("its DeviceN space names more than {MAX_COLORANTS} colorants"));
            return None;
        }
        if self.doc.resolve(space.alternate).as_name() != Some(b"DeviceCMYK") {
            let why = "its alternate space is not DeviceCMYK, so converting it would be a colour conversion";
            report.refuse(at, first, why.into());
            return None;
        }
        let n = space.names.len();
        let tint = match Func::load(self.doc, space.tint) {
            Ok(f) => f,
            Err(e) => {
                report.refuse(at, first, format!("its tint transform cannot be read: {e}"));
                return None;
            }
        };
        let outputs = tint.eval(&vec![1.0; n]).map_or(0, |o| o.len());
        if tint.inputs() != n || outputs != 4 {
            report.refuse(at, first, format!("its tint transform takes {} and gives {outputs} values, not {n} and 4", tint.inputs()));
            return None;
        }
        Some(Conversion { n, tint, inks: spots.into_iter().cloned().collect() })
    }
}

/// Objects rewritten so far, over the document's own.
struct Work<'d> {
    doc: &'d Document,
    objects: BTreeMap<ObjRef, Object>,
}

impl Work<'_> {
    fn current(&self, at: ObjRef) -> Object {
        self.objects.get(&at).cloned().unwrap_or_else(|| (*self.doc.get(at)).clone())
    }
}

/// Convert the spot inks `inks` (every spot ink when empty) to process colours throughout the
/// document; see the module doc. Nothing named is not an error: the counts are zero.
///
/// # Errors
///
/// [`EditError::Invalid`] when [`spot_selection_refusal`] refuses the names.
pub fn convert_spots_to_process(doc: &mut Document, inks: &[String]) -> Result<SpotsConverted, EditError> {
    if let Some(why) = spot_selection_refusal(inks) {
        return Err(EditError::Invalid(why));
    }
    let named: Vec<Vec<u8>> = inks.iter().filter_map(|i| ink_bytes(i)).collect();
    let selection = if named.is_empty() { Selection::All } else { Selection::Only(named.clone()) };
    let mut report = SpotsConverted::default();
    let writes = {
        let doc: &Document = doc;
        let mut spaces = Spaces { doc, selection, memo: HashMap::new(), conversions: Vec::new(), defined: BTreeSet::new() };
        let mut work = Work { doc, objects: BTreeMap::new() };
        content::rewrite_pages(&mut spaces, &mut work, &mut report);
        convert_objects(&mut spaces, &mut work, &mut report);
        for name in named.iter().filter(|n| !spaces.defined.contains(*n)) {
            report.disclose(format!("No colour space names the ink {}.", ink_text(name)));
        }
        work.objects
    };
    if report.shadings > 0 {
        report.disclose(format!("Converted shadings are sampled at {} points and joined linearly.", SHADING_SEGMENTS + 1));
    }
    report.objects = writes.len();
    for (at, obj) in writes {
        doc.set(at, obj);
    }
    Ok(report)
}

/// Images, `Indexed` spaces and shadings, in every object of the document.
fn convert_objects(spaces: &mut Spaces<'_>, work: &mut Work<'_>, report: &mut SpotsConverted) {
    let doc = spaces.doc;
    for (at, read) in doc.scan_objects_checked() {
        if read.is_err() {
            report.disclose("An object of the file could not be read; anything in it was not converted.".into());
            continue;
        }
        let mut obj = work.current(at);
        if let Object::Stream(s) = &obj
            && s.dict.name(b"Subtype") == Some(b"Image")
            && let Some(converted) = samples::image(spaces, at, s, report)
        {
            work.objects.insert(at, Object::Stream(converted));
            report.images += 1;
            continue;
        }
        if objects(spaces, &mut obj, Some(at), 0, report) {
            work.objects.insert(at, obj);
        }
    }
}

/// Rewrite `Indexed` spaces and shadings inside `obj`; true when it changed.
fn objects(spaces: &mut Spaces<'_>, obj: &mut Object, at: Option<ObjRef>, depth: usize, report: &mut SpotsConverted) -> bool {
    if depth > MAX_NESTING {
        return false;
    }
    match obj {
        Object::Array(items) => {
            if let Some(space) = ink_space(spaces.doc, items) {
                spaces.defined.extend(space.names);
            }
            if let Some(converted) = samples::indexed(spaces, items, at, report) {
                *obj = converted;
                report.lookups += 1;
                return true;
            }
            let mut changed = false;
            for item in items.iter_mut() {
                changed |= objects(spaces, item, at, depth + 1, report);
            }
            changed
        }
        Object::Dict(d) => entries(spaces, d, at, depth, report),
        Object::Stream(s) => entries(spaces, &mut s.dict, at, depth, report),
        _ => false,
    }
}

fn entries(spaces: &mut Spaces<'_>, d: &mut Dict, at: Option<ObjRef>, depth: usize, report: &mut SpotsConverted) -> bool {
    if d.contains(b"ShadingType") && samples::shading(spaces, d, at, report) {
        report.shadings += 1;
        return true;
    }
    let keys: Vec<Vec<u8>> = d.iter().map(|(k, _)| k.clone()).collect();
    let mut changed = false;
    for k in keys {
        if let Some(v) = d.get_mut(&k) {
            changed |= objects(spaces, v, at, depth + 1, report);
        }
    }
    changed
}

/// `/DeviceCMYK`.
fn device_cmyk() -> Object {
    Object::Name(b"DeviceCMYK".to_vec())
}

/// `v` rounded to 4 decimals.
fn real(v: f64) -> Object {
    Object::Real((v * 10_000.0).round() / 10_000.0)
}
