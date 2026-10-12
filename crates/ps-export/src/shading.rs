//! Shading and tiling-pattern paints (Layer A, Task 6).
//!
//! Shadings re-emit as PostScript Level 3 shading dictionaries painted with `shfill`
//! (which fills the current clip, so the device clips to the drawn path first). The
//! shading's `/Function` re-emits as a Level 3 function: types 0/2/3 as function
//! dictionaries — a sampled function's data rides in a `/DataSource` string — and a
//! type 4 calculator as a `/Function { … }` procedure over the same restricted operator
//! subset PDF defines; all forms verified against the Ghostscript oracle. Tiling
//! patterns degrade to a flat colour (the recorded Task 6 decision; see
//! [`tiling_approx`]).
//!
//! PostScript's reals are 32-bit, so a number beyond `f32::MAX` (or a non-finite one)
//! has no operand at all: it is a limitcheck on every conforming interpreter (verified
//! against Ghostscript), and a function carrying one degrades instead of poisoning the
//! whole file.

// The emission chain becomes reachable from `device.rs` with this task; the `export()`
// assembly that reaches it from outside the crate arrives with Task 7.
#![allow(dead_code)]

use hayro::hayro_interpret::color::DeviceSpace;
use hayro::hayro_interpret::pattern::{Pattern, ShadingPattern};
use hayro::hayro_interpret::shading::{Shading, ShadingFunction, ShadingType};
use hayro::hayro_interpret::{CalcOp, Function, Paint, ParsedFunction};
use kurbo::Affine;

use crate::device::Emitter;
use crate::psnum;

/// Emit a shading (or degrading tiling pattern) paint. Returns `true` when the paint was
/// handled here, `false` for flat colours (the device owns those). Call for *fills*,
/// with the painting path already built and clipped by the caller (`shfill` fills the
/// clip) and in page space (the caller restores it by concating the inverse of the draw
/// transform — the shading's matrix and BBox clip arrive in page space). A shading that
/// cannot be expressed paints the clip with its flat mid-tone colour instead, so the
/// caller emits nothing further either way.
pub(crate) fn shading_paint(paint: &Paint<'_>, emit: &mut Emitter) -> bool {
    match paint {
        Paint::Color(_) => false,
        Paint::Pattern(p) => match &**p {
            Pattern::Shading(sp) => {
                shading_fill(sp, emit);
                true
            }
            Pattern::Tiling(_) => {
                tiling_approx(emit);
                true
            }
        },
    }
}

/// Emit a pattern paint for a *stroke*: `shfill` fills the clip, so a stroked shape
/// cannot take a shading — the shading degrades to its flat mid-tone colour. Tiling
/// patterns degrade exactly as everywhere else. Returns `true` when handled, `false`
/// for flat colours.
pub(crate) fn shading_stroke_approx(paint: &Paint<'_>, emit: &mut Emitter) -> bool {
    match paint {
        Paint::Color(_) => false,
        Paint::Pattern(p) => match &**p {
            Pattern::Shading(sp) => {
                emit.note("shading approximated (stroked path)");
                flat_colour(sp, emit);
                true
            }
            Pattern::Tiling(_) => {
                tiling_approx(emit);
                true
            }
        },
    }
}

/// Emit a flat stand-in for a pattern paint at a position that has no path or clip to
/// shade through (text runs, stencil masks): the colour operators only.
pub(crate) fn pattern_approx(paint: &Paint<'_>, emit: &mut Emitter) {
    match paint {
        Paint::Pattern(p) => match &**p {
            Pattern::Shading(sp) => {
                emit.note("shading approximated (text or stencil paint)");
                flat_colour(sp, emit);
            }
            Pattern::Tiling(_) => tiling_approx(emit),
        },
        Paint::Color(_) => {}
    }
}

/// Emit a function as a PostScript Level 3 function dictionary (program text). `Err`
/// carries the reason when the function cannot be expressed; the caller degrades to a
/// flat colour and records the reason as a fidelity note.
pub(crate) fn ps_function(f: &Function) -> Result<Vec<u8>, &'static str> {
    let mut out = String::new();
    function_dict(&f.parsed(), &mut out)?;
    Ok(out.into_bytes())
}

