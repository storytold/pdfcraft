//! The annotations a new layout does not generate: comments, markups and the links a user added.
//! Laying a form out again replaces its pages (ISO 32000-2 §7.7.3.2), and with them each page's
//! `/Annots` (§12.5.1); a layout writes only the widgets and the links of its rich text, so the
//! rest is taken off the old pages first and put back on the new ones.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use super::LINK_KEY;

/// Most pages read for annotations to carry over.
const MAX_PAGES: usize = 10_000;
/// Most page-tree nodes visited while finding them (intermediate nodes included).
const MAX_NODES: usize = 20_000;
/// Most levels of the page tree followed.
const MAX_DEPTH: usize = 64;
/// Most annotations carried over in one layout.
const MAX_CARRIED: usize = 100_000;
/// Most `/Annots` entries read in one layout, over all pages (pages may share one long array).
const MAX_READ: usize = 1_000_000;
/// Most entries in an explicit destination (`/FitR` has six, Table 149); a longer array is not one.
const MAX_DEST: usize = 6;

/// The annotations of the document's pages that a layout does not write, each with the index of
/// the page it is on.
pub(crate) struct Carried {
    annots: Vec<Annot>,
    /// Annotations past [`MAX_CARRIED`], not carried over (said in a warning).
    left: usize,
    /// `/Annots` entries past [`MAX_READ`] were not read (said in a warning).
    cut: bool,
    /// Part of the page tree was not walked ([`MAX_DEPTH`], [`MAX_NODES`] or [`MAX_PAGES`]), so
    /// the annotations of the pages there were not read (said in a warning).
    skipped: bool,
    /// The index of each old page, for the links that lead to one.
    old_pages: HashMap<ObjRef, usize>,
}

struct Annot {
    page: usize,
    object: Object,
    /// For a link without [`LINK_KEY`] shaped as layouts wrote rich-text links before they were
    /// marked: its address. It is the earlier layout's own when the new layout writes a link to
    /// the same address again, and is then not put back.
    unmarked: Option<Vec<u8>>,
}

/// Whether `d` is an annotation a layout writes: a widget (every field's widget is generated
/// again from the template), or a link `Emitter::text_link` wrote over rich text, which carries
/// [`LINK_KEY`].
fn written_by_layout(d: &Dict) -> bool {
    match d.name(b"Subtype") {
        Some(b"Widget") => true,
        Some(b"Link") => d.contains(LINK_KEY),
        _ => false,
    }
}

/// The address of a link shaped as `Emitter::text_link` wrote it before its links carried
/// [`LINK_KEY`]: exactly these keys, an invisible `/Border`, the flags of text shown everywhere,
/// on screen only or in print only (`/F` 4, 0 or 36) and a URI action. A link added in PdfCraft
/// has a border style, colour and highlight mode (`/BS`, `/C`, `/H`) instead.
fn unmarked_layout_link(doc: &Document, d: &Dict) -> Option<Vec<u8>> {
    const KEYS: [&[u8]; 7] = [b"Type", b"Subtype", b"Rect", b"P", b"F", b"Border", b"A"];
    if d.name(b"Subtype") != Some(b"Link".as_slice()) || !d.iter().all(|(k, _)| KEYS.contains(&k.as_slice())) {
        return None;
    }
    if !matches!(d.int(b"F"), Some(0 | 4 | 36)) {
        return None;
    }
    let border = doc.resolve(d.get(b"Border")?);
    let invisible = border.as_array().is_some_and(|b| b.len() == 3 && b.iter().all(|v| v.as_f64() == Some(0.0)));
    if !invisible {
        return None;
    }
    uri_of(doc, d)
}

