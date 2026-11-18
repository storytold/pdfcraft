//! `measure_geo` reads a map page's geospatial registration (ISO 32000-2 geospatial measure
//! viewports): the latitude and longitude of a page point, and the page point of a location.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// Call tool `name` and return its JSON result.
fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// A two-page PDF: page 1 is a map whose viewport [100 92 500 692] spans 34–35°N, 118–117°W in
/// WGS 84 (`gcs` overrides its coordinate system), beside a drawing-scale inset; page 2 is blank.
fn map(gcs: &str) -> Vec<u8> {
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 612 792] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /VP [
  << /Type /Viewport /Name (Quadrangle) /BBox [100 92 500 692] /Measure << /Type /Measure /Subtype /GEO
     /GPTS [34 -118 35 -118 35 -117 34 -117] /GCS {gcs} >> >>
  << /Type /Viewport /Name (Legend) /BBox [520 92 600 192] /Measure << /Type /Measure /Subtype /RL /R (1 in = 1 mi)
     /X [<< /U (mi) /C 0.0002 >>] /D [<< /U (mi) /C 1 >>] /A [<< /U (sq mi) /C 1 >>] >> >>
] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF"
    )
    .into_bytes()
}

fn near(v: &Value, expected: f64) {
    let got = v.as_f64().unwrap_or_else(|| panic!("{v} is not a number"));
    assert!((got - expected).abs() < 1e-6, "{got} != {expected}");
}

#[test]
fn a_map_page_reports_latitude_and_longitude_both_ways() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("map.pdf"), map("<< /Type /GEOGCS /EPSG 4326 >>")).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut a, "doc_open", json!({"path":"map.pdf"}))["doc"].as_u64().unwrap();

    // Display points are from the top left: the viewport's centre, user (300, 392), is (300, 400).
    let out = tool(&mut a, "measure_geo", json!({"doc":doc,"page":1,"at":[300,400]}));
    assert_eq!(out["georeferenced"], true);
    near(&out["location"]["lat"], 34.5);
    near(&out["location"]["lon"], -117.5);
    assert_eq!(out["location"]["dms"], "34°30'00.0\"N 117°30'00.0\"W");
    assert_eq!(out["location"]["method"], "geographic");
    assert_eq!(out["location"]["crs"], json!({"name":"WGS 84","epsg":4326,"projected":false}));
    assert_eq!(out["location"]["viewport"], "Quadrangle");
    let viewports = out["viewports"].as_array().unwrap();
    assert_eq!(viewports.len(), 2);
    assert_eq!(viewports[0]["rect"], json!([100.0, 100.0, 500.0, 700.0]));
    assert_eq!(viewports[0]["method"], "geographic");
    assert_eq!(viewports[1], json!({"name":"Legend","rect":[520.0, 600.0, 600.0, 700.0],"scale":true}));

    // Off the map, and on the inset, there is no location.
    assert_eq!(tool(&mut a, "measure_geo", json!({"doc":doc,"page":1,"at":[50,50]}))["location"], Value::Null);
    assert_eq!(tool(&mut a, "measure_geo", json!({"doc":doc,"page":1,"at":[550,650]}))["location"], Value::Null);

    // A latitude and longitude to the page, and a place the page doesn't show.
    let out = tool(&mut a, "measure_geo", json!({"doc":doc,"page":1,"lat":34.25,"lon":-117.25}));
    let point = out["point"].as_array().unwrap();
    near(&point[0], 400.0);
    near(&point[1], 550.0);
    assert!(out.get("location").is_none());
    assert_eq!(tool(&mut a, "measure_geo", json!({"doc":doc,"page":1,"lat":10,"lon":10}))["point"], Value::Null);

    // A page without a map says so.
    let blank = tool(&mut a, "measure_geo", json!({"doc":doc,"page":2,"at":[10,10]}));
    assert_eq!((blank["georeferenced"].clone(), blank["location"].clone(), blank["viewports"].clone()), (json!(false), Value::Null, json!([])));

    // Display coordinates follow the page's rotation: the same place, moved on screen.
    tool(&mut a, "page_rotate", json!({"doc":doc,"pages":[1],"degrees":90}));
    let rotated = tool(&mut a, "measure_geo", json!({"doc":doc,"page":1,"lat":34.25,"lon":-117.25}))["point"].clone();
    assert_ne!(rotated, json!(point));
    let back = tool(&mut a, "measure_geo", json!({"doc":doc,"page":1,"at":rotated}));
    near(&back["location"]["lat"], 34.25);
    near(&back["location"]["lon"], -117.25);

    // Saving keeps the registration as it was.
    tool(&mut a, "doc_save", json!({"doc":doc,"path":"saved.pdf"}));
    let saved = tool(&mut a, "doc_open", json!({"path":"saved.pdf"}))["doc"].as_u64().unwrap();
    let again = tool(&mut a, "measure_geo", json!({"doc":saved,"page":1,"at":rotated}));
    near(&again["location"]["lat"], 34.25);
}

#[test]
fn unusable_registrations_and_arguments_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("bad.pdf"), map("<< /Type /PROJCS /WKT (PROJCS[\"x\",) >>")).unwrap();
    std::fs::write(dir.path().join("albers.pdf"), map("<< /Type /PROJCS /WKT (PROJCS[\"Conus Albers\",PROJECTION[\"Albers_Conic_Equal_Area\"]]) >>"))
        .unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let bad = tool(&mut a, "doc_open", json!({"path":"bad.pdf"}))["doc"].as_u64().unwrap();
    let out = tool(&mut a, "measure_geo", json!({"doc":bad,"page":1,"at":[300,400]}));
    assert_eq!(out["georeferenced"], false);
    assert_eq!(out["location"], Value::Null);
    assert!(out["viewports"][0]["unsupported"].as_str().unwrap().contains("can't be read"), "{out}");

    // A projection PdfCraft doesn't implement is approximated, and says so.
    let albers = tool(&mut a, "doc_open", json!({"path":"albers.pdf"}))["doc"].as_u64().unwrap();
    let out = tool(&mut a, "measure_geo", json!({"doc":albers,"page":1,"at":[300,400]}));
    assert_eq!(out["location"]["method"], "approximate");
    assert_eq!(out["location"]["crs"]["name"], "Conus Albers");
    assert!(out["viewports"][0]["note"].as_str().unwrap().contains("Albers_Conic_Equal_Area"));

    for args in [
        json!({"doc":albers,"page":1,"lat":34}),
        json!({"doc":albers,"page":1,"lat":95,"lon":0}),
        json!({"doc":albers,"page":1,"at":[1e300,0]}),
        json!({"doc":albers,"page":9}),
        json!({"doc":albers,"page":1,"at":"here"}),
    ] {
        assert!(a.call("measure_geo", &args).is_err(), "{args} was accepted");
    }
}
