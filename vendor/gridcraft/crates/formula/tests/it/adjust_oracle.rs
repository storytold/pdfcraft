//! `adjust`, `adjust_if_changed` and `shift_relative` against a verbatim copy of the previous
//! implementation, which cloned and rebuilt every formula. The rewrite must give the same tree
//! (and so the same text) for every formula and edit, and `adjust_if_changed` must say
//! "unchanged" exactly when the old code returned the formula as it was.

use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS, RangeRef};
use gridcraft_formula::adjust::{Axis, Edit, adjust, adjust_if_changed, shift_relative};
use gridcraft_formula::{Expr, parse, print};

/// The implementation before the borrowed check, kept as the test oracle.
mod old {
    use gridcraft_core::{CellError, MAX_COLS, MAX_ROWS};
    use gridcraft_formula::adjust::{Axis, Edit};
    use gridcraft_formula::{Anchor, Expr, RefKind, Reference, SheetSel};

    fn clamp_anchor(row: i64, col: i64) -> Option<(u32, u32)> {
        if (0..MAX_ROWS as i64).contains(&row) && (0..MAX_COLS as i64).contains(&col) { Some((row as u32, col as u32)) } else { None }
    }

    pub fn shift_relative(e: Expr, dr: i64, dc: i64) -> Expr {
        e.map(&mut |x| match x {
            Expr::Ref(r) => match shift_ref(&r, dr, dc) {
                Some(r) => Expr::Ref(r),
                None => Expr::Error(CellError::Ref),
            },
            other => other,
        })
    }

    fn shift_anchor(a: &Anchor, dr: i64, dc: i64) -> Option<Anchor> {
        let row = if a.row_abs { a.row as i64 } else { a.row as i64 + dr };
        let col = if a.col_abs { a.col as i64 } else { a.col as i64 + dc };
        let (row, col) = clamp_anchor(row, col)?;
        Some(Anchor { row, col, ..*a })
    }

    fn shift_line(v: u32, abs: bool, d: i64, max: u32) -> Option<u32> {
        if abs {
            return Some(v);
        }
        let n = v as i64 + d;
        if (0..max as i64).contains(&n) { Some(n as u32) } else { None }
    }

    fn shift_ref(r: &Reference, dr: i64, dc: i64) -> Option<Reference> {
        let kind = match &r.kind {
            RefKind::Cell(a) => RefKind::Cell(shift_anchor(a, dr, dc)?),
            RefKind::Range(a, b) => RefKind::Range(shift_anchor(a, dr, dc)?, shift_anchor(b, dr, dc)?),
            RefKind::Rows(r0, a0, r1, a1) => RefKind::Rows(shift_line(*r0, *a0, dr, MAX_ROWS)?, *a0, shift_line(*r1, *a1, dr, MAX_ROWS)?, *a1),
            RefKind::Cols(c0, a0, c1, a1) => RefKind::Cols(shift_line(*c0, *a0, dc, MAX_COLS)?, *a0, shift_line(*c1, *a1, dc, MAX_COLS)?, *a1),
        };
        Some(Reference { sheet: r.sheet.clone(), kind })
    }

    pub fn adjust(e: Expr, host: &str, target: &str, edit: &Edit) -> Expr {
        e.map(&mut |x| match x {
            Expr::Ref(r) => {
                let on_target = match &r.sheet {
                    SheetSel::Current => host.eq_ignore_ascii_case(target),
                    SheetSel::Named(n) => n.eq_ignore_ascii_case(target),
                    SheetSel::Span(..) => false,
                };
                if !on_target {
                    return Expr::Ref(r);
                }
                match adjust_ref(&r, edit) {
                    Some(r) => Expr::Ref(r),
                    None => Expr::Error(CellError::Ref),
                }
            }
            other => other,
        })
    }

    fn map_index(v: u32, at: u32, count: u32, insert: bool, max: u32) -> Option<u32> {
        if insert {
            if v >= at {
                let n = v as u64 + count as u64;
                if n >= max as u64 { None } else { Some(n as u32) }
            } else {
                Some(v)
            }
        } else if v < at {
            Some(v)
        } else if v >= at.saturating_add(count) {
            Some(v - count)
        } else {
            None
        }
    }

