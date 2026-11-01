//! An independent proof that redaction removed what it claimed to remove.
//!
//! [`apply`](crate::apply) cuts glyphs out with its own interpreter, so asking that interpreter
//! whether anything is left would only repeat its own mistakes. The proof therefore works from
//! the other end, and does not trust the interpreter's font metrics or hit test:
//!
//! 1. **Before** applying, [`Snapshot::capture`] records, per redaction region, the glyph runs the
//!    interpreter is about to remove (their character codes and the text the font gives them), a
//!    digest of each page's content, and the text of the whole document as a separate extractor
//!    sees it. The strings the sweep looks for therefore *start* from what the interpreter
//!    removed; a glyph it wrongly kept is caught by the geometric check below, not by a needle.
//! 2. **After** applying, [`Snapshot::prove`] serializes the document and sweeps the *output*
//!    for those strings on every surface a reader could recover them from: the raw file, every
//!    string object, every decoded stream (orphans included), the text of every page (read by a
//!    minimal extractor of its own, not the interpreter's font metrics or hit test), the Info
//!    dictionary, the XMP packet, embedded files (PDF attachments are swept recursively) and the
//!    revision structure. Each string is looked for in every encoding a PDF can hold it in.
//! 3. It places every glyph still shown on the redacted pages with its own text-state tracking
//!    and width model (the font's `/Widths`, else a flat guess) and fails a region when the
//!    centre of a surviving glyph lies inside it, whatever the interpreter decided.
//! 4. It also re-checks the non-text content under each region — on the page and in the
//!    appearance streams of the annotations left on it: no image still carries pixels there, and
//!    no path or shading paints there unclipped.
//!
//! The proof fails closed: a survivor, but also anything it could not sweep (page content, a form
//! or an appearance stream, XMP, or an attachment that won't decode in full or can't be opened; a
//! nesting or work limit hit; an appearance with content that can't be placed on its page), makes
//! it fail. The one exception is binary data that redaction
//! never edits: a font program, colour profile, `/Indexed` lookup table, function sample table or
//! Flate/LZW image that is damaged or too long to decode is searched as stored bytes and reported
//! as `raw_only`, not as unswept. A proof that cannot fail is worthless.
//!
//! Work is bounded: every stream is decoded strictly under `limits::MAX_STREAM`, all decodes together
//! under `limits::MAX_TOTAL_DECODED`, and all searching under `limits::MAX_WORK`. A document that needs more
//! is reported as unswept, not skipped (binary data as above excepted).
//!
//! The result is a [`Proof`]: a per-region manifest (page, region, glyph counts, SHA-256 of
//! the page content after) and a verdict per surface. It holds counts and digests,
//! never the redacted text, unless [`ProofOptions::include_plaintext`] asks for it.
//!
//! Limits, stated plainly: text that exists only as pixels of an image is not text; a string
//! that also appears in content that was *not* redacted is counted, over the whole document, on
//! the page text, the raw file and the decoded streams (those hold the kept copy too) and fails
//! on a surplus; names are swept against the matches the input itself had, since a redacted word
//! that is also a `/Subtype` or a `/BaseFont` is structure, not a leftover copy, and a surplus of
//! matches still fails; every other surface is swept with no allowance. A word of six characters or more
//! that also spells a structural name (`/Filter`, `/MediaBox`) can still be reported by the raw-file and
//! decoded-stream surfaces, which search stored bytes and cannot tell a name from a copy: that is
//! over-reporting, never a pass. The raw file and the decoded
//! streams are searched for strings of at least six characters only (shorter ones match object
//! syntax and numbers by chance), skipping the trailer's file identifier and the
//! pixels of an image codec (DCT, JPX, JBIG2, CCITT), which are searched as stored bytes only; shorter strings are still found on the page text,
//! in strings, names, Info, XMP and attachment text. A region without extractable text is reported as such, and as
//! unverifiable when the page draws text this extractor cannot decode. The geometric check
//! places glyphs with the font's `/Widths` only: for a font without them the widths are a guess,
//! so a neighbouring glyph may be reported when the guess puts its centre inside a region.
//! `runs_removed` and `codes_removed` are exact counts of what was cut: a proof that is shared
//! discloses how much text sat under each region (never what it said).

use std::fmt::Write as _;

use pdfcraft_cos::ObjRef;

mod extract;
mod geometry;
pub(crate) mod limits;
mod snapshot;
mod sweep;
mod util;

