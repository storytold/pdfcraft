//! The WS-Print tools, end to end against the in-process fake printer (no sockets, no network).
#![cfg(feature = "network-print")]

use std::sync::Arc;

use craft_print::wsd::adapter::{DeviceUrl, Request, Response, Transport, TransportKind, WireError};
use craft_print::wsd::testing::{Ending, FakeConfig as Config, FakePrinter, MemTransport};
use pdfcraft_automation::{Automation, Content, ToolError, tools};
use pdfcraft_print::network::{CancelToken, Document, Job, JobOptions, Printer, UserConfirmed};
use serde_json::{Value, json};

const URL: &str = "http://127.0.0.1:5357/print";

fn json_of(r: Vec<Content>) -> Value {
    r.into_iter()
        .find_map(|c| match c {
            Content::Json(v) => Some(v),
            Content::Png { .. } => None,
        })
        .expect("a JSON result")
}

fn auto(p: &Arc<FakePrinter>) -> Automation {
    Automation::new().with_wsd_transport(Arc::new(MemTransport(p.clone())))
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-wsd-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("temp dir");
    d
}

#[test]
fn the_tools_are_listed_with_the_right_flags() {
    let all = tools();
    let info = all.iter().find(|t| t.name == "wsd_printer_info").expect("wsd_printer_info");
    let print = all.iter().find(|t| t.name == "print_wsd").expect("print_wsd");
    assert!(info.read_only && !print.read_only);
    assert!(print.input_schema["properties"]["user_confirmed"].is_object());
    assert!(print.description.contains("user_confirmed"));
}

#[test]
fn printer_info_reports_description_status_and_capabilities() {
    let p = FakePrinter::new(Config::default());
    let mut a = auto(&p);
    let v = json_of(a.call("wsd_printer_info", &json!({ "printer_url": URL })).expect("info"));
    assert_eq!(v["description"]["name"], "Fake Printer");
    assert_eq!(v["status"]["state"], "Idle");
    assert_eq!(v["capabilities"]["formats"], json!(["application/pdf", "image/pwg-raster"]));
    assert_eq!(v["capabilities"]["copies"]["max"], 10);
    assert_eq!(p.jobs(), 0, "reading the printer creates no job");
}

#[test]
fn nothing_is_sent_without_the_users_confirmation() {
    let p = FakePrinter::new(Config::default());
    let mut a = auto(&p);
    let dir = tmp("noconfirm");
    std::fs::write(dir.join("a.pdf"), b"%PDF-1.4\n%%EOF").unwrap();
    let path = dir.join("a.pdf").to_string_lossy().into_owned();
    for args in [json!({ "printer_url": URL, "path": path }), json!({ "printer_url": URL, "path": path, "user_confirmed": false })] {
        match a.call("print_wsd", &args) {
            Err(ToolError::InvalidArgs(m)) => assert!(m.contains("user_confirmed: true"), "{m}"),
            other => panic!("{other:?}"),
        }
    }
    assert!(p.actions().is_empty(), "not even discovery: {:?}", p.actions());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_numeric_local_addresses_are_accepted() {
    let p = FakePrinter::new(Config::default());
    let mut a = auto(&p);
    for url in ["http://printer.local/", "http://8.8.8.8/", "https://127.0.0.1/", "http://localhost/", "ftp://10.0.0.1/", "not a url"] {
        assert!(matches!(a.call("wsd_printer_info", &json!({ "printer_url": url })), Err(ToolError::InvalidArgs(_))), "{url}");
        assert!(
            matches!(a.call("print_wsd", &json!({ "printer_url": url, "user_confirmed": true, "path": "x.pdf" })), Err(ToolError::InvalidArgs(_))),
            "{url}"
        );
    }
    assert!(p.actions().is_empty());
}

#[test]
fn a_file_is_sent_as_it_is_and_the_result_says_how_it_ended() {
    let p = FakePrinter::new(Config::default());
    let dir = tmp("file");
    let pdf = b"%PDF-1.4\nbinary \xff\x00 bytes\n%%EOF\n";
    std::fs::write(dir.join("a.pdf"), pdf).unwrap();
    let mut a = auto(&p).with_root(&dir).expect("root");
    let v = json_of(
        a.call(
            "print_wsd",
            &json!({ "printer_url": URL, "user_confirmed": true, "path": "a.pdf", "copies": 2, "wait_secs": 10, "job_name": "Hello" }),
        )
        .expect("print"),
    );
    assert_eq!((v["outcome"].as_str(), v["success"].as_bool(), v["format"].as_str()), (Some("completed"), Some(true), Some("application/pdf")));
    assert_eq!(v["printer"], "Fake Printer");
    assert_eq!(p.job(v["job_id"].as_u64().unwrap() as u32).unwrap().document.as_deref(), Some(&pdf[..]));
    assert!(p.job(1).unwrap().ticket.contains("Hello"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_whose_format_the_printer_does_not_list_is_refused_before_a_job_exists() {
    let p = FakePrinter::new(Config::default());
    let dir = tmp("nofmt");
    std::fs::write(dir.join("a.jpg"), [0xff, 0xd8, 0xff]).unwrap();
    let mut a = auto(&p).with_root(&dir).expect("root");
    let e = a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "path": "a.jpg" })).unwrap_err();
    assert!(matches!(&e, ToolError::Failed(m) if m.contains("image/jpeg") && m.contains("application/pdf")), "{e:?}");
    assert_eq!(p.jobs(), 0);
    // An extension we do not know needs the format said.
    std::fs::write(dir.join("a.bin"), [1]).unwrap();
    assert!(matches!(a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "path": "a.bin" })), Err(ToolError::InvalidArgs(_))));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn paths_stay_inside_the_root() {
    let p = FakePrinter::new(Config::default());
    let base = tmp("root");
    let root = base.join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(base.join("outside.pdf"), b"%PDF-1.4\n%%EOF").unwrap();
    let mut a = auto(&p).with_root(&root).expect("root");
    let absolute = base.join("outside.pdf").to_string_lossy().into_owned();
    for path in ["../outside.pdf", absolute.as_str()] {
        assert!(a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "path": path })).is_err(), "{path}");
    }
    assert_eq!(p.jobs(), 0);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn exactly_one_source_is_required() {
    let p = FakePrinter::new(Config::default());
    let mut a = auto(&p);
    let neither = a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true })).unwrap_err();
    assert!(matches!(neither, ToolError::InvalidArgs(m) if m.contains("exactly one")));
    let both = a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "path": "a.pdf", "doc": 1 })).unwrap_err();
    assert!(matches!(both, ToolError::InvalidArgs(_)));
}

