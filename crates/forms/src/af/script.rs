use super::calc::{js_tokens, notation_js, notation_range, notation_tokens};
use super::format::{DATE_PRESETS, TIME_PRESETS};
use super::{CalcOp, Calculate, Format, Validate};

// ── parsing scripts ─────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Arg {
    Num(f64),
    Str(String),
    Bool(bool),
    List(Vec<Arg>),
}

impl Arg {
    fn num(&self) -> Option<f64> {
        match self {
            Arg::Num(n) => Some(*n),
            Arg::Bool(b) => Some(f64::from(u8::from(*b))),
            Arg::Str(s) => s.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
            Arg::List(_) => None,
        }
    }

    pub(super) fn string(&self) -> Option<String> {
        match self {
            Arg::Str(s) => Some(s.clone()),
            Arg::Num(n) => Some(n.to_string()),
            _ => None,
        }
    }

    fn truthy(&self) -> bool {
        match self {
            Arg::Bool(b) => *b,
            Arg::Num(n) => *n != 0.0,
            Arg::Str(s) => !s.is_empty(),
            Arg::List(l) => !l.is_empty(),
        }
    }
}

/// How deeply script arguments and field notation may nest; deeper input is rejected rather
/// than recursed into (a hostile file could otherwise overflow the stack).
pub(super) const MAX_NESTING: usize = 64;

fn hex4(bytes: &[u8]) -> Option<u16> {
    if bytes.len() != 4 {
        return None;
    }
    bytes.iter().try_fold(0u16, |value, byte| {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        value.checked_mul(16)?.checked_add(u16::from(digit))
    })
}

pub(super) struct Parser<'a> {
    pub(super) s: &'a [u8],
    pub(super) i: usize,
    pub(super) depth: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    pub(super) fn string(&mut self) -> Option<String> {
        let q = self.s[self.i];
        self.i += 1;
        let mut out = Vec::new();
        while self.i < self.s.len() {
            let c = self.s[self.i];
            self.i += 1;
            match c {
                b'\\' if self.i < self.s.len() => {
                    let e = self.s[self.i];
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0C),
                        b'v' => out.push(0x0B),
                        b'0' => out.push(0),
                        b'x' => {
                            let digits = self.s.get(self.i..self.i.checked_add(2)?)?;
                            let code = hex4(&[b'0', b'0', digits[0], digits[1]])?;
                            self.i += 2;
                            let ch = char::from_u32(u32::from(code)).unwrap_or('\u{FFFD}');
                            let mut encoded = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
                        }
                        b'u' if self.s.get(self.i) == Some(&b'{') => {
                            self.i += 1;
                            let start = self.i;
                            let mut code = 0u32;
                            while let Some(&digit) = self.s.get(self.i).filter(|&&b| b != b'}') {
                                code = code.checked_mul(16)?.checked_add(char::from(digit).to_digit(16)?)?;
                                if code > 0x10FFFF {
                                    return None;
                                }
                                self.i += 1;
                            }
                            if self.i == start || self.s.get(self.i) != Some(&b'}') {
                                return None;
                            }
                            self.i += 1;
                            // Match the existing handling of unpaired UTF-16 surrogates.
                            let ch = char::from_u32(code).unwrap_or('\u{FFFD}');
                            let mut encoded = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
                        }
                        b'u' => {
                            let code = hex4(self.s.get(self.i..self.i.checked_add(4)?)?)?;
                            self.i += 4;
                            let code = if (0xD800..=0xDBFF).contains(&code)
                                && let Some(pair) = self.i.checked_add(6).and_then(|end| self.s.get(self.i..end))
                                && pair.starts_with(b"\\u")
                                && let Some(low) = hex4(&pair[2..6]).filter(|low| (0xDC00..=0xDFFF).contains(low))
                            {
                                self.i += 6;
                                0x10000 + ((u32::from(code) - 0xD800) << 10) + (u32::from(low) - 0xDC00)
                            } else {
                                u32::from(code)
                            };
                            let ch = char::from_u32(code).unwrap_or('\u{FFFD}');
                            let mut encoded = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                c if c == q => return Some(String::from_utf8_lossy(&out).into_owned()),
                c => out.push(c),
            }
        }
        None
    }

    fn arg(&mut self) -> Option<Arg> {
        self.ws();
        let c = *self.s.get(self.i)?;
        match c {
            b'"' | b'\'' => self.string().map(Arg::Str),
            b'[' => {
                self.i += 1;
                self.nested(b']').map(Arg::List)
            }
            _ if self.s[self.i..].starts_with(b"new Array") => {
                self.i += b"new Array".len();
                self.ws();
                if self.s.get(self.i) != Some(&b'(') {
                    return None;
                }
                self.i += 1;
                self.nested(b')').map(Arg::List)
            }
            _ if self.s[self.i..].starts_with(b"true") => {
                self.i += 4;
                Some(Arg::Bool(true))
            }
            _ if self.s[self.i..].starts_with(b"false") => {
                self.i += 5;
                Some(Arg::Bool(false))
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || matches!(self.s[self.i], b'.' | b'-' | b'+' | b'e' | b'E')) {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[start..self.i]).ok()?.parse::<f64>().ok().filter(|n| n.is_finite()).map(Arg::Num)
            }
        }
    }

    /// A list inside another, at most [`MAX_NESTING`] deep.
    fn nested(&mut self, close: u8) -> Option<Vec<Arg>> {
        if self.depth >= MAX_NESTING {
            return None;
        }
        self.depth += 1;
        let out = self.list(close);
        self.depth -= 1;
        out
    }

    fn list(&mut self, close: u8) -> Option<Vec<Arg>> {
        let mut out = Vec::new();
        loop {
            self.ws();
            if self.s.get(self.i) == Some(&close) {
                self.i += 1;
                return Some(out);
            }
            out.push(self.arg()?);
            self.ws();
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(c) if *c == close => {
                    self.i += 1;
                    return Some(out);
                }
                _ => return None,
            }
        }
    }
}

