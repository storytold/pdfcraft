fn roman(mut n: u32, upper: bool) -> String {
    let table = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut s = String::new();
    if n == 0 || n > 3999 {
        return n.to_string();
    }
    for (v, r) in table {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    if upper { s.to_uppercase() } else { s }
}

fn alpha_num(n: u32, upper: bool) -> String {
    if n == 0 {
        return String::new();
    }
    let n = n - 1;
    let c = (b'a' + (n % 26) as u8) as char;
    let reps = (n / 26 + 1).min(50);
    let s: String = std::iter::repeat_n(c, reps as usize).collect();
    if upper { s.to_uppercase() } else { s }
}

/// Letters of a numbering alphabet: 1 → the first, past the end the letter doubles ("aa"), as
/// the Latin alphabetic schemes do.
fn letters(list: &[char], n: u32) -> String {
    let len = list.len().max(1) as u32;
    let Some(i) = n.checked_sub(1) else { return String::new() };
    let c = list.get((i % len) as usize).copied().unwrap_or('?');
    std::iter::repeat_n(c, (i / len + 1).min(50) as usize).collect()
}

/// `n` written with the digits `zero..zero+9` (Thai, Devanagari, full-width digits).
fn digits_in(n: u32, zero: char) -> String {
    n.to_string().chars().map(|d| d.to_digit(10).and_then(|v| char::from_u32(zero as u32 + v)).unwrap_or(d)).collect()
}

const CJK_DIGITS: [char; 10] = ['〇', '一', '二', '三', '四', '五', '六', '七', '八', '九'];

/// Chinese numerals: 10 → 十, 11 → 十一, 25 → 二十五, 105 → 一百零五.
pub(crate) fn chinese_num(n: u32) -> String {
    if n == 0 || n > 99_999 {
        return n.to_string();
    }
    let units = ['\0', '十', '百', '千', '万'];
    let ds: Vec<u32> = n.to_string().chars().filter_map(|c| c.to_digit(10)).collect();
    let mut out = String::new();
    let mut zero = false;
    for (i, d) in ds.iter().enumerate() {
        let place = ds.len() - 1 - i;
        if *d == 0 {
            zero = true;
            continue;
        }
        if zero {
            out.push('零');
            zero = false;
        }
        // "十" alone for 10–19, not "一十".
        if !(*d == 1 && place == 1 && i == 0) {
            out.push(CJK_DIGITS.get(*d as usize).copied().unwrap_or('?'));
        }
        if let Some(u) = units.get(place).filter(|u| **u != '\0') {
            out.push(*u);
        }
    }
    out
}

/// Text of an auto-number bullet: `arabicPeriod` 3 → "3.". The East Asian, Thai, Hindi, Arabic
/// and Hebrew schemes are written as PowerPoint draws them: `ea1ChsPeriod` 10 → "十．",
/// `ea1JpnKorPeriod` 10 → "一〇．" (digit by digit), `arabicDbPeriod` 13 → "１３．",
/// `thaiAlphaParenR` 4 → "ง)", `hindiAlpha1Period` 1 → "क.", `arabic2Minus` 10 → "ي-"
/// (abjad order), `circleNumWdWhitePlain` 13 → "③" (those circles go to ten).
pub fn autonum(scheme: &str, n: u32) -> String {
    const THAI: [char; 41] = [
        'ก', 'ข', 'ค', 'ง', 'จ', 'ฉ', 'ช', 'ซ', 'ฌ', 'ญ', 'ฎ', 'ฏ', 'ฐ', 'ฑ', 'ฒ', 'ณ', 'ด', 'ต', 'ถ', 'ท', 'ธ', 'น', 'บ', 'ป', 'ผ', 'ฝ', 'พ', 'ฟ',
        'ภ', 'ม', 'ย', 'ร', 'ล', 'ว', 'ศ', 'ษ', 'ส', 'ห', 'ฬ', 'อ', 'ฮ',
    ];
    const HINDI_VOWELS: [char; 11] = ['अ', 'आ', 'इ', 'ई', 'उ', 'ऊ', 'ऋ', 'ए', 'ऐ', 'ओ', 'औ'];
    const HINDI_CONSONANTS: [char; 33] = [
        'क', 'ख', 'ग', 'घ', 'ङ', 'च', 'छ', 'ज', 'झ', 'ञ', 'ट', 'ठ', 'ड', 'ढ', 'ण', 'त', 'थ', 'द', 'ध', 'न', 'प', 'फ', 'ब', 'भ', 'म', 'य', 'र', 'ल',
        'व', 'श', 'ष', 'स', 'ह',
    ];
    const ARABIC_ALPHABET: [char; 28] =
        ['ا', 'ب', 'ت', 'ث', 'ج', 'ح', 'خ', 'د', 'ذ', 'ر', 'ز', 'س', 'ش', 'ص', 'ض', 'ط', 'ظ', 'ع', 'غ', 'ف', 'ق', 'ك', 'ل', 'م', 'ن', 'ه', 'و', 'ي'];
    const ARABIC_ABJAD: [char; 28] =
        ['ا', 'ب', 'ج', 'د', 'ه', 'و', 'ز', 'ح', 'ط', 'ي', 'ك', 'ل', 'م', 'ن', 'س', 'ع', 'ف', 'ص', 'ق', 'ر', 'ش', 'ت', 'ث', 'خ', 'ذ', 'ض', 'ظ', 'غ'];
    const HEBREW: [char; 22] = ['א', 'ב', 'ג', 'ד', 'ה', 'ו', 'ז', 'ח', 'ט', 'י', 'כ', 'ל', 'מ', 'נ', 'ס', 'ע', 'פ', 'צ', 'ק', 'ר', 'ש', 'ת'];
    let circled = |first: u32, count: u32| char::from_u32(first + n.saturating_sub(1) % count).map(String::from).unwrap_or_else(|| n.to_string());
    let core = if scheme.starts_with("romanUc") {
        roman(n, true)
    } else if scheme.starts_with("romanLc") {
        roman(n, false)
    } else if scheme.starts_with("alphaUc") {
        alpha_num(n, true)
    } else if scheme.starts_with("alphaLc") {
        alpha_num(n, false)
    } else if scheme.starts_with("circleNumWdWhite") {
        return circled(0x2460, 10);
    } else if scheme.starts_with("circleNumWdBlack") {
        return circled(0x2776, 10);
    } else if scheme.starts_with("circleNum") {
        return char::from_u32(0x2460 + n.saturating_sub(1).min(19)).map(String::from).unwrap_or_else(|| n.to_string());
    } else if scheme.starts_with("ea1Chs") || scheme.starts_with("ea1Cht") {
        chinese_num(n)
    } else if scheme.starts_with("ea1Jpn") {
        n.to_string().chars().map(|d| d.to_digit(10).and_then(|v| CJK_DIGITS.get(v as usize).copied()).unwrap_or(d)).collect()
    } else if scheme.starts_with("arabicDb") {
        digits_in(n, '０')
    } else if scheme.starts_with("thaiAlpha") {
        letters(&THAI, n)
    } else if scheme.starts_with("thaiNum") {
        digits_in(n, '๐')
    } else if scheme.starts_with("hindiAlpha1") {
        letters(&HINDI_CONSONANTS, n)
    } else if scheme.starts_with("hindiAlpha") {
        letters(&HINDI_VOWELS, n)
    } else if scheme.starts_with("hindiNum") {
        digits_in(n, '०')
    } else if scheme.starts_with("arabic1") {
        letters(&ARABIC_ALPHABET, n)
    } else if scheme.starts_with("arabic2") {
        letters(&ARABIC_ABJAD, n)
    } else if scheme.starts_with("hebrew2") {
        letters(&HEBREW, n)
    } else {
        n.to_string()
    };
    // East Asian schemes end with a full-width full stop.
    let wide = scheme.starts_with("ea1") || scheme.starts_with("arabicDb");
    if scheme.ends_with("ParenBoth") {
        format!("({core})")
    } else if scheme.ends_with("ParenR") {
        format!("{core})")
    } else if scheme.ends_with("Period") {
        format!("{core}{}", if wide { '．' } else { '.' })
    } else if scheme.ends_with("Minus") {
        format!("{core}-")
    } else {
        core
    }
}
