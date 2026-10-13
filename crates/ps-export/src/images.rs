//! Image emission: filter passthrough, decode + re-encode, and stencil masks (Layer A, Task 4).
//!
//! Every image is emitted in the dictionary form of `image`/`imagemask` with a
//! `currentfile` filter chain as `/DataSource` — the DSC-safe standard pattern, verified
//! against the Ghostscript oracle (bare `colorimage` cannot take a file data source; the
//! dictionary form is what PostScript Level 2+ generators emit):
//!
//! ```text
//! gsave
//! [a b c d e f] concat                      %% the transform, image pixel space
//! /DeviceRGB setcolorspace                  %% or `0 setgray` / paint ops
//! %%BeginData: <bytes> Binary Bytes          %% binary passthrough sections only
//! << /ImageType 1 /Width w /Height h /BitsPerComponent 8 /Decode [0 1 0 1 0 1]
//!    /ImageMatrix [1 0 0 1 0 0]
//!    /DataSource currentfile /DCTDecode filter >> image
//! <bytes …>                                  %% the payload, after the invocation line
//! %%EndData
//! grestore
//! ```
//!
//! `/ImageMatrix` is the identity because the interpreter's `transform` already maps
//! *image pixel space* ([0,w]×[0,h], row 0 first) onto the page: it pre-concats
//! `[1/w 0 0 -1/h 0 1]` onto the CTM before `draw_image` (the y-flip puts row 0 at the
//! top), so the sample grid and user space coincide and no further matrix is needed.
//!
//! Payload routing:
//! - `/DCTDecode`, `/FlateDecode` (Level 3), `/CCITTFaxDecode` pass their still-encoded
//!   bytes through verbatim when the colour space, bit depth and decode array are exactly
//!   representable in PostScript — the PDF's own compressed bytes land in the program
//!   unchanged, inside `%%BeginData`/`%%EndData` so DSC consumers can skip the binary.
//! - Everything else decodes through `RasterImage::with_rgba` / `StencilImage::with_stencil`
//!   (the same machinery the raster renderer uses) and re-encodes as Flate + ASCII85,
//!   which keeps the program pure ASCII.
//! - A stencil (`/ImageMask` true) becomes `imagemask`: the decoded luma (255 = paint)
//!   is packed to 1-bit samples and painted with the stencil's own paint colour.
//! - Undecodable images record the note "image skipped (undecodable)" and emit nothing;
//!   the export continues. An image with `/SMask`/`/Mask` is emitted unmasked with the
//!   note "image soft mask ignored" (pre-compositing is not reachable from this crate in
//!   Layer A).

use hayro::hayro_interpret::hayro_syntax::Filter;
use hayro::hayro_interpret::hayro_syntax::object::{Array, Dict, Name};
use hayro::hayro_interpret::{Image, ImageData, RasterImage, StencilImage};
use pdfcraft_filters::{Filter as PsFilter, Params};

use crate::device::{Emitter, paint_ops};

/// Images over this many pixels are skipped (and noted): a hostile `/Width`×`/Height`
/// pair would otherwise size sample buffers far beyond anything a page can show.
const MAX_PIXELS: u64 = 100_000_000;

/// Emit the operators for one image (inside the `gsave`…`grestore` the device wrapped
/// around the draw). Dimensions come from the XObject dict; the sample count is capped
/// at [`MAX_PIXELS`] with a note so a hostile `/Width`×`/Height` can never size buffers.
pub(crate) fn image_ops(image: &Image<'_, '_>, emit: &mut Emitter) {
    let pixels = u64::from(image.width()).checked_mul(u64::from(image.height()));
    if !pixels.is_some_and(|p| p <= MAX_PIXELS) {
        emit.note("image skipped (exceeds 100 megapixel cap)");
        return;
    }
    match image {
        Image::Raster(raster) => raster_ops(raster, emit),
        Image::Stencil(stencil) => stencil_ops(stencil, emit),
    }
}