/// The first call to `name(` in `js` and its arguments.
pub(super) fn call(js: &str, name: &str) -> Option<Vec<Arg>> {
    let (tokens, _) = js_tokens(js);
    for (k, (range, tok)) in tokens.iter().enumerate() {
        if *tok != super::calc::Js::Ident {
            continue;
        }
        if k > 0 && matches!(tokens.get(k - 1), Some((_, super::calc::Js::Punct('.')))) {
            continue;
        }
        let Some(target_end) = range.start.checked_add(name.len()) else { continue };
        if js.get(range.start..target_end) != Some(name) {
            continue;
        }
        let Some(end_k) = (k..tokens.len()).take_while(|&m| tokens[m].0.end <= target_end).find(|&m| tokens[m].0.end == target_end) else {
            continue;
        };
        let Some((open, super::calc::Js::Punct('('))) = tokens.get(end_k + 1) else { continue };
        let mut p = Parser { s: js.as_bytes(), i: open.end, depth: 0 };
        if let Some(args) = p.list(b')') {
            return Some(args);
        }
    }
    None
}

pub fn parse_format(js: &str) -> Option<Format> {
    let a = |args: &[Arg], i: usize| args.get(i).and_then(Arg::num).unwrap_or(0.0).clamp(0.0, 255.0) as u8;
    if let Some(args) = call(js, "AFNumber_Format") {
        return Some(Format::Number {
            decimals: a(&args, 0),
            sep: a(&args, 1),
            neg: a(&args, 2),
            currency: args.get(4).and_then(Arg::string).unwrap_or_default(),
            prepend: args.get(5).is_none_or(Arg::truthy),
        });
    }
    if let Some(args) = call(js, "AFPercent_Format") {
        return Some(Format::Percent { decimals: a(&args, 0), sep: a(&args, 1) });
    }
    if let Some(args) = call(js, "AFDate_FormatEx") {
        return Some(Format::Date(args.first().and_then(Arg::string)?));
    }
    if let Some(args) = call(js, "AFDate_Format") {
        return Some(Format::Date(DATE_PRESETS.get(a(&args, 0) as usize)?.to_string()));
    }
    if let Some(args) = call(js, "AFTime_FormatEx") {
        return Some(Format::Time(args.first().and_then(Arg::string)?));
    }
    if let Some(args) = call(js, "AFTime_Format") {
        return Some(Format::Time(TIME_PRESETS.get(a(&args, 0) as usize)?.to_string()));
    }
    if let Some(args) = call(js, "AFSpecial_Format") {
        return Some(Format::Special(a(&args, 0).min(3)));
    }
    if let Some(args) = call(js, "AFSpecial_KeystrokeEx") {
        return Some(Format::Mask(args.first().and_then(Arg::string)?));
    }
    None
}

