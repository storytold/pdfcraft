//! Sweeping the serialized output for the removed strings, on every surface a reader could
//! recover them from.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};

use super::extract::extract_page;
use super::util::{Ctx, decode_text, find, image_codec, spellings, squash, survivor_encodings, xml_unescape};
use super::{SURFACES, Surface, SurfaceReport, Verdict};
use crate::verify::limits::{MAX_NESTING, MAX_WALK, MIN_NEEDLE, MIN_STRUCTURED_NEEDLE};

/// A string the sweep looks for.
pub(super) struct Needle {
    /// The code bytes as Latin-1 text (the "raw view" of a page) rather than decoded text.
    pub(super) raw_view: bool,
    /// As removed, whitespace included (for the byte encodings).
    pub(super) shown: String,
    /// `shown` without whitespace (for page text, which is compared whitespace-blind).
    pub(super) key: String,
    /// What the raw file may spell it as.
    pub(super) bytes: Vec<Vec<u8>>,
    pub(super) owners: BTreeSet<usize>,
    /// Occurrences that legitimately remain in content that was not redacted. The surfaces that
    /// hold page content (the page text, the raw file, decoded streams) count occurrences over
    /// the whole document and fail on a surplus; every other surface (strings, Info, XMP,
    /// attachments) is swept with no allowance.
    pub(super) allowance: usize,
    /// Names of the input that carry this needle (a redacted word that is also a `/Subtype`, a
    /// `/Filter`, a `/BaseFont`). Names are structure redaction never edits, so the name surface
    /// is swept against the input's own matches: the output may hold as many as the input did,
    /// and only a surplus is a survivor.
    pub(super) name_allowance: usize,
}

impl Needle {
    pub(super) fn short(&self) -> bool {
        self.key.chars().count() < MIN_NEEDLE
    }

    /// Long enough to be looked for in bytes (`min`: the shortest the haystack allows).
    pub(super) fn at_least(&self, min: usize) -> bool {
        self.key.chars().count() >= min
    }
}

pub(super) fn add_needle(set: &mut Vec<Needle>, raw_view: bool, shown: String, owner: Option<usize>) {
    let key = squash(&shown);
    if key.is_empty() {
        return;
    }
    if let Some(n) = set.iter_mut().find(|n| n.raw_view == raw_view && n.key == key) {
        n.owners.extend(owner);
        return;
    }
    let bytes = if raw_view {
        // The code bytes themselves (Latin-1 round-trips them).
        let raw: Vec<u8> = shown.chars().filter_map(|c| u8::try_from(u32::from(c)).ok()).collect();
        spellings([raw])
    } else {
        survivor_encodings(&shown)
    };
    set.push(Needle { raw_view, shown, key, bytes, owners: owner.into_iter().collect(), allowance: 0, name_allowance: 0 });
}

pub(super) struct Acc {
    pub(super) reps: BTreeMap<Surface, SurfaceReport>,
    pub(super) failed_needles: HashSet<usize>,
    pub(super) ctx: Ctx,
}

impl Acc {
    pub(super) fn new(ctx: Ctx) -> Self {
        let mut a = Acc { reps: BTreeMap::new(), failed_needles: HashSet::new(), ctx };
        for s in SURFACES {
            a.rep(s);
        }
        a
    }

    pub(super) fn rep(&mut self, surface: Surface) -> &mut SurfaceReport {
        self.reps.entry(surface).or_insert_with(|| SurfaceReport {
            surface,
            verdict: Verdict::Absent,
            scanned: 0,
            raw_only: 0,
            survivors: 0,
            locations: Vec::new(),
            problems: Vec::new(),
        })
    }

    pub(super) fn scanned(&mut self, s: Surface) {
        self.rep(s).scanned += 1;
    }

    pub(super) fn raw_only(&mut self, s: Surface) {
        self.rep(s).raw_only += 1;
    }

    pub(super) fn survivor(&mut self, s: Surface, loc: String, needle: Option<usize>) {
        self.survivors(s, loc, needle, 1);
    }

