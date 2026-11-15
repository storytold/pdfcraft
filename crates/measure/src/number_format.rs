//! Number format arrays (ISO 32000-2 §12.9.2, number format dictionaries).
//!
//! A rectilinear measure dictionary's `X`, `Y`, `D` and `A` entries are arrays of number format
//! dictionaries. The first element's `C` converts to the largest unit; each later element's `C`
//! is how many of its (smaller) unit make one of the previous element's. A value shows as whole
//! units of every element but the last, then what is left in the last unit, shown as its `F`
//! says: decimals to `1/D` (`/D`), a fraction in `D`ths (`/F`), or whole units rounded (`/R`) or
//! cut off (`/T`). With miles, feet, and inches in eighths, 1.4505 mi shows as
//! "1 mi 2,378 ft 7 5/8 in".

use pdfcraft_cos::{Dict, Document, Object, PdfString};
use serde::Serialize;

/// Most elements one number format array is read with (miles, yards, feet and inches are four).
pub const MAX_UNITS: usize = 8;
/// Longest unit label or separator, in characters, as for a scale's own unit labels.
const MAX_TEXT: usize = 24;
/// Largest denominator of a fraction (`F /F`).
const MAX_DENOMINATOR: i64 = 1 << 16;
/// A remainder this close below a whole step of the last unit, in steps, counts as the step:
/// `3.5 ft` stored as `3.4999999999 ft` still cuts to 6 in, not 5 in.
const NUDGE: f64 = 1e-6;

/// How the part of the last unit below a whole one shows (`F`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Remainder {
    /// `/D`: decimals, to `1 / denominator`.
    Decimal,
    /// `/F`: a fraction in `denominator`ths.
    Fraction,
    /// `/R`: rounded to whole units.
    Round,
    /// `/T`: cut off to whole units.
    Truncate,
}

/// One number format dictionary.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NumberFormat {
    /// `U`: the unit's label.
    pub unit: String,
    /// `C`: this unit in one of the previous element's unit (or, for the first element, in what
    /// the measure dictionary's entry converts from).
    pub factor: f64,
    /// `F`.
    pub remainder: Remainder,
    /// `D`: the decimal precision (a power of ten) or the fraction's denominator; 1 for whole units.
    pub denominator: i64,
    /// `FD`: decimals keep their trailing zeros and fractions are not reduced.
    pub exact: bool,
    /// `RT`: between groups of thousands.
    pub thousands: String,
    /// `RD`: the decimal point.
    pub decimal: String,
    /// `PS`: before the label.
    pub before: String,
    /// `SS`: after the label.
    pub after: String,
    /// `O /P`: the label goes before the number instead of after it.
    pub prefix: bool,
}

impl NumberFormat {
    /// Steps of this unit counted by its remainder: `1/denominator` for decimals and fractions.
    fn steps(&self) -> f64 {
        match self.remainder {
            Remainder::Decimal | Remainder::Fraction => self.denominator.max(1) as f64,
            Remainder::Round | Remainder::Truncate => 1.0,
        }
    }

    /// `count` steps of this unit (whole, finite and not negative) as text.
    fn number(&self, count: f64) -> String {
        let denominator = self.denominator.max(1);
        match self.remainder {
            Remainder::Round | Remainder::Truncate => grouped(&format!("{count:.0}"), &self.thousands),
            Remainder::Decimal => {
                let places = denominator.ilog10() as usize;
                let text = format!("{:.places$}", count / denominator as f64);
                let (whole, part) = text.split_once('.').unwrap_or((&text, ""));
                let part = if self.exact { part } else { part.trim_end_matches('0') };
                let whole = grouped(whole, &self.thousands);
                if part.is_empty() { whole } else { format!("{whole}{}{part}", self.decimal) }
            }
            Remainder::Fraction => {
                // The numerator is below the denominator however large and inexact the count is.
                let numerator = (count.rem_euclid(denominator as f64) as i64).clamp(0, denominator - 1);
                let whole = ((count - numerator as f64) / denominator as f64).round().max(0.0);
                let text = grouped(&format!("{whole:.0}"), &self.thousands);
                if numerator == 0 {
                    return text;
                }
                let g = if self.exact { 1 } else { gcd(numerator, denominator) };
                let fraction = format!("{}/{}", numerator / g, denominator / g);
                if whole == 0.0 { fraction } else { format!("{text} {fraction}") }
            }
        }
    }

