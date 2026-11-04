//! Tiling patterns as PostScript pattern resources (Wave 1, Task 4).
//!
//! A PDF tiling pattern (`/PatternType 1`) re-emits as one PostScript pattern
//! resource `/PdfCraftPat<n>` — a `/PatternType 1 /PaintType 1` dictionary
//! whose `/PaintProc` is the pattern cell's content stream, translated to
//! PostScript by driving the cell through this crate's own device — and every
//! draw painted with it becomes `PdfCraftPat<n> [matrix] makepattern
//! setpattern` before its path and paint operator. The cell is *collected*
//! once ([`TilingPattern::interpret`] drives the collector device with the
//! identity as the cell CTM, so every operator lands in pattern space, which
//! is exactly the space a `PaintProc` runs in), then interned into the
//! document's [`Patterns`] table.
//!
//! Two emission details are measured against the Ghostscript oracle rather
//! than assumed (gs 10.08.0):
//!
//! * The `makepattern` operands are **dictionary first, then the matrix**
//!   (`matrix dict makepattern` typechecks — the PLRM operand order), and the
//!   resource name is *evaluated* (`PdfCraftPat<n>`, no leading `/`): a
//!   literal name is a typecheck in `makepattern`, which reads the name as a
//!   name, not a dictionary lookup (the `setcolorspace` finding of Task 3).
//! * A PaintProc body of ordinary operators — `gsave`, a clip to the cell's
//!   `/BBox`, colour operators, path construction, `fill`, `grestore` —
//!   paints and steps correctly.
//!
//! Uncolored cells (`/PaintType 2`) are baked: the vendor's stencil device
//! substitutes the colour operand of the `scn` that selected the pattern, so
//! the collected PaintProc already paints exactly the ink the raster renderer
//! draws — and the resource is re-emitted as `/PaintType 1`. Dedup therefore
//! keys on the *emitted* bytes (SHA-256 of the whole resource text), never on
//! the pattern's cache key: the same uncolored stream under two colours is
//! two resources, and two streams that only differ in `/BBox` or step stay
//! two resources too.
//!
//! Three things still decline to the flat gray stand-in ([`tiling_approx`]),
//! each with its own note: a cell the interpreter refuses to decode (the
//! nesting/paint budget of a self-referencing pattern), a cell that really is
//! transparent (a mask, sub-unit opacity or non-Normal blend — PostScript has
//! no pattern-colour equivalent), and geometry whose operands would be
//! zero-area or step-less after repair, or a `makepattern` matrix with no
//! operand form (a singular draw transform). [`shading::pattern_approx`]
//! keeps the flat gray for pattern-painted *text* runs and stencil masks
//! (Wave 1 leaves those out; the resource table lives on the emitter, so the
//! text arms are unaffected).

use std::collections::HashMap;

use hayro::hayro_interpret::PathDrawMode;
use hayro::hayro_interpret::pattern::TilingPattern;
use kurbo::Affine;
use sha2::{Digest, Sha256};

use crate::device::{Emitter, PsDevice};
use crate::fonts::FontTable;
use crate::shading;

/// Per-document tiling-pattern resources, deduplicated by the SHA-256 of the
/// emitted resource text (the PaintProc bytes plus the dict geometry — an
/// uncolored cell bakes its substituted colour into the PaintProc, so the same
/// stream under two colours is two resources, matching what the raster
/// renderer paints per use). One table runs across the whole export:
/// `export()` seeds each page's emitter with the previous page's table, so
/// names stay stable and repeats stay single.
#[derive(Default)]
pub(crate) struct Patterns {
    /// SHA-256 of the emitted resource text → the `/PdfCraftPat<n>` name it
    /// was first interned under.
    interned: HashMap<[u8; 32], String>,
    /// The interned resources in first-use order: resource name (without the
    /// `/`) and the pattern-dictionary text to define it by.
    order: Vec<(String, String)>,
    /// The next `/PdfCraftPat<n>` suffix.
    next: usize,
}