    /// `count` survivors at one place.
    pub(super) fn survivors(&mut self, s: Surface, loc: String, needle: Option<usize>, count: usize) {
        if let Some(n) = needle {
            self.failed_needles.insert(n);
        }
        let r = self.rep(s);
        r.survivors = r.survivors.saturating_add(count);
        if !r.locations.contains(&loc) && r.locations.len() < 64 {
            r.locations.push(loc);
        }
    }

    pub(super) fn problem(&mut self, s: Surface, msg: String) {
        let r = self.rep(s);
        if r.problems.len() < 64 {
            r.problems.push(msg);
        }
    }

    /// The needles (at least `min` long) found in `hay`; running out of search work is a
    /// problem of `surface`.
    pub(super) fn bytes(&mut self, surface: Surface, needles: &[Needle], hay: &[u8], min: usize) -> Vec<usize> {
        bytes_hits(needles, hay, min, &self.ctx).unwrap_or_else(|| {
            self.problem(surface, "the search work limit was reached".into());
            Vec::new()
        })
    }

    /// How often each needle (at least `min` long) occurs in `hay`, as `(needle, count)`.
    pub(super) fn counts(&mut self, surface: Surface, needles: &[Needle], hay: &[u8], min: usize) -> Vec<(usize, usize)> {
        bytes_counts(needles, hay, min, &self.ctx).unwrap_or_else(|| {
            self.problem(surface, "the search work limit was reached".into());
            Vec::new()
        })
    }

    /// Survivors and problems so far, over all surfaces.
    pub(super) fn totals(&self) -> (usize, usize) {
        (self.reps.values().map(|r| r.survivors).sum(), self.reps.values().map(|r| r.problems.len()).sum())
    }

    pub(super) fn finish(mut self) -> Vec<SurfaceReport> {
        for r in self.reps.values_mut() {
            r.verdict = if r.survivors > 0 {
                Verdict::Survivor
            } else if !r.problems.is_empty() {
                Verdict::Unswept
            } else if r.scanned + r.raw_only == 0 {
                Verdict::Absent
            } else {
                Verdict::Clean
            };
        }
        self.reps.into_values().collect()
    }
}

/// Occurrences of needles on one surface, per place, so that what legitimately remains
/// (a needle's allowance) is taken off the document-wide total once, not once per place.
#[derive(Default)]
pub(super) struct Tally(BTreeMap<usize, Vec<(String, usize)>>);

impl Tally {
    pub(super) fn add(&mut self, needle: usize, loc: &str, count: usize) {
        if count > 0 {
            self.0.entry(needle).or_default().push((loc.to_string(), count));
        }
    }

    /// Report what is left of each needle's occurrences after its allowance (none inside an
    /// attachment, where nothing legitimately remains).
    pub(super) fn settle(self, surface: Surface, needles: &[Needle], depth: usize, acc: &mut Acc) {
        self.settle_with(surface, needles, depth, acc, |n| n.allowance)
    }

    /// The name surface: the input's own name matches are the allowance (redaction never edits
    /// names), so what is left is a surplus of matches over the input.
    pub(super) fn settle_names(self, surface: Surface, needles: &[Needle], depth: usize, acc: &mut Acc) {
        self.settle_with(surface, needles, depth, acc, |n| n.name_allowance)
    }

    fn settle_with(self, surface: Surface, needles: &[Needle], depth: usize, acc: &mut Acc, allow_of: impl Fn(&Needle) -> usize) {
        for (ni, places) in self.0 {
            let mut allow = if depth == 0 { needles.get(ni).map_or(0, &allow_of) } else { 0 };
            for (loc, count) in places {
                let used = count.min(allow);
                allow -= used;
                if count > used {
                    acc.survivors(surface, loc, Some(ni), count - used);
                }
            }
        }
    }
}