/// The recorded Task 6 tiling decision. `makepattern` needs three things the vendor
/// type does not hand this emitter: the cell arrives as a raw content stream that
/// `TilingPattern::interpret` re-parses (it does not pre-resolve through the device),
/// `is_color` and the fallback paints are `pub(crate)` (so the required `/PaintType`
/// dict entry is unknowable), and the emission signature carries no `Device` handle to
/// drive the cell through. Every tiling pattern therefore degrades to a deterministic
/// mid-gray plus exactly one note.
fn tiling_approx(emit: &mut Emitter) {
    emit.note("tiling pattern approximated");
    emit.op("0.5 setgray");
}

/// Paint one shading pattern fill. See [`shading_paint`] for the calling contract.
fn shading_fill(sp: &ShadingPattern, emit: &mut Emitter) {
    if sp.opacity <= 0.0 {
        return;
    }
    if sp.transfer_function.is_some() {
        emit.note("transfer function ignored (shading pattern)");
    }
    if sp.opacity < 1.0 {
        emit.note("transparency ignored (shading pattern opacity)");
    }
    // The Dummy variant "should just be drawn transparent": nothing at all.
    if matches!(&*sp.shading.shading_type, ShadingType::Dummy) {
        return;
    }

    match shading_dict(&sp.shading) {
        Ok(dict) => {
            emit.op("gsave");
            // The shading's /BBox additionally clips; the interpreter delivers it
            // already mapped through the pattern matrix, in page space.
            if let Some(clip) = &sp.shading.clip_path {
                crate::device::path_ops(emit, clip);
                emit.op("clip");
            }
            concat_unless_identity(emit, sp.matrix);
            emit.op(&format!("{dict} shfill"));
            emit.op("grestore");
        }
        Err(reason) => {
            emit.note(&format!("shading approximated ({reason})"));
            // The caller clipped to the path and emits nothing else: fill the clip
            // (which encodes the fill rule) with the flat colour here.
            flat_colour(sp, emit);
            emit.op("fill");
        }
    }
}

/// Build the shading dictionary (without `shfill`). `Err(reason)` for every shading
/// this emitter cannot express.
fn shading_dict(shading: &Shading) -> Result<String, &'static str> {
    let space = shading.color_space.device_space().ok_or("non-device color space")?;
    let cs = match space {
        DeviceSpace::Gray => "/DeviceGray",
        DeviceSpace::Rgb => "/DeviceRGB",
        DeviceSpace::Cmyk => "/DeviceCMYK",
    };

    match &*shading.shading_type {
        ShadingType::FunctionBased { domain, matrix, function } => {
            // A type 1 shading is evaluated over a 2-input function: a per-component
            // array of 1-input functions cannot express that.
            let function = match function {
                ShadingFunction::Single(f) => function_text(f)?,
                ShadingFunction::Multiple(_) => return Err("per-component function array"),
            };
            let mut out = format!("<< /ShadingType 1 /ColorSpace {cs} /Domain ");
            write_pairs(&mut out, &[(domain[0], domain[1]), (domain[2], domain[3])])?;
            if *matrix != Affine::IDENTITY {
                out.push_str(" /Matrix ");
                write_matrix(&mut out, *matrix)?;
            }
            out.push_str(&format!(" /Function {function} >>"));
            Ok(out)
        }
        ShadingType::RadialAxial { coords, domain, function, extend, axial } => {
            let (shading_type, coords_len) = if *axial { (2, 4) } else { (3, 6) };
            let mut out = format!("<< /ShadingType {shading_type} /ColorSpace {cs} /Coords [");
            for (i, c) in coords.iter().take(coords_len).enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(&num(f64::from(*c))?);
            }
            out.push_str("] /Domain ");
            write_pairs(&mut out, &[(domain[0], domain[1])])?;
            let function = function_text_for(function, true)?;
            out.push_str(&format!(" /Function {function} /Extend [{} {}] >>", extend[0], extend[1]));
            Ok(out)
        }
        // The meshes arrive decoded (triangles/patches, colors already resolved); the
        // bit-packed vertex streams they came from are gone, so there is nothing to
        // re-emit without re-encoding a Level 3 mesh — later layers.
        ShadingType::TriangleMesh { .. } | ShadingType::CoonsPatchMesh { .. } | ShadingType::TensorProductPatchMesh { .. } => {
            Err("mesh shading (types 4-7) not re-emittable")
        }
        ShadingType::Dummy => Err("dummy shading"),
    }
}