    fn map_span(lo: u32, hi: u32, at: u32, count: u32, insert: bool, max: u32) -> Option<(u32, u32)> {
        if insert {
            let lo2 = if lo >= at { lo as u64 + count as u64 } else { lo as u64 };
            let hi2 = if hi >= at { hi as u64 + count as u64 } else { hi as u64 };
            let hi2 = hi2.min(max as u64 - 1);
            if lo2 > hi2 {
                return None;
            }
            Some((lo2 as u32, hi2 as u32))
        } else {
            let end = at.saturating_add(count);
            if lo >= at && hi < end {
                return None;
            }
            let lo2 = if lo < at {
                lo
            } else if lo >= end {
                lo - count
            } else {
                at
            };
            let hi2 = if hi < at {
                hi
            } else if hi >= end {
                hi - count
            } else {
                at.saturating_sub(1)
            };
            Some((lo2, hi2.max(lo2)))
        }
    }

    fn adjust_ref(r: &Reference, edit: &Edit) -> Option<Reference> {
        match edit {
            Edit::Insert { axis, at, count } | Edit::Delete { axis, at, count } => {
                let insert = matches!(edit, Edit::Insert { .. });
                let kind = match (&r.kind, axis) {
                    (RefKind::Cell(a), Axis::Rows) => RefKind::Cell(Anchor { row: map_index(a.row, *at, *count, insert, MAX_ROWS)?, ..*a }),
                    (RefKind::Cell(a), Axis::Cols) => RefKind::Cell(Anchor { col: map_index(a.col, *at, *count, insert, MAX_COLS)?, ..*a }),
                    (RefKind::Range(a, b), Axis::Rows) => {
                        let (lo, hi) = map_span(a.row.min(b.row), a.row.max(b.row), *at, *count, insert, MAX_ROWS)?;
                        RefKind::Range(Anchor { row: lo, ..*a }, Anchor { row: hi, ..*b })
                    }
                    (RefKind::Range(a, b), Axis::Cols) => {
                        let (lo, hi) = map_span(a.col.min(b.col), a.col.max(b.col), *at, *count, insert, MAX_COLS)?;
                        RefKind::Range(Anchor { col: lo, ..*a }, Anchor { col: hi, ..*b })
                    }
                    (RefKind::Rows(r0, a0, r1, a1), Axis::Rows) => {
                        let (lo, hi) = map_span(*r0.min(r1), *r0.max(r1), *at, *count, insert, MAX_ROWS)?;
                        RefKind::Rows(lo, *a0, hi, *a1)
                    }
                    (RefKind::Cols(c0, a0, c1, a1), Axis::Cols) => {
                        let (lo, hi) = map_span(*c0.min(c1), *c0.max(c1), *at, *count, insert, MAX_COLS)?;
                        RefKind::Cols(lo, *a0, hi, *a1)
                    }
                    (k, _) => k.clone(),
                };
                Some(Reference { sheet: r.sheet.clone(), kind })
            }
            Edit::InsertCells { axis, range } | Edit::DeleteCells { axis, range } => {
                let span = r.range();
                let (inside, at, count) = match axis {
                    Axis::Rows => (range.start.col <= span.start.col && span.end.col <= range.end.col, range.start.row, range.height()),
                    Axis::Cols => (range.start.row <= span.start.row && span.end.row <= range.end.row, range.start.col, range.width()),
                };
                if !inside {
                    return Some(r.clone());
                }
                let axis = *axis;
                let lines =
                    if matches!(edit, Edit::InsertCells { .. }) { Edit::Insert { axis, at, count } } else { Edit::Delete { axis, at, count } };
                adjust_ref(r, &lines)
            }
            Edit::Move { from, to_row, to_col } => {
                let dr = *to_row as i64 - from.start.row as i64;
                let dc = *to_col as i64 - from.start.col as i64;
                let range = r.range();
                if !from.contains_range(&range) {
                    return Some(r.clone());
                }
                let mv = |a: &Anchor| -> Option<Anchor> {
                    let (row, col) = clamp_anchor(a.row as i64 + dr, a.col as i64 + dc)?;
                    Some(Anchor { row, col, ..*a })
                };
                let kind = match &r.kind {
                    RefKind::Cell(a) => RefKind::Cell(mv(a)?),
                    RefKind::Range(a, b) => RefKind::Range(mv(a)?, mv(b)?),
                    RefKind::Rows(a, aa, b, ba) => {
                        let dr = *to_row as i64 - from.start.row as i64;
                        RefKind::Rows(shift_line(*a, false, dr, MAX_ROWS)?, *aa, shift_line(*b, false, dr, MAX_ROWS)?, *ba)
                    }
                    RefKind::Cols(a, aa, b, ba) => {
                        let dc = *to_col as i64 - from.start.col as i64;
                        RefKind::Cols(shift_line(*a, false, dc, MAX_COLS)?, *aa, shift_line(*b, false, dc, MAX_COLS)?, *ba)
                    }
                };
                Some(Reference { sheet: r.sheet.clone(), kind })
            }
            Edit::MoveToSheet { from, to_sheet, to_row, to_col } => {
                if !from.contains_range(&r.range()) {
                    return Some(r.clone());
                }
                let mut moved = adjust_ref(r, &Edit::Move { from: *from, to_row: *to_row, to_col: *to_col })?;
                moved.sheet = SheetSel::Named(to_sheet.clone());
                Some(moved)
            }
        }
    }
}

