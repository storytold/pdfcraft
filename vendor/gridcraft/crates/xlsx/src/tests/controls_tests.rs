//! Form controls: the three parts written for each control agree, they read back, and files with
//! only VML or with odd values give valid controls.

use gridcraft_core::CellRef;
use gridcraft_model::{Anchor, AnchorMode, Comment, ControlAction, ControlKind, FormControl, MacroInput, Sheet, Workbook};

use super::minimal;
use crate::package::Package;
use crate::{read_xlsx, write_xlsx};

fn at(a: &str) -> CellRef {
    CellRef::parse(a).unwrap()
}

fn anchor(cell: &str, width: f32, height: f32) -> Anchor {
    Anchor { cell: at(cell), dx: 4.0, dy: 3.0, width, height, mode: AnchorMode::MoveOnly }
}

fn note(author: &str, text: &str) -> Comment {
    Comment {
        author: author.into(),
        text: text.into(),
        replies: vec![],
        threaded: false,
        resolved: false,
        visible: false,
        dates: vec![],
        note_box: None,
    }
}

fn book() -> Workbook {
    let mut wb = Workbook::default();
    wb.sheets.clear();
    let mut sh = Sheet::new("Data");
    sh.comments.insert(at("A1"), note("Me", "a note"));
    sh.controls.push(FormControl {
        id: 1,
        kind: ControlKind::Slider,
        anchor: anchor("C3", 150.0, 20.0),
        link: Some("$B$1".into()),
        min: 5,
        max: 500,
        step: 5,
        page: 50,
        value: 35,
        name: "Rate".into(),
        ..Default::default()
    });
    sh.controls.push(FormControl {
        id: 2,
        kind: ControlKind::Spin,
        anchor: Anchor { mode: AnchorMode::Absolute, ..anchor("F3", 20.0, 40.0) },
        link: Some("'Other Sheet'!$A$2".into()),
        max: 30_000,
        step: 100,
        value: 30_000,
        print: false,
        ..Default::default()
    });
    sh.controls.push(FormControl {
        id: 3,
        kind: ControlKind::Button,
        anchor: Anchor { mode: AnchorMode::MoveAndSize, ..anchor("C8", 100.0, 28.0) },
        text: "Run <it> & go".into(),
        action: Some(ControlAction::Macro { name: "Fill Down".into(), input: MacroInput::Selection, output: None }),
        locked: false,
        ..Default::default()
    });
    sh.controls.push(FormControl { id: 4, kind: ControlKind::Slider, anchor: anchor("H3", 20.0, 120.0), horizontal: false, ..Default::default() });
    wb.sheets.push(sh.into());
    wb.sheets.push(Sheet::new("Other Sheet").into());
    wb
}

fn text(pkg: &mut Package<'_>, part: &str) -> String {
    String::from_utf8(pkg.read(part).unwrap().unwrap_or_else(|| panic!("missing {part}"))).unwrap()
}

#[test]
fn the_three_parts_agree_and_read_back() {
    let wb = book();
    let bytes = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&bytes).unwrap();
    let sheet = text(&mut pkg, "xl/worksheets/sheet1.xml");
    let vml = text(&mut pkg, "xl/drawings/vmlDrawing1.vml");
    let types = text(&mut pkg, "[Content_Types].xml");
    assert!(sheet.contains("<mc:Choice xmlns:x14=") && sheet.contains("<controls>"), "{sheet}");
    // Each control's shape id is a VML shape of the same kind; the note keeps its own id.
    let ids: Vec<String> = sheet.split("<control shapeId=\"").skip(1).map(|s| s.split('"').next().unwrap().to_string()).collect();
    assert_eq!(ids.len(), 8, "a Choice and a Fallback per control");
    for (i, kind) in ["Scroll", "Spin", "Button", "Scroll"].iter().enumerate() {
        let id = &ids[i * 2];
        let shape = vml.split(&format!("id=\"_x0000_s{id}\"")).nth(1).unwrap_or_else(|| panic!("no shape {id}"));
        assert!(shape.starts_with(" type=\"#_x0000_t201\""), "{shape}");
        assert!(shape.split("</v:shape>").next().unwrap().contains(&format!("ObjectType=\"{kind}\"")), "{id}");
    }
    assert!(vml.contains("id=\"_x0000_s1025\"") && vml.contains("ObjectType=\"Note\""), "the note is shape 1025");
    assert!(!ids.contains(&"1025".to_string()));
    for n in 1..=4 {
        assert!(types.contains(&format!("/xl/ctrlProps/ctrlProp{n}.xml")), "content type {n}");
    }
    let prop = text(&mut pkg, "xl/ctrlProps/ctrlProp1.xml");
    assert!(
        prop.contains("objectType=\"Scroll\"") && prop.contains("fmlaLink=\"$B$1\"") && prop.contains("page=\"50\"") && prop.contains("horiz=\"1\""),
        "{prop}"
    );
    assert!(sheet.contains("macro=\"[0]!Fill Down\""));

    let (back, report) = read_xlsx(&bytes).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let got = &back.sheet(0).unwrap().controls;
    assert_eq!(got.len(), 4);
    for (a, b) in wb.sheet(0).unwrap().controls.iter().zip(got) {
        let same = |x: &FormControl| {
            (x.kind, x.link.clone(), x.min, x.max, x.step, x.value, x.horizontal, x.print, x.locked, x.action.clone(), x.anchor.cell, x.anchor.mode)
        };
        assert_eq!(same(a), same(b));
        assert!(
            (a.anchor.width - b.anchor.width).abs() < 1.0
                && (a.anchor.height - b.anchor.height).abs() < 1.0
                && (a.anchor.dx - b.anchor.dx).abs() < 1.0
        );
        if a.kind == ControlKind::Slider {
            assert_eq!(a.page, b.page);
        }
    }
    assert_eq!(got[0].name, "Rate");
    assert_eq!(got[2].text, "Run <it> & go");
    assert_eq!(back.sheet(0).unwrap().comments.len(), 1, "notes still read");
}

#[test]
fn many_shapes_take_several_id_blocks() {
    let mut wb = Workbook::default();
    wb.sheets.clear();
    let mut sh = Sheet::new("Data");
    for i in 0..1100u32 {
        sh.controls.push(FormControl { id: i + 1, kind: ControlKind::Spin, anchor: anchor("C3", 20.0, 30.0), ..Default::default() });
    }
    wb.sheets.push(sh.into());
    wb.sheets.push(Sheet::new("Next").into());
    if let Some(s) = wb.sheet_mut(1) {
        s.comments.insert(at("B2"), note("x", "y"));
    }
    let bytes = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&bytes).unwrap();
    assert!(text(&mut pkg, "xl/drawings/vmlDrawing1.vml").contains("data=\"1,2\""));
    let second = text(&mut pkg, "xl/drawings/vmlDrawing2.vml");
    assert!(second.contains("data=\"3\"") && second.contains("_x0000_s3073"), "the next sheet's ids don't collide");
    let (back, _) = read_xlsx(&bytes).unwrap();
    assert_eq!(back.sheet(0).unwrap().controls.len(), 1100);
}

