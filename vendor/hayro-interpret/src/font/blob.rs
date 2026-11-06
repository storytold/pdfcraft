use crate::font::UNITS_PER_EM;
use crate::font::outline::OutlinePath;
use kurbo::BezPath;
use skrifa::instance::{LocationRef, Size};
use skrifa::metrics::GlyphMetrics;
use skrifa::outline::{DrawSettings, Engine, HintingInstance, HintingOptions, Target};
use skrifa::raw::TableProvider;
use skrifa::raw::ps::cff::{CffFontRef, Subfont, charset::Charset, v1::Cff};
use skrifa::raw::ps::string::Sid;
use skrifa::raw::ps::type1::Type1Font;
use skrifa::raw::tables::post::DEFAULT_GLYPH_NAMES;
use skrifa::raw::{FontData as ReadFontData, FontRead};
use skrifa::{FontRef, GlyphId, MetadataProvider, OutlineGlyphCollection};
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, OnceLock};
use yoke::{Yoke, Yokeable};

type FontData = Arc<dyn AsRef<[u8]> + Send + Sync>;
type OpenTypeFontYoke = Yoke<OTFYoke<'static>, FontData>;
type CffFontYoke = Yoke<CFFYoke<'static>, FontData>;

/// A font blob for type 1 fonts.
#[derive(Clone)]
pub(crate) struct Type1FontBlob(Arc<Type1Font>);

impl Debug for Type1FontBlob {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Type1 Font {{ .. }}")
    }
}

impl Type1FontBlob {
    pub(crate) fn new(data: FontData) -> Option<Self> {
        let table = Type1Font::new(data.as_ref().as_ref()).ok()?;
        Some(Self(Arc::new(table)))
    }

    pub(crate) fn table(&self) -> &Type1Font {
        self.0.as_ref()
    }

    pub(crate) fn outline_glyph(&self, gid: GlyphId) -> BezPath {
        let mut path = OutlinePath::new();
        let _ = self.table().draw(gid, Some(UNITS_PER_EM), &mut path);

        path.take()
    }
}

/// A font blob for CFF-based fonts.
#[derive(Clone)]
pub(crate) struct CffFontBlob(Arc<CffFontYoke>, Arc<OnceLock<GlyphNames>>);

/// PdfCraft patch: (see vendor/README.md hayro-interpret (50)) glyph name to glyph id, built on
/// first use.
type GlyphNames = HashMap<Box<[u8]>, GlyphId>;

impl Debug for CffFontBlob {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Type1 Font {{ .. }}")
    }
}

