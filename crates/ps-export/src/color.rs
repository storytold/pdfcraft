//! `/Separation` spot colours as prolog colorspace resources (Wave 1, Task 3).
//!
//! A PDF `/Separation` colour re-emits as one PostScript color space resource
//! `/PdfCraftCS<n>` — `[/Separation /<colorant> /Device<Space> <tint transform>]` —
//! defined once in the `%%BeginProlog` block and reused at every use site with
//! `<name> setcolorspace` + `<tint> setcolor`. Resources are deduplicated by the
//! SHA-256 of the emitted colorspace-array text (the Type 1 program dedup
//! precedent): the same colorant, alternate space and transform reuse one
//! resource across the whole document, however many pages parse it separately.
//!
//! Two emission details are measured against the Ghostscript oracle rather than
//! assumed, and both deviate from what a PDF-shaped translation would suggest:
//!
//! * The tint transform rides as an executable **procedure**, never as a
//!   function dictionary. gs's own Separation validation (`gs_sepr.ps`) demands
//!   `xcheck`-true element 3 — a dictionary is a typecheck in `setcolorspace`
//!   (measured on gs 10.08.0 for every function type), and gs's PDF machinery
//!   likewise compiles tint functions to procedures before building the space.
//!   [`tint_proc`] therefore writes the parsed function as a procedure:
//!   exponential and calculator functions translate directly (the calculator
//!   reuses `shading::calc_body`, the same restricted operator subset), sampled
//!   and stitched ones decline to the disclosed RGB approximation.
//! * The use site evaluates the resource name (`PdfCraftCS<n> setcolorspace`,
//!   no leading `/`): a literal name is an `/undefined` in `setcolorspace`
//!   (measured), because gs reads it as a space *name*, not a dictionary lookup.
//!
//! Everything the resource builder cannot rebuild — an unusable colorant name,
//! a non-device alternate space, a non-finite tint, an unconvertible transform —
//! declines to the colour's RGBA rendering view with one note per page per
//! reason, naming the colorant.

use std::collections::HashMap;

use hayro::hayro_interpret::color::{Color, DeviceSpace};
use hayro::hayro_interpret::{Function, ParsedFunction};
use sha2::{Digest, Sha256};

use crate::device::Emitter;

/// Per-document `/Separation` colorspace resources, deduplicated by the SHA-256
/// of the emitted colorspace-array text (the Type 1 program dedup precedent).
/// One table runs across the whole export: `export()` seeds each page's emitter
/// with the previous page's table, so names stay stable and repeats stay single.
#[derive(Default)]
pub(crate) struct Colorspaces {
    /// SHA-256 of the emitted colorspace-array text → the `/PdfCraftCS<n>` name
    /// it was first interned under.
    interned: HashMap<[u8; 32], String>,
    /// The interned resources in first-use order: resource name (without the
    /// `/`) and the colorspace-array text to define it by.
    order: Vec<(String, String)>,
    /// The next `/PdfCraftCS<n>` suffix.
    next: usize,
}

impl Colorspaces {
    /// Intern (or reuse) `/PdfCraftCS<n>` for the separation, then emit
    /// `PdfCraftCS<n> setcolorspace` + `<tint> setcolor`. Declines to the RGB
    /// approximation (via [`decline`], one note per page per reason) when the
    /// tint transform cannot ride as a procedure.
    pub(crate) fn setcolor(
        &mut self,
        emit: &mut Emitter,
        color: &Color,
        colorant: &str,
        tint: f32,
        alternate: DeviceSpace,
        tint_transform: &Function,
    ) {
        match self.resource(colorant, alternate, tint_transform) {
            Some(name) => {
                // The name is evaluated (no leading `/`): `setcolorspace` reads
                // a literal name as a space name, not a dictionary lookup.
                emit.op(&format!("{name} setcolorspace"));
                emit.coord(f64::from(tint));
                emit.op("setcolor");
            }
            None => decline(emit, color, colorant, "unconvertible tint function"),
        }
    }

    /// The interned resource name for the separation, interning it first if
    /// needed. `None` when the tint transform cannot ride as a procedure.
    fn resource(&mut self, colorant: &str, alternate: DeviceSpace, tint_transform: &Function) -> Option<String> {
        let text = colorspace_array(colorant, alternate, tint_transform)?;
        let hash: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        if let Some(name) = self.interned.get(&hash) {
            return Some(name.clone());
        }
        let name = format!("PdfCraftCS{}", self.next);
        self.next += 1;
        self.interned.insert(hash, name.clone());
        self.order.push((name.clone(), text));
        Some(name)
    }