/// Formulas with every reference shape the adjustment distinguishes.
const FORMULAS: &[&str] = &[
    "A1",
    "B2+C3*D4",
    "$B$2+B$2+$B2",
    "SUM(A1:A10)+SUM(B2:C3)+SUM($B$2:$C$3)",
    "SUM(C3:B2)",
    "SUM(A:A)+SUM($B:$C)+SUM(2:2)+SUM($3:$5)",
    "SUM(A1:.A7)+SUM(B:.B)+ROWS(2.:.6)+SUM(A1.:B1)",
    "S!B2+s!C3+'S'!A1:B9",
    "Other!B2+Other!A:A+Other!2:3",
    "'My Sheet'!B2+'my sheet'!C3:D4",
    "Dest!B2+S!B2",
    "SUM(S:Other!B2)+SUM(Other:S!A1:C3)",
    "XFD1048576+XFD1+A1048576+SUM(XFC1048575:XFD1048576)",
    "SUM(1048576:1048576)+SUM(XFD:XFD)",
    "VLOOKUP($B2,Lookup!$A$1:$D$500,3,FALSE)",
    "IF(AND(C2>0,D2<100),C2*1.07,IF(E2=\"x\",\"yes\",\"no\"))",
    "Table1[@Qty]*Table1[@Price]+SUM(Table1[[#Data],[Qty]])",
    "SUMPRODUCT(A2:C2,{1,2,3})",
    "LET(x,A2*2,y,B2+x,x*y)",
    "LAMBDA(x,x+B2)(C3)",
    "IFERROR(INDEX(Lookup!$A:$A,MATCH(F2,Lookup!$B:$B,0)),\"\")",
    "-B2%+(C3)^2&\"a\"",
    "A1#+S!B2#",
    "S!MyName+MyName+#REF!+S!#REF!",
    "COUNTIFS(Data!$A:$A,A2,Data!$C:$C,\">=\"&$G$2)",
    "1+2",
    "\"text\"",
    "B2:C3 B3:D4",
    "(B2:C3,E5)",
    "INDIRECT(\"B2\")+OFFSET(B2,1,1)",
    // From the parser edge cases (prefixes and LET parameter names).
    "LET(_xlpm.TRUE,1,_xlpm.TRUE+1)",
    "LET(_xlpm.A1,1,_xlpm.A1)",
    "LAMBDA(_xlpm.x,_xlpm.x+1)(2)",
    "_xlfn.LET(_xlpm.R1C1,1,_xlpm.R1C1*2)",
    "LET(_xlpm.FALSE,2,_xlpm.FALSE)",
    "_xlpm.SUM(1)",
    "LET(_xlpm.x_1,1,_xlpm.x_1)",
    "_xlfn.ANCHORARRAY(A1)+_xlfn.SINGLE(B1:B3)",
    "SUM(_xlfn._TRO_ALL(A1:E10))",
];

fn col(c: u32) -> String {
    gridcraft_core::addr::col_to_letters(c)
}

