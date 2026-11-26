//! Engineering functions: base conversion, bit operations, unit conversion, error function,
//! complex numbers and Bessel functions.

use std::f64::consts::PI;

use gridcraft_core::{CellError, Value, number_to_text};

use crate::special::{erf, erfc};
use crate::util::{A, R, S, flatten, has, num, num_val, opt_num, scalar, text, text_val};
use crate::{Arg, Ctx, FnSpec, VAR};

// ---------------------------------------------------------------------------------------------
// Base conversion

/// Source text of a base-conversion argument (numbers are written out as integers).
fn base_source(a: &[Arg]) -> R<String> {
    let v = a.first().map(scalar).unwrap_or(Value::Empty);
    match v {
        Value::Error(e) => Err(e),
        Value::Bool(_) => Err(CellError::Value),
        Value::Number(n) => {
            if n < 0.0 || n != n.trunc() {
                return Err(CellError::Num);
            }
            Ok(number_to_text(n))
        }
        Value::Empty => Ok(String::new()),
        v => Ok(v.to_text()?.to_string()),
    }
}

/// Parses a 10-digit two's-complement number in `radix`.
fn parse_base(s: &str, radix: u32) -> R<i64> {
    if s.len() > 10 {
        return Err(CellError::Num);
    }
    if s.is_empty() {
        return Ok(0);
    }
    let v = i64::from_str_radix(s, radix).map_err(|_| CellError::Num)?;
    if s.starts_with(['+', '-']) {
        return Err(CellError::Num);
    }
    let bits = match radix {
        2 => 10,
        8 => 30,
        _ => 40,
    };
    // A full 10-digit number with the top bit set is negative.
    if s.len() == 10 && v >= 1i64 << (bits - 1) { Ok(v - (1i64 << bits)) } else { Ok(v) }
}

/// Writes `v` in `radix`, using 10-digit two's complement for negatives and padding to `places`.
fn format_base(v: i64, radix: u32, places: Option<f64>) -> R<Value> {
    let bits: u32 = match radix {
        2 => 10,
        8 => 30,
        _ => 40,
    };
    let min = -(1i64 << (bits - 1));
    let max = (1i64 << (bits - 1)) - 1;
    if v < min || v > max {
        return Err(CellError::Num);
    }
    let places = places
        .map(|p| {
            let p = p.trunc();
            if !(1.0..=10.0).contains(&p) {
                return Err(CellError::Num);
            }
            Ok(p as usize)
        })
        .transpose()?;
    if v < 0 {
        let u = v + (1i64 << bits);
        return text_val(to_radix(u, radix));
    }
    let s = to_radix(v, radix);
    match places {
        None => text_val(s),
        Some(p) => {
            if s.len() > p {
                return Err(CellError::Num);
            }
            text_val(format!("{}{}", "0".repeat(p - s.len()), s))
        }
    }
}

fn to_radix(mut v: i64, radix: u32) -> String {
    if v == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while v > 0 {
        let d = (v % radix as i64) as u32;
        out.push(char::from_digit(d, radix).unwrap_or('0').to_ascii_uppercase());
        v /= radix as i64;
    }
    out.iter().rev().collect()
}

fn places_arg(a: &[Arg]) -> R<Option<f64>> {
    if !has(a, 1) {
        return Ok(None);
    }
    if matches!(a.get(1).map(scalar), Some(Value::Bool(_))) {
        return Err(CellError::Value);
    }
    Ok(Some(num(a, 1)?))
}

fn convert_base(a: &[Arg], from: u32, to: u32) -> R<Value> {
    let s = base_source(a)?;
    let v = parse_base(&s, from)?;
    let places = places_arg(a)?;
    if to == 10 {
        return Ok(Value::Number(v as f64));
    }
    format_base(v, to, places)
}

fn dec_to(a: &[Arg], to: u32) -> R<Value> {
    let v = a.first().map(scalar).unwrap_or(Value::Empty);
    if matches!(v, Value::Bool(_)) {
        return Err(CellError::Value);
    }
    let n = v.to_number()?.trunc();
    if n.abs() > 1e15 {
        return Err(CellError::Num);
    }
    format_base(n as i64, to, places_arg(a)?)
}

macro_rules! base_fn {
    ($name:ident, $from:expr, $to:expr) => {
        fn $name(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
            convert_base(a, $from, $to)
        }
    };
}
base_fn!(bin2dec, 2, 10);
base_fn!(bin2hex, 2, 16);
base_fn!(bin2oct, 2, 8);
base_fn!(hex2bin, 16, 2);
base_fn!(hex2dec, 16, 10);
base_fn!(hex2oct, 16, 8);
base_fn!(oct2bin, 8, 2);
base_fn!(oct2dec, 8, 10);
base_fn!(oct2hex, 8, 16);

fn dec2bin(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    dec_to(a, 2)
}
fn dec2oct(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    dec_to(a, 8)
}
fn dec2hex(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    dec_to(a, 16)
}

// ---------------------------------------------------------------------------------------------
// Bits

const BIT_LIMIT: f64 = 281_474_976_710_656.0; // 2^48