    /// The prolog's colorspace resource definitions — one
    /// `%%BeginResource: colorspace …` block per interned space, in first-use
    /// order — or nothing when no separation was interned.
    pub(crate) fn prolog(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, array) in &self.order {
            out.extend_from_slice(format!("%%BeginResource: colorspace {name}\n").as_bytes());
            out.extend_from_slice(format!("/{name} {array} def\n").as_bytes());
            out.extend_from_slice(b"%%EndResource\n");
        }
        out
    }
}

/// The `paint_ops` `/Separation` arm: validate what the source handed over and
/// route into the emitter's [`Colorspaces`] table. Every decline reason names
/// the colorant and records one note per page.
pub(crate) fn separation_paint(
    emit: &mut Emitter,
    color: &Color,
    colorant: &str,
    tint: Option<f32>,
    alternate: Option<DeviceSpace>,
    tint_transform: &Function,
) {
    let Some(alternate) = alternate else {
        return decline(emit, color, colorant, "no device alternate space");
    };
    let Some(tint) = tint else {
        return decline(emit, color, colorant, "missing tint operand");
    };
    if !usable_name(colorant) {
        return decline(emit, color, colorant, "unusable colorant name");
    }
    if !tint.is_finite() {
        return decline(emit, color, colorant, "non-finite tint");
    }
    // The table lives inside the emitter, so lend it out for the call (the
    // `FontTable` take/reseed pattern): `setcolor` writes both the use-site
    // operators and, through this table, the prolog it will define them by.
    let mut table = std::mem::take(&mut emit.colorspaces);
    table.setcolor(emit, color, colorant, tint, alternate, tint_transform);
    emit.colorspaces = table;
}

/// Fall back to the colour's RGBA rendering view, disclosing why: one note per
/// page per reason (`note_once`), always naming the colorant.
fn decline(emit: &mut Emitter, color: &Color, colorant: &str, reason: &str) {
    emit.note_once(&format!("separation color approximated (RGB, {reason} for {colorant})"));
    crate::device::rgba_color_ops(emit, color);
}

/// The `[/Separation /<colorant> /Device<Space> <tint transform>]` array text.
/// `None` when the tint transform cannot ride as a procedure.
fn colorspace_array(colorant: &str, alternate: DeviceSpace, tint_transform: &Function) -> Option<String> {
    let proc = tint_proc(&tint_transform.parsed(), alternate).ok()?;
    Some(format!("[/Separation /{colorant} {} {proc}]", device_name(alternate)))
}

/// The PostScript device space name of `alternate`.
fn device_name(alternate: DeviceSpace) -> &'static str {
    match alternate {
        DeviceSpace::Gray => "/DeviceGray",
        DeviceSpace::Rgb => "/DeviceRGB",
        DeviceSpace::Cmyk => "/DeviceCMYK",
    }
}

/// The number of color components `alternate` takes.
fn components(alternate: DeviceSpace) -> usize {
    match alternate {
        DeviceSpace::Gray => 1,
        DeviceSpace::Rgb => 3,
        DeviceSpace::Cmyk => 4,
    }
}

