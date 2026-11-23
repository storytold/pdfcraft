use craft::behavior::IconId;
use craft::pages::ui::PageBody;
use craft::theme::variant::Tone;
use craft::ui::contract::Show as _;
use craft::ui::notices::{Notice as Bar, NoticeAction, NoticeIcon, NoticeLayout};

use super::{DocView, Pos2, Rect, SnapshotKind};

/// Whether `p` lies in the bottom-left quadrant of `area`, where the page size shows (#1855).
pub(super) fn in_size_hint_zone(area: Rect, p: Pos2) -> bool {
    p.x >= area.min.x && p.x < area.center().x && p.y > area.center().y && p.y <= area.max.y
}

/// The current page's size, floating at the bottom-left of the document area while the pointer is
/// in that quadrant, as Acrobat does. Same formatter and unit as Document Properties. The pointer
/// counts only when it is over the body in the body's own layer, so a dialog or popup above hides the hint.
pub(super) fn page_size_hint(ui: &mut egui::Ui, body: &PageBody<'_>, page: &pdfcraft_render::PageInfo, units: crate::units::Unit) {
    let probe = ui.interact(body.rect, ui.id().with("page-size-hint-zone"), egui::Sense::hover());
    let inside = probe.hover_pos().is_some_and(|p| in_size_hint_zone(body.rect, p));
    if !inside {
        return;
    }
    let text = units.size(f64::from(page.width), f64::from(page.height));
    // PLAN: a card notice is the framework's floating label with no interaction; its own surface padding is the inset.
    // Shrink-wrapped to its text, a little in from the corner, as production's pill sits.
    let font = egui::TextStyle::Body.resolve(ui.style());
    let wide = ui.painter().layout_no_wrap(text.to_string(), font, egui::Color32::WHITE).size().x + 40.0;
    let corner = egui::Rect::from_min_max(
        egui::pos2(body.rect.min.x + 10.0, body.rect.min.y),
        egui::pos2(body.rect.min.x + 10.0 + wide, body.rect.max.y - 10.0),
    );
    let builder = egui::UiBuilder::new().max_rect(corner).layout(egui::Layout::bottom_up(egui::Align::Min)).sense(egui::Sense::hover());
    ui.scope_builder(builder, |ui| {
        Bar::new(Tone::Neutral, text).layout(NoticeLayout::Card).icon(NoticeIcon::None).show(ui);
    });
}

/// What the notice bar's buttons ask for.
pub(super) enum Notice {
    /// Reload the file that changed on disk, dropping unsaved changes.
    Reload,
    Security,
    Signatures,
    Repairs,
    /// Highlight fields was turned on or off.
    FieldHighlights(bool),
    /// Open the JavaScript console (the form's scripts were turned off; the error is there).
    Console,
    /// Unlock… (enter the permissions password).
    Unlock,
    /// Close this historical snapshot tab (View signed version / View revision).
    Close,
    /// Stop showing the secured-document notice, in every document (#4035).
    HideSecured,
    /// Lock the placed signatures into the page (#2149).
    LockSignatures,
}

