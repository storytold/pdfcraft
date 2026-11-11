//! PdfCraft patch (104): [`StandardCjkGlyphOrder`], Traditional Chinese read from glyph ids that
//! follow a public character order.
//!
//! A Type 0 font shown through `Identity-H` or `Identity-V` with no `/ToUnicode` and no Adobe
//! character collection says nothing about its characters (ISO 32000-2 §9.10.2): its codes are
//! CIDs (§9.7.6.2), a TrueType descendant's CIDs are its glyph ids through `/CIDToGIDMap`
//! (§9.7.4.2), and a subset's program carries no `cmap` or glyph names. Many Traditional Chinese
//! subsets still keep their original font's glyph ids, and the original font laid its glyphs
//! out in a public order, one of:
//!
//! - **big5**: Big5 code order from `A140`, leaving out `A3C0`–`A3FE` and `C6A1`–`C8FE`;
//! - **uni**: the Unicode code point itself;
//! - **uni-big5**: the characters of Big5 sorted by code point, by rank;
//! - **cns1**: Adobe-CNS1 CID order.
//!
//! so a glyph id is a position in one of them plus a constant offset. The rule finds the order
//! and offset that read the document's glyphs as the most often used characters, and reads only
//! when that answer is clear:
//!
//! - **Evidence** is the document survey's count of each glyph shown ([`FontUsage`]), scored
//!   against [`COMMON_TRADITIONAL`], the 750 characters used most in Taiwan's laws and
//!   regulations. An offset's score is the share of common characters among the glyphs it reads
//!   as characters the rule may output, and an offset that reads fewer than half the glyphs
//!   shown as such isn't scored ([`score`]): digits and Latin letters in a CJK subset don't
//!   dilute the score, and an offset that reads mostly symbols doesn't win on a few glyphs. For
//!   each order the three best offsets are kept, twelve in all; equal scores rank by order name,
//!   then offset ([`rank`]).
//! - **Guards**: the best scores at least 0.6; the best reading that differs from it on at least
//!   half the glyphs shown scores at most 0.6 times as much; a reading that differs on 2–50% of
//!   them (another order that agrees on most characters, such as big5 and cns1) reads common
//!   characters at most 0.6 times as often on the glyphs where the two differ; and the font
//!   shows at least ten different glyphs (three to search at all).
//! - **Fonts that are subsets of one font** in the same document (the survey's groups) are
//!   searched together, unless the name they share is a producer's placeholder
//!   (`CIDFont+F1`, [`pools`]). When the group's reading passes the guards and no member whose
//!   own reading passes reads half its glyphs otherwise ([`pooled_reading_stands`]), every
//!   member reads by it; otherwise each member by its own, when that passes ([`decide`]).
//! - **Output** is limited to ideographs (Big5 `A440` on) and the common punctuation row
//!   (`A141`–`A15E`), and the uni order reads only CJK unified ideographs that Big5 has
//!   ([`in_output_scope`]). The punctuation row is read only at the orders and offsets whose
//!   punctuation fonts' own `/ToUnicode` maps confirmed ([`PUNCTUATION_READINGS`]). A glyph the
//!   embedded program doesn't have (a CID outside a CFF program's charset, a glyph id past a
//!   TrueType program's glyphs) reads nothing ([`has_glyph`]). It only fills glyphs without
//!   strict text.
//!
//! The Big5 order's characters come from the Adobe CMaps `ETen-B5-H` (Big5 code to CID) and
//! `Adobe-CNS1-UCS2` (CID to Unicode) that hayro-cmap embeds, read as Windows code page 950
//! reads Big5 at the eleven codes where the two differ ([`CP950_READINGS`]); cns1 reads
//! `Adobe-CNS1-UCS2` directly.

use super::common_traditional::COMMON_TRADITIONAL;
use super::document::{FontUsage, UsageGroup};
use super::{FontFacts, FontKind, GlyphFacts, RecoveryMode, RecoveryRule, Rule};
use hayro_cmap::{BfString, CMap, CMapName};
use std::sync::{Arc, OnceLock};

/// A public character order a subset's glyph ids may follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Order {
    Big5,
    Cns1,
    Uni,
    UniBig5,
}

impl Order {
    const ALL: [Self; 4] = [Self::Big5, Self::Uni, Self::UniBig5, Self::Cns1];

    /// A stable name, for logs and tests.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Big5 => "big5",
            Self::Uni => "uni",
            Self::UniBig5 => "uni_big5",
            Self::Cns1 => "cns1",
        }
    }
}

/// An order and offset: glyph number `n` reads the character at position `n - offset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Reading {
    pub(crate) order: Order,
    pub(crate) offset: i64,
}

