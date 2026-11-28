//! PDF/A-2 and -3 6.9, PDF/A-4 6.10: optional content. Each optional content configuration
//! dictionary, the catalog's `/OCProperties /D` and those in its `/Configs` (ISO 32000-2 8.11.4.2,
//! Tables 98 and 99):
//!
//! - has a `/Name`, unique among the configurations: names are text strings (Table 99), so two
//!   are the same when they decode to the same text (7.9.2.2), however each is encoded;
//! - lists every optional content group of the file (`/OCGs`) when it has an `/Order` array, at any
//!   depth of that array's nesting;
//! - in PDF/A-2 and 3, has no `/AS` (auto state) entry. PDF/A-4 doesn't ask this.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::Level;

/// How many optional content configurations of `/Configs` are read; a longer `/Configs` is reported
/// as not checked in full.
const MAX_CONFIGURATIONS: usize = 4096;
/// How deep an `/Order` array's nested arrays are followed.
const MAX_ORDER_DEPTH: usize = 32;
/// How many entries of one `/Order` array, at every depth, are read.
const MAX_ORDER_ENTRIES: usize = 1 << 16;

/// The groups an `/Order` array lists, at every depth of its nesting, or `None` when it is larger or
/// deeper than is read: then which groups it lists isn't known.
fn ordered(doc: &Document, order: &Object) -> Option<HashSet<ObjRef>> {
    let mut found = HashSet::new();
    let mut stack: Vec<(Object, usize)> = vec![(order.clone(), 0)];
    let mut read = 0usize;
    while let Some((o, depth)) = stack.pop() {
        read = read.saturating_add(1);
        if read > MAX_ORDER_ENTRIES {
            return None;
        }
        let entries = match o {
            Object::Ref(r) => match &*doc.resolve(&Object::Ref(r)) {
                // An array kept as an object of its own is followed like a direct one.
                Object::Array(a) => a.clone(),
                // A reference to null, or to no object, names no group (ISO 32000-2 7.3.10).
                Object::Null => continue,
                _ => {
                    found.insert(r);
                    continue;
                }
            },
            Object::Array(a) => a,
            // A label (a text string) or anything else names no group.
            _ => continue,
        };
        if depth >= MAX_ORDER_DEPTH {
            return None;
        }
        stack.extend(entries.into_iter().map(|x| (x, depth.saturating_add(1))));
    }
    Some(found)
}

/// The optional content rules for `level`, cited as PDF/A-2 numbers them (6.9): the caller gives
/// PDF/A-4 its own number.
pub(crate) fn check(
    doc: &Document,
    level: Level,
    catalog: &Dict,
    issue: &mut dyn FnMut(&'static str, &'static str, Option<String>, Option<usize>, bool),
) {
    let Some(properties) = catalog.get(b"OCProperties").map(|p| doc.resolve(p)) else { return };
    let Some(properties) = properties.as_dict() else { return };
    let mut configurations: Vec<Object> = properties.get(b"D").cloned().into_iter().collect();
    if let Some(list) = properties.get(b"Configs").map(|c| doc.resolve(c))
        && let Some(list) = list.as_array()
    {
        configurations.extend(list.iter().take(MAX_CONFIGURATIONS).cloned());
        // What the configurations past the limit hold isn't known, so the file doesn't pass.
        if list.len() > MAX_CONFIGURATIONS {
            issue(
                "6.9",
                "Some optional content configurations weren't checked: there are more of them than one verification reads",
                None,
                None,
                false,
            );
        }
    }
    // Every group of the file is listed in /OCGs (Table 98).
    let groups: HashSet<ObjRef> = properties
        .get(b"OCGs")
        .map(|g| doc.resolve(g))
        .and_then(|g| g.as_array().map(|a| a.iter().take(MAX_ORDER_ENTRIES).filter_map(Object::as_ref).collect()))
        .unwrap_or_default();
    let mut names = HashSet::new();
    let (mut unnamed, mut repeated, mut unordered, mut auto_state) = (false, false, false, false);
    for configuration in &configurations {
        let configuration = doc.resolve(configuration);
        let Some(configuration) = configuration.as_dict() else { continue };
        match configuration.get(b"Name").map(|n| doc.resolve(n)).and_then(|n| n.as_string().map(pdfcraft_cos::PdfString::to_text)) {
            Some(name) => repeated |= !names.insert(name),
            None => unnamed = true,
        }
        // A null /Order, direct or through a reference, is none (7.3.9); one too large or deep to
        // read in full isn't judged.
        if let Some(order) = configuration.get(b"Order").filter(|o| !matches!(*doc.resolve(o), Object::Null))
            && let Some(listed) = ordered(doc, order)
        {
            unordered |= groups.iter().any(|g| !listed.contains(g));
        }
        auto_state |= configuration.get(b"AS").is_some_and(|a| !matches!(*doc.resolve(a), Object::Null));
    }
    if unnamed {
        issue("6.9", "An optional content configuration has no Name", None, None, false);
    }
    if repeated {
        issue("6.9", "Two optional content configurations have the same Name", None, None, false);
    }
    if unordered {
        issue("6.9", "An optional content configuration's Order doesn't list every optional content group", None, None, false);
    }
    if auto_state && level.part() != 4 {
        issue("6.9", "An optional content configuration has an auto state (AS)", None, None, false);
    }
}