fn bit_operand(a: &[Arg], i: usize) -> R<u64> {
    let n = num(a, i)?;
    if !(0.0..BIT_LIMIT).contains(&n) || n != n.trunc() {
        return Err(CellError::Num);
    }
    Ok(n as u64)
}

fn bitand(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    Ok(Value::Number((bit_operand(a, 0)? & bit_operand(a, 1)?) as f64))
}
fn bitor(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    Ok(Value::Number((bit_operand(a, 0)? | bit_operand(a, 1)?) as f64))
}
fn bitxor(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    Ok(Value::Number((bit_operand(a, 0)? ^ bit_operand(a, 1)?) as f64))
}

fn shift(a: &[Arg], left: bool) -> R<Value> {
    let n = bit_operand(a, 0)?;
    let s = num(a, 1)?.trunc();
    if n == 0 {
        return Ok(Value::Number(0.0));
    }
    if s.abs() > 53.0 {
        return Err(CellError::Num);
    }
    let s = if left { s as i64 } else { -(s as i64) };
    let r = if s >= 0 {
        let r = (n as u128) << s;
        if r >= BIT_LIMIT as u128 {
            return Err(CellError::Num);
        }
        r as u64
    } else {
        n >> (-s)
    };
    Ok(Value::Number(r as f64))
}
fn bitlshift(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    shift(a, true)
}
fn bitrshift(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    shift(a, false)
}

// ---------------------------------------------------------------------------------------------
// DELTA, GESTEP, ERF

fn delta(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = num(a, 0)?;
    let y = opt_num(a, 1, 0.0)?;
    Ok(Value::Number(if x == y { 1.0 } else { 0.0 }))
}
fn gestep(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = num(a, 0)?;
    let y = opt_num(a, 1, 0.0)?;
    Ok(Value::Number(if x >= y { 1.0 } else { 0.0 }))
}
fn erf_fn(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let lo = num(a, 0)?;
    if has(a, 1) {
        let hi = num(a, 1)?;
        return num_val(erf(hi) - erf(lo));
    }
    num_val(erf(lo))
}
fn erf_precise(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(erf(num(a, 0)?))
}
fn erfc_fn(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(erfc(num(a, 0)?))
}

macro_rules! im1 {
    ($name:ident, $f:expr) => {
        fn $name(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
            im_unary(a, $f)
        }
    };
}

mod bessel;
mod complex;
mod units;

use bessel::{besseli, besselj, besselk, bessely};
#[cfg(test)]
use complex::parse_complex;
use complex::{Cx, ONE, complex, im_unary, imabs, imaginary, imargument, imdiv, impower, improduct, imreal, imsub, imsum};
use units::convert;

im1!(imconjugate, |z: Cx| Ok(Cx::new(z.re, -z.im)));
im1!(imexp, |z: Cx| Ok(z.exp()));
im1!(imln, |z: Cx| z.ln());
im1!(imlog10, |z: Cx| z.ln().map(|l| Cx::new(l.re / std::f64::consts::LN_10, l.im / std::f64::consts::LN_10)));
im1!(imlog2, |z: Cx| z.ln().map(|l| Cx::new(l.re / std::f64::consts::LN_2, l.im / std::f64::consts::LN_2)));
// Excel takes the polar route, so IMSQRT("-4") is 1.22514845490862E-16+2i, not 2i. Its cosine
// reduces its argument with a 64-bit-mantissa pi, which is PI/2 + 2.5e-20 and shows up as the
// stray real part. `+ 0.0` turns a -0 imaginary part into +0, so the angle stays +pi.
// shortcut: parts above ~1E154 or below ~1E-155 keep the exact result (Excel gives #NUM! or 0), no known use.
const HALF_PI_EXCESS: f64 = 2.508_278_806_33e-20;
im1!(imsqrt, |z: Cx| {
    let im = z.im + 0.0;
    let theta = match z.re {
        0.0 => {
            if im == 0.0 {
                0.0
            } else {
                std::f64::consts::FRAC_PI_2.copysign(im)
            }
        }
        x if x > 0.0 => (im / x).atan(),
        x => (im / x).atan() + if im < 0.0 { -std::f64::consts::PI } else { std::f64::consts::PI },
    };
    let (half, root) = (theta / 2.0, z.abs().sqrt());
    Ok(Cx::new(root * (half.cos() + HALF_PI_EXCESS * half.abs().sin()), root * half.sin()))
});
im1!(imsin, |z: Cx| Ok(z.sin()));
im1!(imcos, |z: Cx| Ok(z.cos()));
im1!(imtan, |z: Cx| z.sin().div(z.cos()));
im1!(imcot, |z: Cx| z.cos().div(z.sin()));
im1!(imsec, |z: Cx| ONE.div(z.cos()));
im1!(imcsc, |z: Cx| ONE.div(z.sin()));
im1!(imsinh, |z: Cx| Ok(z.sinh()));
im1!(imcosh, |z: Cx| Ok(z.cosh()));
im1!(imsech, |z: Cx| ONE.div(z.cosh()));
im1!(imcsch, |z: Cx| ONE.div(z.sinh()));

