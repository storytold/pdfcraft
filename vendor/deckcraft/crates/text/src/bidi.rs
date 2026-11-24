//! Bidirectional text (UAX #9) for paragraph layout.
//!
//! The pipeline is the one the algorithm prescribes: embedding levels are resolved once for the
//! whole logical paragraph (base level from `a:pPr/@rtl`); shaping and line breaking happen in
//! logical order; then each line is reordered on its own ([`line_levels`] applies rule L1,
//! [`visual_order`] rule L2). Rule L4 (mirrored brackets) is applied by the shaper, which draws
//! mirrorable characters in right-to-left items with their mirrored glyphs.

use unicode_bidi::BidiClass::{self, AL, AN, B, BN, CS, EN, ES, ET, FSI, L, LRE, LRI, LRO, NSM, ON, PDF, PDI, R, RLE, RLI, RLO, S, WS};
use unicode_bidi::{BidiInfo, Level, bidi_class};

/// Does anything in the text need the full algorithm? (Strong right-to-left, Arabic numbers or
/// explicit embeddings.)
fn needs_bidi(chars: &[char]) -> bool {
    chars.iter().any(|c| matches!(bidi_class(*c), R | AL | AN | LRE | RLE | LRO | RLO | PDF | LRI | RLI | FSI | PDI))
}

/// Resolved embedding level of every character of a paragraph (before the line rules), with the
/// paragraph level forced to 1 when `rtl`.
pub(crate) fn levels(text: &str, chars: &[char], rtl: bool) -> Vec<u8> {
    let base = u8::from(rtl);
    if !rtl && !needs_bidi(chars) {
        return vec![0; chars.len()];
    }
    let info = BidiInfo::new(text, Some(if rtl { Level::rtl() } else { Level::ltr() }));
    // `levels` is per byte; take each character's first byte.
    text.char_indices().map(|(b, _)| info.levels.get(b).map(|l| l.number()).unwrap_or(base)).collect()
}

/// Is `lang` (BCP 47) a language written right to left? Its runs resolve neutral characters and
/// digits the right-to-left way (see [`levels_lang`]).
pub(crate) fn is_rtl_lang(lang: &str) -> bool {
    let primary = lang.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    matches!(primary.as_str(), "ar" | "he" | "iw" | "fa" | "ur" | "yi" | "ji" | "ps" | "sd" | "ug" | "dv" | "syr" | "ckb" | "arc")
}

/// How a character resolves before levels are assigned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dir {
    L,
    R,
    /// A number inside right-to-left text: kept left to right, placed with the right-to-left text.
    Num,
    /// Not resolved yet (neutral).
    Open,
}

/// Embedding levels as PowerPoint resolves them: the language of each character's run
/// (`rtl_lang`, from `a:rPr/@lang`) decides how neutral characters and digits go, where UAX #9
/// would look at the surrounding text only. Observed on desktop PowerPoint:
/// - in a run of a left-to-right language (or none), digits and punctuation are left to right,
///   and a space takes the direction of the character before it (so the space after a Hebrew word
///   travels with that word);
/// - in a run of a right-to-left language (Hebrew, Arabic…), punctuation and spaces between text
///   of one direction take it, everywhere else they are right to left; digits are a number in
///   right-to-left text unless left-to-right text precedes them.
///
/// Paragraphs with explicit embeddings or isolates keep the plain algorithm ([`levels`]).
pub(crate) fn levels_lang(text: &str, chars: &[char], rtl: bool, rtl_lang: &[bool]) -> Vec<u8> {
    let base = u8::from(rtl);
    let any_rtl_lang = rtl_lang.iter().any(|r| *r);
    if !rtl && !any_rtl_lang && !needs_bidi(chars) {
        return vec![0; chars.len()];
    }
    let classes: Vec<BidiClass> = chars.iter().map(|c| bidi_class(*c)).collect();
    if classes.iter().any(|c| matches!(c, LRE | RLE | LRO | RLO | PDF | LRI | RLI | FSI | PDI)) {
        return levels(text, chars, rtl);
    }
    let cls = resolve_nsm(classes, rtl);
    let mut dir = initial_dirs(&cls, rtl_lang);
    join_numbers_in_rtl_runs(&mut dir, &cls, rtl_lang);
    // Punctuation in left-to-right runs is left to right.
    for (i, d) in dir.iter_mut().enumerate() {
        if !in_rtl_lang(rtl_lang, i) && *d == Dir::Open && !matches!(cls.get(i), Some(WS | S)) {
            *d = Dir::L;
        }
    }
    resolve_rtl_neutrals(&mut dir, rtl_lang, if rtl { Dir::R } else { Dir::L });
    // Spaces in left-to-right runs follow the character before them.
    let mut prev = Dir::L;
    for d in dir.iter_mut() {
        if *d == Dir::Open {
            *d = prev;
        }
        prev = if *d == Dir::Num { Dir::R } else { *d };
    }
    dir.iter()
        .map(|d| match (d, base % 2 == 1) {
            (Dir::L, false) | (Dir::R, true) => base,
            (Dir::L, true) | (Dir::R, false) | (Dir::Open, _) => base + 1,
            (Dir::Num, false) => base + 2,
            (Dir::Num, true) => base + 1,
        })
        .collect()
}