pub fn parse_validate(js: &str) -> Option<Validate> {
    let args = call(js, "AFRange_Validate")?;
    let flag = |i: usize| args.get(i).is_some_and(Arg::truthy);
    let num = |i: usize| args.get(i).and_then(Arg::num);
    // An enabled but unparseable bound is unsupported, not an absent bound.
    Some(Validate::Range { min: if flag(0) { Some(num(1)?) } else { None }, max: if flag(2) { Some(num(3)?) } else { None } })
}

pub fn parse_calculate(js: &str) -> Option<Calculate> {
    // Simplified field notation: Acrobat keeps the expression in a comment. A script is read for
    // its comments only when it spells the marker.
    if js.contains("BVCALC")
        && let Some(range) = notation_range(js, &js_tokens(js).1)
    {
        return Some(Calculate::Notation(notation_text(js.get(range)?).to_string()));
    }
    let args = call(js, "AFSimple_Calculate")?;
    let op = match args.first().and_then(Arg::string)?.as_str() {
        "SUM" => CalcOp::Sum,
        "PRD" => CalcOp::Product,
        "AVG" => CalcOp::Average,
        "MIN" => CalcOp::Minimum,
        "MAX" => CalcOp::Maximum,
        _ => return None,
    };
    let fields = match args.get(1)? {
        Arg::List(l) => l.iter().filter_map(Arg::string).collect(),
        Arg::Str(s) => s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
        _ => return None,
    };
    Some(Calculate::Simple { op, fields })
}

// ── writing scripts (what Acrobat's tabs generate) ─────────────────────────────────────────

pub(super) fn js_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                o.push('\\');
                o.push(c);
            }
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c.is_ascii() && !c.is_control() => o.push(c),
            c => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    o.push_str(&format!("\\u{unit:04X}"));
                }
            }
        }
    }
    o.push('"');
    o
}

/// The (format, keystroke) scripts for a format.
pub fn format_js(f: &Format) -> Option<(String, String)> {
    Some(match f {
        Format::None => return None,
        Format::Number { decimals, sep, neg, currency, prepend } => {
            let args = format!("{decimals}, {sep}, {neg}, 0, {}, {prepend}", js_str(currency));
            (format!("AFNumber_Format({args});"), format!("AFNumber_Keystroke({args});"))
        }
        Format::Percent { decimals, sep } => (format!("AFPercent_Format({decimals}, {sep});"), format!("AFPercent_Keystroke({decimals}, {sep});")),
        Format::Date(f) => (format!("AFDate_FormatEx({});", js_str(f)), format!("AFDate_KeystrokeEx({});", js_str(f))),
        Format::Time(f) => (format!("AFTime_FormatEx({});", js_str(f)), format!("AFTime_KeystrokeEx({});", js_str(f))),
        Format::Special(n) => (format!("AFSpecial_Format({n});"), format!("AFSpecial_Keystroke({n});")),
        Format::Mask(m) => (String::new(), format!("AFSpecial_KeystrokeEx({});", js_str(m))),
    })
}

pub fn validate_js(v: &Validate) -> Option<String> {
    match v {
        Validate::None => None,
        Validate::Range { min, max } => {
            Some(format!("AFRange_Validate({}, {}, {}, {});", min.is_some(), min.unwrap_or(0.0), max.is_some(), max.unwrap_or(0.0)))
        }
    }
}

/// Why [`notation_storable`] refuses a notation, in the words the Calculate tab and the tools say it.
pub const NOTATION_NOT_STORABLE: &str = "the notation can't be stored as written: it is empty, or holds EVCALC or */, or ends in a lone \\";

/// Whether simplified field notation `expr` can be stored: the script keeps it in a comment,
/// between `BVCALC` and `EVCALC`, and it has to read back from there as the same names and
/// operators. One holding `EVCALC` or `*/` ends early, and one ending in a lone `\` escapes the
/// space after it. An empty one is refused too: it would be stored as `event.value = ;`.
pub fn notation_storable(expr: &str) -> bool {
    let tokens = |s: &str| notation_tokens(s).into_iter().map(|(range, _)| s.get(range).unwrap_or_default().to_owned()).collect::<Vec<_>>();
    let read = calculate_js(&Calculate::Notation(expr.to_owned())).and_then(|js| parse_calculate(&js));
    !tokens(expr).is_empty() && matches!(read, Some(Calculate::Notation(n)) if tokens(&n) == tokens(expr))
}