pub use snapshot::Snapshot;
/// What the proof established for one region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// What was removed was swept for on every surface and is gone.
    Verified,
    /// The region held no extractable text: nothing was removed, and that is recorded as such.
    VerifiedNoTextInRegion,
    /// Neither "text was removed" nor "there was none" can be checked (the page draws text this
    /// extractor can't decode, or an image under the region can't be inspected). The proof can't
    /// pass while any region is unverifiable.
    Unverifiable,
    /// A survivor was found, or the mechanical evidence is missing.
    Failed,
}

/// The places a redacted string is looked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Surface {
    RawBytes,
    ObjectStrings,
    DecodedStreams,
    ExtractedText,
    Info,
    Xmp,
    EmbeddedFiles,
    Revisions,
}

const SURFACES: [Surface; 8] = [
    Surface::RawBytes,
    Surface::ObjectStrings,
    Surface::DecodedStreams,
    Surface::ExtractedText,
    Surface::Info,
    Surface::Xmp,
    Surface::EmbeddedFiles,
    Surface::Revisions,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Swept, nothing found.
    Clean,
    /// Swept, a redacted string (or an earlier revision) is still there.
    Survivor,
    /// Could not be swept; the proof fails.
    Unswept,
    /// There is nothing of this kind in the document.
    Absent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceReport {
    pub surface: Surface,
    pub verdict: Verdict,
    /// Streams, strings, pages or files looked at.
    pub scanned: usize,
    /// Streams scanned as stored bytes only: image-codec pixels (not text), and font, colour or
    /// image streams that could not be decoded in full.
    pub raw_only: usize,
    pub survivors: usize,
    /// Where each survivor sits ("object 12", "page 1", "attachment #1 > object 4"); never the text.
    pub locations: Vec<String>,
    /// Why the surface could not be swept.
    pub problems: Vec<String>,
}

/// The manifest entry of one redaction region.
#[derive(Clone, Debug, PartialEq)]
pub struct Excision {
    /// 0-based page.
    pub page: usize,
    /// The region in user space.
    pub region: [f64; 4],
    /// The Redact annotation it came from.
    pub mark: ObjRef,
    pub status: Status,
    pub detail: String,
    /// Glyph runs (consecutive glyphs of one string) removed under the region, and their
    /// character codes.
    pub runs_removed: usize,
    pub codes_removed: usize,
    /// SHA-256 of the page's decoded content streams, joined, after redaction. There is no digest
    /// of the content before: a proof is meant to be shared, and a hash of the original page
    /// would let a reader confirm a guess at what it said.
    pub page_sha256_after: Option<String>,
    /// Glyph-carrying text operators on the page (forms included) before and after. A `TJ` of
    /// numbers only (the gap left for a removed string) carries no glyphs and does not count.
    pub text_ops_before: Option<usize>,
    pub text_ops_after: Option<usize>,
    /// The removed text, only when [`ProofOptions::include_plaintext`] is set.
    pub removed_text: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Proof {
    pub entries: Vec<Excision>,
    pub surfaces: Vec<SurfaceReport>,
    /// Located, plain reasons for every failure (never the redacted text).
    pub failures: Vec<String>,
}

impl Proof {
    /// Survivors found across all surfaces.
    pub fn survivors(&self) -> usize {
        self.surfaces.iter().map(|s| s.survivors).sum()
    }

    /// Surfaces that could not be swept.
    pub fn unswept(&self) -> usize {
        self.surfaces.iter().filter(|s| s.verdict == Verdict::Unswept || !s.problems.is_empty()).count()
    }

    pub fn unverifiable(&self) -> usize {
        self.entries.iter().filter(|e| e.status == Status::Unverifiable).count()
    }

    /// Pass only when nothing failed, nothing is unverifiable and no surface went unswept.
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
            && self.survivors() == 0
            && self.unswept() == 0
            && self.entries.iter().all(|e| matches!(e.status, Status::Verified | Status::VerifiedNoTextInRegion))
    }

    /// A human-readable report: the verdict, the manifest and the surfaces. It names places and
    /// counts, and the removed text only if it was asked for.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "Redaction proof: {}", if self.passed() { "PASS" } else { "FAIL" });
        for f in &self.failures {
            let _ = writeln!(s, "  failure: {f}");
        }
        for (i, e) in self.entries.iter().enumerate() {
            let _ = writeln!(
                s,
                "  region {} page {} {:?} {:?}: {} run(s), {} code(s) removed; text ops {:?} -> {:?}; page sha256 after {}{}",
                i + 1,
                e.page + 1,
                e.region,
                e.status,
                e.runs_removed,
                e.codes_removed,
                e.text_ops_before,
                e.text_ops_after,
                e.page_sha256_after.as_deref().unwrap_or("-"),
                if e.detail.is_empty() { String::new() } else { format!(" ({})", e.detail) },
            );
        }
        for r in &self.surfaces {
            let _ = writeln!(
                s,
                "  {:?}: {:?} ({} scanned, {} raw only, {} survivor(s)){}",
                r.surface,
                r.verdict,
                r.scanned,
                r.raw_only,
                r.survivors,
                if r.locations.is_empty() { String::new() } else { format!(" at {}", r.locations.join(", ")) },
            );
            for p in &r.problems {
                let _ = writeln!(s, "    unswept: {p}");
            }
        }
        let _ = writeln!(s, "  Evidence, not a guarantee: text that exists only as pixels of an image is not text, and is not searched.");
        s
    }
}