/// A raster image: passthrough when PostScript can consume the PDF's own encoded bytes,
/// decode + re-encode otherwise.
fn raster_ops(raster: &RasterImage<'_>, emit: &mut Emitter) {
    let stream = raster.stream();
    let dict = stream.dict();
    let masked = has_mask(dict);
    let filters = stream.filters();
    let raw = stream.raw_data();

    // Passthrough: single filter, colour space known by name, decode array identity (or
    // absent), and for Flate also 8 bits per component without a predictor (PostScript's
    // FlateDecode knows nothing of PDF's PNG/TIFF predictors). An empty payload cannot
    // pass through (the filter would error mid-page), so it falls to the decode path.
    let plan = match filters.as_slice() {
        [Filter::DctDecode] if !raw.is_empty() => passthrough_components(dict).filter(|&n| decode_is_identity(dict, n)).map(|n| Plan {
            width: raster.width(),
            height: raster.height(),
            components: n,
            source: "/DCTDecode filter".into(),
            bits: 8,
        }),
        [Filter::FlateDecode] if !raw.is_empty() => passthrough_components(dict)
            .filter(|&n| bpc_is_8(dict) && no_predictor(dict) && decode_is_identity(dict, n))
            .map(|n| Plan { width: raster.width(), height: raster.height(), components: n, source: "/FlateDecode filter".into(), bits: 8 }),
        [Filter::CcittFaxDecode] if !raw.is_empty() => decode_is_identity(dict, 1).then(|| ccitt_source(dict)).flatten().map(|source| Plan {
            width: raster.width(),
            height: raster.height(),
            components: 1,
            source,
            bits: 1,
        }),
        _ => None,
    };

    if let Some(plan) = plan {
        if masked {
            emit.note("image soft mask ignored");
        }
        plan.emit(emit, &raw);
        return;
    }

    // Decode through the same machinery the raster renderer uses. `with_rgba` runs its
    // closure only on success, so an undecodable image leaves the slot empty.
    let mut decoded = None;
    raster.with_rgba(|image, _alpha| decoded = Some(image), None);
    let Some(image) = decoded else {
        emit.note("image skipped (undecodable)");
        return;
    };
    if masked {
        emit.note("image soft mask ignored");
    }

    match image {
        ImageData::Rgb(rgb) => {
            let expected = pixel_bytes(rgb.width, rgb.height, 3);
            if !samples_complete(expected, &rgb.data) {
                emit.note("image skipped (undecodable)");
                return;
            }
            emit.op("/DeviceRGB setcolorspace");
            let line = image_invocation(rgb.width, rgb.height, 8, "[0 1 0 1 0 1]", A85_FLATE_SOURCE);
            emit_ascii85(emit, &line, &rgb.data);
        }
        ImageData::Luma(luma) => {
            let expected = pixel_bytes(luma.width, luma.height, 1);
            if !samples_complete(expected, &luma.data) {
                emit.note("image skipped (undecodable)");
                return;
            }
            emit.op("0 setgray");
            let line = image_invocation(luma.width, luma.height, 8, "[0 1]", A85_FLATE_SOURCE);
            emit_ascii85(emit, &line, &luma.data);
        }
    }
}