/// The realistic mix the structural-edit benchmark uses, row-dependent.
fn mix(i: usize, r: u32) -> String {
    let r1 = r + 1;
    match i % 12 {
        0 => format!("SUM(A{r1}:A{})", r1 + 9),
        1 => format!("VLOOKUP($B{r1},Lookup!$A$1:$D$500,3,FALSE)"),
        2 => format!("IF(AND(C{r1}>0,D{r1}<100),C{r1}*1.07,IF(E{r1}=\"x\",\"yes\",\"no\"))"),
        3 => format!("'My Sheet'!B{r1}+Data!C{r1}*2-1"),
        4 => "Table1[@Qty]*Table1[@Price]".to_string(),
        5 => format!("SUMPRODUCT(A{r1}:C{r1},{{1,2,3}})+SUM(Table1[[#Data],[Qty]])"),
        6 => format!("\"Lorem ipsum\"&A{r1}&\" units\""),
        7 => format!("IFERROR(INDEX(Lookup!$A:$A,MATCH(F{r1},Lookup!$B:$B,0)),\"\")"),
        8 => (0..20).map(|k| format!("{}{r1}", col(k))).collect::<Vec<_>>().join("+"),
        9 => format!("LET(x,A{r1}*2,y,B{r1}+x,x*y)"),
        10 => format!("ROUND(AVERAGE(B{r1}:F{r1})*(1+$H$1)^2,2)"),
        _ => format!("COUNTIFS(Data!$A:$A,A{r1},Data!$C:$C,\">=\"&$G$2)"),
    }
}

fn range(r0: u32, c0: u32, r1: u32, c1: u32) -> RangeRef {
    RangeRef::new(CellRef::new(r0, c0), CellRef::new(r1, c1))
}

/// Every edit kind, at the sheet's start, middle and edges, with counts that run past the end.
fn edits() -> Vec<Edit> {
    let mut v = Vec::new();
    for axis in [Axis::Rows, Axis::Cols] {
        let max = if axis == Axis::Rows { MAX_ROWS } else { MAX_COLS };
        for at in [0, 1, 2, 3, 9, max - 2, max - 1] {
            for count in [0, 1, 2, 5, max - 1, max, u32::MAX] {
                v.push(Edit::Insert { axis, at, count });
                v.push(Edit::Delete { axis, at, count });
            }
        }
        for r in [
            range(1, 1, 2, 2),
            range(0, 0, 0, 0),
            range(1, 0, 1, MAX_COLS - 1),
            range(0, 1, MAX_ROWS - 1, 1),
            range(MAX_ROWS - 2, MAX_COLS - 2, MAX_ROWS - 1, MAX_COLS - 1),
        ] {
            v.push(Edit::InsertCells { axis, range: r });
            v.push(Edit::DeleteCells { axis, range: r });
        }
    }
    let blocks = [
        range(1, 1, 2, 2),
        range(0, 0, 0, 0),
        range(0, 0, MAX_ROWS - 1, MAX_COLS - 1),
        range(0, 0, MAX_ROWS - 1, 2),
        range(1, 0, 4, MAX_COLS - 1),
        range(0, 0, 20, 10),
    ];
    let targets = [(9, 3), (0, 0), (1, 1), (MAX_ROWS - 1, MAX_COLS - 1), (MAX_ROWS - 2, 0)];
    for from in blocks {
        for (to_row, to_col) in targets {
            v.push(Edit::Move { from, to_row, to_col });
            for to_sheet in ["Dest", "S", "s", "Other"] {
                v.push(Edit::MoveToSheet { from, to_sheet: to_sheet.to_string(), to_row, to_col });
            }
        }
    }
    v
}

