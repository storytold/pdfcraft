//! Snapshot tests of every side panel (M3): each panel opens on one synthetic document and its
//! AccessKit tree (the roles, labels, values and states a screen reader gets) is compared with a
//! text file in `tests/snapshots/`. Text rather than pixels: with the reported OS pinned (shortcut
//! labels follow it) it is the same on every platform and GPU, it is not an asset (AGENTS.md §1),
//! and a reviewer can read a change in the diff. Positions are left out, so moving a widget is not
//! a change but adding, removing or renaming one is.
//!
//! `UPDATE_SNAPSHOTS=1 cargo test -p pdfcraft-ui-egui --test panel_snapshots` rewrites the files.

use egui::accesskit::{Role, Toggled};
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::{Harness, Node};
use pdfcraft_engine::Session;
use pdfcraft_ui_egui::{PdfCraftApp, RightPanel};

mod support;

/// Two pages with something for every panel: bookmarks, a layer, an attachment, a text field, an
/// unsigned signature field, a comment and a structure tree.
fn fixture() -> Vec<u8> {
    let page1 = "/P << /MCID 0 >> BDC BT /F1 18 Tf 72 720 Td (Panel sample) Tj ET EMC\n\
                 /OC /L1 BDC BT /F1 10 Tf 72 690 Td (Draft notes) Tj ET EMC\n";
    let page2 = "BT /F1 12 Tf 72 720 Td (Second page sample text) Tj ET\n";
    let objs: Vec<String> = vec![
        // 1 catalog
        "<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R /OCProperties << /OCGs [9 0 R] /D << /ON [9 0 R] /Order [9 0 R] >> >> \
         /Names << /EmbeddedFiles << /Names [(notes.txt) 11 0 R] >> >> /AcroForm << /Fields [12 0 R 13 0 R] >> \
         /MarkInfo << /Marked true >> /StructTreeRoot 15 0 R >>"
            .into(),
        // 2 page tree, 3 and 4 its pages
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 612 792] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 17 0 R /StructParents 0 /Resources << /Font << /F1 5 0 R >> /Properties << /L1 9 0 R >> >> \
         /Annots [12 0 R 13 0 R 14 0 R] >>"
            .into(),
        "<< /Type /Page /Parent 2 0 R /Contents 18 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        // 5 font
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        // 6 outline, 7 and 8 its items
        "<< /Type /Outlines /First 7 0 R /Last 8 0 R /Count 2 >>".into(),
        "<< /Title (Introduction) /Parent 6 0 R /Next 8 0 R /Dest [3 0 R /Fit] >>".into(),
        "<< /Title (Details) /Parent 6 0 R /Prev 7 0 R /Dest [4 0 R /Fit] >>".into(),
        // 9 layer
        "<< /Type /OCG /Name (Draft notes) >>".into(),
        // 10 embedded file, 11 its file specification
        "<< /Type /EmbeddedFile /Length 5 >>\nstream\nhello\nendstream".into(),
        "<< /Type /Filespec /F (notes.txt) /UF (notes.txt) /EF << /F 10 0 R >> >>".into(),
        // 12 text field, 13 signature field, 14 comment
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [72 600 272 624] /P 3 0 R /F 4 >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [72 520 272 560] /P 3 0 R /F 4 >>".into(),
        "<< /Type /Annot /Subtype /Text /Rect [400 700 420 720] /P 3 0 R /T (Reviewer) /Contents (Check this heading) \
         /M (D:20260101120000Z) /CreationDate (D:20260101120000Z) >>"
            .into(),
        // 15 structure tree root, 16 its paragraph
        "<< /Type /StructTreeRoot /K [16 0 R] /ParentTree << /Nums [0 [16 0 R]] >> >>".into(),
        "<< /S /P /P 15 0 R /Pg 3 0 R /K 0 >>".into(),
        // 17 and 18 page contents
        format!("<< /Length {} >>\nstream\n{page1}endstream", page1.len()),
        format!("<< /Length {} >>\nstream\n{page2}endstream", page2.len()),
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

fn harness(panel: &'static str) -> Harness<'static, PdfCraftApp> {
    let bytes = fixture();
    // Shortcut labels follow the reported OS ("Ctrl+B" against "⌘B"), so pin it rather than rely
    // on kittest's default; the snapshots then match on every host.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_os(egui::os::OperatingSystem::Nix).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("panels.pdf", None, bytes).expect("opens");
        app.set_option("panel", panel).unwrap();
        app
    });
    h.run_steps(6);
    h
}

/// The side panel's contents: the dock node after the Inspector tab.
fn panel<'h>(h: &'h Harness<'static, PdfCraftApp>) -> Node<'h> {
    let tab = h.get_by(|n| n.role() == Role::Tab && n.label().as_deref() == Some("Inspector"));
    let dock = tab.parent().expect("the Inspector tab sits in a dock node");
    let mut after = dock.children().skip_while(|n| n.accesskit_node().id() != tab.accesskit_node().id()).skip(1);
    after.next().expect("the panel follows its tab")
}

