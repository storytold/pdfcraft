use std::cell::RefCell;

use craft::behavior::{DisplayOrder, IconId, ItemSource};
use craft::theme::variant::Tone;
use craft::ui::buttons::{Button, Link};
use craft::ui::contract::{ActivateOn, CollectionEvent, Show as _, ShowCollection as _, ShowContainer as _};
use craft::ui::layout_components::Stack;
use craft::ui::list::{List, ListRow, OpenSet, RowInk};
use craft::ui::notices::Notice;
use craft::ui::primitives::IconSpec;
use craft::ui::runtime::env::Label;

use super::{Certificate, SignatureInfo, SignatureStatus, sign};

/// The message bar for signed documents: (icon, tone, template, argument).
/// The template renders through [`crate::i18n::tr_fmt`] at the call site, so the bar follows
/// the UI language; names inside `argument` stay as they are.
pub(crate) fn banner(sigs: &[SignatureInfo]) -> Option<(&'static str, Tone, &'static str, String)> {
    let signed: Vec<&SignatureInfo> = sigs.iter().filter(|s| s.signed).collect();
    if signed.is_empty() {
        return None;
    }
    // A valid certification heads the bar only when every signature is valid: it says nothing
    // about the signatures added after it, so their problems come first.
    if let Some(c) = signed.iter().find(|s| s.certify.is_some())
        && signed.iter().all(|s| s.status == SignatureStatus::Valid)
    {
        let by = c.certificate.as_ref().map(|x| {
            let org = x.subject.organization().map(|o| format!(", {o}")).unwrap_or_default();
            let who = format!("{}{org}", x.display_name());
            let (template, issuer) = match x.issuer.common_name() {
                Some(issuer) => ("{who}, certificate issued by {issuer}.", issuer),
                None => ("{who}, certificate issued by an unknown issuer.", ""),
            };
            crate::i18n::fmt(tl!(template), &[("who", &who), ("issuer", issuer)])
        });
        return Some(("badge-check", Tone::Info, "Certified by {by}", by.unwrap_or_default()));
    }
    Some(if signed.iter().any(|s| s.status == SignatureStatus::Invalid) {
        ("circle-x", Tone::Bad, "At least one signature is invalid.", String::new())
    } else if signed.iter().all(|s| s.status == SignatureStatus::Valid) {
        ("circle-check", Tone::Good, "Signed and all signatures are valid.", String::new())
    } else {
        ("triangle-alert", Tone::Warn, "At least one signature has problems.", String::new())
    })
}

/// One look per signature verdict, shared by the Signatures panel and the page badges so
/// the two can't drift apart.
pub(crate) fn status_tone(status: SignatureStatus) -> (IconId, Tone) {
    match status {
        SignatureStatus::Valid => (IconId::new("circle-check"), Tone::Good),
        SignatureStatus::Unknown => (IconId::new("triangle-alert"), Tone::Warn),
        SignatureStatus::Invalid => (IconId::new("circle-x"), Tone::Bad),
    }
}

/// The look of a signature's head row: its verdict, or the pen of a field nobody has signed.
fn head_look(s: &SignatureInfo) -> (IconId, Tone) {
    if s.signed { status_tone(s.status) } else { (IconId::new("pen-line"), Tone::Neutral) }
}

/// What the Signatures panel asks the app to do.
#[derive(Clone, Debug)]
pub enum PanelAction {
    Validate,
    GoTo(usize),
    Trust(Box<Certificate>),
    ViewSigned {
        len: usize,
        revision: usize,
    },
    Sign(String),
    ExportCertificate(Box<Certificate>),
    /// Certificate Viewer for the signer's chain (signer first).
    ViewCertificate(Vec<Certificate>),
}

/// A row of the Signatures tree: a signature's head, or one line of what its open head shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum SigRow {
    Head(String),
    Line(String, usize),
}

