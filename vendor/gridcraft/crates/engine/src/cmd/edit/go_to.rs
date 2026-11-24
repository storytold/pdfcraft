use super::selection::{select_region, selection_set};
use super::{CellRef, EngineError, Json, RangeRef, Result, Session, bad, bool_param, coalesce, json, split_sheet, str_param};

/// How many references the Go To dialog keeps for going back to.
pub(super) const GO_TO_RECENT: usize = 10;

/// The most areas one Go To selects.
pub(super) const GO_TO_MAX_AREAS: usize = 2000;

/// How deep a name that refers to other names is followed.
const GO_TO_NAME_DEPTH: usize = 8;

pub(in super::super) fn go_to(s: &mut Session, p: &Json) -> Result<Json> {
    if bool_param(p, "list") == Some(true) {
        let d = s.doc()?;
        return Ok(json!({"names": go_to_names(&d.wb, d.wb.active_sheet()), "recent": d.go_to_recent}));
    }
    let Some(reference) = str_param(p, "reference").or_else(|| str_param(p, "range")) else { return Err(bad("edit.goTo", "missing `reference`")) };
    let d = s.doc()?;
    let mut names = Resolver::new(&d.wb);
    let mut target: Option<usize> = None;
    let mut areas: Vec<RangeRef> = Vec::new();
    let mut named = false;
    for part in reference_parts(reference) {
        let mut found = Vec::new();
        let Some((sheet, _, by_name)) = names.target(d.wb.active_sheet(), part, 0, Some(&mut found)) else {
            return Err(EngineError::Other("Reference isn't valid.".into()));
        };
        // One selection holds the areas of one sheet.
        if target.is_some_and(|t| t != sheet) {
            return Err(EngineError::Other("Reference isn't valid.".into()));
        }
        target = Some(sheet);
        named |= by_name;
        areas.extend(found);
        if areas.len() > GO_TO_MAX_AREAS {
            return Err(EngineError::Other(format!("Go To selects at most {GO_TO_MAX_AREAS} areas.")));
        }
    }
    let Some(sheet) = target else { return Err(EngineError::Other("Reference isn't valid.".into())) };
    let range = areas.iter().map(RangeRef::a1).collect::<Vec<_>>().join(",");
    let out = selection_set(s, &json!({"sheet": sheet, "range": range}))?;
    if bool_param(p, "record") == Some(true) && !named {
        let d = s.doc_mut()?;
        let prefix = gridcraft_core::quote_sheet(&d.wb.sheet(sheet).map(|sh| sh.name.clone()).unwrap_or_default());
        let entry = areas.iter().map(|r| format!("{prefix}!{}", r.a1())).collect::<Vec<_>>().join(",");
        d.go_to_recent.retain(|r| !r.eq_ignore_ascii_case(&entry));
        d.go_to_recent.insert(0, entry);
        d.go_to_recent.truncate(GO_TO_RECENT);
    }
    Ok(out)
}

/// The parts of a Go To reference, split at the commas outside quoted sheet names and
/// parentheses: `Price, Type` or `B14:C22,F19:G30`. The parts are read one at a time, so a caller
/// that stops at an invalid part or at `GO_TO_MAX_AREAS` does not split the rest of the text.
pub(super) fn reference_parts(text: &str) -> impl Iterator<Item = &str> {
    let (mut depth, mut quoted, mut start, mut done) = (0usize, false, 0, false);
    let mut chars = text.char_indices();
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        for (i, c) in chars.by_ref() {
            match c {
                '\'' => quoted = !quoted,
                '(' if !quoted => depth += 1,
                ')' if !quoted => depth = depth.saturating_sub(1),
                ',' if !quoted && depth == 0 => {
                    let part = text.get(start..i).unwrap_or("").trim();
                    start = i + 1;
                    return Some(part);
                }
                _ => {}
            }
        }
        done = true;
        Some(text.get(start..).unwrap_or("").trim())
    })
}

