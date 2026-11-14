use crate::font::UNITS_PER_EM;
use crate::font::outline::OutlinePath;
use kurbo::BezPath;
use skrifa::instance::{LocationRef, Size};
use skrifa::metrics::GlyphMetrics;
use skrifa::outline::{DrawSettings, Engine, HintingInstance, HintingOptions, Target};
use skrifa::raw::TableProvider;
use skrifa::raw::model::pen::NullPen;
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
        // PdfCraft patch (93): a font dict that cannot be read (a CID font's FDArray entry
        // emptied by a subsetter, with no Private DICT) leaves only the glyphs it selects
        // missing. The font is dropped only when no font dict is usable.
        let subfonts = (0..font.num_subfonts())
            .map(|index| font.subfont(index, &[]).ok())
            .collect::<Vec<_>>();
        if !subfonts.iter().any(Option::is_some) {
            return None;
        }

        // PdfCraft patch (93): the subfonts read above own their data, so they move into the
        // yoke instead of being read again.
        let yoke = Yoke::<CFFYoke<'static>, FontData>::attach_to_cart(data.clone(), move |data| {
            let bytes = data.as_ref();
            let font = CffFontRef::new(bytes, 0, None).unwrap();
            let cff = Cff::read(ReadFontData::new(bytes)).unwrap();
            let charset = font.charset();
            CFFYoke {
                font,
                cff,
                charset,
                subfonts,
            }
        });

        let _ = (cff, charset);
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
        self.0.as_ref().get().subfonts.get(index)?.as_ref()
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

    /// PdfCraft patch (see vendor/README.md hayro-interpret (77)): the glyph's own advance from
    /// its charstring, in thousandths of an em like [`Self::outline_glyph`], or `None` when the
    /// charstring can't be read or gives no usable width.
    pub(crate) fn advance_width(&self, glyph: GlyphId) -> Option<f32> {
        let subfont = self.subfont(glyph)?;
        self.font()
            .draw(subfont, glyph, &[], Some(UNITS_PER_EM), &mut NullPen)
            .ok()
            .flatten()
            .filter(|width| width.is_finite() && *width > 0.0)
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

/// PdfCraft patch (83): a copy of the font with its `head.indexToLocFormat` set to the format its
/// `loca` table is in, when the field holds neither 0 (short offsets) nor 1 (long), or `None` when
/// it needs no change. The glyph outline reader takes only 1 as long, so a font whose field reads
/// 256 (the value's bytes swapped) had its long `loca` read as short offsets: every glyph came
/// out wrong, and a line of text set in it drew boxes (pdf.js `issue2537r.pdf`). The `loca`
/// table's length decides: room for `numGlyphs + 1` offsets of 4 bytes is long, room for only
/// that many of 2 bytes short (a table may carry padding after its offsets), and a table too
/// short for either is left as it is.
fn with_loca_format_fixed(data: &[u8], index: u32) -> Option<Vec<u8>> {
    use skrifa::raw::types::Tag;
    const HEAD: Tag = Tag::new(b"head");
    const LOCA: Tag = Tag::new(b"loca");
    // The `indexToLocFormat` field's offset in `head`.
    const FORMAT_AT: usize = 50;
    let f = FontRef::from_index(data, index).ok()?;
    let format = f.head().ok()?.index_to_loc_format();
    if format == 0 || format == 1 {
        return None;
    }
    let offsets = usize::from(f.maxp().ok()?.num_glyphs()).checked_add(1)?;
    let loca_len = f.table_data(LOCA)?.len();
    let long = if loca_len >= offsets.checked_mul(4)? {
        true
    } else if loca_len >= offsets.checked_mul(2)? {
        false
    } else {
        return None;
    };
    let head_at = f.table_directory.table_records().iter().find(|r| r.tag() == HEAD)?.offset() as usize;
    let at = head_at.checked_add(FORMAT_AT)?;
    let mut fixed = data.to_vec();
    fixed.get_mut(at..at.checked_add(2)?)?.copy_from_slice(&i16::from(long).to_be_bytes());
    Some(fixed)
}

/// PdfCraft patch (109): a copy of the font with its last `loca` offset set to the `glyf` table's
/// length, when that offset is smaller than the last glyph's start or past the table's end, or
/// `None` when it needs no change. `loca` holds `numGlyphs + 1` offsets, the last one the end of
/// the last glyph (OpenType `loca`), and some subsetters write it as 0: the last glyph then had a
/// negative length and was dropped, and with it every composite glyph using it, so an `é` whose
/// accent was the subset's last glyph drew neither the `e` nor the accent (PDFBOX-5917
/// `test.pdf`). A font whose last glyph starts past the end of `glyf` is left as it is, and so
/// are the offsets before the last one.
fn with_loca_end_fixed(data: &[u8], index: u32) -> Option<Vec<u8>> {
    use skrifa::raw::types::Tag;
    const LOCA: Tag = Tag::new(b"loca");
    const GLYF: Tag = Tag::new(b"glyf");
    let f = FontRef::from_index(data, index).ok()?;
    let long = match f.head().ok()?.index_to_loc_format() {
        0 => false,
        1 => true,
        _ => return None,
    };
    let size: usize = if long { 4 } else { 2 };
    let last = usize::from(f.maxp().ok()?.num_glyphs());
    let glyf_len = f.table_data(GLYF)?.len();
    let loca = f.table_data(LOCA)?;
    let loca = loca.as_bytes();
    let offset = |i: usize| -> Option<usize> {
        let at = i.checked_mul(size)?;
        Some(match *loca.get(at..at.checked_add(size)?)? {
            [a, b, c, d] => usize::try_from(u32::from_be_bytes([a, b, c, d])).ok()?,
            [a, b] => usize::from(u16::from_be_bytes([a, b])).checked_mul(2)?,
            _ => return None,
        })
    };
    let start = offset(last.checked_sub(1)?)?;
    let end = offset(last)?;
    if start > glyf_len || (start <= end && end <= glyf_len) {
        return None;
    }
    // Short offsets count 2-byte units: an odd length ends the glyph a byte early, never before
    // its start, which is even.
    let new_end = if long {
        u32::try_from(glyf_len).ok()?.to_be_bytes().to_vec()
    } else {
        u16::try_from(glyf_len / 2).ok()?.to_be_bytes().to_vec()
    };
    let loca_at = f.table_directory.table_records().iter().find(|r| r.tag() == LOCA)?.offset() as usize;
    let at = loca_at.checked_add(last.checked_mul(size)?)?;
    let mut fixed = data.to_vec();
    fixed.get_mut(at..at.checked_add(size)?)?.copy_from_slice(&new_end);
    Some(fixed)
}

impl OpenTypeFontBlob {
    pub(crate) fn new(data: FontData, index: u32) -> Option<Self> {
        // PdfCraft patch (83): see `with_loca_format_fixed`.
        let data = match with_loca_format_fixed(data.as_ref().as_ref(), index) {
            Some(fixed) => Arc::new(fixed) as FontData,
            None => data,
        };
        // PdfCraft patch (109): see `with_loca_end_fixed`.
        let data = match with_loca_end_fixed(data.as_ref().as_ref(), index) {
            Some(fixed) => Arc::new(fixed) as FontData,
            None => data,
        };
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
    /// PdfCraft patch (93): `None` for a font dict that could not be read.
    subfonts: Vec<Option<Subfont>>,
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

    /// A font of `head`, `maxp`, `loca` and a 21-byte `glyf`, with three glyphs and `loca` holding
    /// `offsets`, long when `long`.
    fn font_with_loca(long: bool, offsets: [u32; 4]) -> Vec<u8> {
        let mut head = vec![0; 54];
        head[50..52].copy_from_slice(&u16::from(long).to_be_bytes());
        let mut maxp = vec![0, 0, 0x50, 0];
        maxp.extend(3u16.to_be_bytes());
        let mut loca = Vec::new();
        for offset in offsets {
            if long {
                loca.extend(offset.to_be_bytes());
            } else {
                loca.extend(((offset / 2) as u16).to_be_bytes());
            }
        }
        let tables = [(*b"glyf", vec![0; 21]), (*b"head", head), (*b"loca", loca), (*b"maxp", maxp)];
        let mut out = vec![0, 1, 0, 0, 0, tables.len() as u8, 0, 0, 0, 0, 0, 0];
        let mut payload = Vec::new();
        let header = 12 + 16 * tables.len();
        let mut at = header;
        for (tag, bytes) in tables {
            out.extend(tag);
            out.extend(0u32.to_be_bytes());
            out.extend((at as u32).to_be_bytes());
            out.extend((bytes.len() as u32).to_be_bytes());
            let padded = bytes.len().div_ceil(4) * 4;
            at += padded;
            payload.extend(bytes);
            payload.resize(at - header, 0);
        }
        out.extend(payload);
        out
    }

    #[test]
    fn a_last_loca_offset_out_of_order_ends_at_the_glyf_table() {
        let fixed_end = |long: bool, offsets: [u32; 4]| {
            let font = font_with_loca(long, offsets);
            let fixed = with_loca_end_fixed(&font, 0)?;
            let f = FontRef::new(&fixed).expect("the fixed font parses");
            let loca = f.loca(Some(long)).expect("loca reads");
            assert_eq!(fixed.len(), font.len(), "only bytes change");
            Some(loca.get_raw(3).expect("four offsets"))
        };
        // In order, ending at or before the table's end: left as it is (the font is not copied).
        assert_eq!(fixed_end(true, [0, 8, 16, 21]), None);
        assert_eq!(fixed_end(true, [0, 8, 16, 20]), None);
        assert_eq!(fixed_end(false, [0, 8, 16, 20]), None);
        assert_eq!(fixed_end(true, [0, 8, 16, 16]), None, "an empty last glyph");
        // The last offset 0, or below the last glyph's start: the end of `glyf`.
        assert_eq!(fixed_end(true, [0, 8, 16, 0]), Some(21));
        assert_eq!(fixed_end(true, [0, 8, 16, 12]), Some(21));
        assert_eq!(fixed_end(false, [0, 8, 16, 0]), Some(20), "short offsets round down");
        // Past the end of `glyf`: clamped to it.
        assert_eq!(fixed_end(true, [0, 8, 16, 400]), Some(21));
        assert_eq!(fixed_end(false, [0, 8, 16, 24]), Some(20));
        // The last glyph starting past the end of `glyf` is not repaired.
        assert_eq!(fixed_end(true, [0, 8, 30, 0]), None);
    }
}