/// The four orders' characters by position, and where each order has a common character.
struct Tables {
    big5: Vec<Option<char>>,
    uni_big5: Vec<char>,
    cns1: Vec<Option<char>>,
    /// Characters the rule may output ([`in_output_scope`]), sorted.
    scope: Vec<char>,
    /// The characters of the common punctuation row (Big5 `A141`–`A15E`), sorted.
    punctuation: Vec<char>,
    /// For each order (in [`Order::ALL`]'s order), the positions holding a character of
    /// [`COMMON_TRADITIONAL`].
    hits: [Vec<u32>; 4],
    /// For each order, the runs of positions (first, last) whose character the rule may output
    /// in that order: what [`score`] weighs against.
    scope_runs: [Vec<(u32, u32)>; 4],
}

/// Positions in the uni order (the Basic Multilingual Plane).
const UNI_LEN: u32 = 0x1_0000;

/// CIDs the cns1 order reads (Adobe-CNS1 up to supplement 7 has 19,179).
const CNS1_LEN: u32 = 19_200;

/// Big5 codes where Windows code page 950 reads another character than the Adobe CMaps give,
/// read as code page 950 does (the readings the rule was measured with): the full-width solidus
/// and reverse solidus, the Suzhou numerals ten and thirty (code page 950 reads them as the
/// ideographs 十 and 卅), the modifier macron, the two compatibility ideographs Big5 encodes twice,
/// and four box-drawing arcs.
const CP950_READINGS: [(u16, char); 11] = [
    (0xA241, '\u{2215}'),
    (0xA242, '\u{FE68}'),
    (0xA2CC, '\u{5341}'),
    (0xA2CE, '\u{5345}'),
    (0xA3BC, '\u{02C9}'),
    (0xC94A, '\u{FA0C}'),
    (0xDDFC, '\u{FA0D}'),
    (0xF9FA, '\u{256D}'),
    (0xF9FB, '\u{256E}'),
    (0xF9FC, '\u{2570}'),
    (0xF9FD, '\u{256F}'),
];

/// The Big5 codes of the big5 order, in order.
fn big5_codes() -> impl Iterator<Item = u16> {
    (0xA1_u16..=0xF9).flat_map(|lead| {
        (0x40_u16..=0x7E)
            .chain(0xA1..=0xFE)
            .map(move |trail| (lead << 8) | trail)
            .filter(|&code| {
                !(0xA3C0..=0xA3FE).contains(&code) && !(0xC6A1..=0xC8FE).contains(&code)
            })
    })
}

/// The one character a CMap's text gives: a character, or one followed only by variation
/// selectors (Adobe-CNS1-UCS2 gives a few CIDs a standardized variant), never U+FFFD.
fn one_char(text: BfString) -> Option<char> {
    let c = match text {
        BfString::Char(c) => c,
        BfString::String(s) => {
            let mut chars = s.chars();
            let first = chars.next()?;
            chars
                .all(|c| matches!(c, '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}'))
                .then_some(first)?
        }
    };
    (c != '\u{FFFD}').then_some(c)
}

fn is_common(c: char) -> bool {
    u16::try_from(u32::from(c)).is_ok_and(|c| COMMON_TRADITIONAL.binary_search(&c).is_ok())
}

/// Positions of `table` holding a common character.
fn hits<T: Copy>(table: &[T], char_of: impl Fn(T) -> Option<char>) -> Vec<u32> {
    table
        .iter()
        .enumerate()
        .filter(|(_, c)| char_of(**c).is_some_and(is_common))
        .filter_map(|(p, _)| u32::try_from(p).ok())
        .collect()
}

/// The runs of positions `0..len` where `inside` holds.
fn runs(len: u32, inside: impl Fn(u32) -> bool) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for p in (0..len).filter(|&p| inside(p)) {
        match out.last_mut() {
            Some((_, last)) if last.checked_add(1) == Some(p) => *last = p,
            _ => out.push((p, p)),
        }
    }
    out
}