/// The tint transform as an executable PostScript procedure: the one operand
/// form `setcolorspace` accepts (a dictionary typechecks there; see the module
/// docs). Exponential and calculator functions translate directly; sampled and
/// stitched ones cannot (Wave 1) and decline.
///
/// The procedure mirrors hayro's evaluation semantics exactly: the tint is
/// clamped to the domain, the body runs, the outputs are clamped to the range
/// (`Clamper` in the vendor).
fn tint_proc(f: &ParsedFunction, alternate: DeviceSpace) -> Result<String, &'static str> {
    match f {
        ParsedFunction::Exponential { domain, range, c0, c1, exponent } => {
            // hayro zips C0 with C1, so the shorter array decides the output
            // count; a count the alternate space cannot take is invalid PDF
            // and declines rather than emitting a space gs would reject.
            let count = c0.len().min(c1.len());
            if count == 0 || count != components(alternate) {
                return Err("tint transform output count does not match the alternate space");
            }
            let mut out = String::from("{");
            clamp_input(&mut out, domain.first())?;
            // s = t^N (PostScript `exp` is base-exponent), t dropped after.
            out.push_str(&format!(" dup {} exp exch pop", crate::shading::num(f64::from(*exponent))?));
            // Each component: s·(C1-C0)+C0, the scale `s` kept underneath for
            // the next component and dropped after the last (`exch`-paired).
            for i in 0..count {
                let c0 = f64::from(*c0.get(i).ok_or("missing C0 component")?);
                let c1 = f64::from(*c1.get(i).ok_or("missing C1 component")?);
                out.push_str(&format!(" dup {} mul {} add", crate::shading::num(c1 - c0)?, crate::shading::num(c0)?));
                if i + 1 < count {
                    out.push_str(" exch");
                }
            }
            out.push_str(" exch pop");
            clamp_output(&mut out, range.as_deref(), count)?;
            out.push('}');
            Ok(out)
        }
        ParsedFunction::Calculator { domain, range, program } => {
            // A calculator in a dictionary requires /Range (the `shading.rs`
            // rule); as a procedure the range is still what bounds the pushed
            // outputs, so the same requirement holds.
            let Some(range) = range.as_deref() else {
                return Err("calculator function lacks the required /Range");
            };
            if range.len() != components(alternate) {
                return Err("tint transform output count does not match the alternate space");
            }
            let mut out = String::from("{");
            clamp_input(&mut out, domain.first())?;
            // The calculator body is already legal PostScript procedure code
            // over PDF's restricted operator subset.
            crate::shading::calc_body(program, &mut out)?;
            clamp_output(&mut out, Some(range), range.len())?;
            out.push_str(" }");
            Ok(out)
        }
        // A sampled table has no procedure form short of an interpolation
        // machine, and a stitched transform is a conditional over sub-functions;
        // both stay Wave 1 declines.
        ParsedFunction::Sampled { .. } | ParsedFunction::Stitching { .. } => Err("sampled or stitched tint transform"),
    }
}

/// Clamp the one tint operand to the domain pair `d` (`dup lo lt {pop lo} if …`).
fn clamp_input(out: &mut String, d: Option<&(f32, f32)>) -> Result<(), &'static str> {
    if let Some(&(lo, hi)) = d {
        let lo = crate::shading::num(f64::from(lo))?;
        let hi = crate::shading::num(f64::from(hi))?;
        out.push_str(&format!(" dup {lo} lt {{pop {lo}}} if dup {hi} gt {{pop {hi}}} if"));
    }
    Ok(())
}

/// Clamp `count` stacked outputs to the range pairs (top first, rolled back
/// into order after each — `count` rolls of `count 1 roll` are the identity).
fn clamp_output(out: &mut String, range: Option<&[(f32, f32)]>, count: usize) -> Result<(), &'static str> {
    let Some(range) = range else { return Ok(()) };
    for i in (0..count).rev() {
        if let Some(&(lo, hi)) = range.get(i) {
            let lo = crate::shading::num(f64::from(lo))?;
            let hi = crate::shading::num(f64::from(hi))?;
            out.push_str(&format!(" dup {lo} lt {{pop {lo}}} if dup {hi} gt {{pop {hi}}} if"));
        }
        out.push_str(&format!(" {count} 1 roll"));
    }
    Ok(())
}

/// Whether `name` can ride into the emission as a PostScript literal name:
/// non-empty, short, and free of the delimiters and whitespace the tokenizer
/// would break on, and of `#` — inside a name it introduces a hex escape
/// (`/Ab#42` re-reads as `AbB`; a `#` not followed by two hex digits is a
/// scanner error on strict interpreters) — so a colorant like `Spot#1` cannot
/// ride verbatim (the `fonts.rs` `usable_ps_name` rules, mirrored here).
fn usable_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 127
        && name.bytes().all(|b| matches!(b, b'!'..=b'~') && !matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%' | b'#'))
}

#[cfg(test)]
mod tests {
    use hayro::hayro_interpret::Function;
    use hayro::hayro_interpret::color::{AlphaColor, Color, DeviceSpace};
    use hayro::hayro_interpret::hayro_syntax::object::{Dict, FromBytes, Object};

    use super::separation_paint;
    use crate::device::Emitter;
    use crate::{FidelityNote, PsFormat, export};

    /// A `<< … >> stream … endstream` object body, in the hand-written style of
    /// `crates/engine/src/tests.rs` (length computed, not counted by hand).
    fn stream_obj(dict: &str, body: &[u8]) -> Vec<u8> {
        let mut obj = format!("<< {dict} /Length {} >>\nstream\n", body.len()).into_bytes();
        obj.extend_from_slice(body);
        obj.extend_from_slice(b"\nendstream");
        obj
    }