/// One page notice bar with the icon named `icon`.
fn bar(_tone: Tone, icon: &'static str, text: String) -> Bar<'static> {
    // Production paints every notice bar on the soft accent surface, whatever it warns of; the icon says which kind.
    Bar::new(Tone::Info, text).layout(NoticeLayout::Bar).icon(NoticeIcon::Custom(IconId::new(icon).into()))
}

/// The notices above the pages, one after another: a file changed on disk first, then what a historical
/// snapshot is, then pages cut at the renderer's limits, then the offer to lock a signature just
/// placed, then the signature status bar, a form's scripts having been turned off, security,
/// forms and warnings. None in full screen.
pub(super) fn notices(
    view: &mut DocView,
    doc: &pdfcraft_engine::Document,
    secured: bool,
    repaired: bool,
    lock_offer: bool,
    signed: Option<(&'static str, Tone, &str, String)>,
    ui: &mut egui::Ui,
) -> Option<Notice> {
    // Full screen shows the page and nothing else (#1459), so the notice bar above the pages waits
    // until it is left; filling the screen, a bar would push the page past its bottom (#576).
    if view.presenting() {
        return None;
    }
    let info = &doc.info;
    let xfa = doc.xfa.as_ref();
    // The file changed on disk while this document has unsaved changes (#431): ask before
    // replacing it, ahead of every other notice.
    if view.disk_changed {
        let out = bar(
            Tone::Info,
            "rotate-cw",
            tl!("This file changed on disk. Reload shows the new version and drops your unsaved changes; Save replaces it with this version.")
                .to_string(),
        )
        .action(NoticeAction::new(tl!("Reload").to_string()).default_action())
        .dismiss()
        .show(ui);
        if out.dismissed {
            view.disk_changed = false;
        }
        return (out.action == Some(0)).then_some(Notice::Reload);
    }
    // A historical snapshot (View signed version / View revision): what it is, above the
    // signature and security bars, so the tab is never mistaken for the live document.
    if let Some(snap) = view.snapshot.as_ref() {
        let text = if snap.kind == SnapshotKind::SignedVersion {
            crate::i18n::fmt(
                tl!("You are viewing the signed version of “{from}” (revision {rev}). It is a snapshot: changes here won't affect the original."),
                &[("from", &snap.from), ("rev", &snap.revision.to_string())],
            )
        } else {
            crate::i18n::fmt(
                tl!("You are viewing revision {rev} of “{from}”. It is a snapshot: changes here won't affect the original."),
                &[("rev", &snap.revision.to_string()), ("from", &snap.from)],
            )
        };
        let out = bar(Tone::Info, "clock", text).action(NoticeAction::new(tl!("Close").to_string())).show(ui);
        if out.action == Some(0) {
            return Some(Notice::Close);
        }
    }
    // Content cut at the renderer's limits (#623): what is missing is not visible, so the page
    // is named in a bar of its own, which the signature, form and security bars below can't
    // replace and dismissing them doesn't hide. Dismissing it covers the pages cut so far: a
    // page found cut later shows it again.
    if view.cut_pages().iter().any(|p| !view.cut_notice_dismissed.contains(p)) {
        cut_pages_bar(view, ui);
    }
    // A signature just placed with Fill & Sign is a comment others can move (#2149): offer to
    // lock it into the page now. `lock_offer` is false for a digitally signed document.
    if lock_offer && !view.lock_offer_dismissed {
        let out = bar(Tone::Info, "lock", tl!("Signature added. Others can move it until it's locked.").to_string())
            .action(NoticeAction::new(tl!("Lock now").to_string()).default_action())
            .dismiss()
            .show(ui);
        if out.dismissed {
            view.lock_offer_dismissed = true;
        }
        if out.action == Some(0) {
            return Some(Notice::LockSignatures);
        }
    }
    if let Some((icon, tone, template, arg)) = signed {
        let out =
            bar(tone, icon, crate::i18n::fmt(tl!(template), &[("by", &arg)])).action(NoticeAction::new(tl!("Signature panel").to_string())).show(ui);
        return (out.action == Some(0)).then_some(Notice::Signatures);
    }
    if view.notice_dismissed {
        return None;
    }
    // A form whose scripts were turned off no longer calculates or answers its buttons: that
    // comes before what else there is to say about the document.
    let scripts_off = doc.xfa_scripts_off();
    // What the document is as a form, if it is one: what the bar says first, secured or not.
    let form = if let Some(x) = xfa {
        let lang = crate::i18n::current();
        let pages = crate::i18n::trn(lang, x.pages as u64, "{n} page", "{n} pages");
        let fields = crate::i18n::trn(lang, x.fields as u64, "{n} field", "{n} fields");
        let mut text =
            crate::i18n::fmt(tl!("Dynamic XFA form laid out from its template: {pages}, {fields}."), &[("pages", &pages), ("fields", &fields)]);
        if !x.warnings.is_empty() {
            // The notes are clauses ("a picture is left out"); the first starts a sentence here.
            let notes = x.warnings.join("; ");
            let mut chars = notes.chars();
            text.push(' ');
            text.extend(chars.next().into_iter().flat_map(char::to_uppercase));
            text.push_str(chars.as_str());
            text.push('.');
        }
        Some(("text-cursor-input", Tone::Info, text, true))
    } else if info.xfa == Some(pdfcraft_render::Xfa::Dynamic) {
        Some((
            "triangle-alert",
            Tone::Warn,
            tl!("This is a dynamic XFA form, which PdfCraft can't display yet. What you see is the file's placeholder page.").to_string(),
            false,
        ))
    } else if info.xfa == Some(pdfcraft_render::Xfa::Static) {
        Some((
            "text-cursor-input",
            Tone::Info,
            tl!("XFA form: its fields and its XFA data are kept in step, so other viewers show what you fill in.").to_string(),
            true,
        ))
    } else if info.fields.iter().any(|f| f.kind != pdfcraft_render::FieldKind::PushButton) {
        // Push buttons alone (LaTeX `animate` frames, navigation buttons) leave nothing to fill in.
        Some((
            "text-cursor-input",
            Tone::Info,
            crate::i18n::fmt(tl!("This document contains {n} interactive form fields."), &[("n", &info.fields.len().to_string())]),
            true,
        ))
    } else {
        None
    };
    let msg = if scripts_off {
        Some(("triangle-alert", Tone::Info, crate::js_ui::scripts_off_notice(), true))
    } else if secured {
        // A secured form still says what it is, and keeps Highlight fields: the security
        // sentence follows the form's.
        let security = tl!("This document is secured. Some changes are restricted by its security settings.");
        Some(match form {
            Some((icon, tone, text, fields)) => (icon, tone, format!("{text} {security}"), fields),
            None => ("lock", Tone::Info, security.to_string(), false),
        })
    } else if form.is_some() {
        form
    } else if repaired {
        Some(("bandage", Tone::Info, tl!("This file was damaged and has been repaired. Saving keeps the repaired version.").to_string(), false))
    } else if !info.warnings.is_empty() {
        Some(("triangle-alert", Tone::Warn, info.warnings[0].clone(), false))
    } else {
        None
    };
    let (icon, tone, text, fields) = msg?;
    // What each action returns, by the action's index; `None` is the field highlight toggle.
    let mut results: Vec<Option<Notice>> = Vec::new();
    let mut notice = bar(tone, icon, text).dismiss();
    // Production lays the buttons out right to left, so the bar reads, left to right: Details, Don't show again,
    // Unlock, Security settings, JavaScript Console, then the field highlights next to the dismiss control.
    if repaired && !secured && info.fields.is_empty() {
        notice = notice.action(NoticeAction::new(tl!("Details").to_string()));
        results.push(Some(Notice::Repairs));
    }
    // Only when the bar shows the security message: a form's stopped scripts come first.
    if secured && !scripts_off {
        notice = notice.action(NoticeAction::new(tl!("Don't show again").to_string()));
        results.push(Some(Notice::HideSecured));
    }
    if secured && doc.can_unlock() {
        notice = notice.action(NoticeAction::new(tl!("Unlock…").to_string()).default_action());
        results.push(Some(Notice::Unlock));
    }
    if secured {
        notice = notice.action(NoticeAction::new(tl!("Security settings").to_string()));
        results.push(Some(Notice::Security));
    }
    if scripts_off {
        notice = notice.action(NoticeAction::new(tl!("JavaScript Console").to_string()));
        results.push(Some(Notice::Console));
    }
    if fields {
        let label = if view.session.highlight_fields { tl!("Hide field highlights") } else { tl!("Highlight fields") };
        notice = notice.action(NoticeAction::new(label.to_string()));
        results.push(None);
    }
    let out = notice.show(ui);
    if out.dismissed {
        view.notice_dismissed = true;
    }
    let taken = results.into_iter().nth(out.action?)?;
    Some(taken.unwrap_or_else(|| {
        view.session.highlight_fields = !view.session.highlight_fields;
        Notice::FieldHighlights(view.session.highlight_fields)
    }))
}

/// The notice naming the pages whose content was cut at the renderer's limits (#623).
fn cut_pages_bar(view: &mut DocView, ui: &mut egui::Ui) {
    let cut = view.cut_pages();
    let Some(&first) = cut.first() else { return };
    let page = first.saturating_add(1).to_string();
    let n = cut.len();
    let text = if n == 1 {
        crate::i18n::fmt(
            tl!("Page {page} holds more content than PdfCraft can read: part of it isn't drawn, and only part of its text can be found or copied."),
            &[("page", &page)],
        )
    } else {
        crate::i18n::fmt(
            tl!(
                "{n} pages hold more content than PdfCraft can read, page {page} first: parts of them aren't drawn, and only part of their text can be found or copied."
            ),
            &[("n", &n.to_string()), ("page", &page)],
        )
    };
    if bar(Tone::Warn, "triangle-alert", text).dismiss().show(ui).dismissed {
        view.cut_notice_dismissed.extend(cut);
    }
}