/// What a proof may contain.
#[derive(Clone, Debug, Default)]
pub struct ProofOptions {
    /// Include the removed text in the manifest. Off by default: a proof is meant to be shared,
    /// and must not carry what it proves was removed.
    pub include_plaintext: bool,
}

/// Handles for the tests to reach the byte sweep without a whole document.
#[cfg(test)]
pub(crate) mod tests_support {
    use pdfcraft_content::parse;
    use pdfcraft_cos::{Dict, Object, Stream};

    use super::extract::{carries_glyphs, parse_cmap};
    use super::limits::{MAX_STREAM, MAX_TOTAL_DECODED, MAX_WORK, MIN_STRUCTURED_NEEDLE};
    use super::sweep::{Needle, add_needle, bytes_counts, bytes_hits, walk_strings};
    use super::util::Ctx;
    pub(crate) use super::util::survivor_encodings;

    /// The proof's whole search-work budget (for tests that pre-spend part of it).
    pub(crate) fn work_budget() -> u64 {
        MAX_WORK
    }

    /// The byte sweep of `hay` for a bulk of `count` distinct needles, on a context already
    /// `spent` units into its work budget: `Some` (with the needles that occur) while the sweep
    /// stays under the budget, `None` once it ran out.
    pub(crate) fn bulk_counts(hay: &[u8], text: &str, count: usize, spent: u64) -> Option<usize> {
        let mut needles: Vec<Needle> = Vec::new();
        for i in 0..count {
            add_needle(&mut needles, false, format!("{text}{i:04}"), Some(i));
        }
        let ctx = Ctx::new(Default::default());
        ctx.work.set(spent);
        bytes_counts(&needles, hay, MIN_STRUCTURED_NEEDLE, &ctx).map(|hits| hits.len())
    }

    /// The glyph-carrying text-showing operators of a decoded content stream.
    pub fn count_text_operators(decoded: &[u8]) -> usize {
        parse(decoded).ops.iter().filter(|op| carries_glyphs(op)).count()
    }

    /// Does the byte sweep for `text` find it in `hay`?
    pub fn found(text: &str, hay: &[u8]) -> bool {
        let mut needles: Vec<Needle> = Vec::new();
        add_needle(&mut needles, false, text.to_string(), Some(0));
        bytes_hits(&needles, hay, 3, &Ctx::new(Default::default())).is_some_and(|h| !h.is_empty())
    }

    /// Does this CMap parse completely (not cut short by a bound)?
    pub fn cmap_is_complete(data: &[u8]) -> bool {
        parse_cmap(data).is_some()
    }

    /// Is an object nested `depth` levels deep walked in full?
    pub fn walks_in_full(depth: usize) -> bool {
        let mut o = Object::Null;
        for _ in 0..depth {
            o = Object::Array(vec![o]);
        }
        walk_strings(&o, 0, &mut |_| {})
    }

    /// Does decoding fail once the cumulative decode budget is used up (and not before)?
    pub fn decode_budget_runs_out() -> (bool, bool) {
        let ctx = Ctx::new(Default::default());
        let s = Stream::from_raw(Dict::new(), vec![0u8; 100]);
        let before = ctx.decode(&s).is_ok();
        let spent = Ctx::with_decode_left(50);
        (before, spent.decode(&s).is_err())
    }

