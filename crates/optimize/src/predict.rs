//! PNG prediction for image samples the optimizer stores with Flate (ISO 32000-2 §7.4.4.4,
//! `/Predictor 15`: each row tagged with the PNG filter chosen for it). Smooth images (scans,
//! gradients, rendered art) usually compress much smaller predicted; noise does not.

use pdfcraft_cos::{Dict, Document, Object, Stream};
use pdfcraft_filters::{Filter, Params};

/// Whether image samples stored with Flate are PNG-predicted first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlatePredictor {
    /// Plain Flate.
    #[default]
    Off,
    /// `/Predictor 15` whenever the samples' layout allows it.
    Png,
    /// Both, keeping whichever is smaller as stored, dictionary included (plain on a tie).
    Auto,
}

impl FlatePredictor {
    pub const ALL: [FlatePredictor; 3] = [FlatePredictor::Off, FlatePredictor::Png, FlatePredictor::Auto];

    /// The name tools use (`"off"`, `"png"`, `"auto"`).
    pub fn name(self) -> &'static str {
        match self {
            FlatePredictor::Off => "off",
            FlatePredictor::Png => "png",
            FlatePredictor::Auto => "auto",
        }
    }

    pub fn from_name(name: &str) -> Option<FlatePredictor> {
        FlatePredictor::ALL.into_iter().find(|p| p.name().eq_ignore_ascii_case(name.trim()))
    }
}

/// How image samples lie: `rows` rows of `columns` samples of `colors` components at `bpc` bits,
/// each row starting on a new byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) colors: u32,
    pub(crate) bpc: u32,
    pub(crate) columns: u32,
    pub(crate) rows: u32,
}

impl Layout {
    /// Whether `samples` are exactly its rows, in a layout the predictor takes. Only then: a
    /// reader takes data of exactly `rows` untagged rows as samples, so predicted bytes of
    /// another length could be misread as such.
    fn holds(self, samples: &[u8]) -> bool {
        let row = u64::from(self.columns).checked_mul(u64::from(self.colors)).and_then(|s| s.checked_mul(u64::from(self.bpc))).map(|b| b.div_ceil(8));
        let total = row.and_then(|r| r.checked_mul(u64::from(self.rows)));
        matches!(self.bpc, 1 | 2 | 4 | 8 | 16)
            && (1..=255).contains(&self.colors)
            && self.columns > 0
            && self.rows > 0
            && total.and_then(|t| usize::try_from(t).ok()) == Some(samples.len())
    }
}

/// `dict` holding `samples` stored with Flate, PNG-predicted as `predictor` says. Samples that
/// don't fill `layout` exactly, or that the predictor refuses, are stored plain.
pub(crate) fn flate_image(dict: Dict, samples: &[u8], layout: Layout, predictor: FlatePredictor) -> Stream {
    if predictor == FlatePredictor::Off {
        return Stream::flate(dict, samples);
    }
    let Some(predicted) = predicted(&dict, samples, layout) else {
        return Stream::flate(dict, samples);
    };
    if predictor == FlatePredictor::Auto {
        let plain = Stream::flate(dict, samples);
        // A length that can't be counted is no reason to prefer the predicted copy.
        match (crate::stored_len(&plain), crate::stored_len(&predicted)) {
            (Some(p), Some(q)) if q < p => {}
            _ => return plain,
        }
    }
    predicted
}

fn predicted(dict: &Dict, samples: &[u8], l: Layout) -> Option<Stream> {
    if !l.holds(samples) {
        return None;
    }
    let params = Params {
        predictor: 15,
        colors: i64::from(l.colors),
        bits_per_component: i64::from(l.bpc),
        columns: i64::from(l.columns),
        ..Params::default()
    };
    let raw = pdfcraft_filters::encode(&Filter::Flate, &params, samples).ok()?;
    let mut parms = Dict::new();
    for (k, v) in
        [(&b"Predictor"[..], 15), (b"Colors", params.colors), (b"BitsPerComponent", params.bits_per_component), (b"Columns", params.columns)]
    {
        parms.set(k.to_vec(), Object::Int(v));
    }
    let mut d = dict.clone();
    d.set(b"Filter".to_vec(), Object::name("FlateDecode"));
    d.set(b"DecodeParms".to_vec(), Object::Dict(parms));
    Some(Stream::from_raw(d, raw))
}

