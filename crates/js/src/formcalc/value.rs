// ── values ──────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Val {
    Null,
    Num(f64),
    Str(String),
    /// A form object, by index in the host.
    Node(usize),
    /// Several objects or values (`Row[*]`, `Row[*].Amount`).
    List(Vec<Val>),
    Host,
    Layout,
    Event,
    /// `xfa` itself (`xfa.form`, `xfa.host`, …).
    Xfa,
    /// The instance manager of `name` under `parent`.
    Im(usize, String),
    /// A property object whose contents are not modelled (`$.border.fill…`): reads give
    /// another sink, writes change nothing.
    Sink,
}

/// A number as FormCalc prints it: integers without a fraction, otherwise up to 15 digits.
pub(super) fn num_text(n: f64) -> String {
    if !n.is_finite() {
        return if n.is_nan() {
            "NaN".into()
        } else if n > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if n.fract() == 0.0 && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    let s = format!("{n:.12}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_string() }
}

/// `v` with `decimals` places, rounding exact halves away from zero (Rust's formatter rounds them to even).
pub(super) fn fixed(v: f64, decimals: usize) -> String {
    let k = 10f64.powi(decimals.min(20) as i32);
    let scaled = (v * k).round() / k;
    // A small negative value rounds to -0.0, which would print as "-0".
    let scaled = if scaled == 0.0 { 0.0 } else { scaled };
    format!("{:.decimals$}", if scaled.is_finite() { scaled } else { v })
}

/// The number a string holds: its leading numeric part (`12abc` → 12), `None` for none.
pub(super) fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let mut end = 0;
    let b = t.as_bytes();
    let mut seen_digit = false;
    let mut seen_dot = false;
    let mut seen_e = false;
    let mut i = 0;
    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
        i += 1;
    }
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() {
            seen_digit = true;
            end = i + 1;
        } else if c == b'.' && !seen_dot && !seen_e {
            seen_dot = true;
        } else if (c == b'e' || c == b'E') && seen_digit && !seen_e {
            seen_e = true;
            if i + 1 < b.len() && (b[i + 1] == b'-' || b[i + 1] == b'+') {
                i += 1;
            }
        } else {
            break;
        }
        i += 1;
    }
    if !seen_digit {
        return None;
    }
    t.get(..end).and_then(|p| p.parse::<f64>().ok()).filter(|v| v.is_finite())
}

pub(super) fn is_numeric_text(s: &str) -> bool {
    s.trim().parse::<f64>().is_ok_and(f64::is_finite)
}