impl CffFontBlob {
    pub(crate) fn new(data: FontData) -> Option<Self> {
        let font = CffFontRef::new(data.as_ref().as_ref(), 0, None).ok()?;
        let cff = Cff::read(ReadFontData::new(data.as_ref().as_ref())).ok()?;
        let charset = font.charset();
        let subfonts = (0..font.num_subfonts())
            .map(|index| font.subfont(index, &[]).ok())
            .collect::<Option<Vec<_>>>()?;

        let yoke = Yoke::<CFFYoke<'static>, FontData>::attach_to_cart(data.clone(), |data| {
            let bytes = data.as_ref();
            let font = CffFontRef::new(bytes, 0, None).unwrap();
            let cff = Cff::read(ReadFontData::new(bytes)).unwrap();
            let charset = font.charset();
            let subfonts = (0..font.num_subfonts())
                .map(|index| font.subfont(index, &[]).unwrap())
                .collect();
            CFFYoke {
                font,
                cff,
                charset,
                subfonts,
            }
        });

        let _ = (cff, charset, subfonts);
        Some(Self(Arc::new(yoke), Arc::new(OnceLock::new())))
    }

    pub(crate) fn font_data(&self) -> FontData {
        self.0.backing_cart().clone()
    }

    pub(crate) fn font(&self) -> &CffFontRef<'_> {
        &self.0.as_ref().get().font
    }

    fn charset(&self) -> Option<&Charset<'_>> {
        self.0.as_ref().get().charset.as_ref()
    }

    fn subfont(&self, glyph: GlyphId) -> Option<&Subfont> {
        let index = self.font().subfont_index(glyph)? as usize;
        self.0.as_ref().get().subfonts.get(index)
    }

    pub(crate) fn outline_glyph(&self, glyph: GlyphId) -> BezPath {
        let mut path = OutlinePath::new();
        let Some(subfont) = self.subfont(glyph) else {
            return BezPath::new();
        };

        let _ = self
            .font()
            .draw(subfont, glyph, &[], Some(UNITS_PER_EM), &mut path);

        path.take()
    }

    pub(crate) fn glyph_names(&self) -> Vec<(GlyphId, String)> {
        let Some(charset) = self.charset() else {
            return Vec::new();
        };

        // TODO: Avoid collecting here.
        charset
            .iter()
            .filter_map(|(gid, sid)| {
                let bytes = self.0.as_ref().get().cff.string(sid)?;
                let name = std::str::from_utf8(bytes).ok()?.to_string();
                Some((gid, name))
            })
            .collect()
    }

    pub(crate) fn glyph_index_by_name(&self, name: &str) -> Option<GlyphId> {
        // Called for every character code of a font, so the charset is read once. The first
        // glyph of a name wins, as it did when the charset was searched in order.
        self.1
            .get_or_init(|| {
                let mut names = GlyphNames::new();
                if let Some(charset) = self.charset() {
                    for (gid, sid) in charset.iter() {
                        if let Some(bytes) = self.0.as_ref().get().cff.string(sid) {
                            names.entry(bytes.into()).or_insert(gid);
                        }
                    }
                }
                names
            })
            .get(name.as_bytes())
            .copied()
    }

    pub(crate) fn glyph_index_by_cid(&self, cid: u16) -> Option<GlyphId> {
        self.charset()?.glyph_id(Sid::new(cid))
    }

    pub(crate) fn glyph_index(&self, code: u8) -> Option<GlyphId> {
        self.font()
            .encoding()
            .and_then(|encoding| encoding.map(code))
    }

    pub(crate) fn num_glyphs(&self) -> u32 {
        self.font().num_glyphs()
    }

    /// PdfCraft patch (69): the `CFF ` table of an OpenType font, for a `FontFile3` of
    /// subtype `OpenType` (ISO 32000-2 §9.9) under a simple font.
    pub(crate) fn from_open_type(data: &[u8]) -> Option<Self> {
        let cff = FontRef::new(data).ok()?.cff().ok()?;
        Self::new(Arc::new(cff.offset_data().as_ref().to_vec()))
    }

    pub(crate) fn is_cid(&self) -> bool {
        self.font().is_cid()
    }
}

/// A font blob for OpenType fonts.
#[derive(Clone)]
pub(crate) struct OpenTypeFontBlob {
    yoke: Arc<OpenTypeFontYoke>,
    cff_blob: Option<CffFontBlob>,
}

impl Debug for OpenTypeFontBlob {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "OpenType Font {{ .. }}")
    }
}

