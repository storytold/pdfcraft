//! Bounds of the redaction proof (`verify`). Hitting one is a failure the proof reports, never
//! a quiet skip: a proof that cannot sweep everything it must is not a passing proof.

/// Most decoded bytes of one stream the proof reads.
pub(crate) const MAX_STREAM: usize = 64 << 20;
/// Most decoded bytes the whole proof reads, over every stream and every pass.
pub(crate) const MAX_TOTAL_DECODED: u64 = 2 << 30;
/// Most byte comparisons the needle search may spend in one proof (about 4 GiB of haystack).
pub(crate) const MAX_WORK: u64 = 4 << 30;
/// Most of a haystack one needle's search may be charged to [`MAX_WORK`]. The haystack itself —
/// the region actually scanned — is charged in full once per scan, so a haystack past the budget
/// still fails closed; what is capped is the per-needle charge, so a bulk of needles over one
/// large document (many removed strings, which is ordinary) does not multiply the haystack by
/// the needle count and spend the whole budget many times over.
pub(crate) const MAX_NEEDLE_CHARGE: usize = 1 << 20;
/// Nested attachment levels that are swept (deeper ones are reported as unswept).
pub(crate) const MAX_NESTING: usize = 4;
/// Nesting bound of object walks (deeper is reported, never skipped silently).
pub(crate) const MAX_WALK: usize = 64;
/// Deepest graphics-state stack the paint check follows (saved states carry their clip holes
/// with them, so an unbounded stack is unbounded memory). Deeper is unverifiable.
pub(crate) const MAX_GS_STACK: usize = 4096;
/// Most clip holes one walk may cut (each is kept until the state that made it is restored).
/// More is unverifiable.
pub(crate) const MAX_HOLES: usize = 4096;
/// Most subpaths of one even-odd clip the hole rule examines (it compares them pairwise).
/// More, and no hole is cut: what the clip lets through is reported as paint.
pub(crate) const MAX_CLIP_SUBPATHS: usize = 64;
/// Most decoded bytes of an embedded CMap.
pub(crate) const MAX_CMAP: usize = 4 << 20;
/// Most CMap range steps one font may cost (a range that never grows the map still costs).
pub(crate) const MAX_CMAP_WORK: usize = 4 << 20;
/// Most entries one ToUnicode CMap may define.
pub(crate) const MAX_CMAP_ENTRIES: usize = 1 << 20;
/// Needles shorter than this (in characters) are too common to look for in raw bytes.
pub(crate) const MIN_NEEDLE: usize = 3;
/// Smallest needle looked for in the raw file and in decoded streams. Those hold object syntax,
/// numbers and binary data, where three characters match by chance (`/Length 612`, a
/// `/MediaBox`, pixels); shorter needles are still swept on page text, strings, names, Info, XMP
/// and attachment text.
pub(crate) const MIN_STRUCTURED_NEEDLE: usize = 6;
/// Deepest nesting of forms, patterns and appearance streams the proof's independent walks
/// follow: the bound the interpreter itself walks forms at, so the proof reaches everything a
/// redaction could have removed and the two cannot drift apart.
pub(crate) const MAX_DEPTH: usize = crate::interp::MAX_DEPTH;
/// Most pixels an image may have for the proof to inspect it (larger ones are removed by
/// [`apply`](crate::apply), and the proof's pixel look has the same ceiling: `crate::image`).
pub(crate) const MAX_PIXELS: u64 = crate::image::MAX_PIXELS;