/// A stencil mask (`/ImageMask` true): decoded luma packed to 1 bit and painted with the
/// stencil's own paint colour. `StencilImage` exposes no stream, so there is no
/// passthrough for masks (findings (a)).
fn stencil_ops(stencil: &StencilImage<'_, '_>, emit: &mut Emitter) {
    let mut decoded = None;
    stencil.with_stencil(|luma, paint| decoded = Some((luma, paint.clone())), None);
    let Some((luma, paint)) = decoded else {
        emit.note("image skipped (undecodable)");
        return;
    };

    // PDF mask sample 0 paints; hayro's decode turns that into luma 255, so the packed
    // bit is 1 where the sample paints and `/Decode [1 0]` makes 1s paint in PostScript
    // (PDF and PostScript defaults are each the other way round — verified against the
    // Ghostscript oracle).
    let Some(bits) = pack_mask_bits(&luma.data, luma.width, luma.height) else {
        emit.note("image skipped (undecodable)");
        return;
    };
    paint_ops(emit, &paint);
    let line = format!(
        "<< /ImageType 1 /Width {} /Height {} /BitsPerComponent 1 /Decode [1 0] \
/ImageMatrix [1 0 0 1 0 0] /DataSource currentfile {A85_FLATE_SOURCE} >> imagemask",
        luma.width, luma.height
    );
    emit_ascii85(emit, &line, &bits);
}

/// The `/DataSource` expression for decoded data: ASCII85 over Flate, both standard
/// PostScript Level 2 filters, keeping the program pure ASCII.
const A85_FLATE_SOURCE: &str = "/ASCII85Decode filter /FlateDecode filter";

/// One planned image emission.
struct Plan {
    width: u32,
    height: u32,
    components: u8,
    /// The filter expression placed after `currentfile` (e.g. `/DCTDecode filter`).
    source: String,
    bits: u8,
}

impl Plan {
    fn emit(&self, emit: &mut Emitter, payload: &[u8]) {
        // `passthrough_components` only yields these three; 4 is the catch-all.
        let (setup, decode) = match self.components {
            1 => ("0 setgray", "[0 1]"),
            3 => ("/DeviceRGB setcolorspace", "[0 1 0 1 0 1]"),
            _ => ("/DeviceCMYK setcolorspace", "[0 1 0 1 0 1 0 1]"),
        };
        emit.op(setup);
        let line = image_invocation(self.width, self.height, self.bits, decode, &self.source);
        // The payload is binary: bracket it as a DSC data section so consumers can skip
        // the bytes, and count the invocation line plus its newline plus the payload, in
        // the DSC 3.0 form `<numberof> <type> <bytesorlines>` ("Binary Bytes" — the
        // examples in Adobe 5001.DSC_Spec.pdf, e.g. `%%BeginData: 135174 Hex Bytes`).
        let count = line.len() + 1 + payload.len();
        emit.op(&format!("%%BeginData: {count} Binary Bytes"));
        emit.op(&line);
        emit.out.extend_from_slice(payload);
        emit.out.push(b'\n');
        emit.op("%%EndData");
    }
}

/// The dictionary-form `image`/`imagemask` invocation. `/ImageMatrix` is the identity:
/// the transform the interpreter handed over already maps image pixel space onto the
/// page (see the module docs).
fn image_invocation(width: u32, height: u32, bits: u8, decode: &str, source: &str) -> String {
    format!(
        "<< /ImageType 1 /Width {width} /Height {height} /BitsPerComponent {bits} /Decode {decode} \
/ImageMatrix [1 0 0 1 0 0] /DataSource currentfile {source} >> image"
    )
}

/// The expected sample-byte count of a `width`×`height` image with `components` bytes
/// per pixel, or `None` where the arithmetic overflows.
fn pixel_bytes(width: u32, height: u32, components: u32) -> Option<usize> {
    usize::try_from(u64::from(width).checked_mul(u64::from(height))?.checked_mul(u64::from(components))?).ok()
}

/// Emit `invocation` followed by its data as ASCII85(Flate(`data`)). The encoded form is
/// pure ASCII with `~>` as the end-of-data marker, so no `%%BeginData` bracket is needed.
fn emit_ascii85(emit: &mut Emitter, invocation: &str, data: &[u8]) {
    let z = pdfcraft_filters::encode_flate(data);
    let Ok(a85) = pdfcraft_filters::encode(&PsFilter::Ascii85, &Params::default(), &z) else {
        // Unreachable for ASCII85 (the encoder cannot fail); kept for never-crash shape.
        emit.note("image skipped (undecodable)");
        return;
    };
    emit.op(invocation);
    emit.out.extend_from_slice(&a85);
    emit.out.push(b'\n');
}