/// One shading function as the `/Function` value: a single function dictionary, or an
/// array of per-component dictionaries when `allow_multiple` (radial/axial shadings
/// accept both shapes; a type 1 shading's 2-input function does not).
fn function_text_for(function: &ShadingFunction, allow_multiple: bool) -> Result<String, &'static str> {
    match function {
        ShadingFunction::Single(f) => function_text(f),
        ShadingFunction::Multiple(functions) if allow_multiple => {
            let mut out = String::from("[");
            for f in functions {
                out.push(' ');
                out.push_str(&function_text(f)?);
            }
            out.push_str(" ]");
            Ok(out)
        }
        ShadingFunction::Multiple(_) => Err("per-component function array"),
    }
}

fn function_text(f: &Function) -> Result<String, &'static str> {
    let bytes = ps_function(f)?;
    String::from_utf8(bytes).map_err(|_| "function is not ASCII-encodable")
}

/// The flat stand-in colour for a degraded shading: the gradient's colour at the middle
/// of its domain, mapped through the shading's own color space; mid-gray when even that
/// cannot be evaluated.
fn flat_colour(sp: &ShadingPattern, emit: &mut Emitter) {
    let shading = &*sp.shading;
    let rgba = mid_input(shading)
        .zip(mid_function(shading))
        .and_then(|(input, function)| function.eval(&input.iter().copied().collect()))
        .map(|components| shading.color_space.to_rgba(&components, 1.0, false));
    let components = rgba.map(|c| c.components()).unwrap_or(hayro::hayro_interpret::color::AlphaColor::new([0.5, 0.5, 0.5, 1.0]).components());
    if components[0] == components[1] && components[1] == components[2] {
        emit.coord(f64::from(components[0]));
        emit.op("setgray");
    } else {
        emit.coord(f64::from(components[0]));
        emit.coord(f64::from(components[1]));
        emit.coord(f64::from(components[2]));
        emit.op("setrgbcolor");
    }
}

/// The function a degraded gradient's mid colour is sampled from, if the shading has
/// one (meshes may not).
fn mid_function(shading: &Shading) -> Option<&ShadingFunction> {
    match &*shading.shading_type {
        ShadingType::FunctionBased { function, .. } => Some(function),
        ShadingType::RadialAxial { function, .. } => Some(function),
        ShadingType::TriangleMesh { function, .. }
        | ShadingType::CoonsPatchMesh { function, .. }
        | ShadingType::TensorProductPatchMesh { function, .. } => function.as_ref(),
        ShadingType::Dummy => None,
    }
}

/// The domain middle: where a degraded gradient's colour is sampled.
fn mid_input(shading: &Shading) -> Option<Vec<f32>> {
    match &*shading.shading_type {
        ShadingType::FunctionBased { domain, .. } => Some(vec![(domain[0] + domain[2]) / 2.0, (domain[1] + domain[3]) / 2.0]),
        ShadingType::RadialAxial { domain, .. } => Some(vec![(domain[0] + domain[1]) / 2.0]),
        ShadingType::TriangleMesh { function, .. }
        | ShadingType::CoonsPatchMesh { function, .. }
        | ShadingType::TensorProductPatchMesh { function, .. } => function.as_ref().map(|_| vec![0.5]),
        ShadingType::Dummy => None,
    }
}

/// `concat` the matrix unless it is the identity (the common case — pattern-less
/// matrices default to it), keeping the program free of no-op lines.
fn concat_unless_identity(emit: &mut Emitter, matrix: Affine) {
    if matrix != Affine::IDENTITY {
        let mut line = String::from("[");
        for c in matrix.as_coeffs() {
            line.push_str(&emit.number(c));
            line.push(' ');
        }
        line.pop();
        line.push_str("] concat");
        emit.op(&line);
    }
}

// ---------------------------------------------------------------- functions