/// Search all encodings in one bounded pass, including overlapping matches.
fn pattern_matches<'a>(
    patterns: impl IntoIterator<Item = (usize, &'a [u8])>,
    hay: &[u8],
    ctx: &Ctx,
    mut found: impl FnMut(usize, usize, usize) -> bool,
) -> Option<()> {
    const MAX_PATTERNS: usize = 100_000;
    const MAX_PATTERN_BYTES: usize = 16 << 20;
    const MAX_MATCHES: usize = 1 << 20;
    if !ctx.charge(hay.len()) {
        return None;
    }
    let mut map: BTreeMap<Vec<u8>, Vec<usize>> = BTreeMap::new();
    let (mut bytes, mut links) = (0usize, 0usize);
    for (owner, pattern) in patterns {
        if pattern.is_empty() {
            continue;
        }
        if !ctx.charge(pattern.len().saturating_add(1)) {
            return None;
        }
        if !map.contains_key(pattern) {
            bytes = bytes.checked_add(pattern.len())?;
            if bytes > MAX_PATTERN_BYTES || map.len() >= MAX_PATTERNS {
                return None;
            }
        }
        let owners = map.entry(pattern.to_vec()).or_default();
        if owners.last() != Some(&owner) {
            links = links.checked_add(1)?;
            if links > MAX_PATTERNS {
                return None;
            }
            owners.push(owner);
        }
    }
    if map.is_empty() {
        return Some(());
    }
    let matcher = aho_corasick::AhoCorasick::new(map.keys()).ok()?;
    let owners: Vec<Vec<usize>> = map.into_values().collect();
    let mut matches = 0usize;
    for hit in matcher.find_overlapping_iter(hay) {
        for &owner in owners.get(hit.pattern().as_usize())? {
            matches = matches.checked_add(1)?;
            if matches > MAX_MATCHES || !ctx.charge(1) || !found(owner, hit.start(), hit.end()) {
                return None;
            }
        }
    }
    Some(())
}

/// Count normalized text keys in one pass, with the same non-overlap rule as `str::matches`.
pub(super) fn key_counts(needles: &[Needle], hay: &str, raw: bool, ctx: &Ctx) -> Option<Vec<(usize, usize)>> {
    let patterns = needles.iter().enumerate().filter(|(_, n)| n.raw_view == raw).map(|(i, n)| (i, n.key.as_bytes()));
    let mut spans: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    pattern_matches(patterns, hay.as_bytes(), ctx, |i, a, b| {
        let hits = spans.entry(i).or_default();
        if hits.len() >= MAX_COUNTED {
            return false;
        }
        hits.push((a, b));
        true
    })?;
    Some(
        spans
            .into_iter()
            .map(|(i, mut hits)| {
                hits.sort_unstable();
                let (mut count, mut end) = (0, 0);
                for (a, b) in hits {
                    if a >= end {
                        count += 1;
                        end = b;
                    }
                }
                (i, count)
            })
            .collect(),
    )
}

pub(super) fn bytes_hits(needles: &[Needle], hay: &[u8], min: usize, ctx: &Ctx) -> Option<Vec<usize>> {
    let patterns = needles.iter().enumerate().filter(|(_, n)| n.at_least(min)).flat_map(|(i, n)| n.bytes.iter().map(move |b| (i, b.as_slice())));
    let mut found = BTreeSet::new();
    pattern_matches(patterns, hay, ctx, |i, _, _| {
        found.insert(i);
        true
    })?;
    Some(found.into_iter().collect())
}

/// A larger count is incomplete evidence, never a silently truncated count.
pub(super) const MAX_COUNTED: usize = 1 << 16;

pub(super) fn bytes_counts(needles: &[Needle], hay: &[u8], min: usize, ctx: &Ctx) -> Option<Vec<(usize, usize)>> {
    let patterns = needles.iter().enumerate().filter(|(_, n)| n.at_least(min)).flat_map(|(i, n)| n.bytes.iter().map(move |b| (i, b.as_slice())));
    let mut spans: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    pattern_matches(patterns, hay, ctx, |i, a, b| {
        let hits = spans.entry(i).or_default();
        if hits.len() >= MAX_COUNTED {
            return false;
        }
        hits.push((a, b));
        true
    })?;
    Some(
        spans
            .into_iter()
            .map(|(i, mut spans)| {
                spans.sort_unstable();
                spans.dedup();
                let (mut count, mut end) = (0usize, 0usize);
                for (a, b) in spans {
                    if count == 0 || a >= end {
                        count += 1;
                    }
                    end = end.max(b);
                }
                (i, count)
            })
            .collect(),
    )
}

