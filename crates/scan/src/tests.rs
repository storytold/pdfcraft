use std::sync::atomic::AtomicBool;

use crate::*;

#[test]
fn presets_and_names_round_trip() {
    for p in Preset::ALL {
        assert_eq!(Preset::from_name(p.name()), Some(p));
        assert_eq!(Preset::matching(&p.settings()), Some(p));
    }
    assert_eq!(Preset::BlackWhiteDocument.settings().color, ColorMode::BlackWhite);
    assert_eq!(Source::from_name("duplex"), Some(Source::FeederDuplex));
    assert_eq!(Paper::from_name("A4"), Some(Paper::A4));
    assert_eq!(ColorMode::from_name("grey"), Some(ColorMode::Gray));
}

#[test]
fn closest_resolution_and_area() {
    assert_eq!(closest_dpi(&[75, 150, 300, 600], 200), Some(150));
    assert_eq!(closest_dpi(&[75, 150, 300, 600], 225), Some(300));
    assert_eq!(closest_dpi(&[], 300), None);
    assert_eq!(area_mm(Paper::Legal, Some((216.0, 297.0))), Some((215.9, 297.0)));
    assert_eq!(area_mm(Paper::Full, Some((216.0, 297.0))), Some((216.0, 297.0)));
    assert_eq!(area_mm(Paper::Full, None), None);
}

#[test]
fn unknown_ids_are_refused() {
    let no = AtomicBool::new(false);
    assert!(matches!(scan("hp:1", &ScanSettings::default(), &no), Err(ScanError::UnknownId(_))));
    assert!(matches!(scan("escl:ftp://x/eSCL", &ScanSettings::default(), &no), Err(ScanError::UnknownId(_))));
}

// ── eSCL ──────────────────────────────────────────────────────────────────────────────────

const CAPS: &str = r#"<?xml version="1.0"?>
<scan:ScannerCapabilities xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03" xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm">
  <pwg:MakeAndModel>  ACME   Scan 9000 </pwg:MakeAndModel>
  <scan:Platen><scan:PlatenInputCaps>
    <scan:MaxWidth>2550</scan:MaxWidth><scan:MaxHeight>3508</scan:MaxHeight>
    <scan:SettingProfiles><scan:SettingProfile>
      <scan:ColorModes><scan:ColorMode>Grayscale8</scan:ColorMode><scan:ColorMode>RGB24</scan:ColorMode></scan:ColorModes>
      <scan:DocumentFormats><pwg:DocumentFormat>image/jpeg</pwg:DocumentFormat><pwg:DocumentFormat>application/pdf</pwg:DocumentFormat></scan:DocumentFormats>
      <scan:SupportedResolutions><scan:DiscreteResolutions>
        <scan:DiscreteResolution><scan:XResolution>100</scan:XResolution><scan:YResolution>100</scan:YResolution></scan:DiscreteResolution>
        <scan:DiscreteResolution><scan:XResolution>200</scan:XResolution><scan:YResolution>200</scan:YResolution></scan:DiscreteResolution>
        <scan:DiscreteResolution><scan:XResolution>300</scan:XResolution><scan:YResolution>300</scan:YResolution></scan:DiscreteResolution>
      </scan:DiscreteResolutions></scan:SupportedResolutions>
    </scan:SettingProfile></scan:SettingProfiles>
  </scan:PlatenInputCaps></scan:Platen>
</scan:ScannerCapabilities>"#;

#[test]
fn escl_capabilities_are_read_and_jobs_planned() {
    let caps = escl::parse_capabilities(CAPS).unwrap();
    assert_eq!(caps.make_and_model, "ACME Scan 9000");
    let platen = caps.platen.as_ref().unwrap();
    assert_eq!(platen.resolutions, vec![100, 200, 300]);
    assert!(caps.adf_simplex.is_none());
    // Black and white falls back to gray on a device without 1-bit; no PNG, so JPEG.
    let s = ScanSettings { color: ColorMode::BlackWhite, dpi: 250, source: Source::Flatbed, paper: Paper::A4 };
    let job = escl::plan(&caps, &s).unwrap();
    assert_eq!((job.input_source, job.color_mode.as_str(), job.format.as_str()), ("Platen", "Grayscale8", "image/jpeg"));
    assert_eq!(job.dpi, 300);
    assert_eq!((job.width, job.height), (2480, 3508));
    // No feeder.
    let feeder = ScanSettings { source: Source::Feeder, ..s };
    assert_eq!(escl::plan(&caps, &feeder), Err(ScanError::NoSource("document feeder")));
    let xml = escl::settings_xml(&job);
    assert!(xml.contains("<pwg:InputSource>Platen</pwg:InputSource>") && xml.contains("<scan:XResolution>300</scan:XResolution>"), "{xml}");
    assert!(roxmltree::Document::parse(&xml).is_ok());
}