/// The address of link `d`'s URI action, when a layout could have written it: one longer than
/// [`MAX_LINK`](super::MAX_LINK) characters of at most four bytes each is no layout's, and isn't
/// copied (thousands of annotations may share one long address).
fn uri_of(doc: &Document, d: &Dict) -> Option<Vec<u8>> {
    let action = doc.resolve(d.get(b"A")?);
    let action = action.as_dict()?;
    if action.name(b"S") != Some(b"URI".as_slice()) {
        return None;
    }
    let uri = doc.resolve(action.get(b"URI")?);
    let bytes = &uri.as_string()?.bytes;
    (bytes.len() <= super::MAX_LINK.saturating_mul(4)).then(|| bytes.clone())
}

/// Take note of the annotations on `doc`'s pages that a layout would not write again, in page
/// order. The page tree is walked with a seen-set and depth, node and page limits.
pub(crate) fn take(doc: &Document) -> Carried {
    let mut out = Carried { annots: Vec::new(), left: 0, cut: false, skipped: false, old_pages: HashMap::new() };
    let Some(top) = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|c| c.get(b"Pages")).and_then(Object::as_ref)) else {
        return out;
    };
    let mut seen = HashSet::new();
    let mut stack = vec![(top, 0usize)];
    let mut page = 0usize;
    // An annotation listed twice (on two pages, or twice on one) is carried once, where first met.
    let mut carried = HashSet::new();
    let mut read = 0usize;
    while let Some((node, depth)) = stack.pop() {
        if seen.contains(&node.num) {
            continue;
        }
        if seen.len() >= MAX_NODES || page >= MAX_PAGES {
            out.skipped = true;
            continue;
        }
        seen.insert(node.num);
        let object = doc.get(node);
        let Some(d) = object.as_dict() else { continue };
        let kids = d.get(b"Kids").map(|k| doc.resolve(k));
        let kids = kids.as_deref().and_then(Object::as_array);
        if d.name(b"Type") == Some(b"Pages".as_slice()) || (kids.is_some() && d.name(b"Type") != Some(b"Page".as_slice())) {
            let kids = kids.map(Vec::as_slice).unwrap_or_default();
            if depth >= MAX_DEPTH {
                out.skipped |= !kids.is_empty();
                continue;
            }
            // Pushed in reverse, so the first kid is visited first.
            let room = MAX_NODES.saturating_sub(seen.len());
            out.skipped |= kids.len() > room;
            stack.extend(kids.iter().take(room).rev().filter_map(Object::as_ref).map(|k| (k, depth + 1)));
            continue;
        }
        out.old_pages.entry(node).or_insert(page);
        // Read in place: only what the budget still allows is looked at, never the whole list.
        let annots = d.get(b"Annots").map(|a| doc.resolve(a));
        let annots = annots.as_deref().and_then(Object::as_array).map(Vec::as_slice).unwrap_or_default();
        let budget = MAX_READ.saturating_sub(read);
        if annots.len() > budget {
            out.cut = true;
        }
        for a in annots.iter().take(budget) {
            read += 1;
            if let Object::Ref(r) = a
                && !carried.insert(r.num)
            {
                continue;
            }
            let resolved = doc.resolve(a);
            let Some(ad) = resolved.as_dict() else { continue };
            if written_by_layout(ad) {
                continue;
            }
            if out.annots.len() >= MAX_CARRIED {
                out.left = out.left.saturating_add(1);
                continue;
            }
            out.annots.push(Annot { page, object: a.clone(), unmarked: unmarked_layout_link(doc, ad) });
        }
        page += 1;
    }
    out
}

/// The addresses of the links the new layout wrote on `pages` (the ones with [`LINK_KEY`]).
fn layout_link_addresses(doc: &Document, pages: &[ObjRef]) -> HashSet<Vec<u8>> {
    let mut out = HashSet::new();
    let mut read = 0usize;
    for page in pages {
        let object = doc.get(*page);
        let annots = object.as_dict().and_then(|d| d.get(b"Annots").map(|a| doc.resolve(a)));
        let annots = annots.as_deref().and_then(Object::as_array).map(Vec::as_slice).unwrap_or_default();
        for a in annots.iter().take(MAX_READ.saturating_sub(read)) {
            read += 1;
            let a = doc.resolve(a);
            if let Some(d) = a.as_dict()
                && d.name(b"Subtype") == Some(b"Link".as_slice())
                && d.contains(LINK_KEY)
                && let Some(uri) = uri_of(doc, d)
            {
                out.insert(uri);
            }
        }
    }
    out
}

