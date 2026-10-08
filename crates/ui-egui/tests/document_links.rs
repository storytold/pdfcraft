//! Addresses a document asks to open (links, button URI actions, `app.launchURL`) wait for the
//! user's permission, and anything but a web or email address is refused (#90, #91).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{LinkOrigin, PdfCraftApp, PendingLink};

/// See `tests/view.rs`: WARP's shader JIT crashes when two devices compile at once on ARM64.
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu() -> std::sync::MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

const LINK_URL: &str = "https://example.org/from-a-link";
const BUTTON_URL: &str = "https://example.org/from-a-button";

/// One 300×400 page with a web link (x 50..250, y 300..350), a push button with a URI action
/// ("web") and a push button whose JavaScript asks to open a local program ("sneaky").
fn fixture() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R] /DA (/Helv 0 Tf 0 g) >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R] >>".to_string(),
        format!("<< /Type /Annot /Subtype /Link /Rect [50 300 250 350] /Border [0 0 0] /A << /S /URI /URI ({LINK_URL}) >> >>"),
        format!("<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (web) /Rect [20 200 100 220] /P 3 0 R /A << /S /URI /URI ({BUTTON_URL}) >> >>"),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (sneaky) /Rect [120 200 200 220] /P 3 0 R /AA << /U << /S /JavaScript /JS (app.launchURL\\(\"file:///C:/Windows/System32/calc.exe\", true\\);) >> >> >>".to_string(),
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

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("links.pdf", None, fixture()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(8);
    h
}

fn click_at(h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2) {
    h.hover_at(at);
    h.run_steps(2);
    h.drag_at(at);
    h.drop_at(at);
    h.run_steps(4);
}

/// Click the link on page 1, at the middle of its rectangle.
fn click_link(h: &mut Harness<'static, PdfCraftApp>) {
    let r = h.state().views[0].page_screen_rect(0).expect("page 1 on screen");
    let at = egui::pos2(r.min.x + r.width() * (150.0 / 300.0), r.min.y + r.height() * (1.0 - 325.0 / 400.0));
    click_at(h, at);
}

fn click_field(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    let at = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == name).unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen").center()
    };
    click_at(h, at);
}

fn pending(url: &str, origin: LinkOrigin) -> Option<PendingLink> {
    Some(PendingLink { url: url.into(), origin })
}

#[test]
fn a_link_asks_first_and_opens_when_allowed() {
    let _gpu = gpu();
    let mut h = harness();
    click_link(&mut h);
    assert_eq!(h.state().pending_link, pending(LINK_URL, LinkOrigin::Link));
    assert_eq!(h.state().last_opened_url, None, "nothing opens before the user answers");
    h.get_by_label("example.org");
    h.get_by_label("Open link").click();
    h.run_steps(3);
    assert_eq!(h.state().last_opened_url.as_deref(), Some(LINK_URL));
    assert_eq!(h.state().pending_link, None);
}

#[test]
fn cancel_opens_nothing() {
    let _gpu = gpu();
    let mut h = harness();
    click_link(&mut h);
    assert!(h.state().pending_link.is_some());
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().pending_link, None);
    assert_eq!(h.state().last_opened_url, None);
}

#[test]
fn escape_cancels() {
    let _gpu = gpu();
    let mut h = harness();
    click_link(&mut h);
    assert!(h.state().pending_link.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert_eq!(h.state().pending_link, None);
    assert_eq!(h.state().last_opened_url, None);
}

#[test]
fn a_button_uri_action_asks_first() {
    let _gpu = gpu();
    let mut h = harness();
    click_field(&mut h, "web");
    assert_eq!(h.state().pending_link, pending(BUTTON_URL, LinkOrigin::Button));
    assert_eq!(h.state().last_opened_url, None);
}

#[test]
fn a_button_script_cannot_open_a_local_program() {
    let _gpu = gpu();
    let mut h = harness();
    click_field(&mut h, "sneaky");
    assert_eq!(h.state().pending_link, None, "a file: address is never offered");
    assert_eq!(h.state().last_opened_url, None);
    let toast = h.state().toast.clone().expect("the user is told").0;
    assert!(toast.contains("“file:” address"), "{toast}");
}

#[test]
fn a_script_asks_first_and_only_once() {
    let _gpu = gpu();
    let mut h = harness();
    let id = h.state().views[0].id;
    let script = "app.launchURL('https://example.org/?q=' + 'first'); app.launchURL('https://example.org/second');";
    h.state_mut().run_button_script(id, "web", script);
    h.run_steps(3);
    assert_eq!(h.state().pending_link, pending("https://example.org/?q=first", LinkOrigin::Script), "one dialog, for the first request");
    assert_eq!(h.state().last_opened_url, None);
}

#[test]
fn a_script_cannot_open_other_kinds_of_address() {
    let _gpu = gpu();
    for url in ["file:///etc/passwd", "javascript:alert(1)", "ms-settings:privacy", "smb://server/share", "https://exa\u{202E}gro.elpmaxe"] {
        let mut h = harness();
        let id = h.state().views[0].id;
        let script = format!("app.launchURL({});", serde_json::to_string(url).unwrap());
        h.state_mut().run_button_script(id, "web", &script);
        h.run_steps(3);
        assert_eq!(h.state().pending_link, None, "{url}");
        assert_eq!(h.state().last_opened_url, None, "{url}");
        assert!(h.state().toast.clone().is_some_and(|t| t.0.contains("won't open")), "{url}: {:?}", h.state().toast);
    }
}
