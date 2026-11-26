//! #1969: a wrapped cell's row must fit WORD-wrapped lines, not the character-fit
//! estimate (words can't split, so each drawn line ends early and narrow columns
//! need more lines than width/estimate suggests).

use gridcraft_engine::Session;
use serde_json::json;

fn session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s
}

fn row_height(s: &mut Session, row: u32) -> f32 {
    s.doc().unwrap().wb.active().unwrap().row_height(row)
}

#[test]
fn wrapped_row_fits_word_wrapped_lines() {
    let mut s = session();
    // Column fits ~10 characters; six-character words mean only one word per line.
    s.execute("home.columnWidth", json!({"cols": "A:A", "chars": 10})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "aaaaaa bbbbbb cccccc dddddd"})).unwrap();
    let plain = row_height(&mut s, 0);
    s.execute("home.wrapText", json!({"range": "A1", "on": true})).unwrap();
    let wrapped = row_height(&mut s, 0);
    // Word wrap: 4 lines (one word each). The old estimate: ceil(28/10) = 3 lines.
    // Line height at 11pt is ~19.8, so 4 lines need ~79 vs ~59 for 3.
    assert!(wrapped >= plain + 3.0 * 19.0, "4 word-wrapped lines must grow the row by ~4 line heights: plain={plain}, wrapped={wrapped}");
}

#[test]
fn issue_repro_nasa_sentence() {
    let mut s = session();
    s.execute("home.columnWidth", json!({"cols": "A:A", "chars": 12})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "Since the 1960s, NASA has left hardware across the solar system\u{2013}on the Moon, on Mars, and in deep space."}))
        .unwrap();
    s.execute("home.wrapText", json!({"range": "A1", "on": true})).unwrap();
    let h = row_height(&mut s, 0);
    // Excel shows this at ~10 lines in a 12-char column; the character estimate undercounts.
    assert!(h >= 9.0 * 19.0, "NASA sentence in a 12-char column needs ~10 lines, got height {h}");
}