/// One line under an open head.
enum Child {
    /// The unsigned field's "Sign this field…" button.
    Sign,
    /// The verdict sentence.
    Status,
    /// One finding of the validation, already in the panel's language.
    Detail(String),
    /// The "Signature Details" heading.
    Heading,
    /// A detail's name and value.
    Pair(&'static str, String),
    /// The field's page, as a link that goes there.
    Page(usize),
    /// The buttons under the details.
    Buttons,
}

/// The lines under the head of `s`, in the order they show.
fn children(s: &SignatureInfo) -> Vec<Child> {
    if !s.signed {
        return vec![Child::Sign];
    }
    let mut lines = vec![Child::Status];
    // Each detail is a whole sentence of ours plus what the signature itself says; said in the
    // panel's language, with the signature's own words left as they are.
    lines.extend(s.details.iter().map(|d| Child::Detail(d.said(&crate::i18n::words).to_string())));
    lines.push(Child::Heading);
    if let Some(r) = &s.reason {
        lines.push(Child::Pair("Reason", r.clone()));
    }
    if let Some(l) = &s.location {
        lines.push(Child::Pair("Location", l.clone()));
    }
    if let Some(d) = &s.date {
        lines.push(Child::Pair("Signing time", sign::pdf::display_date(d)));
    }
    if let Some(a) = &s.algorithm {
        lines.push(Child::Pair("Algorithm", a.clone()));
    }
    if let Some(c) = &s.certificate {
        lines.push(Child::Pair("Signer", c.subject.display()));
        lines.push(Child::Pair("Issued by", c.issuer.display()));
        lines.push(Child::Pair("Valid", format!("{} to {}", c.not_before, c.not_after)));
    }
    lines.push(match s.page {
        Some(p) => Child::Page(p),
        None => Child::Pair("Field", s.field.clone()),
    });
    lines.push(Child::Buttons);
    lines
}

/// The rows the tree shows: every head, and the lines of the heads that are open.
struct Rows<'a> {
    entries: Vec<(SigRow, usize, bool)>,
    open: &'a OpenSet<String>,
}

impl DisplayOrder<SigRow> for Rows<'_> {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn key(&self, at: usize) -> SigRow {
        self.entries[at].0.clone()
    }

    fn depth(&self, at: usize) -> usize {
        self.entries[at].1
    }

    fn has_children(&self, at: usize) -> bool {
        self.entries[at].2
    }

    fn open(&self, at: usize) -> bool {
        match &self.entries[at].0 {
            SigRow::Head(field) => self.open.is_open(field),
            SigRow::Line(..) => false,
        }
    }
}

impl ItemSource<SigRow> for Rows<'_> {}

/// The head row of a signature: the revision and signer, the verdict as its icon.
fn head_row(s: &SignatureInfo, open: bool) -> ListRow<'static> {
    let (template, name) = match (s.signed, s.certify.is_some(), s.signer.as_deref()) {
        (false, ..) => ("Unsigned signature field: {name}", s.field.as_str()),
        (true, true, Some(signer)) => ("Rev. {rev}: Certified by {name}", signer),
        (true, true, None) => ("Rev. {rev}: Certified by an unknown signer", ""),
        (true, false, Some(signer)) => ("Rev. {rev}: Signed by {name}", signer),
        (true, false, None) => ("Rev. {rev}: Signed by an unknown signer", ""),
    };
    let head = crate::i18n::fmt(tl!(template), &[("rev", &s.revision.to_string()), ("name", name)]);
    let (icon, tone) = head_look(s);
    ListRow::new(head).depth(0).group(open).strong(true).icon(IconSpec::svg(icon)).tone(tone)
}

/// One line under a head. The buttons and the page link put what they ask for in `action`.
fn line_row<'a>(s: &SignatureInfo, child: &Child, action: &'a RefCell<Option<PanelAction>>) -> ListRow<'a> {
    let row = |label: Label| ListRow::new(label).depth(1);
    match child {
        Child::Sign => {
            let field = s.field.clone();
            row(Label::Empty).accessible_name(tl!("Sign this field…")).below(Button::new(tl!("Sign this field…")), move |out| {
                if out.clicked() {
                    *action.borrow_mut() = Some(PanelAction::Sign(field));
                }
            })
        }
        Child::Status => row(match s.status {
            SignatureStatus::Valid => tl!("Signature is valid:"),
            SignatureStatus::Unknown => tl!("Signature validity is UNKNOWN:"),
            SignatureStatus::Invalid => tl!("Signature is INVALID:"),
        }
        .into())
        .strong(true),
        Child::Detail(line) => row(format!("• {line}").into()).ink(RowInk::Muted),
        Child::Heading => row(tl!("Signature Details").into()).section_heading(),
        Child::Pair(key, value) => row(tl!(*key).into()).trailing(value.clone()),
        Child::Page(p) => {
            let text = crate::i18n::fmt(tl!("Field: {field} on page {p}"), &[("field", &s.field), ("p", &(p + 1).to_string())]);
            let p = *p;
            row(Label::Empty).accessible_name(text.clone()).below(Link::new(text), move |out| {
                if out.clicked() {
                    *action.borrow_mut() = Some(PanelAction::GoTo(p));
                }
            })
        }
        Child::Buttons => {
            let mut row = row(Label::Empty).accessible_name(tl!("Signature actions"));
            // Trust exactly the signer shown above (s.certificate), not the chain top the
            // document embedded: trusting an issuer or root is a separate, explicit step in
            // the Certificate Viewer's Trust tab, which names that certificate.
            if let Some(c) = s.certificate.as_ref()
                && s.status == SignatureStatus::Unknown
            {
                let c = c.clone();
                row = row.below(Button::new(tl!("Add to trusted certificates")), move |out| {
                    if out.clicked() {
                        *action.borrow_mut() = Some(PanelAction::Trust(Box::new(c)));
                    }
                });
            }
            let (len, revision) = (s.signed_len, s.revision);
            row = row.below(Button::new(tl!("View signed version")), move |out| {
                if out.clicked() {
                    *action.borrow_mut() = Some(PanelAction::ViewSigned { len, revision });
                }
            });
            if let Some(c) = &s.certificate {
                let mut chain = vec![c.clone()];
                chain.extend(s.chain.iter().filter(|x| x.raw != c.raw).cloned());
                row = row.below(Button::new(tl!("Show certificate…")), move |out| {
                    if out.clicked() {
                        *action.borrow_mut() = Some(PanelAction::ViewCertificate(chain));
                    }
                });
                let c = c.clone();
                row = row.below(Button::new(tl!("Export certificate…")), move |out| {
                    if out.clicked() {
                        *action.borrow_mut() = Some(PanelAction::ExportCertificate(Box::new(c)));
                    }
                });
            }
            row
        }
    }
}