pub(super) fn string_hits(needles: &[Needle], value: &PdfString, ctx: &Ctx) -> Option<Vec<usize>> {
    let text = value.to_text();
    let key = squash(&text);
    if !ctx.charge(value.bytes.len().saturating_add(key.len())) {
        return None;
    }
    let mut found: BTreeSet<usize> = bytes_hits(needles, &value.bytes, MIN_NEEDLE, ctx)?.into_iter().collect();
    let patterns = needles.iter().enumerate().filter(|(_, n)| !n.raw_view && !n.short()).map(|(i, n)| (i, n.key.as_bytes()));
    pattern_matches(patterns, key.as_bytes(), ctx, |i, _, _| {
        found.insert(i);
        true
    })?;
    let tokens: BTreeSet<&str> = text.split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).collect();
    for (i, needle) in needles.iter().enumerate().filter(|(_, n)| n.short() && n.allowance == 0 && !n.raw_view) {
        if !ctx.charge(needle.shown.len().saturating_add(1)) {
            return None;
        }
        if tokens.contains(needle.shown.as_str()) {
            found.insert(i);
        }
    }
    Some(found.into_iter().collect())
}

/// Streams whose data is binary rather than text: images, embedded font programs, colour
/// profiles, cross-reference and object streams (the objects in the latter are swept as
/// objects). Their decoded data is swept like any other stream's; the pixels of an image codec
/// stay undecoded (text that exists only as pixels is not text), and one that won't decode (or is
/// past the cap) is searched as stored bytes only. A short needle "found" in stored bytes is
/// chance.
pub(super) fn binary_stream(d: &Dict) -> bool {
    let subtype = d.name(b"Subtype");
    subtype == Some(b"Image".as_slice())
        || matches!(d.name(b"Type"), Some(b"XRef" | b"ObjStm"))
        || matches!(subtype, Some(b"Type1C" | b"CIDFontType0C" | b"OpenType"))
        || [b"Length1".as_slice(), b"Length2", b"Length3"].iter().any(|k| d.contains(k))
        || (d.contains(b"N") && !d.contains(b"Type") && subtype.is_none())
}

/// What a file identifier (`/ID` of the trailer) is spelled as in the file.
pub(super) fn id_spellings(doc: &Document) -> Vec<Vec<u8>> {
    let Some(Object::Array(ids)) = doc.trailer().get(b"ID").map(|o| doc.resolve(o)).as_deref().cloned() else { return Vec::new() };
    ids.iter().filter_map(Object::as_string).flat_map(|s| spellings([s.bytes.to_vec()])).collect()
}

/// Visit `o` and everything nested in it. False when something lay deeper than [`MAX_WALK`]
/// and was not visited: the caller reports that as unswept.
pub(super) fn walk_strings(o: &Object, depth: usize, f: &mut impl FnMut(&Object)) -> bool {
    if depth > MAX_WALK {
        return false;
    }
    f(o);
    // Every child is visited even after one is too deep: nothing is skipped silently.
    let mut ok = true;
    match o {
        Object::Array(a) => a.iter().for_each(|x| ok &= walk_strings(x, depth + 1, f)),
        Object::Dict(d) => d.iter().for_each(|(_, v)| ok &= walk_strings(v, depth + 1, f)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, v)| ok &= walk_strings(v, depth + 1, f)),
        _ => {}
    }
    ok
}

/// Sweep a document (the output, or an attachment) on every surface.
pub(super) fn sweep_doc(needles: &[Needle], doc: &Document, bytes: &[u8], depth: usize, tag: &str, acc: &mut Acc) {
    let (embedded, overlay_raw) = sweep_objects(needles, doc, depth, tag, acc);
    sweep_file(needles, doc, bytes, overlay_raw, depth, tag, acc);
    sweep_pages(needles, doc, depth, tag, acc);
    sweep_info(needles, doc, tag, acc);
    sweep_xmp(needles, doc, tag, acc);

    // Revision structure: one revision, no earlier content kept.
    acc.scanned(Surface::Revisions);
    let revs = doc.revisions().len();
    if revs > 1 || doc.trailer().contains(b"Prev") {
        acc.survivor(Surface::Revisions, format!("{tag}{revs} revisions: earlier ones keep the original content"), None);
    }

    sweep_attachments(needles, doc, &embedded, depth, tag, acc);
}

