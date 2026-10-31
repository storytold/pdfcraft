use crate::font::blob::{CffFontBlob, OpenTypeFontBlob, Type1FontBlob};
use crate::font::generated::glyph_names;
use crate::font::standard_font::select_standard_font;
use crate::font::{
    FallbackFontQuery, FontData, FontFlags, FontQuery, read_to_unicode, stretch_glyph,
    strip_subset_prefix,
};
use crate::{CMapResolverFn, CacheKey, FontResolverFn, FurtherFacesFn};
use hayro_cmap::{BfString, CMap, CharacterCollection, CidFamily, WritingMode};
use hayro_syntax::object;
use hayro_syntax::object::Dict;
use hayro_syntax::object::Name;
use hayro_syntax::object::Stream;
use hayro_syntax::object::dict::keys::*;
use hayro_syntax::object::{Array, Object};
use kurbo::{BezPath, Vec2};
use skrifa::attribute::Style;
use skrifa::charmap::MapVariant;
use skrifa::raw::collections::int_set::Domain;
use skrifa::{FontRef, GlyphId, MetadataProvider};
use std::cell::OnceCell;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::ops::Deref;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct Type0Font {
    font_type: FontType,
    horizontal: bool,
    cache_key: u128,
    dw: f32,
    dw2: (f32, f32),
    widths: HashMap<u32, f32>,
    encoding: CMap,
    to_unicode: Option<CMap>,
    widths2: HashMap<u32, [f32; 3]>,
    cid_to_gid_map: CidToGIdMap,
    /// PostScript name from the PDF.
    postscript_name: Option<String>,
    /// Font flags from the font descriptor.
    font_flags: Option<FontFlags>,
    /// Whether this font is using a fallback (non-embedded) font.
    fallback: bool,
    /// Whether the `to_unicode` map is a UCS2 `CMap` (CID-indexed) rather than
    /// a `ToUnicode` `CMap` (code-indexed).
    to_unicode_is_cid_indexed: bool,
    /// PdfCraft patch: whether the CIDs belong to an Adobe character collection (Japan1, GB1,
    /// CNS1, Korea1, KR), whose glyph numbering no substitute other than a CID-keyed CFF font shares.
    adobe_collection: bool,
    /// PdfCraft patch: `/ToUnicode /Identity-H` (a name, not a CMap stream), which CAD exporters
    /// such as WSCAD write for fonts whose two-byte codes are UTF-16 code units. Acrobat shows
    /// such text as the code's Unicode value; the identity CMap the name resolves to maps codes
    /// to CIDs only and gave no text at all.
    identity_to_unicode: bool,
    /// PdfCraft patch (71): for a font drawn with a substitute, the faces that draw the characters
    /// the substitute has no glyph for (`InterpreterSettings::further_faces`), tried in order.
    further_faces: Vec<FurtherFace>,
}