/// The Signatures panel body.
pub(crate) fn panel(ui: &mut egui::Ui, sigs: &[SignatureInfo], open: &mut OpenSet<String>) -> Option<PanelAction> {
    let action = RefCell::new(None);
    Stack::new().show(ui, |ui, _| {
        if Button::new(tl!("Validate all")).show(ui).clicked() {
            *action.borrow_mut() = Some(PanelAction::Validate);
        }
        if sigs.is_empty() {
            Notice::new(Tone::Info, tl!("This document has no signatures.")).show(ui);
            return;
        }
        let mut ordered: Vec<(&SignatureInfo, Vec<Child>)> = sigs.iter().map(|s| (s, children(s))).collect();
        ordered.sort_by_key(|(s, _)| (!s.signed, s.revision));
        let heads: Vec<SigRow> = ordered.iter().map(|(s, _)| SigRow::Head(s.field.clone())).collect();
        let fields: Vec<String> = ordered.iter().map(|(s, _)| s.field.clone()).collect();
        let mut entries = Vec::new();
        for (s, lines) in &ordered {
            entries.push((SigRow::Head(s.field.clone()), 0, true));
            if open.is_open(&s.field) {
                entries.extend((0..lines.len()).map(|i| (SigRow::Line(s.field.clone(), i), 1, false)));
            }
        }
        let rows = Rows { entries, open };
        let find = |field: &str| ordered.iter().find(|(s, _)| s.field == field);
        let out = List::<SigRow>::new(tl!("Signatures")).tree(true).activate(ActivateOn::Click).natural_height(true).show_items(
            ui,
            &rows,
            &mut |key, _| match key {
                SigRow::Head(field) => match find(field) {
                    Some((s, _)) => head_row(s, rows.open.is_open(field)),
                    None => ListRow::new(Label::Empty),
                },
                SigRow::Line(field, i) => match find(field).and_then(|(s, lines)| lines.get(*i).map(|c| (s, c))) {
                    Some((s, child)) => line_row(s, child, &action),
                    None => ListRow::new(Label::Empty),
                },
            },
        );
        let _ = heads;
        // Alt on a disclosure opens or closes every head; clicking a head row toggles it too, as production did.
        for event in &out.events {
            match event {
                CollectionEvent::Open(change) => {
                    if let SigRow::Head(field) = &change.key {
                        open.apply(field, change.open, change.all, fields.iter());
                    }
                }
                CollectionEvent::Activate(SigRow::Head(field)) => {
                    let was = open.is_open(field);
                    open.set(field, !was);
                }
                _ => {}
            }
        }
    });
    action.into_inner()
}

/// Certificate Viewer: a chain (the end certificate first) and the tab shown.
#[derive(Clone, Debug)]
pub struct CertViewer {
    pub chain: Vec<Certificate>,
    pub selected: usize,
    pub tab: CertTab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CertTab {
    Summary,
    Details,
    Trust,
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

pub(super) fn key_usage(bits: u16) -> String {
    const NAMES: [&str; 9] = [
        "Digital Signature",
        "Non-Repudiation",
        "Key Encipherment",
        "Data Encipherment",
        "Key Agreement",
        "Certificate Signing",
        "CRL Signing",
        "Encipher Only",
        "Decipher Only",
    ];
    let used: Vec<&str> = NAMES.iter().enumerate().filter(|(i, _)| bits & (1 << i) != 0).map(|(_, n)| tl!(*n)).collect();
    if used.is_empty() { tl!("None").into() } else { used.join(", ") }
}

/// The text of an exported certificate (`.cer`, PEM).
pub fn certificate_pem(c: &Certificate) -> String {
    sign::x509::to_pem(c)
}