    /// This format as a number format dictionary, every entry written out.
    pub(crate) fn dictionary(&self) -> Object {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("NumberFormat"));
        d.set(b"U".to_vec(), PdfString::text(&self.unit));
        d.set(b"C".to_vec(), Object::Real(self.factor));
        let f = match self.remainder {
            Remainder::Decimal => "D",
            Remainder::Fraction => "F",
            Remainder::Round => "R",
            Remainder::Truncate => "T",
        };
        d.set(b"F".to_vec(), Object::name(f));
        if matches!(self.remainder, Remainder::Decimal | Remainder::Fraction) {
            d.set(b"D".to_vec(), Object::Int(self.denominator));
        }
        d.set(b"FD".to_vec(), Object::Bool(self.exact));
        d.set(b"RT".to_vec(), PdfString::text(&self.thousands));
        d.set(b"RD".to_vec(), PdfString::text(&self.decimal));
        d.set(b"PS".to_vec(), PdfString::text(&self.before));
        d.set(b"SS".to_vec(), PdfString::text(&self.after));
        d.set(b"O".to_vec(), Object::name(if self.prefix { "P" } else { "S" }));
        Object::Dict(d)
    }
}

/// The number format arrays of an imported scale that a single-unit decimal scale can't stand
/// for: compound units (feet and inches), fractions, or values cut off to whole units.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Formats {
    /// `X`: changes along the x axis (and the y axis when there is no `Y`).
    pub x: Vec<NumberFormat>,
    /// `Y`: changes along the y axis, in its own units, as read.
    pub y: Option<Vec<NumberFormat>>,
    /// `CYX`: y-axis units in one x-axis unit, as read with `y` (1 without it).
    pub cyx: f64,
    /// `D`: distances.
    pub distance: Vec<NumberFormat>,
    /// `A`: areas.
    pub area: Vec<NumberFormat>,
}

impl Formats {
    /// Every array, `y` after `x` when there is one.
    fn arrays(&self) -> impl Iterator<Item = &Vec<NumberFormat>> {
        [&self.x, &self.distance, &self.area].into_iter().chain(self.y.as_ref())
    }

    /// Checks every array as [`crate::Scale::validate`] checks a scale's own fields.
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if !self.cyx.is_finite() || !(1e-12..=1e12).contains(&self.cyx) {
            return Err("scale factors must be finite and between 1e-12 and 1e12");
        }
        self.arrays().try_for_each(|a| validate(a))
    }

    /// Bytes held, elements and their text, for a listing's bound on what it keeps.
    pub(crate) fn held_bytes(&self) -> usize {
        self.arrays()
            .flatten()
            .flat_map(|f| [&f.unit, &f.thousands, &f.decimal, &f.before, &f.after])
            .fold(0usize, |n, s| n.saturating_add(s.len()))
            .saturating_add(self.arrays().map(Vec::len).sum::<usize>().saturating_mul(std::mem::size_of::<NumberFormat>()))
    }
}

/// The decimal places of a single-element decimal (`/D`) or rounded (`/R`) array: what a
/// single-unit decimal scale's `precision` describes. `None` for anything else.
pub(crate) fn plain_precision(formats: &[NumberFormat]) -> Option<u8> {
    match formats {
        [f] if f.remainder == Remainder::Decimal => u8::try_from(f.denominator.max(1).ilog10()).ok(),
        [f] if f.remainder == Remainder::Round => Some(0),
        _ => None,
    }
}