impl Patterns {
    /// The interned resource name for `body` (the pattern-dictionary text),
    /// interning it first if needed.
    fn intern(&mut self, body: &str) -> String {
        let hash: [u8; 32] = Sha256::digest(body.as_bytes()).into();
        if let Some(name) = self.interned.get(&hash) {
            return name.clone();
        }
        let name = format!("PdfCraftPat{}", self.next);
        self.next += 1;
        self.interned.insert(hash, name.clone());
        self.order.push((name.clone(), body.to_string()));
        name
    }

    /// The prolog's pattern resource definitions — one
    /// `%%BeginResource: pattern …` block per interned pattern, in first-use
    /// order — or nothing when no pattern was interned.
    pub(crate) fn prolog(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, body) in &self.order {
            out.extend_from_slice(format!("%%BeginResource: pattern {name}\n").as_bytes());
            out.extend_from_slice(format!("/{name} {body} def\n").as_bytes());
            out.extend_from_slice(b"%%EndResource\n");
        }
        out
    }
}

/// One number operand for the pattern dictionary or the `makepattern` matrix.
/// `None` for the values with no operand a conforming interpreter accepts:
/// magnitudes beyond the 32-bit real range (a limitcheck at token scan — the
/// [`shading::num`] rule). Non-finite values fall through to [`Emitter::number`],
/// which repairs them to `0` and records the repair; the caller's token-level
/// checks then decide whether the repaired geometry is still a pattern.
fn operand(emit: &mut Emitter, v: f64) -> Option<String> {
    if v.abs() > f64::from(f32::MAX) {
        return None;
    }
    Some(emit.number(v))
}

/// The decline path: the flat gray stand-in ([`shading::tiling_approx`], which
/// records its own note), the caller's path operators and the paint operator —
/// inside the draw's existing `gsave`, so the fallback still paints the shape —
/// plus the specific `reason`, once per page.
fn decline<F: FnOnce(&mut Emitter)>(emit: &mut Emitter, draw_mode: &PathDrawMode, build: F, reason: &str) {
    emit.note_once(reason);
    shading::tiling_approx(emit);
    build(emit);
    crate::device::paint_op(emit, draw_mode);
}

