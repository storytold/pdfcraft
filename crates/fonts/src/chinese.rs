//! Explicit Chinese face for added page text. Only the approved craft-fonts build input is used.
use std::collections::{BTreeSet, HashMap};

use pdfcraft_cos::{Dict, Document};
use skrifa::{
    FontRef, MetadataProvider,
    instance::{LocationRef, Size},
};

pub fn face() -> Option<&'static crate::CraftFont> {
    crate::CRAFT_FONTS.iter().find(|f| f.family == "LXGW WenKai Lite" && f.style == "Regular")
}

pub fn has(c: char) -> bool {
    matches!(c, '\n' | '\r') || face().and_then(|f| FontRef::new(f.bytes).ok()).is_some_and(|f| f.charmap().map(c).is_some_and(|g| g.to_u32() != 0))
}

pub fn width(text: &str, size: f64) -> f64 {
    let Some(font) = face().and_then(|f| FontRef::new(f.bytes).ok()) else { return 0.0 };
    let metrics = font.metrics(Size::unscaled(), LocationRef::default());
    if metrics.units_per_em == 0 {
        return 0.0;
    }
    let glyphs = font.glyph_metrics(Size::unscaled(), LocationRef::default());
    text.chars()
        .filter_map(|c| font.charmap().map(c))
        .map(|g| f64::from(glyphs.advance_width(g).unwrap_or(0.0)) * size / f64::from(metrics.units_per_em))
        .sum()
}

/// Embed only needed character mappings, retaining the complete program for later edits.
pub fn embed(doc: &mut Document, fonts: &mut Dict, text: &str) -> Result<(String, HashMap<char, u16>), String> {
    if text.chars().take(1_000_001).count() > 1_000_000 {
        return Err("Chinese text exceeds the font mapping budget".into());
    }
    let face = face().ok_or("Chinese text needs the LXGW WenKai Lite face in this build")?;
    crate::embedding::validate_document_input(face.bytes)?;
    let font = FontRef::new(face.bytes).map_err(|e| e.to_string())?;
    let chars: BTreeSet<_> = text.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
    if chars.len() > 65_534 {
        return Err("Chinese text exceeds the font mapping budget".into());
    }
    let mut mapping = Vec::with_capacity(chars.len());
    let mut codes = HashMap::new();
    for (i, c) in chars.into_iter().enumerate() {
        let glyph = font.charmap().map(c).filter(|g| g.to_u32() != 0).ok_or_else(|| format!("the Chinese font cannot show {c:?}"))?;
        let code = u16::try_from(i.saturating_add(1)).map_err(|_| "Chinese character mapping exceeds its budget")?;
        mapping.push((c.to_string(), glyph));
        codes.insert(c, code);
    }
    let resource = crate::embedding::embed_mapping(doc, fonts, face.bytes, "LXGWWenKaiLite-Regular", &mapping)?;
    Ok((resource, codes))
}