fn in_rtl_lang(rtl_lang: &[bool], i: usize) -> bool {
    rtl_lang.get(i).copied().unwrap_or(false)
}

/// W1: combining marks take the class of the character they sit on.
fn resolve_nsm(mut cls: Vec<BidiClass>, rtl: bool) -> Vec<BidiClass> {
    for i in 0..cls.len() {
        if cls.get(i) == Some(&NSM) {
            let prev = if i == 0 { if rtl { R } else { L } } else { cls.get(i - 1).copied().unwrap_or(L) };
            if let Some(c) = cls.get_mut(i) {
                *c = prev;
            }
        }
    }
    cls
}

/// Strong letters and digits get a direction; everything else stays open.
fn initial_dirs(cls: &[BidiClass], rtl_lang: &[bool]) -> Vec<Dir> {
    let mut last_strong: Option<BidiClass> = None;
    cls.iter()
        .enumerate()
        .map(|(i, &c)| {
            let d = match c {
                L => Dir::L,
                R | AL => Dir::R,
                AN => Dir::Num,
                EN if !in_rtl_lang(rtl_lang, i) => Dir::L,
                // W7: European digits after left-to-right text are left to right.
                EN if last_strong == Some(L) => Dir::L,
                EN => Dir::Num,
                _ => Dir::Open,
            };
            if matches!(c, L | R | AL) {
                last_strong = Some(c);
            }
            d
        })
        .collect()
}

/// W4/W5 in right-to-left runs: a separator between digits, and currency or percent signs next
/// to them, belong to the number. `after_et[i]` is the first index after `i` that is not ET,
/// computed once so a long run of currency signs costs linear work, not a scan per sign.
fn join_numbers_in_rtl_runs(dir: &mut [Dir], cls: &[BidiClass], rtl_lang: &[bool]) {
    let after_et = next_non_et(cls);
    for i in 0..dir.len() {
        if !in_rtl_lang(rtl_lang, i) || dir.get(i) != Some(&Dir::Open) {
            continue;
        }
        let c = cls.get(i).copied().unwrap_or(ON);
        let num = |j: Option<usize>| j.and_then(|j| dir.get(j)).is_some_and(|d| *d == Dir::Num);
        let joined = match c {
            ES | CS => num(i.checked_sub(1)) && num(Some(i + 1)),
            ET => num(i.checked_sub(1)) || num(after_et.get(i).copied().flatten()),
            _ => false,
        };
        if joined && let Some(x) = dir.get_mut(i) {
            *x = Dir::Num;
        }
    }
}

fn strong_at(dir: &[Dir], j: usize) -> Option<Dir> {
    match dir.get(j) {
        Some(Dir::L) => Some(Dir::L),
        Some(Dir::R | Dir::Num) => Some(Dir::R),
        _ => None,
    }
}