/// Every object: strings, names and stream data (orphans included). Returns the embedded file
/// streams found, and the raw bytes of the overlay streams (to leave out of the raw sweep).
fn sweep_objects(needles: &[Needle], doc: &Document, depth: usize, tag: &str, acc: &mut Acc) -> (BTreeSet<u32>, Vec<Vec<u8>>) {
    let mut embedded: BTreeSet<u32> = BTreeSet::new();
    let mut overlay_raw: Vec<Vec<u8>> = Vec::new();
    let mut in_streams = Tally::default();
    let mut in_names = Tally::default();
    let lookup = lookup_streams(doc);
    for num in doc.object_numbers() {
        let loc = format!("{tag}object {num}");
        let obj = match doc.try_get(num) {
            Ok(o) => o,
            Err(_) => {
                acc.problem(Surface::ObjectStrings, format!("{loc} can't be read"));
                continue;
            }
        };
        let mut hits: BTreeSet<usize> = BTreeSet::new();
        let mut spent = false;
        let ctx = &acc.ctx;
        // The file identifier is random bytes, never text: a short needle must not "read" it.
        let without_id = match &*obj {
            Object::Stream(s) if s.dict.name(b"Type") == Some(b"XRef") => {
                let mut d = s.dict.clone();
                d.remove(b"ID");
                Some(Object::Dict(d))
            }
            _ => None,
        };
        let complete = walk_strings(without_id.as_ref().unwrap_or(&obj), 0, &mut |o| match o {
            Object::String(s) => match string_hits(needles, s, ctx) {
                Some(h) => hits.extend(h),
                None => spent = true,
            },
            Object::Name(n) => match bytes_hits(needles, n, MIN_NEEDLE, ctx) {
                // A name the input had its own matches in is structure, not a leftover copy:
                // the matches are tallied and settled against the input's count.
                Some(h) => h.into_iter().for_each(|ni| in_names.add(ni, &loc, 1)),
                None => spent = true,
            },
            _ => {}
        });
        if !complete {
            acc.problem(Surface::ObjectStrings, format!("{loc} is nested too deeply to sweep in full"));
        }
        if spent {
            acc.problem(Surface::ObjectStrings, "the search work limit was reached".into());
        }
        acc.scanned(Surface::ObjectStrings);
        for n in hits {
            acc.survivor(Surface::ObjectStrings, loc.clone(), Some(n));
        }
        // File specifications, wherever they sit (a name tree keeps them as separate objects,
        // annotations and collections may inline them).
        walk_strings(&obj, 0, &mut |o| {
            if let Object::Dict(d) = o
                && let Some(Object::Dict(ef)) = d.get(b"EF")
            {
                embedded.extend(ef.iter().filter_map(|(_, v)| v.as_ref()).map(|r| r.num));
            }
        });
        if let Object::Stream(s) = &*obj {
            if s.dict.name(b"Type") == Some(b"EmbeddedFile") {
                embedded.insert(num);
            }
            acc.scanned(Surface::DecodedStreams);
            if binary_stream(&s.dict) && image_codec(&s.dict) {
                // The pixels of an image codec are still encoded and are not text: their stored
                // bytes are searched for whole strings only.
                acc.raw_only(Surface::DecodedStreams);
                for n in acc.bytes(Surface::DecodedStreams, needles, &s.raw, MIN_STRUCTURED_NEEDLE) {
                    acc.survivor(Surface::DecodedStreams, format!("{loc} (raw)"), Some(n));
                }
            } else if binary_stream(&s.dict) {
                // A compressed copy of the removed text can hide in a font program or colour
                // profile (with /Length1 in its dict, its stored bytes hold no plaintext): the
                // stored bytes and the decoded data are both searched. Such a stream that won't
                // decode (damaged, or past the cap) is searched as stored bytes only and counted
                // as raw only: it holds no page content, and redaction never touches it. A
                // cross-reference or object stream that won't decode is a problem of the proof.
                for n in acc.bytes(Surface::DecodedStreams, needles, &s.raw, MIN_STRUCTURED_NEEDLE) {
                    acc.survivor(Surface::DecodedStreams, format!("{loc} (raw)"), Some(n));
                }
                match acc.ctx.decode(s) {
                    Ok(d) => {
                        let loc = format!("{loc} (decoded)");
                        for (n, c) in acc.counts(Surface::DecodedStreams, needles, &d, MIN_STRUCTURED_NEEDLE) {
                            in_streams.add(n, &loc, c);
                        }
                    }
                    Err(()) if matches!(s.dict.name(b"Type"), Some(b"XRef" | b"ObjStm")) => {
                        acc.problem(Surface::DecodedStreams, format!("{loc} can't be decoded in full"));
                    }
                    Err(()) => acc.raw_only(Surface::DecodedStreams),
                }
            } else {
                match acc.ctx.decode(s) {
                    // The boxes and labels this run drew are not text to look for.
                    Ok(d) if acc.ctx.is_overlay(&d) => overlay_raw.push(s.raw.to_vec()),
                    Ok(d) => {
                        let loc = format!("{loc} (decoded)");
                        for (n, c) in acc.counts(Surface::DecodedStreams, needles, &d, MIN_STRUCTURED_NEEDLE) {
                            in_streams.add(n, &loc, c);
                        }
                    }
                    // Colour-space lookup data and function samples are data tables, not content.
                    Err(()) if lookup.contains(&num) || s.dict.contains(b"FunctionType") => {
                        acc.raw_only(Surface::DecodedStreams);
                        for n in acc.bytes(Surface::DecodedStreams, needles, &s.raw, MIN_STRUCTURED_NEEDLE) {
                            acc.survivor(Surface::DecodedStreams, format!("{loc} (raw)"), Some(n));
                        }
                    }
                    Err(()) => acc.problem(Surface::DecodedStreams, format!("{loc} can't be decoded in full")),
                }
            }
        }
    }
    in_names.settle_names(Surface::ObjectStrings, needles, depth, acc);
    in_streams.settle(Surface::DecodedStreams, needles, depth, acc);
    (embedded, overlay_raw)
}