impl OpenTypeFontBlob {
    pub(crate) fn new(data: FontData, index: u32) -> Option<Self> {
        // PdfCraft patch (76): a subset whose `hmtx` kept the full font's length is read through a
        // copy with that length corrected.
        let font = FontRef::from_index(data.as_ref().as_ref(), index).ok()?;
        let data = repaired_horizontal_metrics(&font).unwrap_or(data);
        // Check first whether the font is valid so we can unwrap in the closure.
        let f = FontRef::from_index(data.as_ref().as_ref(), index).ok()?;
        // Reject fonts with invalid post table version, fixes pdf.js issue 9462. Not sure if there
        // is a better fix, for some reason skrifa accepts the font which is completely invalid.
        // PdfCraft patch: also accept 1.0, 2.0 and 3.0 with the two 16-bit halves swapped
        // (`00 00 00 03` for 3.0), as ScanSoft PDF Create! writes it into every font it embeds
        // (#2946). Rejecting those fonts drew their text with a substitute that has no such glyphs.
        let invalid = f.post().is_ok_and(|p| {
            !matches!(
                p.version().to_major_minor(),
                (1, 0) | (2, 0) | (2, 5) | (3, 0)
            ) && !matches!(p.version().to_be_bytes(), [0, 0, 0, 1..=3])
        });

        if invalid {
            return None;
        }

        // We store this separately because we want to be able to cache the subfonts
        // of a CFF OpenType font, which is not possible using the current skrifa API.
        // Hopefully there will be some way in the future.
        let cff_blob = f
            .cff()
            .ok()
            .map(|cff| Arc::new(cff.offset_data().as_ref().to_vec()) as FontData)
            .and_then(CffFontBlob::new);

        let font_ref_yoke =
            Yoke::<OTFYoke<'static>, FontData>::attach_to_cart(data.clone(), |data| {
                let font_ref = FontRef::from_index(data.as_ref(), index).unwrap();

                let hinting_instance = if font_ref.outline_glyphs().require_interpreter() {
                    HintingInstance::new(
                        &font_ref.outline_glyphs(),
                        Size::new(UNITS_PER_EM),
                        LocationRef::default(),
                        HintingOptions {
                            engine: Engine::Interpreter,
                            target: Target::Mono,
                        },
                    )
                    .ok()
                } else {
                    None
                };

                OTFYoke {
                    font_ref: font_ref.clone(),
                    outline_glyphs: font_ref.outline_glyphs(),
                    hinting_instance,
                    glyph_metrics: font_ref
                        .glyph_metrics(Size::new(UNITS_PER_EM), LocationRef::default()),
                }
            });

        Some(Self {
            yoke: Arc::new(font_ref_yoke),
            cff_blob,
        })
    }

    pub(crate) fn font_data(&self) -> FontData {
        self.yoke.backing_cart().clone()
    }

    pub(crate) fn font_ref(&self) -> &FontRef<'_> {
        &self.yoke.as_ref().get().font_ref
    }

    pub(crate) fn glyph_metrics(&self) -> &GlyphMetrics<'_> {
        &self.yoke.as_ref().get().glyph_metrics
    }

    pub(crate) fn glyph_names(&self) -> HashMap<String, GlyphId> {
        // Note: We don't call the `glyph_name` method provided by read-fonts because
        // calling it repeatedly is very slow.
        let mut glyph_names = HashMap::new();
        let Ok(post) = self.font_ref().post() else {
            return glyph_names;
        };

        // PdfCraft patch: a version written with its halves swapped (`00 00 00 02`, accepted in
        // `new`) is read as the version it means, from a copy of the table with the version
        // written correctly: read-fonts reads a 2.0 table's name index only under version 2.0,
        // so otherwise its glyph names were never found (#2946).
        let repaired = match post.version().to_be_bytes() {
            [0, 0, 0, n @ 1..=3] => self
                .font_ref()
                .table_data(skrifa::raw::types::Tag::new(b"post"))
                .map(|data| {
                    let mut bytes = data.as_bytes().to_vec();
                    if let Some(version) = bytes.get_mut(..4) {
                        version.copy_from_slice(&[0, n, 0, 0]);
                    }
                    bytes
                }),
            _ => None,
        };
        let repaired_post = repaired.as_deref().and_then(|b| {
            skrifa::raw::tables::post::Post::read(ReadFontData::new(b)).ok()
        });
        let post = repaired_post.as_ref().unwrap_or(&post);
        match post.version().to_major_minor() {
            (1, 0) => {
                for (gid, name) in DEFAULT_GLYPH_NAMES.iter().enumerate() {
                    glyph_names.insert((*name).to_string(), GlyphId::new(gid as u32));
                }
            }
            (2, 0) => {
                let Some(name_indices) = post.glyph_name_index() else {
                    return glyph_names;
                };
                let Some(string_data) = post.string_data() else {
                    return glyph_names;
                };
                let custom_names = string_data
                    .iter()
                    .map(|entry| entry.ok().map(|name| name.as_str()))
                    .collect::<Vec<_>>();

                for (gid, idx) in name_indices.iter().enumerate() {
                    let idx = idx.get() as usize;
                    if let Some(name) = DEFAULT_GLYPH_NAMES.get(idx).copied().or_else(|| {
                        custom_names
                            .get(idx.saturating_sub(DEFAULT_GLYPH_NAMES.len()))
                            .copied()
                            .flatten()
                    }) {
                        glyph_names.insert(name.to_string(), GlyphId::new(gid as u32));
                    }
                }
            }
            _ => {}
        }

        glyph_names
    }

    fn outline_glyphs(&self) -> &OutlineGlyphCollection<'_> {
        &self.yoke.as_ref().get().outline_glyphs
    }

    pub(crate) fn outline_glyph(&self, glyph: GlyphId) -> BezPath {
        if let Some(cff_blob) = self.cff_blob.as_ref() {
            return cff_blob.outline_glyph(glyph);
        }

        let mut path = OutlinePath::new();

        let draw_settings = if let Some(instance) = self.yoke.get().hinting_instance.as_ref() {
            // Note: We always hint at the font size `UNITS_PER_EM`, which obviously isn't very useful. We don't do this
            // for better text quality (right now), but instead because there are some PDFs with obscure fonts that
            // actually render wrongly if hinting is disabled!
            DrawSettings::hinted(instance, false)
        } else {
            DrawSettings::unhinted(Size::new(UNITS_PER_EM), LocationRef::default())
        };

        let Some(outline) = self.outline_glyphs().get(glyph) else {
            return BezPath::new();
        };

        let _ = outline.draw(draw_settings, &mut path);
        path.take()
    }
}