/// Explicit destination `dest` (§12.3.2.2, Table 149) that leads to an old page, taken to the new
/// page with that page's index, or to the last page when there is none (`true`).
fn moved_dest(doc: &Document, dest: &Object, old: &HashMap<ObjRef, usize>, pages: &[ObjRef]) -> Option<(Object, bool)> {
    let dest = doc.resolve(dest);
    let items = dest.as_array().filter(|a| a.len() <= MAX_DEST)?;
    let index = *old.get(&items.first()?.as_ref()?)?;
    let (target, last) = match pages.get(index) {
        Some(&p) => (p, false),
        None => (*pages.last()?, true),
    };
    let mut items = items.clone();
    if let Some(first) = items.first_mut() {
        *first = Object::Ref(target);
    }
    Some((Object::Array(items), last))
}

/// Make link `annot`'s destination (`/Dest`, or a go-to action's `/D`, §12.6.4.2) lead to the new
/// page with its old page's index; the zoom operands stay. Named destinations, URI and remote
/// actions are left as they are. Returns whether the destination fell to the last page.
fn retarget(doc: &mut Document, annot: &mut Object, old: &HashMap<ObjRef, usize>, pages: &[ObjRef]) -> Result<bool, crate::XfaError> {
    let resolved = doc.resolve(annot);
    let Some(d) = resolved.as_dict().filter(|d| d.name(b"Subtype") == Some(b"Link".as_slice())) else {
        return Ok(false);
    };
    let mut last = false;
    if let Some(dest) = d.get(b"Dest")
        && let Some((new, l)) = moved_dest(doc, dest, old, pages)
    {
        last |= l;
        match dest {
            Object::Ref(r) => doc.set(*r, new),
            _ => set_key(doc, annot, b"Dest", new)?,
        }
    }
    if let Some(a) = d.get(b"A") {
        let action = doc.resolve(a);
        if let Some(act) = action.as_dict().filter(|act| act.name(b"S") == Some(b"GoTo".as_slice()))
            && let Some(dest) = act.get(b"D")
            && let Some((new, l)) = moved_dest(doc, dest, old, pages)
        {
            last |= l;
            match (dest, a) {
                (Object::Ref(r), _) => doc.set(*r, new),
                (_, Object::Ref(r)) => doc.update_dict(*r, |act| act.set(b"D".to_vec(), new))?,
                _ => {
                    let mut act = act.clone();
                    act.set(b"D".to_vec(), new);
                    set_key(doc, annot, b"A", Object::Dict(act))?;
                }
            }
        }
    }
    Ok(last)
}

/// Set `key` of annotation `annot`, held in an object of its own or in place.
fn set_key(doc: &mut Document, annot: &mut Object, key: &[u8], value: Object) -> Result<(), crate::XfaError> {
    match annot {
        Object::Ref(r) => doc.update_dict(*r, |d| d.set(key.to_vec(), value))?,
        Object::Dict(d) => d.set(key.to_vec(), value),
        _ => {}
    }
    Ok(())
}