/// The streams that hold the lookup table of an `/Indexed` colour space (object numbers).
fn lookup_streams(doc: &Document) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    for num in doc.object_numbers() {
        let Ok(obj) = doc.try_get(num) else { continue };
        walk_strings(&obj, 0, &mut |o| {
            if let Object::Array(a) = o
                && matches!(a.first(), Some(Object::Name(n)) if n == b"Indexed" || n == b"I")
                && let Some(Object::Ref(r)) = a.get(3)
            {
                out.insert(r.num);
            }
        });
    }
    out
}

/// The raw bytes of the whole file, the overlay streams (stored as they are, or not at all) and
/// the file identifier (random bytes, regenerated on every write) aside.
fn sweep_file(needles: &[Needle], doc: &Document, bytes: &[u8], overlay_raw: Vec<Vec<u8>>, depth: usize, tag: &str, acc: &mut Acc) {
    acc.scanned(Surface::RawBytes);
    let mut mask = overlay_raw;
    mask.extend(id_spellings(doc));
    let masked = mask_all(bytes, &mask, &acc.ctx);
    let mut in_file = Tally::default();
    for (n, c) in acc.counts(Surface::RawBytes, needles, masked.as_deref().unwrap_or(bytes), MIN_STRUCTURED_NEEDLE) {
        in_file.add(n, &format!("{tag}file"), c);
    }
    in_file.settle(Surface::RawBytes, needles, depth, acc);
    if masked.is_none() && !mask.is_empty() {
        acc.problem(Surface::RawBytes, "the search work limit was reached".into());
    }
}