const VML_ONLY: &str = r##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel">
<v:shape id="_x0000_s1025" type="#_x0000_t201"><x:ClientData ObjectType="Scroll"><x:Anchor>2, 0, 3, 0, 5, 0, 4, 0</x:Anchor><x:FmlaLink>$B$1</x:FmlaLink><x:Val>50000</x:Val><x:Min>-5</x:Min><x:Max>99999</x:Max><x:Inc>0</x:Inc><x:Page>-3</x:Page><x:Horiz/></x:ClientData></v:shape>
<v:shape id="_x0000_s1026" type="#_x0000_t201"><x:ClientData ObjectType="Spin"><x:Anchor>1, 0, 1, 0, 1, 20, 3, 0</x:Anchor><x:Val>7</x:Val><x:Min>9</x:Min><x:Max>9</x:Max></x:ClientData></v:shape>
<v:shape id="_x0000_s1027" o:button="t"><v:textbox><div>Go</div></v:textbox><x:ClientData ObjectType="Button"><x:Anchor>1, 0, 6, 0, 3, 0, 7, 0</x:Anchor><x:FmlaMacro>[0]!Tidy</x:FmlaMacro></x:ClientData></v:shape>
<v:shape id="_x0000_s1028"><x:ClientData ObjectType="Drop"><x:Anchor>1, 0, 1, 0, 2, 0, 2, 0</x:Anchor></x:ClientData></v:shape>
<v:shape id="_x0000_s1029"><x:ClientData ObjectType="Scroll"><x:Anchor>junk</x:Anchor></x:ClientData></v:shape>
</xml>"##;

#[test]
fn vml_only_controls_are_read_and_clamped() {
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/></Relationships>"#;
    let bytes = minimal(
        r#"<sheetData/><legacyDrawing r:id="rId9"/>"#,
        &[("xl/worksheets/_rels/sheet1.xml.rels", rels), ("xl/drawings/vmlDrawing1.vml", VML_ONLY)],
        "",
        "",
    );
    let (wb, report) = read_xlsx(&bytes).unwrap();
    assert!(report.warnings.iter().all(|w| !w.contains("form controls are not supported")), "{:?}", report.warnings);
    let c = &wb.sheet(0).unwrap().controls;
    assert_eq!(c.len(), 3, "the drop-down and the shape without an anchor are skipped");
    assert_eq!((c[0].kind, c[0].min, c[0].max, c[0].step, c[0].value, c[0].horizontal), (ControlKind::Slider, 0, 30_000, 1, 30_000, true));
    assert!(c[0].page >= c[0].step && c[0].check_numbers().is_ok());
    assert_eq!((c[1].min, c[1].max), (9, 10), "a range with no width gets one");
    assert!(c[1].check_numbers().is_ok());
    assert_eq!(c[2].text, "Go");
    assert_eq!(c[2].action, Some(ControlAction::Macro { name: "Tidy".into(), input: MacroInput::Selection, output: None }));
}

/// A producer that quotes `shapeId` with single quotes: the control is read, and the kept
/// `<controls>` fragment drops it as it drops a double-quoted one, so a save holds the control
/// once and no kept copy points at a VML shape that was taken out.
#[test]
fn a_single_quoted_shape_id_is_dropped_from_the_kept_controls_too() {
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/></Relationships>"#;
    let vml = r##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><v:shape id="_x0000_s1025" type="#_x0000_t201"><x:ClientData ObjectType="Scroll"><x:Anchor>2, 0, 3, 0, 5, 0, 4, 0</x:Anchor><x:FmlaLink>$B$1</x:FmlaLink><x:Val>40</x:Val><x:Min>0</x:Min><x:Max>100</x:Max><x:Inc>1</x:Inc><x:Page>10</x:Page></x:ClientData></v:shape></xml>"##;
    let bytes = minimal(
        r#"<sheetData/><legacyDrawing r:id="rId9"/><controls><control shapeId='1025' name='Scroll 1'/></controls>"#,
        &[("xl/worksheets/_rels/sheet1.xml.rels", rels), ("xl/drawings/vmlDrawing1.vml", vml)],
        "",
        "",
    );
    let (wb, report) = read_xlsx(&bytes).unwrap();
    // The minimal package has no styles part; nothing about the control is reported.
    assert!(report.warnings.iter().all(|w| w == "no styles part"), "{:?}", report.warnings);
    let sh = wb.sheet(0).unwrap();
    assert_eq!(sh.controls.len(), 1);
    assert_eq!((sh.controls[0].kind, sh.controls[0].value, sh.controls[0].name.as_str()), (ControlKind::Slider, 40, "Scroll 1"));
    let kept = sh.preserved.as_ref().and_then(|k| k.elements.iter().find(|e| e.slot == "controls")).map(|e| e.xml.clone());
    assert_eq!(kept, None, "nothing of the modelled control is kept for pass-through");
    // The saved file holds the control once, with its one VML shape, and reads back once.
    let out = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&out).unwrap();
    let sheet = text(&mut pkg, "xl/worksheets/sheet1.xml");
    // One <controls> element with one control (each control is written as a Choice with a
    // Fallback inside the element's own AlternateContent), and no kept copy.
    assert_eq!(sheet.matches("<controls").count(), 1, "{sheet}");
    assert_eq!(sheet.matches("<mc:Fallback>").count(), 1, "{sheet}");
    assert!(!sheet.contains("shapeId='1025'"), "the kept copy would point at a shape that is gone: {sheet}");
    assert_eq!(text(&mut pkg, "xl/drawings/vmlDrawing1.vml").matches("<v:shape ").count(), 1);
    let (back, report) = read_xlsx(&out).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(back.sheet(0).unwrap().controls.len(), 1);
}

