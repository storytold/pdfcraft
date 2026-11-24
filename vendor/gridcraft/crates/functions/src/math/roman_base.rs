use super::{Arg, CellError, Ctx, R, Value, num, num_val, opt_num, text, text_val};

// ---------------------------------------------------------------------------------------------
// Number systems

const NUMERALS: [(char, u32); 7] = [('I', 1), ('V', 5), ('X', 10), ('L', 50), ('C', 100), ('D', 500), ('M', 1000)];

/// Tokens (text, value) for a ROMAN form, largest value first. Form 0 is classic; each higher
/// form widens one of two limits on the subtracted numeral's ratio to the larger one, alternating:
/// the limit for I/X/C (10, 10, 100, 100, 1000) and for V/L (none, 20, 20, 200, 200). So form 2
/// allows IC and XM but not VD, and form 3 adds VD and VM but not ID.
fn roman_tokens(form: u32) -> Vec<(String, u32)> {
    let (pow10_max, five_max) = [(10, 0), (10, 20), (100, 20), (100, 200), (1000, 200)].get(form as usize).copied().unwrap_or((10, 0));
    let mut toks: Vec<(String, u32)> = NUMERALS.iter().map(|(c, v)| (c.to_string(), *v)).collect();
    for (i, (sc, sv)) in NUMERALS.iter().enumerate() {
        for (lc, lv) in NUMERALS.iter().skip(i + 1) {
            let ratio = lv / sv;
            let pow10 = matches!(sc, 'I' | 'X' | 'C');
            let ok = ratio <= if pow10 { pow10_max } else { five_max };
            let value = lv - sv;
            // Skip pairs that equal a single numeral (VX = V) – they are never shorter.
            if ok && !NUMERALS.iter().any(|(_, v)| *v == value) {
                toks.push((format!("{sc}{lc}"), value));
            }
        }
    }
    // Largest value first; on ties prefer the shorter / classic token.
    toks.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.len().cmp(&b.0.len())));
    toks
}

pub(super) fn roman(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let n = num(a, 0)?.trunc();
    let form = match a.get(1).map(|x| x.value.scalar()) {
        None => 0.0,
        Some(Value::Bool(true)) => 0.0,
        Some(Value::Bool(false)) => 4.0,
        Some(_) => num(a, 1)?.trunc(),
    };
    if !(0.0..=3999.0).contains(&n) || !(0.0..=4.0).contains(&form) {
        return Err(CellError::Value);
    }
    let toks = roman_tokens(form as u32);
    let mut rem = n as u32;
    let mut out = String::new();
    while rem > 0 {
        let Some((t, v)) = toks.iter().find(|(_, v)| *v <= rem) else { break };
        out.push_str(t);
        rem -= v;
    }
    text_val(out)
}

pub(super) fn arabic(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = text(a, 0)?;
    let s = s.trim();
    if s.chars().count() > 255 {
        return Err(CellError::Value);
    }
    let (neg, body) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let mut vals = Vec::new();
    for ch in body.chars() {
        let u = ch.to_ascii_uppercase();
        let Some((_, v)) = NUMERALS.iter().find(|(c, _)| *c == u) else { return Err(CellError::Value) };
        vals.push(*v as i64);
    }
    let mut total = 0i64;
    for (i, v) in vals.iter().enumerate() {
        if vals.get(i + 1).is_some_and(|next| next > v) {
            total -= v;
        } else {
            total += v;
        }
    }
    num_val(if neg { -total as f64 } else { total as f64 })
}

pub(super) fn base(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let n = num(a, 0)?.trunc();
    let radix = num(a, 1)?.trunc();
    let min_len = opt_num(a, 2, 0.0)?.trunc();
    if !(0.0..9.007_199_254_740_992e15).contains(&n) || !(2.0..=36.0).contains(&radix) || !(0.0..=255.0).contains(&min_len) {
        return Err(CellError::Num);
    }
    let radix = radix as u64;
    let mut v = n as u64;
    let mut digits = Vec::new();
    loop {
        let d = (v % radix) as u32;
        digits.push(char::from_digit(d, radix as u32).unwrap_or('0').to_ascii_uppercase());
        v /= radix;
        if v == 0 {
            break;
        }
    }
    while digits.len() < min_len as usize {
        digits.push('0');
    }
    text_val(digits.iter().rev().collect::<String>())
}

pub(super) fn decimal(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = text(a, 0)?;
    let radix = num(a, 1)?.trunc();
    if !(2.0..=36.0).contains(&radix) || s.chars().count() > 255 {
        return Err(CellError::Num);
    }
    let radix = radix as u32;
    let mut total = 0f64;
    for ch in s.trim().chars() {
        let Some(d) = ch.to_digit(radix) else { return Err(CellError::Num) };
        total = total * radix as f64 + d as f64;
    }
    if total >= 9.007_199_254_740_992e15 {
        return Err(CellError::Num);
    }
    num_val(total)
}