fn build_tables() -> Option<Tables> {
    let load = hayro_cmap::load_embedded;
    let eten = CMap::parse(load(CMapName::ETenB5H)?, load)?;
    let ucs2 = CMap::parse(load(CMapName::AdobeCns1Ucs2)?, load)?;
    let text = |cid: u32| ucs2.lookup_bf_string(cid).and_then(one_char);
    let mut big5 = Vec::new();
    let mut scope = Vec::new();
    let mut punctuation = Vec::new();
    for code in big5_codes() {
        let c = match CP950_READINGS.iter().find(|(c, _)| *c == code) {
            Some((_, c)) => Some(*c),
            None => eten.lookup_cid_code(u32::from(code), 2).and_then(text),
        };
        if let Some(c) = c
            && (code >= 0xA440 || (0xA141..=0xA15E).contains(&code))
            && u32::from(c) >= 0x3000
        {
            scope.push(c);
            if (0xA141..=0xA15E).contains(&code) {
                punctuation.push(c);
            }
        }
        big5.push(c);
    }
    scope.sort_unstable();
    scope.dedup();
    punctuation.sort_unstable();
    punctuation.dedup();
    let mut uni_big5: Vec<char> = big5.iter().flatten().copied().collect();
    uni_big5.sort_unstable();
    uni_big5.dedup();
    let cns1: Vec<Option<char>> = (0..CNS1_LEN).map(text).collect();
    let uni_hits = COMMON_TRADITIONAL.iter().map(|&c| u32::from(c)).collect();
    let hits = [
        hits(&big5, |c| c),
        uni_hits,
        hits(&uni_big5, Some),
        hits(&cns1, |c| c),
    ];
    let mut tables = Tables {
        big5,
        uni_big5,
        cns1,
        scope,
        punctuation,
        hits,
        scope_runs: Default::default(),
    };
    let scope_runs = Order::ALL.map(|order| {
        let reading = Reading { order, offset: 0 };
        runs(tables.len(order), |p| {
            tables
                .read(reading, p)
                .is_some_and(|c| in_output_scope(&tables, order, c))
        })
    });
    tables.scope_runs = scope_runs;
    Some(tables)
}

fn tables() -> Option<&'static Tables> {
    static TABLES: OnceLock<Option<Tables>> = OnceLock::new();
    TABLES.get_or_init(build_tables).as_ref()
}

impl Tables {
    fn len(&self, order: Order) -> u32 {
        let n = match order {
            Order::Big5 => self.big5.len(),
            Order::Uni => return UNI_LEN,
            Order::UniBig5 => self.uni_big5.len(),
            Order::Cns1 => self.cns1.len(),
        };
        u32::try_from(n).unwrap_or(u32::MAX)
    }

    fn index(order: Order) -> usize {
        Order::ALL.iter().position(|o| *o == order).unwrap_or(0)
    }

    fn hits(&self, order: Order) -> &[u32] {
        self.hits.get(Self::index(order)).map_or(&[], Vec::as_slice)
    }

    fn scope_runs(&self, order: Order) -> &[(u32, u32)] {
        self.scope_runs
            .get(Self::index(order))
            .map_or(&[], Vec::as_slice)
    }