/// Compares the new and the old adjustment of `src` for every host and edit; returns the
/// number of (formula, host, edit) cases checked, or 0 when `src` does not parse.
fn check(src: &str, edits: &[Edit]) -> usize {
    let Ok(e) = parse(src) else { return 0 };
    let mut n = 0;
    for host in ["S", "s", "Other", "My Sheet"] {
        for edit in edits {
            let want = old::adjust(e.clone(), host, "S", edit);
            let got = adjust(e.clone(), host, "S", edit);
            assert!(got == want, "{src} on {host} after {edit:?}: {} vs {}", print(&got), print(&want));
            assert_eq!(print(&got), print(&want), "{src} on {host} after {edit:?}");
            match adjust_if_changed(&e, host, "S", edit) {
                None => assert!(want == e, "{src} on {host} after {edit:?}: reported unchanged, old gives {}", print(&want)),
                Some(x) => {
                    assert!(want != e, "{src} on {host} after {edit:?}: reported changed but the old code keeps it");
                    assert!(x == want, "{src} on {host} after {edit:?}: {} vs {}", print(&x), print(&want));
                }
            }
            n += 1;
        }
    }
    for (dr, dc) in [(0, 0), (1, 0), (0, 1), (-1, -1), (5, 3), (-3, 2), (MAX_ROWS as i64, 0), (0, -(MAX_COLS as i64)), (1_048_575, 16_383)] {
        let want = old::shift_relative(e.clone(), dr, dc);
        let got = shift_relative(e.clone(), dr, dc);
        assert!(got == want, "{src} shifted by ({dr}, {dc}): {} vs {}", print(&got), print(&want));
    }
    n
}

#[test]
fn adjust_matches_the_previous_implementation_for_every_edit_kind() {
    let edits = edits();
    let mut cases = 0;
    for src in FORMULAS {
        let n = check(src, &edits);
        assert!(n > 0, "{src} does not parse");
        cases += n;
    }
    for i in 0..48 {
        // Rows 1-2 and 1,048,565-1,048,566 of the mix (its longest range reaches 9 rows further
        // down, to the last row): near both edges of the sheet.
        let row = if i < 24 { (i / 12) as u32 } else { MAX_ROWS - 12 + (i / 12 - 2) as u32 };
        let src = mix(i, row);
        let n = check(&src, &edits);
        assert!(n > 0, "{src} does not parse");
        cases += n;
    }
    assert!(cases > 100_000, "{cases}");
}

/// A small deterministic generator: a failing case is reproduced by rerunning the test, and
/// its message names the case's index in the stream.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn line(&mut self, max: u32) -> u32 {
        match self.below(3) {
            0 => self.below(12) as u32,
            1 => max - 1 - self.below(12) as u32,
            _ => self.below(max as u64) as u32,
        }
    }
}

fn random_ref(rng: &mut Rng) -> String {
    let sheet = ["", "", "S!", "s!", "Other!", "'My Sheet'!", "S:Other!"][rng.below(7) as usize];
    let dollar = |rng: &mut Rng| if rng.below(3) == 0 { "$" } else { "" };
    let cell = |rng: &mut Rng| format!("{}{}{}{}", dollar(rng), col(rng.line(MAX_COLS)), dollar(rng), rng.line(MAX_ROWS) + 1);
    let body = match rng.below(4) {
        0 => cell(rng),
        1 => format!("{}:{}", cell(rng), cell(rng)),
        2 => {
            let (a, b) = (rng.line(MAX_ROWS) + 1, rng.line(MAX_ROWS) + 1);
            format!("{}{a}:{}{b}", dollar(rng), dollar(rng))
        }
        _ => {
            let (a, b) = (rng.line(MAX_COLS), rng.line(MAX_COLS));
            format!("{}{}:{}{}", dollar(rng), col(a), dollar(rng), col(b))
        }
    };
    format!("{sheet}{body}")
}

fn random_edit(rng: &mut Rng) -> Edit {
    let axis = if rng.below(2) == 0 { Axis::Rows } else { Axis::Cols };
    let max = if axis == Axis::Rows { MAX_ROWS } else { MAX_COLS };
    let block = |rng: &mut Rng| {
        let (r0, r1) = (rng.line(MAX_ROWS), rng.line(MAX_ROWS));
        let (c0, c1) = (rng.line(MAX_COLS), rng.line(MAX_COLS));
        range(r0.min(r1), c0.min(c1), r0.max(r1), c0.max(c1))
    };
    let count = if rng.below(8) == 0 { rng.below(u32::MAX as u64) as u32 } else { rng.below(20) as u32 };
    match rng.below(7) {
        0 => Edit::Insert { axis, at: rng.line(max), count },
        1 => Edit::Delete { axis, at: rng.line(max), count },
        2 => Edit::InsertCells { axis, range: block(rng) },
        3 => Edit::DeleteCells { axis, range: block(rng) },
        4 => Edit::Move { from: block(rng), to_row: rng.line(MAX_ROWS), to_col: rng.line(MAX_COLS) },
        _ => Edit::MoveToSheet {
            from: block(rng),
            to_sheet: ["Dest", "S", "other"][rng.below(3) as usize].to_string(),
            to_row: rng.line(MAX_ROWS),
            to_col: rng.line(MAX_COLS),
        },
    }
}

