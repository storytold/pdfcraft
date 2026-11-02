use super::dialog::title;
use super::{Align, Certificate, Color32, Layout, SignatureInfo, SignatureStatus, Tokens, icons, sign, theme, widgets};

/// The message bar for signed documents: (icon, colour, template, argument).
/// The template renders through [`crate::i18n::tr_fmt`] at the call site, so the bar follows
/// the UI language; names inside `argument` stay as they are.
pub(crate) fn banner(sigs: &[SignatureInfo]) -> Option<(&'static str, Color32, &'static str, String)> {
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
        return Some(("badge-check", Color32::from_rgb(0x14, 0x73, 0xE6), "Certified by {by}", by.unwrap_or_default()));
    }
    Some(if signed.iter().any(|s| s.status == SignatureStatus::Invalid) {
        ("circle-x", Color32::from_rgb(0xD7, 0x37, 0x3F), "At least one signature is invalid.", String::new())
    } else if signed.iter().all(|s| s.status == SignatureStatus::Valid) {
        ("circle-check", Color32::from_rgb(0x2D, 0x9D, 0x78), "Signed and all signatures are valid.", String::new())
    } else {
        ("triangle-alert", Color32::from_rgb(0xE6, 0x86, 0x19), "At least one signature has problems.", String::new())
    })
}

fn status_icon(s: &SignatureInfo) -> (&'static str, Color32) {
    if !s.signed {
        return ("pen-line", Color32::from_gray(0x80));
    }
    signature_status_style(s.status)
}