    /// The character `reading` gives glyph number `number`, as the order has it.
    fn read(&self, reading: Reading, number: u32) -> Option<char> {
        let p = usize::try_from(i64::from(number).checked_sub(reading.offset)?).ok()?;
        match reading.order {
            Order::Big5 => self.big5.get(p).copied().flatten(),
            Order::Uni => u32::try_from(p)
                .ok()
                .filter(|&p| p < UNI_LEN && !(0xD800..0xE000).contains(&p))
                .and_then(char::from_u32),
            Order::UniBig5 => self.uni_big5.get(p).copied(),
            Order::Cns1 => self.cns1.get(p).copied().flatten(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The rule's policies, each in one place.
// ---------------------------------------------------------------------------------------------

/// The share of glyphs shown an offset must read as characters the rule may output to be
/// scored at all.
const MIN_OUTPUT_SHARE: f64 = 0.5;

/// A member of a group whose own reading passes the guards and reads this share of its glyphs
/// otherwise than the group's keeps the group from being read together.
const CONFLICT_SHARE: f64 = 0.5;

/// The orders and offsets at which the punctuation row (Big5 `A141`–`A15E`) is read: those whose
/// punctuation agreed with the fonts' own `/ToUnicode` maps at least a hundred times, in two
/// documents or more, and disagreed at most one time in a hundred. Elsewhere (fonts that lay out
/// their punctuation a place apart from their ideographs, such as the `WIN-BF` families at + 514)
/// the row reads nothing.
const PUNCTUATION_READINGS: [(Order, i64); 8] = [
    (Order::Big5, 257),
    (Order::Big5, 499),
    (Order::Big5, 513),
    (Order::Big5, 659),
    (Order::Big5, 755),
    (Order::Big5, 915),
    (Order::Cns1, 0),
    (Order::UniBig5, 356),
];

/// An offset's score from the glyphs it reads as common characters (`common`), those it reads as
/// characters the rule may output (`output`) and all glyphs shown (`shown`): the share of common
/// characters among the output ones, 0 when fewer than [`MIN_OUTPUT_SHARE`] of the glyphs shown
/// are output. Rounded to nine decimals, so that equal shares compare equal.
fn score(common: u64, output: u64, shown: u64) -> f64 {
    if output == 0 || (output as f64) < MIN_OUTPUT_SHARE * shown as f64 {
        return 0.0;
    }
    ((common as f64 / output as f64) * 1e9).round_ties_even() / 1e9
}

/// The ranking of candidates: by score; among equal scores, the order named last alphabetically,
/// then the larger offset. `a` ranks before `b` when this is `Less`.
fn rank(a: &Candidate, b: &Candidate) -> std::cmp::Ordering {
    b.score
        .total_cmp(&a.score)
        .then_with(|| b.reading.order.name().cmp(a.reading.order.name()))
        .then_with(|| b.reading.offset.cmp(&a.reading.offset))
}

/// Whether a group of subsets is searched together: two members or more, under a name that isn't
/// a producer's placeholder ([`placeholder_name`]), which unrelated fonts share.
fn pools(group: &UsageGroup) -> bool {
    group.members >= 2 && !placeholder_name(&group.name)
}

/// Whether the group's reading `pooled` stands against its members: no member whose own reading
/// passes the guards reads [`CONFLICT_SHARE`] or more of its glyphs shown otherwise (a member
/// that is a different font under the same name).
fn pooled_reading_stands(pooled: Reading, group: &UsageGroup) -> bool {
    let Some(tables) = tables() else {
        return false;
    };
    !group.member_usage.iter().any(|(numbers, shown)| {
        let Some(own) = search(numbers).filter(Searched::passes) else {
            return false;
        };
        let differ = numbers
            .iter()
            .filter(|&&(g, _)| tables.read(pooled, g) != tables.read(own.best, g))
            .fold(0_u64, |t, &(_, n)| t.saturating_add(n));
        differ as f64 >= CONFLICT_SHARE * *shown as f64
    })
}

/// Whether a font's name (without its subset tag) is a producer's placeholder, the same for
/// unrelated fonts: `CIDFont+F` and a number, `F` and a number, a number or nothing, once the
/// encoding names that follow it (`-ETen…`, `-Identit…`, `-KSCms…`, `-UniCNS…`, `-B5pc…`, `,…`)
/// are cut.
fn placeholder_name(name: &[u8]) -> bool {
    let cut = [
        &b"-ETen"[..],
        b"-Identit",
        b"-KSCms",
        b"-UniCNS",
        b"-B5pc",
        b",",
    ]
    .iter()
    .filter_map(|s| name.windows(s.len()).position(|w| w == *s))
    .min()
    .unwrap_or(name.len());
    let core = name.get(..cut).unwrap_or(name);
    let digits = |s: &[u8]| s.iter().all(u8::is_ascii_digit);
    let numbered = |prefix: &[u8]| {
        core.strip_prefix(prefix)
            .is_some_and(|d| !d.is_empty() && digits(d))
    };
    digits(core) || numbered(b"CIDFont+F") || numbered(b"F")
}

/// Whether the rule outputs `c`, read in `order`: a character with a Big5 code
/// from `A440` (the ideographs) or in `A141`–`A15E` (the common punctuation), at U+3000 or
/// above; the uni order reads CJK unified ideographs only.
fn in_output_scope(tables: &Tables, order: Order, c: char) -> bool {
    if order == Order::Uni && !matches!(c, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}') {
        return false;
    }
    tables.scope.binary_search(&c).is_ok()
}

/// What the rule outputs for glyph number `number` read with `reading`: a character in its
/// output scope, the punctuation row only at [`PUNCTUATION_READINGS`], in canonical form.
fn output(tables: &Tables, reading: Reading, number: u32) -> Option<char> {
    let c = tables
        .read(reading, number)
        .filter(|&c| in_output_scope(tables, reading.order, c))?;
    if tables.punctuation.binary_search(&c).is_ok()
        && !PUNCTUATION_READINGS.contains(&(reading.order, reading.offset))
    {
        return None;
    }
    Some(canonical(c))
}

/// Whether the embedded program has a glyph for a code: for a CID-keyed CFF program, its CID is
/// in the program's charset (`cids`, sorted); for a TrueType or OpenType program, the glyph id the
/// CID maps to is neither `.notdef` (what a CID a `/CIDToGIDMap` stream leaves out maps to) nor
/// past the program's glyph count. A code without a glyph shows nothing on the page, whatever its
/// number reads as. This filters each glyph's output only: the survey's counts, and so the font's
/// decision, include such codes as shown.
fn has_glyph(
    kind: FontKind,
    number: u32,
    cids: Option<&[u32]>,
    program_glyphs: Option<u32>,
) -> bool {
    match kind {
        FontKind::CidCff => {
            cids.is_some_and(|cids| number != 0 && cids.binary_search(&number).is_ok())
        }
        _ => number != 0 && program_glyphs.is_none_or(|n| number < n),
    }
}

/// The canonical form of a reading (Unicode normalization form C): Big5 encodes 兀 and 嗀 twice,
/// and code page 950 reads the second code of each as a compatibility ideograph.
fn canonical(c: char) -> char {
    match c {
        '\u{FA0C}' => '\u{5140}',
        '\u{FA0D}' => '\u{55C0}',
        c => c,
    }
}

// ---------------------------------------------------------------------------------------------
// The search and the guards.
// ---------------------------------------------------------------------------------------------

/// The guards' thresholds.
const BEST: f64 = 0.6;
const RIVAL: f64 = 0.6;
const CLOSE: f64 = 0.6;
const MIN_GLYPHS: usize = 10;
/// Fewer different glyphs than this aren't searched at all.
const MIN_SEARCHED: usize = 3;
/// Offsets kept for each order.
const PER_ORDER: usize = 3;

/// One order and offset found by the search, with its score.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    reading: Reading,
    score: f64,
}

/// What the search and guards make of one set of counts.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Searched {
    pub(crate) best: Reading,
    pub(crate) best_score: f64,
    /// The best reading that differs from `best` on at least half the glyphs shown (0 if none).
    pub(crate) rival_score: f64,
    /// The highest ratio of common characters a close reading (differing on 2–50% of the
    /// glyphs shown) reads where it differs from `best`.
    pub(crate) close: f64,
    pub(crate) glyphs: usize,
}

impl Searched {
    pub(crate) fn passes(&self) -> bool {
        self.best_score >= BEST
            && self.rival_score <= RIVAL * self.best_score
            && self.close <= CLOSE
            && self.glyphs >= MIN_GLYPHS
    }
}

/// Add `n` at every offset `number - p` for `p` in `first..=last`, through the difference array
/// `diff` (offset `c` at index `c + len - 1`).
fn add_run(diff: &mut [i64], number: u32, first: u32, last: u32, len: i64, n: u64) {
    let n = i64::try_from(n).unwrap_or(i64::MAX);
    let at = |p: u32| usize::try_from(i64::from(number) - i64::from(p) + len - 1).ok();
    if let Some(lo) = at(last).and_then(|i| diff.get_mut(i)) {
        *lo = lo.saturating_add(n);
    }
    if let Some(hi) = at(first)
        .and_then(|i| i.checked_add(1))
        .and_then(|i| diff.get_mut(i))
    {
        *hi = hi.saturating_sub(n);
    }
}

/// Work (numbers times runs) up to which [`output_counts`] adds runs directly; past it, it
/// correlates by a fast Fourier transform.
const DIRECT_WORK: usize = 1 << 22;

#[cfg(feature = "test-counters")]
thread_local! {
    /// Tests: the work past which [`output_counts`] correlates instead (`None`: [`DIRECT_WORK`]).
    static DIRECT_WORK_OVERRIDE: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

fn direct_work() -> usize {
    #[cfg(feature = "test-counters")]
    if let Some(w) = DIRECT_WORK_OVERRIDE.with(std::cell::Cell::get) {
        return w;
    }
    DIRECT_WORK
}

/// For every offset (index `c + len - 1`, `span` of them), how many of the glyphs shown it
/// reads as a character the rule may output in `order`.
fn output_counts(
    tables: &Tables,
    order: Order,
    numbers: &[(u32, u64)],
    span: usize,
    len: i64,
) -> Vec<u64> {
    let runs = tables.scope_runs(order);
    if numbers.len().saturating_mul(runs.len()) <= direct_work() {
        let mut diff = vec![0_i64; span.saturating_add(1)];
        for &(first, last) in runs {
            for &(number, n) in numbers {
                add_run(&mut diff, number, first, last, len, n);
            }
        }
        let mut running = 0_i64;
        return diff
            .iter()
            .take(span)
            .map(|d| {
                running = running.saturating_add(*d);
                u64::try_from(running).unwrap_or(0)
            })
            .collect();
    }
    // out[c + len - 1] = sum over p of scope[p] * shown[p + c]: a cross-correlation.
    let Ok(len_u) = usize::try_from(len) else {
        return vec![0; span];
    };
    let size = span.saturating_add(len_u).next_power_of_two();
    let mut shown = vec![(0.0_f64, 0.0_f64); size];
    for &(number, n) in numbers {
        if let Some(slot) = usize::try_from(number).ok().and_then(|i| shown.get_mut(i)) {
            slot.0 += n as f64;
        }
    }
    let mut scope = vec![(0.0_f64, 0.0_f64); size];
    for &(first, last) in runs {
        for p in first..=last {
            // Reversed: scope[len - 1 - p].
            if let Some(slot) = usize::try_from(p)
                .ok()
                .and_then(|p| len_u.checked_sub(1)?.checked_sub(p))
                .and_then(|i| scope.get_mut(i))
            {
                slot.0 = 1.0;
            }
        }
    }
    fft(&mut shown, false);
    fft(&mut scope, false);
    for (a, b) in shown.iter_mut().zip(&scope) {
        *a = (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0);
    }
    fft(&mut shown, true);
    let n = size as f64;
    shown
        .iter()
        .take(span)
        .map(|(re, _)| {
            let v = (re / n).round();
            if v.is_finite() && v > 0.0 {
                v as u64
            } else {
                0
            }
        })
        .collect()
}

/// An in-place radix-2 fast Fourier transform of `a` (its length a power of two), or its inverse
/// without the division by the length.
fn fft(a: &mut [(f64, f64)], inverse: bool) {
    let n = a.len();
    if n < 2 || !n.is_power_of_two() {
        return;
    }
    let mut j = 0_usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            a.swap(i, j);
        }
    }
    let mut width = 2_usize;
    while width <= n {
        let angle = if inverse { 2.0 } else { -2.0 } * std::f64::consts::PI / width as f64;
        let step = (angle.cos(), angle.sin());
        for start in (0..n).step_by(width) {
            let mut w = (1.0_f64, 0.0_f64);
            for k in 0..width / 2 {
                let (Some(&u), Some(&v)) = (a.get(start + k), a.get(start + k + width / 2)) else {
                    continue;
                };
                let t = (v.0 * w.0 - v.1 * w.1, v.0 * w.1 + v.1 * w.0);
                if let Some(x) = a.get_mut(start + k) {
                    *x = (u.0 + t.0, u.1 + t.1);
                }
                if let Some(x) = a.get_mut(start + k + width / 2) {
                    *x = (u.0 - t.0, u.1 - t.1);
                }
                w = (w.0 * step.0 - w.1 * step.1, w.0 * step.1 + w.1 * step.0);
            }
        }
        width <<= 1;
    }
}