pub(crate) fn specs() -> Vec<FnSpec> {
    vec![
        f!("BIN2DEC", 1, 1, Engineering, S, "BIN2DEC(number)", "Converts a binary number to decimal.", bin2dec),
        f!("BIN2HEX", 1, 2, Engineering, S, "BIN2HEX(number, [places])", "Converts a binary number to hexadecimal.", bin2hex),
        f!("BIN2OCT", 1, 2, Engineering, S, "BIN2OCT(number, [places])", "Converts a binary number to octal.", bin2oct),
        f!("DEC2BIN", 1, 2, Engineering, S, "DEC2BIN(number, [places])", "Converts a decimal number to binary.", dec2bin),
        f!("DEC2HEX", 1, 2, Engineering, S, "DEC2HEX(number, [places])", "Converts a decimal number to hexadecimal.", dec2hex),
        f!("DEC2OCT", 1, 2, Engineering, S, "DEC2OCT(number, [places])", "Converts a decimal number to octal.", dec2oct),
        f!("HEX2BIN", 1, 2, Engineering, S, "HEX2BIN(number, [places])", "Converts a hexadecimal number to binary.", hex2bin),
        f!("HEX2DEC", 1, 1, Engineering, S, "HEX2DEC(number)", "Converts a hexadecimal number to decimal.", hex2dec),
        f!("HEX2OCT", 1, 2, Engineering, S, "HEX2OCT(number, [places])", "Converts a hexadecimal number to octal.", hex2oct),
        f!("OCT2BIN", 1, 2, Engineering, S, "OCT2BIN(number, [places])", "Converts an octal number to binary.", oct2bin),
        f!("OCT2DEC", 1, 1, Engineering, S, "OCT2DEC(number)", "Converts an octal number to decimal.", oct2dec),
        f!("OCT2HEX", 1, 2, Engineering, S, "OCT2HEX(number, [places])", "Converts an octal number to hexadecimal.", oct2hex),
        f!("BITAND", 2, 2, Engineering, S, "BITAND(number1, number2)", "Bitwise AND of two non-negative integers.", bitand),
        f!("BITOR", 2, 2, Engineering, S, "BITOR(number1, number2)", "Bitwise OR of two non-negative integers.", bitor),
        f!("BITXOR", 2, 2, Engineering, S, "BITXOR(number1, number2)", "Bitwise exclusive OR of two non-negative integers.", bitxor),
        f!(
            "BITLSHIFT",
            2,
            2,
            Engineering,
            S,
            "BITLSHIFT(number, shift_amount)",
            "Shifts the bits of a number left (right for negative shifts).",
            bitlshift
        ),
        f!(
            "BITRSHIFT",
            2,
            2,
            Engineering,
            S,
            "BITRSHIFT(number, shift_amount)",
            "Shifts the bits of a number right (left for negative shifts).",
            bitrshift
        ),
        f!("CONVERT", 3, 3, Engineering, S, "CONVERT(number, from_unit, to_unit)", "Converts a measurement from one unit to another.", convert),
        f!("DELTA", 1, 2, Engineering, S, "DELTA(number1, [number2])", "Returns 1 when two numbers are equal, otherwise 0.", delta),
        f!("GESTEP", 1, 2, Engineering, S, "GESTEP(number, [step])", "Returns 1 when a number is at least the step, otherwise 0.", gestep),
        f!(
            "ERF",
            1,
            2,
            Engineering,
            S,
            "ERF(lower_limit, [upper_limit])",
            "Error function integrated from 0 to a limit, or between two limits.",
            erf_fn
        ),
        f!("ERF.PRECISE", 1, 1, Engineering, S, "ERF.PRECISE(x)", "Error function integrated from 0 to x.", erf_precise),
        f!("ERFC", 1, 1, Engineering, S, "ERFC(x)", "Complementary error function, 1 - ERF(x).", erfc_fn),
        f!("ERFC.PRECISE", 1, 1, Engineering, S, "ERFC.PRECISE(x)", "Complementary error function, 1 - ERF(x).", erfc_fn),
        f!(
            "COMPLEX",
            2,
            3,
            Engineering,
            S,
            "COMPLEX(real_num, i_num, [suffix])",
            "Builds a complex number as text from real and imaginary parts.",
            complex
        ),
        f!("IMABS", 1, 1, Engineering, S, "IMABS(inumber)", "Absolute value (modulus) of a complex number.", imabs),
        f!("IMAGINARY", 1, 1, Engineering, S, "IMAGINARY(inumber)", "Imaginary coefficient of a complex number.", imaginary),
        f!("IMREAL", 1, 1, Engineering, S, "IMREAL(inumber)", "Real coefficient of a complex number.", imreal),
        f!("IMARGUMENT", 1, 1, Engineering, S, "IMARGUMENT(inumber)", "Angle of a complex number in radians.", imargument),
        f!("IMCONJUGATE", 1, 1, Engineering, S, "IMCONJUGATE(inumber)", "Complex conjugate of a complex number.", imconjugate),
        f!("IMCOS", 1, 1, Engineering, S, "IMCOS(inumber)", "Cosine of a complex number.", imcos),
        f!("IMCOSH", 1, 1, Engineering, S, "IMCOSH(inumber)", "Hyperbolic cosine of a complex number.", imcosh),
        f!("IMCOT", 1, 1, Engineering, S, "IMCOT(inumber)", "Cotangent of a complex number.", imcot),
        f!("IMCSC", 1, 1, Engineering, S, "IMCSC(inumber)", "Cosecant of a complex number.", imcsc),
        f!("IMCSCH", 1, 1, Engineering, S, "IMCSCH(inumber)", "Hyperbolic cosecant of a complex number.", imcsch),
        f!("IMDIV", 2, 2, Engineering, S, "IMDIV(inumber1, inumber2)", "Quotient of two complex numbers.", imdiv),
        f!("IMEXP", 1, 1, Engineering, S, "IMEXP(inumber)", "Exponential of a complex number.", imexp),
        f!("IMLN", 1, 1, Engineering, S, "IMLN(inumber)", "Natural logarithm of a complex number.", imln),
        f!("IMLOG10", 1, 1, Engineering, S, "IMLOG10(inumber)", "Base-10 logarithm of a complex number.", imlog10),
        f!("IMLOG2", 1, 1, Engineering, S, "IMLOG2(inumber)", "Base-2 logarithm of a complex number.", imlog2),
        f!("IMPOWER", 2, 2, Engineering, S, "IMPOWER(inumber, number)", "Raises a complex number to a power.", impower),
        f!("IMPRODUCT", 1, VAR, Engineering, A, "IMPRODUCT(inumber1, [inumber2], ...)", "Product of complex numbers.", improduct),
        f!("IMSEC", 1, 1, Engineering, S, "IMSEC(inumber)", "Secant of a complex number.", imsec),
        f!("IMSECH", 1, 1, Engineering, S, "IMSECH(inumber)", "Hyperbolic secant of a complex number.", imsech),
        f!("IMSIN", 1, 1, Engineering, S, "IMSIN(inumber)", "Sine of a complex number.", imsin),
        f!("IMSINH", 1, 1, Engineering, S, "IMSINH(inumber)", "Hyperbolic sine of a complex number.", imsinh),
        f!("IMSQRT", 1, 1, Engineering, S, "IMSQRT(inumber)", "Square root of a complex number.", imsqrt),
        f!("IMSUB", 2, 2, Engineering, S, "IMSUB(inumber1, inumber2)", "Difference of two complex numbers.", imsub),
        f!("IMSUM", 1, VAR, Engineering, A, "IMSUM(inumber1, [inumber2], ...)", "Sum of complex numbers.", imsum),
        f!("IMTAN", 1, 1, Engineering, S, "IMTAN(inumber)", "Tangent of a complex number.", imtan),
        f!("BESSELJ", 2, 2, Engineering, S, "BESSELJ(x, n)", "Bessel function of the first kind, J_n(x).", besselj),
        f!("BESSELY", 2, 2, Engineering, S, "BESSELY(x, n)", "Bessel function of the second kind, Y_n(x).", bessely),
        f!("BESSELI", 2, 2, Engineering, S, "BESSELI(x, n)", "Modified Bessel function of the first kind, I_n(x).", besseli),
        f!("BESSELK", 2, 2, Engineering, S, "BESSELK(x, n)", "Modified Bessel function of the second kind, K_n(x).", besselk),
    ]
}

