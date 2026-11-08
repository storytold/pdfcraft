//! Number display pictures (`<format><picture>num{$z,zzz,zz9.99}</picture>`) as Acrobat's
//! `AFNumber_Format` arguments, so a generated number field is drawn through its picture (#5820).
//!
//! Only a picture that `AFNumber_Format` shows as XFA 3.3's picture clauses define it is
//! translated: suppressed digits (`z`) then one digit (`9`) before the radix, digits (`9`) after
//! it, a `,` between each group of three, a `$` before or after the number, parentheses around it,
//! and a second alternative that is the first in parentheses (`$z,zz9.99|($z,zz9.99)`) or after a
//! minus sign. Anything else (zero-filled or space-filled integer digits, optional fraction digits,
//! signs, exponents, `CR`/`DB`, `%`, literal text, a locale whose symbols aren't known, or whose
//! separators Acrobat has no style for) gives `None`, and the field shows its raw value as before.

use crate::model::Locale;
use crate::picture::alternatives;

/// The most digits a translated picture may have on either side of the radix.
const MAX_DIGITS: usize = 30;
/// The most decimals a translated picture may have (`AFNumber_Format` pads to them).
const MAX_DECIMALS: usize = 15;
/// The longest currency text kept, in characters.
const MAX_CURRENCY: usize = 8;

/// The arguments of `AFNumber_Format(decimals, sep, neg, 0, currency, prepend)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NumberFormat {
    /// Digits after the radix.
    pub decimals: u8,
    /// Acrobat's separator style: 0 `1,234.56`, 1 `1234.56`, 2 `1.234,56`, 3 `1234,56`,
    /// 4 `1'234.56`.
    pub sep: u8,
    /// Acrobat's negative style: 0 a minus sign, 2 parentheses.
    pub neg: u8,
    /// The currency text, with the space the picture puts between it and the number; empty for
    /// none.
    pub currency: String,
    /// The currency comes before the number.
    pub prepend: bool,
}

/// The symbols a picture's `.`, `,` and `$` show in a locale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Symbols<'a> {
    decimal: Option<&'a str>,
    grouping: Option<&'a str>,
    currency: Option<&'a str>,
}

/// en_US's symbols, for an en_US the form's `localeSet` doesn't describe: the locale Designer
/// gives new forms.
const EN_US: Symbols<'static> = Symbols { decimal: Some("."), grouping: Some(","), currency: Some("$") };

/// The locale `name` in `locales`. Unnamed or `ambient` (the viewer's own locale) is en_US, as
/// the packet describes it when it does. `None` when the form names a locale it doesn't
/// describe, other than en_US.
fn locale_of<'a>(locales: &'a [Locale], name: Option<&str>) -> Option<(Symbols<'a>, Option<&'a Locale>)> {
    let name = name.filter(|n| !n.eq_ignore_ascii_case("ambient")).unwrap_or("en_US");
    match locales.iter().find(|l| l.name == name) {
        Some(l) => Some((Symbols { decimal: l.decimal.as_deref(), grouping: l.grouping.as_deref(), currency: l.currency.as_deref() }, Some(l))),
        None if name == "en_US" => Some((EN_US, None)),
        None => None,
    }
}

/// A picture clause split into its category (`num`, `num.currency`; `None` for a bare pattern),
/// the locale it names (`num(de_DE){…}`) and its pattern (between the braces).
fn clause(text: &str) -> Option<(Option<String>, Option<&str>, &str)> {
    let text = text.trim();
    let Some(open) = text.find('{') else { return Some((None, None, text)) };
    let head = text.get(..open)?;
    let body = text.get(open + 1..)?.strip_suffix('}')?;
    // The locale is in parentheses after the category, before or after a subcategory
    // (`num(de_DE).currency`).
    let (category, locale) = match (head.find('('), head.find(')')) {
        (Some(a), Some(b)) if a < b => {
            let category = format!("{}{}", head.get(..a)?, head.get(b + 1..)?);
            (category, Some(head.get(a + 1..b)?.trim()).filter(|l| !l.is_empty()))
        }
        (None, None) => (head.to_string(), None),
        _ => return None,
    };
    Some((Some(category.trim().to_ascii_lowercase()), locale, body))
}

/// `pattern`'s parts between `|`s (a locale pattern lists the negative form second).
fn bars(pattern: &str) -> impl Iterator<Item = &str> {
    pattern.split('|').map(str::trim)
}