fn blank_doc(a: &mut Automation) -> u64 {
    json_of(a.call("doc_create", &json!({ "from": "blank", "pages": 2 })).expect("create"))["doc"].as_u64().expect("doc id")
}

#[test]
fn an_open_document_goes_as_a_print_ready_pdf() {
    let p = FakePrinter::new(Config::default());
    let mut a = auto(&p);
    let doc = blank_doc(&mut a);
    let v = json_of(
        a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "doc": doc, "paper": "iso_a4_210x297mm", "wait_secs": 10 }))
            .expect("print"),
    );
    assert_eq!(v["outcome"], "completed");
    assert_eq!(v["format"], "application/pdf");
    let sent = p.job(1).unwrap().document.unwrap();
    assert!(sent.starts_with(b"%PDF-"), "{:?}", &sent[..8.min(sent.len())]);
}

#[test]
fn an_open_document_is_rendered_to_pwg_raster_for_a_printer_without_pdf() {
    let p = FakePrinter::new(Config { formats: vec!["image/pwg-raster"], ..Default::default() });
    let mut a = auto(&p);
    let doc = blank_doc(&mut a);
    let v = json_of(
        a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "doc": doc, "dpi": 50, "color": "mono", "wait_secs": 10 }))
            .expect("print"),
    );
    assert_eq!((v["outcome"].as_str(), v["format"].as_str()), (Some("completed"), Some("image/pwg-raster")));
    let sent = p.job(1).unwrap().document.unwrap();
    assert_eq!(&sent[..4], b"RaS2");
    // Two pages, 8 bits per pixel: header(1796) per page.
    let pages = sent.windows(9).filter(|w| *w == b"PwgRaster").count();
    assert_eq!(pages, 2);
}

#[test]
fn bad_option_values_are_argument_errors() {
    let p = FakePrinter::new(Config::default());
    let mut a = auto(&p);
    for extra in [
        json!({ "copies": 0 }),
        json!({ "copies": 100000 }),
        json!({ "duplex": "sideways" }),
        json!({ "orientation": "tilted" }),
        json!({ "quality": "ultra" }),
        json!({ "dpi": 5 }),
        json!({ "color": "blue" }),
        json!({ "transfer": "carrier-pigeon" }),
        json!({ "wait_secs": 99999 }),
    ] {
        let mut args = json!({ "printer_url": URL, "user_confirmed": true, "path": "x.pdf" });
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        assert!(matches!(a.call("print_wsd", &args), Err(ToolError::InvalidArgs(_))), "{args}");
    }
    assert!(p.actions().is_empty());
}

