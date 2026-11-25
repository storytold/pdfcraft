//! The Certificate Viewer dialog: the chain as a tree, and the selected certificate's tabs.

use craft::behavior::{DisplayOrder, ItemSource, Selection, SelectionChange};
use craft::ui::buttons::Button;
use craft::ui::contract::{Bind, CollectionEvent, SelectMode, Show as _, ShowCollection as _, ShowContainer as _};
use craft::ui::dialog::Dialog;
use craft::ui::display::Text;
use craft::ui::form::Field;
use craft::ui::layout_components::{Row, Stack};
use craft::ui::list::{List, ListRow};
use craft::ui::runtime::env::Label;
use craft::ui::sections::{DialogRole, FooterButton};
use craft::ui::tabs::{Tab, TabKind, TabStrip};
use craft::ui::text::SelectableText;

use super::panel::{CertTab, CertViewer, hex, key_usage};
use super::{Certificate, PdfCraftApp, sign};

/// Opens the viewer on `chain` (the signer first), with the signer selected.
pub(crate) fn open(app: &mut PdfCraftApp, chain: Vec<Certificate>) {
    app.cert_viewer = Some(CertViewer { chain, selected: 0, tab: CertTab::Summary });
}

/// The chain as a tree, root first: the entry at `at` is the certificate `len - 1 - at` of the chain, `at` levels
/// below the root, and every group is open.
struct Chain {
    len: usize,
}

impl DisplayOrder<usize> for Chain {
    fn len(&self) -> usize {
        self.len
    }

    fn key(&self, at: usize) -> usize {
        self.len - 1 - at
    }

    fn index_of(&self, key: &usize) -> Option<usize> {
        (*key < self.len).then(|| self.len - 1 - key)
    }

    fn depth(&self, at: usize) -> usize {
        at
    }

    fn has_children(&self, at: usize) -> bool {
        at + 1 < self.len
    }

    fn open(&self, _at: usize) -> bool {
        true
    }
}

impl ItemSource<usize> for Chain {}

/// The tabs, in order.
struct Tabs;

const TABS: [CertTab; 3] = [CertTab::Summary, CertTab::Details, CertTab::Trust];

impl DisplayOrder<CertTab> for Tabs {
    fn len(&self) -> usize {
        TABS.len()
    }

    fn key(&self, at: usize) -> CertTab {
        TABS[at]
    }
}

impl ItemSource<CertTab> for Tabs {}

fn tab_label(tab: CertTab) -> &'static str {
    match tab {
        CertTab::Summary => tl!("Summary"),
        CertTab::Details => tl!("Details"),
        CertTab::Trust => tl!("Trust"),
    }
}

/// The name and value rows of the Summary and Details tabs.
fn rows(tab: CertTab, c: &Certificate) -> Vec<(&'static str, String)> {
    match tab {
        CertTab::Summary => vec![
            ("Issued to", c.subject.display()),
            ("Issued by", c.issuer.display()),
            ("Valid from", c.not_before.to_string()),
            ("Valid to", c.not_after.to_string()),
            ("Intended usage", c.key_usage.map(key_usage).unwrap_or_else(|| tl!("Any").to_string())),
        ],
        CertTab::Details => vec![
            ("Version", "3".into()),
            ("Serial number", c.serial_hex()),
            ("Issuer", c.issuer.display()),
            ("Subject", c.subject.display()),
            ("Validity starts", c.not_before.to_string()),
            ("Validity ends", c.not_after.to_string()),
            ("Public key", c.public_key.describe()),
            ("Basic constraints", tl!(if c.is_ca { "Certificate authority" } else { "End entity" }).to_string()),
            ("Key usage", c.key_usage.map(key_usage).unwrap_or_else(|| tl!("Not present").to_string())),
            ("Self-signed", if c.is_self_signed() { tl!("Yes").to_string() } else { tl!("No").to_string() }),
            ("SHA-1 digest", hex(&sign::keys::DigestAlg::Sha1.digest(&[&c.raw]))),
            ("SHA-256 digest", hex(&sign::keys::DigestAlg::Sha256.digest(&[&c.raw]))),
        ],
        CertTab::Trust => Vec::new(),
    }
}

