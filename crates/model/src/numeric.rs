//! What numeric arrays in the object graph mean: rectangles, groups of quadrilaterals and device
//! colours, read through [`NumberReader`] so a bad entry is reported where it is instead of
//! shifting the others.
//!
//! These interpret; they don't decide. Whether a malformed `/QuadPoints` falls back to `/Rect`,
//! or a missing colour is transparent or black, is each caller's policy. Absence of a key stays
//! outside these functions (`dict.get(key)`), so absent, empty, malformed and too-long remain
//! distinct.

use pdfcraft_cos::Object;
pub use pdfcraft_cos::numbers::{ArrayError, MAX_NUMERIC_ENTRIES, NumberError, NumberReader};

/// A rectangle: exactly four finite numbers, its corners ordered. No area or page-size policy:
/// a rectangle with no width is still one.
pub fn rect(reader: &NumberReader<'_>, object: &Object) -> Result<[f64; 4], ArrayError> {
    // `tuple` gives only finite numbers, so ordering them can't meet a NaN.
    let [x0, y0, x1, y1] = reader.tuple::<4>(object)?;
    Ok([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
}

/// Why one group of eight numbers is not a quadrilateral.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum QuadError {
    /// The first bad corner value, at `offset` (0..8) within its group.
    #[error("corner value {offset}: {error}")]
    InvalidCorner { offset: usize, error: NumberError },
}

/// A numeric array read as quadrilaterals of eight numbers each (`/QuadPoints`): group `i` is
/// original entries `8 * i .. 8 * i + 8`, whatever the groups before it held.
#[derive(Clone, Debug, PartialEq)]
pub struct QuadGroups {
    pub groups: Vec<Result<[f64; 8], QuadError>>,
    /// The entries left after the last whole group (0..=7), valid or not; never dropped silently.
    pub trailing: usize,
}

/// `object` as groups of eight finite numbers, from at most `cap` entries (and never more than
/// [`MAX_NUMERIC_ENTRIES`]). Says nothing about whether the quads are usable: degenerate or
/// partial ones are for the caller to judge.
pub fn quad_groups(reader: &NumberReader<'_>, object: &Object, cap: usize) -> Result<QuadGroups, ArrayError> {
    let seq = reader.sequence(object, cap)?;
    let (chunks, rest) = seq.entries.as_chunks::<8>();
    let trailing = rest.len();
    let mut groups = Vec::new();
    groups.try_reserve_exact(chunks.len()).map_err(|_| ArrayError::Allocation)?;
    for chunk in chunks {
        let mut quad = [0.0; 8];
        let mut bad = None;
        for (offset, (slot, entry)) in quad.iter_mut().zip(chunk).enumerate() {
            match entry {
                Ok(v) => *slot = *v,
                Err(error) => {
                    bad = Some(QuadError::InvalidCorner { offset, error: *error });
                    break;
                }
            }
        }
        groups.push(bad.map_or(Ok(quad), Err));
    }
    Ok(QuadGroups { groups, trailing })
}

/// A colour in a device colour space, as `/C`, `/IC` and similar arrays give it (by count).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeviceColor {
    Gray(f64),
    Rgb([f64; 3]),
    Cmyk([f64; 4]),
}

/// Why an array is not a device colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ColorError {
    #[error(transparent)]
    Array(ArrayError),
    /// The array has this many entries, not 1, 3 or 4 (an empty one included).
    #[error("{0} colour components, not 1, 3 or 4")]
    Components(usize),
}

/// `object` as a gray, RGB or CMYK colour: its length (1, 3 or 4) is checked before any entry is
/// read, and any entry that is not a finite number rejects the whole colour. `[null r g b]` is
/// not an RGB colour. An empty array is [`ColorError::Components`]`(0)`: a caller that reads `[]`
/// as transparent says so itself.
pub fn device_color(reader: &NumberReader<'_>, object: &Object) -> Result<DeviceColor, ColorError> {
    // A wrong length resolves no entry, so trying the sizes in turn costs at most the outer array.
    match reader.tuple::<4>(object) {
        Ok(c) => Ok(DeviceColor::Cmyk(c)),
        Err(ArrayError::WrongLength { actual: 1, .. }) => reader.tuple::<1>(object).map(|[g]| DeviceColor::Gray(g)).map_err(ColorError::Array),
        Err(ArrayError::WrongLength { actual: 3, .. }) => reader.tuple::<3>(object).map(DeviceColor::Rgb).map_err(ColorError::Array),
        Err(ArrayError::WrongLength { actual, .. }) => Err(ColorError::Components(actual)),
        Err(e) => Err(ColorError::Array(e)),
    }
}

impl DeviceColor {
    /// As RGB: gray repeated, RGB itself, CMYK as `(1 - c)(1 - k)` per channel. `None` when that
    /// isn't finite. Nothing is clamped; callers that clamp do it themselves.
    pub fn rgb(&self) -> Option<[f64; 3]> {
        let rgb = match *self {
            DeviceColor::Gray(g) => [g; 3],
            DeviceColor::Rgb(c) => c,
            DeviceColor::Cmyk([c, m, y, k]) => [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)],
        };
        rgb.iter().all(|v| v.is_finite()).then_some(rgb)
    }
}

#[cfg(test)]
mod tests {
    use pdfcraft_cos::{Document, Lexer};

    use super::*;

    fn parse(text: &[u8]) -> Object {
        Lexer::new(text, 0).object().unwrap()
    }

