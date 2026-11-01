use super::ast::Expr;
use super::dates::{MAX_DAYS, civil_from_days, days_from_civil, format_date, parse_date};
use super::parser::parse;
use super::value::{Val, fixed, is_numeric_text, parse_num};
use super::words::word_num;
use super::{Interp, MAX_STRING, R, STRING_TOO_LONG, XfaKind, is_limit};

impl Interp<'_> {
    /// A built-in function: its result is never a string past [`MAX_STRING`], and work on
    /// long strings is charged to the step budget.
    pub(super) fn builtin(&mut self, name: &str, args: &[Expr]) -> R<Val> {
        let v = self.builtin_inner(name, args)?;
        if let Val::Str(s) = &v {
            if s.len() > MAX_STRING {
                return Err(STRING_TOO_LONG.into());
            }
            self.charge((s.len() / 256) as u64)?;
        }
        Ok(v)
    }

    fn builtin_inner(&mut self, name: &str, args: &[Expr]) -> R<Val> {
        let lower = name.to_ascii_lowercase();
        let n = |v: f64| Ok(Val::Num(v));
        if let Some(value) = self.date_builtin(&lower, args)? {
            return Ok(value);
        }
        if let Some(value) = self.financial_builtin(&lower, args)? {
            return Ok(value);
        }
        if let Some(value) = self.unit_builtin(&lower, args)? {
            return Ok(value);
        }
        Ok(match lower.as_str() {
            // ── arithmetic
            "abs" => return n(self.arg_num(args, 0)?.abs()),
            "ceil" => return n(self.arg_num(args, 0)?.ceil()),
            "floor" => return n(self.arg_num(args, 0)?.floor()),
            "mod" => {
                let (a, b) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?);
                if b == 0.0 {
                    return Err("division by zero".into());
                }
                Val::Num(a - b * (a / b).trunc())
            }
            "round" => {
                let a = self.arg_num(args, 0)?;
                let places = if args.len() > 1 { self.arg_num(args, 1)?.clamp(0.0, 12.0) as i32 } else { 0 };
                let k = 10f64.powi(places);
                let scaled = a * k;
                if scaled.is_finite() { Val::Num(scaled.round() / k) } else { Val::Num(a) }
            }
            "count" => {
                // Non-null values and objects: an empty field doesn't count, a subform does.
                let vals = self.arg_values_keeping(args, true)?;
                let counts = |v: &Val| match v {
                    Val::Node(i) => self.node(*i).is_some_and(|n| !matches!(n.kind, XfaKind::Field | XfaKind::ExclGroup) || !n.value.is_empty()),
                    other => !self.is_null(other),
                };
                Val::Num(vals.iter().filter(|v| counts(v)).count() as f64)
            }
            "sum" | "avg" | "max" | "min" => {
                let vals = self.arg_values(args)?;
                let nums: Vec<f64> = vals.iter().map(|v| self.to_num(v)).collect();
                match lower.as_str() {
                    "sum" => Val::Num(nums.iter().sum()),
                    "avg" => {
                        if nums.is_empty() {
                            Val::Null
                        } else {
                            Val::Num(nums.iter().sum::<f64>() / nums.len() as f64)
                        }
                    }
                    "max" => nums.iter().copied().fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x)))).map_or(Val::Null, Val::Num),
                    _ => nums.iter().copied().fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.min(x)))).map_or(Val::Null, Val::Num),
                }
            }
            // ── logical
            "choose" => {
                let i = self.arg_num(args, 0)?;
                if !i.is_finite() || i < 1.0 {
                    return Ok(Val::Null);
                }
                self.arg(args, i as usize)?
            }
            "exists" => {
                let Some(e) = args.first() else { return Ok(Val::Num(0.0)) };
                // A name that resolves to nothing is the answer "no"; a limit the lookup ran
                // into (time, steps, depth, size) still stops the script.
                match self.all_of(e) {
                    Ok(v) => Val::Num(if v.is_empty() { 0.0 } else { 1.0 }),
                    Err(e) if is_limit(&e) => return Err(e),
                    Err(_) => Val::Num(0.0),
                }
            }
            "hasvalue" => {
                let v = self.arg(args, 0)?;
                Val::Num(if self.is_null(&v) || matches!(&v, Val::Str(s) if s.trim().is_empty()) { 0.0 } else { 1.0 })
            }
            "oneof" => {
                let v = self.arg(args, 0)?;
                let rest = self.arg_values(args.get(1..).unwrap_or_default())?;
                let mut hit = false;
                for x in &rest {
                    // Each comparison copies and compares `v`: charged per item.
                    self.charge(self.cost(&v).saturating_add(self.cost(x)).saturating_add(1))?;
                    if self.binary("==", v.clone(), x.clone()).is_ok_and(|r| self.truthy(&r)) {
                        hit = true;
                        break;
                    }
                }
                Val::Num(if hit { 1.0 } else { 0.0 })
            }
            "within" => {
                let v = self.arg(args, 0)?;
                let (lo, hi) = (self.arg(args, 1)?, self.arg(args, 2)?);
                let ok = self.binary(">=", v.clone(), lo).is_ok_and(|r| self.truthy(&r)) && self.binary("<=", v, hi).is_ok_and(|r| self.truthy(&r));
                Val::Num(if ok { 1.0 } else { 0.0 })
            }
            // ── strings
            "at" => {
                let (s, sub) = (self.arg_str(args, 0)?, self.arg_str(args, 1)?);
                Val::Num(if sub.is_empty() { 0.0 } else { s.find(&sub).map_or(0.0, |b| s[..b].chars().count() as f64 + 1.0) })
            }
            "concat" => {
                let mut out = String::new();
                for i in 0..args.len() {
                    out.push_str(&self.arg_str(args, i)?);
                    if out.len() > MAX_STRING {
                        return Err(STRING_TOO_LONG.into());
                    }
                }
                Val::Str(out)
            }
            "left" => {
                let (s, k) = (self.arg_str(args, 0)?, self.arg_num(args, 1)?.max(0.0) as usize);
                Val::Str(s.chars().take(k).collect())
            }
            "right" => {
                let (s, k) = (self.arg_str(args, 0)?, self.arg_num(args, 1)?.max(0.0) as usize);
                let len = s.chars().count();
                Val::Str(s.chars().skip(len.saturating_sub(k)).collect())
            }
            "len" => Val::Num(self.arg_str(args, 0)?.chars().count() as f64),
            "lower" => Val::Str(self.arg_str(args, 0)?.to_lowercase()),
            "upper" => Val::Str(self.arg_str(args, 0)?.to_uppercase()),
            "ltrim" => Val::Str(self.arg_str(args, 0)?.trim_start().to_string()),
            "rtrim" => Val::Str(self.arg_str(args, 0)?.trim_end().to_string()),
            "space" => {
                let k = self.arg_num(args, 0)?.clamp(0.0, MAX_STRING as f64) as usize;
                Val::Str(" ".repeat(k))
            }
            "str" => {
                let v = self.arg_num(args, 0)?;
                let width = if args.len() > 1 { self.arg_num(args, 1)?.clamp(0.0, 1000.0) as usize } else { 10 };
                let prec = if args.len() > 2 { self.arg_num(args, 2)?.clamp(0.0, 20.0) as usize } else { 0 };
                let text = fixed(v, prec);
                Val::Str(if text.len() > width { "*".repeat(width) } else { format!("{text:>width$}") })
            }
            "substr" => {
                let s = self.arg_str(args, 0)?;
                let start = self.arg_num(args, 1)?.max(1.0) as usize;
                let len = self.arg_num(args, 2)?.max(0.0) as usize;
                Val::Str(s.chars().skip(start - 1).take(len).collect())
            }
            "stuff" => {
                let s = self.arg_str(args, 0)?;
                let start = self.arg_num(args, 1)?.max(1.0) as usize;
                let del = self.arg_num(args, 2)?.max(0.0) as usize;
                let ins = if args.len() > 3 { self.arg_str(args, 3)? } else { String::new() };
                let chars: Vec<char> = s.chars().collect();
                let a = start.saturating_sub(1).min(chars.len());
                let b = a.saturating_add(del).min(chars.len());
                let out: String = chars[..a].iter().chain(ins.chars().collect::<Vec<_>>().iter()).chain(chars[b..].iter()).collect();
                if out.len() > MAX_STRING {
                    return Err(STRING_TOO_LONG.into());
                }
                Val::Str(out)
            }
            "replace" => {
                let (s, old) = (self.arg_str(args, 0)?, self.arg_str(args, 1)?);
                let new = if args.len() > 2 { self.arg_str(args, 2)? } else { String::new() };
                if old.is_empty() {
                    return Ok(Val::Str(s));
                }
                let grown = s.matches(old.as_str()).count().saturating_mul(new.len()).saturating_add(s.len());
                if grown > MAX_STRING {
                    return Err(STRING_TOO_LONG.into());
                }
                Val::Str(s.replace(&old, &new))
            }
            "wordnum" => {
                let v = self.arg_num(args, 0)?;
                let kind = if args.len() > 1 { self.arg_num(args, 1)? } else { 0.0 };
                Val::Str(word_num(v, kind == 2.0))
            }
            "uuid" => Val::Str(format!("{:032x}", (self.steps as u128).wrapping_mul(0x9E37_79B9_7F4A_7C15_F39C_C060_5CED_C835))),
            "encode" | "decode" => {
                let s = self.arg_str(args, 0)?;
                let kind = if args.len() > 1 { self.arg_str(args, 1)?.to_ascii_lowercase() } else { "url".into() };
                Val::Str(match (lower.as_str(), kind.as_str()) {
                    ("encode", "html" | "xml") => s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;"),
                    ("decode", "html" | "xml") => s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&amp;", "&"),
                    ("encode", _) => s
                        .bytes()
                        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
                        .collect(),
                    (_, _) => {
                        let mut out = Vec::new();
                        let b = s.as_bytes();
                        let mut i = 0;
                        while i < b.len() {
                            if b[i] == b'%'
                                && let Some(h) = b
                                    .get(i + 1..i + 3)
                                    .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
                                    .and_then(|h| std::str::from_utf8(h).ok())
                                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                            {
                                out.push(h);
                                i += 3;
                            } else {
                                out.push(b[i]);
                                i += 1;
                            }
                        }
                        String::from_utf8_lossy(&out).into_owned()
                    }
                })
            }
            "format" => {
                // Number pictures: `9` digit, `z` optional digit, `,` separator, `.` point, `$`.
                let pic = self.arg_str(args, 0)?;
                let v = self.arg(args, 1)?;
                if pic.to_ascii_uppercase().contains("YY") || pic.to_ascii_uppercase().contains("DD") {
                    let text = self.to_str(&v);
                    let days = parse_num(&text).filter(|d| is_numeric_text(&text) && d.abs() < MAX_DAYS).map(|d| d as i64);
                    return Ok(match days {
                        Some(d) => {
                            let (y, m, dd) = civil_from_days(d);
                            Val::Str(format_date(y, m, dd, &pic))
                        }
                        None => parse_date(&text, "YYYY-MM-DD").map_or(Val::Str(text), |(y, m, d)| Val::Str(format_date(y, m, d, &pic))),
                    });
                }
                let num = self.to_num(&v);
                let decimals = pic.split_once('.').map_or(0, |(_, f)| f.chars().filter(|c| matches!(c, '9' | 'z' | 'Z')).count()).min(20);
                // `9` in the integer part forces a digit; `z` does not.
                let min_int = pic.split('.').next().map_or(0, |i| i.chars().filter(|c| *c == '9').count()).min(20);
                let width = min_int + if decimals > 0 { decimals + 1 } else { 0 };
                let grouped = pic.contains(',');
                let mut text = format!("{:0>width$}", fixed(num.abs(), decimals));
                if grouped {
                    let (int, frac) = text.split_once('.').map_or((text.as_str(), ""), |(a, b)| (a, b));
                    let mut g = String::new();
                    for (k, c) in int.chars().enumerate() {
                        if k > 0 && (int.len() - k) % 3 == 0 {
                            g.push(',');
                        }
                        g.push(c);
                    }
                    text = if frac.is_empty() { g } else { format!("{g}.{frac}") };
                }
                let nonzero = text.chars().any(|c| ('1'..='9').contains(&c));
                if pic.contains('$') {
                    text = format!("${text}");
                }
                if num < 0.0 && nonzero {
                    text = format!("-{text}");
                }
                Val::Str(text)
            }
            "parse" => {
                let pic = self.arg_str(args, 0)?;
                let s = self.arg_str(args, 1)?;
                if pic.to_ascii_uppercase().contains("YY") {
                    // The canonical form of a date is ISO.
                    return Ok(parse_date(&s, &pic).map_or(Val::Null, |(y, m, d)| Val::Str(format_date(y, m, d, "YYYY-MM-DD"))));
                }
                let cleaned: String = s.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
                parse_num(&cleaned).map_or(Val::Null, Val::Num)
            }
            "ref" => {
                let Some(e) = args.first() else { return Ok(Val::Null) };
                self.eval(e)?
            }
            "eval" => {
                let src = self.arg_str(args, 0)?;
                self.charge((src.len() / 16) as u64)?;
                let body = parse(&src)?;
                let (_, v) = self.block(&body)?;
                v
            }
            "get" | "post" | "put" => return Err(format!("{name}: network access is not available to form scripts")),
            _ => return Err(format!("unknown function {name}")),
        })
    }

    fn date_builtin(&mut self, lower: &str, args: &[Expr]) -> R<Option<Val>> {
        let value = match lower {
            "date" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
                    Val::Num((secs.div_euclid(86_400) + 25_568) as f64)
                }
                #[cfg(target_arch = "wasm32")]
                {
                    Val::Num(25_568.0)
                }
            }
            "time" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
                    Val::Num(ms.rem_euclid(86_400_000) as f64)
                }
                #[cfg(target_arch = "wasm32")]
                {
                    Val::Num(0.0)
                }
            }
            "date2num" | "isodate2num" => {
                let s = self.arg_str(args, 0)?;
                let pic = if lower == "isodate2num" || args.len() < 2 { "YYYY-MM-DD".to_string() } else { self.arg_str(args, 1)? };
                let parsed = parse_date(&s, &pic).or_else(|| parse_date(&s, "YYYYMMDD")).or_else(|| parse_date(&s, "YYYY-MM-DD"));
                parsed.map_or(Val::Num(0.0), |(y, m, d)| Val::Num(days_from_civil(y, m, d) as f64))
            }
            "num2date" => {
                let days = self.arg_num(args, 0)?;
                let pic = if args.len() > 1 { self.arg_str(args, 1)? } else { "MMM D, YYYY".into() };
                if !days.is_finite() || days.abs() >= MAX_DAYS {
                    return Ok(Some(Val::Null));
                }
                let (y, m, d) = civil_from_days(days as i64);
                Val::Str(format_date(y, m, d, &pic))
            }
            "datefmt" | "localdatefmt" => {
                let style = if args.is_empty() { 0.0 } else { self.arg_num(args, 0)? };
                Val::Str(
                    match style as i32 {
                        1 => "M/D/YY",
                        2 => "MMM D, YYYY",
                        3 => "MMMM D, YYYY",
                        4 => "EEEE, MMMM D, YYYY",
                        _ => "MMM D, YYYY",
                    }
                    .into(),
                )
            }
            "timefmt" | "localtimefmt" => Val::Str("HH:MM:SS".into()),
            "num2time" | "num2gmtime" => {
                let ms = self.arg_num(args, 0)?;
                if !ms.is_finite() || ms < 0.0 {
                    return Ok(Some(Val::Null));
                }
                let s = (ms / 1000.0) as u64;
                Val::Str(format!("{:02}:{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60, s % 60))
            }
            "time2num" | "isotime2num" => {
                let s = self.arg_str(args, 0)?;
                // ISO 8601 allows a leading "T", a trailing "Z" and fractional seconds.
                let t = s.trim();
                let t = t.strip_prefix(['T', 't']).unwrap_or(t);
                let t = t.strip_suffix(['Z', 'z']).unwrap_or(t);
                let mut parts = t.split(':');
                let mut whole = || parts.next().and_then(|p| p.trim().parse::<u64>().ok()).unwrap_or(0).min(1_000_000);
                let (h, m) = (whole(), whole());
                let sec =
                    parts.next().and_then(|p| p.trim().parse::<f64>().ok()).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0).min(1_000_000.0);
                Val::Num(((h * 3600 + m * 60) * 1000) as f64 + (sec * 1000.0).round())
            }
            _ => return Ok(None),
        };
        Ok(Some(value))
    }

    fn financial_builtin(&mut self, lower: &str, args: &[Expr]) -> R<Option<Val>> {
        let value = match lower {
            "fv" => {
                let (p, r, k) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?, self.arg_num(args, 2)?);
                Val::Num(if r == 0.0 { p * k } else { p * (((1.0 + r).powf(k) - 1.0) / r) })
            }
            "pv" => {
                let (p, r, k) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?, self.arg_num(args, 2)?);
                Val::Num(if r == 0.0 { p * k } else { p * ((1.0 - (1.0 + r).powf(-k)) / r) })
            }
            "pmt" => {
                let (pv, r, k) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?, self.arg_num(args, 2)?);
                if k <= 0.0 {
                    return Err("Pmt needs a positive number of periods".into());
                }
                Val::Num(if r == 0.0 { pv / k } else { pv * r / (1.0 - (1.0 + r).powf(-k)) })
            }
            "term" => {
                let (p, r, fv) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?, self.arg_num(args, 2)?);
                if p <= 0.0 || r <= 0.0 || fv <= 0.0 {
                    return Err("Term needs positive arguments".into());
                }
                Val::Num(((fv * r / p) + 1.0).ln() / (1.0 + r).ln())
            }
            "cterm" => {
                let (r, fv, pv) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?, self.arg_num(args, 2)?);
                if r <= 0.0 || fv <= 0.0 || pv <= 0.0 {
                    return Err("CTerm needs positive arguments".into());
                }
                Val::Num((fv / pv).ln() / (1.0 + r).ln())
            }
            "rate" => {
                let (fv, pv, k) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?, self.arg_num(args, 2)?);
                if fv <= 0.0 || pv <= 0.0 || k <= 0.0 {
                    return Err("Rate needs positive arguments".into());
                }
                Val::Num((fv / pv).powf(1.0 / k) - 1.0)
            }
            "npv" => {
                let r = self.arg_num(args, 0)?;
                let flows = self.arg_values(args.get(1..).unwrap_or_default())?;
                Val::Num(flows.iter().enumerate().map(|(i, v)| self.to_num(v) / (1.0 + r).powi(i as i32 + 1)).sum())
            }
            "ipmt" | "ppmt" => {
                let (pv, r, pmt, first, count) = (
                    self.arg_num(args, 0)?,
                    self.arg_num(args, 1)?,
                    self.arg_num(args, 2)?,
                    self.arg_num(args, 3)?.max(1.0),
                    self.arg_num(args, 4)?.max(1.0),
                );
                let (mut balance, mut interest, mut principal) = (pv, 0.0, 0.0);
                for period in 1..=((first + count - 1.0).min(10_000.0) as u32) {
                    self.step()?;
                    let i = balance * r;
                    let p = (pmt - i).min(balance);
                    if period >= first as u32 {
                        interest += i;
                        principal += p;
                    }
                    balance -= p;
                    if balance <= 0.0 {
                        break;
                    }
                }
                Val::Num(if lower == "ipmt" { interest } else { principal })
            }
            "apr" => {
                let (principal, payment, periods) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?, self.arg_num(args, 2)?);
                if principal <= 0.0 || payment <= 0.0 || periods <= 0.0 {
                    return Err("Apr needs positive arguments".into());
                }
                // Bisection on the monthly rate.
                let (mut lo, mut hi) = (0.0f64, 1.0f64);
                for _ in 0..200 {
                    let mid = (lo + hi) / 2.0;
                    let pmt = if mid == 0.0 { principal / periods } else { principal * mid / (1.0 - (1.0 + mid).powf(-periods)) };
                    if pmt > payment {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                Val::Num((lo + hi) / 2.0 * 12.0)
            }
            _ => return Ok(None),
        };
        Ok(Some(value))
    }

    fn unit_builtin(&mut self, lower: &str, args: &[Expr]) -> R<Option<Val>> {
        let value = match lower {
            "unittype" => {
                let s = self.arg_str(args, 0)?;
                let unit: String = s.trim().chars().skip_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+').collect();
                Val::Str(
                    match unit.trim().to_ascii_lowercase().as_str() {
                        "in" | "inch" | "inches" => "in",
                        "mm" | "millimeter" | "millimeters" => "mm",
                        "cm" | "centimeter" | "centimeters" => "cm",
                        "pt" | "point" | "points" => "pt",
                        "pc" | "pica" => "pc",
                        _ => "in",
                    }
                    .into(),
                )
            }
            "unitvalue" => {
                let s = self.arg_str(args, 0)?;
                let to = if args.len() > 1 { self.arg_str(args, 1)? } else { String::new() };
                let v = parse_num(&s).unwrap_or(0.0);
                let unit: String = s.trim().chars().skip_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+').collect();
                let per_in = |u: &str| match u.trim().to_ascii_lowercase().as_str() {
                    "mm" | "millimeter" | "millimeters" => 25.4,
                    "cm" | "centimeter" | "centimeters" => 2.54,
                    "pt" | "point" | "points" => 72.0,
                    "pc" | "pica" => 6.0,
                    _ => 1.0,
                };
                let target = if to.trim().is_empty() { unit.clone() } else { to };
                Val::Num(v / per_in(&unit) * per_in(&target))
            }
            _ => return Ok(None),
        };
        Ok(Some(value))
    }
}
