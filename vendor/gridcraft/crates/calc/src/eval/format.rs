use super::{CellError, Value};
use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS};

/// CELL("format") code for a number format.
pub(super) fn cell_format_code(code: &str) -> String {
    let c = code.to_ascii_lowercase();
    if c == "general" {
        return "G".into();
    }
    if c.contains('%') {
        return format!("P{}", decimals(&c));
    }
    if c.contains('e') && c.contains('0') {
        return format!("S{}", decimals(&c));
    }
    if c.contains('$') {
        return format!("C{}", decimals(&c));
    }
    if c.contains('y') || c.contains('d') {
        return "D1".into();
    }
    if c.contains('h') {
        return "D9".into();
    }
    if c.contains(',') {
        return format!(",{}", decimals(&c));
    }
    format!("F{}", decimals(&c))
}

fn decimals(c: &str) -> usize {
    c.split_once('.').map_or(0, |(_, d)| d.chars().take_while(|ch| *ch == '0').count())
}

pub(super) fn same_kind(a: &Value, b: &Value) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b) || (a.is_empty() || b.is_empty())
}

/// Converts an R1C1 reference text to A1 (for INDIRECT(…, FALSE)). `Ok(None)` means the text is
/// not R1C1; `Err` means it is R1C1 but points past the sheet.
pub fn r1c1_to_a1(s: &str, at: CellRef) -> Result<Option<String>, CellError> {
    let (sheet, body) = match s.rsplit_once('!') {
        Some((sh, b)) => (format!("{sh}!"), b),
        None => (String::new(), s),
    };
    let conv = |part: &str| -> Result<Option<String>, CellError> {
        let u = part.to_ascii_uppercase();
        let Some((rpart, cpart)) = u.strip_prefix('R').and_then(|rest| rest.split_once('C')) else { return Ok(None) };
        // `None`: not R1C1. `Some(None)`: R1C1, but past the sheet.
        let num = |p: &str, base: u32, max: u32| -> Option<Option<(u32, bool)>> {
            let (v, absolute) = if p.is_empty() {
                (Some(i64::from(base)), false)
            } else if let Some(inner) = p.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
                (i64::from(base).checked_add(inner.parse().ok()?), false)
            } else {
                (p.parse::<u32>().ok()?.checked_sub(1).map(i64::from), true)
            };
            Some(v.filter(|v| (0..i64::from(max)).contains(v)).map(|v| (v as u32, absolute)))
        };
        let (Some(r), Some(c)) = (num(rpart, at.row, MAX_ROWS), num(cpart, at.col, MAX_COLS)) else { return Ok(None) };
        let (Some((r, ra)), Some((c, ca))) = (r, c) else { return Err(CellError::Ref) };
        Ok(Some(format!("{}{}{}{}", if ca { "$" } else { "" }, gridcraft_core::col_to_letters(c), if ra { "$" } else { "" }, r + 1)))
    };
    let out = match body.split_once(':') {
        Some((a, b)) => match (conv(a)?, conv(b)?) {
            (Some(a), Some(b)) => format!("{a}:{b}"),
            _ => return Ok(None),
        },
        None => match conv(body)? {
            Some(x) => x,
            None => return Ok(None),
        },
    };
    Ok(Some(format!("{sheet}{out}")))
}