/// PdfCraft patch (76): a TrueType subset whose `hmtx` entry in the table directory kept the
/// length the table had in the full font runs past the end of the font, so skrifa finds no
/// `hmtx` and, needing its advances for the phantom points, builds no outline for any glyph.
/// OpenType's `hmtx` is four bytes for each of `hhea`'s `numberOfHMetrics` and two for each
/// other glyph `maxp` counts. When the recorded length is longer and those bytes lie after the
/// table directory and belong to no other table, this returns a copy of the font with only that
/// length corrected. `None` keeps the font as it is: its `hmtx` reads, it has no `glyf`, the
/// counts are invalid or don't fit, or it is over 64 MiB.
fn repaired_horizontal_metrics(font: &FontRef<'_>) -> Option<FontData> {
    let bytes = font.data().as_bytes();
    if bytes.len() > 64 * 1024 * 1024 || font.hmtx().is_ok() || font.glyf().is_err() {
        return None;
    }
    let metric_count = usize::from(font.hhea().ok()?.number_of_h_metrics());
    let glyph_count = usize::from(font.maxp().ok()?.num_glyphs());
    if metric_count == 0 {
        return None;
    }
    let trailing_count = glyph_count.checked_sub(metric_count)?;
    let length = metric_count
        .checked_mul(4)?
        .checked_add(trailing_count.checked_mul(2)?)?;

    let records = font.table_directory().table_records();
    let tag = skrifa::raw::types::Tag::new(b"hmtx");
    let record_index = records.iter().position(|record| record.tag() == tag)?;
    let record = records.get(record_index)?;
    if usize::try_from(record.length()).ok()? <= length {
        return None;
    }
    let offset = usize::try_from(record.offset()).ok()?;
    let end = offset.checked_add(length)?;
    bytes.get(offset..end)?;
    for (index, other) in records.iter().enumerate() {
        let start = usize::try_from(other.offset()).ok()?;
        let other_end = start.saturating_add(usize::try_from(other.length()).ok()?);
        if index != record_index && offset.max(start) < end.min(other_end) {
            return None;
        }
    }

    let directory_offset = bytes
        .len()
        .checked_sub(font.table_directory().offset_data().as_bytes().len())?;
    let directory_end = directory_offset
        .checked_add(12)?
        .checked_add(records.len().checked_mul(16)?)?;
    if offset < directory_end {
        return None;
    }
    let length_offset = directory_offset
        .checked_add(12)?
        .checked_add(record_index.checked_mul(16)?)?
        .checked_add(12)?;
    let length_end = length_offset.checked_add(4)?;
    bytes.get(length_offset..length_end)?;

    let mut repaired = Vec::new();
    repaired.try_reserve_exact(bytes.len()).ok()?;
    repaired.extend_from_slice(bytes);
    repaired
        .get_mut(length_offset..length_end)?
        .copy_from_slice(&u32::try_from(length).ok()?.to_be_bytes());
    warn!(
        "repaired overlong hmtx table length from {} to {} for rendering",
        record.length(),
        length
    );
    Some(Arc::new(repaired))
}

#[derive(Yokeable, Clone)]
struct OTFYoke<'a> {
    font_ref: FontRef<'a>,
    glyph_metrics: GlyphMetrics<'a>,
    hinting_instance: Option<HintingInstance>,
    outline_glyphs: OutlineGlyphCollection<'a>,
}