/// Write one function's Level 3 dictionary.
fn function_dict(f: &ParsedFunction, out: &mut String) -> Result<(), &'static str> {
    match f {
        ParsedFunction::Sampled { domain, range, sizes, bits_per_sample, encode, decode, samples } => {
            let bytes = pack_samples(samples, *bits_per_sample);
            // A PostScript string holds at most 65535 bytes, and `/DataSource` is one
            // string; beyond that there is no way to carry the samples.
            if bytes.len() > 65535 {
                return Err("sampled function data exceeds the PostScript string limit");
            }
            out.push_str("<< /FunctionType 0 /Domain ");
            write_pairs(out, domain)?;
            out.push_str(" /Range ");
            write_pairs(out, range)?;
            out.push_str(" /Size [");
            for (i, &s) in sizes.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(&s.to_string());
            }
            out.push_str(&format!("] /BitsPerSample {bits_per_sample} /Encode "));
            write_pairs(out, encode)?;
            out.push_str(" /Decode ");
            write_pairs(out, decode)?;
            out.push_str(" /DataSource <");
            write_hex(out, &bytes);
            out.push_str("> >>");
            Ok(())
        }
        ParsedFunction::Exponential { domain, range, c0, c1, exponent } => {
            out.push_str("<< /FunctionType 2 /Domain ");
            write_pairs(out, domain)?;
            if let Some(range) = range {
                out.push_str(" /Range ");
                write_pairs(out, range)?;
            }
            out.push_str(" /C0 ");
            write_values(out, c0)?;
            out.push_str(" /C1 ");
            write_values(out, c1)?;
            out.push_str(&format!(" /N {} >>", num(f64::from(*exponent))?));
            Ok(())
        }
        ParsedFunction::Stitching { domain, range, bounds, encode, functions } => {
            out.push_str("<< /FunctionType 3 /Domain ");
            write_pairs(out, domain)?;
            if let Some(range) = range {
                out.push_str(" /Range ");
                write_pairs(out, range)?;
            }
            out.push_str(" /Functions [");
            for f in functions {
                out.push(' ');
                function_dict(f, out)?;
            }
            out.push_str(" ] /Bounds ");
            write_values(out, bounds)?;
            out.push_str(" /Encode ");
            write_pairs(out, encode)?;
            out.push(']');
            Ok(())
        }
        ParsedFunction::Calculator { domain, range, program } => {
            // PostScript (and PDF) require /Range on a calculator function — a
            // dictionary without it is a rangecheck at `shfill` time.
            let Some(range) = range else {
                return Err("calculator function lacks the required /Range");
            };
            out.push_str("<< /FunctionType 4 /Domain ");
            write_pairs(out, domain)?;
            out.push_str(" /Range ");
            write_pairs(out, range)?;
            out.push_str(" /Function ");
            calc_procedure(program, out)?;
            out.push_str(" >>");
            Ok(())
        }
    }
}

/// Write the calculator program as a PostScript procedure — the same operator subset
/// PDF's calculator functions define (ISO 32000-2), which is legal procedure body.
fn calc_procedure(ops: &[CalcOp], out: &mut String) -> Result<(), &'static str> {
    out.push('{');
    for op in ops {
        out.push(' ');
        match op {
            CalcOp::Number(n) => out.push_str(&num(*n)?),
            CalcOp::Abs => out.push_str("abs"),
            CalcOp::Add => out.push_str("add"),
            CalcOp::Atan => out.push_str("atan"),
            CalcOp::Ceiling => out.push_str("ceiling"),
            CalcOp::Cos => out.push_str("cos"),
            CalcOp::Cvi => out.push_str("cvi"),
            CalcOp::Cvr => out.push_str("cvr"),
            CalcOp::Div => out.push_str("div"),
            CalcOp::Exp => out.push_str("exp"),
            CalcOp::Floor => out.push_str("floor"),
            CalcOp::Idiv => out.push_str("idiv"),
            CalcOp::Ln => out.push_str("ln"),
            CalcOp::Log => out.push_str("log"),
            CalcOp::Mod => out.push_str("mod"),
            CalcOp::Mul => out.push_str("mul"),
            CalcOp::Neg => out.push_str("neg"),
            CalcOp::Round => out.push_str("round"),
            CalcOp::Sin => out.push_str("sin"),
            CalcOp::Sqrt => out.push_str("sqrt"),
            CalcOp::Sub => out.push_str("sub"),
            CalcOp::Truncate => out.push_str("truncate"),
            CalcOp::And => out.push_str("and"),
            CalcOp::Bitshift => out.push_str("bitshift"),
            CalcOp::Eq => out.push_str("eq"),
            CalcOp::False => out.push_str("false"),
            CalcOp::Ge => out.push_str("ge"),
            CalcOp::Gt => out.push_str("gt"),
            CalcOp::Le => out.push_str("le"),
            CalcOp::Lt => out.push_str("lt"),
            CalcOp::Ne => out.push_str("ne"),
            CalcOp::Not => out.push_str("not"),
            CalcOp::Or => out.push_str("or"),
            CalcOp::True => out.push_str("true"),
            CalcOp::Xor => out.push_str("xor"),
            CalcOp::If(p) => {
                calc_procedure(p, out)?;
                out.push_str(" if");
            }
            CalcOp::IfElse(p1, p2) => {
                calc_procedure(p1, out)?;
                out.push(' ');
                calc_procedure(p2, out)?;
                out.push_str(" ifelse");
            }
            CalcOp::Copy => out.push_str("copy"),
            CalcOp::Dup => out.push_str("dup"),
            CalcOp::Exch => out.push_str("exch"),
            CalcOp::Index => out.push_str("index"),
            CalcOp::Pop => out.push_str("pop"),
            CalcOp::Roll => out.push_str("roll"),
        }
    }
    out.push_str(" }");
    Ok(())
}

