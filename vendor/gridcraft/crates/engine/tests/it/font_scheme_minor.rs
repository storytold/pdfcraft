//! A font a formatting command derives from the theme's body font keeps following it (#2517).

use std::io::Read;

use gridcraft_engine::Session;
use serde_json::json;

fn styles_xml(session: &Session) -> String {
    let bytes = gridcraft_xlsx::write_xlsx(&session.doc().unwrap().wb).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    zip.by_name("xl/styles.xml").unwrap().read_to_string(&mut xml).unwrap();
    xml
}

#[test]
fn derived_body_font_keeps_minor_scheme() {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("home.bold", json!({"range": "A1"})).unwrap();
    session.execute("home.fontSize", json!({"range": "B1", "size": 14})).unwrap();
    let xml = styles_xml(&session);
    let fonts = xml.split("<fonts").nth(1).unwrap().split("</fonts>").next().unwrap();
    let fonts: Vec<&str> = fonts.split("<font>").skip(1).collect();
    assert!(fonts.len() >= 3, "{xml}");
    for f in &fonts {
        assert!(f.contains("<scheme val=\"minor\"/>"), "every body-font font follows the theme: {f}");
    }
}