/// One look per signature verdict, shared by the Signatures panel and the page badges so
/// the two can't drift apart.
pub(crate) fn signature_status_style(status: SignatureStatus) -> (&'static str, Color32) {
    match status {
        SignatureStatus::Valid => ("circle-check", Color32::from_rgb(0x2D, 0x9D, 0x78)),
        SignatureStatus::Unknown => ("triangle-alert", Color32::from_rgb(0xE6, 0x86, 0x19)),
        SignatureStatus::Invalid => ("circle-x", Color32::from_rgb(0xD7, 0x37, 0x3F)),
    }
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

/// The Signatures panel body.
pub(crate) fn panel(ui: &mut egui::Ui, t: &Tokens, sigs: &[SignatureInfo], expanded: &mut Vec<String>) -> Option<PanelAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        if widgets::pill_button(ui, tl!("Validate all"), false).clicked() {
            action = Some(PanelAction::Validate);
        }
    });
    ui.add_space(6.0);
    if sigs.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.add(icons::image("signature", 32.0, t.text_faint));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(tl!("This document has no signatures.")).color(t.text_muted));
        });
        return action;
    }
    let mut ordered: Vec<&SignatureInfo> = sigs.iter().collect();
    ordered.sort_by_key(|s| (!s.signed, s.revision));
    for s in ordered {
        let open = expanded.contains(&s.field);
        let (icon, color) = status_icon(s);
        let (template, name) = match (s.signed, s.certify.is_some(), s.signer.as_deref()) {
            (false, ..) => ("Unsigned signature field: {name}", s.field.as_str()),
            (true, true, Some(signer)) => ("Rev. {rev}: Certified by {name}", signer),
            (true, true, None) => ("Rev. {rev}: Certified by an unknown signer", ""),
            (true, false, Some(signer)) => ("Rev. {rev}: Signed by {name}", signer),
            (true, false, None) => ("Rev. {rev}: Signed by an unknown signer", ""),
        };
        let head = crate::i18n::fmt(tl!(template), &[("rev", &s.revision.to_string()), ("name", name)]);
        let resp = ui
            .horizontal(|ui| {
                let chevron = if open { "chevron-down" } else { "chevron-right" };
                let toggle = icons::button(ui, chevron, 20.0, false, if open { "Collapse" } else { "Expand" }).clicked();
                ui.add(icons::image(icon, 16.0, color));
                let l = ui.add(egui::Label::new(egui::RichText::new(&head).font(theme::semibold(12.5)).color(t.text)).sense(egui::Sense::click()));
                toggle || l.clicked()
            })
            .inner;
        if resp {
            if open {
                expanded.retain(|f| f != &s.field);
            } else {
                expanded.push(s.field.clone());
            }
        }
        if !open {
            continue;
        }
        egui::Frame::NONE.inner_margin(egui::Margin { left: 44, right: 4, top: 2, bottom: 8 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            if !s.signed {
                if widgets::pill_button(ui, tl!("Sign this field…"), true).clicked() {
                    action = Some(PanelAction::Sign(s.field.clone()));
                }
                return;
            }
            ui.label(
                egui::RichText::new(match s.status {
                    SignatureStatus::Valid => tl!("Signature is valid:"),
                    SignatureStatus::Unknown => tl!("Signature validity is UNKNOWN:"),
                    SignatureStatus::Invalid => tl!("Signature is INVALID:"),
                })
                .font(theme::semibold(12.0)),
            );
            for detail in &s.details {
                // Each detail is a whole sentence of ours plus what the signature itself says; said
                // here in the panel's language, with the signature's own words left as they are.
                let line = detail.said(&crate::i18n::words);
                ui.add(egui::Label::new(egui::RichText::new(format!("• {line}")).font(theme::regular(11.5)).color(t.text_muted)).wrap());
            }
            ui.add_space(4.0);
            ui.label(egui::RichText::new(tl!("Signature Details")).font(theme::semibold(12.0)));
            let row = |ui: &mut egui::Ui, k: &str, v: &str| {
                ui.add(egui::Label::new(egui::RichText::new(format!("{}: {v}", tl!(k))).font(theme::regular(11.5))).wrap());
            };
            if let Some(r) = &s.reason {
                row(ui, "Reason", r);
            }
            if let Some(l) = &s.location {
                row(ui, "Location", l);
            }
            if let Some(d) = &s.date {
                row(ui, "Signing time", &sign::pdf::display_date(d));
            }
            if let Some(a) = &s.algorithm {
                row(ui, "Algorithm", a);
            }
            if let Some(c) = &s.certificate {
                row(ui, "Signer", &c.subject.display());
                row(ui, "Issued by", &c.issuer.display());
                row(ui, "Valid", &format!("{} to {}", c.not_before, c.not_after));
            }
            match s.page {
                Some(p) => {
                    let link = ui.add(
                        egui::Label::new(
                            egui::RichText::new(crate::i18n::fmt(
                                tl!("Field: {field} on page {p}"),
                                &[("field", &s.field), ("p", &(p + 1).to_string())],
                            ))
                            .font(theme::regular(11.5))
                            .color(t.accent_text)
                            .underline(),
                        )
                        .sense(egui::Sense::click()),
                    );
                    if link.clicked() {
                        action = Some(PanelAction::GoTo(p));
                    }
                }
                None => row(ui, "Field", &s.field),
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                // Trust exactly the signer shown above (s.certificate), not the chain top the
                // document embedded: trusting an issuer or root is a separate, explicit step in
                // the Certificate Viewer's Trust tab, which names that certificate.
                if let Some(c) = s.certificate.as_ref()
                    && s.status == SignatureStatus::Unknown
                    && ui.button(tl!("Add to trusted certificates")).clicked()
                {
                    action = Some(PanelAction::Trust(Box::new(c.clone())));
                }
                if ui.button(tl!("View signed version")).clicked() {
                    action = Some(PanelAction::ViewSigned { len: s.signed_len, revision: s.revision });
                }
                if let Some(c) = &s.certificate
                    && ui.button(tl!("Show certificate…")).clicked()
                {
                    let mut chain = vec![c.clone()];
                    chain.extend(s.chain.iter().filter(|x| x.raw != c.raw).cloned());
                    action = Some(PanelAction::ViewCertificate(chain));
                }
                if let Some(c) = &s.certificate
                    && ui.button(tl!("Export certificate…")).clicked()
                {
                    action = Some(PanelAction::ExportCertificate(Box::new(c.clone())));
                }
            });
        });
    }
    action
}

