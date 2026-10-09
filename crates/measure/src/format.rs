//! ISO 32000 number format arrays. Conversion and presentation are separate:
//! the first C converts to the largest unit; later C values convert remainders.
use crate::{Result, invalid};
use pdfcraft_cos::{Dict, Document, Object, PdfString};
use serde::{Deserialize, Serialize};

const MAX_FORMATS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fraction {
    Decimal,
    Fraction,
    Round,
    Truncate,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NumberFormat {
    pub unit: String,
    pub factor: f64,
    pub fraction: Fraction,
    pub denominator: u32,
    pub fixed: bool,
    pub thousands: String,
    pub decimal: String,
    pub prefix: String,
    pub suffix: String,
    pub unit_first: bool,
    #[serde(skip)]
    pub source: Option<Dict>,
}
impl NumberFormat {
    pub fn decimal(unit: &str, factor: f64, precision: u8) -> Self {
        Self {
            unit: unit.into(),
            factor,
            fraction: if precision == 0 { Fraction::Round } else { Fraction::Decimal },
            denominator: 10_u32.pow(u32::from(precision.min(9))),
            fixed: true,
            thousands: String::new(),
            decimal: ".".into(),
            prefix: " ".into(),
            suffix: String::new(),
            unit_first: false,
            source: None,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if !self.factor.is_finite() || !(1e-12..=1e12).contains(&self.factor) {
            return Err(invalid("number format conversion must be finite and between 1e-12 and 1e12"));
        }
        if self.unit.trim().is_empty() || self.unit.chars().count() > 24 || self.unit.chars().any(char::is_control) {
            return Err(invalid("number format units must be 1 to 24 printable characters"));
        }
        if self.denominator == 0 || self.denominator > 1_000_000_000 {
            return Err(invalid("number format denominator must be between 1 and 1000000000"));
        }
        if self.fraction == Fraction::Decimal && !self.denominator.is_multiple_of(10) {
            return Err(invalid("decimal precision must be a positive multiple of 10"));
        }
        for s in [&self.thousands, &self.decimal, &self.prefix, &self.suffix] {
            if s.chars().count() > 24 || s.chars().any(char::is_control) {
                return Err(invalid("number format separators must have at most 24 printable characters"));
            }
        }
        Ok(())
    }
    fn read(doc: &Document, object: &Object) -> Result<Self> {
        let object = doc.resolve(object);
        let d = object.as_dict().ok_or_else(|| invalid("invalid number format dictionary"))?;
        let text = |key: &[u8], default: &str| -> Result<String> {
            match d.get(key) {
                None => Ok(default.into()),
                Some(o) => {
                    doc.resolve(o).as_string().map(PdfString::to_text).ok_or_else(|| invalid("number format label or separator is not a text string"))
                }
            }
        };
        let name = |key: &[u8], default: &[u8]| -> Result<Vec<u8>> {
            match d.get(key) {
                None => Ok(default.to_vec()),
                Some(o) => doc.resolve(o).as_name().map(<[u8]>::to_vec).ok_or_else(|| invalid("invalid number format name")),
            }
        };
        let fraction = match name(b"F", b"D")?.as_slice() {
            b"D" => Fraction::Decimal,
            b"F" => Fraction::Fraction,
            b"R" => Fraction::Round,
            b"T" => Fraction::Truncate,
            _ => return Err(invalid("number format F must be D, F, R or T")),
        };
        let denominator = match d.get(b"D") {
            Some(o) => u32::try_from(doc.resolve(o).as_int().ok_or_else(|| invalid("number format denominator must be an integer"))?)
                .map_err(|_| invalid("invalid number format denominator"))?,
            None => {
                if fraction == Fraction::Fraction {
                    16
                } else {
                    100
                }
            }
        };
        let unit_first = match name(b"O", b"S")?.as_slice() {
            b"P" => true,
            b"S" => false,
            _ => return Err(invalid("number format O must be P or S")),
        };
        let fixed = match d.get(b"FD") {
            None => false,
            Some(o) => match doc.resolve(o).as_ref() {
                Object::Bool(v) => *v,
                _ => return Err(invalid("number format FD must be boolean")),
            },
        };
        let decimal = text(b"RD", ".")?;
        let out = Self {
            unit: text(b"U", "")?,
            factor: d.get(b"C").map(|o| doc.resolve(o)).and_then(|o| o.as_f64()).ok_or_else(|| invalid("missing number format conversion factor"))?,
            fraction,
            denominator,
            fixed,
            thousands: text(b"RT", ",")?,
            decimal: if decimal.is_empty() { ".".into() } else { decimal },
            prefix: text(b"PS", " ")?,
            suffix: text(b"SS", " ")?,
            unit_first,
            source: Some(d.clone()),
        };
        out.validate()?;
        Ok(out)
    }
    fn dictionary(&self) -> Result<Object> {
        self.validate()?;
        let mut d = self.source.clone().unwrap_or_default();
        d.set(b"Type".to_vec(), Object::name("NumberFormat"));
        for (key, value) in
            [(b"U".as_slice(), &self.unit), (b"RT", &self.thousands), (b"RD", &self.decimal), (b"PS", &self.prefix), (b"SS", &self.suffix)]
        {
            d.set(key.to_vec(), PdfString::text(value));
        }
        d.set(b"C".to_vec(), Object::Real(self.factor));
        d.set(
            b"F".to_vec(),
            Object::name(match self.fraction {
                Fraction::Decimal => "D",
                Fraction::Fraction => "F",
                Fraction::Round => "R",
                Fraction::Truncate => "T",
            }),
        );
        d.set(b"D".to_vec(), Object::Int(i64::from(self.denominator)));
        d.set(b"FD".to_vec(), Object::Bool(self.fixed));
        d.set(b"O".to_vec(), Object::name(if self.unit_first { "P" } else { "S" }));
        Ok(Object::Dict(d))
    }
    fn digits(&self) -> usize {
        let mut n = self.denominator;
        let (mut twos, mut fives) = (0, 0);
        while n.is_multiple_of(2) {
            n /= 2;
            twos += 1;
        }
        while n.is_multiple_of(5) {
            n /= 5;
            fives += 1;
        }
        if n == 1 { twos.max(fives) } else { 12 }
    }
    fn number(&self, value: f64, last: bool) -> String {
        let (integer, tail) = if !last {
            (format!("{:.0}", value.floor()), String::new())
        } else {
            match self.fraction {
                Fraction::Round => (format!("{:.0}", value.round()), String::new()),
                Fraction::Truncate => (format!("{:.0}", value.trunc()), String::new()),
                Fraction::Decimal => {
                    let mut n = format!("{value:.digits$}", digits = self.digits());
                    if !self.fixed && n.contains('.') {
                        while n.ends_with('0') {
                            n.pop();
                        }
                        if n.ends_with('.') {
                            n.pop();
                        }
                    }
                    let (whole, fractional) = n.split_once('.').unwrap_or((&n, ""));
                    (whole.to_string(), if fractional.is_empty() { String::new() } else { format!("{}{fractional}", self.decimal) })
                }
                Fraction::Fraction => {
                    let denominator = u64::from(self.denominator);
                    let total = (value * denominator as f64).round();
                    let whole = (total / denominator as f64).floor();
                    let mut numerator = (total - whole * denominator as f64).round().clamp(0.0, denominator as f64) as u64;
                    let mut denominator = denominator;
                    if !self.fixed && numerator > 0 {
                        let (mut a, mut b) = (numerator, denominator);
                        while b != 0 {
                            (a, b) = (b, a % b);
                        }
                        numerator /= a;
                        denominator /= a;
                    }
                    (format!("{whole:.0}"), if numerator == 0 { String::new() } else { format!(" {numerator}/{denominator}") })
                }
            }
        };
        let mut grouped = String::new();
        for (i, c) in integer.chars().enumerate() {
            if i > 0 && (integer.len() - i).is_multiple_of(3) {
                grouped.push_str(&self.thousands);
            }
            grouped.push(c);
        }
        grouped.push_str(&tail);
        grouped
    }
}

pub fn read_array(doc: &Document, object: &Object) -> Result<Vec<NumberFormat>> {
    let object = doc.resolve(object);
    let a = object.as_array().ok_or_else(|| invalid("invalid measurement number format array"))?;
    if a.is_empty() || a.len() > MAX_FORMATS {
        return Err(invalid("a number format array needs 1 to 16 units"));
    }
    a.iter().map(|o| NumberFormat::read(doc, o)).collect()
}
pub fn dictionary(formats: &[NumberFormat]) -> Result<Object> {
    validate(formats)?;
    Ok(Object::Array(formats.iter().map(NumberFormat::dictionary).collect::<Result<_>>()?))
}
pub fn validate(formats: &[NumberFormat]) -> Result<()> {
    if formats.is_empty() || formats.len() > MAX_FORMATS {
        return Err(invalid("a number format array needs 1 to 16 units"));
    }
    for f in formats {
        f.validate()?;
    }
    let mut product = 1.0;
    for f in formats.iter().skip(1) {
        if f.factor < 1.0 {
            return Err(invalid("compound units must run from largest to smallest"));
        }
        product *= f.factor;
        if !product.is_finite() || product > 1e18 {
            return Err(invalid("compound conversion exceeds numeric precision"));
        }
    }
    Ok(())
}
/// Format a value that has already been converted to the first (largest) unit.
/// Quantize before splitting so rounding 11 63/64 inches carries into feet.
pub fn label(value: f64, formats: &[NumberFormat]) -> Result<String> {
    validate(formats)?;
    if !value.is_finite() || value.abs() > 1e24 {
        return Err(invalid("measurement label exceeds numeric range"));
    }
    let last = formats.last().ok_or_else(|| invalid("empty number format"))?;
    let conversion: f64 = formats.iter().skip(1).map(|f| f.factor).product();
    let precision = match last.fraction {
        Fraction::Decimal | Fraction::Fraction => f64::from(last.denominator),
        _ => 1.0,
    };
    let ticks = value.abs() * conversion * precision;
    if !ticks.is_finite() {
        return Err(invalid("measurement label overflows its precision"));
    }
    let ticks = if last.fraction == Fraction::Truncate { ticks.floor() } else { ticks.round() };
    let mut remainder = ticks / (conversion * precision);
    let mut out = if value < 0.0 { "-".to_string() } else { String::new() };
    for (i, f) in formats.iter().enumerate() {
        if i > 0 {
            remainder *= f.factor;
        }
        let last = i + 1 == formats.len();
        let number = f.number(remainder, last);
        let unit = format!("{}{}{}", f.prefix, f.unit, f.suffix);
        if f.unit_first {
            out.push_str(&unit);
            out.push_str(&number);
        } else {
            out.push_str(&number);
            out.push_str(&unit);
        }
        if last {
            break;
        }
        remainder -= remainder.floor();
        if remainder.abs() <= 1e-12 {
            break;
        }
    }
    if out.len() > 4096 {
        return Err(invalid("measurement label is too long"));
    }
    Ok(out.trim_end().to_string())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NumberFormats {
    pub x: Vec<NumberFormat>,
    pub y: Option<Vec<NumberFormat>>,
    pub distance: Vec<NumberFormat>,
    pub area: Vec<NumberFormat>,
    pub cyx: Option<f64>,
}
impl NumberFormats {
    pub fn validate(&self) -> Result<()> {
        for f in [&self.x, &self.distance, &self.area] {
            validate(f)?;
        }
        if let Some(y) = &self.y {
            validate(y)?;
            let c = self.cyx.ok_or_else(|| invalid("Y scale requires CYX for lengths and areas"))?;
            if !c.is_finite() || !(1e-12..=1e12).contains(&c) {
                return Err(invalid("invalid CYX conversion"));
            }
        }
        Ok(())
    }
}