#[cfg(test)]
mod tests {
    use gridcraft_core::{CellError, Value};

    use super::parse_complex;
    use crate::util::testutil::*;

    #[track_caller]
    fn cx(v: Value, re: f64, im: f64) {
        let Value::Text(s) = &v else { panic!("expected complex text, got {v:?}") };
        let (z, _) = parse_complex(s).unwrap();
        let tol = 1e-12;
        assert!((z.re - re).abs() <= tol * re.abs().max(1.0), "{s}: re {} vs {re}", z.re);
        assert!((z.im - im).abs() <= tol * im.abs().max(1.0), "{s}: im {} vs {im}", z.im);
    }

    #[test]
    fn base_conversions() {
        close(ev("BIN2DEC", vec![n(1100100.0)]), 100.0);
        close(ev("BIN2DEC", vec![n(1111111111.0)]), -1.0);
        is_text(ev("BIN2HEX", vec![n(11111011.0), n(4.0)]), "00FB");
        is_text(ev("BIN2HEX", vec![n(1110.0)]), "E");
        is_text(ev("BIN2HEX", vec![n(1111111111.0)]), "FFFFFFFFFF");
        is_text(ev("BIN2OCT", vec![n(1001.0), n(3.0)]), "011");
        is_text(ev("BIN2OCT", vec![n(1111111111.0)]), "7777777777");
        is_text(ev("DEC2BIN", vec![n(9.0), n(4.0)]), "1001");
        is_text(ev("DEC2BIN", vec![n(-100.0)]), "1110011100");
        is_err(ev("DEC2BIN", vec![n(-1.0), n(0.0)]), CellError::Num);
        is_err(ev("DEC2HEX", vec![n(-1.0), n(11.0)]), CellError::Num);
        is_err(ev("DEC2OCT", vec![n(-1.0), n(-1.0)]), CellError::Num);
        is_err(ev("DEC2BIN", vec![n(1.0), crate::Arg::val(Value::Bool(true))]), CellError::Value);
        is_text(ev("DEC2BIN", vec![n(-1.0), n(1.0)]), "1111111111");
        is_text(ev("DEC2HEX", vec![n(-1.0), n(10.0)]), "FFFFFFFFFF");
        is_err(ev("DEC2BIN", vec![n(512.0)]), CellError::Num);
        is_text(ev("DEC2HEX", vec![n(100.0), n(4.0)]), "0064");
        is_text(ev("DEC2HEX", vec![n(-54.0)]), "FFFFFFFFCA");
        is_text(ev("DEC2HEX", vec![n(28.0)]), "1C");
        is_err(ev("DEC2HEX", vec![n(64.0), n(1.0)]), CellError::Num);
        is_text(ev("DEC2OCT", vec![n(58.0), n(3.0)]), "072");
        is_text(ev("DEC2OCT", vec![n(-100.0)]), "7777777634");
        is_text(ev("HEX2BIN", vec![t("F"), n(8.0)]), "00001111");
        is_text(ev("HEX2BIN", vec![t("B7")]), "10110111");
        is_text(ev("HEX2BIN", vec![t("FFFFFFFE00")]), "1000000000");
        is_err(ev("HEX2BIN", vec![t("200")]), CellError::Num);
        close(ev("HEX2DEC", vec![t("A5")]), 165.0);
        close(ev("HEX2DEC", vec![t("FFFFFFFF5B")]), -165.0);
        close(ev("HEX2DEC", vec![t("3DA408B9")]), 1034160313.0);
        is_text(ev("HEX2OCT", vec![t("F"), n(3.0)]), "017");
        is_text(ev("HEX2OCT", vec![t("3B4E")]), "35516");
        is_text(ev("HEX2OCT", vec![t("FFFFFFFF00")]), "7777777400");
        is_text(ev("OCT2BIN", vec![n(3.0), n(3.0)]), "011");
        is_text(ev("OCT2BIN", vec![n(7777777000.0)]), "1000000000");
        close(ev("OCT2DEC", vec![n(54.0)]), 44.0);
        close(ev("OCT2DEC", vec![n(7777777533.0)]), -165.0);
        is_text(ev("OCT2HEX", vec![n(100.0), n(4.0)]), "0040");
        is_text(ev("OCT2HEX", vec![n(7777777533.0)]), "FFFFFFFF5B");
        is_err(ev("BIN2DEC", vec![n(102.0)]), CellError::Num);
        for (f, input) in [
            ("BIN2DEC", " 10"),
            ("BIN2DEC", "10 "),
            ("BIN2DEC", "\t10"),
            ("BIN2HEX", " 10"),
            ("BIN2OCT", "10 "),
            ("HEX2DEC", " 10"),
            ("HEX2BIN", "10 "),
            ("HEX2OCT", "\t10"),
            ("OCT2DEC", " 10"),
            ("OCT2BIN", "10 "),
            ("OCT2HEX", "\t10"),
        ] {
            is_err(ev(f, vec![t(input)]), CellError::Num);
        }
        is_err(ev("HEX2DEC", vec![t("GG")]), CellError::Num);
        is_err(ev("BIN2DEC", vec![t("11111111111")]), CellError::Num);
        is_err(ev("DEC2BIN", vec![t("abc")]), CellError::Value);
        is_text(ev("DEC2BIN", vec![n(9.7)]), "1001");
        close(ev("HEX2DEC", vec![t("ff")]), 255.0);
    }

