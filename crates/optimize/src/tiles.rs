//! Large pictures drawn as tiles, for Windows printing (#5131).
//!
//! Windows prints each page through its own PDF renderer (`Windows.Data.Pdf`), which draws an
//! image of more than 2²² pixels from a copy shrunk to that size, however finely the page is
//! printed: a 600 ppi scan of a page prints at about 200 ppi. Drawn as tiles under that size, the
//! same picture keeps every pixel, and tiles that meet leave no seam (measured in #5131).
//! [`tile_large_images`] does this to the copy of a document that goes to the spooler.

use std::collections::HashSet;
use std::fmt::Write as _;

use pdfcraft_cos::{Dict, Document, ImageBudget, Object, Stream};
use pdfcraft_filters::Filter;

use crate::images::{self, MAX_PIXELS};

/// Images of more pixels than this are printed from a smaller copy on Windows: every larger image
/// in #5131 came out at 4,194,304 pixels.
pub const DOWNSAMPLED_ABOVE: u64 = 1 << 22;

/// A tile's side in pixels, before its overlap. With the overlap a tile has at most 2001 × 2001
/// pixels, under [`DOWNSAMPLED_ABOVE`]. A multiple of 8, so that in every row a tile's samples
/// start on a byte whatever the image's bits per pixel.
const TILE: u32 = 2000;

/// Pixels each tile reaches into the next one, so that rounding in the tiles' matrices, or a
/// printer driver that softens an image's edges, can't open a seam between them.
/// `Windows.Data.Pdf` itself leaves none either way, turned and scaled too (#5131). Not for an image
/// with a soft mask: where two tiles overlap, a pixel the mask makes see-through would be blended
/// twice and come out darker. The same goes for an opaque image drawn see-through (`/ca` below 1),
/// which this can't tell from the image: a faint line one pixel wide, against a seam on paper for
/// every tiled image on such a driver.
const OVERLAP: u32 = 1;

/// What [`tile_large_images`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TileReport {
    /// Images now drawn as tiles.
    pub tiled: usize,
    /// Images over [`DOWNSAMPLED_ABOVE`] left whole: JPEG 2000, a JPEG that isn't grey or RGB or
    /// whose colour transform readers disagree about, a mask of another size than its image, more
    /// than 80 million pixels, data shorter than the image, or past the decode allowance.
    pub left_whole: usize,
}

/// Replaces every image XObject of more than [`DOWNSAMPLED_ABOVE`] pixels with a form XObject of
/// the same object number that draws it as tiles of at most [`TILE`] pixels a side, overlapping
/// by [`OVERLAP`] unless it has a soft mask. Whatever draws the image (pages, forms, patterns, appearances) then draws the
/// tiles, unchanged. The samples are copied as they are, colour space, decode array and
/// interpolation included; a JPEG's are decoded and stored with Flate. A soft mask or stencil
/// mask of the image's size is cut into the same tiles. An image used as another's mask goes with
/// that image, and is otherwise left whole, as are the images [`TileReport::left_whole`] lists.
pub fn tile_large_images(doc: &mut Document) -> TileReport {
    let mut budget = ImageBudget::new(doc);
    let mut large = Vec::new();
    let mut masks = HashSet::new();
    for (r, object) in doc.scan_objects() {
        let Object::Stream(s) = &*object else { continue };
        if s.dict.name(b"Subtype") != Some(b"Image") {
            continue;
        }
        masks.extend([b"SMask".as_slice(), b"Mask"].into_iter().filter_map(|key| s.dict.reference(key)));
        if size(s).is_some_and(|(w, h)| u64::from(w) * u64::from(h) > DOWNSAMPLED_ABOVE) {
            large.push(r);
        }
    }
    let mut report = TileReport::default();
    for r in large.into_iter().filter(|r| !masks.contains(r)) {
        let object = doc.get(r);
        let Object::Stream(image) = &*object else { continue };
        match tiles(doc, image, &mut budget) {
            Some(tiles) => {
                let form = tiles.store(doc, image);
                doc.set(r, Object::Stream(form));
                report.tiled += 1;
            }
            None => report.left_whole += 1,
        }
    }
    report
}

/// An image's `/Width` and `/Height`, when both are positive.
fn size(s: &Stream) -> Option<(u32, u32)> {
    let side = |key: &[u8]| s.dict.int(key).and_then(|v| u32::try_from(v).ok()).filter(|v| *v > 0);
    Some((side(b"Width")?, side(b"Height")?))
}

