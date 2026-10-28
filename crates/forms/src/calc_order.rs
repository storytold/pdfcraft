//! The calculation order (`/CO`, ISO 32000-2 §12.7.3): the calculated fields in the order they
//! recalculate, the fields each one's formula reads, and what is wrong with that order.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

use pdfcraft_cos::{Document, ObjRef, Object};

use crate::{Field, FormError, acroform, fields};

/// At most this many reads between calculated fields are checked. A group name stands for every
/// field under it, so a form could otherwise make a check build billions of them.
const MAX_LINKS: usize = 1 << 20;

/// A calculated field, at its place in the calculation order.
#[derive(Clone, Debug, PartialEq)]
pub struct CalculatedField {
    /// The field's full name.
    pub name: String,
    /// The fields its formula reads, each once, as the formula names them: a name also stands for
    /// the fields under it. A script's names built at run time (`getField("row" + i)`) aren't here.
    pub reads: Vec<String>,
    /// `/CO` lists it. A field it leaves out calculates after the listed ones.
    pub listed: bool,
    /// What is wrong with its place in the order.
    pub issues: Vec<CalculationIssue>,
}

/// Something wrong with a calculated field's place in the calculation order.
#[derive(Clone, Debug, PartialEq)]
pub enum CalculationIssue {
    /// It reads `field`, which calculates after it, so it uses that field's previous value.
    RunsTooEarly { field: String },
    /// Its formula reads its own result: it reads `fields`, which read it back (itself, when it
    /// reads itself directly). No order is right for a loop: one of the formulas has to change.
    Cycle { fields: Vec<String> },
    /// `/CO` leaves it out. PdfCraft calculates it after the listed fields, but other PDF viewers
    /// may not calculate it at all.
    NotInOrder,
}

/// A form's calculation order and what is wrong with it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CalculationOrder {
    /// The calculated fields in the order [`crate::recalculate`] runs them: as `/CO` lists them,
    /// then the ones it leaves out, in field order.
    pub fields: Vec<CalculatedField>,
    /// `/CO` entries that calculate nothing: a field without a calculation, a second entry for a
    /// field, or an object that isn't a field (written as `12 0 R`).
    pub extra: Vec<String>,
    /// Fix order's order, as indexes into `fields`.
    fixed: Vec<usize>,
}

impl CalculationOrder {
    /// The names in the order Fix order writes: each field after the calculated fields it reads,
    /// keeping the current order wherever that allows. Reads within a loop are left as they are.
    pub fn fixed(&self) -> Vec<String> {
        self.fixed.iter().filter_map(|&i| self.fields.get(i)).map(|f| f.name.clone()).collect()
    }

    /// Whether Fix order changes anything: a field moves, joins `/CO`, or an extra entry goes.
    pub fn fixable(&self) -> bool {
        !self.extra.is_empty() || self.fields.iter().any(|f| !f.listed) || self.fixed.iter().enumerate().any(|(k, &i)| k != i)
    }
}

/// Whether a field has a calculation: a Calculate tab formula or a custom Calculate script.
pub(crate) fn is_calculated(f: &Field) -> bool {
    f.actions.calculate != crate::af::Calculate::None || f.actions.scripts.calculate.is_some()
}

/// The entries of `/CO`, as written.
pub(crate) fn co_entries(doc: &Document) -> Vec<Object> {
    acroform(doc).and_then(|af| af.get(b"CO").map(|c| doc.resolve(c)).and_then(|c| c.as_array().cloned())).unwrap_or_default()
}

/// Keep `/CO` in step with whether field `obj` calculates: a field that gets a calculation joins
/// the end of the order, one that loses it leaves, and one whose calculation changes keeps its
/// place, so editing a formula doesn't undo an order set by hand.
pub(crate) fn keep_listed(doc: &mut Document, obj: ObjRef, calculates: bool) -> Result<(), FormError> {
    let mut co = co_entries(doc);
    match (co.iter().any(|o| o.as_ref() == Some(obj)), calculates) {
        (false, true) => co.push(Object::Ref(obj)),
        (true, false) => co.retain(|o| o.as_ref() != Some(obj)),
        _ => return Ok(()),
    }
    write_co(doc, co)
}

/// Write `/CO` (none when `co` is empty), creating the form if needed.
fn write_co(doc: &mut Document, co: Vec<Object>) -> Result<(), FormError> {
    let af_ref = crate::author::ensure_form(doc)?;
    doc.update_dict(af_ref, |d| {
        if co.is_empty() {
            d.remove(b"CO");
        } else {
            d.set(b"CO".to_vec(), Object::Array(co));
        }
    })?;
    Ok(())
}

