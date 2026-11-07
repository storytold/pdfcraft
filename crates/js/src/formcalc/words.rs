const ONES: [&str; 20] = [
    "Zero",
    "One",
    "Two",
    "Three",
    "Four",
    "Five",
    "Six",
    "Seven",
    "Eight",
    "Nine",
    "Ten",
    "Eleven",
    "Twelve",
    "Thirteen",
    "Fourteen",
    "Fifteen",
    "Sixteen",
    "Seventeen",
    "Eighteen",
    "Nineteen",
];

const TENS: [&str; 10] = ["", "", "Twenty", "Thirty", "Forty", "Fifty", "Sixty", "Seventy", "Eighty", "Ninety"];

fn words_below_1000(n: u64) -> String {
    let mut parts = Vec::new();
    if n >= 100 {
        parts.push(format!("{} Hundred", ONES[(n / 100) as usize]));
    }
    let r = n % 100;
    if r >= 20 {
        parts.push(if r.is_multiple_of(10) {
            TENS[(r / 10) as usize].to_string()
        } else {
            format!("{}-{}", TENS[(r / 10) as usize], ONES[(r % 10) as usize])
        });
    } else if r > 0 || n == 0 {
        parts.push(ONES[r as usize].to_string());
    }
    parts.join(" ")
}

/// `WordNum`: an integer in English words (up to the billions).
pub(super) fn word_num(n: f64, cents: bool) -> String {
    if !n.is_finite() || !(0.0..1e12).contains(&n) {
        return String::new();
    }
    let cents_total = (n * 100.0).round() as u64;
    let whole = if cents { cents_total / 100 } else { n.trunc() as u64 };
    if whole >= 1_000_000_000_000 {
        // Rounding to cents carried past the supported range.
        return String::new();
    }
    let mut parts = Vec::new();
    let groups = [(1_000_000_000u64, "Billion"), (1_000_000, "Million"), (1_000, "Thousand")];
    let mut rest = whole;
    for (unit, name) in groups {
        if rest >= unit {
            parts.push(format!("{} {name}", words_below_1000(rest / unit)));
            rest %= unit;
        }
    }
    if rest > 0 || parts.is_empty() {
        parts.push(words_below_1000(rest));
    }
    let mut out = parts.join(" ");
    if cents {
        let c = cents_total % 100;
        let dollars = if whole == 1 { "Dollar" } else { "Dollars" };
        let cent = if c == 1 { "Cent" } else { "Cents" };
        out.push_str(&format!(" {dollars} And {c:02} {cent}"));
    }
    out
}