#[test]
fn a_job_still_running_when_the_wait_ends_is_accepted_and_left_alone() {
    let p = FakePrinter::new(Config { ending: Ending::Hangs, ..Default::default() });
    let dir = tmp("hangs");
    std::fs::write(dir.join("a.pdf"), b"%PDF-1.4\n%%EOF").unwrap();
    let mut a = auto(&p).with_root(&dir).expect("root");
    let v = json_of(a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "path": "a.pdf", "wait_secs": 1 })).expect("print"));
    assert_eq!((v["accepted"].as_bool(), v["outcome"].as_str(), v["success"].as_bool()), (Some(true), Some("still_running"), Some(false)), "{v}");
    assert!(v["job"].as_str().unwrap().starts_with("wsd:http://127.0.0.1:5357/print#"), "{v}");
    // Accepted once, never sent again, and not cancelled.
    assert_eq!(p.actions().iter().filter(|x| *x == "SendDocument").count(), 1);
    assert!(!p.actions().iter().any(|x| x == "CancelJob"), "{:?}", p.actions());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Delivers every request to the fake and loses the reply to `SendDocument`: the printer did the
/// work and the client saw the connection close with no answer.
struct LoseSendReply(Arc<FakePrinter>);

impl Transport for LoseSendReply {
    fn post(&self, _: &DeviceUrl, request: Request) -> Result<Response, WireError> {
        let (status, content_type, body) = self.0.handle(&request.content_type, &request.body);
        if request.soap_action.ends_with("SendDocument") {
            return Err(WireError::transport(TransportKind::ConnectionClosed, "closed without a reply"));
        }
        Ok(Response::new(status, Some(content_type), body))
    }
}