/// Put `carried` back on the pages `doc` now has: each on the page with its old index, or, when
/// the new layout has fewer pages, on its last page (with a warning, as the rest of what was on
/// those pages is gone). Each annotation's `/P` names its new page (§12.5.2, Table 166), and a
/// link to an old page leads to the new page with that page's index. Returns the warnings.
pub(crate) fn put_back(doc: &mut Document, carried: Carried) -> Result<Vec<String>, crate::XfaError> {
    let mut warnings = Vec::new();
    if carried.skipped {
        warnings.push(format!(
            "The page tree is larger than PdfCraft reads ({MAX_DEPTH} levels, {MAX_NODES} nodes, {MAX_PAGES} pages); annotations on the pages past that were not carried over when the form was laid out again"
        ));
    }
    if carried.cut {
        warnings.push(format!(
            "The pages list more than {MAX_READ} annotations; those past them were not read, and not carried over when the form was laid out again"
        ));
    }
    if carried.left > 0 {
        warnings.push(format!("{} annotation(s) past the first {MAX_CARRIED} were not carried over when the form was laid out again", carried.left));
    }
    if carried.annots.is_empty() {
        return Ok(warnings);
    }
    let pages: Vec<ObjRef> = doc
        .root()
        .and_then(|r| doc.get(r).as_dict().and_then(|c| c.get(b"Pages")).map(|p| doc.resolve(p)))
        .and_then(|p| p.as_dict().and_then(|d| d.get(b"Kids")).map(|k| doc.resolve(k)))
        .and_then(|k| k.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(Object::as_ref)
        .collect();
    // A link an earlier layout wrote before its links were marked is written again by this one.
    let rewritten = if carried.annots.iter().any(|a| a.unmarked.is_some()) { layout_link_addresses(doc, &pages) } else { HashSet::new() };
    let annots: Vec<Annot> = carried.annots.into_iter().filter(|a| a.unmarked.as_ref().is_none_or(|uri| !rewritten.contains(uri))).collect();
    if annots.is_empty() {
        return Ok(warnings);
    }
    let Some(&last) = pages.last() else {
        warnings.push(format!("{} annotation(s) were not carried over: the form was laid out to no pages", annots.len()));
        return Ok(warnings);
    };
    let mut moved = 0usize;
    let mut led_to_last = 0usize;
    let mut by_page: Vec<Vec<Object>> = vec![Vec::new(); pages.len()];
    for Annot { page: index, object: mut a, .. } in annots {
        let (slot, target) = match pages.get(index) {
            Some(&p) => (index, p),
            None => {
                moved = moved.saturating_add(1);
                (pages.len().saturating_sub(1), last)
            }
        };
        match &mut a {
            Object::Ref(r) => {
                let r = *r;
                if doc.get(r).as_dict().is_some() {
                    doc.update_dict(r, |d| d.set(b"P".to_vec(), Object::Ref(target)))?;
                }
            }
            Object::Dict(d) => d.set(b"P".to_vec(), Object::Ref(target)),
            _ => {}
        }
        if retarget(doc, &mut a, &carried.old_pages, &pages)? {
            led_to_last = led_to_last.saturating_add(1);
        }
        if let Some(list) = by_page.get_mut(slot) {
            list.push(a);
        }
    }
    for (page, extra) in pages.iter().zip(by_page) {
        if extra.is_empty() {
            continue;
        }
        let at = doc.get(*page).as_dict().and_then(|d| d.get(b"Annots").and_then(Object::as_ref));
        let mut list =
            doc.get(*page).as_dict().and_then(|d| d.get(b"Annots").map(|a| doc.resolve(a))).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        // After the page's widgets and links: drawn over them, as they were (§12.5.1).
        list.extend(extra);
        match at {
            Some(a) => doc.set(a, Object::Array(list)),
            None => doc.update_dict(*page, |d| d.set(b"Annots".to_vec(), Object::Array(list)))?,
        }
    }
    if moved > 0 {
        warnings.push(format!("{moved} annotation(s) were on pages the form no longer lays out, and were moved to its last page"));
    }
    if led_to_last > 0 {
        warnings.push(format!("{led_to_last} link(s) led to pages the form no longer lays out, and now lead to its last page"));
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(entries: &[(&[u8], Object)]) -> Object {
        let mut d = Dict::new();
        for (k, v) in entries {
            d.set(k.to_vec(), v.clone());
        }
        Object::Dict(d)
    }

    /// A document whose page tree is one `/Pages` node over `pages`, set as the catalog's.
    fn with_pages(doc: &mut Document, pages: &[ObjRef]) {
        let kids = Object::Array(pages.iter().map(|p| Object::Ref(*p)).collect());
        let node = doc.add(dict(&[(b"Type", Object::name("Pages")), (b"Kids", kids), (b"Count", Object::Int(pages.len() as i64))]));
        let catalog = doc.add(dict(&[(b"Type", Object::name("Catalog")), (b"Pages", Object::Ref(node))]));
        doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(catalog));
    }

    fn page_with(doc: &mut Document, annots: Vec<Object>) -> ObjRef {
        doc.add(dict(&[(b"Type", Object::name("Page")), (b"Annots", Object::Array(annots))]))
    }

    fn uri_link(uri: &str, flags: i64) -> Object {
        let action = dict(&[(b"S", Object::name("URI")), (b"URI", Object::String(pdfcraft_cos::PdfString::literal(uri.as_bytes().to_vec())))]);
        dict(&[
            (b"Type", Object::name("Annot")),
            (b"Subtype", Object::name("Link")),
            (b"Rect", Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(10), Object::Int(10)])),
            (b"F", Object::Int(flags)),
            (b"Border", Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(0)])),
            (b"A", action),
        ])
    }

    /// The first element of the explicit destination `dest`, and the rest.
    fn dest_parts(doc: &Document, dest: &Object) -> (Option<ObjRef>, Vec<Object>) {
        let dest = doc.resolve(dest);
        let items = dest.as_array().unwrap();
        (items[0].as_ref(), items[1..].to_vec())
    }

    /// A page tree whose node lists itself and its parent, and three pages sharing one `/Annots`
    /// array of 1.2 million entries naming one comment: the walk ends, reads at most
    /// [`MAX_READ`] entries, carries the comment once, and says it stopped reading.
    #[test]
    fn hostile_page_trees_and_shared_annotation_lists_are_read_within_the_limits() {
        let mut doc = Document::new_empty();
        let note = doc.add(dict(&[(b"Type", Object::name("Annot")), (b"Subtype", Object::name("Text"))]));
        let list = doc.add(Object::Array(vec![Object::Ref(note); 1_200_000]));
        let pages = doc.add(Object::Null);
        let node = doc.add(Object::Null);
        let page = |doc: &mut Document| doc.add(dict(&[(b"Type", Object::name("Page")), (b"Annots", Object::Ref(list))]));
        let kids: Vec<Object> = (0..3).map(|_| Object::Ref(page(&mut doc))).collect();
        doc.set(
            node,
            dict(&[(b"Type", Object::name("Pages")), (b"Kids", Object::Array([kids, vec![Object::Ref(node), Object::Ref(pages)]].concat()))]),
        );
        doc.set(pages, dict(&[(b"Type", Object::name("Pages")), (b"Kids", Object::Array(vec![Object::Ref(node)]))]));
        let catalog = doc.add(dict(&[(b"Type", Object::name("Catalog")), (b"Pages", Object::Ref(pages))]));
        doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(catalog));
        let carried = take(&doc);
        assert_eq!(carried.annots.len(), 1);
        assert!(carried.cut);
        assert_eq!(carried.left, 0);
        assert!(!carried.skipped, "the whole (cyclic) tree was walked");
    }

    /// Thousands of pages sharing one `/Annots` array of 1.2 million entries: once the read
    /// budget is spent, the pages after are not read at all, so the walk stays quick rather than
    /// copying the long list once per page.
    #[test]
    fn pages_sharing_a_long_annotation_list_are_not_copied_once_each() {
        let mut doc = Document::new_empty();
        let note = doc.add(dict(&[(b"Type", Object::name("Annot")), (b"Subtype", Object::name("Text"))]));
        let list = doc.add(Object::Array(vec![Object::Ref(note); 1_200_000]));
        let pages: Vec<ObjRef> = (0..2_000).map(|_| doc.add(dict(&[(b"Type", Object::name("Page")), (b"Annots", Object::Ref(list))]))).collect();
        with_pages(&mut doc, &pages);
        let start = std::time::Instant::now();
        let carried = take(&doc);
        let took = start.elapsed();
        assert!(carried.cut);
        assert_eq!(carried.annots.len(), 1);
        assert_eq!(carried.old_pages.len(), 2_000);
        assert!(took < std::time::Duration::from_secs(20), "the walk took {took:?}");
    }

    /// A page tree deeper than [`MAX_DEPTH`] with a comment on its one page: the page is not
    /// read, and the warning says the tree was not read to its end, though nothing was carried.
    #[test]
    fn a_page_tree_past_the_limits_is_said() {
        let mut doc = Document::new_empty();
        let note = doc.add(dict(&[(b"Type", Object::name("Annot")), (b"Subtype", Object::name("Text"))]));
        let mut node = page_with(&mut doc, vec![Object::Ref(note)]);
        for _ in 0..=MAX_DEPTH {
            node = doc.add(dict(&[(b"Type", Object::name("Pages")), (b"Kids", Object::Array(vec![Object::Ref(node)]))]));
        }
        let catalog = doc.add(dict(&[(b"Type", Object::name("Catalog")), (b"Pages", Object::Ref(node))]));
        doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(catalog));
        let carried = take(&doc);
        assert!(carried.annots.is_empty());
        assert!(carried.skipped);
        let warnings = put_back(&mut doc, carried).unwrap();
        assert!(warnings.iter().any(|w| w.contains("page tree is larger")), "{warnings:?}");
    }

    /// A link a user added that happens to have the shape layouts gave their rich-text links
    /// before those were marked (as other producers write links) stays, and so does its address;
    /// an unmarked link of that shape whose address the new layout writes again was the earlier
    /// layout's (here a screen-only one, `/F 0`) and is not put back; a marked one never is.
    #[test]
    fn only_the_layouts_own_links_are_left_behind() {
        let mut doc = Document::new_empty();
        let by_hand = doc.add(uri_link("https://example.org/mine", 4));
        let earlier = doc.add(uri_link("https://example.org/form", 0));
        let marked = {
            let mut l = uri_link("https://example.org/other", 36);
            if let Object::Dict(d) = &mut l {
                d.set(LINK_KEY.to_vec(), Object::Bool(true));
            }
            doc.add(l)
        };
        let old = page_with(&mut doc, vec![Object::Ref(by_hand), Object::Ref(earlier), Object::Ref(marked)]);
        with_pages(&mut doc, &[old]);
        let carried = take(&doc);
        assert_eq!(carried.annots.len(), 2, "the marked link is the layout's");
        // The new layout writes its link to the form's address again, marked.
        let again = {
            let mut l = uri_link("https://example.org/form", 0);
            if let Object::Dict(d) = &mut l {
                d.set(LINK_KEY.to_vec(), Object::Bool(true));
            }
            doc.add(l)
        };
        let new = page_with(&mut doc, vec![Object::Ref(again)]);
        with_pages(&mut doc, &[new]);
        let warnings = put_back(&mut doc, carried).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let annots = doc.get(new).as_dict().unwrap().get(b"Annots").cloned().unwrap();
        assert_eq!(annots, Object::Array(vec![Object::Ref(again), Object::Ref(by_hand)]));
    }

    /// A link of the layouts' earlier shape whose address is longer than any a layout writes is
    /// a link of its own: its address is not copied for each annotation naming it, and the link is
    /// carried over all the same.
    #[test]
    fn a_link_address_longer_than_a_layout_writes_is_not_copied() {
        let mut doc = Document::new_empty();
        let longest = format!("https://example.org/{}", "a".repeat(4 * super::super::MAX_LINK - 20));
        let longer = format!("{longest}a");
        let at_limit = doc.add(uri_link(&longest, 4));
        let past = doc.add(uri_link(&longer, 4));
        let old = page_with(&mut doc, vec![Object::Ref(at_limit), Object::Ref(past)]);
        with_pages(&mut doc, &[old]);
        let carried = take(&doc);
        let unmarked: Vec<Option<usize>> = carried.annots.iter().map(|a| a.unmarked.as_ref().map(Vec::len)).collect();
        assert_eq!(unmarked, [Some(longest.len()), None]);
        let new = page_with(&mut doc, vec![]);
        with_pages(&mut doc, &[new]);
        put_back(&mut doc, carried).unwrap();
        let annots = doc.get(new).as_dict().unwrap().get(b"Annots").cloned().unwrap();
        assert_eq!(annots, Object::Array(vec![Object::Ref(at_limit), Object::Ref(past)]));
    }

    /// Links to pages lead, after a new layout, to the new page with the same index, the zoom
    /// operands kept: a `/Dest` in the annotation, a go-to action of its own whose `/D` is in
    /// place, and a `/Dest` held as an object of its own. A link to a page the new layout no
    /// longer has leads to its last page, and that is said; a named destination is left as it is.
    #[test]
    fn links_to_pages_lead_to_the_new_pages() {
        let mut doc = Document::new_empty();
        let old: Vec<ObjRef> = (0..3).map(|_| doc.add(Object::Null)).collect();
        let xyz = |p: ObjRef| Object::Array(vec![Object::Ref(p), Object::name("XYZ"), Object::Int(10), Object::Int(20), Object::Int(0)]);
        let link = |extra: (&[u8], Object)| {
            dict(&[(b"Type", Object::name("Annot")), (b"Subtype", Object::name("Link")), (b"Rect", Object::Array(vec![])), extra])
        };
        let in_place = doc.add(link((b"Dest", xyz(old[1]))));
        let action = doc.add(dict(&[(b"S", Object::name("GoTo")), (b"D", Object::Array(vec![Object::Ref(old[0]), Object::name("Fit")]))]));
        let by_action = doc.add(link((b"A", Object::Ref(action))));
        let dest_obj = doc.add(xyz(old[2]));
        let gone = doc.add(link((b"Dest", Object::Ref(dest_obj))));
        let named = doc.add(link((b"Dest", Object::name("chapter"))));
        for (i, p) in old.iter().enumerate() {
            let annots = if i == 0 { vec![Object::Ref(in_place), Object::Ref(by_action), Object::Ref(gone), Object::Ref(named)] } else { vec![] };
            doc.set(*p, dict(&[(b"Type", Object::name("Page")), (b"Annots", Object::Array(annots))]));
        }
        with_pages(&mut doc, &old);
        let carried = take(&doc);
        let new: Vec<ObjRef> = (0..2).map(|_| page_with(&mut doc, vec![])).collect();
        with_pages(&mut doc, &new);
        let warnings = put_back(&mut doc, carried).unwrap();
        let get = |r: ObjRef, k: &[u8]| doc.get(r).as_dict().unwrap().get(k).cloned().unwrap();
        let zoom = vec![Object::name("XYZ"), Object::Int(10), Object::Int(20), Object::Int(0)];
        assert_eq!(dest_parts(&doc, &get(in_place, b"Dest")), (Some(new[1]), zoom.clone()));
        assert_eq!(dest_parts(&doc, &get(action, b"D")), (Some(new[0]), vec![Object::name("Fit")]));
        assert_eq!(get(gone, b"Dest"), Object::Ref(dest_obj), "the destination object is updated in place");
        assert_eq!(dest_parts(&doc, &get(gone, b"Dest")), (Some(new[1]), zoom), "to the last page");
        assert_eq!(get(named, b"Dest"), Object::name("chapter"));
        assert!(warnings.iter().any(|w| w.starts_with("1 link(s) led to pages")), "{warnings:?}");
    }
}
