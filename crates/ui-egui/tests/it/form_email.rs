//! A form whose submit action is an email address (`mailto:`) opens a new message in the user's
//! email app, after offering to save the document for them to attach; nothing is sent (#7693).

use crate::support;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{LinkOrigin, PdfCraftApp, PendingLink};

use crate::gpu;

const MAIL: &str = "mailto:forms@example.org?subject=Ref%2042%20Form%20Response&body=Please%20find%20the%20completed%20form%20attached.";
/// The same address as a URL file specification (`/F << /FS /URL /F (…) >>`).
const MAIL_SPEC: &str = "mailto:office@example.org?subject=Spec";

/// One 300×400 page with push buttons whose mouse-up action submits the form: by email ("email"),
/// by email through a URL file specification ("spec"), to a web address ("web") and by an email
/// address that asks to attach a local file ("attach").
fn fixture() -> Vec<u8> {
    let button = |name: &str, x: u32, target: &str| {
        format!(
            "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T ({name}) /Rect [{x} 200 {} 220] /P 3 0 R /A << /S /SubmitForm /F {target} >> >>",
            x + 50
        )
    };
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R 7 0 R] /DA (/Helv 0 Tf 0 g) >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R 7 0 R] >>".to_string(),
        button("email", 10, &format!("({MAIL})")),
        button("spec", 80, &format!("<< /FS /URL /F ({MAIL_SPEC}) >>")),
        button("web", 150, "(https://example.org/submit)"),
        button("attach", 220, "(mailto:forms@example.org?attach=/etc/passwd)"),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// The form opened from `path` (a file on disk), or from bytes alone.
fn harness(path: Option<String>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", path, fixture()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(8);
    h
}

fn click_field(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    let at = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == name).unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen").center()
    };
    h.hover_at(at);
    h.run_steps(2);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(4);
}

fn submission(h: &Harness<'static, PdfCraftApp>, url: &str, origin: LinkOrigin) -> Option<PendingLink> {
    Some(PendingLink { url: url.into(), origin, submit: Some(h.state().views[0].id) })
}

fn toast(h: &Harness<'static, PdfCraftApp>) -> String {
    h.state().toast.clone().map(|t| t.0).unwrap_or_default()
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-form-email-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn an_email_submission_shows_who_gets_the_form_and_offers_to_save_it_first() {
    let _gpu = gpu();
    let mut h = harness(None);
    click_field(&mut h, "email");
    assert_eq!(h.state().pending_link, submission(&h, MAIL, LinkOrigin::Button));
    assert_eq!(h.state().last_opened_url, None, "nothing opens before the user answers");
    h.run_steps(2);
    for label in ["Email this form?", "forms@example.org", "Ref 42 Form Response", "Please find the completed form attached."] {
        assert!(h.query_by_label(label).is_some(), "{label} is shown");
    }
    // A document opened from bytes has no file to attach yet.
    assert!(h.query_by_label("Save and open email app").is_some());
    h.get_by_label("Open email app without saving").click();
    h.run_steps(2);
    assert_eq!(h.state().pending_link, None);
    assert_eq!(h.state().last_opened_url.as_deref(), Some(MAIL), "the document's address goes to the email app unchanged");
    let said = toast(&h);
    assert!(said.contains("Save form.pdf, then attach it"), "{said}");
}

#[test]
fn save_and_open_writes_the_form_before_the_email_opens() {
    let _gpu = gpu();
    let dir = temp_dir("save");
    let out = dir.join("filled.pdf");
    let mut h = harness(None);
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    click_field(&mut h, "spec");
    assert_eq!(h.state().pending_link, submission(&h, MAIL_SPEC, LinkOrigin::Button), "a URL file specification is read too");
    h.run_steps(2);
    h.get_by_label("Save and open email app").click();
    support::wait_until(&mut h, "the save and the email", |app| app.last_opened_url.is_some());
    assert!(std::fs::read(&out).unwrap().starts_with(b"%PDF-"), "the form is written first");
    assert_eq!(h.state().last_opened_url.as_deref(), Some(MAIL_SPEC));
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert!(!doc.dirty);
    let said = toast(&h);
    // The file written, not the document's name before the save (a web download after Apply
    // redactions is named with `_Redacted`).
    assert!(said.contains("Attach filled.pdf to the new email"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_saved_form_opens_the_email_without_offering_to_save() {
    let _gpu = gpu();
    let dir = temp_dir("saved");
    let path = dir.join("form.pdf");
    std::fs::write(&path, fixture()).unwrap();
    let mut h = harness(Some(path.to_string_lossy().into_owned()));
    click_field(&mut h, "email");
    h.run_steps(2);
    assert!(h.query_by_label("Save and open email app").is_none(), "nothing to save");
    h.get_by_label("Open email app").click();
    h.run_steps(2);
    assert_eq!(h.state().last_opened_url.as_deref(), Some(MAIL));
    let said = toast(&h);
    assert!(said.contains("Attach form.pdf to the new email"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_saved_form_with_unsaved_changes_offers_to_save_first() {
    let _gpu = gpu();
    let dir = temp_dir("edited");
    let path = dir.join("form.pdf");
    std::fs::write(&path, fixture()).unwrap();
    let mut h = harness(Some(path.to_string_lossy().into_owned()));
    let id = h.state().views[0].id;
    h.state_mut().session.apply(id, pdfcraft_engine::Edit::InsertBlankPage { at: 1, width: 300.0, height: 400.0 }).unwrap();
    h.run_steps(2);
    click_field(&mut h, "email");
    h.run_steps(2);
    assert!(h.query_by_label("Save and open email app").is_some(), "the file on disk lacks the change");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancel_opens_no_email() {
    let _gpu = gpu();
    let mut h = harness(None);
    click_field(&mut h, "email");
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(h.state().pending_link, None);
    assert_eq!(h.state().last_opened_url, None);
}

#[test]
fn a_web_submission_is_only_explained() {
    let _gpu = gpu();
    let mut h = harness(None);
    click_field(&mut h, "web");
    assert_eq!(h.state().pending_link, None, "form data is never sent to a web address");
    let said = toast(&h);
    assert!(said.contains("web submits the form to https://example.org/submit; PdfCraft doesn't send form data"), "{said}");
}

#[test]
fn an_email_submission_that_attaches_a_local_file_is_refused() {
    let _gpu = gpu();
    let mut h = harness(None);
    click_field(&mut h, "attach");
    assert_eq!(h.state().pending_link, None);
    let said = toast(&h);
    assert!(said.contains("attach or insert a file"), "{said}");
}

#[test]
fn a_script_submitting_by_email_asks_the_same_way() {
    let _gpu = gpu();
    let mut h = harness(None);
    let id = h.state().views[0].id;
    h.state_mut().run_button_script(id, "web", "this.submitForm({ cURL: 'mailto:forms@example.org?subject=From%20a%20script' });");
    support::wait_until(&mut h, "script submission", |app| app.pending_link.is_some());
    assert_eq!(h.state().pending_link, submission(&h, "mailto:forms@example.org?subject=From%20a%20script", LinkOrigin::Script));
    // A script's web submission is still only explained.
    h.state_mut().pending_link = None;
    h.state_mut().run_button_script(id, "web", "this.submitForm('https://example.org/form');");
    support::wait_until(&mut h, "script notice", |app| app.toast.is_some());
    assert_eq!(h.state().pending_link, None);
    let said = toast(&h);
    assert!(said.contains("PdfCraft doesn't send form data"), "{said}");
}