/// The best [`PER_ORDER`] offsets of `order` for `numbers` (glyph number, times shown), by their
/// score; equal scores keep the smaller offset.
fn best_offsets(
    tables: &Tables,
    order: Order,
    numbers: &[(u32, u64)],
    shown: u64,
) -> Vec<Candidate> {
    let len = i64::from(tables.len(order));
    let Some(&(max, _)) = numbers.last() else {
        return Vec::new();
    };
    // Offset `c` (from `-(len - 1)` to `max`) at index `c + len - 1`.
    let span = usize::try_from(i64::from(max).saturating_add(len)).unwrap_or(0);
    let mut common = vec![0_u64; span];
    for &p in tables.hits(order) {
        let shift = len - 1 - i64::from(p);
        for &(number, n) in numbers {
            if let Some(slot) = usize::try_from(i64::from(number) + shift)
                .ok()
                .and_then(|i| common.get_mut(i))
            {
                *slot = slot.saturating_add(n);
            }
        }
    }
    let output = output_counts(tables, order, numbers, span, len);
    let mut best: Vec<(f64, usize)> = Vec::with_capacity(PER_ORDER + 1);
    for (i, &c) in common.iter().enumerate() {
        let s = score(c, output.get(i).copied().unwrap_or(0), shown);
        if best.len() == PER_ORDER && best.last().is_some_and(|&(b, _)| s <= b) {
            continue;
        }
        let at = best.partition_point(|&(b, _)| b >= s);
        best.insert(at, (s, i));
        best.truncate(PER_ORDER);
    }
    best.into_iter()
        .filter_map(|(score, i)| {
            let offset = i64::try_from(i).ok()? - (len - 1);
            Some(Candidate {
                reading: Reading { order, offset },
                score,
            })
        })
        .collect()
}