impl Type0Font {
    /// Reads a Type 0 font from its dictionary: the descendant CID font, the encoding CMap and
    /// the code-to-Unicode map (`/ToUnicode`, the `/Identity-H` name, or the UCS2 CMap).
    pub(crate) fn new(
        dict: &Dict<'_>,
        font_resolver: &FontResolverFn,
        further_faces_resolver: &FurtherFacesFn,
        cmap_resolver: &CMapResolverFn,
    ) -> Option<Self> {
        let cmap = read_encoding(&dict.get::<Object<'_>>(ENCODING)?, cmap_resolver)?;

        let horizontal = cmap.metadata().writing_mode != Some(WritingMode::Vertical);

        let descendant_font = dict
            .get::<Array<'_>>(DESCENDANT_FONTS)?
            .iter::<Dict<'_>>()
            .next()?;
        let font_descriptor = descendant_font
            .get::<Dict<'_>>(FONT_DESC)
            .unwrap_or_default();

        let character_collection = cmap
            .metadata()
            .character_collection
            .clone()
            .filter(|cc| cc.family != CidFamily::AdobeIdentity)
            .or_else(|| read_cid_system_info(&descendant_font));

        // PdfCraft patch: the query the further faces are asked for, when there is a substitute.
        let mut further_query = None;
        let (font_type, fallback, _is_standard_fallback) = match FontType::new(&font_descriptor) {
            Some(ft) => (ft, false, false),
            None => {
                // PdfCraft patch: the style comes from the descendant's descriptor.
                let mut fallback_query =
                    FallbackFontQuery::with_descriptor(dict, Some(font_descriptor.clone()));
                fallback_query.character_collection = character_collection.clone();
                let (query, is_standard) = match select_standard_font(dict, &font_descriptor) {
                    // PdfCraft patch (72): a CJK font whose name only hints at a standard family
                    // (`MonotypeSung-Light` holds "mono") is asked for as a fallback carrying that
                    // hint, so a face for its script can stand in; a resolver without one picks
                    // the hinted font, as before.
                    Some((standard, false))
                        if is_adobe_cjk(character_collection.as_ref())
                            && !names_standard_family(&fallback_query) =>
                    {
                        fallback_query.standard_hint = Some(standard);
                        (FontQuery::Fallback(fallback_query.clone()), false)
                    }
                    Some((standard, _)) => (FontQuery::Standard(standard), true),
                    None => {
                        warn!(
                            "unable to load CID font {} ({:?}), attempting fallback",
                            fallback_query
                                .post_script_name
                                .as_deref()
                                .unwrap_or("(no name)"),
                            dict.obj_id()
                        );

                        (FontQuery::Fallback(fallback_query.clone()), false)
                    }
                };

                let (data, index) = font_resolver(&query)?;
                let substitute = (data.clone(), index);
                let blob = OpenTypeFontBlob::new(data.clone(), index)
                    .map(FontType::OpenType)
                    .or_else(|| CffFontBlob::new(data).map(FontType::Cff))?;
                further_query = Some((fallback_query, substitute));
                (blob, true, is_standard)
            }
        };
        let further_faces = further_query
            .map(|(query, (substitute, substitute_index))| {
                further_faces_resolver(&query)
                    .into_iter()
                    // The substitute itself is no further face: it was tried first.
                    .filter(|(data, index)| {
                        !(*index == substitute_index
                            && std::ptr::eq(data.as_ref().as_ref(), substitute.as_ref().as_ref()))
                    })
                    .take(MAX_FURTHER_FACES)
                    .map(|(data, index)| FurtherFace {
                        data,
                        index,
                        blob: OnceCell::new(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let default_width = descendant_font.get::<f32>(DW).unwrap_or(1000.0);
        let dw2 = descendant_font
            .get::<[f32; 2]>(DW2)
            .map(|v| (v[0], v[1]))
            .unwrap_or((880.0, -1000.0));

        let widths = descendant_font
            .get::<Array<'_>>(W)
            .and_then(|a| read_widths(&a))
            .unwrap_or_default();
        let widths2 = descendant_font
            .get::<Array<'_>>(W2)
            .and_then(|a| read_widths2(&a))
            .unwrap_or_default();
        let cid_to_gid_map = CidToGIdMap::new(&descendant_font).unwrap_or_default();
        let cache_key = dict.cache_key();

        let mut to_unicode = read_to_unicode(dict, cmap_resolver);
        let identity_to_unicode = dict
            .get::<Name<'_>>(TO_UNICODE)
            .is_some_and(|n| matches!(n.as_ref(), b"Identity-H" | b"Identity-V"));
        let mut to_unicode_is_cid_indexed = false;

        // If there is no ToUnicode map, try to get the UCS2 CMap.
        if fallback
            && to_unicode.is_none()
            && let Some(cc) = character_collection.as_ref()
            && let Some(ucs2_name) = cc.family.ucs2_cmap()
            && let Some(data) = (cmap_resolver)(ucs2_name)
        {
            let resolver = cmap_resolver.clone();
            if let Some(ucs2_cmap) = CMap::parse(data, move |n| (resolver)(n)) {
                to_unicode = Some(ucs2_cmap);
                to_unicode_is_cid_indexed = true;
            }
        }

        // Adobe-KR has no predefined UCS2 CMap, so it counts by registry and ordering.
        let adobe_collection = is_adobe_cjk(character_collection.as_ref());

        let postscript_name = dict
            .get::<Name<'_>>(BASE_FONT)
            .map(|n| strip_subset_prefix(n.as_str()).to_string());

        // Extract font flags from descriptor
        let font_flags = font_descriptor
            .get::<u32>(FLAGS)
            .and_then(FontFlags::from_bits);

        Some(Self {
            cache_key,
            horizontal,
            encoding: cmap,
            to_unicode,
            font_type,
            dw: default_width,
            dw2,
            widths,
            widths2,
            cid_to_gid_map,
            postscript_name,
            font_flags,
            fallback,
            to_unicode_is_cid_indexed,
            adobe_collection,
            identity_to_unicode,
            further_faces,
        })
    }

    pub(crate) fn map_code(&self, code: u32) -> GlyphId {
        let Some(cid) = self.code_to_cid(code) else {
            return GlyphId::NOTDEF;
        };

        // UCS2 maps are indexed by CIDs, while embededd `ToUnicode` maps are
        // indexed by character code.
        let to_unicode_key = if self.to_unicode_is_cid_indexed {
            cid
        } else {
            code
        };

        if self.fallback
            && let Some((character, selector)) = self.unicode_of(to_unicode_key)
        {
            let glyph = match &self.font_type {
                FontType::OpenType(t) => open_type_glyph(t, character, selector),
                FontType::Cff(c) => cff_glyph(c, character),
                // We could in theory map to a glyph via AFL and then look up
                // the glyph index in the encoding, but since skrifa doesn't have
                // an API for that and this is only really necessary for fallback
                // fonts (which currently only supports OpenType and CFF as fallback),
                // so we don't support this for now.
                FontType::Type1(_) => None,
            };
            if let Some(glyph) = glyph.filter(|g| *g != GlyphId::NOTDEF) {
                // Yay, Unicode worked!
                return self.substitute_glyph(glyph);
            }
            // PdfCraft patch (71): a character the substitute has no glyph for draws in the first
            // further face that has one.
            if let Some(glyph) = self.further_glyph(character, selector) {
                return glyph;
            }
            if let Some(glyph) = glyph {
                return glyph;
            }
        }

        // PdfCraft patch: a substitute that isn't CID-keyed numbers its glyphs its own way, so
        // the CID of an Adobe character collection taken as its glyph index draws an unrelated
        // glyph. Draw .notdef, as for a CID the font has no glyph for.
        if self.fallback
            && self.adobe_collection
            && !matches!(&self.font_type, FontType::Cff(c) if c.is_cid())
        {
            return GlyphId::NOTDEF;
        }

        // At this point, not much we can do anymore. Just hope that the
        // selected font has the right glyph order, and map via that.

        let glyph = match &self.font_type {
            FontType::OpenType(_) => self.cid_to_gid_map.map(cid as u16),
            FontType::Cff(c) => {
                if c.is_cid() {
                    // Very confusing stuff going on here, see https://github.com/mozilla/pdf.js/pull/15563.
                    // The PDF spec makes it sounds like cid-to-gid map should only be used for TrueType fonts,
                    // but Acrobat also seems to support it for CFF fonts with some weird behavior.
                    if matches!(self.cid_to_gid_map, CidToGIdMap::Identity) {
                        c.glyph_index_by_cid(cid as u16).unwrap_or(GlyphId::NOTDEF)
                    } else {
                        GlyphId::new(self.cid_to_gid_map.inverse_map(GlyphId::new(cid)) as u32)
                    }
                } else {
                    self.cid_to_gid_map.map(cid as u16)
                }
            }
            // Maybe we need similar processing to CFF fonts? But since
            // Type1 fonts are invalid anyway, let's just ignore for now.
            FontType::Type1(_) => GlyphId::new(cid),
        };
        self.substitute_glyph(glyph)
    }

    /// PdfCraft patch: `glyph` of the font's own program or substitute, or `.notdef` when its id
    /// would read as a further face's glyph (see [`FACE_SHIFT`]). Programs have at most 65,535
    /// glyphs, so only a broken one has such an id.
    fn substitute_glyph(&self, glyph: GlyphId) -> GlyphId {
        if !self.further_faces.is_empty() && glyph.to_u32() > u32::from(u16::MAX) {
            GlyphId::NOTDEF
        } else {
            glyph
        }
    }

    /// PdfCraft patch: the glyph of the first further face that has one for `character`
    /// (in the form `selector` asks for, when that face has it), tagged with the face's position.
    fn further_glyph(&self, character: char, selector: Option<char>) -> Option<GlyphId> {
        self.further_faces
            .iter()
            .enumerate()
            .find_map(|(position, face)| {
                let glyph = match face.blob()? {
                    FaceBlob::OpenType(t) => open_type_glyph(t, character, selector),
                    FaceBlob::Cff(c) => cff_glyph(c, character),
                }
                .filter(|g| *g != GlyphId::NOTDEF)?;
                tag_further_glyph(position, glyph)
            })
    }

    /// PdfCraft patch: the further face `glyph` belongs to and its id there, or `None` for a
    /// glyph of the font's own program or substitute.
    fn split_further_glyph(&self, glyph: GlyphId) -> Option<(&FurtherFace, GlyphId)> {
        let tag = glyph.to_u32() >> FACE_SHIFT;
        let position = usize::try_from(tag).ok()?.checked_sub(1)?;
        let face = self.further_faces.get(position)?;
        Some((face, GlyphId::new(glyph.to_u32() & u32::from(u16::MAX))))
    }

    /// PdfCraft patch: whether `glyph` is drawn by one of the further faces.
    pub(crate) fn is_further_glyph(&self, glyph: GlyphId) -> bool {
        self.split_further_glyph(glyph).is_some()
    }

    /// The character (and variation selector) a code (or CID) stands for, through the
    /// `ToUnicode` map or the collection's UCS2 `CMap`.
    fn unicode_of(&self, key: u32) -> Option<(char, Option<char>)> {
        let to_unicode = self.to_unicode.as_ref()?;

        to_unicode
            .lookup_bf_string(key)
            .or_else(|| {
                for len in 0..4 {
                    if let Some(code) = to_unicode.lookup_cid_code(key, len)
                        && let Some(code) = char::from_u32(code.to_u32())
                    {
                        return Some(BfString::Char(code));
                    }
                }

                None
            })
            .and_then(|bf| match bf {
                BfString::Char(c) => Some((c, None)),
                // PdfCraft patch: a character and a variation selector is that character in a
                // given form, as Adobe-Japan1 maps the JIS X 0208 forms of the kanji that
                // JIS X 0213:2004 redrew (噂 is U+5642 U+E0100). Other strings (ligatures) still
                // have no glyph.
                BfString::String(s) => {
                    let mut chars = s.chars();
                    let character = chars.next()?;
                    let selector = chars.next().filter(|c| is_variation_selector(*c))?;
                    chars.next().is_none().then_some((character, Some(selector)))
                }
            })
    }

    fn code_to_cid(&self, code: u32) -> Option<u32> {
        for byte_len in 1..=4_u8 {
            if let Some(cid) = self.encoding.lookup_cid_code(code, byte_len) {
                return Some(cid);
            }
        }

        None
    }

    pub(crate) fn outline_glyph(&self, glyph: GlyphId, code: u32) -> BezPath {
        // PdfCraft patch: a further face's glyph comes from that face, fitted to the advance
        // the PDF gives like a glyph of the substitute.
        if let Some((face, glyph)) = self.split_further_glyph(glyph) {
            return match face.blob() {
                Some(FaceBlob::OpenType(t)) => self.fit_to_width(
                    t.outline_glyph(glyph),
                    code,
                    t.glyph_metrics().advance_width(glyph),
                ),
                Some(FaceBlob::Cff(c)) => c.outline_glyph(glyph),
                None => BezPath::new(),
            };
        }

        match &self.font_type {
            FontType::OpenType(t) if self.fallback => self.fit_to_width(
                t.outline_glyph(glyph),
                code,
                t.glyph_metrics().advance_width(glyph),
            ),
            FontType::OpenType(t) => t.outline_glyph(glyph),
            FontType::Cff(c) => c.outline_glyph(glyph),
            FontType::Type1(t) => t.outline_glyph(glyph),
        }
    }

    /// A substitute's glyph `path` stretched from its own advance, `actual_width`, to the one
    /// the PDF gives the code.
    fn fit_to_width(&self, path: BezPath, code: u32, actual_width: Option<f32>) -> BezPath {
        if let Some(cid) = self.code_to_cid(code)
            && let Some(actual_width) = actual_width
            // Only use an expected width if there is an explicit /W array,
            // not the default width, as it leads to weird results from my testing.
            && let Some(expected_width) = self.widths.get(&cid).copied()
        {
            return stretch_glyph(path, expected_width, actual_width);
        }

        path
    }

    pub(crate) fn font_data(&self) -> Option<FontData> {
        match &self.font_type {
            FontType::OpenType(t) => Some(t.font_data()),
            FontType::Cff(c) => Some(c.font_data()),
            FontType::Type1(_) => None,
        }
    }

    /// Get the PostScript name.
    pub(crate) fn postscript_name(&self) -> Option<&str> {
        self.postscript_name.as_deref()
    }

    /// Get the font weight (100-900, 400=normal, 700=bold).
    ///
    /// Returns `None` if weight cannot be determined (CFF fonts or invalid weight).
    pub(crate) fn weight(&self) -> Option<u32> {
        match &self.font_type {
            FontType::OpenType(t) => {
                let weight = t.font_ref().attributes().weight.value().round() as u32;
                if weight > 0 { Some(weight) } else { None }
            }
            FontType::Cff(_) | FontType::Type1(_) => None,
        }
    }

    /// Check if font is italic based on font flags or font attributes.
    pub(crate) fn is_italic(&self) -> bool {
        if let Some(flags) = &self.font_flags
            && flags.contains(FontFlags::ITALIC)
        {
            return true;
        }
        match &self.font_type {
            FontType::OpenType(t) => t.font_ref().attributes().style != Style::Normal,
            FontType::Cff(_) | FontType::Type1(_) => false,
        }
    }

    /// Check if font is serif based on font flags.
    pub(crate) fn is_serif(&self) -> bool {
        self.font_flags
            .as_ref()
            .is_some_and(|f| f.contains(FontFlags::SERIF))
    }

    /// Check if font is monospace based on font flags or font metrics.
    pub(crate) fn is_monospace(&self) -> bool {
        if let Some(flags) = &self.font_flags
            && flags.contains(FontFlags::FIXED_PITCH)
        {
            return true;
        }
        match &self.font_type {
            FontType::OpenType(t) => {
                t.font_ref()
                    .metrics(
                        skrifa::instance::Size::unscaled(),
                        skrifa::instance::LocationRef::default(),
                    )
                    .is_monospace
            }
            FontType::Cff(_) | FontType::Type1(_) => false,
        }
    }

    pub(crate) fn code_advance(&self, code: u32) -> Vec2 {
        let cid = self.code_to_cid(code).unwrap_or(0);
        if self.horizontal {
            Vec2::new(self.horizontal_width(cid) as f64, 0.0)
        } else if let Some([w, _, _]) = self.widths2.get(&cid) {
            Vec2::new(0.0, *w as f64)
        } else {
            Vec2::new(0.0, self.dw2.1 as f64)
        }
    }

    fn horizontal_width(&self, cid: u32) -> f32 {
        self.widths.get(&cid).copied().unwrap_or(self.dw)
    }

    pub(crate) fn is_horizontal(&self) -> bool {
        self.horizontal
    }

    pub(crate) fn read_code(&self, bytes: &[u8], offset: usize) -> (u32, usize) {
        let mut code = 0_u32;
        let remaining = bytes.len() - offset;

        for n in 0..4.min(remaining) {
            code = (code << 8) | bytes[offset + n] as u32;

            if self.encoding.lookup_cid_code(code, (n + 1) as u8).is_some() {
                return (code, n + 1);
            }
        }

        (0, 1)
    }

    pub(crate) fn origin_displacement(&self, code: u32) -> Vec2 {
        let cid = self.code_to_cid(code).unwrap_or(0);

        if self.is_horizontal() {
            Vec2::default()
        } else if let Some([_, v1, v2]) = self.widths2.get(&cid) {
            Vec2::new(-*v1 as f64, -*v2 as f64)
        } else {
            Vec2::new(-self.horizontal_width(cid) as f64 / 2.0, -self.dw2.0 as f64)
        }
    }

    /// The text a character code stands for, or `None` when the font gives none (or, for an
    /// identity `/ToUnicode` name, when the code is a control or a lone surrogate).
    pub(crate) fn char_code_to_unicode(&self, code: u32) -> Option<BfString> {
        if self.identity_to_unicode {
            // Controls and lone surrogates are not text. The same rule builds the editing
            // map, `identity_to_unicode` in crates/fonts/src/pdf.rs: keep both alike.
            return char::from_u32(code)
                .filter(|c| !c.is_control())
                .map(BfString::Char);
        }
        if let Some(to_unicode) = &self.to_unicode {
            let key = if self.to_unicode_is_cid_indexed {
                self.code_to_cid(code).unwrap_or(0)
            } else {
                code
            };
            return to_unicode.lookup_bf_string(key);
        }

        None
    }
}

impl CacheKey for Type0Font {
    fn cache_key(&self) -> u128 {
        self.cache_key
    }
}

#[derive(Debug)]
enum FontType {
    /// An OpenType font.
    OpenType(OpenTypeFontBlob),
    /// A CFF font.
    Cff(CffFontBlob),
    /// A Type1 font. (Yes, there actually exist PDFs that embed a Type1 font in a
    /// CID font :))
    Type1(Type1FontBlob),
}

impl FontType {
    fn new(descriptor: &Dict<'_>) -> Option<Self> {
        // PDF has a distinction between CIDFontType0 and CIDFontType 2, with
        // specific requirements what font type can appear where. However, some
        // PDFs use wrong metadata, so we simply bruteforce without even looking
        // at the metadata.

        let data = descriptor
            .get::<Stream<'_>>(FONT_FILE2)
            .or_else(|| descriptor.get::<Stream<'_>>(FONT_FILE3))
            // See PDFBOX-2599. Apparently, Acrobat accepts this as well.
            .or_else(|| descriptor.get::<Stream<'_>>(FONT_FILE))?
            .decoded()
            .ok()?;

        let data = Arc::new(data.into_owned());

        let parsed = if let Ok(_font_ref) = FontRef::from_index(data.as_ref(), 0) {
            // It's an OpenType font, either TrueType or CFF.
            Self::OpenType(OpenTypeFontBlob::new(data, 0)?)
        } else if let Some(cff) = CffFontBlob::new(data.clone()) {
            // It's a CFF font.
            Self::Cff(cff)
        } else if let Some(t1) = Type1FontBlob::new(data) {
            // It's a Type1 (PFB) font.
            Self::Type1(t1)
        } else {
            return None;
        };

        Some(parsed)
    }
}

#[derive(Debug, Default)]
enum CidToGIdMap {
    #[default]
    Identity,
    Mapped {
        forward: HashMap<u16, GlyphId>,
        inverse: HashMap<GlyphId, u16>,
    },
}

impl CidToGIdMap {
    fn new(dict: &Dict<'_>) -> Option<Self> {
        if let Some(name) = dict.get::<Name<'_>>(CID_TO_GID_MAP) {
            if name.deref() == IDENTITY {
                Some(Self::Identity)
            } else {
                None
            }
        } else if let Some(stream) = dict.get::<Stream<'_>>(CID_TO_GID_MAP) {
            let decoded = stream.decoded().ok()?;
            let mut forward = HashMap::new();
            let mut inverse = HashMap::new();

            for (cid, gid) in decoded.chunks_exact(2).enumerate() {
                let gid = GlyphId::new(u16::from_be_bytes([gid[0], gid[1]]) as u32);

                forward.insert(cid as u16, gid);
                inverse.insert(gid, cid as u16);
            }

            Some(Self::Mapped { forward, inverse })
        } else {
            None
        }
    }

    fn map(&self, code: u16) -> GlyphId {
        match self {
            Self::Identity => GlyphId::new(code as u32),
            Self::Mapped { forward, .. } => forward.get(&code).copied().unwrap_or(GlyphId::NOTDEF),
        }
    }

    fn inverse_map(&self, gid: GlyphId) -> u16 {
        match self {
            Self::Identity => gid.to_u32() as u16,
            Self::Mapped { inverse, .. } => {
                inverse.get(&gid).copied().unwrap_or(gid.to_u32() as u16)
            }
        }
    }
}

/// PdfCraft patch (72): whether `collection` is one of Adobe's CJK character collections
/// (Japan1, GB1, CNS1, Korea1, KR).
fn is_adobe_cjk(collection: Option<&CharacterCollection>) -> bool {
    collection.is_some_and(|c| {
        c.family.ucs2_cmap().is_some()
            || matches!(&c.family, CidFamily::Custom { registry, ordering }
                if registry.as_slice() == b"Adobe" && ordering.as_slice() == b"KR")
    })
}

/// PdfCraft patch (72): whether the font's name names a standard family outright although
/// `select_standard_font` calls the match inexact: "Arial" is Helvetica and "Zapf"/"Dingbats"
/// ZapfDingbats, as PdfCraft's substitution chain (`Std14::recognise` in
/// `crates/fonts/src/std14.rs`) reads them too. Keep the two alike.
fn names_standard_family(query: &FallbackFontQuery) -> bool {
    query.post_script_name.as_deref().is_some_and(|name| {
        let name = name.to_ascii_lowercase();
        ["arial", "zapf", "dingbat"]
            .iter()
            .any(|family| name.contains(family))
    })
}

/// PdfCraft patch: a further face's glyph ids carry the face's position above the 16 bits a
/// glyph id of an OpenType or CFF program has: `(n + 1) << FACE_SHIFT | glyph` is `glyph` of the
/// n-th further face (counting from 0). The ids stay distinct per font, so glyph caches keyed by
/// font and glyph id keep them apart.
const FACE_SHIFT: u32 = 16;

/// PdfCraft patch: at most this many further faces per font; the rest are ignored.
const MAX_FURTHER_FACES: usize = 64;

/// PdfCraft patch: `glyph` of the further face at `position`, tagged (see [`FACE_SHIFT`]), or
/// `None` when the id doesn't fit in 16 bits or the position is past [`MAX_FURTHER_FACES`].
fn tag_further_glyph(position: usize, glyph: GlyphId) -> Option<GlyphId> {
    if position >= MAX_FURTHER_FACES {
        return None;
    }
    let glyph = u16::try_from(glyph.to_u32()).ok()?;
    let tag = u32::try_from(position).ok()?.checked_add(1)?;
    Some(GlyphId::new((tag << FACE_SHIFT) | u32::from(glyph)))
}

/// PdfCraft patch: a further face, parsed the first time a character needs it.
struct FurtherFace {
    data: FontData,
    index: u32,
    blob: OnceCell<Option<FaceBlob>>,
}

impl FurtherFace {
    /// The parsed face, or `None` when its data is neither an OpenType font nor a CFF program.
    fn blob(&self) -> Option<&FaceBlob> {
        self.blob
            .get_or_init(|| {
                OpenTypeFontBlob::new(self.data.clone(), self.index)
                    .map(FaceBlob::OpenType)
                    .or_else(|| CffFontBlob::new(self.data.clone()).map(FaceBlob::Cff))
            })
            .as_ref()
    }
}

impl Debug for FurtherFace {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "FurtherFace {{ index: {} }}", self.index)
    }
}

/// PdfCraft patch: the program of a further face.
enum FaceBlob {
    OpenType(OpenTypeFontBlob),
    Cff(CffFontBlob),
}

/// The glyph an OpenType substitute has for `character`, in the form `selector` asks for when it
/// has that form.
fn open_type_glyph(
    font: &OpenTypeFontBlob,
    character: char,
    selector: Option<char>,
) -> Option<GlyphId> {
    let charmap = font.font_ref().charmap();
    // PdfCraft patch: the face's glyph for that form when its `cmap` has one, else its
    // glyph for the character, as when a form isn't supported (Unicode FAQ on
    // variation sequences).
    if let Some(selector) = selector
        && let Some(MapVariant::Variant(glyph)) = charmap.map_variant(character, selector)
    {
        return Some(glyph);
    }
    // PdfCraft patch: a substitute may lack a spacing character (#3601; e.g.
    // Adobe-Japan1's U+2002). Draw its ordinary space instead of .notdef;
    // the PDF's widths and Unicode mapping still determine advance and text.
    charmap.map(character).or_else(|| match character {
        '\u{00a0}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => charmap.map(' '),
        _ => None,
    })
}

/// The glyph a CFF substitute has for `character`: the glyph its Adobe Glyph List name names.
fn cff_glyph(font: &CffFontBlob, character: char) -> Option<GlyphId> {
    // Map codepoint to glyph name via AFL, and then look it up.
    let name = glyph_names::get_reverse(character)?;
    font.glyph_index_by_name(name)
}

/// PdfCraft patch: Variation Selectors (U+FE00–U+FE0F) and their supplement (U+E0100–U+E01EF).
fn is_variation_selector(c: char) -> bool {
    matches!(c, '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
}

/// PdfCraft patch: the largest CID (ISO 32000-2 §9.7.2; CIDs are at most two bytes).
const MAX_CID: u32 = 65_535;

fn read_widths(arr: &Array<'_>) -> Option<HashMap<u32, f32>> {
    let mut map = HashMap::new();
    let mut iter = arr.flex_iter();

    loop {
        if let Some((mut first, range)) = iter.next::<(u32, Array<'_>)>() {
            for width in range.iter::<f32>() {
                if first > MAX_CID {
                    break;
                }
                map.insert(first, width);
                first = first.checked_add(1)?;
            }
        } else if let Some((first, second, width)) = iter.next::<(u32, u32, f32)>() {
            // PdfCraft patch: CIDs stop at 65535. A corrupt range such as
            // `0 4294967295 500` inserted billions of entries (found by fuzzing).
            for i in first..=second.min(MAX_CID) {
                map.insert(i, width);
            }
        } else {
            break;
        }
    }

    Some(map)
}

fn read_widths2(arr: &Array<'_>) -> Option<HashMap<u32, [f32; 3]>> {
    let mut map = HashMap::new();
    let mut iter = arr.flex_iter();

    loop {
        if let Some((mut first, range)) = iter.next::<(u32, Array<'_>)>() {
            let mut iter = range.iter::<f32>();

            while let Some(w) = iter.next() {
                // PdfCraft patch: as in `read_widths`, checked before the run's metrics are
                // read. Without it, a run starting at 4294967295 overflowed `first` and
                // discarded the whole /W2 table.
                if first > MAX_CID {
                    break;
                }
                let v1 = iter.next()?;
                let v2 = iter.next()?;
                map.insert(first, [w, v1, v2]);
                first = first.checked_add(1)?;
            }
        } else if let Some((first, second, w, v1, v2)) = iter.next::<(u32, u32, f32, f32, f32)>() {
            // PdfCraft patch: as in `read_widths`.
            for i in first..=second.min(MAX_CID) {
                map.insert(i, [w, v1, v2]);
            }
        } else {
            break;
        }
    }

    Some(map)
}

fn read_cid_system_info(descendant_font: &Dict<'_>) -> Option<CharacterCollection> {
    let info = descendant_font.get::<Dict<'_>>(CIDSYSTEMINFO)?;
    let registry = info.get::<object::String<'_>>(REGISTRY)?;
    let ordering = info.get::<object::String<'_>>(ORDERING)?;
    let supplement = info.get::<i32>(SUPPLEMENT).unwrap_or(0);
    let family = CidFamily::from_registry_ordering(registry.deref(), ordering.deref());

    Some(CharacterCollection { family, supplement })
}

fn read_encoding(object: &Object<'_>, cmap_resolver: &CMapResolverFn) -> Option<CMap> {
    // TODO: Support fetching CMaps referenced via `usecmap` in the PDF.
    match object {
        Object::Name(n) => {
            let cmap_type = hayro_cmap::CMapName::from_bytes(n.deref());
            match cmap_type {
                hayro_cmap::CMapName::IdentityH => Some(CMap::identity_h()),
                hayro_cmap::CMapName::IdentityV => Some(CMap::identity_v()),
                _ => {
                    let data = (cmap_resolver)(cmap_type)?;
                    let resolver = cmap_resolver.clone();
                    CMap::parse(data, move |n| (resolver)(n))
                }
            }
        }
        Object::Stream(s) => {
            let decoded = s.decoded().ok()?;
            let resolver = cmap_resolver.clone();
            CMap::parse(&decoded, move |n| (resolver)(n))
        }
        _ => None,
    }
}

/// PdfCraft patch test (ported from craft-libs): a /W2 run whose first CID is past `MAX_CID`
/// is skipped and the rest of the table kept.
#[cfg(test)]
mod tests {
    use super::read_widths2;
    use hayro_syntax::object::{Array, FromBytes};

    #[test]
    fn w2_run_past_the_largest_cid_keeps_the_rest_of_the_table() {
        let arr =
            Array::from_bytes(b"[4294967295 [1000 500] 10 [900 450 860] 20 30 800 400 870]")
                .unwrap();
        let map = read_widths2(&arr).unwrap();

        assert_eq!(map.get(&4_294_967_295), None);
        assert_eq!(map.get(&10), Some(&[900.0, 450.0, 860.0]));
        assert_eq!(map.get(&25), Some(&[800.0, 400.0, 870.0]));
        assert_eq!(map.len(), 12);
    }
}