/// The `AFNumber_Format` arguments that show values as display picture `picture` does, for a
/// field in locale `locale` (its own or an enclosing object's; `None` when unnamed) of a form
/// whose `localeSet` describes `locales`. `None` when the picture isn't one this can express.
pub(crate) fn number_format(picture: &str, locale: Option<&str>, locales: &[Locale]) -> Option<NumberFormat> {
    // (pattern, its locale's symbols), the positive form first.
    let mut forms: Vec<(String, Symbols)> = Vec::new();
    for alternative in alternatives(picture) {
        let (category, named, body) = clause(alternative)?;
        let (symbols, described) = locale_of(locales, named.or(locale))?;
        let pattern = match category.as_deref() {
            None | Some("num") => body.to_string(),
            // A predefined clause is the locale's own pattern; only the packet gives those.
            Some(predefined) if body.trim().is_empty() => {
                let name = match predefined {
                    "num.decimal" => "numeric",
                    "num.currency" => "currency",
                    _ => return None,
                };
                described?.number_pattern(name)?.to_string()
            }
            _ => return None,
        };
        for part in bars(&pattern) {
            forms.push((part.to_string(), symbols));
            if forms.len() > 2 {
                return None;
            }
        }
    }
    let ((positive, symbols), negative) = match forms.as_slice() {
        [first] => (first, None),
        [first, second] => (first, Some(second)),
        _ => return None,
    };
    let mut format = translate(positive, symbols)?;
    if let Some((negative, negative_symbols)) = negative {
        // Another locale's symbols for negative values can't be shown by one format.
        if negative_symbols != symbols || format.neg != 0 {
            return None;
        }
        if *negative == format!("({positive})") {
            format.neg = 2;
        } else if *negative != format!("-{positive}") {
            return None;
        }
    }
    Some(format)
}

/// One pattern (no `|`) in `symbols`, or `None` when `AFNumber_Format` can't show it.
fn translate(pattern: &str, symbols: &Symbols) -> Option<NumberFormat> {
    let mut rest = pattern;
    let mut neg = 0;
    // `(` and `)` show parentheses around a negative value (and spaces around a positive one,
    // which Acrobat leaves out).
    if let Some(inner) = rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        rest = inner;
        neg = 2;
    }
    let (mut currency, mut prepend) = (String::new(), true);
    if let Some(after) = rest.strip_prefix('$') {
        let (after, space) = after.strip_prefix(' ').map_or((after, ""), |a| (a, " "));
        currency = format!("{}{space}", currency_text(symbols.currency?)?);
        rest = after;
    } else if let Some(before) = rest.strip_suffix('$') {
        let (before, space) = before.strip_suffix(' ').map_or((before, ""), |b| (b, " "));
        currency = format!("{space}{}", currency_text(symbols.currency?)?);
        prepend = false;
        rest = before;
    }
    let (int, frac) = match rest.split_once('.') {
        Some((int, frac)) => (int, Some(frac)),
        None => (rest, None),
    };
    // Suppressed digits, then the one digit that shows 0: Acrobat shows no leading zeros.
    let digits: Vec<char> = int.chars().filter(|c| *c != ',').collect();
    let (last, leading) = digits.split_last()?;
    if *last != '9' || !leading.iter().all(|c| *c == 'z') || digits.len() > MAX_DIGITS {
        return None;
    }
    // Groups of three, as Acrobat groups digits: `z,zz,zz9` is not one.
    let grouped = int.contains(',');
    if grouped {
        let mut groups = int.split(',');
        let first = groups.next()?;
        if first.is_empty() || first.len() > 3 || !groups.all(|g| g.len() == 3) {
            return None;
        }
    }
    let decimals = match frac {
        None => 0,
        // Every decimal shown: Acrobat pads to them, so optional ones (`z`) can't be.
        Some(f) if !f.is_empty() && f.len() <= MAX_DECIMALS && f.chars().all(|c| c == '9') => u8::try_from(f.len()).ok()?,
        Some(_) => return None,
    };
    let sep = match (grouped, symbols.decimal?, symbols.grouping) {
        (true, ".", Some(",")) => 0,
        (true, ",", Some(".")) => 2,
        (true, ".", Some("'")) => 4,
        (false, ".", _) => 1,
        (false, ",", _) => 3,
        _ => return None,
    };
    Some(NumberFormat { decimals, sep, neg, currency, prepend })
}

/// A currency symbol as the format's text: one with a control character, or longer than any
/// currency's, is `None`. (The action quotes it as a JavaScript string, whatever it holds.)
fn currency_text(symbol: &str) -> Option<&str> {
    let plain = !symbol.is_empty() && symbol.chars().count() <= MAX_CURRENCY && !symbol.chars().any(char::is_control);
    plain.then_some(symbol)
}