/// Random references near the sheet's edges and random edits: "unchanged" is reported exactly
/// when the previous implementation leaves the formula as it was, and a reported change is the
/// old result.
#[test]
fn adjust_if_changed_reports_unchanged_exactly_when_nothing_moves() {
    let mut rng = Rng(0x5eed);
    let (mut unchanged, mut changed) = (0, 0);
    for case in 0..20_000 {
        let refs: Vec<String> = (0..1 + rng.below(4)).map(|_| random_ref(&mut rng)).collect();
        let src = format!("SUM({})", refs.join(","));
        let Ok(e) = parse(&src) else { panic!("case {case}: {src} does not parse") };
        let edit = random_edit(&mut rng);
        let host = ["S", "Other"][rng.below(2) as usize];
        let want = old::adjust(e.clone(), host, "S", &edit);
        assert!(adjust(e.clone(), host, "S", &edit) == want, "case {case}: {src} on {host} after {edit:?}");
        match adjust_if_changed(&e, host, "S", &edit) {
            None => {
                assert!(want == e, "case {case}: {src} on {host} after {edit:?}: reported unchanged, old gives {}", print(&want));
                unchanged += 1;
            }
            Some(x) => {
                assert!(want != e && x == want, "case {case}: {src} on {host} after {edit:?}");
                changed += 1;
            }
        }
    }
    // Both outcomes are exercised in quantity.
    assert!(unchanged > 2_000 && changed > 2_000, "unchanged {unchanged}, changed {changed}");
}

/// The whole formula corpus (one formula per line, with or without `=`) when
/// `GRIDCRAFT_ADJUST_CORPUS` names a file: `cargo test -p gridcraft-formula --test it -- --ignored adjust_corpus`.
#[test]
#[ignore = "needs GRIDCRAFT_ADJUST_CORPUS"]
fn adjust_corpus_matches_the_previous_implementation() {
    let Ok(path) = std::env::var("GRIDCRAFT_ADJUST_CORPUS") else { return };
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let edits = edits();
    let (mut parsed, mut skipped, mut cases) = (0, 0, 0);
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match check(line, &edits) {
            0 => skipped += 1,
            n => {
                parsed += 1;
                cases += n;
            }
        }
    }
    eprintln!("corpus: {parsed} formulas parsed, {skipped} skipped (do not parse), {cases} cases");
}

#[test]
fn unaffected_formula_is_returned_as_is() {
    let e: Expr = parse("Other!B2+SUM(A1:A3)").unwrap();
    let ins = Edit::Insert { axis: Axis::Rows, at: 5, count: 1 };
    // Row 6 inserted: A1:A3 lies above it and Other!B2 is not on S, so nothing moves, whichever
    // sheet the formula lives on (on Other, A1:A3 is on Other too).
    assert!(adjust_if_changed(&e, "S", "S", &ins).is_none());
    assert!(adjust_if_changed(&e, "Other", "S", &ins).is_none());
    // Row 1 inserted: A1:A3 on S moves, Other!B2 does not.
    let top = Edit::Insert { axis: Axis::Rows, at: 0, count: 1 };
    assert_eq!(adjust_if_changed(&e, "S", "S", &top).map(|x| print(&x)).as_deref(), Some("Other!B2+SUM(A2:A4)"));
    // A cut that moves a block onto the same place still qualifies its references with the sheet.
    let same_place = Edit::MoveToSheet { from: range(0, 0, 2, 0), to_sheet: "S".into(), to_row: 0, to_col: 0 };
    assert_eq!(adjust_if_changed(&e, "S", "S", &same_place).map(|x| print(&x)).as_deref(), Some("Other!B2+SUM(S!A1:A3)"));
}