/// Decode XML (`html == false`) or HTML (`html == true`) character and entity references in a
/// single pass so `&amp;lt;` becomes `&lt;` rather than `<`, and invalid or unrecognized
/// references remain literal without panicking.
pub(super) fn decode_entities(s: &str, html: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(rest.get(..at).unwrap_or_default());
        rest = rest.get(at..).unwrap_or_default();
        match entity_at(rest, html) {
            Some((c, len)) => {
                out.push(c);
                rest = rest.get(len..).unwrap_or_default();
            }
            None => {
                out.push('&');
                rest = rest.get(1..).unwrap_or_default();
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity_at(s: &str, html: bool) -> Option<(char, usize)> {
    if let Some(body) = s.strip_prefix("&#") {
        let (radix, digits, prefix_len) = match body.strip_prefix(['x', 'X']) {
            Some(hex) => (16u32, hex, 3usize),
            None => (10u32, body, 2usize),
        };
        let mut value = 0u32;
        let mut any_digit = false;
        for (i, c) in digits.char_indices() {
            match c.to_digit(radix) {
                Some(d) => {
                    any_digit = true;
                    value = value.checked_mul(radix)?.checked_add(d)?;
                    if value > 0x10_FFFF {
                        return None;
                    }
                }
                None if c == ';' && any_digit && value != 0 => {
                    return Some((char::from_u32(value)?, prefix_len + i + 1));
                }
                None => return None,
            }
        }
        return None;
    }
    let body = s.strip_prefix('&')?;
    let semi = body.bytes().take(9).position(|b| b == b';')?;
    let name = body.get(..semi).filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_alphanumeric()))?;
    let ch = match name {
        "lt" => '<',
        "gt" => '>',
        "amp" => '&',
        "quot" => '"',
        "apos" => '\'',
        _ if html => html_named_entity(name)?,
        _ => return None,
    };
    Some((ch, 1 + semi + 1))
}

fn html_named_entity(name: &str) -> Option<char> {
    if let Some(idx) = LATIN1_ENTITIES.iter().position(|&n| n == name) {
        return char::from_u32(0xA0 + idx as u32);
    }
    HTML_ENTITIES.iter().find(|&&(n, _)| n == name).map(|&(_, c)| c)
}

#[rustfmt::skip]
const LATIN1_ENTITIES: [&str; 96] = [
    "nbsp", "iexcl", "cent", "pound", "curren", "yen", "brvbar", "sect", "uml", "copy", "ordf", "laquo", "not", "shy", "reg", "macr",
    "deg", "plusmn", "sup2", "sup3", "acute", "micro", "para", "middot", "cedil", "sup1", "ordm", "raquo", "frac14", "frac12", "frac34", "iquest",
    "Agrave", "Aacute", "Acirc", "Atilde", "Auml", "Aring", "AElig", "Ccedil", "Egrave", "Eacute", "Ecirc", "Euml", "Igrave", "Iacute", "Icirc", "Iuml",
    "ETH", "Ntilde", "Ograve", "Oacute", "Ocirc", "Otilde", "Ouml", "times", "Oslash", "Ugrave", "Uacute", "Ucirc", "Uuml", "Yacute", "THORN", "szlig",
    "agrave", "aacute", "acirc", "atilde", "auml", "aring", "aelig", "ccedil", "egrave", "eacute", "ecirc", "euml", "igrave", "iacute", "icirc", "iuml",
    "eth", "ntilde", "ograve", "oacute", "ocirc", "otilde", "ouml", "divide", "oslash", "ugrave", "uacute", "ucirc", "uuml", "yacute", "thorn", "yuml",
];

#[rustfmt::skip]
const HTML_ENTITIES: &[(&str, char)] = &[
    ("OElig", '\u{0152}'), ("oelig", '\u{0153}'), ("Scaron", '\u{0160}'), ("scaron", '\u{0161}'), ("Yuml", '\u{0178}'), ("fnof", '\u{0192}'), ("circ", '\u{02C6}'), ("tilde", '\u{02DC}'),
    ("Alpha", '\u{0391}'), ("Beta", '\u{0392}'), ("Gamma", '\u{0393}'), ("Delta", '\u{0394}'), ("Epsilon", '\u{0395}'), ("Zeta", '\u{0396}'), ("Eta", '\u{0397}'), ("Theta", '\u{0398}'),
    ("Iota", '\u{0399}'), ("Kappa", '\u{039A}'), ("Lambda", '\u{039B}'), ("Mu", '\u{039C}'), ("Nu", '\u{039D}'), ("Xi", '\u{039E}'), ("Omicron", '\u{039F}'), ("Pi", '\u{03A0}'),
    ("Rho", '\u{03A1}'), ("Sigma", '\u{03A3}'), ("Tau", '\u{03A4}'), ("Upsilon", '\u{03A5}'), ("Phi", '\u{03A6}'), ("Chi", '\u{03A7}'), ("Psi", '\u{03A8}'), ("Omega", '\u{03A9}'),
    ("alpha", '\u{03B1}'), ("beta", '\u{03B2}'), ("gamma", '\u{03B3}'), ("delta", '\u{03B4}'), ("epsilon", '\u{03B5}'), ("zeta", '\u{03B6}'), ("eta", '\u{03B7}'), ("theta", '\u{03B8}'),
    ("iota", '\u{03B9}'), ("kappa", '\u{03BA}'), ("lambda", '\u{03BB}'), ("mu", '\u{03BC}'), ("nu", '\u{03BD}'), ("xi", '\u{03BE}'), ("omicron", '\u{03BF}'), ("pi", '\u{03C0}'),
    ("rho", '\u{03C1}'), ("sigmaf", '\u{03C2}'), ("sigma", '\u{03C3}'), ("tau", '\u{03C4}'), ("upsilon", '\u{03C5}'), ("phi", '\u{03C6}'), ("chi", '\u{03C7}'), ("psi", '\u{03C8}'),
    ("omega", '\u{03C9}'), ("thetasym", '\u{03D1}'), ("upsih", '\u{03D2}'), ("piv", '\u{03D6}'), ("ensp", '\u{2002}'), ("emsp", '\u{2003}'), ("thinsp", '\u{2009}'), ("zwnj", '\u{200C}'),
    ("zwj", '\u{200D}'), ("lrm", '\u{200E}'), ("rlm", '\u{200F}'), ("ndash", '\u{2013}'), ("mdash", '\u{2014}'), ("lsquo", '\u{2018}'), ("rsquo", '\u{2019}'), ("sbquo", '\u{201A}'),
    ("ldquo", '\u{201C}'), ("rdquo", '\u{201D}'), ("bdquo", '\u{201E}'), ("dagger", '\u{2020}'), ("Dagger", '\u{2021}'), ("bull", '\u{2022}'), ("hellip", '\u{2026}'), ("permil", '\u{2030}'),
    ("prime", '\u{2032}'), ("Prime", '\u{2033}'), ("lsaquo", '\u{2039}'), ("rsaquo", '\u{203A}'), ("oline", '\u{203E}'), ("frasl", '\u{2044}'), ("euro", '\u{20AC}'), ("image", '\u{2111}'),
    ("weierp", '\u{2118}'), ("real", '\u{211C}'), ("trade", '\u{2122}'), ("alefsym", '\u{2135}'), ("larr", '\u{2190}'), ("uarr", '\u{2191}'), ("rarr", '\u{2192}'), ("darr", '\u{2193}'),
    ("harr", '\u{2194}'), ("crarr", '\u{21B5}'), ("lArr", '\u{21D0}'), ("uArr", '\u{21D1}'), ("rArr", '\u{21D2}'), ("dArr", '\u{21D3}'), ("hArr", '\u{21D4}'), ("forall", '\u{2200}'),
    ("part", '\u{2202}'), ("exist", '\u{2203}'), ("empty", '\u{2205}'), ("nabla", '\u{2207}'), ("isin", '\u{2208}'), ("notin", '\u{2209}'), ("ni", '\u{220B}'), ("prod", '\u{220F}'),
    ("sum", '\u{2211}'), ("minus", '\u{2212}'), ("lowast", '\u{2217}'), ("radic", '\u{221A}'), ("prop", '\u{221D}'), ("infin", '\u{221E}'), ("ang", '\u{2220}'), ("and", '\u{2227}'),
    ("or", '\u{2228}'), ("cap", '\u{2229}'), ("cup", '\u{222A}'), ("int", '\u{222B}'), ("there4", '\u{2234}'), ("sim", '\u{223C}'), ("cong", '\u{2245}'), ("asymp", '\u{2248}'),
    ("ne", '\u{2260}'), ("equiv", '\u{2261}'), ("le", '\u{2264}'), ("ge", '\u{2265}'), ("sub", '\u{2282}'), ("sup", '\u{2283}'), ("nsub", '\u{2284}'), ("sube", '\u{2286}'),
    ("supe", '\u{2287}'), ("oplus", '\u{2295}'), ("otimes", '\u{2297}'), ("perp", '\u{22A5}'), ("sdot", '\u{22C5}'), ("lceil", '\u{2308}'), ("rceil", '\u{2309}'), ("lfloor", '\u{230A}'),
    ("rfloor", '\u{230B}'), ("lang", '\u{2329}'), ("rang", '\u{232A}'), ("loz", '\u{25CA}'), ("spades", '\u{2660}'), ("clubs", '\u{2663}'), ("hearts", '\u{2665}'), ("diams", '\u{2666}'),
];