/// One `mc:AlternateContent` holds a scroll bar, which is modelled, and a list box, which stays
/// with pass-through: only the scroll bar leaves the kept fragment, the list box keeps its
/// worksheet entry, its VML shape and its properties part through a save, and the file reads
/// back with the one modelled control and the kept one.
#[test]
fn an_unsupported_control_beside_a_modelled_one_in_one_alternate_content_is_kept() {
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/><Relationship Id="rIdA" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/ctrlProp" Target="../ctrlProps/a.xml"/><Relationship Id="rIdB" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/ctrlProp" Target="../ctrlProps/b.xml"/></Relationships>"#;
    let prop_a = r#"<?xml version="1.0"?><formControlPr xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" objectType="Scroll" fmlaLink="$B$1" min="0" max="100" inc="1" page="10" val="40"/>"#;
    let prop_b = r#"<?xml version="1.0"?><formControlPr xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" objectType="List" fmlaRange="$D$1:$D$3" sel="2"/>"#;
    let vml = r##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><v:shape id="_x0000_s1025" type="#_x0000_t201"><x:ClientData ObjectType="Scroll"><x:Anchor>2, 0, 3, 0, 5, 0, 4, 0</x:Anchor><x:FmlaLink>$B$1</x:FmlaLink><x:Val>40</x:Val><x:Min>0</x:Min><x:Max>100</x:Max><x:Inc>1</x:Inc><x:Page>10</x:Page></x:ClientData></v:shape><v:shape id="_x0000_s1026" type="#_x0000_t201"><x:ClientData ObjectType="List"><x:Anchor>2, 0, 6, 0, 5, 0, 9, 0</x:Anchor><x:FmlaRange>$D$1:$D$3</x:FmlaRange><x:Sel>2</x:Sel></x:ClientData></v:shape></xml>"##;
    let anchor = |from: u32, to: u32| {
        format!(
            r#"<controlPr><anchor><from><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{from}</xdr:row><xdr:rowOff>0</xdr:rowOff></from><to><xdr:col>5</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{to}</xdr:row><xdr:rowOff>0</xdr:rowOff></to></anchor></controlPr>"#
        )
    };
    let controls = format!(
        r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice Requires="x14" xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"><controls><mc:AlternateContent><mc:Choice Requires="x14"><control shapeId="1025" r:id="rIdA" name="Scroll 1">{}</control><control shapeId="1026" r:id="rIdB" name="List 1">{}</control></mc:Choice><mc:Fallback><control shapeId="1025" r:id="rIdA" name="Scroll 1"/><control shapeId="1026" r:id="rIdB" name="List 1"/></mc:Fallback></mc:AlternateContent></controls></mc:Choice></mc:AlternateContent>"#,
        anchor(3, 4),
        anchor(6, 9)
    );
    let bytes = minimal(
        &format!(r#"<sheetData/><legacyDrawing r:id="rId9"/>{controls}"#),
        &[
            ("xl/worksheets/_rels/sheet1.xml.rels", rels),
            ("xl/ctrlProps/a.xml", prop_a),
            ("xl/ctrlProps/b.xml", prop_b),
            ("xl/drawings/vmlDrawing1.vml", vml),
        ],
        "",
        "",
    );
    let (wb, _) = read_xlsx(&bytes).unwrap();
    let sh = wb.sheet(0).unwrap();
    assert_eq!(sh.controls.len(), 1, "the scroll bar is modelled, the list box is not");
    assert_eq!((sh.controls[0].kind, sh.controls[0].name.as_str()), (ControlKind::Slider, "Scroll 1"));
    let kept = sh.preserved.as_ref().and_then(|k| k.elements.iter().find(|e| e.slot == "controls")).map(|e| e.xml.clone()).unwrap_or_default();
    assert!(kept.contains("List 1") && kept.contains("<mc:Fallback>"), "the list box stays, in its wrapper: {kept}");
    assert!(!kept.contains("Scroll 1") && !kept.contains("shapeId=\"1025\""), "the scroll bar leaves every branch: {kept}");
    // Saved: the scroll bar once (one Fallback of ours), the list box in the kept wrapper with its
    // shape and its properties part.
    let out = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&out).unwrap();
    let sheet = text(&mut pkg, "xl/worksheets/sheet1.xml");
    assert_eq!(sheet.matches("<controls").count(), 1, "{sheet}");
    assert_eq!(sheet.matches("name=\"Scroll 1\"").count(), 2, "the modelled control, in its Choice and Fallback: {sheet}");
    assert_eq!(sheet.matches("name=\"List 1\"").count(), 2, "the kept control, in its Choice and Fallback: {sheet}");
    assert_eq!(sheet.matches("shapeId=\"1026\"").count(), 2, "{sheet}");
    let vml_out = text(&mut pkg, "xl/drawings/vmlDrawing1.vml");
    assert!(vml_out.contains("_x0000_s1026") && vml_out.contains("ObjectType=\"List\""), "the list box's shape is kept: {vml_out}");
    assert_eq!(vml_out.matches("ObjectType=\"Scroll\"").count(), 1, "{vml_out}");
    // Part names come back lower-cased.
    let props: Vec<String> = pkg.names().into_iter().filter(|n| n.starts_with("xl/ctrlprops/")).collect();
    assert_eq!(props.len(), 2, "ours and the kept one: {props:?}");
    assert!(props.iter().any(|n| text(&mut pkg, n).contains("objectType=\"List\"")), "the list box's properties part survives: {props:?}");
    // Read back: the one modelled control and the kept one.
    let (back, _) = read_xlsx(&out).unwrap();
    let sh = back.sheet(0).unwrap();
    assert_eq!(sh.controls.len(), 1);
    let kept = sh.preserved.as_ref().and_then(|k| k.elements.iter().find(|e| e.slot == "controls")).map(|e| e.xml.clone()).unwrap_or_default();
    assert!(kept.contains("List 1") && !kept.contains("Scroll 1"), "{kept}");
}

/// A producer that prefixes the main namespace (`<x:controls>`): the kept element is still the
/// sheet's one `<controls>` element, so a modelled control is written into it rather than into a
/// second one, which the worksheet schema does not allow.
#[test]
fn a_modelled_control_joins_a_kept_controls_element_with_a_namespace_prefix() {
    use gridcraft_model::{Anchor, ControlKind, FormControl};
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/><Relationship Id="rIdB" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/ctrlProp" Target="../ctrlProps/b.xml"/></Relationships>"#;
    let prop_b = r#"<?xml version="1.0"?><formControlPr xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" objectType="List" fmlaRange="$D$1:$D$3" sel="2"/>"#;
    let vml = r##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><v:shape id="_x0000_s1026" type="#_x0000_t201"><x:ClientData ObjectType="List"><x:Anchor>2, 0, 6, 0, 5, 0, 9, 0</x:Anchor><x:FmlaRange>$D$1:$D$3</x:FmlaRange><x:Sel>2</x:Sel></x:ClientData></v:shape></xml>"##;
    let controls = r#"<x:controls xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"><x:control shapeId="1026" r:id="rIdB" name="List 1"><x:controlPr><x:anchor><x:from><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>6</xdr:row><xdr:rowOff>0</xdr:rowOff></x:from><x:to><xdr:col>5</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>9</xdr:row><xdr:rowOff>0</xdr:rowOff></x:to></x:anchor></x:controlPr></x:control></x:controls>"#;
    let bytes = minimal(
        &format!(r#"<sheetData/><legacyDrawing r:id="rId9"/>{controls}"#),
        &[("xl/worksheets/_rels/sheet1.xml.rels", rels), ("xl/ctrlProps/b.xml", prop_b), ("xl/drawings/vmlDrawing1.vml", vml)],
        "",
        "",
    );
    let (mut wb, _) = read_xlsx(&bytes).unwrap();
    assert!(wb.sheet(0).unwrap().controls.is_empty(), "the list box stays with pass-through");
    let kept = wb
        .sheet(0)
        .unwrap()
        .preserved
        .as_ref()
        .and_then(|k| k.elements.iter().find(|e| e.slot == "controls"))
        .map(|e| e.xml.clone())
        .unwrap_or_default();
    assert!(kept.contains("</x:controls>"), "kept with its prefix: {kept}");
    let anchor = Anchor { cell: gridcraft_core::CellRef::new(2, 1), width: 120.0, height: 20.0, ..Default::default() };
    wb.sheet_mut(0).unwrap().controls.push(FormControl {
        id: 900,
        kind: ControlKind::Slider,
        anchor,
        link: Some("$B$1".into()),
        value: 40,
        ..Default::default()
    });
    let out = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&out).unwrap();
    let sheet = text(&mut pkg, "xl/worksheets/sheet1.xml");
    let opened = sheet.matches("<controls").count() + sheet.matches("<x:controls").count();
    assert_eq!(opened, 1, "one controls element: {sheet}");
    let start = sheet.find("<x:controls").or_else(|| sheet.find("<controls")).unwrap();
    let end = sheet.rfind("controls>").unwrap();
    let inner = &sheet[start..end];
    assert!(inner.contains("name=\"List 1\"") && inner.contains("<mc:Fallback>"), "both controls inside the one element: {sheet}");
    let (back, _) = read_xlsx(&out).unwrap();
    assert_eq!(back.sheet(0).unwrap().controls.len(), 1, "the slider reads back; the list box stays kept");
}