    /// Does a decode that fails past the one-stream cap pay for the attempt, so the same bomb
    /// cannot be decoded (and fail) again and again for free?
    /// `(refused, charged, then refused for want of allowance)`.
    pub fn failed_decode_charges_the_budget() -> (bool, bool, bool) {
        let big = vec![0u8; MAX_STREAM + (1 << 20)];
        let bomb = Stream::flate(Dict::new(), &big);
        drop(big);
        let small = Stream::from_raw(Dict::new(), vec![0u8; 100]);
        let ctx = Ctx::with_decode_left(u64::try_from(MAX_STREAM + 50).unwrap_or(u64::MAX));
        let refused = ctx.decode(&bomb).is_err();
        let charged = ctx.decoded.get() == MAX_TOTAL_DECODED - 50;
        let gone = ctx.decode(&small).is_err();
        (refused, charged, gone)
    }

    /// Does the byte search report `None` once the work budget is used up (and not before)?
    pub fn search_budget_runs_out() -> (bool, bool) {
        let mut needles: Vec<Needle> = Vec::new();
        add_needle(&mut needles, false, "secret".to_string(), Some(0));
        let ctx = Ctx::new(Default::default());
        let before = bytes_hits(&needles, b"some bytes", 3, &ctx).is_some();
        ctx.work.set(MAX_WORK);
        (before, bytes_hits(&needles, b"some bytes", 3, &ctx).is_none())
    }

    /// Is a survivor counted on an earlier page still reported when the work budget runs out on
    /// a later one? `(survivor reported, page left unswept)`.
    pub fn page_sweep_survivors_survive_the_limit() -> (bool, bool) {
        use super::Surface;
        use super::sweep::{Acc, sweep_pages};
        let objs = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 5 0 R /Resources << /Font << /F1 6 0 R >> >> >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 7 0 R /Resources << /Font << /F1 6 0 R >> >> >>".to_vec(),
            crate::tests_verify::stream("", b"BT /F1 10 Tf 10 100 Td (SECRETVALUE) Tj ET"),
            b"<< /Type /Font /Subtype /TrueType /BaseFont /Arial >>".to_vec(),
            crate::tests_verify::stream("", &format!("BT /F1 10 Tf 10 100 Td ({}) Tj ET", "x".repeat(700_000)).into_bytes()),
        ];
        let doc = pdfcraft_cos::Document::open(std::sync::Arc::new(crate::tests_verify::pdf_bytes(objs, ""))).unwrap();
        let mut needles = Vec::new();
        add_needle(&mut needles, false, "SECRETVALUE".to_string(), Some(0));
        let mut acc = Acc::new(Ctx::new(Default::default()));
        // Page 1 (the survivor's) fits in what is left; page 2 does not.
        acc.ctx.work.set(MAX_WORK - 100_000);
        sweep_pages(&needles, &doc, 0, "", &mut acc);
        let text = acc.reps.get(&Surface::ExtractedText).unwrap();
        (text.survivors > 0, !text.problems.is_empty())
    }

    /// Does a font's `/W` parsing answer to the shared work budget, and not only to its own
    /// counter (a form invoked again rebuilds its fonts, and would pay for the same array
    /// again)? `(read in full, refused on a spent budget)`.
    pub fn font_widths_charge_shares_the_budget() -> (bool, bool) {
        use super::extract::extract_page;
        let font = b"<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts [<< /Type /Font /Subtype /CIDFontType2 /BaseFont /X /DW 1000 /W [1 65535 250] >>] >>".to_vec();
        let objs = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F2 5 0 R >> >> >>".to_vec(),
            crate::tests_verify::stream("", b"BT /F2 10 Tf 10 100 Td <00410042> Tj ET"),
            font,
        ];
        let bytes = std::sync::Arc::new(crate::tests_verify::pdf_bytes(objs, ""));
        let probe = |spent: u64| -> bool {
            let doc = pdfcraft_cos::Document::open(bytes.clone()).unwrap();
            let ctx = Ctx::new(Default::default());
            ctx.work.set(spent);
            let page = &pdfcraft_model::pages(&doc)[0];
            extract_page(&doc, &page.dict, &ctx, &[]).is_ok()
        };
        (probe(0), !probe(MAX_WORK - 10_000))
    }
}