#[test]
fn escl_hostile_documents_are_errors_not_crashes() {
    for bad in
        ["", "<", "<x/>", "<ScannerCapabilities><Platen><PlatenInputCaps><MaxWidth>-5</MaxWidth></PlatenInputCaps></Platen></ScannerCapabilities>"]
    {
        let _ = escl::parse_capabilities(bad);
    }
    let range = "<ScannerCapabilities><Platen><PlatenInputCaps><XResolutionRange><Min>300</Min><Max>100</Max><Step>0</Step></XResolutionRange></PlatenInputCaps></Platen></ScannerCapabilities>";
    let caps = escl::parse_capabilities(range).unwrap();
    assert_eq!(caps.platen.as_ref().unwrap().range, None);
    let job = escl::plan(&caps, &ScanSettings { paper: Paper::Full, ..ScanSettings::default() }).unwrap();
    assert!(job.width > 0 && job.height > 0);
    assert_eq!(escl::parse_status("<<<"), escl::Status::default());
    let st = escl::parse_status("<ScannerStatus><State>Idle</State><AdfState>ScannerAdfEmpty</AdfState></ScannerStatus>");
    assert_eq!(st.error(), Some(ScanError::FeederEmpty));
}

#[test]
fn escl_addresses_are_normalized() {
    assert_eq!(escl::normalize_base("192.168.1.20").as_deref(), Some("http://192.168.1.20/eSCL"));
    assert_eq!(escl::normalize_base("http://printer.local:8080/eSCL/"), None);
    assert_eq!(escl::normalize_base("https://[fe80::1]/escl").as_deref(), Some("https://[fe80::1]/escl"));
    assert_eq!(escl::normalize_base("file:///etc/passwd"), None);
    assert_eq!(escl::normalize_base("user@host"), None);
    assert_eq!(escl::normalize_base(""), None);
    let base = "http://10.0.0.5:80/eSCL";
    assert_eq!(escl::resolve_location(base, "/eSCL/ScanJobs/7/").as_deref(), Some("http://10.0.0.5/eSCL/ScanJobs/7"));
    assert_eq!(escl::resolve_location(base, "http://10.0.0.5/eSCL/ScanJobs/7").as_deref(), Some("http://10.0.0.5/eSCL/ScanJobs/7"));
    assert_eq!(escl::resolve_location(base, "ScanJobs/7").as_deref(), Some("http://10.0.0.5/eSCL/ScanJobs/7"));
    assert_eq!(escl::id_for("10.0.0.5".parse().unwrap(), 8080, Some("/eSCL")), "escl:http://10.0.0.5:8080/eSCL");
    assert_eq!(escl::scanner_name("HP OfficeJet [12AB]._uscan._tcp.local.", None), "HP OfficeJet [12AB]");
}

#[test]
fn scanner_addresses_and_job_locations_cannot_escape_the_local_device() {
    for address in ["127.0.0.1", "10.0.0.1", "172.16.0.1", "192.168.1.2", "169.254.1.2", "[::1]", "[fd00::1]", "[fe80::1]", "[::ffff:192.168.1.2]"] {
        assert!(escl::normalize_base(address).is_some(), "{address}");
    }
    for address in [
        "8.8.8.8",
        "172.32.0.1",
        "100.64.0.1",
        "0.0.0.0",
        "224.0.0.1",
        "[2001:4860:4860::8888]",
        "[::ffff:8.8.8.8]",
        "example.com",
        "localhost",
        "http://127.0.0.1@8.8.8.8/",
        "http://127.0.0.1/?host=8.8.8.8",
        "http://127.0.0.1/#fragment",
        "http://127.0.0.1\\@8.8.8.8/",
    ] {
        assert!(escl::normalize_base(address).is_none(), "{address}");
    }
    let base = "http://127.0.0.1:8080/eSCL";
    for location in [
        "http://8.8.8.8/job",
        "http://127.0.0.2:8080/job",
        "http://127.0.0.1:8081/job",
        "https://127.0.0.1:8080/job",
        "//8.8.8.8/job",
        "//127.0.0.2:8080/job",
        "http://user@127.0.0.1:8080/job",
    ] {
        assert!(escl::resolve_location(base, location).is_none(), "{location}");
    }
    assert_eq!(escl::resolve_location(base, "ScanJobs/1").as_deref(), Some("http://127.0.0.1:8080/eSCL/ScanJobs/1"));
}