    #[test]
    fn bits() {
        close(ev("BITAND", vec![n(1.0), n(5.0)]), 1.0);
        close(ev("BITAND", vec![n(13.0), n(25.0)]), 9.0);
        close(ev("BITOR", vec![n(23.0), n(10.0)]), 31.0);
        close(ev("BITXOR", vec![n(5.0), n(3.0)]), 6.0);
        close(ev("BITLSHIFT", vec![n(4.0), n(2.0)]), 16.0);
        close(ev("BITRSHIFT", vec![n(13.0), n(2.0)]), 3.0);
        close(ev("BITRSHIFT", vec![n(13.0), n(-2.0)]), 52.0);
        is_err(ev("BITAND", vec![n(-1.0), n(5.0)]), CellError::Num);
        is_err(ev("BITAND", vec![n(1.5), n(5.0)]), CellError::Num);
        is_err(ev("BITLSHIFT", vec![n(1.0), n(54.0)]), CellError::Num);
        is_err(ev("BITLSHIFT", vec![n(2f64.powi(47)), n(1.0)]), CellError::Num);
        for f in ["BITLSHIFT", "BITRSHIFT"] {
            for shift in [-100.0, -54.0, -53.0, 53.0, 54.0, 100.0] {
                close(ev(f, vec![n(0.0), n(shift)]), 0.0);
            }
            for shift in [-100.0, -54.0, 54.0, 100.0] {
                is_err(ev(f, vec![n(1.0), n(shift)]), CellError::Num);
            }
            if f == "BITLSHIFT" {
                is_err(ev(f, vec![n(1.0), n(53.0)]), CellError::Num);
            } else {
                close(ev(f, vec![n(1.0), n(53.0)]), 0.0);
            }
            is_err(ev(f, vec![n(0.0), t("bad")]), CellError::Value);
        }
        is_err(ev("BITRSHIFT", vec![n(-1.0), n(100.0)]), CellError::Num);
    }