/// One line per node that says something: layout containers are skipped (their children move up a
/// level) and text runs repeat their label.
fn describe(node: &Node<'_>, depth: usize, out: &mut String) {
    let n = node.accesskit_node();
    let quiet = n.role() == Role::GenericContainer && n.label().is_none() && n.value().is_none();
    let next = if quiet || n.role() == Role::TextRun {
        depth
    } else {
        let mut line = format!("{:indent$}{:?}", "", n.role(), indent = depth * 2);
        if let Some(l) = n.label() {
            line.push_str(&format!(" {l:?}"));
        }
        if let Some(v) = n.value().filter(|v| !v.is_empty() && Some(v) != n.label().as_ref()) {
            line.push_str(&format!(" = {v:?}"));
        }
        match n.toggled() {
            Some(Toggled::True) => line.push_str(" [on]"),
            Some(Toggled::False) => line.push_str(" [off]"),
            Some(Toggled::Mixed) => line.push_str(" [mixed]"),
            None => {}
        }
        if n.is_selected() == Some(true) {
            line.push_str(" [selected]");
        }
        if n.is_disabled() {
            line.push_str(" [disabled]");
        }
        out.push_str(&line);
        out.push('\n');
        depth + 1
    };
    if n.role() == Role::TextRun {
        return;
    }
    for c in node.children() {
        describe(&c, next, out);
    }
}

/// Compares the panel with `tests/snapshots/panel_<name>.txt`, or writes it with `UPDATE_SNAPSHOTS`.
fn check(h: &Harness<'static, PdfCraftApp>, name: &str) -> Result<(), String> {
    let mut got = String::new();
    describe(&panel(h), 0, &mut got);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots").join(format!("panel_{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &got).unwrap();
        return Ok(());
    }
    match std::fs::read_to_string(&path) {
        Ok(want) if want == got => Ok(()),
        Ok(want) => Err(format!(
            "{name} panel differs from {}
--- want
{want}
--- got
{got}",
            path.display()
        )),
        Err(e) => Err(format!(
            "{}: {e}
--- got
{got}",
            path.display()
        )),
    }
}

/// Every panel, in the order of the rail.
const ALL: [RightPanel; 11] = [
    RightPanel::Comments,
    RightPanel::Bookmarks,
    RightPanel::Pages,
    RightPanel::Fields,
    RightPanel::Layers,
    RightPanel::Attachments,
    RightPanel::Signatures,
    RightPanel::Accessibility,
    RightPanel::Tags,
    RightPanel::Search,
    RightPanel::Compare,
];

/// Opens `p` with something in it and names its snapshot. No wildcard on purpose: a new
/// `RightPanel` stops this file compiling until it is set up here (and listed in [`ALL`]).
fn open(p: RightPanel) -> (Harness<'static, PdfCraftApp>, &'static str) {
    let name = match p {
        RightPanel::Comments => "comments",
        RightPanel::Bookmarks => "bookmarks",
        RightPanel::Pages => "pages",
        RightPanel::Fields => "fields",
        RightPanel::Layers => "layers",
        RightPanel::Attachments => "attachments",
        RightPanel::Signatures => "signatures",
        RightPanel::Tags => "tags",
        RightPanel::Accessibility => {
            let mut h = harness("none");
            assert!(h.state_mut().execute("a11y.check"));
            h.run_steps(2);
            h.get_by_label("Start Checking").click();
            h.run_steps(3);
            return (h, "accessibility");
        }
        RightPanel::Search => {
            let mut h = harness("none");
            assert!(h.state_mut().execute("edit.advanced_search"));
            h.run_steps(2);
            h.state_mut().views[0].find.as_mut().expect("Advanced Search opens a search").query = "sample".into();
            support::wait_until(&mut h, "both pages searched", |app| app.views[0].find.as_ref().is_some_and(|f| f.matches.len() == 2));
            h.run_steps(2);
            return (h, "search");
        }
        RightPanel::Compare => {
            let s = Session::new();
            let old = s.create_from_text("t", "Delivery within five days. Returns accepted.").unwrap().to_vec();
            let new = s.create_from_text("t", "Delivery within three days. Returns accepted for a week.").unwrap().to_vec();
            let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
                let mut app = PdfCraftApp::new();
                app.set_option("language", "en").unwrap();
                app.open_bytes("old.pdf", None, old.clone()).unwrap();
                app.open_bytes("new.pdf", None, new.clone()).unwrap();
                app
            });
            h.run_steps(4);
            assert!(h.state_mut().execute("doc.compare"));
            h.run_steps(2);
            h.get_by_label("Compare").click();
            h.run_steps(3);
            return (h, "compare");
        }
    };
    (harness(name), name)
}

/// Every panel matches its snapshot. All of them are checked before failing, so one run shows
/// every panel a change touched.
#[test]
fn every_panel_matches_its_snapshot() {
    let mut failures = Vec::new();
    for p in ALL {
        let (h, name) = open(p);
        assert_eq!(h.state().right, Some(p), "{name} opens its own panel");
        if let Err(e) = check(&h, name) {
            failures.push(e);
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} panels differ (UPDATE_SNAPSHOTS=1 rewrites them)

{}",
        failures.len(),
        ALL.len(),
        failures.join(
            "

"
        )
    );
}