/// A kept `mc:AlternateContent` with separate `<controls>` elements in its Choice and Fallback:
/// a modelled control is written into the Choice's element, the one the reader (and an
/// application that knows x14) reads, so it comes back with its name and its properties part;
/// the Fallback's element is left as the file had it.
#[test]
fn a_modelled_control_joins_the_choice_controls_element_of_a_kept_alternate_content() {
    use gridcraft_model::{Anchor, ControlKind, FormControl};
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/><Relationship Id="rIdB" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/ctrlProp" Target="../ctrlProps/b.xml"/></Relationships>"#;
    let prop_b = r#"<?xml version="1.0"?><formControlPr xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" objectType="List" fmlaRange="$D$1:$D$3" sel="2"/>"#;
    let vml = r##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><v:shape id="_x0000_s1026" type="#_x0000_t201"><x:ClientData ObjectType="List"><x:Anchor>2, 0, 6, 0, 5, 0, 9, 0</x:Anchor><x:FmlaRange>$D$1:$D$3</x:FmlaRange><x:Sel>2</x:Sel></x:ClientData></v:shape></xml>"##;
    let list = r#"<control shapeId="1026" r:id="rIdB" name="List 1"><controlPr><anchor><from><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>6</xdr:row><xdr:rowOff>0</xdr:rowOff></from><to><xdr:col>5</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>9</xdr:row><xdr:rowOff>0</xdr:rowOff></to></anchor></controlPr></control>"#;
    let controls = format!(
        r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"><mc:Choice Requires="x14"><controls>{list}</controls></mc:Choice><mc:Fallback><controls><control shapeId="1026" r:id="rIdB" name="List 1"/></controls></mc:Fallback></mc:AlternateContent>"#
    );
    let bytes = minimal(
        &format!(r#"<sheetData/><legacyDrawing r:id="rId9"/>{controls}"#),
        &[("xl/worksheets/_rels/sheet1.xml.rels", rels), ("xl/ctrlProps/b.xml", prop_b), ("xl/drawings/vmlDrawing1.vml", vml)],
        "",
        "",
    );
    let (mut wb, _) = read_xlsx(&bytes).unwrap();
    assert!(wb.sheet(0).unwrap().controls.is_empty(), "the list box stays with pass-through");
    let anchor = Anchor { cell: gridcraft_core::CellRef::new(2, 1), width: 120.0, height: 20.0, ..Default::default() };
    wb.sheet_mut(0).unwrap().controls.push(FormControl {
        id: 900,
        kind: ControlKind::Slider,
        anchor,
        name: "Speed".into(),
        link: Some("$B$1".into()),
        value: 40,
        ..Default::default()
    });
    let out = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&out).unwrap();
    let sheet = text(&mut pkg, "xl/worksheets/sheet1.xml");
    // The Choice's <controls> comes first in the sheet, the Fallback's last.
    let (first, last) = (sheet.find("<controls").unwrap(), sheet.rfind("<controls").unwrap());
    assert!(first < last, "two controls elements, the kept Choice's and Fallback's: {sheet}");
    let choice = &sheet[first..last];
    assert!(choice.contains("name=\"Speed\"") && choice.contains("name=\"List 1\""), "the Choice holds both: {sheet}");
    assert!(!sheet[last..].contains("name=\"Speed\""), "the Fallback is as the file had it: {sheet}");
    let (back, _) = read_xlsx(&out).unwrap();
    let sh = back.sheet(0).unwrap();
    assert_eq!(sh.controls.len(), 1);
    assert_eq!(
        (sh.controls[0].name.as_str(), sh.controls[0].link.as_deref(), sh.controls[0].value),
        ("Speed", Some("$B$1"), 40),
        "read from its worksheet entry and properties part"
    );
    let kept = sh.preserved.as_ref().and_then(|k| k.elements.iter().find(|e| e.slot == "controls")).map(|e| e.xml.clone()).unwrap_or_default();
    assert!(kept.contains("List 1") && !kept.contains("Speed"), "{kept}");
}

/// Shapes with a named `id` and their number in `o:spid`, as some producers write them.
const VML_SPID: &str = r##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel">
<v:shape id="Button 1" o:spid="_x0000_s1025" o:button="t"><v:textbox><div>Go</div></v:textbox><x:ClientData ObjectType="Button"><x:Anchor>1, 0, 6, 0, 3, 0, 7, 0</x:Anchor><x:FmlaMacro>[0]!Tidy</x:FmlaMacro></x:ClientData></v:shape>
<v:shape id="Spinner 2" o:spid="_x0000_s1026"><x:ClientData ObjectType="Spin"><x:Anchor>1, 0, 1, 0, 1, 20, 3, 0</x:Anchor><x:Val>7</x:Val><x:Max>9</x:Max></x:ClientData></v:shape>
</xml>"##;

#[test]
fn vml_shapes_are_also_found_by_their_spid() {
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/></Relationships>"#;
    // The button is listed in <controls> without a properties part; the spin button is only in
    // the VML.
    let bytes = minimal(
        r#"<sheetData/><legacyDrawing r:id="rId9"/><controls><control shapeId="1025" name="Go Button"/></controls>"#,
        &[("xl/worksheets/_rels/sheet1.xml.rels", rels), ("xl/drawings/vmlDrawing1.vml", VML_SPID)],
        "",
        "",
    );
    let (wb, _) = read_xlsx(&bytes).unwrap();
    let c = &wb.sheet(0).unwrap().controls;
    assert_eq!(c.len(), 2, "{c:?}");
    assert_eq!((c[0].kind, c[0].name.as_str(), c[0].text.as_str()), (ControlKind::Button, "Go Button", "Go"));
    assert_eq!(c[0].action, Some(ControlAction::Macro { name: "Tidy".into(), input: MacroInput::Selection, output: None }));
    assert_eq!((c[1].kind, c[1].value, c[1].max), (ControlKind::Spin, 7, 9));
    // Saved again, each is written once and nothing of them is kept twice.
    let out = write_xlsx(&wb).unwrap();
    let (back, _) = read_xlsx(&out).unwrap();
    assert_eq!(back.sheet(0).unwrap().controls.len(), 2);
}