/// Search `numbers` (glyph number, times shown; sorted by number) in every order and weigh the
/// best reading against its rivals. `None` for fewer than [`MIN_SEARCHED`]
/// different glyphs.
pub(crate) fn search(numbers: &[(u32, u64)]) -> Option<Searched> {
    let tables = tables()?;
    if numbers.len() < MIN_SEARCHED {
        return None;
    }
    let shown: u64 = numbers.iter().fold(0_u64, |t, &(_, n)| t.saturating_add(n));
    let mut candidates: Vec<Candidate> = Order::ALL
        .iter()
        .flat_map(|&order| best_offsets(tables, order, numbers, shown))
        .collect();
    candidates.sort_by(rank);
    let (best, rivals) = candidates.split_first()?;
    let read = |reading: Reading| -> Vec<Option<char>> {
        numbers
            .iter()
            .map(|&(g, _)| tables.read(reading, g))
            .collect()
    };
    let best_read = read(best.reading);
    let (mut rival_score, mut close) = (None, 0.0_f64);
    for rival in rivals {
        let rival_read = read(rival.reading);
        let (mut differ, mut best_common, mut rival_common) = (0_u64, 0_u64, 0_u64);
        for ((&(_, n), b), r) in numbers.iter().zip(&best_read).zip(&rival_read) {
            if b == r {
                continue;
            }
            differ = differ.saturating_add(n);
            if b.is_some_and(is_common) {
                best_common = best_common.saturating_add(n);
            }
            if r.is_some_and(is_common) {
                rival_common = rival_common.saturating_add(n);
            }
        }
        if differ as f64 >= 0.5 * shown as f64 {
            if rival_score.is_none() {
                rival_score = Some(rival.score);
            }
        } else if differ as f64 >= 0.02 * shown as f64 {
            let ratio = if best_common > 0 {
                rival_common as f64 / best_common as f64
            } else if rival_common > 0 {
                1.0
            } else {
                0.0
            };
            close = close.max(ratio);
        }
    }
    Some(Searched {
        best: best.reading,
        best_score: best.score,
        rival_score: rival_score.unwrap_or(0.0),
        close,
        glyphs: numbers.len(),
    })
}

