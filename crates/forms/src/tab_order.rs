use super::read::{fields, page_refs};
use super::{Document, Field, FormError, ObjRef, Object, invalid};

/// A page's tab order (`/Tabs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabOrder {
    /// Rows, top to bottom, left to right within a row (`/R`).
    Row,
    /// Columns, left to right, top to bottom within a column (`/C`).
    Column,
    /// The document structure (`/S`; annotation order when the file has none).
    Structure,
    /// Unspecified: annotation order.
    Annotations,
}

impl TabOrder {
    fn of(name: Option<&[u8]>) -> TabOrder {
        match name {
            Some(b"R") => TabOrder::Row,
            Some(b"C") => TabOrder::Column,
            Some(b"S") => TabOrder::Structure,
            _ => TabOrder::Annotations,
        }
    }
}

/// A widget on a page for tab ordering: (page, field, widget, rect, annotation index).
type TabItem = (usize, usize, usize, [f64; 4], usize);

/// Every widget's place in the tab order.
pub(crate) fn rank_tabs(doc: &Document, pages: &[ObjRef], annot_index: &std::collections::HashMap<ObjRef, usize>, out: &mut [Field]) {
    let mut items: Vec<TabItem> = Vec::new();
    for (fi, f) in out.iter().enumerate() {
        for (wi, w) in f.widgets.iter().enumerate() {
            if let Some(p) = w.page {
                items.push((p, fi, wi, w.rect, annot_index.get(&w.obj).copied().unwrap_or(usize::MAX)));
            }
        }
    }
    let mut ordered: Vec<(usize, usize)> = Vec::with_capacity(items.len());
    for (pi, p) in pages.iter().enumerate() {
        let mut on: Vec<_> = items.iter().filter(|i| i.0 == pi).collect();
        if on.is_empty() {
            continue;
        }
        let tabs = TabOrder::of(doc.get(*p).as_dict().and_then(|d| d.name(b"Tabs")));
        match tabs {
            TabOrder::Row | TabOrder::Column => {
                let row = tabs == TabOrder::Row;
                // Primary axis: rows by top edge (down the page), columns by left edge.
                on.sort_by(|a, b| if row { b.3[3].total_cmp(&a.3[3]) } else { a.3[0].total_cmp(&b.3[0]) });
                let mut groups: Vec<Vec<&TabItem>> = Vec::new();
                for it in on {
                    let fits = groups.last().is_some_and(|g| {
                        let first = g[0];
                        if row {
                            (first.3[3] - it.3[3]).abs() < ((first.3[3] - first.3[1]) / 2.0).max(2.0)
                        } else {
                            (it.3[0] - first.3[0]).abs() < ((first.3[2] - first.3[0]) / 2.0).max(2.0)
                        }
                    });
                    if fits && let Some(g) = groups.last_mut() {
                        g.push(it);
                    } else {
                        groups.push(vec![it]);
                    }
                }
                for mut g in groups {
                    g.sort_by(|a, b| if row { a.3[0].total_cmp(&b.3[0]) } else { b.3[3].total_cmp(&a.3[3]) });
                    ordered.extend(g.iter().map(|i| (i.1, i.2)));
                }
            }
            TabOrder::Structure | TabOrder::Annotations => {
                on.sort_by_key(|i| i.4);
                ordered.extend(on.iter().map(|i| (i.1, i.2)));
            }
        }
    }
    for f in out.iter_mut() {
        for w in &mut f.widgets {
            w.tab = usize::MAX;
        }
    }
    for (rank, (fi, wi)) in ordered.into_iter().enumerate() {
        out[fi].widgets[wi].tab = rank;
    }
}

/// Set the tab order of pages (0-based).
pub fn set_tab_order(doc: &mut Document, pages: &[usize], order: TabOrder) -> Result<(), FormError> {
    let refs = page_refs(doc);
    for &p in pages {
        let r = *refs.get(p).ok_or_else(|| FormError::Invalid(format!("page {} does not exist", p + 1)))?;
        doc.update_dict(r, |d| match order {
            TabOrder::Row => d.set(b"Tabs".to_vec(), Object::name("R")),
            TabOrder::Column => d.set(b"Tabs".to_vec(), Object::name("C")),
            TabOrder::Structure => d.set(b"Tabs".to_vec(), Object::name("S")),
            TabOrder::Annotations => {
                d.remove(b"Tabs");
            }
        })?;
    }
    Ok(())
}

/// Order tabs manually (Acrobat: Fields ▸ Tab Order ▸ Order Tabs Manually, then drag a field):
/// move field `name` one place earlier or later in its page's tab order. The page switches to
/// annotation order (`/Tabs` removed) and its widgets are reordered in `/Annots`; other
/// annotations keep their places.
pub fn move_in_tab_order(doc: &mut Document, name: &str, earlier: bool) -> Result<(), FormError> {
    let all = fields(doc);
    let f = all.iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?;
    let page = f.widgets.iter().filter_map(|w| w.page).min().ok_or_else(|| FormError::Invalid(format!("{name} is not on a page")))?;
    // The page's fields in their current tab order (a field counts once, at its first widget).
    let mut order: Vec<(usize, &str)> =
        all.iter().filter_map(|g| g.widgets.iter().filter(|w| w.page == Some(page)).map(|w| w.tab).min().map(|t| (t, g.name.as_str()))).collect();
    order.sort();
    let names: Vec<&str> = order.iter().map(|(_, n)| *n).collect();
    let Some(i) = names.iter().position(|n| *n == name) else { return invalid(format!("{name} is not on a page")) };
    let j = if earlier { i.checked_sub(1) } else { Some(i + 1).filter(|j| *j < names.len()) };
    let Some(j) = j else { return Ok(()) };
    let mut names = names.into_iter().map(str::to_string).collect::<Vec<_>>();
    names.swap(i, j);
    // The widgets of this page in the new order.
    let mut widgets: Vec<ObjRef> = Vec::new();
    for n in &names {
        if let Some(g) = all.iter().find(|g| &g.name == n) {
            widgets.extend(g.widgets.iter().filter(|w| w.page == Some(page)).map(|w| w.obj));
        }
    }
    let pr = *page_refs(doc).get(page).ok_or_else(|| FormError::Invalid("no such page".into()))?;
    let annots_obj =
        doc.get(pr).as_dict().and_then(|d| d.get(b"Annots").cloned()).ok_or_else(|| FormError::Invalid("the page has no annotations".into()))?;
    let list = doc.resolve(&annots_obj).as_array().cloned().unwrap_or_default();
    let mut next = widgets.into_iter();
    let set: std::collections::HashSet<ObjRef> = all.iter().flat_map(|g| g.widgets.iter().filter(|w| w.page == Some(page)).map(|w| w.obj)).collect();
    let new_list: Vec<Object> = list
        .iter()
        .map(|o| match o.as_ref() {
            Some(r) if set.contains(&r) => next.next().map(Object::Ref).unwrap_or_else(|| o.clone()),
            _ => o.clone(),
        })
        .collect();
    match annots_obj {
        Object::Ref(ar) => doc.set(ar, Object::Array(new_list)),
        _ => doc.update_dict(pr, |d| d.set(b"Annots".to_vec(), Object::Array(new_list)))?,
    }
    doc.update_dict(pr, |d| {
        d.remove(b"Tabs");
    })?;
    Ok(())
}