    /// Assemble a PDF from object bodies (numbered from 1), computing the xref like
    /// `crates/engine/src/tests.rs::fixture` (lengths and offsets, never counted by hand).
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

    /// Export a one-page fixture through the real pipeline (`crate::export`,
    /// the same entry the oracle drives): the program text and the notes.
    fn exported(bytes: Vec<u8>) -> (String, Vec<FidelityNote>) {
        exported_pages(bytes, &[0])
    }

    /// The same, for a multi-page selection.
    fn exported_pages(bytes: Vec<u8>, pages: &[usize]) -> (String, Vec<FidelityNote>) {
        let out = export(std::sync::Arc::new(bytes), pages, PsFormat::Ps).expect("fixture exports");
        (String::from_utf8_lossy(&out.bytes).into_owned(), out.warnings)
    }

    /// A one-page PDF whose only draw fills a rectangle through the colorspace
    /// resource array `space` at `tint` (the tint-transform function rides as
    /// object 5, the exponential every fixture here shares).
    fn separation_pdf(space: &str, tint: &str, function: Vec<Vec<u8>>) -> Vec<u8> {
        let mut objs = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
            format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ColorSpace << /CS1 {space} >> >> >>").into_bytes(),
            stream_obj("", format!("/CS1 cs {tint} sc 20 20 160 60 re f").as_bytes()),
        ];
        objs.extend(function);
        assemble(objs)
    }

    /// A usable spot fill keeps its identity end to end: the colorspace is
    /// defined once in the prolog (with the colorant, the device alternate and
    /// the tint transform as a procedure) and the page body selects it by name
    /// and paints with the raw tint — no degradation anywhere.
    #[test]
    fn separation_paint_emits_resource_and_setcolor() {
        let (s, notes) = exported(separation_pdf(
            "[/Separation /Spot /DeviceCMYK 5 0 R]",
            "0.5",
            vec![b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 1] /C1 [0.8 0 0 1] /N 1 >>".to_vec()],
        ));
        // The prolog resource: the array with colorant, alternate space and a
        // procedure-form tint transform (the exponential's N and C1-C0 scale).
        assert!(s.contains("%%BeginResource: colorspace PdfCraftCS0\n"), "one colorspace resource: {s}");
        assert!(s.contains("/PdfCraftCS0 [/Separation /Spot /DeviceCMYK {"), "the colorspace array: {s}");
        assert!(s.contains("dup 1 exp"), "the exponential tint transform (N=1): {s}");
        assert!(s.contains("0.8 mul"), "the C1-C0 scale of the first component: {s}");
        assert!(s.contains("] def\n"), "the resource definition: {s}");
        assert!(s.contains("%%BeginProlog\n"), "the resources ride in the prolog: {s}");
        // The use site: the resource name (evaluated) plus the raw tint.
        assert!(s.contains("PdfCraftCS0 setcolorspace\n0.5 setcolor\n"), "setcolorspace + setcolor with the raw tint: {s}");
        assert!(s.matches("setcolorspace\n").count() == 1, "one use site: {s}");
        assert_eq!(s.matches("fill\n").count(), 1, "the fill still paints: {s}");
        assert_eq!(notes, Vec::<FidelityNote>::new(), "a usable separation degrades nothing: {notes:?}");
    }

    /// A calculator tint transform rides as a procedure too — the calculator
    /// body (`shading.rs`'s operator subset) between the domain clamp and the
    /// range clamps that bound its pushed outputs.
    #[test]
    fn calculator_tint_transform_rides_as_procedure() {
        let (s, notes) = exported(separation_pdf(
            "[/Separation /Spot /DeviceCMYK 5 0 R]",
            "0.5",
            vec![stream_obj("/FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1 0 1]", b"{ pop 1.5 0 -0.2 0 }")],
        ));
        assert!(s.contains("/PdfCraftCS0 [/Separation /Spot /DeviceCMYK {"), "the colorspace array: {s}");
        // The domain clamp, the calculator body verbatim, then the range clamps
        // rolling each output back into order.
        assert!(s.contains("dup 0 lt {pop 0} if dup 1 gt {pop 1} if"), "the domain clamp: {s}");
        assert!(s.contains("pop 1.5 0 -0.2 0"), "the calculator body: {s}");
        assert_eq!(s.matches("4 1 roll").count(), 4, "one roll per clamped output: {s}");
        assert!(s.contains("PdfCraftCS0 setcolorspace\n0.5 setcolor\n"), "the use site: {s}");
        assert_eq!(notes, Vec::<FidelityNote>::new(), "a usable calculator degrades nothing: {notes:?}");
    }

    /// Tint transforms that cannot ride as a procedure fall back to the
    /// disclosed RGB approximation: a calculator function without the required
    /// `/Range`, and a sampled function (Wave 1 declines every table). Both
    /// still paint, both note the colorant, neither defines a resource.
    #[test]
    fn unconvertible_tint_functions_fall_back() {
        let cases: [(&str, Vec<Vec<u8>>, &str); 2] = [
            (
                "calculator without /Range",
                vec![stream_obj("/FunctionType 4 /Domain [0 1]", b"{ pop 0.8 0 0 0 }")],
                "the calculator paints CMYK(0.8 0 0 0), a non-gray",
            ),
            (
                "sampled table",
                vec![stream_obj(
                    "/FunctionType 0 /Domain [0 1] /Range [0 1 0 1 0 1 0 1] /Size [4] /BitsPerSample 8",
                    &[0x99, 0x00, 0x00, 0x00, 0x99, 0x00, 0x00, 0x00, 0x99, 0x00, 0x00, 0x00, 0x99, 0x00, 0x00, 0x00],
                )],
                "the samples paint CMYK(0.6 0 0 0), a non-gray",
            ),
        ];
        for (name, function, why) in cases {
            let (s, notes) = exported(separation_pdf("[/Separation /Spot /DeviceCMYK 5 0 R]", "0.5", function));
            assert!(!s.contains("setcolorspace"), "{name}: no colorspace resource: {s}");
            assert!(s.contains("setrgbcolor"), "{name}: {why}: {s}");
            assert_eq!(s.matches("fill\n").count(), 1, "{name}: the fill still paints: {s}");
            assert_eq!(
                notes,
                vec![FidelityNote { page: Some(1), text: "separation color approximated (RGB, unconvertible tint function for Spot)".into() }],
                "{name}: the decline names the colorant"
            );
        }
    }

    /// The dedup ledger: two pages painting the same colorant through separate
    /// but identical colorspace arrays define exactly one resource and select
    /// it under the same name on both pages; a *different* colorant on page 2
    /// adds exactly one more, with the next name.
    #[test]
    fn separation_colorspace_deduplicates_across_pages() {
        let fn_cmyk = b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 1] /C1 [0.8 0 0 1] /N 1 >>".to_vec();
        let build = |page2_colorant: &str| {
            assemble(vec![
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 300] >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /ColorSpace << /CS1 [/Separation /Spot /DeviceCMYK 7 0 R] >> >> >>".to_vec(),
                format!(
                    "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /ColorSpace << /CS1 [/Separation /{page2_colorant} /DeviceCMYK 8 0 R] >> >> >>"
                )
                .into_bytes(),
                stream_obj("", b"/CS1 cs 0.5 sc 20 20 160 60 re f"),
                stream_obj("", b"/CS1 cs 0.9 sc 20 120 160 60 re f"),
                fn_cmyk.clone(),
                fn_cmyk.clone(),
            ])
        };

        // Same colorant, separately parsed (identical text): one resource, one name.
        let (s, notes) = exported_pages(build("Spot"), &[0, 1]);
        assert_eq!(s.matches("%%BeginResource: colorspace").count(), 1, "one resource for the repeated colorant: {s}");
        assert_eq!(s.matches("PdfCraftCS0 setcolorspace").count(), 2, "both pages select the same resource: {s}");
        assert_eq!(s.matches("0.9 setcolor").count(), 1, "each page paints its own tint: {s}");
        assert_eq!(notes, Vec::<FidelityNote>::new(), "dedup is not a degradation: {notes:?}");

        // A different colorant: exactly one more resource, the next name, page 2's.
        let (s, _) = exported_pages(build("Varnish"), &[0, 1]);
        assert_eq!(s.matches("%%BeginResource: colorspace").count(), 2, "one resource per distinct colorspace: {s}");
        assert!(s.contains("/PdfCraftCS0 [/Separation /Spot /DeviceCMYK"), "page 1's colorant: {s}");
        assert!(s.contains("/PdfCraftCS1 [/Separation /Varnish /DeviceCMYK"), "page 2's colorant: {s}");
        let page2 = s.find("%%Page: 2 2").expect("page 2 block");
        assert!(s[page2..].contains("PdfCraftCS1 setcolorspace"), "page 2 selects its own resource: {s}");
        assert_eq!(s.matches("PdfCraftCS0 setcolorspace").count(), 1, "page 1 keeps the first resource: {s}");
    }

    /// A non-finite tint has no operand to hand `setcolor`: the RGB view with a
    /// note naming the colorant (PDF literals cannot spell one, so this drives
    /// the arm directly).
    #[test]
    fn non_finite_tint_declines() {
        let function = Function::new(&Object::Dict(
            Dict::from_bytes(b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 1 1 0] /N 1 >>").expect("fixture function dict"),
        ))
        .expect("the fixture function parses");
        let mut emit = Emitter::default();
        let color = Color::from_rgba(AlphaColor::new([1.0, 0.0, 0.0, 1.0]));
        separation_paint(&mut emit, &color, "Spot", Some(f32::NAN), Some(DeviceSpace::Cmyk), &function);
        let out = String::from_utf8_lossy(&emit.out).into_owned();
        assert!(out.contains("setrgbcolor"), "the RGB approximation paints: {out}");
        assert!(
            emit.notes.iter().any(|n| n.text == "separation color approximated (RGB, non-finite tint for Spot)"),
            "the decline names the colorant: {:?}",
            emit.notes
        );
    }

    /// A colorant that cannot ride a PostScript name (`#28` unescapes to `(`)
    /// declines rather than emitting a name the tokenizer would break on.
    #[test]
    fn unusable_colorant_name_declines() {
        let (s, notes) = exported(separation_pdf(
            "[/Separation /Sp#28ot /DeviceCMYK 5 0 R]",
            "0.5",
            vec![b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 1 1 0] /N 1 >>".to_vec()],
        ));
        assert!(!s.contains("setcolorspace"), "no colorspace resource: {s}");
        assert!(s.contains("setrgbcolor"), "the RGB approximation paints: {s}");
        assert_eq!(
            notes.iter().map(|n| n.text.clone()).collect::<Vec<_>>(),
            vec!["separation color approximated (RGB, unusable colorant name for Sp(ot)".to_string()],
            "the decline names the colorant as parsed"
        );
    }

    /// A colorant whose *parsed* name carries `#` (`Spot#1` — spelled
    /// `/Spot#231`, whose `#23` unescapes to the `#` itself) declines too:
    /// inside a PostScript name `#` introduces a hex escape (`/Ab#42`
    /// re-reads as `AbB`), and a `#` not followed by two hex digits is a
    /// scanner error on strict interpreters — a colorant named `Spot#1` would
    /// emit a plate name that mangles or breaks the program.
    #[test]
    fn hash_in_colorant_name_declines() {
        let (s, notes) = exported(separation_pdf(
            "[/Separation /Spot#231 /DeviceCMYK 5 0 R]",
            "0.5",
            vec![b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 1 1 0] /N 1 >>".to_vec()],
        ));
        assert!(!s.contains("setcolorspace"), "no colorspace resource: {s}");
        assert!(s.contains("setrgbcolor"), "the RGB approximation paints: {s}");
        assert_eq!(
            notes.iter().map(|n| n.text.clone()).collect::<Vec<_>>(),
            vec!["separation color approximated (RGB, unusable colorant name for Spot#1)".to_string()],
            "the decline names the colorant as parsed"
        );
    }

    /// An alternate space that is not one of the three device spaces (here a
    /// `/CalRGB` dictionary) cannot be named in PostScript by this emitter: the
    /// RGB view, with a note.
    #[test]
    fn non_device_alternate_space_declines() {
        let (s, notes) = exported(separation_pdf(
            "[/Separation /Spot [/CalRGB 6 0 R] 5 0 R]",
            "0.5",
            vec![b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [0.8 0 0] /N 1 >>".to_vec(), b"<< /WhitePoint [0.9505 1 1.089] >>".to_vec()],
        ));
        assert!(!s.contains("setcolorspace"), "no colorspace resource: {s}");
        assert!(s.contains("setrgbcolor"), "the RGB approximation paints: {s}");
        assert_eq!(
            notes.iter().map(|n| n.text.clone()).collect::<Vec<_>>(),
            vec!["separation color approximated (RGB, no device alternate space for Spot)".to_string()],
            "the decline names the colorant and the reason"
        );
    }
}
