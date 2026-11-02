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