/// Resolves Go To references against one workbook. Defined names are found through an index
/// built once, and whether a name refers to a reference is remembered: listing the names of a
/// workbook full of names that alias other names would otherwise scan every name for every alias
/// of every name.
struct Resolver<'a> {
    wb: &'a gridcraft_model::Workbook,
    /// Indexes into `wb.names` by scope and folded name, in workbook order.
    by_name: std::collections::HashMap<(Option<usize>, String), Vec<usize>>,
    /// [`Resolver::name_areas`] without areas, by name index, sheet and depth: the target sheet
    /// and the number of areas.
    memo: std::collections::HashMap<(usize, usize, usize), Option<(usize, usize)>>,
}

impl<'a> Resolver<'a> {
    fn new(wb: &'a gridcraft_model::Workbook) -> Self {
        let mut by_name: std::collections::HashMap<(Option<usize>, String), Vec<usize>> = std::collections::HashMap::new();
        for (i, n) in wb.names.iter().enumerate() {
            by_name.entry((n.scope, gridcraft_formula::fold_name(&n.name))).or_default().push(i);
        }
        Self { wb, by_name, memo: std::collections::HashMap::new() }
    }

    /// The first name of scope `scope` that `same` accepts, as a scan of `wb.names` finds it.
    /// Names `same` accepts fold alike, so only that bucket is checked.
    fn find(&self, scope: Option<usize>, body: &str, same: impl Fn(&str, &str) -> bool) -> Option<usize> {
        self.by_name
            .get(&(scope, gridcraft_formula::fold_name(body)))?
            .iter()
            .copied()
            .find(|&i| self.wb.names.get(i).is_some_and(|n| same(&n.name, body)))
    }

    /// Where one part of a Go To reference points, seen from sheet `sheet`: the sheet, how many
    /// areas (appended to `areas` when given), and whether a defined name or a table gave them.
    /// `Sheet2!Name` is the name scoped to Sheet2. A name counts when what it refers to is a
    /// reference; a constant or a formula is not one.
    fn target(&mut self, sheet: usize, part: &str, depth: usize, areas: Option<&mut Vec<RangeRef>>) -> Option<(usize, usize, bool)> {
        let wb = self.wb;
        if part.is_empty() || depth > GO_TO_NAME_DEPTH {
            return None;
        }
        let (prefix, body) = split_sheet(part);
        let on = match &prefix {
            Some(n) => wb.sheet_index(n)?,
            None => sheet,
        };
        if let Some(r) = RangeRef::parse(&body.replace('$', "")) {
            if let Some(a) = areas {
                a.push(r);
            }
            return Some((on, 1, false));
        }
        // As `Workbook::name`: the sheet's own name, else the workbook's.
        let name = match prefix {
            Some(_) => self.find(Some(on), body, gridcraft_formula::same_name),
            None => self.find(Some(sheet), body, str::eq_ignore_ascii_case).or_else(|| self.find(None, body, str::eq_ignore_ascii_case)),
        };
        if let Some(i) = name {
            return self.name_areas(i, sheet, depth, areas).map(|(t, n)| (t, n, true));
        }
        if prefix.is_none()
            && let Some((si, ti)) = wb.table(body)
        {
            // As in Excel, a table's name stands for its data, without the header and totals rows.
            let t = wb.sheet(si)?.tables.get(ti)?;
            if let Some(a) = areas {
                a.push(t.data_range().unwrap_or(t.range));
            }
            return Some((si, 1, true));
        }
        None
    }

    /// The sheet and number of areas defined name `wb.names[i]` refers to, seen from sheet
    /// `sheet`, when that is a reference; the areas are appended to `areas` when given.
    fn name_areas(&mut self, i: usize, sheet: usize, depth: usize, mut areas: Option<&mut Vec<RangeRef>>) -> Option<(usize, usize)> {
        let key = (i, sheet, depth);
        if areas.is_none()
            && let Some(known) = self.memo.get(&key)
        {
            return *known;
        }
        let out = self.name_areas_uncached(i, sheet, depth, areas.as_deref_mut());
        if areas.is_none() {
            self.memo.insert(key, out);
        }
        out
    }