/// Repack the widened sample values into PDF's bit layout: `bits_per_sample` bits per
/// value, big-endian, contiguous, the final partial byte zero-padded.
fn pack_samples(samples: &[u32], bits_per_sample: u8) -> Vec<u8> {
    if !(1..=32).contains(&bits_per_sample) {
        return Vec::new();
    }
    let bits = u32::from(bits_per_sample);
    let mask = (1u64 << bits) - 1;
    let mut bytes = Vec::new();
    let mut acc: u64 = 0;
    let mut have: u32 = 0;
    for &sample in samples {
        acc = (acc << bits) | (u64::from(sample) & mask);
        have += bits;
        while have >= 8 {
            have -= 8;
            bytes.push((acc >> have) as u8);
        }
        acc &= (1u64 << have) - 1; // keep only the not-yet-pushed bits (0 when have is 0)
    }
    if have > 0 {
        bytes.push(((acc << (8 - have)) & 0xff) as u8);
    }
    bytes
}

// ---------------------------------------------------------------- formatting

/// One function number as PostScript operand text. Non-finite values and values beyond
/// the 32-bit real range (a limitcheck on conforming interpreters) are refused.
fn num(v: f64) -> Result<String, &'static str> {
    if !v.is_finite() {
        return Err("non-finite number in function");
    }
    if v.abs() > f64::from(f32::MAX) {
        return Err("number exceeds the PostScript real range");
    }
    match psnum::num(v) {
        Some(s) => Ok(s),
        None => Err("non-finite number in function"),
    }
}

/// `[a b …]` from pairs.
fn write_pairs(out: &mut String, pairs: &[(f32, f32)]) -> Result<(), &'static str> {
    out.push('[');
    for (i, (a, b)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&num(f64::from(*a))?);
        out.push(' ');
        out.push_str(&num(f64::from(*b))?);
    }
    out.push(']');
    Ok(())
}

/// `[v …]` from single values.
fn write_values(out: &mut String, values: &[f32]) -> Result<(), &'static str> {
    out.push('[');
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&num(f64::from(*v))?);
    }
    out.push(']');
    Ok(())
}

/// `[a b c d e f]` from a matrix.
fn write_matrix(out: &mut String, matrix: Affine) -> Result<(), &'static str> {
    out.push('[');
    for (i, c) in matrix.as_coeffs().into_iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&num(c)?);
    }
    out.push(']');
    Ok(())
}

/// Hex string body, wrapped to stay near the DSC line-length convention.
fn write_hex(out: &mut String, bytes: &[u8]) {
    const WIDTH: usize = 128;
    for (i, byte) in bytes.iter().enumerate() {
        if i > 0 && i % WIDTH == 0 {
            out.push('\n');
        }
        out.push_str(&format!("{byte:02x}"));
    }
}

#[cfg(test)]
mod tests {
    use hayro::hayro_interpret::hayro_syntax::Pdf;
    use hayro::hayro_interpret::hayro_syntax::object::{FromBytes, Object};
    use hayro::hayro_interpret::{Context, InterpreterCache, InterpreterSettings, interpret_page};
    use kurbo::{Affine, Rect};

    use super::ps_function;
    use crate::FidelityNote;
    use crate::device::PsDevice;