/// The calculated fields in the order they recalculate, as indexes into `fields` paired with
/// whether `/CO` lists them, and the `/CO` entries that calculate nothing.
pub(crate) fn run_order(doc: &Document, fields: &[Field]) -> (Vec<(usize, bool)>, Vec<String>) {
    let mut by_obj: HashMap<ObjRef, usize> = HashMap::with_capacity(fields.len());
    for (i, f) in fields.iter().enumerate() {
        by_obj.entry(f.obj).or_insert(i);
    }
    let mut placed = HashSet::new();
    let (mut run, mut extra) = (Vec::new(), Vec::new());
    for entry in co_entries(doc) {
        let field = entry.as_ref().and_then(|r| by_obj.get(&r).copied()).and_then(|i| Some((i, fields.get(i)?)));
        match field {
            Some((i, f)) if is_calculated(f) && placed.insert(i) => run.push((i, true)),
            Some((_, f)) => extra.push(f.name.clone()),
            None => extra.push(entry.as_ref().map_or_else(|| "not a reference".to_owned(), |r| r.to_string())),
        }
    }
    run.extend(fields.iter().enumerate().filter(|(i, f)| is_calculated(f) && !placed.contains(i)).map(|(i, _)| (i, false)));
    (run, extra)
}

/// The form's calculation order (`fields` as [`fields`] reads them), with each calculated field's
/// reads and issues, and Fix order's order.
pub fn calculation_order(doc: &Document, fields: &[Field]) -> CalculationOrder {
    let (run, extra) = run_order(doc, fields);
    let calculated: Vec<(&Field, bool)> = run.iter().filter_map(|&(i, listed)| Some((fields.get(i)?, listed))).collect();
    let reads: Vec<Vec<String>> = calculated
        .iter()
        .map(|(f, _)| {
            let mut names = f.actions.calculate.reads();
            if let Some(js) = &f.actions.scripts.calculate {
                for name in crate::af::calculation_reads(js) {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }
            names
        })
        .collect();
    let links = links(calculated.iter().map(|(f, _)| f.name.as_str()), &reads);
    let component = components(&links);
    let name = |k: usize| calculated.get(k).map(|(f, _)| f.name.clone()).unwrap_or_default();
    let mut out = Vec::with_capacity(calculated.len());
    for (k, ((f, listed), reads)) in calculated.iter().zip(reads).enumerate() {
        let own = component.get(k).copied();
        let mut issues = Vec::new();
        let read = links.get(k).map(Vec::as_slice).unwrap_or_default();
        for &j in read {
            if j > k && component.get(j).copied() != own {
                issues.push(CalculationIssue::RunsTooEarly { field: name(j) });
            }
        }
        // Only the loop's fields it reads itself are named, so a loop through thousands of fields
        // isn't listed again for each of them.
        let looped: Vec<String> = read.iter().filter(|&&j| component.get(j).copied() == own).map(|&j| name(j)).collect();
        if !looped.is_empty() {
            issues.push(CalculationIssue::Cycle { fields: looped });
        }
        if !listed {
            issues.push(CalculationIssue::NotInOrder);
        }
        out.push(CalculatedField { name: f.name.clone(), reads, listed: *listed, issues });
    }
    let fixed = fixed_order(&links, &component);
    CalculationOrder { fields: out, extra, fixed }
}

/// For each calculated field (by its place in `names`), the places of the calculated fields it
/// reads, sorted. A read name finds the field of that name and the fields under it, as
/// [`crate::recalculate`] looks values up.
fn links<'a>(names: impl Iterator<Item = &'a str>, reads: &[Vec<String>]) -> Vec<Vec<usize>> {
    let mut sorted: Vec<(&str, usize)> = names.enumerate().map(|(k, n)| (n, k)).collect();
    sorted.sort_unstable();
    let mut budget = MAX_LINKS;
    reads
        .iter()
        .map(|names| {
            let mut out = Vec::new();
            for name in names {
                let prefix = format!("{name}.");
                let exact = sorted.partition_point(|(n, _)| *n < name.as_str());
                let under = sorted.partition_point(|(n, _)| *n < prefix.as_str());
                let same = sorted.get(exact..).unwrap_or_default().iter().take_while(|(n, _)| n == name);
                let below = sorted.get(under..).unwrap_or_default().iter().take_while(|(n, _)| n.starts_with(&prefix));
                for &(_, k) in same.chain(below) {
                    if budget == 0 {
                        break;
                    }
                    budget -= 1;
                    out.push(k);
                }
            }
            out.sort_unstable();
            out.dedup();
            out
        })
        .collect()
}