#[test]
fn scanner_http_redirects_are_refused_without_following_them() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let mut bytes = [0; 4096];
        assert!(stream.read(&mut bytes).unwrap() > 0);
        stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://8.8.8.8/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let result = escl::capabilities(&format!("http://{address}/eSCL"));
    worker.join().unwrap();
    assert!(matches!(result, Err(ScanError::Failed(ref message)) if message.contains("redirects")), "{result:?}");
}

#[test]
fn wia_is_unavailable_without_launching_a_helper() {
    assert!(matches!(scan("wia:device", &ScanSettings::default(), &AtomicBool::new(false)), Err(ScanError::BackendMissing(_))));
}

fn png_size(bytes: &[u8]) -> (u32, u32) {
    let d = png::Decoder::new(std::io::Cursor::new(bytes));
    let r = d.read_info().unwrap();
    (r.info().width, r.info().height)
}

#[cfg(feature = "fake-escl")]
#[test]
fn escl_flatbed_and_duplex_feeder_against_a_fake_scanner() {
    let fake = fake::FakeEscl::start(2).unwrap();
    let no = AtomicBool::new(false);
    // Flatbed, Letter, 75 dpi gray: one page of 637 × 825 pixels.
    let s = ScanSettings { color: ColorMode::Gray, dpi: 75, source: Source::Flatbed, paper: Paper::Letter };
    let pages = scan(&fake.id(), &s, &no).unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].dpi, 75.0);
    assert_eq!(png_size(&pages[0].bytes), (637, 825));
    let job = fake.requests().into_iter().find(|r| r.method == "POST").unwrap();
    assert!(job.body.contains("<scan:ColorMode>Grayscale8</scan:ColorMode>") && job.body.contains("image/png"), "{}", job.body);
    // Both sides of two sheets: four pages; the feeder's range snaps 100 dpi to 75.
    let s = ScanSettings { color: ColorMode::Color, dpi: 100, source: Source::FeederDuplex, paper: Paper::A5 };
    let pages = scan(&fake.id(), &s, &no).unwrap();
    assert_eq!(pages.len(), 4);
    assert!(pages.iter().all(|p| p.dpi == 75.0));
    // Now the feeder is empty.
    assert_eq!(scan(&fake.id(), &ScanSettings { source: Source::Feeder, ..s }, &no), Err(ScanError::FeederEmpty));
    // Cancelled before starting.
    assert_eq!(scan(&fake.id(), &ScanSettings::default(), &AtomicBool::new(true)), Err(ScanError::Cancelled));
    // Nothing listens there.
    let gone = scan("escl:127.0.0.1:9", &ScanSettings::default(), &no);
    assert!(matches!(gone, Err(ScanError::NotFound(_))), "{gone:?}");
}

// ── SANE ──────────────────────────────────────────────────────────────────────────────────

const SANE_A: &str = "\nAll options specific to device `test':\n  Scan Mode:\n    --mode Gray|Color [Gray]\n        Selects the scan mode.\n    --resolution 1..1200dpi (in steps of 1) [50]\n        Sets the resolution.\n    --source Flatbed|Automatic Document Feeder [Flatbed]\n        If Automatic Document Feeder is selected...\n  Geometry:\n    -l 0..200mm (in steps of 1) [0]\n    -t 0..200mm (in steps of 1) [0]\n    -x 0..200mm (in steps of 1) [80]\n    -y 0..200mm (in steps of 1) [100]\n    --hand-scanner[=(yes|no)] [no]\n";

