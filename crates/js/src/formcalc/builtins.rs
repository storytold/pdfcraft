use super::ast::Expr;
use super::dates::{
    MAX_DAYS, PicCat, civil_from_days, days_from_civil, format_date, format_datetime, format_num_pic, format_text_pic, format_time, format_time_iso,
    format_zero_pic, parse_date, parse_date_exact, parse_datetime, parse_iso_time, parse_num_pic, parse_picture_clauses, parse_text_pic,
    parse_time_ms, parse_time_parts, render_literal_clause,
};
use super::parser::parse;
use super::value::{Val, fixed, is_numeric_text, parse_num};
use super::words::{decode_entities, word_num};
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
                Val::Num(if sub.is_empty() { 0.0 } else { s.find(&sub).and_then(|b| s.get(..b)).map_or(0.0, |p| p.chars().count() as f64 + 1.0) })
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
                let prefix = chars.get(..a).unwrap_or(&[]);
                let suffix = chars.get(b..).unwrap_or(&[]);
                let out: String = prefix.iter().copied().chain(ins.chars()).chain(suffix.iter().copied()).collect();
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
                    ("decode", "xml") => decode_entities(&s, false),
                    ("decode", "html") => decode_entities(&s, true),
                    ("encode", _) => s
                        .bytes()
                        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
                        .collect(),
                    (_, _) => {
                        let mut out = Vec::new();
                        let b = s.as_bytes();
                        let mut i = 0;
                        while let Some(&byte) = b.get(i) {
                            if byte == b'%'
                                && let Some(h) = b
                                    .get(i + 1..i + 3)
                                    .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
                                    .and_then(|h| std::str::from_utf8(h).ok())
                                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                            {
                                out.push(h);
                                i += 3;
                            } else {
                                out.push(byte);
                                i += 1;
                            }
                        }
                        String::from_utf8_lossy(&out).into_owned()
                    }
                })
            }
            "format" => {
                let pic = self.arg_str(args, 0)?;
                let v = self.arg(args, 1)?;
                return self.format_with_picture(&pic, &v);
            }
            "parse" => {
                let pic = self.arg_str(args, 0)?;
                let s = self.arg_str(args, 1)?;
                return self.parse_with_picture(&pic, &s);
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

    fn format_with_picture(&mut self, pic: &str, v: &Val) -> R<Val> {
        let text = self.to_str(v);
        self.charge(((pic.len().saturating_add(text.len())) / 64) as u64)?;
        let clauses = parse_picture_clauses(pic);
        let is_empty_or_null = self.is_null(v) || matches!(v, Val::Str(s) if s.is_empty());
        if is_empty_or_null && let Some(null_clause) = clauses.iter().find(|c| c.cat == PicCat::Null) {
            return Ok(Val::Str(render_literal_clause(&null_clause.pattern)));
        }
        let is_zero_num = !is_empty_or_null && (matches!(v, Val::Num(_)) || is_numeric_text(&text)) && self.to_num(v) == 0.0;
        if is_zero_num && let Some(zero_clause) = clauses.iter().find(|c| c.cat == PicCat::Zero) {
            return Ok(Val::Str(format_zero_pic(&zero_clause.pattern)));
        }
        let data_clauses: Vec<_> = clauses.iter().filter(|c| !matches!(c.cat, PicCat::Null | PicCat::Zero)).collect();
        for c in &data_clauses {
            self.step()?;
            if let Some(out) = self.try_format_clause(c.cat, &c.pattern, v, &text) {
                return Ok(Val::Str(out));
            }
        }
        if let [single] = data_clauses.as_slice()
            && single.cat == PicCat::Num
        {
            return Ok(Val::Str(format_num_pic(self.to_num(v), &single.pattern)));
        }
        Ok(Val::Str(text))
    }

    fn try_format_clause(&self, cat: PicCat, pattern: &str, v: &Val, text: &str) -> Option<String> {
        match cat {
            PicCat::Date => {
                let days = parse_num(text).filter(|d| is_numeric_text(text) && d.abs() < MAX_DAYS).map(|d| d as i64);
                match days {
                    Some(d) => {
                        let (y, m, dd) = civil_from_days(d);
                        Some(format_date(y, m, dd, pattern))
                    }
                    None => parse_date(text, "YYYY-MM-DD").or_else(|| parse_date(text, "YYYYMMDD")).map(|(y, m, d)| format_date(y, m, d, pattern)),
                }
            }
            PicCat::Time => {
                let ms = if matches!(v, Val::Num(n) if n.is_finite() && *n >= 0.0) {
                    Some(self.to_num(v))
                } else if is_numeric_text(text) {
                    parse_num(text).filter(|n| n.is_finite() && *n >= 0.0)
                } else {
                    parse_iso_time(text).map(f64::from)
                };
                ms.map(|m| format_time(m, pattern))
            }
            PicCat::DateTime => {
                let days = parse_num(text).filter(|d| is_numeric_text(text) && d.abs() < MAX_DAYS).map(|d| d as i64);
                format_datetime(text, days, pattern)
            }
            PicCat::Text => format_text_pic(text, pattern),
            PicCat::Num | PicCat::Zero | PicCat::Null => {
                (matches!(v, Val::Num(_)) || is_numeric_text(text)).then(|| format_num_pic(self.to_num(v), pattern))
            }
        }
    }

    fn parse_with_picture(&mut self, pic: &str, s: &str) -> R<Val> {
        self.charge(((pic.len().saturating_add(s.len())) / 64) as u64)?;
        let clauses = parse_picture_clauses(pic);
        for c in &clauses {
            self.step()?;
            match c.cat {
                PicCat::Null => {
                    let lit = render_literal_clause(&c.pattern);
                    if s == lit || (!lit.is_empty() && s.trim() == lit.trim()) {
                        return Ok(Val::Null);
                    }
                }
                PicCat::Zero => {
                    let lit = format_zero_pic(&c.pattern);
                    if s == lit || (!lit.is_empty() && s.trim() == lit.trim()) {
                        return Ok(Val::Num(0.0));
                    }
                }
                PicCat::Date => {
                    if let Some((y, m, d)) = parse_date_exact(s, &c.pattern) {
                        return Ok(Val::Str(format_date(y, m, d, "YYYY-MM-DD")));
                    }
                }
                PicCat::Time => {
                    if let Some((h, m, sec, ms)) = parse_time_parts(s, &c.pattern, true) {
                        return Ok(Val::Str(format_time_iso(h, m, sec, ms)));
                    }
                }
                PicCat::DateTime => {
                    if let Some(iso) = parse_datetime(s, &c.pattern) {
                        return Ok(Val::Str(iso));
                    }
                }
                PicCat::Text => {
                    if let Some(extracted) = parse_text_pic(s, &c.pattern) {
                        return Ok(Val::Str(extracted));
                    }
                }
                PicCat::Num => {
                    if let Some(val) = parse_num_pic(s, &c.pattern) {
                        return Ok(val);
                    }
                }
            }
        }
        Ok(Val::Null)
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
                self.charge(((s.len().saturating_add(pic.len())) / 64) as u64)?;
                let parsed = parse_date(&s, &pic).or_else(|| parse_date(&s, "YYYYMMDD")).or_else(|| parse_date(&s, "YYYY-MM-DD"));
                parsed.map_or(Val::Num(0.0), |(y, m, d)| Val::Num(days_from_civil(y, m, d) as f64))
            }
            "num2date" => {
                let days = self.arg_num(args, 0)?;
                let pic = if args.len() > 1 { self.arg_str(args, 1)? } else { "MMM D, YYYY".into() };
                self.charge((pic.len() / 64) as u64)?;
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
            "timefmt" | "localtimefmt" => {
                let style = if args.is_empty() { 0.0 } else { self.arg_num(args, 0)? };
                Val::Str(
                    match style as i32 {
                        1 => "h:MM A",
                        2 => "h:MM:SS A",
                        3 | 4 => "h:MM:SS A Z",
                        _ => "HH:MM:SS",
                    }
                    .into(),
                )
            }
            "num2time" | "num2gmtime" => {
                let ms = self.arg_num(args, 0)?;
                let pic = if args.len() > 1 { self.arg_str(args, 1)? } else { "HH:MM:SS".into() };
                self.charge((pic.len() / 64) as u64)?;
                if !ms.is_finite() || ms < 0.0 {
                    return Ok(Some(Val::Null));
                }
                Val::Str(format_time(ms, &pic))
            }
            "time2num" | "isotime2num" => {
                let s = self.arg_str(args, 0)?;
                let parsed = if lower == "isotime2num" || args.len() < 2 {
                    self.charge((s.len() / 64) as u64)?;
                    parse_iso_time(&s).or_else(|| parse_time_ms(&s, "HH:MM:SS"))
                } else {
                    let pic = self.arg_str(args, 1)?;
                    self.charge(((s.len().saturating_add(pic.len())) / 64) as u64)?;
                    parse_time_ms(&s, &pic).or_else(|| parse_iso_time(&s))
                };
                Val::Num(parsed.map_or(0.0, f64::from))
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

#[cfg(test)]
mod tests {
    use crate::Limits;
    use crate::formcalc::run_formcalc;
    use crate::xfa::{XfaDoc, XfaEvent, XfaNode};

    fn eval_fc(script: &str) -> String {
        let root = XfaNode::default();
        let out = run_formcalc(script, &XfaEvent::default(), &XfaDoc::default(), &root, Limits::default());
        assert!(out.error.is_none(), "unexpected error in `{script}`: {:?}", out.error);
        out.result.unwrap_or_default()
    }

    fn assert_cases(cases: &[(&str, &str)]) {
        for &(script, expected) in cases {
            assert_eq!(eval_fc(script), expected, "{script}");
        }
    }

    #[test]
    fn time_picture_clauses_format_and_parse_all_clock_symbols() {
        assert_cases(&[
            // 24-hour H / HH and 1..=24 k / kk (where 24 = midnight 00:00)
            (r#"Format("time{HH:MM:SS}", 0)"#, "00:00:00"),
            (r#"Format("time{H:M:S}", 3661000)"#, "1:1:1"),
            (r#"Format("time{kk:MM:SS}", 0)"#, "24:00:00"),
            (r#"Format("time{k:MM}", 3600000)"#, "1:00"),
            (r#"Parse("time{kk:MM:SS}", "24:15:30")"#, "00:15:30"),
            (r#"Parse("time{kk:MM:SS}", "00:15:30") == null"#, "1"),
            (r#"Parse("time{kk:MM:SS}", "25:00:00") == null"#, "1"),
            (r#"Parse("time{HH:MM:SS}", "24:00:00") == null"#, "1"),
            // 12-hour h / hh (1..=12) vs K / KK (0..=11) with meridiem A and AA
            (r#"Format("time{h:MM A}", 0)"#, "12:00 A"),
            (r#"Format("time{hh:MM AA}", 0)"#, "12:00 AM"),
            (r#"Format("time{K:MM AA}", 0)"#, "0:00 AM"),
            (r#"Format("time{KK:MM AA}", 43200000)"#, "00:00 PM"),
            (r#"Format("time{h:MM:SS AA}", 43200000)"#, "12:00:00 PM"),
            (r#"Format("time{h:MM:SS A}", 52205000)"#, "2:30:05 P"),
            (r#"Format("time{hh:MM:SS AA}", 52205000)"#, "02:30:05 PM"),
            (r#"Parse("time{h:MM AA}", "12:30 AM")"#, "00:30:00"),
            (r#"Parse("time{h:MM AA}", "12:30 PM")"#, "12:30:00"),
            (r#"Parse("time{h:MM AA}", "0:30 AM") == null"#, "1"),
            (r#"Parse("time{h:MM AA}", "13:30 PM") == null"#, "1"),
            (r#"Parse("time{h:MM A}", "2:30 P")"#, "14:30:00"),
            (r#"Parse("time{h:MM A}", "2:30 pm")"#, "14:30:00"),
            (r#"Parse("time{K:MM AA}", "0:45 AM")"#, "00:45:00"),
            (r#"Parse("time{K:MM AA}", "0:45 PM")"#, "12:45:00"),
            (r#"Parse("time{K:MM AA}", "11:59 PM")"#, "23:59:00"),
            (r#"Parse("time{K:MM AA}", "12:00 PM") == null"#, "1"),
            // Sub-second F / FF / FFF tokens and quoted literals
            (r#"Format("time{HH:MM:SS.F}", 52205250)"#, "14:30:05.2"),
            (r#"Format("time{HH:MM:SS.FF}", 52205250)"#, "14:30:05.25"),
            (r#"Format("time{HH:MM:SS.FFF}", 52205250)"#, "14:30:05.250"),
            (r#"Parse("time{HH:MM:SS.F}", "14:30:05.2")"#, "14:30:05.200"),
            (r#"Parse("time{HH:MM:SS.FF}", "14:30:05.25")"#, "14:30:05.250"),
            (r#"Parse("time{HH:MM:SS.FFF}", "14:30:05.250")"#, "14:30:05.250"),
            (r#"Parse("time{HH:MM:SS.FFF}", "14:30:05.000")"#, "14:30:05"),
            (r#"Format("time{h:MM:SS.FFF AA}", "14:30:05.250")"#, "2:30:05.250 PM"),
            (r#"Format("time{'At 'HH'h'MM'm'}", "09:05:00")"#, "At 09h05m"),
            (r#"Parse("time{'At 'HH'h'MM'm'}", "At 09h05m")"#, "09:05:00"),
        ]);
    }

    #[test]
    fn time_builtins_and_timezones_convert_to_and_from_gmt() {
        assert_cases(&[
            ("TimeFmt()", "HH:MM:SS"),
            ("TimeFmt(1)", "h:MM A"),
            ("TimeFmt(2)", "h:MM:SS A"),
            ("TimeFmt(3)", "h:MM:SS A Z"),
            ("LocalTimeFmt()", "HH:MM:SS"),
            ("LocalTimeFmt(4)", "h:MM:SS A Z"),
            (r#"Num2Time(3661000)"#, "01:01:01"),
            (r#"Num2Time(52205000, "time{h:MM:SS AA}")"#, "2:30:05 PM"),
            (r#"Num2GMTime(52205000, "HH:MM:SS Z")"#, "14:30:05 Z"),
            (r#"Num2Time(-1) == null"#, "1"),
            (r#"Num2GMTime(-100) == null"#, "1"),
            (r#"Time2Num("01:00:00")"#, "3600000"),
            (r#"Time2Num("2:30:05 PM", "h:MM:SS A")"#, "52205000"),
            (r#"Time2Num("14:30:00-05:00", "HH:MM:SS Z")"#, "70200000"),
            (r#"Time2Num("14:30:00+0200", "HH:MM:SS Z")"#, "45000000"),
            (r#"Time2Num("14:30:00-0500", "HH:MM:SS Z")"#, "70200000"),
            (r#"Time2Num("14:30:00 GMT-05:00", "HH:MM:SS z")"#, "70200000"),
            (r#"Time2Num("14:30:00 GMT+02:00", "HH:MM:SS z")"#, "45000000"),
            (r#"Parse("time{HH:MM:SS Z}", "14:30:00-05:00")"#, "19:30:00"),
            (r#"Parse("time{HH:MM:SS z}", "14:30:00 GMT+02:00")"#, "12:30:00"),
            (r#"Format("time{HH:MM:SS z}", 52205000)"#, "14:30:05 GMT"),
            (r#"Parse("time{HH:MM:SS z}", "14:30:05 GMT")"#, "14:30:05"),
            (r#"Parse("time{HH:MM:SS Z}", "14:30:00Z")"#, "14:30:00"),
            (r#"Time2Num("not-a-time", "HH:MM:SS")"#, "0"),
        ]);
    }

    #[test]
    fn text_picture_clauses_format_and_parse_placeholders_and_literals() {
        assert_cases(&[
            (r#"Format("text{(999) 999-9999}", "8005550199")"#, "(800) 555-0199"),
            (r#"Parse("text{(999) 999-9999}", "(800) 555-0199")"#, "8005550199"),
            (r#"Parse("text{(999) 999-9999}", "(800) ABC-0199") == null"#, "1"),
            (r#"Format("(999) 999-9999", "8005550199")"#, "(800) 555-0199"),
            (r#"Parse("(999) 999-9999", "(800) 555-0199")"#, "8005550199"),
            (r#"Format("AAA-999", "ABC123")"#, "ABC-123"),
            (r#"Parse("AAA-999", "ABC-123")"#, "ABC123"),
            (r#"Format("text{AAA-999-OO-00-X+}", "Abc123A1B2?-")"#, "Abc-123-A1-B2-?-"),
            (r#"Parse("text{AAA-999-OO-00-X+}", "Abc-123-A1-B2-?-")"#, "Abc123A1B2?-"),
            (r#"Format("text{+99}", " 42")"#, " 42"),
            (r#"Format("text{+99}", "+42")"#, "+42"),
            (r#"Format("text{+99}", "A42")"#, "A42"),
            (r#"Format("text{'It''s #'\9: AAA}", "XYZ")"#, "It's #9: XYZ"),
            (r#"Parse("text{'It''s #'\9: AAA}", "It's #9: XYZ")"#, "XYZ"),
            (r#"Format("text{AA-XX-OO}", "éß★☺Ω7")"#, "éß-★☺-Ω7"),
            (r#"Parse("text{AA-XX-OO}", "éß-★☺-Ω7")"#, "éß★☺Ω7"),
            (r#"Format("text{AAA-999}", "too-short")"#, "too-short"),
            (r#"Format("text{AAA}", "123")"#, "123"),
        ]);
    }

    #[test]
    fn number_picture_clauses_support_wrappers_signs_parens_cr_db_and_percent() {
        assert_cases(&[
            (r#"Format("num(en_US){$z,zz9.99}", 1234.5)"#, "$1,234.50"),
            (r#"Format("num{($z,zz9.99)}", -1234.5)"#, "($1,234.50)"),
            (r#"Format("num{($z,zz9.99)}", 1234.5)"#, "$1,234.50"),
            (r#"Format("$(z,zz9.99)", -1234.5)"#, "$(1,234.50)"),
            (r#"Format("(z,zz9.99)", -1234.5)"#, "(1,234.50)"),
            (r#"Format("z,zz9.99-", -1234.5)"#, "1,234.50-"),
            (r#"Format("z,zz9.99-", 1234.5)"#, "1,234.50"),
            (r#"Format("z,zz9.99 CR", -1234.5)"#, "1,234.50 CR"),
            (r#"Format("z,zz9.99 CR", 1234.5)"#, "1,234.50"),
            (r#"Format("z,zz9.99 DB", -1234.5)"#, "1,234.50 DB"),
            (r#"Format("+z,zz9.99", 1234.5)"#, "+1,234.50"),
            (r#"Format("+z,zz9.99", -1234.5)"#, "-1,234.50"),
            (r#"Format("z,zz9.99+", 1234.5)"#, "1,234.50+"),
            (r#"Format("z,zz9.99+", -1234.5)"#, "1,234.50-"),
            (r#"Format("sz,zz9.99", -1234.5)"#, "-1,234.50"),
            (r#"Format("Sz,zz9.99", 1234.5)"#, "1,234.50"),
            (r#"Format("num{zz9.99%}", 0.1234)"#, "12.34%"),
            (r#"Format("num{zz9.99'%' }", 0.1234)"#, "0.12% "),
            (r#"Format("num{'Total: '$z,zz9.99' USD'}", 42.5)"#, "Total: $42.50 USD"),
            (r#"Parse("num{($z,zz9.99)}", "($1,234.50)")"#, "-1234.5"),
            (r#"Parse("(z,zz9.99)", "(1,234.50)")"#, "-1234.5"),
            (r#"Parse("z,zz9.99-", "1,234.50-")"#, "-1234.5"),
            (r#"Parse("z,zz9.99 CR", "1,234.50 CR")"#, "-1234.5"),
            (r#"Parse("z,zz9.99 DB", "1,234.50 DB")"#, "-1234.5"),
            (r#"Parse("+z,zz9.99", "+1,234.50")"#, "1234.5"),
            (r#"Parse("num{zz9.99%}", "12.34%")"#, "0.1234"),
            (r#"Parse("num{zzz,zz9.99}", "not-a-number") == null"#, "1"),
            (r#"Parse("num{zzz,zz9.99}", "") == null"#, "1"),
            (r#"Parse("num{z,zz9.99}", "--123") == null"#, "1"),
            (r#"Parse("num{z,zz9.99}", "((123))") == null"#, "1"),
        ]);
    }

    #[test]
    fn compound_alternatives_zero_null_and_datetime_clauses_work() {
        assert_cases(&[
            (r#"Format("num{$z,zz9.99}|zero{'Free'}|null{'N/A'}", 25.5)"#, "$25.50"),
            (r#"Format("num{$z,zz9.99}|zero{'Free'}|null{'N/A'}", 0)"#, "Free"),
            (r#"Format("num{$z,zz9.99}|zero{'Free'}|null{'N/A'}", null)"#, "N/A"),
            (r#"Format("num{$z,zz9.99}|zero{'Free'}|null{'N/A'}", "")"#, "N/A"),
            (r#"Format("num{$z,zz9.99}|text{AAA-999}", "ABC123")"#, "ABC-123"),
            (r#"Format("num{$z,zz9.99}|date{MM/DD/YYYY}", "2024-02-29")"#, "02/29/2024"),
            (r#"Parse("num{$z,zz9.99}|zero{'Free'}|null{'N/A'}", "$25.50")"#, "25.5"),
            (r#"Parse("num{$z,zz9.99}|zero{'Free'}|null{'N/A'}", "Free")"#, "0"),
            (r#"Parse("num{$z,zz9.99}|zero{'Free'}|null{'N/A'}", "N/A") == null"#, "1"),
            (r#"Format("num{$z,zz9.99}|zero{9.99}|null{}", 0)"#, "0.00"),
            (r#"Format("num{$z,zz9.99}|zero{9.99}|null{}", null)"#, ""),
            (r#"Parse("num{$z,zz9.99}|zero{9.99}|null{}", "0.00")"#, "0"),
            (r#"Parse("num{$z,zz9.99}|zero{9.99}|null{}", "") == null"#, "1"),
            (r#"Format("text{AAA'|'999}", "ABC123")"#, "ABC|123"),
            (r#"Parse("text{AAA'|'999}", "ABC|123")"#, "ABC123"),
            (r#"Format("datetime{YYYY-MM-DD'T'HH:MM:SS}", "2024-02-29T14:30:05")"#, "2024-02-29T14:30:05"),
            (r#"Format("datetime{MMM D, YYYY h:MM AA}", "2024-02-29T14:30:00")"#, "Feb 29, 2024 2:30 PM"),
            (r#"Parse("datetime{MMM D, YYYY h:MM AA}", "Feb 29, 2024 2:30 PM")"#, "2024-02-29T14:30:00"),
            (r#"Format("datetime{HH:MM MM/DD/YYYY}", "2024-02-29T14:30:00")"#, "14:30 02/29/2024"),
            (r#"Parse("datetime{HH:MM MM/DD/YYYY}", "14:30 02/29/2024")"#, "2024-02-29T14:30:00"),
            (r#"Parse("datetime{YYYY-MM-DD'T'HH:MM:SS Z}", "2024-02-29T14:30:00Z")"#, "2024-02-29T14:30:00"),
            (r#"Parse("datetime{YYYY-MM-DD'T'HH:MM:SS Z}", "2024-02-29T21:30:00-05:00")"#, "2024-03-01T02:30:00"),
            (r#"Parse("date{YYYY-MM-DD}|date{MM/DD/YYYY}|time{h:MM AA}", "02/29/2024")"#, "2024-02-29"),
            (r#"Parse("date{YYYY-MM-DD}|date{MM/DD/YYYY}|time{h:MM AA}", "2:30 PM")"#, "14:30:00"),
            (r#"Parse(DateFmt(4), Num2Date(45350, DateFmt(4)))"#, "2024-02-29"),
            (r#"Format("date{YYYY-JJJ}", "2024-02-29")"#, "2024-060"),
            (r#"Parse("date{YYYY-JJJ}", "2024-060")"#, "2024-02-29"),
        ]);
    }

    #[test]
    fn hostile_and_boundary_picture_inputs_never_panic_or_exceed_limits() {
        assert_cases(&[
            (r#"Format("text{'unclosed", "abc")"#, "abc"),
            (r#"Parse("text{'unclosed}", "unclosed}")"#, ""),
            (r#"Format("num{999\}", 5)"#, "005"),
            (r#"Parse("||||", "abc") == null"#, "1"),
            (r#"Format("{{{999}}}", 7)"#, "{{{007}}}"),
            (r#"Num2Date(1000000000) == null"#, "1"),
            (r#"Num2Time(86400000, "HH:MM:SS")"#, "00:00:00"),
            (r#"Len(Num2Time(1e30, "HH:MM:SS"))"#, "8"),
        ]);
        assert!(eval_fc(r#"Format("num{$z,zz9.99}", 1e20)"#).starts_with('$'));

        let root = XfaNode::default();
        let out =
            run_formcalc(r#"Format(Replace(Space(300000), " ", "MMMM"), 1)"#, &XfaEvent::default(), &XfaDoc::default(), &root, Limits::default());
        assert!(out.error.as_deref().unwrap_or_default().contains("too long"));
    }
}