/// Emit one tiling-painted draw. `build` emits the path ops; called inside the
/// draw's gsave/concat, so user space is established. The `makepattern` matrix
/// operand is `transform.inverse() * t.matrix` (pattern space → the concated
/// user space): frozen at `makepattern` time against that CTM, it lays the
/// tiles out exactly where the pattern matrix maps them — and composes
/// correctly through nesting, because the vendor hands a nested pattern a
/// matrix relative to *its* cell's space (the cell context's root transform).
/// PaintType is always emitted as 1 (colored): an uncolored cell's substituted
/// colour is baked into the PaintProc by the vendor's stencil device
/// ([`TilingPattern::is_color`] governs that substitution, not the emission),
/// which is exactly the ink the raster renderer draws.
pub(crate) fn tiling_paint(
    emit: &mut Emitter,
    fonts: &mut FontTable,
    t: &TilingPattern<'_>,
    transform: Affine,
    is_stroke: bool,
    draw_mode: &PathDrawMode,
    build: impl FnOnce(&mut Emitter),
) {
    // Collect the cell: a fresh device over its own buffer, seeded with the
    // document's font and resource tables (the take/reseed pattern `export()`
    // uses for fonts), so a cell that names /Separation colours or paints
    // further tiling patterns defines its resources in the same prolog. The
    // cell's own tiling fills recurse through this function on the collector's
    // tables, which makes nesting reentrant by construction.
    let page = emit.page;
    let mut cell = PsDevice::cell(page, std::mem::take(fonts));
    cell.emit.colorspaces = std::mem::take(&mut emit.colorspaces);
    cell.emit.patterns = std::mem::take(&mut emit.patterns);
    let decoded = t.interpret(&mut cell, Affine::IDENTITY, is_stroke);
    let transparent = cell.saw_transparency;
    cell.finish();
    let cell_ops = std::mem::take(&mut cell.emit.out);
    let cell_notes = std::mem::take(&mut cell.emit.notes);
    *fonts = std::mem::take(&mut cell.fonts);
    emit.colorspaces = std::mem::take(&mut cell.emit.colorspaces);
    emit.patterns = std::mem::take(&mut cell.emit.patterns);
    for note in &cell_notes {
        emit.note_once(&note.text);
    }

    if decoded.is_none() {
        return decline(emit, draw_mode, build, "tiling pattern approximated (cell undecodable)");
    }
    if transparent {
        return decline(emit, draw_mode, build, "tiling pattern approximated (transparent cell)");
    }

    // The dict geometry, normalized and repaired: the /BBox parses through
    // f32, so an overflowing entry arrives non-finite and `Emitter::number`
    // repairs it to 0 (noted) — and repaired geometry must still describe a
    // cell. The gate reads the formatted tokens, not the raw values (the
    // `setdash` lesson): a BBox edge or step whose token collapses to "0"
    // leaves nothing to tile, and a magnitude beyond the 32-bit real range has
    // no operand at all.
    let b = t.bbox;
    let values = [b.x0.min(b.x1), b.y0.min(b.y1), b.x0.max(b.x1), b.y0.max(b.y1), f64::from(t.x_step), f64::from(t.y_step)];
    let mut tokens: Vec<String> = Vec::with_capacity(values.len());
    for &v in &values {
        if let Some(token) = operand(emit, v) {
            tokens.push(token);
        }
    }
    // Fewer than six tokens means `operand` refused a magnitude beyond the
    // PostScript real range; the same decline covers the collapsed tokens.
    let [x0, y0, x1, y1, x_step, y_step] = tokens.as_slice() else {
        return decline(emit, draw_mode, build, "tiling pattern approximated (unusable geometry)");
    };
    // The token-level degeneracy gates: a collapsed BBox edge or step tiles nothing.
    if x0 == x1 || y0 == y1 || x_step.as_str() == "0" || y_step.as_str() == "0" {
        return decline(emit, draw_mode, build, "tiling pattern approximated (unusable geometry)");
    }

    // The makepattern matrix: pattern space → the concated user space. A
    // singular draw transform inverts to non-finite coefficients, and a
    // hostile pattern matrix can carry its own non-finite or out-of-range
    // ones — either way there is no operand form, and a repaired matrix would
    // silently misplace the tiles, so the draw declines instead.
    let matrix = transform.inverse() * t.matrix;
    if matrix.as_coeffs().iter().any(|c| !c.is_finite() || c.abs() > f64::from(f32::MAX)) {
        return decline(emit, draw_mode, build, "tiling pattern approximated (unusable makepattern matrix)");
    }

    // The resource: dict first, the collected cell as the PaintProc body. The
    // procedure starts with `pop`: PLRM hands the PaintProc the pattern dictionary
    // as an operand, and the procedure must consume it — Ghostscript cleans up after
    // a leftover, but a strict RIP does not have to, and nested patterns would stack
    // one dictionary per tile.
    let body = format!(
        "<< /PatternType 1 /PaintType 1 /TilingType 1 /BBox [{x0} {y0} {x1} {y1}] /XStep {x_step} /YStep {y_step} /PaintProc {{ pop\n{} }} >>",
        String::from_utf8_lossy(&cell_ops),
    );
    // The table lives inside the emitter, so lend it out for the call (the
    // `Colorspaces` take/reseed pattern).
    let mut table = std::mem::take(&mut emit.patterns);
    let name = table.intern(&body);
    emit.patterns = table;

    // The use site: the name evaluated (a literal typechecks in makepattern,
    // measured on gs 10.08.0), the dictionary first and the matrix second
    // (the PLRM operand order; the reverse typechecks too).
    let mut line = format!("{name} [");
    for c in matrix.as_coeffs() {
        line.push_str(&emit.number(c));
        line.push(' ');
    }
    line.pop();
    line.push_str("] makepattern setpattern");
    emit.op(&line);
    build(emit);
    crate::device::paint_op(emit, draw_mode);
}

#[cfg(test)]
mod tests {
    use crate::{FidelityNote, PsFormat, export};

    // ---------------------------------------------------------------- fixtures

    /// A `<< … >> stream … endstream` object body (the repo fixture style; the
    /// length is computed, never counted by hand).
    fn stream_obj(dict: &str, body: &[u8]) -> Vec<u8> {
        let mut obj = format!("<< {dict} /Length {} >>\nstream\n", body.len()).into_bytes();
        obj.extend_from_slice(body);
        obj.extend_from_slice(b"\nendstream");
        obj
    }