/// Reads the number format array `object` (resolving references), refusing a structure it can't
/// read or a precision it can't show. Labels, separators and later units' factors are checked
/// only when they are shown ([`Formats::validate`]): a single-unit decimal array is read for its
/// unit, factor and precision alone, as it always was.
pub(crate) fn read(doc: &Document, object: &Object) -> Result<Vec<NumberFormat>, &'static str> {
    let a = doc.resolve(object);
    let a = a.as_array().ok_or("invalid measurement number format")?;
    if a.is_empty() {
        return Err("empty number format");
    }
    if a.len() > MAX_UNITS {
        return Err("number formats of more than 8 units are not supported");
    }
    a.iter().enumerate().map(|(i, o)| read_one(doc, &doc.resolve(o), i + 1 == a.len())).collect()
}

fn read_one(doc: &Document, o: &Object, is_last: bool) -> Result<NumberFormat, &'static str> {
    let f = o.as_dict().ok_or("invalid number format dictionary")?;
    let get = |k: &[u8]| f.get(k).map(|v| doc.resolve(v));
    let text = |k: &[u8], default: &str| get(k).and_then(|v| v.as_string().map(PdfString::to_text)).unwrap_or_else(|| default.into());
    // ISO 32000-2 §12.9.2: only the last element formats a remainder (`/F`, `/D`, `/FD`);
    // earlier elements always format whole units.
    let (remainder, denominator, exact) = if is_last {
        let remainder = match get(b"F").as_deref().and_then(Object::as_name) {
            None | Some(b"D") => Remainder::Decimal,
            Some(b"F") => Remainder::Fraction,
            Some(b"R") => Remainder::Round,
            Some(b"T") => Remainder::Truncate,
            Some(_) => return Err("unknown measurement number format"),
        };
        let denominator = match remainder {
            Remainder::Decimal => get(b"D").and_then(|v| v.as_int()).unwrap_or(100),
            Remainder::Fraction => get(b"D").and_then(|v| v.as_int()).unwrap_or(16),
            Remainder::Round | Remainder::Truncate => 1,
        };
        let denominator_ok = match remainder {
            Remainder::Decimal => (0..=6).any(|p| 10_i64.pow(p) == denominator),
            Remainder::Fraction => (1..=MAX_DENOMINATOR).contains(&denominator),
            Remainder::Round | Remainder::Truncate => true,
        };
        if !denominator_ok {
            return Err("unsupported measurement precision");
        }
        (remainder, denominator, matches!(get(b"FD").as_deref(), Some(Object::Bool(true))))
    } else {
        (Remainder::Decimal, 100, false)
    };
    // An empty RD means the default decimal point.
    let decimal = Some(text(b"RD", ".")).filter(|s| !s.is_empty()).unwrap_or_else(|| ".".into());
    Ok(NumberFormat {
        unit: text(b"U", ""),
        factor: get(b"C").and_then(|v| v.as_f64()).ok_or("missing conversion factor")?,
        remainder,
        denominator,
        exact,
        thousands: text(b"RT", ","),
        decimal,
        before: text(b"PS", " "),
        after: text(b"SS", " "),
        prefix: get(b"O").as_deref().and_then(Object::as_name) == Some(b"P".as_slice()),
    })
}

/// Checks an array: 1 to [`MAX_UNITS`] elements, factors as a scale's, labels and separators of
/// at most 24 printable characters, and on the last element a decimal precision of 0 to 6 places
/// or a fraction denominator of 1 to 65,536.
fn validate(formats: &[NumberFormat]) -> Result<(), &'static str> {
    if formats.is_empty() || formats.len() > MAX_UNITS {
        return Err("number formats have 1 to 8 units");
    }
    for (i, f) in formats.iter().enumerate() {
        if !f.factor.is_finite() || !(1e-12..=1e12).contains(&f.factor) {
            return Err("scale factors must be finite and between 1e-12 and 1e12");
        }
        if f.unit.trim().is_empty() || f.unit.chars().count() > MAX_TEXT || f.unit.chars().any(char::is_control) {
            return Err("unit labels must be 1 to 24 printable characters");
        }
        if [&f.thousands, &f.decimal, &f.before, &f.after].iter().any(|s| s.chars().count() > MAX_TEXT || s.chars().any(char::is_control)) {
            return Err("number format separators must be at most 24 printable characters");
        }
        if i + 1 == formats.len() {
            let denominator_ok = match f.remainder {
                Remainder::Decimal => (0..=6).any(|p| 10_i64.pow(p) == f.denominator),
                Remainder::Fraction => (1..=MAX_DENOMINATOR).contains(&f.denominator),
                Remainder::Round | Remainder::Truncate => f.denominator == 1,
            };
            if !denominator_ok {
                return Err("unsupported measurement precision");
            }
        }
    }
    Ok(())
}