    #[test]
    fn convert_units() {
        close(ev("CONVERT", vec![n(1.0), t("lbm"), t("kg")]), 0.45359237);
        close(ev("CONVERT", vec![n(68.0), t("F"), t("C")]), 20.0);
        close(ev("CONVERT", vec![n(100.0), t("C"), t("F")]), 212.0);
        close(ev("CONVERT", vec![n(0.0), t("C"), t("K")]), 273.15);
        close(ev("CONVERT", vec![n(1.0), t("in"), t("cm")]), 2.54);
        close(ev("CONVERT", vec![n(1.0), t("mi"), t("km")]), 1.609344);
        close(ev("CONVERT", vec![n(1.0), t("gal"), t("l")]), 3.785411784);
        close(ev("CONVERT", vec![n(1.0), t("byte"), t("bit")]), 8.0);
        close(ev("CONVERT", vec![n(1.0), t("kibyte"), t("byte")]), 1024.0);
        close(ev("CONVERT", vec![n(100.0), t("km/h"), t("m/s")]), 100.0 / 3.6);
        close(ev("CONVERT", vec![n(1.0), t("m2"), t("ft2")]), 10.763910416709722);
        close(ev("CONVERT", vec![n(1.0), t("km2"), t("m2")]), 1e6);
        close(ev("CONVERT", vec![n(1.0), t("hr"), t("mn")]), 60.0);
        close(ev("CONVERT", vec![n(1.0), t("atm"), t("Pa")]), 101325.0);
        close(ev("CONVERT", vec![n(1.0), t("ha"), t("m^2")]), 10000.0);
        close(ev("CONVERT", vec![n(1.0), t("cal"), t("J")]), 4.1868);
        close(ev("CONVERT", vec![n(1.0), t("mm"), t("m")]), 0.001);
        close(ev("CONVERT", vec![n(6.0), t("ft"), t("yd")]), 2.0);
        close(ev("CONVERT", vec![n(1.0), t("cup"), t("tsp")]), 48.0);
        is_err(ev("CONVERT", vec![n(2.5), t("ft"), t("sec")]), CellError::NA);
        is_err(ev("CONVERT", vec![n(1.0), t("xyz"), t("m")]), CellError::NA);
        is_err(ev("CONVERT", vec![n(1.0), t("kft"), t("m")]), CellError::NA);
        is_err(ev("CONVERT", vec![n(1.0), t("LBM"), t("kg")]), CellError::NA);
    }

    #[test]
    fn delta_gestep_erf() {
        close(ev("DELTA", vec![n(5.0), n(4.0)]), 0.0);
        close(ev("DELTA", vec![n(5.0), n(5.0)]), 1.0);
        close(ev("DELTA", vec![n(0.5)]), 0.0);
        close(ev("GESTEP", vec![n(5.0), n(4.0)]), 1.0);
        close(ev("GESTEP", vec![n(-4.0)]), 0.0);
        close_tol(ev("ERF", vec![n(0.745)]), 0.707_928_920, 1e-9);
        close_tol(ev("ERF", vec![n(1.0)]), 0.842_700_792_949_715, 1e-12);
        close_tol(ev("ERF", vec![n(0.0), n(1.5)]), 0.966_105_146_475_311, 1e-12);
        close_tol(ev("ERF.PRECISE", vec![n(1.0)]), 0.842_700_792_949_715, 1e-12);
        close_tol(ev("ERFC", vec![n(1.0)]), 0.157_299_207_050_285, 1e-12);
        close_tol(ev("ERFC.PRECISE", vec![n(-1.0)]), 1.842_700_792_949_715, 1e-12);
    }