/// An image's samples, decoded: rows of `bits_per_pixel` × width bits each, padded to a byte.
/// Kept as the image stores them, in its own colour space and depth (CMYK, indexed, 16-bit, a
/// stencil mask), so each tile redraws exactly the picture it replaces. That is why this is not
/// the crate's [`crate::Samples`], which turns an image into 8-bit grey or RGB for tools that
/// repaint it and refuses the rest.
struct StoredSamples {
    data: Vec<u8>,
    bits_per_pixel: usize,
    /// Bits per colour component as stored: 8 for a decoded JPEG.
    bits_per_component: i64,
}

impl StoredSamples {
    /// Image `s`, `w` × `h`, decoded. A JPEG gives eight-bit grey or RGB ([`images::pixels`]),
    /// anything else its samples as they are stored ([`ImageBudget::decode`]). A soft mask is grey
    /// whether or not it says so: ISO 32000-2 requires `/ColorSpace /DeviceGray` of one.
    fn decode(doc: &Document, s: &Stream, (w, h): (u32, u32), soft_mask: bool, budget: &mut ImageBudget) -> Option<StoredSamples> {
        if u64::from(w) * u64::from(h) > MAX_PIXELS {
            return None;
        }
        let stencil = matches!(s.dict.get(b"ImageMask").map(|m| doc.resolve(m)).as_deref(), Some(Object::Bool(true)));
        let components = |colour_space: fn(&Document, &Object) -> Option<usize>| match s.dict.get(b"ColorSpace") {
            Some(cs) => colour_space(doc, cs),
            None => soft_mask.then_some(1),
        };
        match s.filters().last().map(|(f, _)| f.clone()) {
            Some(Filter::Jpx) => None,
            Some(Filter::Dct) if !stencil => {
                let components = components(images::components)?;
                let data = images::pixels(doc, s, components, budget)?;
                Some(StoredSamples { data, bits_per_pixel: components.checked_mul(8)?, bits_per_component: 8 })
            }
            Some(Filter::Dct) => None,
            _ => {
                let (components, bits_per_component) =
                    if stencil { (1, 1) } else { (components(sample_components)?, s.dict.int(b"BitsPerComponent")?) };
                if !matches!(bits_per_component, 1 | 2 | 4 | 8 | 16) {
                    return None;
                }
                let bits_per_pixel = components.checked_mul(usize::try_from(bits_per_component).ok()?)?;
                let len = row_bytes(w, bits_per_pixel)?.checked_mul(usize::try_from(h).ok()?)?;
                let mut data = budget.decode(doc, s, images::allowance(len, h)?).ok()?;
                // Fewer bytes would leave rows unpainted; readers ignore what follows the last row.
                if data.len() < len {
                    return None;
                }
                data.truncate(len);
                Some(StoredSamples { data, bits_per_pixel, bits_per_component })
            }
        }
    }

    /// The samples of columns `x` and rows `y` (start and end), rows padded to a byte. `x.0` is a
    /// multiple of [`TILE`], so of 8: each row of the tile starts on a byte of the image's row.
    fn cut(&self, w: u32, x: (u32, u32), y: (u32, u32)) -> Option<Vec<u8>> {
        let stride = row_bytes(w, self.bits_per_pixel)?;
        let start = usize::try_from(x.0).ok()?.checked_mul(self.bits_per_pixel)? / 8;
        let len = row_bytes(x.1.checked_sub(x.0)?, self.bits_per_pixel)?;
        let mut out = Vec::with_capacity(len.checked_mul(usize::try_from(y.1.checked_sub(y.0)?).ok()?)?);
        for row in y.0..y.1 {
            let from = usize::try_from(row).ok()?.checked_mul(stride)?.checked_add(start)?;
            out.extend_from_slice(self.data.get(from..from.checked_add(len)?)?);
        }
        Some(out)
    }
}

/// Bytes in a row of `w` pixels of `bits_per_pixel` bits, padded to a byte.
fn row_bytes(w: u32, bits_per_pixel: usize) -> Option<usize> {
    Some(usize::try_from(w).ok()?.checked_mul(bits_per_pixel)?.div_ceil(8))
}

/// Colour components in a sample of colour space `cs` (ISO 32000-2 §8.6): `None` for a space an
/// image can't be in, or one that can't be read.
fn sample_components(doc: &Document, cs: &Object) -> Option<usize> {
    match &*doc.resolve(cs) {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"G" => Some(1),
            b"DeviceRGB" | b"RGB" => Some(3),
            b"DeviceCMYK" | b"CMYK" => Some(4),
            _ => None,
        },
        Object::Array(a) => match doc.resolve(a.first()?).as_name()? {
            b"CalGray" | b"Indexed" | b"I" | b"Separation" => Some(1),
            b"CalRGB" | b"Lab" => Some(3),
            b"ICCBased" => match &*doc.resolve(a.get(1)?) {
                Object::Stream(icc) => usize::try_from(icc.dict.int(b"N")?).ok().filter(|n| (1..=32).contains(n)),
                _ => None,
            },
            b"DeviceN" => Some(doc.resolve(a.get(1)?).as_array()?.len()).filter(|n| (1..=32).contains(n)),
            _ => None,
        },
        _ => None,
    }
}