#[derive(Yokeable, Clone)]
struct CFFYoke<'a> {
    font: CffFontRef<'a>,
    cff: Cff<'a>,
    charset: Option<Charset<'a>>,
    subfonts: Vec<Subfont>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CFF font with glyphs `.notdef`, `Alpha`, `Alpha` (a duplicate name) and `Beta`.
    fn cff_with_names() -> Vec<u8> {
        fn index(items: &[&[u8]]) -> Vec<u8> {
            let mut out = (items.len() as u16).to_be_bytes().to_vec();
            if items.is_empty() {
                return out;
            }
            out.push(1);
            let mut offset = 1u8;
            out.push(offset);
            for item in items {
                offset += item.len() as u8;
                out.push(offset);
            }
            for item in items {
                out.extend_from_slice(item);
            }
            out
        }
        // Operands use the 5-byte form so the Top DICT has a fixed size.
        fn int(value: usize) -> Vec<u8> {
            let mut out = vec![29];
            out.extend_from_slice(&(value as u32).to_be_bytes());
            out
        }
        fn build_top(charset: usize, char_strings: usize, private: usize) -> Vec<u8> {
            let mut dict = Vec::new();
            dict.extend(int(charset));
            dict.push(15);
            dict.extend(int(char_strings));
            dict.push(17);
            dict.extend(int(0));
            dict.extend(int(private));
            dict.push(18);
            dict
        }
        let header = vec![1, 0, 4, 1];
        let names = index(&[b"T"]);
        let strings = index(&[b"Alpha", b"Beta"]);
        let subrs = index(&[]);
        // Standard strings take SIDs 0..=390, so the first custom string is 391.
        let charset = vec![0, 0x01, 0x87, 0x01, 0x87, 0x01, 0x88];
        let char_strings = index(&[&[14], &[14], &[14], &[14]]);
        // The Top DICT has a fixed size because every operand takes the 5-byte form.
        let top_len = index(&[&build_top(0, 0, 0)]).len();
        let charset_at = header.len() + names.len() + top_len + strings.len() + subrs.len();
        let char_strings_at = charset_at + charset.len();
        let private_at = char_strings_at + char_strings.len();
        let dict = build_top(charset_at, char_strings_at, private_at);
        let top = index(&[&dict]);
        let mut out = header;
        for part in [names, top, strings, subrs, charset, char_strings] {
            out.extend(part);
        }
        out
    }

    /// The charset search `glyph_index_by_name` replaced.
    fn linear(blob: &CffFontBlob, name: &str) -> Option<GlyphId> {
        blob.charset()?.iter().find_map(|(gid, sid)| {
            let bytes = blob.0.as_ref().get().cff.string(sid)?;
            (bytes == name.as_bytes()).then_some(gid)
        })
    }

    #[test]
    fn glyph_names_resolve_to_the_first_glyph_with_that_name() {
        let data: FontData = Arc::new(cff_with_names());
        let blob = CffFontBlob::new(data).expect("the test font parses");
        assert!(blob.1.get().is_none(), "the name map is built on first use");
        assert_eq!(blob.glyph_index_by_name("Alpha"), Some(GlyphId::new(1)));
        assert_eq!(blob.glyph_index_by_name("Beta"), Some(GlyphId::new(3)));
        assert_eq!(blob.glyph_index_by_name("Missing"), None);
        assert_eq!(blob.glyph_index_by_name(""), None);
        for name in ["Alpha", "Beta", "Missing", ".notdef", ""] {
            assert_eq!(
                blob.glyph_index_by_name(name),
                linear(&blob, name),
                "{name:?}"
            );
            // The second lookup is answered from the stored map.
            assert_eq!(
                blob.glyph_index_by_name(name),
                linear(&blob, name),
                "{name:?}"
            );
        }
        assert!(blob.1.get().is_some(), "the lookups built the name map");
        let shared = blob.clone();
        assert!(Arc::ptr_eq(&blob.1, &shared.1), "clones share one name map");
        assert_eq!(shared.glyph_index_by_name("Alpha"), Some(GlyphId::new(1)));
    }

    fn true_type_with_horizontal_metrics() -> (Vec<u8>, usize) {
        let mut head = vec![0; 54];
        head[..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        head[12..16].copy_from_slice(&0x5f0f_3cf5_u32.to_be_bytes());
        head[18..20].copy_from_slice(&1000_u16.to_be_bytes());
        head[40..42].copy_from_slice(&500_i16.to_be_bytes());
        head[42..44].copy_from_slice(&600_i16.to_be_bytes());
        head[50..52].copy_from_slice(&1_i16.to_be_bytes());

        let mut hhea = vec![0; 36];
        hhea[..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        hhea[4..6].copy_from_slice(&600_i16.to_be_bytes());
        hhea[34..36].copy_from_slice(&2_u16.to_be_bytes());

        let mut maxp = vec![0; 32];
        maxp[..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        maxp[4..6].copy_from_slice(&2_u16.to_be_bytes());
        maxp[6..8].copy_from_slice(&4_u16.to_be_bytes());
        maxp[8..10].copy_from_slice(&1_u16.to_be_bytes());
        maxp[14..16].copy_from_slice(&2_u16.to_be_bytes());

        let mut glyph = Vec::new();
        for value in [1_i16, 0, 0, 500, 600, 3, 0] {
            glyph.extend(value.to_be_bytes());
        }
        glyph.extend([1_u8; 4]);
        for value in [0_i16, 500, 0, -500, 0, 0, 600, 0] {
            glyph.extend(value.to_be_bytes());
        }
        let mut loca = Vec::new();
        for offset in [0_u32, 0, glyph.len() as u32] {
            loca.extend(offset.to_be_bytes());
        }
        let mut hmtx = Vec::new();
        for value in [500_u16, 0, 500, 0] {
            hmtx.extend(value.to_be_bytes());
        }

        let tables = [
            (*b"glyf", glyph),
            (*b"head", head),
            (*b"hhea", hhea),
            (*b"hmtx", hmtx),
            (*b"loca", loca),
            (*b"maxp", maxp),
        ];
        let mut font = vec![0; 12 + tables.len() * 16];
        font[..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        font[4..6].copy_from_slice(&(tables.len() as u16).to_be_bytes());
        font[6..8].copy_from_slice(&64_u16.to_be_bytes());
        font[8..10].copy_from_slice(&2_u16.to_be_bytes());
        font[10..12].copy_from_slice(&32_u16.to_be_bytes());
        let mut hmtx_record = 0;
        for (table_index, (tag, table)) in tables.into_iter().enumerate() {
            while !font.len().is_multiple_of(4) {
                font.push(0);
            }
            let record = 12 + table_index * 16;
            let offset = font.len() as u32;
            font[record..record + 4].copy_from_slice(&tag);
            font[record + 8..record + 12].copy_from_slice(&offset.to_be_bytes());
            font[record + 12..record + 16].copy_from_slice(&(table.len() as u32).to_be_bytes());
            if tag == *b"hmtx" {
                hmtx_record = record;
            }
            font.extend(table);
        }
        (font, hmtx_record)
    }

    #[test]
    fn overlong_subset_hmtx_retains_glyph_outlines() {
        let (mut font, hmtx_record) = true_type_with_horizontal_metrics();
        let valid =
            OpenTypeFontBlob::new(Arc::new(font.clone()), 0).expect("the synthetic font parses");
        assert!(!valid.outline_glyph(GlyphId::new(1)).elements().is_empty());

        let stale_length = font.len() as u32 + 4096;
        font[hmtx_record + 12..hmtx_record + 16].copy_from_slice(&stale_length.to_be_bytes());
        let subset = OpenTypeFontBlob::new(Arc::new(font), 0)
            .expect("a stale table length must not hide the font");
        assert!(
            !subset.outline_glyph(GlyphId::new(1)).elements().is_empty(),
            "the subset glyph must remain visible when hmtx retains the full font's length"
        );
    }

    #[test]
    fn valid_horizontal_metrics_reuse_the_original_font_bytes() {
        let (font, _) = true_type_with_horizontal_metrics();
        let data: FontData = Arc::new(font);
        let blob = OpenTypeFontBlob::new(data.clone(), 0).expect("the test font parses");
        assert!(Arc::ptr_eq(&data, &blob.font_data()));
    }

    #[test]
    fn horizontal_metric_repair_changes_only_the_rendering_copy() {
        let (mut font, hmtx_record) = true_type_with_horizontal_metrics();
        let stale_length = font.len() as u32 + 4096;
        font[hmtx_record + 12..hmtx_record + 16].copy_from_slice(&stale_length.to_be_bytes());
        let original_bytes = font.clone();
        let data: FontData = Arc::new(font);
        let blob = OpenTypeFontBlob::new(data.clone(), 0).expect("the subset font parses");
        let mut expected = original_bytes.clone();
        expected[hmtx_record + 12..hmtx_record + 16].copy_from_slice(&8_u32.to_be_bytes());
        assert_eq!(data.as_ref().as_ref(), original_bytes.as_slice());
        assert_eq!(blob.font_data().as_ref().as_ref(), expected.as_slice());
        assert!(!Arc::ptr_eq(&data, &blob.font_data()));
    }

    #[test]
    fn horizontal_metric_repair_counts_trailing_left_side_bearings() {
        let (mut font, hmtx_record) = true_type_with_horizontal_metrics();
        let reference = FontRef::new(&font).expect("the test font parses");
        let offset_of = |tag: &[u8; 4]| {
            reference
                .table_directory()
                .table_records()
                .iter()
                .find(|record| record.tag() == skrifa::raw::types::Tag::new(tag))
                .expect("the test font has the table")
                .offset() as usize
        };
        let (hhea_offset, hmtx_offset) = (offset_of(b"hhea"), offset_of(b"hmtx"));
        // One long metric for both glyphs: glyph 1 has only its left side bearing.
        font[hhea_offset + 34..hhea_offset + 36].copy_from_slice(&1_u16.to_be_bytes());
        font[hmtx_offset + 4..hmtx_offset + 6].copy_from_slice(&0_i16.to_be_bytes());
        let stale_length = font.len() as u32 + 4096;
        font[hmtx_record + 12..hmtx_record + 16].copy_from_slice(&stale_length.to_be_bytes());
        let blob = OpenTypeFontBlob::new(Arc::new(font), 0).expect("the subset font parses");
        let repaired = blob.font_data();
        assert_eq!(
            repaired.as_ref().as_ref()[hmtx_record + 12..hmtx_record + 16],
            6_u32.to_be_bytes()
        );
        assert!(!blob.outline_glyph(GlyphId::new(1)).elements().is_empty());
    }

    #[test]
    fn horizontal_metric_repair_refuses_invalid_counts_and_overlapping_tables() {
        let (font, hmtx_record) = true_type_with_horizontal_metrics();
        let reference = FontRef::new(&font).expect("the test font parses");
        let records = reference.table_directory().table_records();
        let hhea_offset = records
            .iter()
            .find(|record| record.tag() == skrifa::raw::types::Tag::new(b"hhea"))
            .expect("the test font has hhea")
            .offset() as usize;
        let hhea_record = 12
            + records
                .iter()
                .position(|record| record.tag() == skrifa::raw::types::Tag::new(b"hhea"))
                .expect("the test font has hhea")
                * 16;
        let loca_record = 12
            + records
                .iter()
                .position(|record| record.tag() == skrifa::raw::types::Tag::new(b"loca"))
                .expect("the test font has loca")
                * 16;
        let hmtx_offset = records
            .iter()
            .find(|record| record.tag() == skrifa::raw::types::Tag::new(b"hmtx"))
            .expect("the test font has hmtx")
            .offset();
        for case in 0..5 {
            let mut malformed = font.clone();
            let stale_length = malformed.len() as u32 + 4096;
            malformed[hmtx_record + 12..hmtx_record + 16]
                .copy_from_slice(&stale_length.to_be_bytes());
            match case {
                0 => malformed[hhea_offset + 34..hhea_offset + 36]
                    .copy_from_slice(&0_u16.to_be_bytes()),
                1 => malformed[hhea_offset + 34..hhea_offset + 36]
                    .copy_from_slice(&3_u16.to_be_bytes()),
                2 => malformed[loca_record + 8..loca_record + 12]
                    .copy_from_slice(&(hmtx_offset + 4).to_be_bytes()),
                3 => malformed[loca_record + 8..loca_record + 12]
                    .copy_from_slice(&hmtx_offset.to_be_bytes()),
                _ => malformed[hhea_record + 12..hhea_record + 16]
                    .copy_from_slice(&(hmtx_offset - hhea_offset as u32 + 2).to_be_bytes()),
            }
            let data: FontData = Arc::new(malformed);
            let blob =
                OpenTypeFontBlob::new(data.clone(), 0).expect("the table directory still parses");
            assert!(Arc::ptr_eq(&data, &blob.font_data()), "case {case}");
            assert!(blob.outline_glyph(GlyphId::new(1)).elements().is_empty());
        }
    }
}