#[test]
fn sane_listing_and_options_are_read() {
    let list = "device `pixma:04A9176D_3EBCAD' is a CANON Canon PIXMA MG3600 multi-function peripheral\ndevice `airscan:e0:HP OfficeJet' is a eSCL HP OfficeJet ip=192.168.1.5\nnot a device line\ndevice `' is a nothing\n";
    let found = sane::parse_list(list);
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].id, "sane:pixma:04A9176D_3EBCAD");
    assert_eq!(found[0].name, "CANON Canon PIXMA MG3600 multi-function peripheral");
    let o = sane::parse_options(SANE_A);
    assert_eq!(o.modes, vec!["Gray", "Color"]);
    assert_eq!(o.sources, vec!["Flatbed", "Automatic Document Feeder"]);
    assert_eq!(o.range, Some((1, 1200)));
    assert_eq!(o.max_mm, Some((200.0, 200.0)));
    assert_eq!(sane::pick_mode(&o, ColorMode::BlackWhite).as_deref(), Some("Gray"));
    assert_eq!(sane::pick_source(&o, Source::Feeder).unwrap().as_deref(), Some("Automatic Document Feeder"));
    assert_eq!(sane::pick_source(&o, Source::FeederDuplex), Err(ScanError::NoSource("double-sided document feeder")));
    let s = ScanSettings { color: ColorMode::Color, dpi: 300, source: Source::Flatbed, paper: Paper::A4 };
    let (args, dpi) = sane::args("test", &o, &s, None).unwrap();
    assert_eq!(dpi, 300);
    assert!(args.contains(&"--mode=Color".to_string()) && args.contains(&"--source=Flatbed".to_string()));
    // A4 is clipped to the 200 mm bed.
    assert!(args.windows(2).any(|w| w == ["-y", "200"]), "{args:?}");
    let discrete = sane::parse_options(
        "    --resolution 75|150|300|600dpi [75]\n    --mode Lineart|Gray|Color [Color]\n    --source ADF Front|ADF Back|ADF Duplex [ADF Front]\n",
    );
    assert_eq!(sane::pick_dpi(&discrete, 200), 150);
    assert_eq!(sane::pick_mode(&discrete, ColorMode::BlackWhite).as_deref(), Some("Lineart"));
    assert_eq!(sane::pick_source(&discrete, Source::Feeder).unwrap().as_deref(), Some("ADF Front"));
    assert_eq!(sane::pick_source(&discrete, Source::FeederDuplex).unwrap().as_deref(), Some("ADF Duplex"));
    // A device without geometry options gets no area.
    let (args, _) = sane::args("x", &discrete, &s, None).unwrap();
    assert!(!args.contains(&"-x".to_string()));
}

#[test]
fn sane_messages_become_errors() {
    assert_eq!(sane::error_from_stderr("scanimage: sane_start: Document feeder out of documents\n"), ScanError::FeederEmpty);
    assert_eq!(sane::error_from_stderr("scanimage: sane_read: Device busy"), ScanError::Busy);
    assert_eq!(sane::error_from_stderr("scanimage: sane_read: Document feeder jammed"), ScanError::Jammed);
    assert!(matches!(sane::error_from_stderr("scanimage: open of device nonexist failed: Invalid argument"), ScanError::NotFound(_)));
    assert_eq!(sane::error_from_stderr("Scanning page 1\nweird thing\n"), ScanError::Failed("weird thing".into()));
}

/// Real scans through SANE's `test` backend, when `scanimage` is installed.
#[cfg(unix)]
#[test]
fn sane_test_device_scans_flatbed_and_feeder() {
    let program = std::path::Path::new(sane::PROGRAM);
    if std::process::Command::new(program).arg("--version").output().is_err() {
        eprintln!("skipping: scanimage is not installed");
        return;
    }
    let no = AtomicBool::new(false);
    let s = ScanSettings { color: ColorMode::Color, dpi: 50, source: Source::Flatbed, paper: Paper::A5 };
    let pages = sane::scan_with(program, "test", &s, &no).unwrap();
    assert_eq!(pages.len(), 1);
    // 148 × 200 mm (the test bed is 200 mm) at 50 dpi.
    assert_eq!(png_size(&pages[0].bytes), (291, 393));
    // The test feeder holds ten sheets.
    let pages = sane::scan_with(program, "test", &ScanSettings { source: Source::Feeder, ..s }, &no).unwrap();
    assert_eq!(pages.len(), 10);
    assert!(matches!(sane::scan_with(program, "no-such-device", &s, &no), Err(ScanError::NotFound(_))));
    assert_eq!(
        sane::scan_with(std::path::Path::new("/nonexistent/scanimage"), "test", &s, &no),
        Err(ScanError::BackendMissing("SANE (the scanimage program)"))
    );
}