/// Whether a decoded sample buffer has exactly the length its dimensions promise (hayro
/// guarantees this; a violation means the buffer cannot be emitted whole).
fn samples_complete(expected: Option<usize>, data: &[u8]) -> bool {
    expected.is_some_and(|n| data.len() == n)
}

/// Pack decoded stencil luma (255 = paint) into 1-bit samples, MSB first within each
/// row, rows padded to whole bytes (the PDF/PostScript mask layout). Luma data shorter
/// than `width`×`height` cannot be packed and yields `None`.
fn pack_mask_bits(luma: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let w = usize::try_from(width).ok()?;
    let h = usize::try_from(height).ok()?;
    if luma.len() < w.checked_mul(h)? {
        return None;
    }
    let stride = w.checked_add(7)? / 8;
    let mut out = vec![0u8; stride.checked_mul(h)?];
    for y in 0..h {
        for x in 0..w {
            if luma[y * w + x] >= 128 {
                out[y * stride + x / 8] |= 1 << (7 - (x % 8));
            }
        }
    }
    Some(out)
}

/// Whether the image dict carries any kind of alpha mask (the same keys hayro's
/// `has_mask` checks): such images are emitted unmasked, with a note.
fn has_mask(dict: &Dict<'_>) -> bool {
    dict.contains_key(b"SMask") || dict.contains_key(b"Mask") || dict.contains_key(b"SMaskInData")
}

/// The component count the dict's colour space implies, when it is a plain device space
/// PostScript can name. Arrays (Indexed, ICCBased, CalRGB, Separation, …) and unknown
/// names return `None` — those take the decode path, which normalises to RGB/gray.
fn passthrough_components(dict: &Dict<'_>) -> Option<u8> {
    let cs = dict.get::<Name<'_>>(b"ColorSpace").or_else(|| dict.get::<Name<'_>>(b"CS"))?;
    match cs.as_str() {
        "DeviceGray" | "G" => Some(1),
        "DeviceRGB" | "RGB" => Some(3),
        "DeviceCMYK" | "CMYK" => Some(4),
        _ => None,
    }
}

/// Whether `/Decode` is absent or the identity `[0 1]` per component. An inverted or
/// otherwise remapped decode array has no passthrough equivalent.
fn decode_is_identity(dict: &Dict<'_>, components: u8) -> bool {
    let Some(array) = dict.get::<Array<'_>>(b"Decode").or_else(|| dict.get::<Array<'_>>(b"D")) else {
        return true;
    };
    let pairs: Vec<(f32, f32)> = array.iter::<(f32, f32)>().collect();
    pairs.len() == components as usize && pairs.iter().all(|&(lo, hi)| lo == 0.0 && hi == 1.0)
}

/// Whether `/BitsPerComponent` is 8 (or absent, which hayro reads as 8 outside stencils).
fn bpc_is_8(dict: &Dict<'_>) -> bool {
    dict.get::<u8>(b"BitsPerComponent").or_else(|| dict.get::<u8>(b"BPC")).unwrap_or(8) == 8
}

/// Whether the general-purpose decode parameters carry no predictor (Predictor ≤ 1 or
/// absent). PDF's predictors have no PostScript FlateDecode equivalent. Read as i64 so
/// an out-of-range value blocks passthrough instead of falling back to the default.
fn no_predictor(dict: &Dict<'_>) -> bool {
    decode_parms(dict).is_none_or(|dp| dp.get::<i64>(b"Predictor").unwrap_or(1) <= 1)
}