/// The strongly connected components of `links` (Tarjan's algorithm, without recursion): one
/// component number per node. Fields in one component read each other in a loop.
fn components(links: &[Vec<usize>]) -> Vec<usize> {
    const UNSEEN: usize = usize::MAX;
    let n = links.len();
    let (mut index, mut low, mut component) = (vec![UNSEEN; n], vec![0; n], vec![UNSEEN; n]);
    let mut on_stack = vec![false; n];
    let (mut stack, mut work): (Vec<usize>, Vec<(usize, usize)>) = (Vec::new(), Vec::new());
    let (mut next, mut count) = (0, 0);
    // Every index below is a node: `links` only holds places below `n` (see `links`).
    for root in 0..n {
        if index[root] != UNSEEN {
            continue;
        }
        work.push((root, 0));
        while let Some((v, e)) = work.pop() {
            if e == 0 {
                index[v] = next;
                low[v] = next;
                next += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            if let Some(&w) = links[v].get(e) {
                work.push((v, e + 1));
                if index[w] == UNSEEN {
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            if low[v] == index[v] {
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    component[w] = count;
                    if w == v {
                        break;
                    }
                }
                count += 1;
            }
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[v]);
            }
        }
    }
    component
}

/// Fix order: every node after the nodes it reads in other components, the earliest ready node
/// first, so an order that is already right stays as it is and otherwise only what must move
/// moves. Reads within a component (a loop) are left out: no order satisfies them.
fn fixed_order(links: &[Vec<usize>], component: &[usize]) -> Vec<usize> {
    let n = links.len();
    let mut waits = vec![0usize; n];
    let mut readers: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (k, read) in links.iter().enumerate() {
        for &j in read {
            if component.get(j) != component.get(k)
                && let (Some(w), Some(r)) = (waits.get_mut(k), readers.get_mut(j))
            {
                *w += 1;
                r.push(k);
            }
        }
    }
    let mut ready: BinaryHeap<Reverse<usize>> = waits.iter().enumerate().filter(|(_, w)| **w == 0).map(|(k, _)| Reverse(k)).collect();
    let mut out = Vec::with_capacity(n);
    while let Some(Reverse(k)) = ready.pop() {
        out.push(k);
        for &r in readers.get(k).map(Vec::as_slice).unwrap_or_default() {
            if let Some(w) = waits.get_mut(r) {
                *w = w.saturating_sub(1);
                if *w == 0 {
                    ready.push(Reverse(r));
                }
            }
        }
    }
    // Without the reads inside loops no loop is left, so every node is placed; kept total anyway.
    if out.len() < n {
        let placed: HashSet<usize> = out.iter().copied().collect();
        out.extend((0..n).filter(|k| !placed.contains(k)));
    }
    out
}