    #[test]
    fn rect_is_an_exact_finite_tuple_normalized() {
        let doc = Document::new_empty();
        let r = NumberReader::new(&doc);
        assert_eq!(rect(&r, &parse(b"[40 110 20 95]")), Ok([20.0, 95.0, 40.0, 110.0]));
        assert_eq!(rect(&r, &parse(b"[1 1 1 1]")), Ok([1.0, 1.0, 1.0, 1.0]), "degenerate is allowed here");
        assert_eq!(rect(&r, &parse(b"[null 1 2 3 4]")), Err(ArrayError::WrongLength { expected: 4, actual: 5 }));
        assert_eq!(rect(&r, &parse(b"[1 2 3]")), Err(ArrayError::WrongLength { expected: 4, actual: 3 }));
        assert_eq!(rect(&r, &parse(b"[1 2 -nan 4]")), Err(ArrayError::InvalidEntry { index: 2, error: NumberError::NonFinite }));
        let mut doc = Document::new_empty();
        let x = doc.add(Object::Int(40));
        let arr = doc.add(Object::Array(vec![Object::Int(20), Object::Int(95), Object::Ref(x), Object::Int(110)]));
        assert_eq!(rect(&NumberReader::new(&doc), &Object::Ref(arr)), Ok([20.0, 95.0, 40.0, 110.0]), "indirect array and entry");
    }

    #[test]
    fn quad_groups_keep_original_positions() {
        let doc = Document::new_empty();
        let r = NumberReader::new(&doc);
        // An invalid first group does not move the second one's corners.
        let q = quad_groups(&r, &parse(b"[null 110 40 110 30 95 40 95 1 2 3 4 5 6 7 8]"), 64).unwrap();
        assert_eq!(
            q,
            QuadGroups {
                groups: vec![
                    Err(QuadError::InvalidCorner { offset: 0, error: NumberError::NotNumber }),
                    Ok([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0])
                ],
                trailing: 0
            }
        );
        // The first bad corner is the one reported.
        let q = quad_groups(&r, &parse(b"[1 2 3 -nan 5 null 7 8]"), 64).unwrap();
        assert_eq!(q.groups, vec![Err(QuadError::InvalidCorner { offset: 3, error: NumberError::NonFinite })]);
        // Every tail length 1..=7 is reported, valid entries or not.
        for tail in 1..=7usize {
            let mut text = b"[1 2 3 4 5 6 7 8".to_vec();
            for _ in 0..tail {
                text.extend_from_slice(b" 9");
            }
            text.push(b']');
            let q = quad_groups(&r, &parse(&text), 64).unwrap();
            assert_eq!((q.groups.len(), q.trailing), (1, tail));
        }
        assert_eq!(quad_groups(&r, &parse(b"[]"), 64), Ok(QuadGroups { groups: Vec::new(), trailing: 0 }));
        assert_eq!(quad_groups(&r, &parse(b"[1 2 3]"), 64), Ok(QuadGroups { groups: Vec::new(), trailing: 3 }));
        assert_eq!(quad_groups(&r, &parse(b"/Name"), 64), Err(ArrayError::NotArray));
        assert_eq!(quad_groups(&r, &parse(b"[1 2 3 4 5 6 7 8 9]"), 8), Err(ArrayError::TooLong { len: 9, cap: 8 }));
    }

    #[test]
    fn device_colors_by_count_with_no_partial_reading() {
        let doc = Document::new_empty();
        let r = NumberReader::new(&doc);
        assert_eq!(device_color(&r, &parse(b"[0.5]")), Ok(DeviceColor::Gray(0.5)));
        assert_eq!(device_color(&r, &parse(b"[1 0 0]")), Ok(DeviceColor::Rgb([1.0, 0.0, 0.0])));
        assert_eq!(device_color(&r, &parse(b"[0 1 0 0.5]")), Ok(DeviceColor::Cmyk([0.0, 1.0, 0.0, 0.5])));
        for (text, n) in [(&b"[]"[..], 0), (b"[1 2]", 2), (b"[1 2 3 4 5]", 5)] {
            assert_eq!(device_color(&r, &parse(text)), Err(ColorError::Components(n)));
        }
        assert_eq!(
            device_color(&r, &parse(b"[null 1 0 0]")),
            Err(ColorError::Array(ArrayError::InvalidEntry { index: 0, error: NumberError::NotNumber })),
            "not RGB from the last three"
        );
        assert_eq!(device_color(&r, &parse(b"[null 0.5]")), Err(ColorError::Components(2)), "not gray from the second");
        assert_eq!(device_color(&r, &Object::Int(1)), Err(ColorError::Array(ArrayError::NotArray)));
        let mut doc = Document::new_empty();
        let g = doc.add(Object::Real(0.25));
        let inner = doc.add(Object::Ref(g));
        let arr = doc.add(Object::Array(vec![Object::Ref(inner)]));
        assert_eq!(device_color(&NumberReader::new(&doc), &Object::Ref(arr)), Ok(DeviceColor::Gray(0.25)), "chains");
    }

    #[test]
    fn rgb_converts_without_clamping_and_checks_finite_output() {
        assert_eq!(DeviceColor::Gray(0.25).rgb(), Some([0.25; 3]));
        assert_eq!(DeviceColor::Rgb([2.0, -1.0, 0.5]).rgb(), Some([2.0, -1.0, 0.5]), "no clamp");
        assert_eq!(DeviceColor::Cmyk([0.0, 1.0, 0.5, 0.5]).rgb(), Some([0.5, 0.0, 0.25]));
        assert_eq!(DeviceColor::Cmyk([-f64::MAX, 0.0, 0.0, -f64::MAX]).rgb(), None, "overflow");
    }
}