/// The reading a group of subsets reads with, when it is searched together, passes the guards
/// and stands against its members ([`pools`], [`pooled_reading_stands`]).
fn group_reading(group: &UsageGroup) -> Option<Reading> {
    if !pools(group) {
        return None;
    }
    let pooled = search(&group.numbers).filter(Searched::passes)?.best;
    pooled_reading_stands(pooled, group).then_some(pooled)
}

/// A font's reading from the document's evidence: its group's (worked out once for the group),
/// else its own, when that passes the guards.
pub(crate) fn decide(usage: &FontUsage) -> Option<Reading> {
    let pooled = usage.group.as_ref().and_then(|group| {
        let decided = group
            .decision
            .get_or_init(|| Arc::new(group_reading(group)));
        decided.downcast_ref::<Option<Reading>>().copied().flatten()
    });
    pooled.or_else(|| {
        search(&usage.numbers)
            .filter(Searched::passes)
            .map(|s| s.best)
    })
}

/// [`RecoveryRule::StandardCjkGlyphOrder`]: see the module documentation.
pub(crate) struct StandardCjkGlyphOrder;

/// What [`StandardCjkGlyphOrder`] decides for a font: its reading, and for a CID-keyed CFF
/// program the CIDs of its charset, sorted ([`has_glyph`]).
#[derive(Debug)]
pub(crate) struct FontReading {
    reading: Reading,
    cids: Option<Arc<[u32]>>,
}

impl Rule for StandardCjkGlyphOrder {
    const RULE: RecoveryRule = RecoveryRule::StandardCjkGlyphOrder;
    const KINDS: &'static [FontKind] = &[FontKind::CidTrueType, FontKind::CidCff];
    type Guard = FontReading;

    fn guard(font: &FontFacts<'_>) -> Option<FontReading> {
        if font.has_to_unicode || !font.identity_encoding {
            return None;
        }
        let reading = decide(font.usage.as_deref()?)?;
        let cids = (font.kind == FontKind::CidCff).then(|| {
            let mut cids = font.ids.map(|ids| ids()).unwrap_or_default();
            cids.sort_unstable();
            cids.dedup();
            Arc::from(cids)
        });
        Some(FontReading { reading, cids })
    }

    fn read(
        guard: &FontReading,
        font: &FontFacts<'_>,
        glyph: &GlyphFacts<'_>,
    ) -> Option<(BfString, RecoveryMode)> {
        let number = match font.kind {
            FontKind::CidTrueType => glyph.gid.to_u32(),
            FontKind::CidCff => glyph.id?,
            _ => return None,
        };
        if !has_glyph(
            font.kind,
            number,
            guard.cids.as_deref(),
            font.program_glyphs,
        ) {
            return None;
        }
        let c = output(tables()?, guard.reading, number)?;
        Some((BfString::Char(c), RecoveryMode::Fill))
    }
}

/// Test and verification hooks (the `test-counters` feature).
#[cfg(feature = "test-counters")]
pub(crate) mod hooks {
    use super::*;

    /// The character `order` (by name) has at `position`, as the rule reads it before its output
    /// scope; `None` for an unknown order or an empty position.
    pub(crate) fn order_char(order: &str, position: u32) -> Option<char> {
        let order = *Order::ALL.iter().find(|o| o.name() == order)?;
        tables()?.read(Reading { order, offset: 0 }, position)
    }

    /// Whether a search with these scores, close ratio and glyph count passes the guards.
    pub(crate) fn guards(best: f64, rival: f64, close: f64, glyphs: usize) -> bool {
        Searched {
            best: Reading {
                order: Order::Big5,
                offset: 0,
            },
            best_score: best,
            rival_score: rival,
            close,
            glyphs,
        }
        .passes()
    }

    /// Whether `c` is one of the common characters the rule scores by.
    pub(crate) fn common(c: char) -> bool {
        is_common(c)
    }

    /// Whether the rule outputs `c` read in `order`.
    pub(crate) fn outputs(order: &str, c: char) -> bool {
        let Some(order) = Order::ALL.iter().find(|o| o.name() == order) else {
            return false;
        };
        tables().is_some_and(|t| in_output_scope(t, *order, c))
    }

    /// Whether `name` (a `/BaseFont` without its subset tag) is a producer's placeholder.
    pub(crate) fn placeholder(name: &str) -> bool {
        placeholder_name(name.as_bytes())
    }

