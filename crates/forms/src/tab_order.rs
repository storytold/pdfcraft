use super::read::{fields, page_refs};
use super::{Document, Field, FormError, ObjRef, Object, invalid};

/// A page's tab order (`/Tabs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabOrder {
    /// Rows, top to bottom, left to right within a row (`/R`; right to left in a document bound
    /// on the right).
    Row,
    /// Columns, left to right, top to bottom within a column (`/C`; columns right to left in a
    /// document bound on the right).
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

    /// The tab order of the page at `r`, with an indirect `/Tabs` resolved.
    fn of_page(doc: &Document, r: ObjRef) -> TabOrder {
        let tabs = doc.get(r).as_dict().and_then(|d| d.get(b"Tabs")).map(|o| doc.resolve(o));
        TabOrder::of(tabs.as_deref().and_then(Object::as_name))
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
    // Rows and columns run right to left in a document bound on the right: rank the mirrored
    // rectangles there.
    if right_to_left(doc) {
        for i in &mut items {
            i.3 = [-i.3[2], i.3[1], -i.3[0], i.3[3]];
        }
    }
    let mut ordered: Vec<(usize, usize)> = Vec::with_capacity(items.len());
    for (pi, p) in pages.iter().enumerate() {
        let mut on: Vec<_> = items.iter().filter(|i| i.0 == pi).collect();
        if on.is_empty() {
            continue;
        }
        let tabs = TabOrder::of_page(doc, *p);
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

/// Whether the document is bound on the right (`/ViewerPreferences /Direction /R2L`).
fn right_to_left(doc: &Document) -> bool {
    let Some(root) = doc.root() else { return false };
    let Some(prefs) = doc.get(root).as_dict().and_then(|c| c.get(b"ViewerPreferences")).map(|o| doc.resolve(o)) else { return false };
    prefs.as_dict().and_then(|p| p.get(b"Direction")).map(|o| doc.resolve(o)).is_some_and(|d| d.as_name() == Some(b"R2L"))
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

/// The tab order of each page, in page order (`/Tabs`; a page without one is unspecified).
pub fn tab_orders(doc: &Document) -> Vec<TabOrder> {
    page_refs(doc).into_iter().map(|r| TabOrder::of_page(doc, r)).collect()
}

/// Order tabs manually (Acrobat: Fields ▸ Tab Order ▸ Order Tabs Manually, then drag a field):
/// move field `name` one place earlier or later in its page's tab order. The page switches to
/// annotation order (`/Tabs` removed) and its widgets are reordered in `/Annots`; other
/// annotations keep their places.
pub fn move_in_tab_order(doc: &mut Document, name: &str, earlier: bool) -> Result<(), FormError> {
    let all = fields(doc);
    if !all.iter().any(|f| f.name == name) {
        return Err(FormError::NoSuchField(name.into()));
    }
    let (target_fi, page) = all
        .iter()
        .enumerate()
        .filter(|(_, f)| f.name == name)
        .find_map(|(fi, f)| Some((fi, f.widgets.iter().filter_map(|w| w.page).min()?)))
        .ok_or_else(|| FormError::Invalid(format!("{name} is not on a page")))?;
    // The page's fields in their current tab order (a field counts once, at its first widget).
    let mut order: Vec<(usize, usize)> = all
        .iter()
        .enumerate()
        .filter_map(|(fi, g)| g.widgets.iter().filter(|w| w.page == Some(page)).map(|w| w.tab).min().map(|t| (t, fi)))
        .collect();
    order.sort();
    let Some(i) = order.iter().position(|&(_, fi)| fi == target_fi) else { return invalid(format!("{name} is not on a page")) };
    let j = if earlier { i.checked_sub(1) } else { Some(i + 1).filter(|j| *j < order.len()) };
    let Some(j) = j else { return Ok(()) };
    order.swap(i, j);
    // The widgets of this page in the new order, preserving each field's relative widget tab order.
    let mut widgets: Vec<ObjRef> = Vec::new();
    let mut queued = std::collections::HashSet::new();
    for &(_, fi) in &order {
        if let Some(g) = all.get(fi) {
            let mut own: Vec<(usize, ObjRef)> = g.widgets.iter().filter(|w| w.page == Some(page)).map(|w| (w.tab, w.obj)).collect();
            own.sort_by_key(|&(tab, _)| tab);
            widgets.extend(own.into_iter().map(|(_, obj)| obj).filter(|obj| queued.insert(*obj)));
        }
    }
    let pr = *page_refs(doc).get(page).ok_or_else(|| FormError::Invalid("no such page".into()))?;
    let annots_obj =
        doc.get(pr).as_dict().and_then(|d| d.get(b"Annots").cloned()).ok_or_else(|| FormError::Invalid("the page has no annotations".into()))?;
    let list = doc.resolve(&annots_obj).as_array().cloned().unwrap_or_default();
    let mut next = widgets.into_iter();
    let set: std::collections::HashSet<ObjRef> = all.iter().flat_map(|g| g.widgets.iter().filter(|w| w.page == Some(page)).map(|w| w.obj)).collect();
    let mut seen_slots = std::collections::HashSet::new();
    let new_list: Vec<Object> = list
        .iter()
        .map(|o| match o.as_ref() {
            Some(r) if set.contains(&r) && seen_slots.insert(r) => next.next().map(Object::Ref).unwrap_or_else(|| o.clone()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::{Dict, PdfString};

    #[test]
    fn move_in_tab_order_preserves_multi_widget_relative_tab_order() {
        let mut doc = Document::new_empty();
        let pages_ref = doc.add(Object::Null);
        let page_ref = doc.add(Object::Null);

        let mut w1 = Dict::new();
        w1.set(b"Subtype".to_vec(), Object::name("Widget"));
        w1.set(b"Rect".to_vec(), Object::Array(vec![Object::Int(200), Object::Int(700), Object::Int(220), Object::Int(720)]));
        let w1_ref = doc.add(Object::Dict(w1));

        let mut w2 = Dict::new();
        w2.set(b"Subtype".to_vec(), Object::name("Widget"));
        w2.set(b"Rect".to_vec(), Object::Array(vec![Object::Int(50), Object::Int(700), Object::Int(70), Object::Int(720)]));
        let w2_ref = doc.add(Object::Dict(w2));

        let mut radio = Dict::new();
        radio.set(b"FT".to_vec(), Object::name("Btn"));
        radio.set(b"Ff".to_vec(), Object::Int(1 << 15));
        radio.set(b"T".to_vec(), PdfString::text("choice"));
        // /Kids order is [w1 (right), w2 (left)], opposite to /R (Row) left-to-right order.
        radio.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(w1_ref), Object::Ref(w2_ref)]));
        let radio_ref = doc.add(Object::Dict(radio));
        doc.update_dict(w1_ref, |d| d.set(b"Parent".to_vec(), Object::Ref(radio_ref))).unwrap();
        doc.update_dict(w2_ref, |d| d.set(b"Parent".to_vec(), Object::Ref(radio_ref))).unwrap();

        let mut text = Dict::new();
        text.set(b"FT".to_vec(), Object::name("Tx"));
        text.set(b"Subtype".to_vec(), Object::name("Widget"));
        text.set(b"T".to_vec(), PdfString::text("note"));
        text.set(b"Rect".to_vec(), Object::Array(vec![Object::Int(50), Object::Int(600), Object::Int(150), Object::Int(620)]));
        let text_ref = doc.add(Object::Dict(text));

        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::name("Page"));
        page.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        page.set(b"Tabs".to_vec(), Object::name("R"));
        page.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(w1_ref), Object::Ref(w2_ref), Object::Ref(text_ref)]));
        doc.set(page_ref, Object::Dict(page));

        let mut pages = Dict::new();
        pages.set(b"Type".to_vec(), Object::name("Pages"));
        pages.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page_ref)]));
        pages.set(b"Count".to_vec(), Object::Int(1));
        doc.set(pages_ref, Object::Dict(pages));

        let mut af = Dict::new();
        af.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(radio_ref), Object::Ref(text_ref)]));
        let af_ref = doc.add(Object::Dict(af));

        let mut catalog = Dict::new();
        catalog.set(b"Type".to_vec(), Object::name("Catalog"));
        catalog.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        catalog.set(b"AcroForm".to_vec(), Object::Ref(af_ref));
        let root_ref = doc.root().unwrap();
        doc.set(root_ref, Object::Dict(catalog));

        // Move "note" earlier (before "choice"): "choice"'s own buttons should stay in their
        // /R tab order (w2 then w1), not revert to /Kids order (w1 then w2).
        move_in_tab_order(&mut doc, "note", true).unwrap();
        let annots = doc.get(page_ref).as_dict().and_then(|d| d.get(b"Annots")).and_then(Object::as_array).cloned().unwrap();
        assert_eq!(annots, vec![Object::Ref(text_ref), Object::Ref(w2_ref), Object::Ref(w1_ref)]);
    }
}