/// Certificate Viewer: a chain (the end certificate first) and the tab shown.
#[derive(Clone, Debug)]
pub struct CertViewer {
    pub chain: Vec<Certificate>,
    pub selected: usize,
    pub tab: CertTab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CertTab {
    Summary,
    Details,
    Trust,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

fn key_usage(bits: u16) -> String {
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

/// What the viewer asks for.
pub(crate) enum CertAction {
    Trust(Box<Certificate>),
    Export(Box<Certificate>),
}

/// The Certificate Viewer dialog: the chain on the left, the selected certificate's tabs.
pub(crate) fn cert_viewer(ui: &mut egui::Ui, v: &mut CertViewer, trusted: &[Certificate], t: &Tokens) -> (bool, Option<CertAction>) {
    title(ui, "Certificate Viewer");
    ui.label(egui::RichText::new(tl!("This dialog shows the details of a certificate and its chain.")).color(t.text_muted));
    ui.add_space(8.0);
    let mut action = None;
    v.selected = v.selected.min(v.chain.len().saturating_sub(1));
    ui.horizontal_top(|ui| {
        // The chain, root at the top (as Acrobat shows it).
        ui.vertical(|ui| {
            ui.set_width(200.0);
            for (depth, i) in (0..v.chain.len()).rev().enumerate() {
                let c = &v.chain[i];
                ui.horizontal(|ui| {
                    ui.add_space(depth as f32 * 12.0);
                    if ui.selectable_label(v.selected == i, c.display_name()).clicked() {
                        v.selected = i;
                    }
                });
            }
        });
        ui.add_space(16.0);
        ui.vertical(|ui| {
            ui.set_width(440.0);
            let Some(c) = v.chain.get(v.selected).cloned() else { return };
            ui.horizontal(|ui| {
                for (tab, label) in [(CertTab::Summary, tl!("Summary")), (CertTab::Details, tl!("Details")), (CertTab::Trust, tl!("Trust"))] {
                    if widgets::pill_button(ui, label, v.tab == tab).clicked() {
                        v.tab = tab;
                    }
                }
            });
            ui.add_space(8.0);
            let grid = |ui: &mut egui::Ui, rows: Vec<(&str, String)>| {
                egui::Grid::new(("cert-rows", v.tab as u8)).num_columns(2).spacing([12.0, 5.0]).show(ui, |ui| {
                    for (k, val) in rows {
                        ui.label(egui::RichText::new(tl_ctx!("certificate", k)).color(t.text_muted));
                        ui.add(egui::Label::new(val).wrap());
                        ui.end_row();
                    }
                });
            };
            match v.tab {
                CertTab::Summary => grid(
                    ui,
                    vec![
                        ("Issued to", c.subject.display()),
                        ("Issued by", c.issuer.display()),
                        ("Valid from", c.not_before.to_string()),
                        ("Valid to", c.not_after.to_string()),
                        ("Intended usage", c.key_usage.map(key_usage).unwrap_or_else(|| tl!("Any").to_string())),
                    ],
                ),
                CertTab::Details => grid(
                    ui,
                    vec![
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
                ),
                CertTab::Trust => {
                    let is_trusted = trusted.iter().any(|x| x.raw == c.raw);
                    let anchored = v.chain.iter().any(|x| trusted.iter().any(|y| y.raw == x.raw));
                    ui.label(if is_trusted {
                        tl!("This certificate is in your list of trusted certificates.")
                    } else if anchored {
                        tl!("This certificate is trusted through a certificate above it in the chain.")
                    } else {
                        tl!("This certificate is not trusted. Signatures made with it show an unknown identity.")
                    });
                    ui.add_space(8.0);
                    if !is_trusted && widgets::pill_button(ui, tl!("Add to Trusted Certificates"), false).clicked() {
                        action = Some(CertAction::Trust(Box::new(c.clone())));
                    }
                }
            }
            ui.add_space(8.0);
            if widgets::pill_button(ui, tl!("Export…"), false).clicked() {
                action = Some(CertAction::Export(Box::new(c.clone())));
            }
        });
    });
    ui.add_space(10.0);
    let mut close = false;
    ui.horizontal(|ui| ui.with_layout(Layout::right_to_left(Align::Center), |ui| close = widgets::pill_button(ui, tl!("OK"), true).clicked()));
    (close, action)
}

/// The text of an exported certificate (`.cer`, PEM).
pub fn certificate_pem(c: &Certificate) -> String {
    sign::x509::to_pem(c)
}