    #[test]
    fn complex_numbers() {
        is_text(ev("COMPLEX", vec![n(3.0), n(4.0)]), "3+4i");
        is_text(ev("COMPLEX", vec![n(3.0), n(4.0), t("j")]), "3+4j");
        is_text(ev("COMPLEX", vec![n(0.0), n(1.0)]), "i");
        is_text(ev("COMPLEX", vec![n(0.0), n(-1.0)]), "-i");
        is_text(ev("COMPLEX", vec![n(1.5), n(-2.0)]), "1.5-2i");
        is_text(ev("COMPLEX", vec![n(3.0), n(0.0)]), "3");
        is_text(ev("COMPLEX", vec![n(0.0), n(0.0)]), "0");
        is_err(ev("COMPLEX", vec![n(1.0), n(1.0), t("k")]), CellError::Value);
        close(ev("IMABS", vec![t("5+12i")]), 13.0);
        close(ev("IMREAL", vec![t("6-9i")]), 6.0);
        close(ev("IMAGINARY", vec![t("3+4i")]), 4.0);
        close(ev("IMAGINARY", vec![t("-j")]), -1.0);
        close(ev("IMAGINARY", vec![t("4")]), 0.0);
        close(ev("IMREAL", vec![n(7.0)]), 7.0);
        close(ev("IMARGUMENT", vec![t("3+4i")]), 0.927_295_218_001_612_2);
        is_err(ev("IMARGUMENT", vec![t("0")]), CellError::Div0);
        is_text(ev("IMCONJUGATE", vec![t("3+4i")]), "3-4i");
        is_text(ev("IMDIV", vec![t("-238+240i"), t("10+24i")]), "5+12i");
        is_err(ev("IMDIV", vec![t("1+i"), t("0")]), CellError::Num);
        is_text(ev("IMPRODUCT", vec![t("3+4i"), t("5-3i")]), "27+11i");
        is_text(ev("IMPRODUCT", vec![t("1+2i"), n(30.0)]), "30+60i");
        is_text(ev("IMSUM", vec![t("3+4i"), t("5-3i")]), "8+i");
        is_text(ev("IMSUM", vec![rf(arr(vec![vec![tv("1+i"), Value::Empty, tv("2")]]))]), "3+i");
        is_text(ev("IMSUB", vec![t("13+4i"), t("5+3i")]), "8+i");
        is_err(ev("IMSUM", vec![t("1+i"), t("1+j")]), CellError::Value);
        is_err(ev("IMABS", vec![t("abc")]), CellError::Num);
        is_err(ev("IMABS", vec![b(true)]), CellError::Value);
        cx(ev("IMEXP", vec![t("1+i")]), 1.468_693_939_915_885_2, 2.287_355_287_178_842);
        cx(ev("IMLN", vec![t("3+4i")]), 1.609_437_912_434_100_3, 0.927_295_218_001_612_2);
        cx(ev("IMLOG10", vec![t("3+4i")]), 0.698_970_004_336_018_8, 0.402_719_196_273_373_2);
        cx(ev("IMLOG2", vec![t("3+4i")]), 2.321_928_094_887_362, 1.337_804_212_450_976_8);
        cx(ev("IMSQRT", vec![t("1+i")]), 1.098_684_113_467_809_9, 0.455_089_860_562_227_3);
        // Strings read back from Excel (16.0): its stray real part on negative reals included.
        for (input, want) in [
            ("-4", "1.22514845490862E-16+2i"),
            ("-9", "1.83772268236293E-16+3i"),
            ("-1", "6.1257422745431E-17+i"),
            ("-2", "8.66310780426106E-17+1.4142135623731i"),
            ("-100", "6.1257422745431E-16+10i"),
            ("-1E10", "6.1257422745431E-12+100000i"),
            ("-1E-10", "6.1257422745431E-22+0.00001i"),
            ("-4+0j", "1.22514845490862E-16+2j"),
            ("-4-0i", "1.22514845490862E-16+2i"),
            ("-4-1E-20i", "1.22514845490862E-16-2i"),
            ("-1+1E-10i", "5.00000653944413E-11+i"),
            ("-0.5-1E-12i", "7.07212958930194E-13-0.707106781186548i"),
            ("4", "2"),
            ("2", "1.4142135623731"),
            ("0", "0"),
            ("3+4i", "2+i"),
            ("3+4j", "2+j"),
            ("-3+4i", "1+2i"),
            ("-3-4i", "1-2i"),
            ("-2i", "1-i"),
            ("2i", "1+i"),
            ("i", "0.707106781186548+0.707106781186547i"),
            ("5-12i", "3-2i"),
            ("-5+12i", "2+3i"),
            ("1E10+1E10i", "109868.411346781+45508.9860562227i"),
        ] {
            is_text(ev("IMSQRT", vec![t(input)]), want);
        }
        is_text(ev("IMSQRT", vec![t("4")]), "2");
        is_text(ev("IMSQRT", vec![t("-2i")]), "1-i");
        is_text(ev("IMPOWER", vec![t("2+3i"), n(3.0)]), "-46+9i");
        cx(ev("IMPOWER", vec![t("2+3i"), n(0.5)]), 1.674_149_228_035_540_2, 0.895_977_476_129_838);
        cx(ev("IMSIN", vec![t("3+4i")]), 3.853_738_037_919_377, -27.016_813_258_003_93);
        cx(ev("IMCOS", vec![t("1+i")]), 0.833_730_025_131_149, -0.988_897_705_762_865);
        cx(ev("IMSINH", vec![t("4+3i")]), -27.016_813_258_003_93, 3.853_738_037_919_377);
        cx(ev("IMCOSH", vec![t("4+3i")]), -27.034_945_603_074_224, 3.851_153_334_811_777);
        cx(ev("IMTAN", vec![t("4+3i")]), 0.004_908_258_067_496_06, 1.000_709_536_067_233);
        cx(ev("IMCOT", vec![t("4+3i")]), 0.004_901_182_394_304_4, -0.999_266_927_805_902);
        cx(ev("IMSEC", vec![t("4+3i")]), -0.065_294_027_857_947_1, -0.075_224_960_302_773_2);
        cx(ev("IMCSC", vec![t("4+3i")]), -0.075_489_832_915_863_7, 0.064_877_471_370_635_5);
        is_text(ev("IMSECH", vec![t("0")]), "1");
        is_err(ev("IMCSCH", vec![t("0")]), CellError::Num);
        // Excel-cached results (Excel for Mac, `formulas` project test.xlsx, ENGINEERING sheet): a
        // part needing more than 21 characters as a decimal is written in scientific notation.
        is_text(ev("IMCSCH", vec![t("13+5j")]), "1.28233995888454E-06+4.33496947416312E-06j");
        is_text(ev("IMEXP", vec![t("-13+25j")]), "2.24044486393732E-06-2.99158552811404E-07j");
        is_text(ev("IMSECH", vec![t("7+25j")]), "0.00180771853884451+0.000241377759972715j");
        // sech z · cosh z = 1
        let Value::Text(c) = ev("IMCOSH", vec![t("4+3i")]) else { panic!() };
        let Value::Text(sc) = ev("IMSECH", vec![t("4+3i")]) else { panic!() };
        cx(ev("IMPRODUCT", vec![t(&c), t(&sc)]), 1.0, 0.0);
        let Value::Text(sh) = ev("IMSINH", vec![t("4+3i")]) else { panic!() };
        let Value::Text(csh) = ev("IMCSCH", vec![t("4+3i")]) else { panic!() };
        cx(ev("IMPRODUCT", vec![t(&sh), t(&csh)]), 1.0, 0.0);
        cx(ev("IMSUM", vec![t("1.5e1+2E-1j")]), 15.0, 0.2);
    }