/// Neutrals in right-to-left runs: the direction around them when both sides agree, else right to
/// left. Unresolved spaces of left-to-right runs are looked through.
///
/// Resolving a span only changes characters before the scan position, so the nearest strong
/// direction after a span can be computed once up front, and the one before it is carried along:
/// linear work however many neutral spans the paragraph has.
fn resolve_rtl_neutrals(dir: &mut [Dir], rtl_lang: &[bool], sos: Dir) {
    let n = dir.len();
    let rl = |dir: &[Dir], i: usize| in_rtl_lang(rtl_lang, i) && dir.get(i) == Some(&Dir::Open);
    let mut next_strong: Vec<Option<Dir>> = vec![None; n + 1];
    for j in (0..n).rev() {
        let here = strong_at(dir, j).or(next_strong.get(j + 1).copied().flatten());
        if let Some(x) = next_strong.get_mut(j) {
            *x = here;
        }
    }
    let mut last_strong: Option<Dir> = None;
    let mut i = 0;
    while i < n {
        if !rl(dir, i) {
            last_strong = strong_at(dir, i).or(last_strong);
            i += 1;
            continue;
        }
        let start = i;
        while i < n && rl(dir, i) {
            i += 1;
        }
        let before = last_strong.unwrap_or(sos);
        let after = next_strong.get(i).copied().flatten().unwrap_or(sos);
        let d = if before == after { before } else { Dir::R };
        for x in dir.get_mut(start..i).into_iter().flatten() {
            *x = d;
        }
        last_strong = Some(if d == Dir::Num { Dir::R } else { d });
    }
}

/// For each index, the first later index whose class is not ET (`None` when only ET follows).
fn next_non_et(cls: &[BidiClass]) -> Vec<Option<usize>> {
    let mut out = vec![None; cls.len()];
    let mut next = None;
    for i in (0..cls.len()).rev() {
        if let Some(x) = out.get_mut(i) {
            *x = next;
        }
        if cls.get(i) != Some(&ET) {
            next = Some(i);
        }
    }
    out
}

/// Whitespace-like for rule L1: reset to the paragraph level when trailing a line or preceding a
/// segment/paragraph separator.
fn l1_whitespace(c: BidiClass) -> bool {
    matches!(c, WS | FSI | LRI | RLI | PDI | BN | LRE | RLE | LRO | RLO | PDF)
}

/// Rule L1 for one line: segment and paragraph separators, any whitespace run before them, and
/// whitespace at the end of the line take the paragraph level.
pub(crate) fn line_levels(chars: &[char], levels: &[u8], para_level: u8) -> Vec<u8> {
    let mut out: Vec<u8> = levels.to_vec();
    out.resize(chars.len(), para_level);
    // Walk backwards: `trailing` is true while every character after this one (up to the line end
    // or a separator) is whitespace.
    let mut trailing = true;
    for (i, c) in chars.iter().enumerate().rev() {
        let class = bidi_class(*c);
        if matches!(class, S | B) {
            if let Some(l) = out.get_mut(i) {
                *l = para_level;
            }
            trailing = true;
        } else if l1_whitespace(class) {
            if trailing && let Some(l) = out.get_mut(i) {
                *l = para_level;
            }
        } else {
            trailing = false;
        }
    }
    out
}