/// The image's `/DecodeParms` as a single dict (a one-element array counts too, the shape
/// PDF allows for single-filter streams).
fn decode_parms<'a>(dict: &Dict<'a>) -> Option<Dict<'a>> {
    dict.get::<Dict<'_>>(b"DecodeParms")
        .or_else(|| dict.get::<Dict<'_>>(b"DP"))
        .or_else(|| dict.get::<Array<'_>>(b"DecodeParms").and_then(|a| a.iter::<Dict<'_>>().next()))
        .or_else(|| dict.get::<Array<'_>>(b"DP").and_then(|a| a.iter::<Dict<'_>>().next()))
}

/// The `/DataSource` expression for a CCITT image: the PostScript filter with the decode
/// parameters mapped across. PDF and PostScript share the CCITT parameter defaults
/// (K 0, Columns 1728, Rows 0, and the flags false/false/true), so only what the PDF
/// actually says is emitted; `/Columns` and `/Rows` are passed even when they equal the
/// image dimensions, because the filter defaults (1728/0) would silently misread rows.
/// `None` (no passthrough) when the parameters are outside the range a fax image can
/// have, matching hayro's own `ccitt_size_ok` bounds.
fn ccitt_source(dict: &Dict<'_>) -> Option<String> {
    const MAX_SIDE: i64 = 1 << 20;
    let dp = decode_parms(dict);
    let parm = |key: &[u8], default: i64| dp.as_ref().and_then(|d| d.get::<i64>(key)).unwrap_or(default);
    let columns = parm(b"Columns", 1728);
    let rows = parm(b"Rows", 0);
    if !(1..=MAX_SIDE).contains(&columns) || !(0..=MAX_SIDE).contains(&rows) {
        return None;
    }
    let mut parms = format!("/K {} /Columns {columns} /Rows {rows}", parm(b"K", 0));
    let flag = |key: &[u8], default: bool| dp.as_ref().and_then(|d| d.get::<bool>(key)).unwrap_or(default);
    if flag(b"BlackIs1", false) {
        parms.push_str(" /BlackIs1 true");
    }
    if flag(b"EndOfLine", false) {
        parms.push_str(" /EndOfLine true");
    }
    if flag(b"EncodedByteAlign", false) {
        parms.push_str(" /EncodedByteAlign true");
    }
    if !flag(b"EndOfBlock", true) {
        parms.push_str(" /EndOfBlock false");
    }
    Some(format!("<< {parms} >> /CCITTFaxDecode filter"))
}

#[cfg(test)]
mod tests {
    use hayro::hayro_interpret::hayro_syntax::Pdf;
    use hayro::hayro_interpret::{Context, InterpreterCache, InterpreterSettings, interpret_page};
    use kurbo::{Affine, Rect};

    use super::*;
    use crate::device::PsDevice;