/// Embedded files, PDF attachments swept in full.
fn sweep_attachments(needles: &[Needle], doc: &Document, embedded: &BTreeSet<u32>, depth: usize, tag: &str, acc: &mut Acc) {
    for &num in embedded {
        let loc = format!("{tag}attachment object {num}");
        acc.scanned(Surface::EmbeddedFiles);
        let obj = doc.get(ObjRef::new(num, doc.generation(num)));
        let Object::Stream(s) = &*obj else {
            acc.problem(Surface::EmbeddedFiles, format!("{loc} is not a stream"));
            continue;
        };
        let payload = match acc.ctx.decode(s) {
            Ok(p) => p,
            Err(()) => {
                acc.problem(Surface::EmbeddedFiles, format!("{loc} can't be decoded in full"));
                continue;
            }
        };
        sweep_payload(needles, &payload, depth, &loc, acc);
    }
}

/// `bytes` with every occurrence of `pats` zeroed (`None` when the work limit ran out, or
/// there is nothing to mask).
pub(super) fn mask_all(bytes: &[u8], pats: &[Vec<u8>], ctx: &Ctx) -> Option<Vec<u8>> {
    if pats.iter().all(Vec::is_empty) {
        return None;
    }
    let mut out = bytes.to_vec();
    for p in pats.iter().filter(|p| !p.is_empty()) {
        if !ctx.charge(bytes.len()) {
            return None;
        }
        let mut at = 0;
        while let Some(w) = bytes.get(at..at + p.len()) {
            if w == p.as_slice() {
                if let Some(z) = out.get_mut(at..at + p.len()) {
                    z.fill(0);
                }
                at += p.len();
            } else {
                at += 1;
            }
        }
    }
    Some(out)
}

/// One attachment: a PDF is swept as a document, text is searched, containers are reported as
/// unswept (their content can't be seen), images are searched as raw bytes only.
pub(super) fn sweep_payload(needles: &[Needle], payload: &[u8], depth: usize, loc: &str, acc: &mut Acc) {
    let head = payload.get(..1024).unwrap_or(payload);
    let tag = format!("{loc} > ");
    if find(head, b"%PDF-") {
        if depth >= MAX_NESTING {
            acc.problem(Surface::EmbeddedFiles, format!("{loc} is nested too deep to sweep"));
            return;
        }
        match Document::open(Arc::new(payload.to_vec())) {
            Ok(inner) => {
                let before = acc.totals();
                sweep_doc(needles, &inner, payload, depth + 1, &tag, acc);
                // What the attachment's own sweep found counts against the attachment too.
                let after = acc.totals();
                if after.0 > before.0 {
                    acc.survivor(Surface::EmbeddedFiles, loc.to_string(), None);
                }
                if after.1 > before.1 {
                    acc.problem(Surface::EmbeddedFiles, format!("{loc} could not be swept in full"));
                }
            }
            Err(_) => acc.problem(Surface::EmbeddedFiles, format!("{loc} is a PDF that can't be opened")),
        }
        return;
    }
    let starts = |sig: &[u8]| payload.starts_with(sig);
    let container = starts(b"PK\x03\x04")
        || starts(b"PK\x05\x06")
        || starts(&[0x1F, 0x8B])
        || starts(b"BZh")
        || starts(&[0xFD, b'7', b'z', b'X', b'Z', 0])
        || starts(&[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C])
        || starts(b"Rar!")
        || starts(&[0xD0, 0xCF, 0x11, 0xE0])
        || payload.get(257..262) == Some(b"ustar");
    if container {
        acc.problem(Surface::EmbeddedFiles, format!("{loc} is an archive or container that can't be opened"));
        return;
    }
    let image = starts(&[0x89, b'P', b'N', b'G']) || starts(&[0xFF, 0xD8, 0xFF]) || starts(b"GIF8");
    if image {
        acc.raw_only(Surface::EmbeddedFiles);
    }
    let min = if image { MIN_STRUCTURED_NEEDLE } else { MIN_NEEDLE };
    for n in acc.bytes(Surface::EmbeddedFiles, needles, payload, min) {
        acc.survivor(Surface::EmbeddedFiles, loc.to_string(), Some(n));
    }
    if !image {
        let key = squash(&decode_text(payload));
        match key_counts(needles, &key, false, &acc.ctx) {
            Some(counts) => {
                for (i, count) in counts {
                    if count > 0 && needles.get(i).is_some_and(|n| n.at_least(MIN_NEEDLE)) {
                        acc.survivor(Surface::EmbeddedFiles, loc.to_string(), Some(i));
                    }
                }
            }
            None => acc.problem(Surface::EmbeddedFiles, format!("{loc} could not be searched in full")),
        }
    }
}