/// The spans, start and end, an image side of `len` pixels is cut into: one every [`TILE`],
/// each `overlap` longer where the side goes on. None lies wholly inside the one before's overlap.
fn spans(len: u32, overlap: u32) -> impl Iterator<Item = (u32, u32)> {
    (0..len.saturating_sub(overlap).div_ceil(TILE).max(1)).map(move |i| {
        let start = i.saturating_mul(TILE);
        (start, start.saturating_add(TILE).saturating_add(overlap).min(len))
    })
}

/// One tile of an image: the columns and rows it covers (start and end), its samples, and its
/// mask's when the image has a mask stream.
struct Tile {
    x: (u32, u32),
    y: (u32, u32),
    image: Stream,
    mask: Option<Stream>,
}

/// An image cut into tiles, with its mask cut the same way, ready to be stored.
struct Tiles {
    size: (u32, u32),
    tiles: Vec<Tile>,
    /// The key the image names its mask by: `SMask` or `Mask`.
    mask_key: &'static [u8],
}

/// Image `image` cut into tiles, or `None` when it can't be read faithfully.
fn tiles(doc: &Document, image: &Stream, budget: &mut ImageBudget) -> Option<Tiles> {
    let dims = size(image)?;
    let samples = StoredSamples::decode(doc, image, dims, false, budget)?;
    // A soft mask overrides `/Mask` (ISO 32000-2, the image dictionary's `/SMask`). A mask stream
    // goes into the same tiles, so it has to have the image's size.
    let soft = image.dict.contains(b"SMask");
    let mask_key: &'static [u8] = if soft { b"SMask" } else { b"Mask" };
    let mask_object = image.dict.get(mask_key).map(|m| doc.resolve(m));
    let mask = match mask_object.as_deref() {
        Some(Object::Stream(m)) if size(m) == Some(dims) => Some((m, StoredSamples::decode(doc, m, dims, soft, budget)?)),
        // A colour key mask goes on each tile as it is (`tile_dict`).
        Some(Object::Array(_)) if !soft => None,
        None | Some(Object::Null) => None,
        Some(_) => return None,
    };
    let mut tiles = Vec::new();
    // Overlapping tiles blend a see-through pixel twice ([`OVERLAP`]).
    let overlap = if soft { 0 } else { OVERLAP };
    for y in spans(dims.1, overlap) {
        for x in spans(dims.0, overlap) {
            let tile = |s: &Stream, samples: &StoredSamples| -> Option<Stream> {
                let dict = tile_dict(doc, &s.dict, (x.1.saturating_sub(x.0), y.1.saturating_sub(y.0)), samples.bits_per_component);
                Some(Stream::flate(dict, &samples.cut(dims.0, x, y)?))
            };
            let mask_tile = match &mask {
                Some((m, samples)) => Some(tile(m, samples)?),
                None => None,
            };
            tiles.push(Tile { x, y, image: tile(image, &samples)?, mask: mask_tile });
        }
    }
    Some(Tiles { size: dims, tiles, mask_key })
}

/// The dictionary of a tile `w` × `h` of the image whose dictionary is `image`: what paints its
/// samples, and nothing that belongs to the image as a whole (its mask goes on separately; its
/// optional content, structure and metadata stay with the form that replaces it).
fn tile_dict(doc: &Document, image: &Dict, (w, h): (u32, u32), bits_per_component: i64) -> Dict {
    let mut d = Dict::new();
    d.set("Type", Object::name("XObject"));
    d.set("Subtype", Object::name("Image"));
    d.set("Width", Object::Int(i64::from(w)));
    d.set("Height", Object::Int(i64::from(h)));
    d.set("BitsPerComponent", Object::Int(bits_per_component));
    for key in [b"ColorSpace".as_slice(), b"Decode", b"ImageMask", b"Interpolate", b"Intent", b"Matte"] {
        if let Some(v) = image.get(key) {
            d.set(key.to_vec(), v.clone());
        }
    }
    // A colour key mask lists sample values, which the tiles keep.
    if let Some(Object::Array(key)) = image.get(b"Mask").map(|m| doc.resolve(m)).as_deref() {
        d.set("Mask", Object::Array(key.clone()));
    }
    d
}