#[test]
fn a_send_whose_outcome_is_unknown_says_to_check_the_printer_and_is_never_repeated() {
    let p = FakePrinter::new(Config::default());
    let dir = tmp("lost");
    std::fs::write(dir.join("a.pdf"), b"%PDF-1.4\n%%EOF").unwrap();
    let mut a = Automation::new().with_wsd_transport(Arc::new(LoseSendReply(p.clone()))).with_root(&dir).expect("root");
    let e = a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "path": "a.pdf", "wait_secs": 5 })).unwrap_err();
    assert!(matches!(&e, ToolError::Failed(m) if m.contains("check the printer before sending again")), "{e:?}");
    let sends = p.actions().iter().filter(|x| *x == "SendDocument").count();
    assert_eq!(sends, 1, "no automatic second send: {:?}", p.actions());
    assert!(!p.actions().iter().any(|x| x == "CancelJob"), "a job that may be printing is left alone: {:?}", p.actions());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn per_job_settings_reach_the_ticket_through_the_registry() {
    let p = FakePrinter::new(Config::default());
    let dir = tmp("options");
    std::fs::write(dir.join("a.pdf"), b"%PDF-1.4\n%%EOF").unwrap();
    let mut a = auto(&p).with_root(&dir).expect("root");
    let v = json_of(
        a.call(
            "print_wsd",
            &json!({
                "printer_url": URL, "user_confirmed": true, "path": "a.pdf", "wait_secs": 10,
                "orientation": "landscape", "paper": "iso_a4_210x297mm", "quality": "high", "dpi": 600, "duplex": "long-edge", "copies": 3
            }),
        )
        .expect("print"),
    );
    assert_eq!(v["outcome"], "completed", "{v}");
    let ticket = p.job(1).unwrap().ticket;
    for want in ["Landscape", "iso_a4_210x297mm", "High", "600", "TwoSidedLongEdge", "3"] {
        assert!(ticket.contains(want), "{want} missing from {ticket}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn photo_quality_reaches_the_ticket() {
    let p = FakePrinter::new(Config { qualities: vec!["Normal", "Photo"], ..Default::default() });
    let dir = tmp("photo");
    std::fs::write(dir.join("a.pdf"), b"%PDF-1.4\n%%EOF").unwrap();
    let mut a = auto(&p).with_root(&dir).expect("root");
    let v = json_of(
        a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "path": "a.pdf", "quality": "photo", "wait_secs": 10 }))
            .expect("print"),
    );
    assert_eq!(v["outcome"], "completed", "{v}");
    assert!(v["warnings"].as_array().unwrap().is_empty(), "{v}");
    let ticket = p.job(1).unwrap().ticket;
    assert!(ticket.contains("Photo"), "Photo missing from {ticket}");
    // The tool schema offers it too.
    let print = tools().into_iter().find(|t| t.name == "print_wsd").unwrap();
    assert!(print.input_schema["properties"]["quality"]["enum"].as_array().unwrap().contains(&json!("photo")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_photo_the_printer_does_not_list_is_left_out_with_a_warning() {
    let p = FakePrinter::new(Config { qualities: vec!["Normal"], ..Default::default() });
    let dir = tmp("nophoto");
    std::fs::write(dir.join("a.pdf"), b"%PDF-1.4\n%%EOF").unwrap();
    let mut a = auto(&p).with_root(&dir).expect("root");
    let v = json_of(
        a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "path": "a.pdf", "quality": "photo", "wait_secs": 10 }))
            .expect("print"),
    );
    assert_eq!(v["outcome"], "completed", "{v}");
    assert!(!v["warnings"].as_array().unwrap().is_empty(), "the dropped setting is reported: {v}");
    assert!(!p.job(1).unwrap().ticket.contains("Photo"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn images_text_and_xps_are_sent_as_they_are_when_the_printer_lists_them() {
    let p = FakePrinter::new(Config {
        formats: vec!["application/pdf", "image/jpeg", "image/png", "image/tiff", "text/plain", "application/vnd.ms-xpsdocument", "application/oxps"],
        ..Default::default()
    });
    let dir = tmp("asis");
    let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0, 16, b'J', b'F', b'I', b'F', 0];
    let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0];
    let tiff = [b'I', b'I', 42, 0, 8, 0, 0, 0];
    let text = b"hello, printer\n";
    let xps = b"PK\x03\x04 an xps package";
    // XPS is a ZIP and is never sniffed: the format is said.
    let cases: [(&str, &[u8], Option<&str>, &str); 6] = [
        ("a.jpg", &jpeg, None, "image/jpeg"),
        ("b.png", &png, None, "image/png"),
        ("c.tif", &tiff, None, "image/tiff"),
        ("d.txt", text, None, "text/plain"),
        ("e.xps", xps, Some("application/vnd.ms-xpsdocument"), "application/vnd.ms-xpsdocument"),
        ("f.oxps", xps, Some("application/oxps"), "application/oxps"),
    ];
    let mut a = auto(&p).with_root(&dir).expect("root");
    for (n, (name, bytes, format, mime)) in cases.into_iter().enumerate() {
        std::fs::write(dir.join(name), bytes).unwrap();
        let mut args = json!({ "printer_url": URL, "user_confirmed": true, "path": name, "wait_secs": 10 });
        if let Some(f) = format {
            args["format"] = json!(f);
        }
        let v = json_of(a.call("print_wsd", &args).unwrap_or_else(|e| panic!("{name}: {e:?}")));
        assert_eq!((v["outcome"].as_str(), v["format"].as_str()), (Some("completed"), Some(mime)), "{name}: {v}");
        assert!(v["converted_from"].is_null(), "{name}: sent unchanged, not converted: {v}");
        let sent = p.job(n as u32 + 1).unwrap();
        assert_eq!(sent.document.as_deref(), Some(bytes), "{name}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_format_the_printer_does_not_list_is_still_refused_with_what_it_lists() {
    let p = FakePrinter::new(Config { formats: vec!["application/pdf", "image/jpeg"], ..Default::default() });
    let dir = tmp("png");
    std::fs::write(dir.join("a.png"), [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]).unwrap();
    let mut a = auto(&p).with_root(&dir).expect("root");
    let e = a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "path": "a.png" })).unwrap_err();
    assert!(matches!(&e, ToolError::Failed(m) if m.contains("image/png") && m.contains("image/jpeg")), "{e:?}");
    assert_eq!(p.jobs(), 0);
    // A format craft-print has no name for needs saying, and a wrong name is an argument error.
    std::fs::write(dir.join("a.gif"), b"GIF89a\x01\x00\x01\x00\x80\x00\x00").unwrap();
    assert!(matches!(a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "path": "a.gif" })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(
        a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "path": "a.png", "format": "image/gif" })),
        Err(ToolError::InvalidArgs(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The first page's `HWResolution` of a PWG raster stream (big-endian, at byte 276 of the header).
fn pwg_dpi(raster: &[u8]) -> (u32, u32) {
    let at = |o: usize| u32::from_be_bytes(raster[4 + o..4 + o + 4].try_into().unwrap());
    (at(276), at(280))
}

#[test]
fn a_pdf_is_converted_to_raster_at_the_resolution_the_printer_lists() {
    let p = FakePrinter::new(Config { formats: vec!["image/pwg-raster"], resolutions: vec![(150, 150), (600, 600)], ..Default::default() });
    let mut a = auto(&p);
    let doc = blank_doc(&mut a);
    let v = json_of(a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "doc": doc, "wait_secs": 10 })).expect("print"));
    assert_eq!(
        (v["outcome"].as_str(), v["format"].as_str(), v["converted_from"].as_str()),
        (Some("completed"), Some("image/pwg-raster"), Some("application/pdf")),
        "{v}"
    );
    assert_eq!(v["resolution"], json!([150, 150]), "the listed resolution nearest 300 dpi: {v}");
    let sent = p.job(1).unwrap();
    assert_eq!(pwg_dpi(sent.document.as_ref().unwrap()), (150, 150));
    assert!(sent.ticket.contains("150"), "the ticket agrees with the raster: {}", sent.ticket);
    // The caller's own dpi wins over the list.
    let v = json_of(
        a.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "doc": doc, "dpi": 72, "color": "mono", "wait_secs": 10 }))
            .expect("print"),
    );
    assert_eq!(v["resolution"], json!([72, 72]), "{v}");
    assert_eq!(pwg_dpi(p.job(2).unwrap().document.as_ref().unwrap()), (72, 72));
    // A printer that lists no resolution gets 300 dpi.
    let q = FakePrinter::new(Config { formats: vec!["image/pwg-raster"], ..Default::default() });
    let mut b = auto(&q);
    let doc = blank_doc(&mut b);
    let v = json_of(b.call("print_wsd", &json!({ "printer_url": URL, "user_confirmed": true, "doc": doc, "wait_secs": 10 })).expect("print"));
    assert_eq!(v["resolution"], json!([300, 300]), "{v}");
}

