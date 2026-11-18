//! Finalize the cluster metadata after the shared Type3 outline registrar has run.

use pdfcraft_cos::{Dict, Document, Object, Stream};
use pdfcraft_fonts::TextPlan;

pub(super) fn register_cluster_maps(doc: &mut Document, plan: &TextPlan) -> Result<Dict, String> {
    let mut editable = Dict::new();
    let descriptor = if plan.fonts.iter().any(|font| font.family == "Inter") {
        let faces = pdfcraft_fonts::document_latin_fonts(false);
        let face = faces.first().ok_or("the editable OCR source face is unavailable")?;
        // Keep the complete, already-approved program once, as a real Type0 resource.
        // Type3 CharProcs remain the painted font; their descriptor contains no font file.
        let name = pdfcraft_fonts::embedding::embed(doc, &mut editable, face.bytes, face.family)?;
        let full = editable.get(name.as_bytes()).and_then(|font| doc.dict(font)).ok_or("the editable OCR source font is missing")?;
        let cid = full
            .get(b"DescendantFonts")
            .and_then(|fonts| doc.resolve(fonts).as_array().and_then(|a| a.first()).cloned())
            .and_then(|font| doc.dict(&font))
            .ok_or("the editable OCR source font has no descendant")?;
        let source = cid.get(b"FontDescriptor").and_then(|d| doc.dict(d)).ok_or("the editable OCR source descriptor is missing")?;
        let mut metadata = Dict::new();
        // Do not add ascent/descent/bbox: the existing Type3 envelope owns selection
        // geometry, including tall marks. FontFile entries belong only to Type0.
        for key in [b"Type".as_slice(), b"FontName", b"FontFamily", b"Flags", b"ItalicAngle", b"CapHeight"] {
            metadata.set(key.to_vec(), source.get(key).cloned().ok_or("the editable OCR source metadata is incomplete")?);
        }
        let name = metadata.get(b"FontName").cloned().ok_or("the editable OCR source face is unnamed")?;
        Some((doc.add(Object::Dict(metadata)), name))
    } else {
        None
    };
    for (fi, font) in plan.fonts.iter().enumerate() {
        let object = font.object.ok_or("the OCR font was not registered")?;
        if font.family == "Inter"
            && let Some((descriptor, name)) = &descriptor
        {
            doc.update_dict(object, |dict| {
                dict.set(b"Name".to_vec(), name.clone());
                dict.set(b"FontDescriptor".to_vec(), Object::Ref(*descriptor));
            })
            .map_err(|e| e.to_string())?;
        }
        // The shared scalar registrar rounds widths to 1/1000 em. Correct every
        // editable font together: otherwise NFC grouped Tj and NFD cluster Tj runs
        // have different advances despite identical ink and planned word fit.
        let widths = font.glyphs.iter().map(|(_, glyph)| Object::Real(glyph.width * 1000.0)).collect();
        doc.update_dict(object, |dict| dict.set(b"Widths".to_vec(), Object::Array(widths))).map_err(|e| e.to_string())?;
        if !(1..=font.glyphs.len()).any(|code| u8::try_from(code).ok().is_some_and(|code| plan.cluster_text(fi, code).is_some())) {
            continue;
        }
        let mut cmap = String::from(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (PdfCraft) /Ordering (Unicode) /Supplement 0 >> def\n/CMapName /PdfCraftOCRClusters def\n/CMapType 2 def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
        );
        for (batch, chunk) in font.glyphs.chunks(100).enumerate() {
            cmap.push_str(&format!("{} beginbfchar\n", chunk.len()));
            for (i, (label, _)) in chunk.iter().enumerate() {
                let code = u8::try_from(batch * 100 + i + 1).map_err(|_| "OCR font code exceeds its byte budget")?;
                let scalar = label.to_string();
                let source = plan.cluster_text(fi, code).unwrap_or(&scalar);
                let hex: String = source.encode_utf16().map(|u| format!("{u:04X}")).collect();
                cmap.push_str(&format!("<{code:02X}> <{hex}>\n"));
            }
            cmap.push_str("endbfchar\n");
        }
        cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        let map = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));
        // Expand the existing scalar envelope, never tighten it: tightening would
        // change extraction/selection boxes for canonically equivalent ordinary text.
        let mut bounds = [0.0f64, -0.3, 1.0, 1.0];
        for (_, glyph) in &font.glyphs {
            bounds[0] = bounds[0].min(glyph.bbox[0]);
            bounds[1] = bounds[1].min(glyph.bbox[1]);
            bounds[2] = bounds[2].max(glyph.bbox[2]).max(glyph.width);
            bounds[3] = bounds[3].max(glyph.bbox[3]);
        }
        // The shared scalar registrar uses a fixed box, which can clip stacked marks.
        let bbox = [(bounds[0] * 1000.0).floor(), (bounds[1] * 1000.0).floor(), (bounds[2] * 1000.0).ceil(), (bounds[3] * 1000.0).ceil()];
        doc.update_dict(object, |dict| {
            dict.set(b"ToUnicode".to_vec(), Object::Ref(map));
            dict.set(b"FontBBox".to_vec(), Object::Array(bbox.into_iter().map(Object::Real).collect()));
        })
        .map_err(|e| e.to_string())?;
    }
    Ok(editable)
}