/// The viewer's content: the chain beside the selected certificate's tabs. A request to trust the selected
/// certificate goes to `trust`.
fn body(ui: &mut egui::Ui, v: &mut CertViewer, trusted: &[Certificate], trust: &mut Option<Certificate>) {
    Text::new(tl!("This dialog shows the details of a certificate and its chain.")).show(ui);
    Row::new().show(ui, |ui, _| {
        let len = v.chain.len();
        let mut selection = Selection::single(v.selected);
        let out = List::<usize>::new(tl!("Certificate chain"))
            .tree(true)
            .select(SelectMode::Single)
            .natural_height(true)
            .selection(Bind::bound(&mut selection))
            .show_items(ui, &Chain { len }, &mut |key, _| {
                let name = v.chain.get(*key).map(Certificate::display_name).unwrap_or_default();
                let row = ListRow::new(name).depth(len - 1 - key);
                if *key > 0 { row.group(true) } else { row }
            });
        for event in &out.events {
            if let CollectionEvent::Selection(SelectionChange::Set(keys)) = event
                && let Some(key) = keys.last()
            {
                v.selected = *key;
            }
        }
        let Some(c) = v.chain.get(v.selected).cloned() else { return };
        Stack::new().show(ui, |ui, _| {
            let out = TabStrip::new(tl!("Certificate details"))
                .kind(TabKind::Text)
                .shown(v.tab)
                .show_items(ui, &Tabs, &mut |tab, _| Tab::new(tab_label(*tab)));
            for event in out.response.events {
                if let CollectionEvent::Selection(SelectionChange::Set(keys)) = event
                    && let Some(tab) = keys.into_iter().last()
                {
                    v.tab = tab;
                }
            }
            match v.tab {
                CertTab::Trust => {
                    let is_trusted = trusted.iter().any(|x| x.raw == c.raw);
                    let anchored = v.chain.iter().any(|x| trusted.iter().any(|y| y.raw == x.raw));
                    Text::new(if is_trusted {
                        tl!("This certificate is in your list of trusted certificates.")
                    } else if anchored {
                        tl!("This certificate is trusted through a certificate above it in the chain.")
                    } else {
                        tl!("This certificate is not trusted. Signatures made with it show an unknown identity.")
                    })
                    .show(ui);
                    if !is_trusted && Button::new(tl!("Add to Trusted Certificates")).show(ui).clicked() {
                        *trust = Some(c.clone());
                    }
                }
                tab => {
                    for (key, value) in rows(tab, &c) {
                        let name: Label = tl_ctx!("certificate", key).into();
                        Field::new(name.clone()).show(ui, |ui, _| {
                            SelectableText::new(name.clone(), &value).multiline().show(ui);
                        });
                    }
                }
            }
        });
    });
}

/// Shows the Certificate Viewer while the app holds one. Export… keeps it open; OK, Escape and the close action
/// drop it.
pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let trusted = app.session.trusted_certificates().to_vec();
    let Some(v) = app.cert_viewer.as_mut() else { return };
    v.selected = v.selected.min(v.chain.len().saturating_sub(1));
    let mut trust = None;
    let shown = Dialog::new(tl!("Certificate Viewer"))
        .buttons([FooterButton::new(tl!("Export…"), DialogRole::Other).stays_open(true), FooterButton::new(tl!("OK"), DialogRole::Accept)])
        .show(ui, |ui, _| body(ui, v, &trusted, &mut trust));
    let export = if shown.output.picked == Some(0) { v.chain.get(v.selected).cloned() } else { None };
    let close = shown.output.closed.is_some();
    if let Some(c) = trust {
        app.trust_certificate(c);
    }
    if let Some(c) = export {
        app.export_certificate(&c);
    }
    if close {
        app.cert_viewer = None;
    }
}