fn reads(p: &FakePrinter) -> usize {
    p.actions().iter().filter(|x| *x == "GetPrinterElements").count()
}

/// The bytes of a one-page blank document, made by the tool set.
fn blank_pdf(dir: &std::path::Path) -> Vec<u8> {
    let mut a = Automation::new().with_root(dir).expect("root");
    let doc = blank_doc(&mut a);
    a.call("doc_save", &json!({ "doc": doc, "path": "blank.pdf" })).expect("save");
    std::fs::read(dir.join("blank.pdf")).expect("saved")
}

fn print_pdf(printer: &Printer, pdf: &[u8]) -> pdfcraft_print::network::Sent {
    let consent = UserConfirmed::attest(Some(true)).unwrap();
    printer.submit(Document::pdf(pdf), Job::default(), JobOptions::new(), consent, &CancelToken::new(), &mut |_| {}).expect("print")
}

#[test]
fn a_converted_print_with_the_printers_details_in_hand_reads_the_printer_once() {
    let p = FakePrinter::new(Config { formats: vec!["image/pwg-raster"], ..Default::default() });
    let dir = tmp("reads");
    let pdf = blank_pdf(&dir);
    let printer = Printer::open(URL, Some(Arc::new(MemTransport(p.clone())))).expect("open");
    printer.info().expect("the dialog's query");
    assert_eq!(reads(&p), 1);
    let sent = print_pdf(&printer, &pdf);
    assert_eq!(sent.conversion.as_ref().map(|c| c.to.mime()), Some("image/pwg-raster"));
    assert_eq!(reads(&p) - 1, 1, "one read during the print (the backend's own, for the ticket); the registry's was skipped: {:?}", p.actions());
    // Without details in hand the print reads the printer first, and nothing more.
    let q = FakePrinter::new(Config { formats: vec!["image/pwg-raster"], ..Default::default() });
    let printer = Printer::open(URL, Some(Arc::new(MemTransport(q.clone())))).expect("open");
    print_pdf(&printer, &pdf);
    assert_eq!(reads(&q), 2, "{:?}", q.actions());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_format_the_printer_lists_is_not_converted() {
    let p = FakePrinter::new(Config::default());
    let dir = tmp("noconvert");
    let pdf = blank_pdf(&dir);
    let printer = Printer::open(URL, Some(Arc::new(MemTransport(p.clone())))).expect("open");
    let sent = print_pdf(&printer, &pdf);
    assert!(sent.conversion.is_none());
    assert_eq!(p.job(1).unwrap().document.as_deref(), Some(&pdf[..]));
    let _ = std::fs::remove_dir_all(&dir);
}