    fn name_areas_uncached(&mut self, i: usize, sheet: usize, depth: usize, mut areas: Option<&mut Vec<RangeRef>>) -> Option<(usize, usize)> {
        let n = self.wb.names.get(i)?;
        let from = n.scope.unwrap_or(sheet);
        let formula = n.formula.trim().trim_start_matches('=');
        let mut target: Option<usize> = None;
        let mut count = 0usize;
        for p in reference_parts(formula) {
            let (s, found, _) = self.target(from, p, depth + 1, areas.as_deref_mut())?;
            if target.is_some_and(|t| t != s) {
                return None;
            }
            target = Some(s);
            count += found;
            if count > GO_TO_MAX_AREAS {
                return None;
            }
        }
        target.map(|t| (t, count))
    }
}

/// The names the Go To dialog lists, seen from sheet `sheet`: the named ranges of the workbook and
/// of every sheet, in alphabetical order. A name scoped to another sheet is written `Sheet2!Name`,
/// and the active sheet's own name hides the workbook name it shares. Hidden names and names that
/// do not refer to a range are left out.
pub fn go_to_names(wb: &gridcraft_model::Workbook, sheet: usize) -> Vec<String> {
    let own: std::collections::HashSet<String> =
        wb.names.iter().filter(|n| n.scope == Some(sheet)).map(|n| gridcraft_formula::fold_name(&n.name)).collect();
    let mut names = Resolver::new(wb);
    let mut out: Vec<String> = wb
        .names
        .iter()
        .enumerate()
        .filter(|(i, n)| !n.hidden && names.name_areas(*i, sheet, 0, None).is_some())
        .filter_map(|(_, n)| match n.scope {
            None if own.contains(&gridcraft_formula::fold_name(&n.name)) => None,
            None => Some(n.name.clone()),
            Some(s) if s == sheet => Some(n.name.clone()),
            Some(s) => Some(format!("{}!{}", gridcraft_core::quote_sheet(&wb.sheet(s)?.name), n.name)),
        })
        .collect();
    out.sort_by_cached_key(|n| gridcraft_formula::fold_name(n));
    out.dedup_by(|a, b| gridcraft_formula::same_name(a, b));
    out
}

pub(super) fn go_to_special(s: &mut Session, p: &Json) -> Result<Json> {
    let kind = str_param(p, "kind").unwrap_or("blanks");
    let d = s.doc()?;
    let Some(sheet) = d.wb.active() else { return Err(EngineError::NoDocument) };
    let scope = if d.selection.is_single_cell() { sheet.used_range().unwrap_or(RangeRef::cell(d.selection.active)) } else { d.selection.bounds() };
    if kind == "lastCell" {
        let c = sheet.used_range().map(|r| r.end).unwrap_or_default();
        return selection_set(s, &json!({"cell": c.a1()}));
    }
    if kind == "currentRegion" {
        return select_region(s, &json!({}));
    }
    let mut hits: Vec<CellRef> = Vec::new();
    if scope.count() > 4_000_000 {
        return Err(bad("edit.goToSpecial", "range too large"));
    }
    for c in scope.iter() {
        let cell = sheet.cell(c);
        let v = sheet.value(c);
        let ok = match kind {
            "blanks" => v.is_empty(),
            "constants" => cell.is_some_and(|x| x.formula.is_none() && x.py.is_none() && !x.value.is_empty()),
            "formulas" => cell.is_some_and(|x| x.formula.is_some() || x.py.is_some()),
            "comments" | "notes" => sheet.comments.contains_key(&c),
            "errors" => v.is_error(),
            "numbers" => v.is_number(),
            "text" => v.is_text(),
            "visible" => !sheet.is_row_hidden(c.row) && !sheet.is_col_hidden(c.col),
            "conditionalFormats" => sheet.cond_formats.iter().any(|cf| cf.ranges.iter().any(|r| r.contains(c))),
            "dataValidation" => sheet.validations.iter().any(|dv| dv.ranges.iter().any(|r| r.contains(c))),
            _ => return Err(bad("edit.goToSpecial", format!("unknown kind `{kind}`"))),
        };
        if ok {
            hits.push(c);
        }
    }
    if hits.is_empty() {
        return Err(EngineError::Other("No cells were found.".into()));
    }
    let ranges = coalesce(&hits);
    let d = s.doc_mut()?;
    d.selection = crate::Selection { active: hits[0], anchor: hits[0], ranges };
    Ok(json!({"selection": d.selection.a1(), "count": hits.len()}))
}