/// Rule L2: the visual order of a line's characters (indices into `levels`, left to right). From
/// the highest level down to the lowest odd level, every maximal run at that level or higher is
/// reversed.
pub(crate) fn visual_order(levels: &[u8]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    let Some(&max) = levels.iter().max() else { return order };
    let min_odd = levels.iter().copied().filter(|l| l % 2 == 1).min().unwrap_or(max.saturating_add(1));
    let mut lvl = max;
    while lvl >= min_odd && lvl > 0 {
        let mut i = 0;
        while i < order.len() {
            let at = |k: usize| order.get(k).and_then(|&j| levels.get(j)).copied().unwrap_or(0);
            if at(i) >= lvl {
                let start = i;
                while i < order.len() && at(i) >= lvl {
                    i += 1;
                }
                if let Some(run) = order.get_mut(start..i) {
                    run.reverse();
                }
            } else {
                i += 1;
            }
        }
        lvl -= 1;
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lv(s: &str, rtl: bool) -> Vec<u8> {
        let chars: Vec<char> = s.chars().collect();
        levels(s, &chars, rtl)
    }

    #[test]
    fn mixed_arabic_latin_numbers_and_brackets() {
        let s = "سنة 2024 (Retrait) فقط";
        let l = lv(s, true);
        let chars: Vec<char> = s.chars().collect();
        assert_eq!(l.len(), chars.len());
        // Arabic letters at the paragraph level 1; European digits at 2; Latin at 2.
        assert_eq!(l[0], 1);
        assert_eq!(&l[4..8], &[2, 2, 2, 2]);
        assert_eq!(&l[10..17], &[2; 7]);
        assert_eq!(l[19], 1);
        // Brackets around Latin inside an RTL paragraph resolve to the embedding direction (N0).
        assert_eq!(l[9] % 2, 1, "( is right-to-left");
        assert_eq!(l[17] % 2, 1, ") is right-to-left");
        // Visual order, left to right: the Arabic words reversed, Latin and digits kept, the
        // bracket characters swapped (their glyphs are mirrored back by the shaper, rule L4).
        let order = visual_order(&line_levels(&chars, &l, 1));
        let vis: String = order.iter().map(|&i| chars[i]).collect();
        assert_eq!(vis, "طقف )Retrait( 2024 ةنس");
    }

    #[test]
    fn ltr_paragraph_fast_path_and_embedded_rtl() {
        assert_eq!(lv("Hello 123", false), vec![0; 9]);
        let s = "ab سن cd";
        let chars: Vec<char> = s.chars().collect();
        let l = lv(s, false);
        assert_eq!(l, vec![0, 0, 0, 1, 1, 0, 0, 0]);
        let vis: String = visual_order(&l).iter().map(|&i| chars[i]).collect();
        assert_eq!(vis, "ab نس cd");
        // An RTL paragraph of Latin text: the text stays left to right inside it.
        assert_eq!(lv("ab", true), vec![2, 2]);
    }

    /// Visual string of `s` laid out on one line with every character in a run of language
    /// direction `rtl_lang`, as PowerPoint orders it.
    fn vis_lang(s: &str, rtl: bool, rtl_lang: bool) -> String {
        let chars: Vec<char> = s.chars().collect();
        let l = levels_lang(s, &chars, rtl, &vec![rtl_lang; chars.len()]);
        visual_order(&line_levels(&chars, &l, u8::from(rtl))).iter().map(|&i| chars[i]).collect()
    }

    #[test]
    fn run_language_decides_neutrals_like_powerpoint() {
        // Observed in PowerPoint with each space highlighted. An English run: the space after a
        // Hebrew word goes with it, punctuation is left to right.
        assert_eq!(vis_lang("x אב גד y", false, false), "x  דג באy");
        assert_eq!(vis_lang("אב גד x y", false, false), " דג באx y");
        assert_eq!(vis_lang("x אב, גד! y", false, false), "x בא, דג! y");
        // Digits after Hebrew stay left to right in an English run.
        assert_eq!(vis_lang("אב 12", false, false), " בא12");
        // A Hebrew run: neutrals between different directions are right to left, and digits are
        // a number inside the right-to-left text.
        assert_eq!(vis_lang("אב גד x y", false, true), " דג באx y");
        assert_eq!(vis_lang("12 אב 34 x", false, true), " 34 בא 12x");
        assert_eq!(vis_lang("x אב, גד! y", false, true), "x !דג ,בא y");
        // Between two left-to-right words a Hebrew run's space stays left to right.
        assert_eq!(vis_lang("Version 2.0 ok", false, true), "Version 2.0 ok");
        // An English sentence in a right-to-left paragraph keeps its full stop at its end.
        assert_eq!(vis_lang("Hello world.", true, false), "Hello world.");
        // Right-to-left runs keep W4: "45.6%" is one number.
        assert_eq!(vis_lang("אב 45.6% גד", false, true), "דג 45.6% בא");
        // Without right-to-left text or runs, the fast path.
        let chars: Vec<char> = "plain, text".chars().collect();
        assert_eq!(levels_lang("plain, text", &chars, false, &[false; 11]), vec![0; 11]);
        assert!(is_rtl_lang("he-IL") && is_rtl_lang("ar") && is_rtl_lang("FA-ir") && !is_rtl_lang("en-US") && !is_rtl_lang(""));
    }

    #[test]
    fn l1_resets_trailing_whitespace_and_tabs() {
        let chars: Vec<char> = "ab\tcd  ".chars().collect();
        let l = line_levels(&chars, &[2, 2, 2, 2, 2, 2, 2], 1);
        assert_eq!(l, vec![2, 2, 1, 2, 2, 1, 1]);
        // Short level slices never panic.
        assert_eq!(line_levels(&['a', 'b'], &[], 1), vec![1, 1]);
    }

    #[test]
    fn visual_order_edge_cases() {
        assert!(visual_order(&[]).is_empty());
        assert_eq!(visual_order(&[0, 0]), vec![0, 1]);
        assert_eq!(visual_order(&[1, 1, 1]), vec![2, 1, 0]);
        // Numbers (level 2) inside RTL keep their order.
        assert_eq!(visual_order(&[1, 2, 2, 1]), vec![3, 1, 2, 0]);
        assert_eq!(visual_order(&[125, 125]), vec![1, 0]);
    }

    /// The pre-#403 `levels_lang`, kept as the reference the linear version must match.
    fn reference_levels_lang(text: &str, chars: &[char], rtl: bool, rtl_lang: &[bool]) -> Vec<u8> {
        let base = u8::from(rtl);
        let any_rtl_lang = rtl_lang.iter().any(|r| *r);
        if !rtl && !any_rtl_lang && !needs_bidi(chars) {
            return vec![0; chars.len()];
        }
        let classes: Vec<BidiClass> = chars.iter().map(|c| bidi_class(*c)).collect();
        if classes.iter().any(|c| matches!(c, LRE | RLE | LRO | RLO | PDF | LRI | RLI | FSI | PDI)) {
            return levels(text, chars, rtl);
        }
        let n = chars.len();
        let rl = |i: usize| rtl_lang.get(i).copied().unwrap_or(false);
        let sos = if rtl { Dir::R } else { Dir::L };
        // W1: combining marks take the class of the character they sit on.
        let mut cls = classes;
        for i in 0..n {
            if cls.get(i) == Some(&NSM) {
                let prev = if i == 0 { if rtl { R } else { L } } else { cls.get(i - 1).copied().unwrap_or(L) };
                if let Some(c) = cls.get_mut(i) {
                    *c = prev;
                }
            }
        }
        let mut dir = vec![Dir::Open; n];
        // Strong letters and digits.
        let mut last_strong: Option<BidiClass> = None;
        for i in 0..n {
            let c = cls.get(i).copied().unwrap_or(ON);
            let d = match c {
                L => Dir::L,
                R | AL => Dir::R,
                AN => Dir::Num,
                EN if !rl(i) => Dir::L,
                // W7: European digits after left-to-right text are left to right.
                EN if last_strong == Some(L) => Dir::L,
                EN => Dir::Num,
                _ => Dir::Open,
            };
            if matches!(c, L | R | AL) {
                last_strong = Some(c);
            }
            if let Some(x) = dir.get_mut(i) {
                *x = d;
            }
        }
        // W4/W5 in right-to-left runs: a separator between digits, and currency or percent signs next
        // to them, belong to the number.
        for i in 0..n {
            if !rl(i) || dir.get(i) != Some(&Dir::Open) {
                continue;
            }
            let c = cls.get(i).copied().unwrap_or(ON);
            let num = |j: Option<usize>| j.and_then(|j| dir.get(j)).is_some_and(|d| *d == Dir::Num);
            let joined = match c {
                ES | CS => num(i.checked_sub(1)) && num(Some(i + 1)),
                ET => num(i.checked_sub(1)) || (i + 1..n).find(|&j| cls.get(j) != Some(&ET)).is_some_and(|j| num(Some(j))),
                _ => false,
            };
            if joined && let Some(x) = dir.get_mut(i) {
                *x = Dir::Num;
            }
        }
        // Punctuation in left-to-right runs is left to right.
        for i in 0..n {
            if !rl(i)
                && dir.get(i) == Some(&Dir::Open)
                && !matches!(cls.get(i), Some(WS | S))
                && let Some(x) = dir.get_mut(i)
            {
                *x = Dir::L;
            }
        }
        // Neutrals in right-to-left runs: the direction around them when both sides agree, else
        // right to left. Unresolved spaces of left-to-right runs are looked through.
        let strong_at = |dir: &[Dir], j: usize| match dir.get(j) {
            Some(Dir::L) => Some(Dir::L),
            Some(Dir::R | Dir::Num) => Some(Dir::R),
            _ => None,
        };
        let mut i = 0;
        while i < n {
            if !(rl(i) && dir.get(i) == Some(&Dir::Open)) {
                i += 1;
                continue;
            }
            let start = i;
            while i < n && rl(i) && dir.get(i) == Some(&Dir::Open) {
                i += 1;
            }
            let before = (0..start).rev().find_map(|j| strong_at(&dir, j)).unwrap_or(sos);
            let after = (i..n).find_map(|j| strong_at(&dir, j)).unwrap_or(sos);
            let d = if before == after { before } else { Dir::R };
            for x in dir.get_mut(start..i).into_iter().flatten() {
                *x = d;
            }
        }
        // Spaces in left-to-right runs follow the character before them.
        let mut prev = Dir::L;
        for d in dir.iter_mut() {
            if *d == Dir::Open {
                *d = prev;
            }
            prev = if *d == Dir::Num { Dir::R } else { *d };
        }
        dir.iter()
            .map(|d| match (d, base % 2 == 1) {
                (Dir::L, false) | (Dir::R, true) => base,
                (Dir::L, true) | (Dir::R, false) | (Dir::Open, _) => base + 1,
                (Dir::Num, false) => base + 2,
                (Dir::Num, true) => base + 1,
            })
            .collect()
    }

    fn lang(s: &str, rtl: bool, rtl_lang: &[bool]) -> Vec<u8> {
        let chars: Vec<char> = s.chars().collect();
        levels_lang(s, &chars, rtl, rtl_lang)
    }

    #[test]
    fn next_non_et_points_past_each_et_run() {
        let cls: Vec<BidiClass> = "a$$1%".chars().map(bidi_class).collect();
        assert_eq!(next_non_et(&cls), vec![Some(3), Some(3), Some(3), None, None]);
    }

    #[test]
    fn currency_signs_join_numbers_on_either_side_in_rtl_runs() {
        // Hebrew word, then "$$5" and "5%%": the signs travel with the digits.
        let s = "שלום $$5 5%% $$";
        let n = s.chars().count();
        let l = lang(s, true, &vec![true; n]);
        let chars: Vec<char> = s.chars().collect();
        let num = l[chars.iter().position(|c| *c == '5').unwrap()];
        assert_eq!(&l[5..8], &[num; 3], "{l:?}");
        assert_eq!(&l[9..12], &[num; 3], "{l:?}");
        // Signs with no number next to them stay with the right-to-left text.
        assert_eq!(l[13] % 2, 1);
        assert_ne!(l[13], num);
    }

    #[test]
    fn long_currency_and_neutral_runs_are_linear() {
        // Quadratic resolution took minutes on these; linear finishes at once even unoptimised.
        let n = 200_000;
        let dollars: String = std::iter::once('ש').chain(std::iter::repeat_n('$', n)).collect();
        let l = lang(&dollars, true, &vec![true; n + 1]);
        assert!(l.iter().all(|v| v % 2 == 1));
        // Alternating right-to-left-language neutrals and left-to-right-language spaces: many spans.
        let s: String = std::iter::repeat_n("! ", n / 2).collect();
        let langs: Vec<bool> = (0..n).map(|i| i % 2 == 0).collect();
        assert_eq!(lang(&s, true, &langs).len(), n);
    }

    #[test]
    fn matches_the_reference_on_random_mixed_text() {
        let alphabet: Vec<char> = "aZש ب$%#+-,.:1٣ ()!\u{300}".chars().collect();
        let mut seed = 0x2545_f491_u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for _ in 0..4000 {
            let len = (next() % 24) as usize;
            let chars: Vec<char> = (0..len).map(|_| alphabet[next() as usize % alphabet.len()]).collect();
            let text: String = chars.iter().collect();
            let langs: Vec<bool> = (0..len).map(|_| next() % 3 != 0).collect();
            let rtl = next() % 2 == 0;
            assert_eq!(
                levels_lang(&text, &chars, rtl, &langs),
                reference_levels_lang(&text, &chars, rtl, &langs),
                "{text:?} rtl={rtl} langs={langs:?}"
            );
        }
    }
}