/// As [`flate_image`], except that under `Png` a predicted copy no smaller as stored than what it
/// would replace, `limit` bytes, gives way to plain Flate, which may be.
fn flate_image_under(dict: Dict, samples: &[u8], layout: Layout, predictor: FlatePredictor, limit: Option<usize>) -> Stream {
    let s = flate_image(dict.clone(), samples, layout, predictor);
    match (limit, crate::stored_len(&s)) {
        (Some(limit), Some(len)) if is_predicted(&s) && len >= limit => Stream::flate(dict, samples),
        _ => s,
    }
}

/// `dict` holding `w` × `h` 1-bit samples, rows byte-padded, stored with Flate as
/// [`flate_image_under`].
pub(crate) fn flate_one_bit(dict: Dict, samples: &[u8], (w, h): (u32, u32), predictor: FlatePredictor, limit: Option<usize>) -> Stream {
    flate_image_under(dict, samples, Layout { colors: 1, bpc: 1, columns: w, rows: h }, predictor, limit)
}

/// The unfiltered stream data `raw` under `dict` stored with Flate: as [`flate_image_under`]
/// when `dict` is an image XObject whose layout [`image_layout`] knows, plain otherwise.
pub(crate) fn flate_unfiltered(doc: &Document, dict: Dict, raw: &[u8], predictor: FlatePredictor, limit: Option<usize>) -> Stream {
    match image_layout(doc, &dict).filter(|_| dict.name(b"Subtype") == Some(b"Image")) {
        Some(layout) => flate_image_under(dict, raw, layout, predictor, limit),
        None => Stream::flate(dict, raw),
    }
}

/// Whether `s` is stored with Flate alone under a PNG predictor.
pub(crate) fn is_predicted(s: &Stream) -> bool {
    s.dict.name(b"Filter") == Some(b"FlateDecode")
        && matches!(s.dict.get(b"DecodeParms"), Some(Object::Dict(p)) if p.int(b"Predictor").is_some_and(|v| (10..=15).contains(&v)))
}

/// The layout of an unfiltered image XObject's data, read from direct entries: an image mask is
/// one component of one bit. `None` when a value is missing, indirect or out of range, or the
/// colour space's component count isn't known for certain.
pub(crate) fn image_layout(doc: &Document, d: &Dict) -> Option<Layout> {
    let side = |k: &[u8]| d.int(k).and_then(|v| u32::try_from(v).ok());
    let (columns, rows) = (side(b"Width")?, side(b"Height")?);
    if matches!(d.get(b"ImageMask"), Some(Object::Bool(true))) {
        return Some(Layout { colors: 1, bpc: 1, columns, rows });
    }
    let bpc = side(b"BitsPerComponent")?;
    let colors = colours(doc, d.get(b"ColorSpace")?)?;
    Some(Layout { colors, bpc, columns, rows })
}