    /// Arbitrary but recognisable still-encoded payloads; nothing in a passthrough test
    /// decodes them.
    const DCT_PAYLOAD: [u8; 6] = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc];
    const CCITT_PAYLOAD: [u8; 6] = [0x93, 0x51, 0x40, 0x04, 0x00, 0x40];
    /// 2×2 RGB samples, row 0 red twice, row 1 blue twice.
    const RGB_SAMPLES: [u8; 12] = [0xFF, 0, 0, 0xFF, 0, 0, 0, 0, 0xFF, 0, 0, 0xFF];
    /// 2×2 stencil mask bits, row 0 `10`, row 1 `01`.
    const MASK_BITS: [u8; 2] = [0x80, 0x40];

    /// A `<< … >> stream … endstream` object body, in the hand-written style of
    /// `crates/engine/src/tests.rs` (length computed, not counted by hand).
    fn stream_obj(dict: &str, body: &[u8]) -> Vec<u8> {
        let mut obj = format!("<< {dict} /Length {} >>\nstream\n", body.len()).into_bytes();
        obj.extend_from_slice(body);
        obj.extend_from_slice(b"\nendstream");
        obj
    }

    /// A minimal one-page PDF drawing one image XObject (`/Im1`) with `content`. Object 3
    /// is the image; `extra` objects are appended after it (for `/SMask`), so the page
    /// object lands last.
    fn image_pdf(image: &str, image_body: &[u8], extra: &[(&str, &[u8])], content: &[u8]) -> Vec<u8> {
        let content_no = 3 + extra.len() + 1;
        let page_no = content_no + 1;
        let mut objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            format!("<< /Type /Pages /Kids [{page_no} 0 R] /Count 1 /MediaBox [0 0 200 300] >>").into_bytes(),
            stream_obj(image, image_body),
        ];
        for (dict, body) in extra {
            objs.push(stream_obj(dict, body));
        }
        objs.push(stream_obj("", content));
        objs.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /Contents {content_no} 0 R \
                 /Resources << /XObject << /Im1 3 0 R >> >> >>"
            )
            .into_bytes(),
        );
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

    /// Parse `pdf` and drive the `PsDevice` over its single page, the same entry the
    /// raster renderer uses, returning the emitted program and the note texts. The
    /// content's `q 20 0 0 20 10 10 cm /Im1 Do` makes the device's image transform
    /// `[10 0 0 -10 10 30]` for a 2×2 image (the interpreter pre-concats
    /// `[1/2 0 0 -1/2 0 1]`, which also y-flips so row 0 lands at the top).
    fn driven(pdf: &[u8]) -> (Vec<u8>, Vec<String>) {
        let pdf = Pdf::new(pdf.to_vec()).expect("fixture parses");
        let page = pdf.pages().first().expect("fixture has a page");
        let cache = InterpreterCache::new();
        let mut state = Context::new(Affine::IDENTITY, Rect::new(0.0, 0.0, 200.0, 300.0), &cache, page.xref(), InterpreterSettings::default());
        let mut device = PsDevice::new(None);
        interpret_page(page, &mut state, &mut device);
        (device.emit.out.clone(), device.emit.notes.iter().map(|n| n.text.clone()).collect())
    }

    /// The program bytes as a (lossy) string plus the notes joined with newlines.
    fn output(raw: &[u8], notes: &[String]) -> (String, String) {
        (String::from_utf8_lossy(raw).into_owned(), notes.join("\n"))
    }

    /// Whether `haystack` contains `needle` as raw bytes.
    fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    /// ASCII85(Flate(`data`)) exactly as the emitter produces it.
    fn a85_flate(data: &[u8]) -> Vec<u8> {
        let z = pdfcraft_filters::encode_flate(data);
        pdfcraft_filters::encode(&PsFilter::Ascii85, &Params::default(), &z).unwrap()
    }

    /// A DCT image's encoded JPEG bytes pass through verbatim, inside a `%%BeginData`
    /// binary section whose byte count covers invocation line + newline + payload.
    #[test]
    fn jpeg_passthrough_keeps_bytes() {
        let pdf = image_pdf(
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode",
            &DCT_PAYLOAD,
            &[],
            b"q 20 0 0 20 10 10 cm /Im1 Do Q",
        );
        let (raw, notes) = driven(&pdf);
        let (out, notes) = output(&raw, &notes);
        let invocation = "<< /ImageType 1 /Width 2 /Height 2 /BitsPerComponent 8 /Decode [0 1 0 1 0 1] \
             /ImageMatrix [1 0 0 1 0 0] /DataSource currentfile /DCTDecode filter >> image";
        assert!(out.contains(invocation), "DCT invocation: {out}");
        assert!(out.contains("[10 0 0 -10 10 30] concat\n"), "pixel-space transform: {out}");
        assert!(out.contains("/DeviceRGB setcolorspace\n"), "3-component space: {out}");
        assert!(contains_bytes(&raw, &DCT_PAYLOAD), "exact DCT bytes verbatim: {out:?}");
        let count = invocation.len() + 1 + DCT_PAYLOAD.len();
        assert!(out.contains(&format!("%%BeginData: {count} Binary Bytes\n")), "count covers line+payload: {out}");
        assert!(out.contains("\n%%EndData\n"), "DSC data section closed: {out}");
        assert_eq!(notes, "", "no degradations for a clean passthrough");
    }

    /// A Flate-encoded 8-bit image passes its zlib bytes through verbatim for Level 3
    /// PostScript (`/FlateDecode` is a Level 3 filter).
    #[test]
    fn flate_passthrough_level3() {
        let payload = pdfcraft_filters::encode_flate(&RGB_SAMPLES);
        let pdf = image_pdf(
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode",
            &payload,
            &[],
            b"q 20 0 0 20 10 10 cm /Im1 Do Q",
        );
        let (raw, notes) = driven(&pdf);
        let (out, notes) = output(&raw, &notes);
        let invocation = "<< /ImageType 1 /Width 2 /Height 2 /BitsPerComponent 8 /Decode [0 1 0 1 0 1] \
             /ImageMatrix [1 0 0 1 0 0] /DataSource currentfile /FlateDecode filter >> image";
        assert!(out.contains(invocation), "Flate invocation: {out}");
        assert!(contains_bytes(&raw, &payload), "zlib bytes verbatim: {out:?}");
        assert!(out.contains("/DeviceRGB setcolorspace\n"), "{out}");
        assert_eq!(notes, "", "no degradations for a clean passthrough");
    }

    /// An image that can neither pass through nor decode is skipped with a note, and the
    /// program continues: the operators after it still land.
    #[test]
    fn undecodable_image_notes_and_continues() {
        let pdf = image_pdf(
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /JPXDecode",
            &[0x01, 0x02, 0x03],
            &[],
            b"q 20 0 0 20 10 10 cm /Im1 Do Q\n1 0 0 rg\n40 40 20 20 re f",
        );
        let (raw, notes) = driven(&pdf);
        let (out, notes) = output(&raw, &notes);
        assert_eq!(notes, "image skipped (undecodable)", "the skip is recorded: {notes}");
        assert!(!out.contains(">> image") && !out.contains("imagemask"), "no image operator: {out}");
        assert!(out.contains("1 0 0 setrgbcolor\nnewpath\n40 40 moveto\n") && out.contains("fill\n"), "export continues: {out}");
        assert_eq!(out.matches("gsave").count(), out.matches("grestore").count(), "balanced wrap: {out}");
    }

    /// A stencil mask (`/ImageMask true`) becomes `imagemask` with its decoded samples
    /// packed to 1 bit (1 = paint) and `/Decode [1 0]`, painted with the current colour.
    #[test]
    fn stencil_uses_imagemask() {
        let pdf = image_pdf(
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ImageMask true /BitsPerComponent 1",
            &MASK_BITS,
            &[],
            b"1 0 0 rg\nq 20 0 0 20 10 10 cm /Im1 Do Q",
        );
        let (raw, notes) = driven(&pdf);
        let (out, notes) = output(&raw, &notes);
        let invocation = "<< /ImageType 1 /Width 2 /Height 2 /BitsPerComponent 1 /Decode [1 0] \
             /ImageMatrix [1 0 0 1 0 0] /DataSource currentfile /ASCII85Decode filter /FlateDecode filter >> imagemask";
        assert!(out.contains(invocation), "imagemask invocation: {out}");
        // PDF mask sample 0 paints (hayro decodes it to luma 255), so the packed bits
        // invert the raw samples: row 0 `01`, row 1 `10` → 0x40 0x80.
        let expected = a85_flate(&[0x40, 0x80]);
        assert!(contains_bytes(&raw, &expected), "packed+re-encoded samples: {out:?}");
        let color = out.find("1 0 0 setrgbcolor").unwrap_or(usize::MAX);
        let mask = out.find("imagemask").unwrap_or(usize::MAX);
        assert!(color < mask, "the stencil's paint precedes the mask: {out}");
        assert_eq!(notes, "", "a clean stencil mask degrades nothing");
    }

    /// An image with an `/SMask` is emitted unmasked and the drop is recorded.
    #[test]
    fn smask_note_present() {
        let pdf = image_pdf(
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /SMask 4 0 R",
            &DCT_PAYLOAD,
            &[("/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8", &[0u8, 4][..])],
            b"q 20 0 0 20 10 10 cm /Im1 Do Q",
        );
        let (raw, notes) = driven(&pdf);
        let (out, notes) = output(&raw, &notes);
        assert_eq!(notes, "image soft mask ignored", "the dropped alpha is recorded: {notes}");
        assert!(out.contains("/DCTDecode filter >> image"), "the image itself is still emitted: {out}");
    }

    /// An image with no filter decodes and re-encodes as ASCII85(Flate(samples)) — an
    /// ASCII-only data section, so no `%%BeginData` bracket is needed.
    #[test]
    fn unfiltered_image_reencodes_ascii85_flate() {
        let pdf = image_pdf(
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8",
            &RGB_SAMPLES,
            &[],
            b"q 20 0 0 20 10 10 cm /Im1 Do Q",
        );
        let (raw, notes) = driven(&pdf);
        let (out, notes) = output(&raw, &notes);
        let invocation = "<< /ImageType 1 /Width 2 /Height 2 /BitsPerComponent 8 /Decode [0 1 0 1 0 1] \
             /ImageMatrix [1 0 0 1 0 0] /DataSource currentfile /ASCII85Decode filter /FlateDecode filter >> image";
        assert!(out.contains(invocation), "re-encode invocation: {out}");
        assert!(contains_bytes(&raw, &a85_flate(&RGB_SAMPLES)), "decoded samples re-encoded: {out:?}");
        assert!(!out.contains("%%BeginData"), "ASCII section needs no binary bracket: {out}");
        assert_eq!(notes, "", "no degradations");
    }

    /// A CCITT fax image passes through with its decode parameters mapped onto the
    /// PostScript filter's parameter dictionary.
    #[test]
    fn ccitt_passthrough_emits_params() {
        let pdf = image_pdf(
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 2 /Rows 2 >>",
            &CCITT_PAYLOAD,
            &[],
            b"q 20 0 0 20 10 10 cm /Im1 Do Q",
        );
        let (raw, notes) = driven(&pdf);
        let (out, notes) = output(&raw, &notes);
        let invocation = "<< /ImageType 1 /Width 2 /Height 2 /BitsPerComponent 1 /Decode [0 1] \
             /ImageMatrix [1 0 0 1 0 0] /DataSource currentfile << /K -1 /Columns 2 /Rows 2 >> /CCITTFaxDecode filter >> image";
        assert!(out.contains(invocation), "CCITT invocation with params: {out}");
        assert!(out.contains("0 setgray\n"), "1-component space: {out}");
        assert!(contains_bytes(&raw, &CCITT_PAYLOAD), "fax bytes verbatim: {out:?}");
        assert_eq!(notes, "", "no degradations");
    }

    /// An image whose declared dimensions exceed the 100-megapixel cap is skipped with a
    /// note and emits no operator (and never decodes).
    #[test]
    fn megapixel_cap_notes_and_skips() {
        let pdf = image_pdf(
            "/Type /XObject /Subtype /Image /Width 20000 /Height 20000 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode",
            &DCT_PAYLOAD,
            &[],
            b"q 20 0 0 20 10 10 cm /Im1 Do Q",
        );
        let (raw, notes) = driven(&pdf);
        let (out, notes) = output(&raw, &notes);
        assert_eq!(notes, "image skipped (exceeds 100 megapixel cap)", "the cap is recorded: {notes}");
        assert!(!out.contains(">> image") && !out.contains("imagemask"), "no image operator: {out}");
    }
}