impl Tiles {
    /// Adds the tiles to `doc` and returns the form XObject that draws them where `image` was: the
    /// unit square, each tile in its share of it, the image's top row at the top.
    fn store(self, doc: &mut Document, image: &Stream) -> Stream {
        let (w, h) = (f64::from(self.size.0), f64::from(self.size.1));
        let mut names = Dict::new();
        let mut content = String::new();
        for (i, Tile { x, y, image: mut tile, mask }) in self.tiles.into_iter().enumerate() {
            if let Some(mask) = mask {
                tile.dict.set(self.mask_key.to_vec(), Object::Ref(doc.add(Object::Stream(mask))));
            }
            names.set(format!("T{i}").into_bytes(), Object::Ref(doc.add(Object::Stream(tile))));
            let [a, d] = [f64::from(x.1.saturating_sub(x.0)) / w, f64::from(y.1.saturating_sub(y.0)) / h];
            let [e, f] = [f64::from(x.0) / w, 1.0 - f64::from(y.1) / h];
            // Writing to a String can't fail.
            let _ = writeln!(content, "q {} 0 0 {} {} {} cm /T{i} Do Q", number(a), number(d), number(e), number(f));
        }
        let mut resources = Dict::new();
        resources.set("XObject", Object::Dict(names));
        let mut form = Dict::new();
        form.set("Type", Object::name("XObject"));
        form.set("Subtype", Object::name("Form"));
        form.set("BBox", Object::Array([0, 0, 1, 1].map(Object::Int).to_vec()));
        form.set("Resources", Object::Dict(resources));
        if let Some(oc) = image.dict.get(b"OC") {
            form.set("OC", oc.clone());
        }
        Stream::flate(form, content.as_bytes())
    }
}

/// `v` in a content stream: ten decimals, enough for a pixel of an image 80 million wide, trailing
/// zeros dropped.
fn number(v: f64) -> String {
    let s = format!("{v:.10}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" { "0".to_string() } else { s.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sides_are_cut_every_tile_with_an_overlap() {
        let spans = |len| spans(len, OVERLAP).collect::<Vec<_>>();
        assert_eq!(spans(1), [(0, 1)]);
        assert_eq!(spans(2000), [(0, 2000)]);
        // The 2001st pixel is the first tile's overlap: no tile of its own.
        assert_eq!(spans(2001), [(0, 2001)]);
        assert_eq!(spans(2002), [(0, 2001), (2000, 2002)]);
        assert_eq!(spans(6000), [(0, 2001), (2000, 4001), (4000, 6000)]);
        // Every pixel is in a tile, and no tile has more than its side and the overlap.
        for len in [1, 1999, 2000, 2001, 2002, 4001, 4002, 6000, 123_457] {
            let s = spans(len);
            let zero: Vec<_> = super::spans(len, 0).collect();
            assert!(zero.windows(2).all(|w| w[1].0 == w[0].1), "{len}: tiles without an overlap meet exactly");
            assert_eq!(s.first().map(|t| t.0), Some(0));
            assert_eq!(s.last().map(|t| t.1), Some(len));
            assert!(s.windows(2).all(|w| w[1].0 < w[0].1), "{len}: a gap");
            assert!(s.iter().all(|(a, b)| b - a <= TILE + OVERLAP && a % 8 == 0), "{len}");
        }
    }

    #[test]
    fn a_tile_takes_whole_bytes_of_each_row_at_any_bit_depth() {
        // Two rows of 4004 one-bit pixels: 501 bytes each, the last holding 4 pixels.
        let row: Vec<u8> = (0..501).map(|i| i as u8).collect();
        let samples = StoredSamples { data: [row.clone(), row.iter().map(|b| !b).collect()].concat(), bits_per_pixel: 1, bits_per_component: 1 };
        let tile = samples.cut(4004, (4000, 4004), (0, 2)).unwrap();
        assert_eq!(tile, [row[500], !row[500]]);
        let first = samples.cut(4004, (0, 2001), (1, 2)).unwrap();
        assert_eq!(first.len(), 251);
        assert_eq!(first[0], !row[0]);
        // Sixteen-bit RGB: six bytes a pixel.
        let rgb16 = StoredSamples { data: (0..=255u8).cycle().take(6 * 2002).collect(), bits_per_pixel: 48, bits_per_component: 16 };
        assert_eq!(rgb16.cut(2002, (2000, 2002), (0, 1)).unwrap(), rgb16.data[12_000..12_012]);
        // Rows past the data are no tile, not a panic.
        assert_eq!(samples.cut(4004, (0, 8), (1, 3)), None);
    }

    #[test]
    fn numbers_are_written_plainly() {
        assert_eq!(number(0.0), "0");
        assert_eq!(number(1.0), "1");
        assert_eq!(number(0.5), "0.5");
        assert_eq!(number(2001.0 / 35_000.0), "0.0571714286");
        assert_eq!(number(1.0 - 4001.0 / 6000.0), "0.3331666667");
    }
}