#[test]
fn the_properties_part_wins_over_the_vml() {
    let mut wb = book();
    if let Some(s) = wb.sheet_mut(0) {
        s.comments.clear();
        s.controls.truncate(1);
    }
    let bytes = write_xlsx(&wb).unwrap();
    // Rewrite the VML's numbers: the reader still takes the properties part's.
    let mut pkg = Package::open(&bytes).unwrap();
    let mut parts = Vec::new();
    for name in pkg.names() {
        let mut data = pkg.read(&name).unwrap().unwrap();
        if name.ends_with(".vml") {
            data = String::from_utf8(data).unwrap().replace("<x:Max>500</x:Max>", "<x:Max>9</x:Max>").into_bytes();
        }
        parts.push((name, data));
    }
    let zipped = super::make_zip(&parts.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect::<Vec<_>>());
    let (back, _) = read_xlsx(&zipped).unwrap();
    assert_eq!(back.sheet(0).unwrap().controls[0].max, 500);
}

/// A hostile sheet: more controls than a sheet keeps, properties parts that are not XML or hold
/// numbers no control can take, shape ids that are not numbers, and oversized names, captions and
/// links. Reading must not panic, must keep at most the cap, and every control it keeps must pass
/// the model's rules.
#[test]
fn hostile_form_controls_are_capped_and_repaired() {
    let n = 13_000;
    let mut controls = String::from("<controls>");
    let anchor = r#"<controlPr><anchor><from><xdr:col>1</xdr:col><xdr:colOff>-99999999999</xdr:colOff><xdr:row>99999999999</xdr:row><xdr:rowOff>0</xdr:rowOff></from><to><xdr:col>-4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>2</xdr:row><xdr:rowOff>0</xdr:rowOff></to></anchor></controlPr>"#;
    for i in 0..n {
        let rid = match i % 10 {
            0 => "rIdB",
            1 => "rIdC",
            _ => "rIdA",
        };
        let _ = std::fmt::Write::write_fmt(
            &mut controls,
            format_args!(r#"<control shapeId="{}" r:id="{rid}" name="{}">{anchor}</control>"#, 2000 + i, "n".repeat(400)),
        );
    }
    controls
        .push_str(r#"<control shapeId="99999999999999999999999" r:id="rIdA"/><control shapeId="x" r:id="rIdA"/><control r:id="rIdA"/></controls>"#);
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdA" Type="http://schemas.microsoft.com/office/2006/relationships/ctrlProp" Target="../ctrlProps/a.xml"/><Relationship Id="rIdB" Type="http://schemas.microsoft.com/office/2006/relationships/ctrlProp" Target="../ctrlProps/b.xml"/><Relationship Id="rIdC" Type="http://schemas.microsoft.com/office/2006/relationships/ctrlProp" Target="../ctrlProps/missing.xml"/></Relationships>"#;
    let link = format!("$B${}", "9".repeat(500));
    let prop_a = format!(
        r#"<?xml version="1.0"?><formControlPr xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" objectType="Scroll" fmlaLink="{link}" min="1e400" max="NaN" inc="-inf" page="18446744073709551616" val="-7"/>"#
    );
    let body = format!(r#"<sheetData/>{controls}"#)
        .replace("<controls>", r#"<controls xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing">"#);
    let bytes = minimal(
        &body,
        &[("xl/worksheets/_rels/sheet1.xml.rels", rels), ("xl/ctrlProps/a.xml", &prop_a), ("xl/ctrlProps/b.xml", "\u{0}\u{1}not xml <<<")],
        "",
        "",
    );
    let (wb, _) = read_xlsx(&bytes).unwrap();
    let c = &wb.sheet(0).unwrap().controls;
    assert_eq!(c.len(), 10_000, "the cap of one sheet");
    for x in c {
        assert_eq!(x.kind, ControlKind::Slider);
        assert!(x.check_numbers().is_ok(), "{x:?}");
        assert!(x.value >= x.min && x.value <= x.max, "{x:?}");
        assert!(x.name.chars().count() <= 255 && x.link.is_none(), "{x:?}");
        assert!(x.anchor.width >= 1.0 && x.anchor.height >= 1.0 && x.anchor.cell.row < gridcraft_core::MAX_ROWS, "{x:?}");
    }
    // Written back and read again, the kept controls stay valid.
    let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert_eq!(back.sheet(0).unwrap().controls.len(), c.len());
}

/// Each listed control finds its VML shape through an index, so a sheet of many controls reads in
/// linear time (it used to scan every shape for every control).
#[test]
#[allow(clippy::disallowed_methods)] // a native-only timing test: the clock is read off wasm
fn many_listed_controls_read_in_linear_time() {
    let mut wb = Workbook::default();
    if let Some(sh) = wb.sheet_mut(0) {
        for i in 0..10_000u32 {
            let a = anchor(&format!("B{}", i + 1), 60.0, 18.0);
            sh.controls.push(FormControl { id: i + 1, kind: ControlKind::Spin, anchor: a, ..Default::default() });
        }
    }
    let bytes = write_xlsx(&wb).unwrap();
    let t0 = std::time::Instant::now();
    let (back, _) = read_xlsx(&bytes).unwrap();
    let took = t0.elapsed();
    assert_eq!(back.sheet(0).unwrap().controls.len(), 10_000);
    // About 0.4 s here; scanning every shape for every control took over 8 s.
    assert!(took < std::time::Duration::from_secs(3), "{took:?}");
}

/// The sheet relates each control properties part with the relationship type MS-XLSX defines and
/// Excel writes (in the officeDocument namespace); Excel finds the part by that type.
#[test]
fn the_properties_parts_use_the_ctrl_prop_relationship_type() {
    let bytes = write_xlsx(&book()).unwrap();
    let mut pkg = Package::open(&bytes).unwrap();
    let rels = text(&mut pkg, "xl/worksheets/_rels/sheet1.xml.rels");
    let types: Vec<&str> = rels.split("Type=\"").skip(1).filter_map(|r| r.split('"').next()).filter(|t| t.ends_with("/ctrlProp")).collect();
    assert_eq!(types.len(), 4, "{rels}");
    assert!(types.iter().all(|t| *t == "http://schemas.openxmlformats.org/officeDocument/2006/relationships/ctrlProp"), "{types:?}");
}

/// A sheet with a header and footer drawing and a drawing for its controls: the controls come
/// from the drawing `<legacyDrawing r:id>` names, whichever relationship is listed first.
#[test]
fn controls_come_from_the_legacy_drawing_not_the_header_and_footer_drawing() {
    let hf = r##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office"><v:shape id="LH" o:spid="_x0000_s2049" type="#_x0000_t75"><v:imagedata o:title="logo"/></v:shape></xml>"##;
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing" Target="../drawings/vmlDrawingHF1.vml"/><Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/></Relationships>"#;
    let bytes = minimal(
        r#"<sheetData/><legacyDrawing r:id="rId9"/><legacyDrawingHF r:id="rId1"/>"#,
        &[("xl/worksheets/_rels/sheet1.xml.rels", rels), ("xl/drawings/vmlDrawingHF1.vml", hf), ("xl/drawings/vmlDrawing1.vml", VML_ONLY)],
        "",
        "",
    );
    let (wb, _) = read_xlsx(&bytes).unwrap();
    let c = &wb.sheet(0).unwrap().controls;
    assert_eq!(c.len(), 3, "{c:?}");
    assert_eq!(c[2].text, "Go");
}

/// Every listed control that names the same properties part shares one read of it. Each read is
/// charged to the package's 1 GB budget, so 10,000 reads of a 200 KB part would spend it, drop the
/// controls after that, and fail the parts read after the controls (here the sheet's notes). A
/// hostile file can list that many controls against one part.
#[test]
fn controls_naming_one_properties_part_read_it_once() {
    let n = 13_000;
    let mut controls = String::from(r#"<controls xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing">"#);
    for i in 0..n {
        let _ = std::fmt::Write::write_fmt(
            &mut controls,
            format_args!(
                r#"<control shapeId="{}" r:id="rIdA" name="c{i}"><controlPr><anchor><from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{i}</xdr:row><xdr:rowOff>0</xdr:rowOff></from><to><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></to></anchor></controlPr></control>"#,
                2000 + i,
                i + 1
            ),
        );
    }
    controls.push_str("</controls>");
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdA" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/ctrlProp" Target="../ctrlProps/a.xml"/><Relationship Id="rIdN" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="../comments1.xml"/></Relationships>"#;
    // 200 KB of attribute whitespace: read once per control, 10,000 reads would be 2 GB.
    let prop_a = format!(
        r#"<?xml version="1.0"?><formControlPr xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" objectType="Scroll" fmlaLink="$B$1"{}min="0" max="200" inc="5" page="20" val="45"/>"#,
        " ".repeat(200 * 1024)
    );
    let notes = r#"<?xml version="1.0"?><comments xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><authors><author>Ann</author></authors><commentList><comment ref="A1" authorId="0"><text><t>read after the controls</t></text></comment></commentList></comments>"#;
    let bytes = minimal(
        &format!("<sheetData/>{controls}"),
        &[("xl/worksheets/_rels/sheet1.xml.rels", rels), ("xl/ctrlProps/a.xml", &prop_a), ("xl/comments1.xml", notes)],
        "",
        "",
    );
    let (wb, report) = read_xlsx(&bytes).unwrap();
    let sh = wb.sheet(0).unwrap();
    assert_eq!(sh.controls.len(), 10_000, "the cap of one sheet, every control read from the one part");
    assert!(sh.controls.iter().all(|c| {
        c.kind == ControlKind::Slider && c.link.as_deref() == Some("$B$1") && (c.min, c.max, c.step, c.page, c.value) == (0, 200, 5, 20, 45)
    }));
    assert_eq!(sh.comments.len(), 1, "a part read after the controls still reads: {:?}", report.warnings);
}

/// A legacy VML drawing with a button (`1025`, modelled), a check box (`1026`) and an option
/// button (`1027`, with a picture through the drawing's own relationship) that only the drawing
/// lists, plus `note` (a note shape for A1 whose client data carries that `ObjectType` attribute,
/// with its comments part) when given. `listed`: the sheet's
/// `<controls>` lists the button.
fn vml_only_others(listed: bool, button: bool, note: Option<&str>) -> Vec<u8> {
    const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let mut rels = format!(r#"<Relationship Id="rId9" Type="{REL}/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/>"#);
    if listed {
        rels.push_str(&format!(r#"<Relationship Id="rIdA" Type="{REL}/ctrlProp" Target="../ctrlProps/a.xml"/>"#));
    }
    if note.is_some() {
        rels.push_str(&format!(r#"<Relationship Id="rIdN" Type="{REL}/comments" Target="../comments1.xml"/>"#));
    }
    let rels =
        format!(r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#);
    let vml_rels = format!(
        r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/image" Target="../media/image1.png"/></Relationships>"#
    );
    let prop_a = r#"<?xml version="1.0"?><formControlPr xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" objectType="Button" lockText="1"/>"#;
    let mut shapes = String::new();
    if let Some(attr) = note {
        shapes.push_str(&format!(r##"<v:shape id="_x0000_s1028" type="#_x0000_t202"><x:ClientData {attr}><x:Anchor>1, 15, 0, 2, 3, 15, 4, 4</x:Anchor><x:Row>0</x:Row><x:Column>0</x:Column></x:ClientData></v:shape>"##));
    }
    if button {
        shapes.push_str(r##"<v:shape id="_x0000_s1025" type="#_x0000_t201"><v:textbox><div>Go</div></v:textbox><x:ClientData ObjectType="Button"><x:Anchor>2, 0, 3, 0, 4, 0, 5, 0</x:Anchor><x:FmlaMacro>[0]!Go</x:FmlaMacro></x:ClientData></v:shape>"##);
    }
    shapes.push_str(r##"<v:shape id="_x0000_s1026" type="#_x0000_t201"><v:textbox><div>Paid</div></v:textbox><x:ClientData ObjectType="Checkbox"><x:Anchor>2, 0, 7, 0, 4, 0, 8, 0</x:Anchor><x:FmlaLink>$B$7</x:FmlaLink><x:Checked>1</x:Checked></x:ClientData></v:shape>"##);
    shapes.push_str(r##"<v:shape id="_x0000_s1027" type="#_x0000_t201"><v:imagedata o:relid="rId1" o:title=""/><x:ClientData ObjectType="Radio"><x:Anchor>2, 0, 9, 0, 4, 0, 10, 0</x:Anchor><x:FmlaLink>$B$9</x:FmlaLink></x:ClientData></v:shape>"##);
    let vml = format!(
        r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="1"/></o:shapelayout>{shapes}</xml>"#
    );
    let controls = if listed {
        r#"<controls xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"><control shapeId="1025" r:id="rIdA" name="Button 1"><controlPr><anchor><from><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>3</xdr:row><xdr:rowOff>0</xdr:rowOff></from><to><xdr:col>4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>5</xdr:row><xdr:rowOff>0</xdr:rowOff></to></anchor></controlPr></control></controls>"#
    } else {
        ""
    };
    let notes = r#"<?xml version="1.0"?><comments xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><authors><author>Ann</author></authors><commentList><comment ref="A1" authorId="0"><text><t>check the totals</t></text></comment></commentList></comments>"#;
    let mut parts: Vec<(&str, &str)> = vec![
        ("xl/worksheets/_rels/sheet1.xml.rels", &rels),
        ("xl/drawings/vmlDrawing1.vml", &vml),
        ("xl/drawings/_rels/vmlDrawing1.vml.rels", &vml_rels),
        ("xl/media/image1.png", "not really a png"),
    ];
    if listed {
        parts.push(("xl/ctrlProps/a.xml", prop_a));
    }
    if note.is_some() {
        parts.push(("xl/comments1.xml", notes));
    }
    // As `minimal` builds it, with content types for the picture and the drawing.
    let ct = super::CT.replace(
        "</Types>",
        r#"<Default Extension="png" ContentType="image/png"/><Default Extension="vml" ContentType="application/vnd.openxmlformats-officedocument.vmlDrawing"/></Types>"#,
    );
    let base = minimal(&format!(r#"<sheetData/><legacyDrawing r:id="rId9"/>{controls}"#), &parts, "", "");
    let mut zip = Package::open(&base).unwrap();
    let mut files: Vec<(String, Vec<u8>)> = vec![("[Content_Types].xml".into(), ct.into_bytes())];
    for n in zip.names() {
        if n != "[Content_Types].xml" {
            let data = zip.read(&n).unwrap().unwrap();
            files.push((n, data));
        }
    }
    super::make_zip(&files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect::<Vec<_>>())
}

/// Checks a saved package keeps the check box and option button shapes once each, in the sheet's
/// one VML drawing, with that drawing's one relationship to its one picture; returns the drawing.
fn kept_vml_only_others(out: &[u8]) -> String {
    let mut pkg = Package::open(out).unwrap();
    let sheet = text(&mut pkg, "xl/worksheets/sheet1.xml");
    assert_eq!(sheet.matches("<legacyDrawing ").count(), 1, "{sheet}");
    let rels = text(&mut pkg, "xl/worksheets/_rels/sheet1.xml.rels");
    assert_eq!(rels.matches("/vmlDrawing\"").count(), 1, "one VML drawing relationship: {rels}");
    let names = pkg.names();
    let vmls: Vec<&String> = names.iter().filter(|n| n.ends_with(".vml")).collect();
    assert_eq!(vmls.len(), 1, "{names:?}");
    let vml_name = vmls[0].clone();
    let vml = text(&mut pkg, &vml_name);
    assert_eq!(vml.matches("ObjectType=\"Checkbox\"").count(), 1, "the check box is kept once: {vml}");
    assert_eq!(vml.matches("ObjectType=\"Radio\"").count(), 1, "the option button is kept once: {vml}");
    let (dir, file) = vml_name.rsplit_once('/').unwrap();
    let vml_rels = text(&mut pkg, &format!("{dir}/_rels/{file}.rels"));
    assert_eq!(vml_rels.matches("<Relationship ").count(), 1, "{vml_rels}");
    let media: Vec<&String> = names.iter().filter(|n| n.starts_with("xl/media/")).collect();
    assert_eq!(media.len(), 1, "the option button's picture once: {names:?}");
    vml
}

/// The sheet's `<controls>` lists only a button, which is modelled; a check box and an option
/// button in the same VML drawing have no worksheet entry. With the button's entry gone from the
/// kept content no `<controls>` element is left, but the drawing still holds the two, so it is
/// kept: the save writes them beside the regenerated button, and they survive a second round trip.
#[test]
fn vml_only_controls_survive_when_the_last_listed_control_is_modelled() {
    let bytes = vml_only_others(true, true, None);
    let (wb, report) = read_xlsx(&bytes).unwrap();
    let sh = wb.sheet(0).unwrap();
    assert_eq!(sh.controls.len(), 1, "the button is modelled");
    assert_eq!(sh.controls[0].kind, ControlKind::Button);
    assert!(sh.preserved.as_ref().is_some_and(|p| p.vml.is_some()), "the drawing is kept: {:?}", report.warnings);
    assert!(report.warnings.iter().any(|w| w.contains("form controls are not shown")), "{:?}", report.warnings);
    let out = write_xlsx(&wb).unwrap();
    let vml = kept_vml_only_others(&out);
    assert_eq!(vml.matches("ObjectType=\"Button\"").count(), 1, "the button once, written from the model: {vml}");
    let (back, _) = read_xlsx(&out).unwrap();
    let sh = back.sheet(0).unwrap();
    assert_eq!(sh.controls.len(), 1);
    assert_eq!(sh.controls[0].text, "Go");
    // Saved again: still once each.
    let again = write_xlsx(&back).unwrap();
    let vml = kept_vml_only_others(&again);
    assert_eq!(vml.matches("ObjectType=\"Button\"").count(), 1, "{vml}");
}

/// A sheet whose only controls are a check box and an option button that only the VML drawing
/// lists (no `<controls>`, no notes, nothing modelled): the drawing is kept on its own.
#[test]
fn a_sheet_whose_only_controls_are_vml_only_and_unmodelled_keeps_them() {
    let bytes = vml_only_others(false, false, None);
    let (wb, report) = read_xlsx(&bytes).unwrap();
    assert!(wb.sheet(0).unwrap().controls.is_empty());
    let out = write_xlsx(&wb).unwrap();
    let vml = kept_vml_only_others(&out);
    assert!(!vml.contains("ObjectType=\"Button\""), "{vml}");
    let (back, _) = read_xlsx(&out).unwrap();
    let again = write_xlsx(&back).unwrap();
    kept_vml_only_others(&again);
    assert!(report.warnings.iter().any(|w| w.contains("form controls are not shown")), "{:?}", report.warnings);
}

/// The same with a note in the drawing (one VML drawing for one comments part) and a VML-only
/// button that is modelled: the note and the button are written from the model, the check box and
/// option button stay, all in the one drawing.
#[test]
fn vml_only_controls_beside_a_note_are_kept() {
    let bytes = vml_only_others(false, true, Some(r#"ObjectType="Note""#));
    let (wb, report) = read_xlsx(&bytes).unwrap();
    let sh = wb.sheet(0).unwrap();
    assert_eq!((sh.controls.len(), sh.comments.len()), (1, 1), "{:?}", report.warnings);
    let out = write_xlsx(&wb).unwrap();
    let vml = kept_vml_only_others(&out);
    assert_eq!(vml.matches("ObjectType=\"Note\"").count(), 1, "{vml}");
    assert_eq!(vml.matches("ObjectType=\"Button\"").count(), 1, "{vml}");
    let (back, _) = read_xlsx(&out).unwrap();
    let sh = back.sheet(0).unwrap();
    assert_eq!((sh.controls.len(), sh.comments.len()), (1, 1));
    let vml = kept_vml_only_others(&write_xlsx(&back).unwrap());
    assert_eq!(vml.matches("ObjectType=\"Note\"").count(), 1, "{vml}");
}

#[test]
fn a_note_spelled_in_another_case_or_quote_is_not_kept_beside_the_written_one() {
    // The note's client data reads ObjectType='note': still a note, so the kept drawing drops it
    // and the note GridCraft writes for the comment is the only one.
    for attr in ["ObjectType='note'", "ObjectType = \"NOTE\"", "ObjectType=\" Note \""] {
        let (wb, report) = read_xlsx(&vml_only_others(false, true, Some(attr))).unwrap();
        assert_eq!(wb.sheet(0).unwrap().comments.len(), 1, "{attr}: {:?}", report.warnings);
        let out = write_xlsx(&wb).unwrap();
        let vml = kept_vml_only_others(&out);
        assert_eq!(note_shapes(&vml), 1, "{attr}: {vml}");
        let (back, _) = read_xlsx(&out).unwrap();
        assert_eq!(back.sheet(0).unwrap().comments.len(), 1, "{attr}");
    }
}

/// The note shapes in VML text, counted as the reader tells a note: by each shape's client data
/// `ObjectType`, whatever its spelling.
fn note_shapes(vml: &str) -> usize {
    crate::xml::parse(vml.as_bytes()).unwrap().kids("shape").filter(|s| s.child("ClientData").is_some_and(crate::controls::is_note_data)).count()
}

/// The saved package's one VML drawing.
fn only_vml(out: &[u8]) -> String {
    let mut pkg = Package::open(out).unwrap();
    let names: Vec<String> = pkg.names().into_iter().filter(|n| n.ends_with(".vml")).collect();
    assert_eq!(names.len(), 1, "{names:?}");
    text(&mut pkg, &names[0])
}

/// A sheet's VML drawing, as the only part beside the sheet's relationships.
fn with_vml(shapes: &str) -> Vec<u8> {
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/></Relationships>"#;
    let vml = format!(
        r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel">{shapes}</xml>"#
    );
    minimal(
        r#"<sheetData/><legacyDrawing r:id="rId9"/>"#,
        &[("xl/worksheets/_rels/sheet1.xml.rels", rels), ("xl/drawings/vmlDrawing1.vml", &vml)],
        "",
        "",
    )
}

/// Text that reads like a note's `ObjectType` in a check box's caption or in another attribute of
/// an option button doesn't make either a note: both are kept, and a modelled button with that
/// caption keeps it.
#[test]
fn a_control_whose_text_reads_like_a_note_object_type_is_kept() {
    let bytes = with_vml(concat!(
        r##"<v:shape id="_x0000_s1025" type="#_x0000_t201"><v:textbox><div>ObjectType="Note"</div></v:textbox><x:ClientData ObjectType="Button"><x:Anchor>2, 0, 3, 0, 4, 0, 5, 0</x:Anchor></x:ClientData></v:shape>"##,
        r##"<v:shape id="_x0000_s1026" type="#_x0000_t201"><v:textbox><div>ObjectType="Note"</div></v:textbox><x:ClientData ObjectType="Checkbox"><x:Anchor>2, 0, 7, 0, 4, 0, 8, 0</x:Anchor><x:FmlaLink>$B$7</x:FmlaLink></x:ClientData></v:shape>"##,
        r##"<v:shape id="_x0000_s1027" type="#_x0000_t201" alt="ObjectType='note'"><x:ClientData ObjectType="Radio"><x:Anchor>2, 0, 9, 0, 4, 0, 10, 0</x:Anchor></x:ClientData></v:shape>"##,
    ));
    let (wb, report) = read_xlsx(&bytes).unwrap();
    let sh = wb.sheet(0).unwrap();
    assert_eq!(sh.controls.len(), 1, "{:?}", report.warnings);
    assert_eq!(sh.controls[0].text, "ObjectType=\"Note\"");
    let mut out = write_xlsx(&wb).unwrap();
    for _ in 0..2 {
        let vml = only_vml(&out);
        assert_eq!(vml.matches("ObjectType=\"Checkbox\"").count(), 1, "the check box is kept: {vml}");
        assert_eq!(vml.matches("ObjectType=\"Radio\"").count(), 1, "the option button is kept: {vml}");
        assert_eq!(vml.matches("ObjectType=\"Button\"").count(), 1, "the button once: {vml}");
        assert_eq!(note_shapes(&vml), 0, "{vml}");
        let (back, _) = read_xlsx(&out).unwrap();
        let sh = back.sheet(0).unwrap();
        assert_eq!(sh.controls.len(), 1);
        assert_eq!(sh.controls[0].text, "ObjectType=\"Note\"");
        out = write_xlsx(&back).unwrap();
    }
}

/// Kept VML leaves out the modelled controls by the number the reader gave them: the `id`, else
/// the `o:spid`, wherever either comes in the shape's tag. A button whose `o:spid` comes before a
/// different `id`, and one with a plain numeric `id`, are written once, from the model; a check
/// box whose `o:spid` is a modelled button's number but whose `id` is its own is kept; a button
/// with no number isn't modelled and is kept; notes are dropped whatever their numbers.
#[test]
fn kept_vml_leaves_out_modelled_controls_by_the_readers_shape_number() {
    let bytes = with_vml(concat!(
        r##"<v:shape o:spid="_x0000_s1030" id="_x0000_s1025" type="#_x0000_t201"><v:textbox><div>A</div></v:textbox><x:ClientData ObjectType="Button"><x:Anchor>2, 0, 1, 0, 4, 0, 2, 0</x:Anchor></x:ClientData></v:shape>"##,
        r##"<v:shape id="1026" o:spid="_x0000_s1031" type="#_x0000_t201"><v:textbox><div>B</div></v:textbox><x:ClientData ObjectType="Button"><x:Anchor>2, 0, 3, 0, 4, 0, 4, 0</x:Anchor></x:ClientData></v:shape>"##,
        r##"<v:shape o:spid="_x0000_s1025" id="_x0000_s1027" type="#_x0000_t201"><v:textbox><div>Paid</div></v:textbox><x:ClientData ObjectType="Checkbox"><x:Anchor>2, 0, 5, 0, 4, 0, 6, 0</x:Anchor></x:ClientData></v:shape>"##,
        r##"<v:shape id="Button 9" type="#_x0000_t201"><v:textbox><div>C</div></v:textbox><x:ClientData ObjectType="Button"><x:Anchor>2, 0, 7, 0, 4, 0, 8, 0</x:Anchor></x:ClientData></v:shape>"##,
        r##"<v:shape id="_x0000_s1028" o:spid="_x0000_s1040" type="#_x0000_t202"><x:ClientData ObjectType="Note"><x:Row>0</x:Row><x:Column>0</x:Column></x:ClientData></v:shape>"##,
        r##"<v:shape type="#_x0000_t202"><x:ClientData ObjectType="Note"><x:Row>1</x:Row><x:Column>0</x:Column></x:ClientData></v:shape>"##,
    ));
    let (wb, report) = read_xlsx(&bytes).unwrap();
    let texts: Vec<&str> = wb.sheet(0).unwrap().controls.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, ["A", "B"], "{:?}", report.warnings);
    let mut out = write_xlsx(&wb).unwrap();
    for _ in 0..2 {
        let vml = only_vml(&out);
        for caption in [">A</div>", ">B</div>", ">C</div>", ">Paid</div>"] {
            assert_eq!(vml.matches(caption).count(), 1, "each control once, kept or written: {caption} {vml}");
        }
        assert_eq!(vml.matches("ObjectType=\"Checkbox\"").count(), 1, "the check box is kept: {vml}");
        assert_eq!(note_shapes(&vml), 0, "{vml}");
        let (back, _) = read_xlsx(&out).unwrap();
        assert_eq!(back.sheet(0).unwrap().controls.len(), 2);
        out = write_xlsx(&back).unwrap();
    }
}
