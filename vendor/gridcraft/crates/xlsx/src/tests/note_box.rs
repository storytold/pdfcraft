//! #1608: a note's box keeps the place and size the file gave it through open and save.

use std::io::Read;

use gridcraft_core::CellRef;
use gridcraft_model::{Comment, NoteBox, NoteCorner, Workbook};

use super::minimal;
use crate::{read_xlsx, write_xlsx};

const RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="../comments1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/></Relationships>"#;
const COMMENTS: &str = r#"<?xml version="1.0"?><comments xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><authors><author>A</author></authors><commentList><comment ref="B2" authorId="0"><text><t>note</t></text></comment></commentList></comments>"#;
/// The issue's note on B2: Excel's box over B1, 224.25 x 89.25 pt.
const VML: &str = r##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="1"/></o:shapelayout><v:shape id="_x0000_s1025" type="#_x0000_t202" style="position:absolute;margin-left:66pt;margin-top:2.25pt;width:224.25pt;height:89.25pt;z-index:1;visibility:hidden" fillcolor="#ffffe1" o:insetmode="auto"><v:textbox><div>note</div></v:textbox><x:ClientData ObjectType="Note"><x:MoveWithCells/><x:SizeWithCells/><x:Anchor>
    1, 10, 0, 2, 5, 32, 5, 8</x:Anchor><x:AutoFill>False</x:AutoFill><x:Row>1</x:Row><x:Column>1</x:Column></x:ClientData></v:shape></xml>"##;

fn package() -> Vec<u8> {
    minimal(
        r#"<sheetData><row r="2"><c r="B2"><v>1</v></c></row></sheetData><legacyDrawing r:id="rId2"/>"#,
        &[("xl/worksheets/_rels/sheet1.xml.rels", RELS), ("xl/comments1.xml", COMMENTS), ("xl/drawings/vmlDrawing1.vml", VML)],
        "",
        "",
    )
}

/// The written VML part (GridCraft writes one per sheet with notes).
fn written_vml(bytes: &[u8]) -> String {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let name = (0..z.len()).map(|i| z.by_index(i).unwrap().name().to_string()).find(|n| n.ends_with(".vml")).expect("a VML part");
    let mut s = String::new();
    z.by_name(&name).unwrap().read_to_string(&mut s).unwrap();
    s
}

fn b2() -> CellRef {
    CellRef::new(1, 1)
}

#[test]
fn a_notes_box_is_read_relative_to_its_cell() {
    let (wb, _) = read_xlsx(&package()).unwrap();
    let c = &wb.sheets[0].comments[&b2()];
    assert_eq!(
        c.note_box,
        Some(NoteBox {
            from: NoteCorner { col: 0, col_px: 10, row: -1, row_px: 2 },
            to: NoteCorner { col: 4, col_px: 32, row: 4, row_px: 8 },
            size_pt: Some((224.25, 89.25)),
        })
    );
}

#[test]
fn an_unedited_notes_box_is_saved_where_and_as_big_as_it_was() {
    let (wb, _) = read_xlsx(&package()).unwrap();
    let vml = written_vml(&write_xlsx(&wb).unwrap());
    assert!(vml.contains("<x:Anchor>1, 10, 0, 2, 5, 32, 5, 8</x:Anchor>"), "{vml}");
    assert!(vml.contains("width:224.25pt;height:89.25pt"), "{vml}");
    // and once more through GridCraft's own output
    let (wb2, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert_eq!(wb2.sheets[0].comments[&b2()].note_box, wb.sheets[0].comments[&b2()].note_box);
}

#[test]
fn a_notes_box_moves_with_its_note() {
    // A row and a column inserted before the note move it to C3; its box follows it.
    let (mut wb, _) = read_xlsx(&package()).unwrap();
    let s = wb.sheet_mut(0).unwrap();
    let c = s.comments.remove(&b2()).unwrap();
    s.comments.insert(CellRef::new(2, 2), c);
    let vml = written_vml(&write_xlsx(&wb).unwrap());
    assert!(vml.contains("<x:Anchor>2, 10, 1, 2, 6, 32, 6, 8</x:Anchor>"), "{vml}");
}

#[test]
fn a_box_that_would_start_before_the_sheet_is_clamped_to_it() {
    // The B2 note's box starts one row above it; moved to A1 that row doesn't exist.
    let (mut wb, _) = read_xlsx(&package()).unwrap();
    let s = wb.sheet_mut(0).unwrap();
    let c = s.comments.remove(&b2()).unwrap();
    s.comments.insert(CellRef::new(0, 0), c);
    let vml = written_vml(&write_xlsx(&wb).unwrap());
    assert!(vml.contains("<x:Anchor>0, 10, 0, 2, 4, 32, 4, 8</x:Anchor>"), "{vml}");
}

#[test]
fn a_new_note_gets_the_default_box() {
    let mut wb = Workbook::new();
    let note = Comment {
        author: "A".into(),
        text: "t".into(),
        replies: vec![],
        threaded: false,
        resolved: false,
        visible: false,
        dates: vec![],
        note_box: None,
    };
    wb.sheet_mut(0).unwrap().comments.insert(b2(), note);
    let vml = written_vml(&write_xlsx(&wb).unwrap());
    assert!(vml.contains("<x:Anchor>2, 15, 1, 2, 4, 15, 5, 16</x:Anchor>") && vml.contains("width:108pt;height:59.25pt"), "{vml}");
}

#[test]
fn a_damaged_anchor_is_ignored() {
    for anchor in ["1, 10, 0", "1, x, 0, 2, 5, 32, 5, 8", "-1, 10, 0, 2, 5, 32, 5, 8", "99999999999, 0, 0, 0, 0, 0, 0, 0"] {
        let vml = VML.replace("\n    1, 10, 0, 2, 5, 32, 5, 8", anchor);
        let bytes = minimal(
            r#"<sheetData/><legacyDrawing r:id="rId2"/>"#,
            &[("xl/worksheets/_rels/sheet1.xml.rels", RELS), ("xl/comments1.xml", COMMENTS), ("xl/drawings/vmlDrawing1.vml", &vml)],
            "",
            "",
        );
        let (wb, _) = read_xlsx(&bytes).unwrap();
        assert_eq!(wb.sheets[0].comments[&b2()].note_box, None, "{anchor}");
    }
}
