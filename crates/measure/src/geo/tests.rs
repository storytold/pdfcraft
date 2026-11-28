use super::*;
use pdfcraft_testkit::Pdf;
use std::sync::Arc;

/// A one-page document whose page has the viewports `vp` (PDF source of the array's items).
fn page_with(vp: &str) -> Document {
    let pdf = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>")
        .obj(format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /VP [{vp}] >>"))
        .stream("", "");
    Document::open(Arc::new(pdf.build())).unwrap()
}
/// A geospatial viewport over [100 100 500 700] with the given measure dictionary entries.
fn geo_viewport(entries: &str) -> String {
    format!("<< /Type /Viewport /Name (Map) /BBox [100 100 500 700] /Measure << /Type /Measure /Subtype /GEO {entries} >> >>")
}
/// A latitude/longitude rectangle 34–35°N, 118–117°W, in /Bounds' default order.
const CORNERS: &str = "/GPTS [34 -118 35 -118 35 -117 34 -117]";
fn near(a: f64, b: f64, tolerance: f64) {
    assert!((a - b).abs() <= tolerance, "{a} != {b} (±{tolerance})");
}
fn located(page: &GeoPage, at: Point) -> Location {
    page.locate(at).unwrap_or_else(|| panic!("{at:?} is not located"))
}
fn unsupported(page: &GeoPage) -> String {
    match &page.viewports.first().unwrap().kind {
        ViewportKind::Unsupported(why) => why.clone(),
        other => panic!("expected an unsupported viewport, got {other:?}"),
    }
}

#[test]
fn utm_matches_the_published_example_and_round_trips() {
    // The CN Tower: 43°38'33.24"N 79°23'13.7"W is 630084 m E, 4833439 m N in UTM zone 17.
    let p = utm(WGS84, 17, false).unwrap();
    let [e, n] = p.forward(43.642567_f64.to_radians(), (-79.387139_f64).to_radians()).unwrap();
    near(e, 630_084.0, 1.0);
    near(n, 4_833_439.0, 1.0);
    for lat in [-80.0, -45.5, -0.1, 0.0, 12.25, 60.0, 84.0] {
        for dlon in [-3.0, -1.5, 0.0, 0.75, 3.0] {
            let (lat, lon) = (f64::to_radians(lat), f64::to_radians(-81.0 + dlon));
            let back = p.inverse(p.forward(lat, lon).unwrap()).unwrap();
            near(back.0, lat, 1e-11);
            near(back.1, lon, 1e-11);
        }
    }
    // Southern zones have a false northing of 10 000 km.
    let [_, n] = utm(WGS84, 56, true).unwrap().forward(0.0, 153_f64.to_radians()).unwrap();
    near(n, 10_000_000.0, 1e-6);
    assert!(utm(WGS84, 0, false).is_none() && utm(WGS84, 61, false).is_none());
}

#[test]
fn lambert_matches_snyders_worked_example_and_round_trips() {
    // Snyder, Map Projections: A Working Manual, p. 296: Clarke 1866, parallels 33° and 45°,
    // origin 23°N 96°W; 35°N 75°W is x = 1 894 410.9 m, y = 1 564 649.5 m.
    let o = Origin { lat: 23_f64.to_radians(), lon: (-96_f64).to_radians(), k0: 1.0, east: 0.0, north: 0.0 };
    let p = Projection::Lambert(Lambert::new(CLARKE_1866, o, 33_f64.to_radians(), Some(45_f64.to_radians())).unwrap());
    let [x, y] = p.forward(35_f64.to_radians(), (-75_f64).to_radians()).unwrap();
    near(x, 1_894_410.9, 0.5);
    near(y, 1_564_649.5, 0.5);
    for lat in [10.0, 23.0, 35.5, 60.0] {
        for lon in [-120.0, -96.0, -75.0] {
            let (lat, lon) = (f64::to_radians(lat), f64::to_radians(lon));
            let back = p.inverse(p.forward(lat, lon).unwrap()).unwrap();
            near(back.0, lat, 1e-11);
            near(back.1, lon, 1e-11);
        }
    }
    // Parallels symmetric about the equator make a cylinder, not a cone.
    assert!(Lambert::new(GRS80, o, 30_f64.to_radians(), Some((-30_f64).to_radians())).is_none());
    // A southern-hemisphere cone (negative n) round-trips too.
    let south = Projection::Lambert(
        Lambert::new(GRS80, Origin { lat: (-30_f64).to_radians(), ..o }, (-25_f64).to_radians(), Some((-35_f64).to_radians())).unwrap(),
    );
    let back = south.inverse(south.forward((-31_f64).to_radians(), (-90_f64).to_radians()).unwrap()).unwrap();
    near(back.0, (-31_f64).to_radians(), 1e-11);
    near(back.1, (-90_f64).to_radians(), 1e-11);
}

#[test]
fn web_mercator_spans_the_published_extent() {
    let p = Projection::WebMercator { r: WGS84.a };
    near(p.forward(0.0, PI).unwrap()[0], 20_037_508.342_789_244, 1e-6);
    let back = p.inverse(p.forward(0.7, -2.0).unwrap()).unwrap();
    near(back.0, 0.7, 1e-12);
    near(back.1, -2.0, 1e-12);
    assert!(p.forward(FRAC_PI_2, 0.0).is_none());
}

#[test]
fn a_geographic_viewport_locates_points_inside_its_bounds() {
    let doc = page_with(&geo_viewport(&format!("{CORNERS} /GCS << /Type /GEOGCS /EPSG 4326 >>")));
    let page = GeoPage::read(&doc, 0).unwrap();
    assert!(page.georeferenced());
    let centre = located(&page, [300.0, 400.0]);
    near(centre.lat, 34.5, 1e-9);
    near(centre.lon, -117.5, 1e-9);
    assert_eq!(centre.method, Method::Geographic);
    assert_eq!(centre.crs, Crs { name: "WGS 84".into(), epsg: Some(4326), projected: false });
    assert_eq!(centre.viewport, "Map");
    assert_eq!(centre.decimal, "34.500000, -117.500000");
    assert_eq!(centre.dms, "34°30'00.0\"N 117°30'00.0\"W");
    let corner = located(&page, [100.0, 700.0]);
    near(corner.lat, 35.0, 1e-9);
    near(corner.lon, -118.0, 1e-9);
    // Outside the viewport, nothing.
    assert!(page.locate([50.0, 400.0]).is_none());
    assert!(page.locate([f64::NAN, 400.0]).is_none());
    // And back: a latitude and longitude to the page.
    let at = page.find(34.25, -117.25).unwrap();
    near(at[0], 400.0, 1e-6);
    near(at[1], 250.0, 1e-6);
    assert!(page.find(40.0, -117.5).is_none());
    assert!(page.find(f64::NAN, 0.0).is_none());
}

#[test]
fn a_utm_viewport_is_fitted_in_the_projected_plane() {
    // A 10 km square of UTM zone 11N whose corners are given as latitudes and longitudes: its centre
    // is the centre of the square in UTM, which a straight fit of latitude and longitude misses.
    let tm = utm(WGS84, 11, false).unwrap();
    let corner = |e: f64, n: f64| {
        let (lat, lon) = tm.inverse([e, n]).unwrap();
        format!("{} {}", lat.to_degrees(), lon.to_degrees())
    };
    let gpts =
        [corner(400_000.0, 3_770_000.0), corner(400_000.0, 3_780_000.0), corner(410_000.0, 3_780_000.0), corner(410_000.0, 3_770_000.0)].join(" ");
    let doc = page_with(&geo_viewport(&format!("/GPTS [{gpts}] /GCS << /Type /PROJCS /EPSG 32611 >>")));
    let page = GeoPage::read(&doc, 0).unwrap();
    let centre = located(&page, [300.0, 400.0]);
    let (lat, lon) = tm.inverse([405_000.0, 3_775_000.0]).unwrap();
    near(centre.lat, lat.to_degrees(), 1e-9);
    near(centre.lon, lon.to_degrees(), 1e-9);
    assert_eq!(centre.method, Method::Projected);
    assert_eq!(centre.crs.name, "WGS 84 / UTM zone 11N");
    let at = page.find(centre.lat, centre.lon).unwrap();
    near(at[0], 300.0, 1e-6);
    near(at[1], 400.0, 1e-6);

    // The same points in a projection PdfCraft doesn't implement are fitted directly, and say so.
    let doc = page_with(&geo_viewport(&format!("/GPTS [{gpts}] /GCS << /Type /PROJCS /EPSG 2229 >>")));
    let rough = located(&GeoPage::read(&doc, 0).unwrap(), [300.0, 400.0]);
    assert_eq!(rough.method, Method::Approximate);
    assert_eq!(rough.crs.name, "EPSG:2229");
    near(rough.lat, centre.lat, 1e-3);
    assert!((rough.lat - centre.lat).abs() > 1e-9 || (rough.lon - centre.lon).abs() > 1e-9);
}

#[test]
fn wkt_names_the_projection_in_either_version() {
    let wkt1 = r#"PROJCS["NAD83 / UTM zone 11N",GEOGCS["NAD83",DATUM["North_American_Datum_1983",SPHEROID["GRS 1980",6378137,298.257222101,AUTHORITY["EPSG","7019"]]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],PARAMETER["central_meridian",-117],PARAMETER["scale_factor",0.9996],PARAMETER["false_easting",500000],PARAMETER["false_northing",0],UNIT["metre",1],AXIS["Easting",EAST],AXIS["Northing",NORTH]]"#;
    let (known, why) = from_wkt(wkt1).unwrap();
    assert!(why.is_ok() && known.projected);
    assert_eq!(known.name, "NAD83 / UTM zone 11N");
    // The same zone, to the last digit of the WKT's own degree (0.0174532925199433).
    let (ours, theirs) = (known.projection.unwrap(), utm(GRS80, 11, false).unwrap());
    for (lat, lon) in [(34.0_f64, -117.0_f64), (36.5, -119.5)] {
        let (a, b) = (ours.forward(lat.to_radians(), lon.to_radians()).unwrap(), theirs.forward(lat.to_radians(), lon.to_radians()).unwrap());
        near(a[0], b[0], 1e-6);
        near(a[1], b[1], 1e-6);
    }

    let wkt2 = r#"PROJCRS["NAD83 / California zone 5",BASEGEOGCRS["NAD83",DATUM["North American Datum 1983",ELLIPSOID["GRS 1980",6378137,298.257222101,LENGTHUNIT["metre",1]]],ANGLEUNIT["degree",0.0174532925199433]],CONVERSION["SPCS83 California zone 5",METHOD["Lambert Conic Conformal (2SP)",ID["EPSG",9802]],PARAMETER["Latitude of false origin",33.5,ANGLEUNIT["degree",0.0174532925199433]],PARAMETER["Longitude of false origin",-118,ANGLEUNIT["degree",0.0174532925199433]],PARAMETER["Latitude of 1st standard parallel",35.4666666666667,ANGLEUNIT["degree",0.0174532925199433]],PARAMETER["Latitude of 2nd standard parallel",34.0333333333333,ANGLEUNIT["degree",0.0174532925199433]],PARAMETER["Easting at false origin",2000000,LENGTHUNIT["metre",1]],PARAMETER["Northing at false origin",500000,LENGTHUNIT["metre",1]]],CS[Cartesian,2],AXIS["easting (X)",east,ORDER[1]],AXIS["northing (Y)",north,ORDER[2]],LENGTHUNIT["metre",1]]"#;
    let (known, why) = from_wkt(wkt2).unwrap();
    assert!(why.is_ok());
    let Some(Projection::Lambert(l)) = &known.projection else { panic!("{:?}", known.projection) };
    near(l.lon0, (-118_f64).to_radians(), 1e-12);
    near(l.false_easting, 2_000_000.0, 1e-9);

    let geographic =
        r#"GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
    let (known, why) = from_wkt(geographic).unwrap();
    assert!(why.is_ok() && !known.projected && known.projection.is_none());
    let compound = format!(r#"COMPD_CS["x",{geographic},VERT_CS["h",VERT_DATUM["d",2005],UNIT["metre",1]]]"#);
    assert!(!from_wkt(&compound).unwrap().0.projected);

    let albers = r#"PROJCS["NAD83 / Conus Albers",GEOGCS["NAD83",DATUM["D",SPHEROID["GRS 1980",6378137,298.257222101]]],PROJECTION["Albers_Conic_Equal_Area"],PARAMETER["central_meridian",-96]]"#;
    let (known, why) = from_wkt(albers).unwrap();
    assert!(known.projected && known.projection.is_none());
    assert!(why.unwrap_err().contains("Albers_Conic_Equal_Area"));
}

#[test]
fn a_viewport_with_unsupported_wkt_is_approximated_and_says_why() {
    let wkt = r#"PROJCS["Albers",GEOGCS["G",DATUM["D",SPHEROID["S",6378137,298.257]]],PROJECTION["Albers_Conic_Equal_Area"]]"#;
    let doc = page_with(&geo_viewport(&format!("{CORNERS} /GCS << /Type /PROJCS /WKT ({wkt}) >>")));
    let page = GeoPage::read(&doc, 0).unwrap();
    let ViewportKind::Geo(r) = &page.viewports[0].kind else { panic!("{:?}", page.viewports[0].kind) };
    assert_eq!(r.method, Method::Approximate);
    assert!(r.note.as_deref().unwrap().contains("Albers"));
    assert_eq!(r.crs.name, "Albers");
}

#[test]
fn malformed_wkt_is_refused_without_panicking() {
    for bad in [
        "",
        "PROJCS[",
        "PROJCS[\"x\"",
        "PROJCS[\"x\",]",
        "PROJCS[\"unterminated]",
        "PROJCS[\"x\"] trailing",
        "PROJCS[\"x\",1e999]",
        "PROJCS[\"x\",-]",
        "[\"x\"]",
        "PROJCS(\"x\"]",
        "VERT_CS[\"h\"]",
    ] {
        assert!(from_wkt(bad).is_err(), "{bad:?} was accepted");
    }
    let deep = format!("{}{}", "A[".repeat(100), "]".repeat(100));
    assert!(from_wkt(&deep).unwrap_err().contains("nested"));
    let long = format!("GEOGCS[\"{}\"]", "x".repeat(MAX_WKT));
    assert!(from_wkt(&long).unwrap_err().contains("too long"));
    let many = format!("GEOGCS[\"x\"{}]", ",A[1]".repeat(MAX_WKT_NODES));
    assert!(from_wkt(&many).unwrap_err().contains("too long"));
    // Non-ASCII text and odd spacing parse.
    assert_eq!(from_wkt(" GEOGCS ( \"Système \"\"géodésique\"\"\" , UNIT[\"degree\", 0.0174] ) ").unwrap().0.name, "Système \"géodésique\"");
}

#[test]
fn unusable_geospatial_dictionaries_are_reported_with_a_reason() {
    let gcs = "/GCS << /Type /GEOGCS /EPSG 4326 >>";
    for (entries, reason) in [
        (gcs.to_string(), "no /GPTS"),
        (CORNERS.to_string(), "no /GCS"),
        (format!("{CORNERS} /GCS 5"), "not a dictionary"),
        (format!("/GPTS [34 -118 35 -118 35] {gcs}"), "three pairs"),
        (format!("/GPTS [34 -118 35 -118] {gcs}"), "three pairs"),
        (format!("/GPTS [34 -118 35 (x) 35 -117 34 -117] {gcs}"), "other than a number"),
        (format!("/GPTS 7 {gcs}"), "not an array"),
        (format!("/GPTS [{}] {gcs}", "1 2 ".repeat(MAX_GEO_POINTS + 1)), "too many"),
        (format!("/GPTS [95 -118 35 -118 35 -117 34 -117] {gcs}"), "out of range"),
        (format!("{CORNERS} /LPTS [0 0 1 1 0.5 0.5] {gcs}"), "different numbers"),
        (format!("/GPTS [34 -118 35 -117 36 -116] /LPTS [0 0 0.5 0.5 1 1] {gcs}"), "one line"),
        (format!("{CORNERS} /LPTS [0 0 0 1 1 1 1e9 0] {gcs}"), "out of range"),
        (format!("{CORNERS} /GCS << /Type /PROJCS /WKT (PROJCS[) >>"), "can't be read"),
    ] {
        let doc = page_with(&geo_viewport(&entries));
        let page = GeoPage::read(&doc, 0).unwrap();
        match &page.viewports[0].kind {
            ViewportKind::Unsupported(why) => assert!(why.contains(reason), "{entries}: {why}"),
            other => panic!("{entries}: {other:?}"),
        }
        assert!(page.locate([300.0, 400.0]).is_none(), "{entries}");
    }
    // A control point beyond a projection's reach.
    let doc = page_with(&geo_viewport("/GPTS [90 0 89 1 89 2 88 3] /GCS << /Type /PROJCS /EPSG 3857 >>"));
    assert!(unsupported(&GeoPage::read(&doc, 0).unwrap()).contains("outside its projection"));
    // A viewport without area.
    let doc = page_with(&format!("<< /BBox [100 100 100 700] /Measure << /Subtype /GEO {CORNERS} /GCS << /EPSG 4326 >> >> >>"));
    assert!(unsupported(&GeoPage::read(&doc, 0).unwrap()).contains("no area"));
}

#[test]
fn the_last_viewport_containing_a_point_decides_and_bounds_limit_the_area() {
    let geo = geo_viewport(&format!("{CORNERS} /GCS << /Type /GEOGCS /EPSG 4326 >>"));
    // A drawing-scale inset over the map's lower left.
    let inset = "<< /BBox [100 100 200 200] /Measure << /Type /Measure /Subtype /RL >> >>";
    let page = GeoPage::read(&page_with(&format!("{geo} {inset}")), 0).unwrap();
    assert!(page.locate([150.0, 150.0]).is_none());
    assert!(page.locate([300.0, 400.0]).is_some());
    // The same inset under the map doesn't hide it; viewports without a measure are skipped.
    let page = GeoPage::read(&page_with(&format!("{inset} {geo} << /BBox [0 0 612 792] >> 7 << /Measure 1 >>")), 0).unwrap();
    assert!(page.locate([150.0, 150.0]).is_some());
    assert_eq!(page.viewports.len(), 2);
    // /Bounds: only the lower-left triangle of the viewport is the map.
    let page = GeoPage::read(
        &page_with(&geo_viewport("/Bounds [0 0 0 1 1 0] /GPTS [34 -118 35 -118 34 -117] /LPTS [0 0 0 1 1 0] /GCS << /EPSG 4326 >>")),
        0,
    )
    .unwrap();
    near(located(&page, [150.0, 200.0]).lat, 34.0 + 100.0 / 600.0, 1e-9);
    assert!(page.locate([450.0, 650.0]).is_none());
    assert!(page.find(34.9, -117.1).is_none());
    // A page without viewports, or with an invalid /VP, is simply not georeferenced (or an error).
    assert!(!GeoPage::read(&page_with(""), 0).unwrap().georeferenced());
    assert!(GeoPage::read(&page_with(""), 3).is_err());
}

#[test]
fn a_map_across_the_antimeridian_wraps_its_longitudes() {
    let doc = page_with(&geo_viewport("/GPTS [-20 179 -19 179 -19 -179 -20 -179] /GCS << /Type /GEOGCS /EPSG 4326 >>"));
    let page = GeoPage::read(&doc, 0).unwrap();
    near(located(&page, [300.0, 400.0]).lon.abs(), 180.0, 1e-9);
    near(located(&page, [400.0, 400.0]).lon, -179.5, 1e-9);
    let at = page.find(-19.5, -179.5).unwrap();
    near(at[0], 400.0, 1e-6);
}

#[test]
fn shared_and_indirect_objects_resolve() {
    let pdf = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /VP 5 0 R >>")
        .stream("", "")
        .obj("[6 0 R 6 0 R]")
        .obj("<< /BBox 7 0 R /Measure << /Subtype /GEO /GPTS 8 0 R /GCS << /Type /GEOGCS /EPSG 4326 >> >> >>")
        .obj("[100 100 500 700]")
        .obj("[34 -118 35 -118 35 -117 34 -117]");
    let doc = Document::open(Arc::new(pdf.build())).unwrap();
    let page = GeoPage::read(&doc, 0).unwrap();
    assert_eq!(page.viewports.len(), 2);
    near(located(&page, [300.0, 400.0]).lat, 34.5, 1e-9);
}

#[test]
fn degrees_minutes_and_seconds_round_and_carry() {
    assert_eq!(dms(34.13675, -118.125028), "34°08'12.3\"N 118°07'30.1\"W");
    assert_eq!(dms(-33.999_999_9, 151.0), "34°00'00.0\"S 151°00'00.0\"E");
    assert_eq!(dms(0.0, 0.0), "0°00'00.0\"N 0°00'00.0\"E");
    assert_eq!(dms(f64::INFINITY, -1e300), "360°00'00.0\"N 360°00'00.0\"W");
}

#[test]
fn longitudes_written_from_0_to_360_find_their_page_point() {
    // 242°E is 118°W: locate reports -117.5, and find must accept the -180..180 form it reports.
    let doc = page_with(&geo_viewport("/GPTS [34 242 35 242 35 243 34 243] /GCS << /Type /GEOGCS /EPSG 4326 >>"));
    let page = GeoPage::read(&doc, 0).unwrap();
    let centre = located(&page, [300.0, 400.0]);
    near(centre.lon, -117.5, 1e-9);
    let at = page.find(34.5, -117.5).expect("the page shows 34.5°N 117.5°W");
    near(at[0], 300.0, 1e-6);
    near(at[1], 400.0, 1e-6);
    near(page.find(34.5, 242.5).unwrap()[0], 300.0, 1e-6);
}

#[test]
fn find_skips_a_location_another_viewport_covers() {
    // Two maps in the same rectangle: the upper one is what the page shows there.
    let lower = geo_viewport(&format!("{CORNERS} /GCS << /Type /GEOGCS /EPSG 4326 >>"));
    let upper = geo_viewport("/GPTS [40 -120 41 -120 41 -119 40 -119] /GCS << /Type /GEOGCS /EPSG 4326 >>");
    let page = GeoPage::read(&page_with(&format!("{lower} {upper}")), 0).unwrap();
    assert!(page.find(34.5, -117.5).is_none(), "the lower map is hidden");
    let at = page.find(40.5, -119.5).unwrap();
    near(at[0], 300.0, 1e-6);
    near(located(&page, at).lat, 40.5, 1e-9);
}

#[test]
fn edge_longitudes_of_180_keep_their_side() {
    // Rows from review: whole-world maps, a map written 0..360, and a map from 0 to 180.
    for (gpts, gcs, lat, lon) in [
        ("[-90 -180 90 -180 90 180 -90 180]", "/EPSG 4326", 0.0, 0.0),
        ("[-85 -180 85 -180 85 180 -85 180]", "/Type /PROJCS /EPSG 3857", 0.0, 0.0),
        ("[-90 0 90 0 90 360 -90 360]", "/EPSG 4326", 0.0, 180.0),
        ("[-90 0 90 0 90 180 -90 180]", "/EPSG 4326", 0.0, 90.0),
    ] {
        let page = GeoPage::read(&page_with(&geo_viewport(&format!("/GPTS {gpts} /GCS << {gcs} >>"))), 0).unwrap();
        let centre = page.locate([300.0, 400.0]).unwrap_or_else(|| panic!("{gpts} {gcs}: {:?}", page.viewports[0].kind));
        near(centre.lat, lat, 1e-6);
        // 180 and -180 are one meridian.
        near((centre.lon - lon + 540.0).rem_euclid(360.0) - 180.0, 0.0, 1e-6);
    }
    // The 0..180 map isn't mirrored: a quarter across is 45°E.
    let page = GeoPage::read(&page_with(&geo_viewport("/GPTS [-90 0 90 0 90 180 -90 180] /GCS << /EPSG 4326 >>")), 0).unwrap();
    near(located(&page, [200.0, 400.0]).lon, 45.0, 1e-6);
    near(page.find(0.0, 45.0).unwrap()[0], 200.0, 1e-6);
}

#[test]
fn a_web_mercator_map_across_the_antimeridian_stays_continuous() {
    for gpts in ["[34 179 35 179 35 181 34 181]", "[34 179 35 179 35 -179 34 -179]"] {
        let page = GeoPage::read(&page_with(&geo_viewport(&format!("/GPTS {gpts} /GCS << /Type /PROJCS /EPSG 3857 >>"))), 0).unwrap();
        let centre = located(&page, [300.0, 400.0]);
        near(centre.lon.abs(), 180.0, 1e-6);
        let east = located(&page, [400.0, 400.0]);
        near(east.lon, -179.5, 1e-6);
        let at = page.find(east.lat, -179.5).unwrap();
        near(at[0], 400.0, 1e-6);
        near(page.find(east.lat, 180.5 - 360.0).unwrap()[0], 400.0, 1e-6);
    }
}

#[test]
fn wkt1_parameters_without_a_unit_take_the_geographic_unit() {
    // OGC 01-009: PROJCS parameters are in the GEOGCS angular unit. 50 grads is 45°.
    let grads = r#"PROJCS["NTF (Paris) / Lambert zone II",GEOGCS["NTF (Paris)",DATUM["D",SPHEROID["Clarke 1880 (IGN)",6378249.2,293.4660212936269]],PRIMEM["Paris",2.33722917],UNIT["grad",0.01570796326794897]],PROJECTION["Lambert_Conformal_Conic_1SP"],PARAMETER["latitude_of_origin",50],PARAMETER["central_meridian",0],PARAMETER["scale_factor",0.99987742],PARAMETER["false_easting",600000],PARAMETER["false_northing",2200000],UNIT["metre",1]]"#;
    let (known, why) = from_wkt(grads).unwrap();
    assert!(why.is_ok());
    let Some(Projection::Lambert(l)) = &known.projection else { panic!("{:?}", known.projection) };
    near(l.n, 45_f64.to_radians().sin(), 1e-12);
    // WKT2: an explicit parameter unit wins over the base CRS's.
    let wkt2 = r#"PROJCRS["x",BASEGEOGCRS["g",DATUM["d",ELLIPSOID["GRS 1980",6378137,298.257222101]],ANGLEUNIT["grad",0.01570796326794897]],CONVERSION["c",METHOD["Lambert Conic Conformal (1SP)"],PARAMETER["Latitude of natural origin",45,ANGLEUNIT["degree",0.0174532925199433]],PARAMETER["Longitude of natural origin",10],PARAMETER["Scale factor at natural origin",1]]]"#;
    let (known, _) = from_wkt(wkt2).unwrap();
    let Some(Projection::Lambert(l)) = &known.projection else { panic!("{:?}", known.projection) };
    near(l.n, 45_f64.to_radians().sin(), 1e-12);
    near(l.lon0, 9_f64.to_radians(), 1e-12);
}