    #[test]
    fn bessel() {
        close_tol(ev("BESSELJ", vec![n(1.9), n(2.0)]), 0.329_925_829, 1e-6);
        close_tol(ev("BESSELY", vec![n(2.5), n(1.0)]), 0.145_918_138, 1e-6);
        close_tol(ev("BESSELI", vec![n(1.5), n(1.0)]), 0.981_666_428, 1e-6);
        close_tol(ev("BESSELK", vec![n(1.5), n(1.0)]), 0.277_387_804, 1e-6);
        close_tol(ev("BESSELJ", vec![n(1.0), n(0.0)]), 0.765_197_686_557_966_6, 1e-12);
        close_tol(ev("BESSELJ", vec![n(1.0), n(1.0)]), 0.440_050_585_744_933_5, 1e-12);
        close_tol(ev("BESSELJ", vec![n(-1.0), n(1.0)]), -0.440_050_585_744_933_5, 1e-12);
        close_tol(ev("BESSELY", vec![n(1.0), n(0.0)]), 0.088_256_964_215_676_96, 1e-11);
        close_tol(ev("BESSELY", vec![n(1.0), n(1.0)]), -0.781_212_821_300_288_7, 1e-11);
        close_tol(ev("BESSELI", vec![n(1.0), n(0.0)]), 1.266_065_877_752_008_4, 1e-12);
        close_tol(ev("BESSELI", vec![n(1.0), n(1.0)]), 0.565_159_103_992_485_1, 1e-12);
        close_tol(ev("BESSELK", vec![n(1.0), n(0.0)]), 0.421_024_438_240_708_3, 1e-11);
        close_tol(ev("BESSELK", vec![n(1.0), n(1.0)]), 0.601_907_230_197_234_6, 1e-11);
        is_err(ev("BESSELJ", vec![n(1.0), n(-1.0)]), CellError::Num);
        is_err(ev("BESSELY", vec![n(0.0), n(1.0)]), CellError::Num);
        is_err(ev("BESSELK", vec![n(-1.0), n(1.0)]), CellError::Num);
        is_err(ev("BESSELJ", vec![t("x"), n(1.0)]), CellError::Value);
        // Wronskian J_{1}Y_0 − J_0Y_1 = 2/(πx), checking series and asymptotic regimes.
        for x in [0.5, 7.0, 15.0, 24.0, 30.0, 80.0] {
            let j0 = match ev("BESSELJ", vec![n(x), n(0.0)]) {
                Value::Number(v) => v,
                _ => panic!(),
            };
            let j1 = match ev("BESSELJ", vec![n(x), n(1.0)]) {
                Value::Number(v) => v,
                _ => panic!(),
            };
            let y0 = match ev("BESSELY", vec![n(x), n(0.0)]) {
                Value::Number(v) => v,
                _ => panic!(),
            };
            let y1 = match ev("BESSELY", vec![n(x), n(1.0)]) {
                Value::Number(v) => v,
                _ => panic!(),
            };
            let w = j1 * y0 - j0 * y1;
            let expect = 2.0 / (std::f64::consts::PI * x);
            assert!((w - expect).abs() < 1e-9 * expect.max(1.0), "x={x}: {w} vs {expect}");
        }
        // Large orders and arguments stay finite.
        assert!(matches!(ev("BESSELJ", vec![n(50.0), n(3.0)]), Value::Number(_)));
        assert!(matches!(ev("BESSELK", vec![n(30.0), n(5.0)]), Value::Number(_)));
    }
}
