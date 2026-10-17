//! Find and replace from the find bar (Edit text mode): the engine plans the change
//! ([`pdfcraft_engine::Document::plan_replace`]) and it is applied as one edit, so Replace all
//! is one undo step; what was left alone is said, never skipped silently.

use std::collections::BTreeMap;

use pdfcraft_engine::find_replace::{Replace, Replacement, Which};

use crate::PdfCraftApp;

impl PdfCraftApp {
    /// Replace the find bar's highlighted match (`all` false) or every match in the document, in
    /// view `i`, with its Replace field's text.
    pub(crate) fn replace_found(&mut self, i: usize, all: bool) {
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
            for (p, _) in &find.matches {
                let n = found.entry(*p).or_insert(0);
                *n = n.saturating_add(1);
            }
            ((0..doc.info.pages.len()).collect::<Vec<_>>(), Which::All, Some(found))
        } else {
            // The highlighted match, by where its first and last glyphs are drawn.
            let Some((page, range)) = find.current.and_then(|c| find.matches.get(c)).cloned() else { return };
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