/// `src` from its first notation token to its last: the spaces around it left out, but not an
/// escaped space that ends the last name (`Total\ `), as `str::trim` would.
fn notation_text(src: &str) -> &str {
    let tokens = notation_tokens(src);
    match (tokens.first(), tokens.last()) {
        (Some((first, _)), Some((last, _))) => src.get(first.start..last.end).unwrap_or_default(),
        _ => "",
    }
}

pub fn calculate_js(c: &Calculate) -> Option<String> {
    match c {
        Calculate::None => None,
        Calculate::Simple { op, fields } => Some(format!(
            "AFSimple_Calculate({}, new Array({}));",
            js_str(op.code()),
            fields.iter().map(|f| js_str(f)).collect::<Vec<_>>().join(", ")
        )),
        // Acrobat's own translation follows the comment, so a viewer that runs the script instead
        // of reading the comment computes the same value (a placeholder here showed 0).
        Calculate::Notation(expr) => Some(format!("/** BVCALC {expr} EVCALC **/ event.value = {};", notation_js(expr))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpaired_high_surrogate_at_end_of_string_does_not_abort_parsing() {
        let js = "AFDate_FormatEx(\"\\uD800\")";
        assert_eq!(parse_format(js), Some(Format::Date("\u{FFFD}".to_owned())));
    }

    #[test]
    fn call_skips_commented_or_prefixed_identifiers_and_rejects_infinite_numbers() {
        let js = "// AFPercent_Format(0, 0);\nmy_AFPercent_Format(0, 0); $AFPercent_Format(0, 0); AFPercent_Format (2, 1);";
        assert_eq!(parse_format(js), Some(Format::Percent { decimals: 2, sep: 1 }));

        assert_eq!(parse_validate("AFRange_Validate(true, 1e500, false, 0);"), None);
    }
    #[test]
    fn review_enabled_nonfinite_bound_is_unsupported() {
        for script in [r#"AFRange_Validate(true, "1e500", false, 0)"#, r#"AFRange_Validate(false, 0, true, "-1e500")"#] {
            assert_eq!(parse_validate(script), None);
        }
        assert_eq!(parse_validate(r#"AFRange_Validate(false, "1e500", false, "NaN")"#), Some(Validate::Range { min: None, max: None }));
    }

    #[test]
    fn review_format_after_postfix_division() {
        for op in ["++", "--"] {
            let js = format!("var y = x{op} / 2; AFPercent_Format(2, 1); var z = 1 / 2;");
            assert_eq!(parse_format(&js), Some(Format::Percent { decimals: 2, sep: 1 }));
            let commented = format!("var y = x/* comment */{op} / 2; AFPercent_Format(2, 1); var z = 1 / 2;");
            assert_eq!(parse_format(&commented), Some(Format::Percent { decimals: 2, sep: 1 }));
        }
        // A newline makes ++ a prefix operator; the following regex must remain opaque.
        for js in ["x\n++/AFPercent_Format(2, 1)/.lastIndex;", "x/*\n*/++/AFPercent_Format(2, 1)/.lastIndex;"] {
            assert_eq!(parse_format(js), None);
        }
    }

    #[test]
    fn review_format_after_brace_unicode() {
        assert_eq!(parse_format(r#"var s = "\u{1F600}"; AFPercent_Format(2, 1);"#), Some(Format::Percent { decimals: 2, sep: 1 }));
        assert_eq!(parse_format(r#"AFDate_FormatEx("\u{1F600}")"#), Some(Format::Date("😀".to_owned())));
        assert_eq!(parse_format(r#"AFDate_FormatEx("\u{00000041}\u0042\uD83D\uDE00")"#), Some(Format::Date("AB😀".to_owned())));
        for escape in ["\\u{}", "\\u{110000}", "\\u{xyz}", "\\u{41"] {
            assert_eq!(parse_format(&format!("AFDate_FormatEx(\"{escape}\")")), None);
        }
    }

    #[test]
    fn review_notation_separates_adjacent_operators() {
        let field = |name| format!("AFMakeNumber(this.getField(\"{name}\").value)");
        for (expr, operators) in [("a++b", "+ +"), ("a--b", "- -"), ("a+++b", "+ + +")] {
            let js = calculate_js(&Calculate::Notation(expr.to_owned())).unwrap();
            let executable = js.split("event.value = ").nth(1).unwrap();
            assert_eq!(executable, format!("{}{operators}{};", field("a"), field("b")));
            assert_eq!(super::super::calc::calculate(&Calculate::Notation(expr.to_owned()), &|_| vec!["2".to_owned()]), Some("4".to_owned()));
        }
    }
}