/// Set the calculation order (Acrobat: Set Field Calculation Order): `names` calculate first, in
/// that order, then the calculated fields left out, in their current order. `/CO` is written
/// afresh, so it lists every calculated field once and nothing else.
///
/// Two calculated fields can share a full name in a malformed form, and the order then names
/// both: a repeated name stands for the next of them in the current order, so the current order
/// sets back unchanged.
pub fn set_calculation_order(doc: &mut Document, names: &[String]) -> Result<(), FormError> {
    let all = fields(doc);
    let (run, _) = run_order(doc, &all);
    let mut by_name: HashMap<&str, VecDeque<usize>> = HashMap::with_capacity(run.len());
    for &(i, _) in &run {
        if let Some(f) = all.get(i) {
            by_name.entry(f.name.as_str()).or_default().push_back(i);
        }
    }
    let mut chosen = HashSet::new();
    let mut co = Vec::with_capacity(names.len());
    for name in names {
        let Some(next) = by_name.get_mut(name.as_str()) else {
            return Err(if all.iter().any(|f| f.name == *name) {
                FormError::Invalid(format!("{name} has no calculation"))
            } else {
                FormError::NoSuchField(name.clone())
            });
        };
        let i = next.pop_front().ok_or_else(|| FormError::Invalid(format!("{name} is in the order twice")))?;
        if let Some(f) = all.get(i) {
            chosen.insert(i);
            co.push(Object::Ref(f.obj));
        }
    }
    co.extend(run.iter().filter(|(i, _)| !chosen.contains(i)).filter_map(|&(i, _)| all.get(i)).map(|f| Object::Ref(f.obj)));
    write_co(doc, co)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fix order on `links` (node `k` reads `links[k]`), as places.
    fn fix(links: &[Vec<usize>]) -> Vec<usize> {
        fixed_order(links, &components(links))
    }

    #[test]
    fn loops_are_found_without_recursion() {
        // 0 ↔ 1, 2 reads itself, 3 reads 0, 4 alone.
        let c = components(&[vec![1], vec![0], vec![2], vec![0], vec![]]);
        assert_eq!(c[0], c[1]);
        assert!(c[2] != c[0] && c[3] != c[0] && c[4] != c[0] && c[3] != c[4]);
        // A chain a million fields long is one pass, not a million nested calls.
        let n = 1_000_000;
        let chain: Vec<Vec<usize>> = (0..n).map(|k| if k + 1 < n { vec![k + 1] } else { vec![0] }).collect();
        let c = components(&chain);
        assert!(c.iter().all(|x| *x == c[0]), "one loop through every field");
    }

    #[test]
    fn fix_order_moves_only_what_must_move() {
        // Already right: unchanged.
        assert_eq!(fix(&[vec![], vec![0], vec![1], vec![]]), [0, 1, 2, 3]);
        // 0 reads 2: it moves after 2, the rest keep their order.
        assert_eq!(fix(&[vec![2], vec![], vec![], vec![]]), [1, 2, 0, 3]);
        // A chain written backwards comes out forwards.
        assert_eq!(fix(&[vec![1], vec![2], vec![]]), [2, 1, 0]);
        // A loop (0 ↔ 1) keeps its order; 2, which reads it, goes after it.
        assert_eq!(fix(&[vec![1], vec![0], vec![]]), [0, 1, 2]);
        assert_eq!(fix(&[vec![], vec![2], vec![1]]), [0, 1, 2]);
        assert_eq!(fix(&[vec![1, 2], vec![0], vec![]]), [1, 2, 0], "a loop member still waits for what it reads outside the loop");
        // Reading itself is a loop, not a reason to move.
        assert_eq!(fix(&[vec![0], vec![]]), [0, 1]);
    }

    #[test]
    fn a_name_finds_the_fields_under_it() {
        let names = ["row.1", "row", "row.10", "rows", "row-1", "total"];
        let reads = vec![vec![], vec![], vec![], vec![], vec![], vec!["row".to_owned(), "missing".to_owned(), "row".to_owned()]];
        assert_eq!(links(names.into_iter(), &reads)[5], [0, 1, 2], "row, row.1 and row.10; not rows or row-1");
    }

    /// Two thousand formulas each reading a group of two thousand fields would be four million
    /// reads; the checks stop at their cap.
    #[test]
    fn reads_of_a_large_group_are_capped() {
        let n = 2000;
        let names: Vec<String> = (0..n).map(|i| format!("g.{i}")).chain((0..n).map(|i| format!("r{i}"))).collect();
        let reads: Vec<Vec<String>> = (0..2 * n).map(|i| if i < n { Vec::new() } else { vec!["g".to_owned()] }).collect();
        let total: usize = links(names.iter().map(String::as_str), &reads).iter().map(Vec::len).sum();
        assert_eq!(total, MAX_LINKS);
    }

    proptest::proptest! {
        /// Fix order is a reordering of the fields after which none reads a field that calculates
        /// later, except within a loop; an order that is already right comes back unchanged.
        #[test]
        fn fix_order_leaves_nothing_too_early(links in proptest::collection::vec(proptest::collection::vec(0usize..12, 0..4), 1..12)) {
            let n = links.len();
            let links: Vec<Vec<usize>> = links.into_iter().map(|mut l| { l.retain(|j| *j < n); l.sort_unstable(); l.dedup(); l }).collect();
            let component = components(&links);
            // Components are exactly the nodes that reach each other.
            let reach = |from: usize| {
                let mut seen = vec![false; n];
                let mut todo = vec![from];
                while let Some(v) = todo.pop() {
                    for &w in &links[v] {
                        if !seen[w] { seen[w] = true; todo.push(w); }
                    }
                }
                seen
            };
            let reaches: Vec<Vec<bool>> = (0..n).map(reach).collect();
            for a in 0..n {
                for b in 0..n {
                    if a != b {
                        proptest::prop_assert_eq!(component[a] == component[b], reaches[a][b] && reaches[b][a]);
                    }
                }
            }
            let fixed = fixed_order(&links, &component);
            let mut sorted = fixed.clone();
            sorted.sort_unstable();
            proptest::prop_assert_eq!(sorted, (0..n).collect::<Vec<_>>(), "a reordering");
            let mut at = vec![0; n];
            for (place, &k) in fixed.iter().enumerate() {
                at[k] = place;
            }
            for (k, read) in links.iter().enumerate() {
                for &j in read {
                    proptest::prop_assert!(component[j] == component[k] || at[j] < at[k], "{k} reads {j}: {fixed:?}");
                }
            }
            // Fixed once, the order needs no more moves.
            let renumbered: Vec<Vec<usize>> = fixed.iter().map(|&k| { let mut l: Vec<usize> = links[k].iter().map(|&j| at[j]).collect(); l.sort_unstable(); l }).collect();
            proptest::prop_assert_eq!(fix(&renumbered), (0..n).collect::<Vec<_>>());
        }
    }
}
