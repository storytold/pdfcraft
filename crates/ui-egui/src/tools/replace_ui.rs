//! Find and replace from the find bar (Edit text mode): the engine plans the change
//! ([`pdfcraft_engine::Document::plan_replace`]) and it is applied as one edit, so Replace all
//! is one undo step; what was left alone is said, never skipped silently.

use std::collections::BTreeMap;

use pdfcraft_engine::find_replace::{Replace, Replacement, Which};

use crate::PdfCraftApp;
use crate::canvas::FindHit;

impl PdfCraftApp {
    /// Replace the find bar's highlighted match (`all` false) or every match in the document, in
    /// view `i`, with its Replace field's text.
    pub(crate) fn replace_found(&mut self, i: usize, all: bool) {
        // A comment hit has no glyphs. Said here, before the view is borrowed for the plan.
        if !all && highlighted_is_comment(self, i) {
            let why = crate::i18n::t("not editable text");
            self.notify(crate::i18n::fmt(tl!("Not replaced: {why}"), &[("why", why)]));
            return;
        }
        let Some(view) = self.views.get(i) else { return };
        let Some(find) = view.find.as_ref() else { return };
        if find.case_query.trim().is_empty() {
            return;
        }
        let query = Replace {
            find: find.case_query.clone(),
            with: find.replacement.clone(),
            case_sensitive: find.case_sensitive,
            whole_words: find.whole_words,
        };
        let Some(doc) = self.session.get(view.id) else { return };
        let (pages, which, found) = if all {
            // What the find bar holds on each page it has searched, so matches no paragraph
            // holds (a form field, a comment) are counted rather than lost.
            let mut found: BTreeMap<usize, usize> = view.texts.keys().map(|p| (*p, 0)).collect();
            for hit in &find.matches {
                let n = found.entry(hit.page()).or_insert(0);
                *n = n.saturating_add(1);
            }
            ((0..doc.info.pages.len()).collect::<Vec<_>>(), Which::All, Some(found))
        } else {
            // The highlighted match, by where its first and last glyphs are drawn.
            // A comment hit was reported above; what remains is page text.
            let Some(hit) = find.current.and_then(|c| find.matches.get(c)) else { return };
            let Some((page, range)) = text_span(hit) else { return };
            let centre = |g: usize| {
                view.texts.get(&page).and_then(|t| t.glyphs.get(g)).map(|g| [(g.rect[0] + g.rect[2]) / 2.0, (g.rect[1] + g.rect[3]) / 2.0])
            };
            let (Some(start), Some(end)) = (centre(range.start), centre(range.end.saturating_sub(1))) else { return };
            (vec![page], Which::At { page, start, end }, None)
        };
        let plan = match doc.plan_replace(&pages, &query, which, found.as_ref()) {
            Ok(plan) => plan,
            Err(e) => {
                self.notify_fmt("{label} failed: {e}", &[("label", tl!("Replace")), ("e", &e.to_string())]);
                return;
            }
        };
        if let Some(edit) = plan.edit.clone()
            && !self.apply_edit(edit)
        {
            return;
        }
        if let Some(message) = outcome(&plan, all) {
            self.notify(message);
        }
    }
}

/// What to tell the user: the count and pages after Replace all, what was left and why. Nothing
/// after one match replaced (the page shows it).
fn outcome(plan: &Replacement, all: bool) -> Option<String> {
    let skipped = plan.skipped_total();
    if plan.replaced == 0 && skipped == 0 {
        return Some(tl!("No matches to replace").to_string());
    }
    let why = plan
        .skipped
        .iter()
        .map(|(reason, n)| crate::i18n::fmt(tl!("{why} ({n})"), &[("why", crate::i18n::t(reason.wording())), ("n", &n.to_string())]))
        .collect::<Vec<_>>()
        .join(", ");
    if !all {
        return (skipped > 0).then(|| crate::i18n::fmt(tl!("Not replaced: {why}"), &[("why", &why)]));
    }
    let lang = crate::i18n::current();
    let pages = crate::i18n::trn(lang, plan.pages.len() as u64, "{n} page", "{n} pages");
    let mut message = if plan.replaced == 0 {
        tl!("Nothing replaced").to_string()
    } else {
        crate::i18n::fmt(tl!("Replaced {n} on {pages}"), &[("n", &plan.replaced.to_string()), ("pages", &pages)])
    };
    if skipped > 0 {
        message.push_str(". ");
        message.push_str(&crate::i18n::fmt(tl!("{k} skipped: {why}"), &[("k", &skipped.to_string()), ("why", &why)]));
    }
    Some(message)
}

/// The highlighted hit is a comment, and there is a query to replace. An empty query is left
/// to the caller, which does nothing.
fn highlighted_is_comment(app: &PdfCraftApp, i: usize) -> bool {
    let Some(find) = app.views.get(i).and_then(|view| view.find.as_ref()) else { return false };
    if find.case_query.trim().is_empty() {
        return false;
    }
    find.current.and_then(|c| find.matches.get(c)).is_some_and(|hit| text_span(hit).is_none())
}

/// The page and glyph range of a page-text hit. A comment hit is not editable page text.
fn text_span(hit: &FindHit) -> Option<(usize, std::ops::Range<usize>)> {
    match hit {
        FindHit::Text { page, range } => Some((*page, range.clone())),
        FindHit::Comment { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::text_span;
    use crate::canvas::FindHit;

    #[test]
    fn replace_takes_a_text_hit_and_leaves_a_comment() {
        assert_eq!(text_span(&FindHit::text(2, 4..9)), Some((2, 4..9)));
        let comment = FindHit::Comment { page: 1, index: 0, top: 10.0, rect: [0.0, 0.0, 1.0, 1.0], kind: "Note".into(), excerpt: "zebra".into() };
        assert_eq!(text_span(&comment), None);
    }
}