pub(super) fn sweep_pages(needles: &[Needle], doc: &Document, depth: usize, tag: &str, acc: &mut Acc) {
    let mut shown = Tally::default();
    'pages: for (i, p) in pdfcraft_model::pages(doc).iter().enumerate() {
        acc.scanned(Surface::ExtractedText);
        let loc = format!("{tag}page {}", i + 1);
        let Ok(e) = extract_page(doc, &p.dict, &acc.ctx, &[]) else {
            acc.problem(Surface::ExtractedText, format!("{loc} can't be read"));
            continue;
        };
        for (hay, raw) in [(&e.text, false), (&e.raw, true)] {
            let Some(counts) = key_counts(needles, hay, raw, &acc.ctx) else {
                acc.problem(Surface::ExtractedText, "the search work limit was reached".into());
                break 'pages;
            };
            for (ni, count) in counts {
                shown.add(ni, &loc, count);
            }
        }
    }
    // A surplus over what legitimately remains anywhere in the document is a survivor. The
    // counts made before the work ran out are settled too: a survivor already seen is
    // reported where it sits, not hidden behind the limit.
    shown.settle(Surface::ExtractedText, needles, depth, acc);
}

pub(super) fn sweep_info(needles: &[Needle], doc: &Document, tag: &str, acc: &mut Acc) {
    let Some(info) = doc.trailer().get(b"Info").map(|i| doc.resolve(i)) else { return };
    let Some(d) = info.as_dict() else {
        acc.problem(Surface::Info, format!("{tag}Info is not a dictionary"));
        return;
    };
    for (k, v) in d.iter() {
        acc.scanned(Surface::Info);
        let v = doc.resolve(v);
        let mut hits: BTreeSet<usize> = BTreeSet::new();
        let mut spent = false;
        let ctx = &acc.ctx;
        let complete = walk_strings(&v, 0, &mut |o| {
            if let Object::String(s) = o {
                match string_hits(needles, s, ctx) {
                    Some(h) => hits.extend(h),
                    None => spent = true,
                }
            }
        });
        if !complete || spent {
            acc.problem(Surface::Info, format!("{tag}Info /{} can't be swept in full", String::from_utf8_lossy(k)));
        }
        for n in hits {
            acc.survivor(Surface::Info, format!("{tag}Info /{}", String::from_utf8_lossy(k)), Some(n));
        }
    }
}

pub(super) fn sweep_xmp(needles: &[Needle], doc: &Document, tag: &str, acc: &mut Acc) {
    let Some(root) = doc.root() else { return };
    let Some(meta) = doc.get(root).as_dict().and_then(|c| c.get(b"Metadata")).map(|m| doc.resolve(m)) else { return };
    acc.scanned(Surface::Xmp);
    let loc = format!("{tag}XMP metadata");
    let Object::Stream(s) = &*meta else {
        acc.problem(Surface::Xmp, format!("{loc} is not a stream"));
        return;
    };
    let Ok(data) = acc.ctx.decode(s) else {
        acc.problem(Surface::Xmp, format!("{loc} can't be decoded in full"));
        return;
    };
    let mut hits: BTreeSet<usize> = acc.bytes(Surface::Xmp, needles, &data, MIN_NEEDLE).into_iter().collect();
    let key = squash(&xml_unescape(&decode_text(&data)));
    for (i, n) in needles.iter().enumerate() {
        if n.at_least(MIN_NEEDLE) && !n.raw_view && key.contains(&n.key) {
            hits.insert(i);
        }
    }
    for n in hits {
        acc.survivor(Surface::Xmp, loc.clone(), Some(n));
    }
}