    /// Assemble a PDF from object bodies (numbered from 1), computing the xref
    /// like `crates/engine/src/tests.rs::fixture` (lengths and offsets, never
    /// counted by hand).
    fn assemble(objs: Vec<Vec<u8>>) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// A PDF whose objects 1 (catalog) and 2 (the pages tree over every page)
    /// are assembled here, `objects` ride verbatim from object 3, and one page
    /// dict (plus its content stream) follows per `(resources, content)` entry.
    /// Pages are 200×300; the page dicts and their contents are numbered after
    /// `objects` (pages first, then contents).
    fn pdf(objects: Vec<Vec<u8>>, pages: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
        let first_page = 3 + objects.len();
        let first_content = first_page + pages.len();
        let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", first_page + i)).collect();
        let mut objs = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            format!("<< /Type /Pages /Kids [{}] /Count {} /MediaBox [0 0 200 300] >>", kids.join(" "), pages.len()).into_bytes(),
        ];
        objs.extend(objects);
        for (i, (resources, _)) in pages.iter().enumerate() {
            objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << {resources} >> >>", first_content + i).into_bytes());
        }
        for (_, content) in &pages {
            objs.push(stream_obj("", content));
        }
        assemble(objs)
    }

    /// A coloured tiling pattern: one blue 8×8 square inside a 10×10 cell that
    /// steps every 20 units.
    fn colored_pattern() -> Vec<u8> {
        stream_obj("/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 20 /YStep 20 /Resources << >>", b"0 0 1 rg 1 1 8 8 re f")
    }

    /// One page filling the 20 20 160 100 rectangle with pattern object 3.
    fn tiling_page(pattern: Vec<u8>) -> Vec<u8> {
        pdf(vec![pattern], vec![("/Pattern << /P1 3 0 R >>", b"/Pattern cs /P1 scn 20 20 160 100 re f".to_vec())])
    }

    /// Export pages of `bytes` through the real pipeline (`crate::export`, the
    /// same entry the oracle drives): the program text and the fidelity notes.
    fn exported_pages(bytes: Vec<u8>, pages: &[usize]) -> (String, Vec<FidelityNote>) {
        let out = export(std::sync::Arc::new(bytes), pages, PsFormat::Ps).expect("fixture exports");
        (String::from_utf8_lossy(&out.bytes).into_owned(), out.warnings)
    }

    /// The same, for page 0 alone.
    fn exported(bytes: Vec<u8>) -> (String, Vec<FidelityNote>) {
        exported_pages(bytes, &[0])
    }

    // ------------------------------------------------------------------- tests

    /// A tiling fill becomes a prolog pattern resource plus the `makepattern`/
    /// `setpattern` use site: the resource carries the PatternType 1 dictionary
    /// with the cell's operators as the `/PaintProc` body (the interpreter's
    /// /BBox clip around the blue square, both gsave/grestore-balanced), and
    /// the page's own draw keeps its gsave, concat, path and fill around the
    /// `makepattern` line — with the resource name *evaluated* and the
    /// dictionary first on the stack (gs reads `/Name makepattern` as a
    /// typecheck and `matrix dict makepattern` as one too: measured on
    /// gs 10.08.0). Nothing degrades.
    #[test]
    fn tiling_fill_emits_makepattern_and_paintproc() {
        let (s, notes) = exported(tiling_page(colored_pattern()));

        // The prolog resource: the dict head, the collected cell, the def.
        assert!(s.contains("%%BeginResource: pattern PdfCraftPat0\n"), "one pattern resource: {s}");
        assert!(
            s.contains("/PdfCraftPat0 << /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 20 /YStep 20 /PaintProc { pop\n"),
            "the dict head opens a PaintProc that first pops its dictionary operand: {s}"
        );
        assert!(s.contains("0 0 1 setrgbcolor\n"), "the cell's own colour operator: {s}");
        assert!(
            s.contains("closepath\nclip\ngsave\n[1 0 0 1 0 0] concat\n0 0 1 setrgbcolor\nnewpath\n1 1 moveto\n9 1 lineto\n9 9 lineto\n1 9 lineto\nclosepath\nfill\ngrestore\n"),
            "the interpreter's /BBox clip around the blue square: {s}"
        );
        assert!(s.contains(" } >> def\n%%EndResource\n"), "the dict closes and defines: {s}");

        // The use site: the evaluated name, the matrix operand, the path, the fill.
        assert!(
            s.contains("[1 0 0 1 0 0] concat\nPdfCraftPat0 [1 0 0 1 0 0] makepattern setpattern\nnewpath\n20 20 moveto\n180 20 lineto\n180 120 lineto\n20 120 lineto\nclosepath\nfill\ngrestore\n"),
            "the draw keeps its gsave/concat/path/fill around the makepattern line: {s}"
        );
        assert_eq!(s.matches("makepattern").count(), 1, "one makepattern per use site: {s}");
        assert!(!s.contains("0.5 setgray"), "no flat stand-in: {s}");
        assert_eq!(notes, Vec::<FidelityNote>::new(), "a tiling fill degrades nothing: {notes:?}");
    }

    /// The dedup ledger: two pages painting separately parsed but identical
    /// coloured patterns define exactly one resource and both use its name;
    /// an *uncolored* (`/PaintType 2`) cell painted through two different
    /// colours defines two resources — both re-emitted as `/PaintType 1`,
    /// each with its substituted colour baked into the PaintProc — because
    /// dedup keys on the emitted bytes, not the pattern's cache key.
    #[test]
    fn tiling_pattern_deduplicates_across_pages() {
        // The same coloured pattern, parsed once per page: one resource.
        let (s, notes) = exported_pages(
            pdf(
                vec![colored_pattern(), colored_pattern()],
                vec![
                    ("/Pattern << /P1 3 0 R >>", b"/Pattern cs /P1 scn 20 20 160 40 re f".to_vec()),
                    ("/Pattern << /P1 4 0 R >>", b"/Pattern cs /P1 scn 20 120 160 40 re f".to_vec()),
                ],
            ),
            &[0, 1],
        );
        assert_eq!(s.matches("%%BeginResource: pattern").count(), 1, "one resource for the repeated pattern: {s}");
        assert_eq!(s.matches("PdfCraftPat0 [1 0 0 1 0 0] makepattern setpattern").count(), 2, "both pages use the same name: {s}");
        assert!(!s.contains("PdfCraftPat1"), "no second name: {s}");
        assert_eq!(notes, Vec::<FidelityNote>::new(), "dedup is not a degradation: {notes:?}");

        // The same uncolored cell under red and green: two resources, both
        // coloured, each baked.
        let uncolored =
            || stream_obj("/PatternType 1 /PaintType 2 /TilingType 1 /BBox [0 0 10 10] /XStep 20 /YStep 20 /Resources << >>", b"1 1 8 8 re f");
        let (s, notes) = exported_pages(
            pdf(
                vec![uncolored(), uncolored()],
                vec![
                    ("/Pattern << /P1 3 0 R >> /ColorSpace << /CS1 [/Pattern /DeviceRGB] >>", b"/CS1 cs 1 0 0 /P1 scn 20 20 160 40 re f".to_vec()),
                    ("/Pattern << /P1 4 0 R >> /ColorSpace << /CS1 [/Pattern /DeviceRGB] >>", b"/CS1 cs 0 1 0 /P1 scn 20 120 160 40 re f".to_vec()),
                ],
            ),
            &[0, 1],
        );
        assert_eq!(s.matches("%%BeginResource: pattern").count(), 2, "one resource per baked colour: {s}");
        assert_eq!(s.matches("/PaintType 1").count(), 2, "both re-emit as coloured patterns: {s}");
        assert!(s.contains("1 0 0 setrgbcolor\n"), "page 1's substituted red is baked: {s}");
        assert!(s.contains("0 1 0 setrgbcolor\n"), "page 2's substituted green is baked: {s}");
        assert!(s.contains("PdfCraftPat0 [1 0 0 1 0 0] makepattern setpattern\n"), "page 1's use site: {s}");
        assert!(s.contains("PdfCraftPat1 [1 0 0 1 0 0] makepattern setpattern\n"), "page 2's use site: {s}");
        assert_eq!(notes, Vec::<FidelityNote>::new(), "baking is what the raster renderer paints, not a degradation: {notes:?}");
    }

    /// A cell that carries real transparency (an ExtGState `/BM /Multiply`
    /// inside the pattern's own resources) cannot ride a PostScript pattern
    /// colour: the fill degrades to the flat gray stand-in with the
    /// transparent-cell note, and the program stays gsave/grestore-balanced.
    #[test]
    fn transparent_cell_degrades_to_gray() {
        let pattern = stream_obj(
            "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 20 /YStep 20 /Resources << /ExtGState << /GS1 4 0 R >> >>",
            b"q /GS1 gs 0 0 1 rg 1 1 8 8 re f Q",
        );
        let ext_g_state = b"<< /Type /ExtGState /BM /Multiply >>".to_vec();
        let page = ("/Pattern << /P1 3 0 R >>", b"/Pattern cs /P1 scn 20 20 160 100 re f".to_vec());
        let (s, notes) = exported(pdf(vec![pattern, ext_g_state], vec![page]));
        assert!(!s.contains("makepattern"), "no pattern instance: {s}");
        assert!(!s.contains("%%BeginResource: pattern"), "no pattern resource: {s}");
        assert!(s.contains("0.5 setgray\n"), "the flat gray stand-in: {s}");
        assert!(s.contains("\nfill\n"), "the rectangle still paints: {s}");
        assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "the program stays balanced: {s}");
        assert!(
            notes.iter().filter(|n| n.text == "tiling pattern approximated (transparent cell)").count() == 1,
            "the transparent-cell note, exactly once: {notes:?}"
        );
    }

    /// Hostile geometry degrades instead of breaking the program: a huge but
    /// finite `/XStep` keeps its literal operand; a `/BBox` entry that
    /// overflows the f32 parse (non-finite) declines with the geometry note
    /// after repair; and a pattern that paints itself completes on the
    /// interpreter's nesting bound (the deepest decline noted once per page).
    /// Every export stays gsave/grestore-balanced.
    #[test]
    fn hostile_tiling_geometry_degrades() {
        // (a) A finite, huge step: the literal operand, no decline.
        let huge_step = stream_obj(
            "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 1000000 /YStep 20 /Resources << >>",
            b"0 0 1 rg 1 1 8 8 re f",
        );
        let (s, notes) = exported(tiling_page(huge_step));
        assert!(s.contains("/XStep 1000000 /YStep 20"), "the huge step as a literal operand: {s}");
        assert!(s.contains("makepattern"), "the pattern still emits: {s}");
        assert_eq!(notes, Vec::<FidelityNote>::new(), "nothing to repair: {notes:?}");

        // (b) A /BBox entry beyond the f32 range: no operand form exists, the
        // repair leaves a zero-area cell, and the draw declines.
        let nonfinite_bbox = stream_obj(
            "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10000000000000000000000000000000000000000 10] /XStep 20 /YStep 20 /Resources << >>",
            b"0 0 1 rg 1 1 8 8 re f",
        );
        let (s, notes) = exported(tiling_page(nonfinite_bbox));
        assert!(!s.contains("makepattern"), "no pattern instance: {s}");
        assert!(s.contains("0.5 setgray\n"), "the flat gray stand-in: {s}");
        let lowered = s.to_lowercase();
        assert!(!lowered.contains("nan") && !lowered.contains("inf"), "no non-finite operand: {s}");
        assert!(notes.iter().any(|n| n.text == "tiling pattern approximated (unusable geometry)"), "the geometry decline is disclosed: {notes:?}");
        assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "balanced: {s}");

        // (c) A pattern whose resources name itself: the interpreter's nesting
        // bound stops the recursion; the outer pattern still emits.
        let self_ref = stream_obj(
            "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 20 /YStep 20 /Resources << /Pattern << /P1 3 0 R >> >>",
            b"/Pattern cs /P1 scn 1 1 8 8 re f",
        );
        let (s, notes) = exported(tiling_page(self_ref));
        assert!(s.contains("makepattern"), "the outer pattern still emits: {s}");
        assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "balanced: {s}");
        assert!(
            notes.iter().filter(|n| n.text == "tiling pattern approximated (cell undecodable)").count() == 1,
            "the nesting bound's decline, noted once per page: {notes:?}"
        );
    }
}