    /// The character the rule outputs for glyph `number` read in `order` at `offset`.
    pub(crate) fn output_char(order: &str, offset: i64, number: u32) -> Option<char> {
        let order = *Order::ALL.iter().find(|o| o.name() == order)?;
        output(tables()?, Reading { order, offset }, number)
    }

    /// The reading a group named `name` (without its subset tag) of fonts with `members`' counts
    /// (each sorted by number) reads with, if any.
    pub(crate) fn group_reading_of(
        name: &str,
        members: &[Vec<(u32, u64)>],
    ) -> Option<(&'static str, i64)> {
        let mut pooled: std::collections::BTreeMap<u32, u64> = std::collections::BTreeMap::new();
        for &(g, n) in members.iter().flatten() {
            *pooled.entry(g).or_insert(0) += n;
        }
        let member_usage = members
            .iter()
            .map(|m| (m.clone(), m.iter().map(|&(_, n)| n).sum()))
            .collect();
        let group = UsageGroup::new(
            name.as_bytes().to_vec(),
            member_usage,
            pooled.into_iter().collect(),
        );
        group_reading(&group).map(|r| (r.order.name(), r.offset))
    }

    /// Whether the output counts of `numbers` in every order are the same added run by run and
    /// correlated by the Fourier transform.
    pub(crate) fn output_counts_agree(numbers: &[(u32, u64)]) -> bool {
        let Some(tables) = tables() else {
            return false;
        };
        let Some(&(max, _)) = numbers.last() else {
            return true;
        };
        Order::ALL.iter().all(|&order| {
            let len = i64::from(tables.len(order));
            let span = usize::try_from(i64::from(max) + len).unwrap_or(0);
            let set = |w: Option<usize>| DIRECT_WORK_OVERRIDE.with(|c| c.set(w));
            set(Some(usize::MAX));
            let direct = output_counts(tables, order, numbers, span, len);
            set(Some(0));
            let fourier = output_counts(tables, order, numbers, span, len);
            set(None);
            direct == fourier
        })
    }

    fn describe(s: Option<Searched>) -> String {
        match s {
            Some(s) => format!(
                "{} {} {:.6} {:.6} {:.6} {} {}",
                s.best.order.name(),
                s.best.offset,
                s.best_score,
                s.rival_score,
                s.close,
                s.glyphs,
                s.passes()
            ),
            None => "-".into(),
        }
    }

    /// See `test_hooks::cjk_report`.
    pub(crate) fn report(
        pdf: &hayro_syntax::Pdf,
        settings: &crate::InterpreterSettings,
    ) -> Option<Vec<String>> {
        let (over, fonts) = super::super::survey::report(pdf, settings)?;
        let tables = tables()?;
        let mut out = vec![format!("over_cap\t{over}")];
        for f in fonts {
            let (single, pooled, decided, how) = match &f.usage {
                Some(u) => {
                    let single = search(&u.numbers);
                    let decided = decide(u);
                    let group = u.group.as_ref().and_then(|g| {
                        g.decision
                            .get()
                            .and_then(|d| d.downcast_ref::<Option<Reading>>().copied().flatten())
                    });
                    let pooled = u.group.as_ref().map(|g| (g.members, search(&g.numbers)));
                    let how = match (decided, group) {
                        (Some(_), Some(_)) => "pooled",
                        (Some(_), None) => "single",
                        _ => "",
                    };
                    (single, pooled, decided, how)
                }
                None => (None, None, None, ""),
            };
            let readings: Vec<String> = match decided {
                Some(r) => f
                    .codes
                    .iter()
                    .filter_map(|&(code, _, number)| {
                        let number = number?;
                        if !has_glyph(f.kind, number, f.charset.as_deref(), f.program_glyphs) {
                            return None;
                        }
                        let c = output(tables, r, number)?;
                        Some(format!("{code}:{c}"))
                    })
                    .collect(),
                None => Vec::new(),
            };
            out.push(format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                f.object.map_or(-1, |o| o),
                String::from_utf8_lossy(&f.base),
                f.codes.len(),
                f.codes.iter().map(|c| c.1).sum::<u64>(),
                describe(single),
                pooled.map_or("-".into(), |(m, p)| format!("{m} {}", describe(p))),
                decided.map_or("-".into(), |r| format!("{} {}", r.order.name(), r.offset)),
                how,
                readings.join(" ")
            ));
        }
        Some(out)
    }

    /// The search of `numbers`: (order, offset, best score, rival score, close ratio, glyphs,
    /// passes).
    pub(crate) fn searched(
        numbers: &[(u32, u64)],
    ) -> Option<(&'static str, i64, f64, f64, f64, usize, bool)> {
        search(numbers).map(|s| {
            (
                s.best.order.name(),
                s.best.offset,
                s.best_score,
                s.rival_score,
                s.close,
                s.glyphs,
                s.passes(),
            )
        })
    }
}