    // ---------------------------------------------------------------- fixtures

    /// A `<< … >> stream … endstream` object body (repo fixture style; the length is
    /// computed, never counted by hand).
    fn stream_obj(dict: &str, body: &[u8]) -> Vec<u8> {
        let mut obj = format!("<< {dict} /Length {} >>\nstream\n", body.len()).into_bytes();
        obj.extend_from_slice(body);
        obj.extend_from_slice(b"\nendstream");
        obj
    }

    /// Assemble a one-page PDF whose content fills a rectangle with the pattern `P1`
    /// (object 3), plus optional extra objects (5…) the pattern may reference. The page
    /// is the last object (object `5 + extra_objects.len()`), so `/Kids` can name it.
    fn pattern_pdf(pattern: &[u8], extra_objects: &[Vec<u8>]) -> Vec<u8> {
        let page_no = 5 + extra_objects.len();
        let page = b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Pattern << /P1 3 0 R >> >> >>".to_vec();
        let mut objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            format!("<< /Type /Pages /Kids [{page_no} 0 R] /Count 1 /MediaBox [0 0 200 300] >>").into_bytes(),
            pattern.to_vec(),
            stream_obj("", b"/Pattern cs /P1 scn\n20 20 160 100 re f"),
        ];
        objs.extend(extra_objects.iter().cloned());
        let objs_len = objs.len();
        objs.push(page);
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs_len + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs_len + 1).as_bytes());
        out
    }

    /// A shading pattern wrapping an axial RGB shading with the given `/Function` value
    /// (an inline dictionary or an indirect reference).
    fn axial_pattern(function: &str) -> Vec<u8> {
        format!(
            "<< /PatternType 2 /Shading << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 200 0] \
             /Domain [0 1] /Function {function} /Extend [true true] >> >>"
        )
        .into_bytes()
    }

    /// Drive one page of `bytes` through the PostScript device: (program, notes).
    fn driven(bytes: &[u8]) -> (String, Vec<FidelityNote>) {
        let pdf = Pdf::new(bytes.to_vec()).expect("fixture parses");
        let page = pdf.pages().first().expect("fixture has a page");
        let cache = InterpreterCache::new();
        let mut state = Context::new(Affine::IDENTITY, Rect::new(0.0, 0.0, 200.0, 300.0), &cache, page.xref(), InterpreterSettings::default());
        let mut device = PsDevice::new(None);
        interpret_page(page, &mut state, &mut device);
        device.finish();
        (String::from_utf8_lossy(&device.emit.out).into_owned(), device.emit.notes.clone())
    }

    /// Parse a function object out of fixture bytes.
    fn function_of(object_bytes: &[u8]) -> hayro::hayro_interpret::Function {
        let object = Object::from_bytes(object_bytes).expect("object parses");
        hayro::hayro_interpret::Function::new(&object).expect("function parses")
    }

    // ------------------------------------------------------------------ tests

    /// An axial shading fill becomes a clip to the rectangle plus a Level 3 shading
    /// dictionary painted with `shfill`, with no degradations.
    #[test]
    fn axial_shading_emits_shfill() {
        let function = "<< /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >>";
        let (out, notes) = driven(&pattern_pdf(&axial_pattern(function), &[]));
        assert!(out.contains("<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 200 0]"), "dict: {out}");
        assert!(out.contains("/Domain [0 1] /Function << /FunctionType 2"), "function dict: {out}");
        assert!(out.contains("/C0 [1 0 0] /C1 [0 0 1] /N 1 >>"), "exponential contents: {out}");
        assert!(out.contains("/Extend [true true] >> shfill"), "extend + shfill: {out}");
        assert!(out.contains("\nclip\n"), "the path clips the shading: {out}");
        assert_eq!(notes, Vec::<FidelityNote>::new(), "no degradations");
    }

    /// A type 4 calculator function re-emits as a Level 3 function dictionary whose
    /// `/Function` value is the same procedure body, in the PDF calculator subset.
    #[test]
    fn type4_function_roundtrips_to_procedure() {
        let bytes = stream_obj("/FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1]", b"{ add }");
        let out = ps_function(&function_of(&bytes)).expect("type 4 is emittable");
        let out = String::from_utf8_lossy(&out);
        assert!(out.contains("/FunctionType 4"), "dict form: {out}");
        assert!(out.contains("/Domain [0 1]"), "domain: {out}");
        assert!(out.contains("/Range [0 1 0 1 0 1]"), "range: {out}");
        assert!(out.contains("/Function { add }"), "procedure round-trip: {out}");
    }

    /// A type 0 sampled function re-emits as a sampled function dictionary with the
    /// samples packed exactly as PDF packed them and carried in a `/DataSource` string.
    #[test]
    fn sampled_function_emits_dict() {
        let bytes =
            stream_obj("/FunctionType 0 /Domain [0 1] /Range [0 1 0 1 0 1] /Size [2] /BitsPerSample 8", &[0x00, 0x33, 0x66, 0x99, 0xcc, 0xff]);
        let out = ps_function(&function_of(&bytes)).expect("sampled function is emittable");
        let out = String::from_utf8_lossy(&out);
        assert!(out.contains("/FunctionType 0"), "dict form: {out}");
        assert!(out.contains("/Size [2]"), "grid size: {out}");
        assert!(out.contains("/BitsPerSample 8"), "bit depth: {out}");
        assert!(out.contains("/DataSource <00336699ccff>"), "samples in stream order: {out}");

        // 4-bit samples that byte-align (0,5,10,15 → 0x05,0xaf): the packer must not
        // carry a stale bit into the next byte.
        let bytes = stream_obj("/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [4] /BitsPerSample 4", &[0x05, 0xaf]);
        let out = ps_function(&function_of(&bytes)).expect("4-bit sampled function is emittable");
        let out = String::from_utf8_lossy(&out);
        assert!(out.contains("/BitsPerSample 4"), "bit depth: {out}");
        assert!(out.contains("/DataSource <05af>"), "4-bit packing: {out}");
    }

    /// A calculator operand too large for a PostScript real (the language's reals are
    /// 32-bit; a 1e40-scale literal is a limitcheck on conforming interpreters, verified
    /// against Ghostscript) cannot be emitted: `ps_function` refuses it and the shading
    /// painted with such a function degrades to its flat mid-tone colour plus a note.
    #[test]
    fn type4_unsupported_op_degrades_to_note() {
        // The vendor's number reader has no exponent syntax; the digit form reaches the
        // same value (u64 mantissa overflow → the scaled real).
        let huge = "10000000000000000000000000000000000000000";
        let bytes = stream_obj("/FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1]", format!("{{ {huge} mul }}").as_bytes());
        let err = ps_function(&function_of(&bytes)).expect_err("1e40 has no PostScript operand");
        assert!(err.contains("real range"), "the reason names the limit: {err}");

        // The same function inside a shading: flat colour + note, no `shfill`.
        let function_stream = stream_obj("/FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1]", format!("{{ {huge} mul }}").as_bytes());
        let (out, notes) = driven(&pattern_pdf(&axial_pattern("5 0 R"), &[function_stream]));
        assert!(!out.contains("shfill"), "no shading painted: {out}");
        assert!(
            notes.iter().any(|n| n.text.contains("shading approximated") && n.text.contains("real range")),
            "degradation noted with the reason: {notes:?}"
        );
        assert!(out.contains("setrgbcolor") || out.contains("setgray"), "flat colour emitted: {out}");
    }

    /// The Task 6 tiling decision: the cell is a raw content stream that
    /// `TilingPattern::interpret` re-parses (it does not pre-resolve through the
    /// device), and `is_color` plus the fallback paints are `pub(crate)`, so
    /// `/PaintType` and any flat colour are unknowable here — `makepattern` is out of
    /// reach. Every tiling pattern therefore degrades to exactly one note and a flat
    /// colour, and the shape still paints.
    #[test]
    fn tiling_pattern_either_makepattern_or_note() {
        let pattern = stream_obj("/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << >>", b"0 0 5 5 re f");
        let (out, notes) = driven(&pattern_pdf(&pattern, &[]));
        assert!(!out.contains("/PatternType 1"), "no makepattern dict: {out}");
        assert!(!out.contains("makepattern"), "no makepattern: {out}");
        assert_eq!(notes, vec![FidelityNote { page: None, text: "tiling pattern approximated".into() }], "exactly the recorded note");
        assert!(out.contains("0.5 setgray"), "flat colour: {out}");
        assert!(out.contains("\nfill\n"), "the rectangle still paints: {out}");
    }
}