/// `value`, in the first element's unit, as `formats` show it (ISO 32000-2 §12.9.2): whole units
/// of every element but the last, then the rest in the last element's unit. A last unit that
/// rounds up to a whole one of the unit before carries into it ("1 ft 0 in", not "0 ft 12 in").
/// Not-a-number and infinite values show as nothing.
pub(crate) fn format(value: f64, formats: &[NumberFormat]) -> String {
    let Some(last) = formats.last() else { return String::new() };
    if !value.is_finite() {
        return String::new();
    }
    let steps = last.steps();
    let mut wholes = Vec::with_capacity(formats.len());
    let mut rest = value.abs();
    for (i, next) in formats.iter().enumerate().skip(1) {
        // The nudge is a fraction of a step of the last unit, however many units lie between.
        let per_step = formats.iter().skip(i).fold(steps, |p, f| p * f.factor);
        let nudge = if per_step.is_finite() && per_step > 1.0 { NUDGE / per_step } else { NUDGE };
        let whole = (rest + nudge).floor();
        wholes.push(whole);
        rest = ((rest - whole) * next.factor).max(0.0);
    }
    let mut count = if last.remainder == Remainder::Truncate { (rest * steps + NUDGE).floor() } else { (rest * steps).round() };
    carry(&mut count, steps, &mut wholes, formats);
    let mut out = String::new();
    if value < 0.0 && (count > 0.0 || wholes.iter().any(|w| *w > 0.0)) {
        out.push('-');
    }
    for (whole, f) in wholes.iter().zip(formats) {
        push_segment(&mut out, &grouped(&format!("{whole:.0}"), &f.thousands), f);
    }
    push_segment(&mut out, &last.number(count), last);
    out.trim().to_string()
}

/// Carries a last unit's `count` of `steps` that reached a whole one of the unit before into
/// it, and then each whole count of a unit into the unit before that. Only whole conversion
/// factors carry: a unit that isn't a whole number of the next has no whole count to carry.
fn carry(count: &mut f64, steps: f64, wholes: &mut [f64], formats: &[NumberFormat]) {
    let full = formats.last().map_or(0.0, |f| f.factor * steps);
    let Some(previous) = wholes.last_mut() else { return };
    if !is_whole(full) || *count < full {
        return;
    }
    *count -= full;
    *previous += 1.0;
    for i in (1..wholes.len()).rev() {
        let per = formats.get(i).map_or(0.0, |f| f.factor);
        let (Some(&w), true) = (wholes.get(i), is_whole(per)) else { continue };
        if w >= per
            && let Some([before, here]) = wholes.get_mut(i - 1..=i)
        {
            *here -= per;
            *before += 1.0;
        }
    }
}

fn is_whole(v: f64) -> bool {
    v.is_finite() && v >= 1.0 && v.fract() == 0.0
}

fn push_segment(out: &mut String, number: &str, f: &NumberFormat) {
    let label = [f.before.as_str(), &f.unit, &f.after].concat();
    if f.prefix {
        out.push_str(&label);
        out.push_str(number);
    } else {
        out.push_str(number);
        out.push_str(&label);
    }
}

/// The ASCII digits of a whole number with `separator` between groups of three, from the right.
fn grouped(digits: &str, separator: &str) -> String {
    let n = digits.len();
    let mut out = String::with_capacity(n + n / 3 * separator.len());
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (n - i).is_multiple_of(3) {
            out.push_str(separator);
        }
        out.push(c);
    }
    out
}

fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn unit(unit: &str, factor: f64, remainder: Remainder, denominator: i64) -> NumberFormat {
        NumberFormat {
            unit: unit.into(),
            factor,
            remainder,
            denominator,
            exact: false,
            thousands: ",".into(),
            decimal: ".".into(),
            before: " ".into(),
            after: " ".into(),
            prefix: false,
        }
    }

    fn feet_inches(denominator: i64) -> Vec<NumberFormat> {
        vec![unit("ft", 1.0 / 864.0, Remainder::Decimal, 100), unit("in", 12.0, Remainder::Fraction, denominator)]
    }

    /// A file of a catalog, an empty page tree and `objects`, numbered from 3.
    fn doc_with(objects: &[&str]) -> Document {
        let pdf = pdfcraft_testkit::Pdf::new().obj("<< /Type /Catalog /Pages 2 0 R >>").obj("<< /Type /Pages /Kids [] /Count 0 >>").objs(objects);
        Document::open(Arc::new(pdf.build())).unwrap()
    }

    /// Reads `array`, stored by reference as the object after `objects` (numbered from 3).
    fn parsed(objects: &[&str], array: &str) -> Result<Vec<NumberFormat>, &'static str> {
        let doc = doc_with(&[objects, &[array]].concat());
        let num = u32::try_from(objects.len() + 3).unwrap();
        read(&doc, &Object::Ref(pdfcraft_cos::ObjRef { num, generation: 0 }))
    }

    /// The miles, feet and eighths of inches that show "1 mi 2,378 ft 7 5/8 in".
    #[test]
    fn compound_units_show_whole_units_then_the_rest() {
        let formats =
            vec![unit("mi", 1.0, Remainder::Decimal, 100), unit("ft", 5280.0, Remainder::Decimal, 100), unit("in", 12.0, Remainder::Fraction, 8)];
        assert_eq!(format(1.4505, &formats), "1 mi 2,378 ft 7 5/8 in");
        // A remainder reduces (4/8 is 1/2) unless FD keeps its denominator.
        assert_eq!(format(3.0 + 6.5 / 12.0, &feet_inches(8)), "3 ft 6 1/2 in");
        let mut exact = feet_inches(8);
        exact[1].exact = true;
        assert_eq!(format(3.0 + 6.5 / 12.0, &exact), "3 ft 6 4/8 in");
        // No fractional part: the whole number alone; under one unit: the fraction alone.
        assert_eq!(format(2.5, &feet_inches(16)), "2 ft 6 in");
        assert_eq!(format(1.0 / 96.0, &feet_inches(16)), "0 ft 1/8 in");
    }

    /// A remainder that rounds up to a whole one of the unit before carries into it, through
    /// every unit that becomes whole in turn.
    #[test]
    fn a_remainder_that_rounds_up_carries() {
        assert_eq!(format(1.0 - 1e-4, &feet_inches(16)), "1 ft 0 in");
        let formats = vec![unit("yd", 1.0, Remainder::Decimal, 100), unit("ft", 3.0, Remainder::Decimal, 100), unit("in", 12.0, Remainder::Round, 1)];
        assert_eq!(format(2.0 - 1e-4, &formats), "2 yd 0 ft 0 in");
        // A factor that isn't whole has no whole count to carry: the rounded rest stays.
        let odd = vec![unit("a", 1.0, Remainder::Decimal, 100), unit("b", 2.5, Remainder::Round, 1)];
        assert_eq!(format(1.9, &odd), "1 a 2 b");
        // A whole unit stored a hair below itself is still whole, factor whole or not.
        assert_eq!(format(2.0 - 1e-10, &odd), "2 a 0 b");
        // ... but a value a step of the last unit short of it is not, however small the step.
        let metres = vec![unit("km", 1.0, Remainder::Decimal, 100), unit("m", 1000.0, Remainder::Decimal, 1000)];
        assert_eq!(format(1.0 - 9e-7, &metres), "0 km 999.999 m");
        let miles =
            vec![unit("mi", 1.0, Remainder::Decimal, 100), unit("ft", 5280.0, Remainder::Decimal, 100), unit("in", 12.0, Remainder::Fraction, 64)];
        assert_eq!(format(1.0 - 9e-7, &miles), "0 mi 5,279 ft 11 15/16 in");
        // A value stored a hair below a whole step still cuts to it.
        let cut = vec![unit("ft", 1.0, Remainder::Decimal, 100), unit("in", 12.0, Remainder::Truncate, 1)];
        assert_eq!(format(3.4999999999, &cut), "3 ft 6 in");
        assert_eq!(format(3.49, &cut), "3 ft 5 in");
    }

    /// Single-unit decimals, rounding and cutting, with the spec's separators and label position.
    #[test]
    fn decimals_separators_and_label_position() {
        let mut decimal = unit("m", 1.0, Remainder::Decimal, 1000);
        assert_eq!(format(1234.5, std::slice::from_ref(&decimal)), "1,234.5 m");
        decimal.exact = true;
        assert_eq!(format(1234.5, std::slice::from_ref(&decimal)), "1,234.500 m");
        let euro = NumberFormat { thousands: ".".into(), decimal: ",".into(), ..decimal.clone() };
        assert_eq!(format(1_234_567.25, std::slice::from_ref(&euro)), "1.234.567,250 m");
        assert_eq!(format(7.0, &[unit("m", 1.0, Remainder::Decimal, 100)]), "7 m");
        assert_eq!(format(7.6, &[unit("m", 1.0, Remainder::Round, 1)]), "8 m");
        assert_eq!(format(7.6, &[unit("m", 1.0, Remainder::Truncate, 1)]), "7 m");
        let prefixed = NumberFormat { prefix: true, before: String::new(), after: String::new(), ..unit("$", 1.0, Remainder::Decimal, 100) };
        assert_eq!(format(12.5, &[prefixed]), "$12.5");
        let tight = vec![
            NumberFormat { before: String::new(), after: String::new(), ..unit("'", 1.0, Remainder::Decimal, 100) },
            NumberFormat { before: String::new(), after: String::new(), ..unit("\"", 12.0, Remainder::Fraction, 4) },
        ];
        assert_eq!(format(12.0 + 3.25 / 12.0, &tight), "12'3 1/4\"");
        // Negative values keep their sign; nothing shows for none and for non-finite values.
        assert_eq!(format(-2.5, &feet_inches(16)), "-2 ft 6 in");
        assert_eq!(format(-0.0, &feet_inches(16)), "0 ft 0 in");
        assert_eq!(format(1.0, &[]), "");
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(format(bad, &feet_inches(16)), "");
        }
        // The largest values a scale allows show in full, without an exponent.
        let huge = format(1e30, &feet_inches(16));
        assert!(huge.starts_with("1,000,000,000,000,000,019,884,624,838,656 ft"), "{huge}");
        // A fraction of a value too large to be exact still has a numerator below its denominator.
        let fraction = format(1e30, &[unit("in", 1.0, Remainder::Fraction, 3)]);
        let tokens: Vec<&str> = fraction.split_whitespace().collect();
        assert!(matches!(tokens.as_slice(), [_, "in"] | [_, "1/3" | "2/3", "in"]), "{fraction}");
    }

    /// Every entry is read with its default, references resolved, and written back as read.
    #[test]
    fn arrays_read_with_defaults_and_write_back() {
        let source = "[<< /U (ft) /C 0.5 >> << /U 3 0 R /C 4 0 R /F 5 0 R /D 8 /FD true /RT (.) /RD () /PS () /SS (-) /O /P >>]";
        let read_back = parsed(&["(in)", "12", "/F"], source).unwrap();
        assert_eq!(read_back[0], unit("ft", 0.5, Remainder::Decimal, 100));
        let inches = &read_back[1];
        assert_eq!((inches.unit.as_str(), inches.factor, inches.remainder, inches.denominator), ("in", 12.0, Remainder::Fraction, 8));
        assert!(inches.exact && inches.prefix);
        assert_eq!((inches.thousands.as_str(), inches.decimal.as_str(), inches.before.as_str(), inches.after.as_str()), (".", ".", "", "-"));
        assert_eq!(parsed(&[], "[<< /U (in) /C 1 /F /F >>]").unwrap()[0].denominator, 16);
        // Written out and read again, nothing changes.
        let written = Object::Array(read_back.iter().map(NumberFormat::dictionary).collect());
        assert_eq!(read(&doc_with(&[]), &written).unwrap(), read_back);
        assert_eq!(plain_precision(&read_back), None);
        assert_eq!(plain_precision(&read_back[..1]), Some(2));
        assert_eq!(plain_precision(&[unit("m", 1.0, Remainder::Round, 1)]), Some(0));
        assert_eq!(plain_precision(&[unit("m", 1.0, Remainder::Fraction, 16)]), None);
        assert_eq!(plain_precision(&[unit("m", 1.0, Remainder::Truncate, 1)]), None);
    }

    /// What can't be shown faithfully is refused with a reason, before anything is formatted.
    #[test]
    fn unreadable_arrays_are_refused() {
        let units = |n: usize| format!("[{}]", vec!["<< /U (u) /C 1 >>"; n].join(" "));
        let nine = units(9);
        for (source, reason) in [
            ("[]", "empty number format"),
            ("<< /U (ft) /C 1 >>", "invalid measurement number format"),
            (nine.as_str(), "number formats of more than 8 units are not supported"),
            ("[(ft)]", "invalid number format dictionary"),
            ("[<< /U (ft) /C 1 /F /X >>]", "unknown measurement number format"),
            ("[<< /U (ft) >>]", "missing conversion factor"),
            ("[<< /U (ft) /C 0 >>]", "scale factors must be finite and between 1e-12 and 1e12"),
            ("[<< /C 1 >>]", "unit labels must be 1 to 24 printable characters"),
            ("[<< /U (ft) /C 1 /D 12 >>]", "unsupported measurement precision"),
            ("[<< /U (ft) /C 1 /D 10000000 >>]", "unsupported measurement precision"),
            ("[<< /U (ft) /C 1 /F /F /D 0 >>]", "unsupported measurement precision"),
            ("[<< /U (ft) /C 1 /F /F /D 65537 >>]", "unsupported measurement precision"),
            ("[<< /U (ft) /C 1 /SS (aaaaaaaaaaaaaaaaaaaaaaaaa) >>]", "number format separators must be at most 24 printable characters"),
        ] {
            assert_eq!(parsed(&[], source).and_then(|f| validate(&f).map(|()| f)), Err(reason), "{source}");
        }
        assert_eq!(parsed(&[], "[<< /U (ft) /C 1 /F /F /D 65536 >>]").unwrap()[0].denominator, 65536);
        // Only the last element's `/F` and `/D` format a remainder; earlier elements' `/F` and `/D` are ignored.
        assert_eq!(parsed(&[], "[<< /U (ft) /C 1 /F /X /D 12 >> << /U (in) /C 12 /F /F /D 16 >>]").map(|f| f.len()), Ok(2));
        assert_eq!(parsed(&[], &units(MAX_UNITS)).map(|f| f.len()), Ok(MAX_UNITS));
    }

    /// What formats hold is counted for the listing bound.
    #[test]
    fn formats_count_their_text() {
        let f = Formats {
            x: feet_inches(16),
            y: Some(feet_inches(16)),
            cyx: 1.0,
            distance: feet_inches(16),
            area: vec![unit("ft²", 1.0, Remainder::Decimal, 100)],
        };
        // Each element holds its label and "," "." " " " " (4 bytes).
        let elements = 3 * 2 + 1;
        assert_eq!(f.held_bytes(), 3 * ((2 + 4) + (2 + 4)) + ("ft²".len() + 4) + elements * std::mem::size_of::<NumberFormat>());
        assert_eq!(f.validate(), Ok(()));
        let bad = Formats { cyx: 0.0, ..f.clone() };
        assert_eq!(bad.validate(), Err("scale factors must be finite and between 1e-12 and 1e12"));
        let bad = Formats { y: Some(Vec::new()), ..f };
        assert_eq!(bad.validate(), Err("number formats have 1 to 8 units"));
    }
}