/// The colour components of a sample in colour space `cs`, for the spaces whose count is fixed
/// by their family (ISO 32000-2 §8.6): an indexed or separation sample is one, DeviceN as many
/// as its colourants, ICC-based as its `/N`.
fn colours(doc: &Document, cs: &Object) -> Option<u32> {
    let cs = doc.resolve(cs);
    match &*cs {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"G" => Some(1),
            b"DeviceRGB" | b"RGB" => Some(3),
            b"DeviceCMYK" | b"CMYK" => Some(4),
            _ => None,
        },
        Object::Array(a) => match a.first()?.as_name()? {
            b"CalGray" | b"Indexed" | b"I" | b"Separation" => Some(1),
            b"CalRGB" | b"Lab" => Some(3),
            b"DeviceN" => u32::try_from(doc.resolve(a.get(1)?).as_array()?.len()).ok().filter(|n| *n > 0),
            b"ICCBased" => match &*doc.resolve(a.get(1)?) {
                Object::Stream(icc) => icc.dict.int(b"N").and_then(|n| u32::try_from(n).ok()).filter(|n| (1..=4).contains(n)),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(colors: u32, bpc: u32, columns: u32, rows: u32) -> Layout {
        Layout { colors, bpc, columns, rows }
    }

    fn decoded(s: &Stream) -> Vec<u8> {
        let max = s.raw.len().saturating_mul(1000).saturating_add(1 << 16);
        pdfcraft_filters::decode(&s.filters(), &s.raw, max).unwrap()
    }

    /// A smooth ramp over `n` components of `bytes` bytes each, `w` × `h`.
    fn ramp(w: usize, h: usize, n: usize) -> Vec<u8> {
        (0..h).flat_map(|y| (0..w).flat_map(move |x| (0..n).map(move |c| ((x * 3 + y * 2 + c * 40) % 256) as u8))).collect()
    }

    fn noise(len: usize) -> Vec<u8> {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        (0..len)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn a_smooth_image_gets_smaller_predicted_and_decodes_to_the_same_samples() {
        let rgb = ramp(96, 96, 3);
        let l = layout(3, 8, 96, 96);
        let plain = flate_image(Dict::new(), &rgb, l, FlatePredictor::Off);
        let auto = flate_image(Dict::new(), &rgb, l, FlatePredictor::Auto);
        assert!(is_predicted(&auto) && !is_predicted(&plain));
        assert!(crate::stored_len(&auto) < crate::stored_len(&plain), "{} vs {}", auto.raw.len(), plain.raw.len());
        assert_eq!(decoded(&auto), rgb);
    }

    #[test]
    fn auto_keeps_plain_flate_for_noise_and_png_predicts_it_anyway() {
        let n = noise(64 * 64);
        let l = layout(1, 8, 64, 64);
        let auto = flate_image(Dict::new(), &n, l, FlatePredictor::Auto);
        assert!(!is_predicted(&auto));
        assert_eq!(auto.raw, flate_image(Dict::new(), &n, l, FlatePredictor::Off).raw);
        let png = flate_image(Dict::new(), &n, l, FlatePredictor::Png);
        assert!(is_predicted(&png));
        assert_eq!(decoded(&png), n);
    }

    #[test]
    fn every_bit_depth_round_trips_through_the_predictor() {
        for (colors, bpc) in [(1, 1), (1, 2), (1, 4), (3, 8), (4, 8), (1, 16), (3, 16)] {
            let (w, h) = (37u32, 11u32);
            let row = (w * colors * bpc).div_ceil(8) as usize;
            let samples: Vec<u8> = (0..row * h as usize).map(|i| (i * 7 % 251) as u8).collect();
            let s = flate_image(Dict::new(), &samples, layout(colors, bpc, w, h), FlatePredictor::Png);
            assert!(is_predicted(&s), "{colors} × {bpc}");
            assert_eq!(decoded(&s), samples, "{colors} × {bpc}");
        }
    }

    /// Worked by hand from §7.4.4.4's filters and the least-sum-of-residuals choice: a 16-bit grey
    /// image of 2 × 3 samples, two bytes a sample. Row 1 gets Sub (residuals 1 2 2 2 against None's
    /// 1 2 3 4), row 2 Up (it repeats row 1), row 3 Sub again (0 16 0 0, less than Up's or Paeth's).
    #[test]
    fn png_predicted_rows_are_the_filter_types_and_residuals_worked_by_hand() {
        let samples = [0x01, 0x02, 0x03, 0x04, 0x01, 0x02, 0x03, 0x04, 0x00, 0x10, 0x00, 0x10];
        let s = flate_image(Dict::new(), &samples, layout(1, 16, 2, 3), FlatePredictor::Png);
        let inflated = pdfcraft_filters::decode(&[(Filter::Flate, Params::default())], &s.raw, 1 << 10).unwrap();
        assert_eq!(inflated, [1, 0x01, 0x02, 0x02, 0x02, 2, 0, 0, 0, 0, 1, 0x00, 0x10, 0x00, 0x00]);
        let parms = s.dict.get(b"DecodeParms").and_then(Object::as_dict).unwrap();
        let ints = [&b"Predictor"[..], b"Colors", b"BitsPerComponent", b"Columns"].map(|k| parms.int(k));
        assert_eq!(ints, [Some(15), Some(1), Some(16), Some(2)]);
    }

    #[test]
    fn samples_that_dont_fill_their_layout_are_stored_plain() {
        let rgb = ramp(16, 16, 3);
        for l in [layout(3, 8, 16, 15), layout(3, 8, 16, 17), layout(3, 3, 16, 16), layout(0, 8, 16, 16), layout(3, 8, u32::MAX, u32::MAX)] {
            assert!(!is_predicted(&flate_image(Dict::new(), &rgb, l, FlatePredictor::Png)), "{l:?}");
        }
    }
}
