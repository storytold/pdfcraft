#![cfg(not(target_arch = "wasm32"))]
use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn call(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        _ => panic!("expected JSON"),
    }
}

#[test]
fn hot_folder_optimization_never_revives_stale_page_content() {
    use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_incremental};
    use std::sync::Arc;

    let root = tempfile::tempdir().unwrap();
    for folder in ["in", "out"] {
        std::fs::create_dir(root.path().join(folder)).unwrap();
    }
    let source = pdfcraft_engine::Session::new().create_blank(100.0, 100.0, 1).unwrap();
    let mut document = Document::open(source).unwrap();
    let stale = document.add(Object::Stream(Stream::from_raw(Dict::new(), Vec::new())));
    let current = ObjRef::new(stale.num, stale.generation + 1);
    let catalog = document.root().unwrap();
    let pages = document.get(document.get(catalog).as_dict().unwrap().reference(b"Pages").unwrap());
    let page = pages.as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap()[0].as_ref().unwrap();
    document.update_dict(page, |page| page.set(b"Contents".to_vec(), Object::Ref(stale))).unwrap();
    document.free(stale);
    let replacement = b"0 0 0 rg 0 0 100 100 re f";
    document.set(current, Object::Stream(Stream::from_raw(Dict::new(), replacement.to_vec())));
    let padding = document.add(Object::Stream(Stream::from_raw(Dict::new(), vec![b'x'; 65_536])));
    document
        .update_dict(catalog, |catalog| {
            catalog.set(b"LiveExtension".to_vec(), Object::Ref(current));
            catalog.set(b"Padding".to_vec(), Object::Ref(padding));
        })
        .unwrap();
    let source = write_incremental(&document, &SaveOptions::default()).unwrap();
    std::fs::write(root.path().join("in/source.pdf"), &source).unwrap();
    let mut a = Automation::new().with_root(root.path()).unwrap();
    let steps: Vec<_> = ["off", "lossless", "reduce", "custom"]
        .into_iter()
        .map(|mode| json!({"format":"pdf","name_template":mode,"optimization":{"mode":mode}}))
        .collect();
    let task = json!({
        "id":"generations", "name":"Generations", "input":"in", "output":"out", "enabled":true,
        "interval_seconds":1, "settle_seconds":1,
        "settings":{"recognize":false,"auto_rotate":false,"deskew":false},
        "output_plan":{"steps":steps}
    });
    call(&mut a, "hot_folder_configure", json!({"config":"tasks.json","settings":{"tasks":[task]}}));
    call(&mut a, "hot_folder_poll", json!({"config":"tasks.json"}));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let state = call(&mut a, "hot_folder_poll", json!({"config":"tasks.json"}));
    assert_eq!(state["tasks"]["generations"]["completed"], 1, "{state}");
    assert_eq!(state["tasks"]["generations"]["last_optimization"]["changed"], 3);
    for mode in ["lossless", "reduce", "custom"] {
        let bytes = std::fs::read(root.path().join(format!("out/{mode}.pdf"))).unwrap();
        assert!(bytes.len() < source.len(), "{mode} must exercise the published optimized candidate");
        let saved = Document::open(Arc::new(bytes)).unwrap();
        let catalog = saved.get(saved.root().unwrap());
        let catalog = catalog.as_dict().unwrap();
        let pages = saved.get(catalog.reference(b"Pages").unwrap());
        let page = saved.resolve(&pages.as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap()[0]);
        assert_eq!(*saved.resolve(page.as_dict().unwrap().get(b"Contents").unwrap()), Object::Null, "{mode}");
        let live = saved.resolve(catalog.get(b"LiveExtension").unwrap());
        let Object::Stream(live) = &*live else { panic!("live extension must remain a stream") };
        assert_eq!(live.decoded().unwrap().as_slice(), replacement);
    }
    assert_eq!(std::fs::read(root.path().join("out/off.pdf")).unwrap(), source);
    assert_eq!(std::fs::read(root.path().join("in/source.pdf")).unwrap(), source);
}

#[test]
fn hot_folder_pdf_optimization_is_optional_reports_real_sizes_and_preserves_sources() {
    use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};
    let root = tempfile::tempdir().unwrap();
    for folder in ["in", "out"] {
        std::fs::create_dir(root.path().join(folder)).unwrap();
    }
    let source = pdfcraft_engine::Session::new().create_from_text("source", "Searchable text after automatic PDF optimization").unwrap();
    let mut document = Document::open(source).unwrap();
    let extension = document.add(Object::Stream(Stream::from_raw(Dict::new(), vec![b'x'; 65_536])));
    document.update_dict(document.root().unwrap(), |catalog| catalog.set(b"PdfCraftTestExtension".to_vec(), Object::Ref(extension))).unwrap();
    let source = write_full(&document, &SaveOptions::default()).unwrap();
    std::fs::write(root.path().join("in/source.pdf"), &source).unwrap();
    let mut a = Automation::new().with_root(root.path()).unwrap();
    let task = json!({
        "id":"optimize", "name":"Optimize", "input":"in", "output":"out", "enabled":true,
        "interval_seconds":1, "settle_seconds":1,
        "settings":{"recognize":false,"auto_rotate":false,"deskew":false},
        "output_plan":{"steps":[
            {"format":"pdf","name_template":"unchanged"},
            {"format":"pdf","name_template":"lossless","optimization":{"mode":"lossless"}},
            {"format":"pdf","name_template":"reduce","optimization":{"mode":"reduce"}},
            {"format":"pdf","name_template":"custom","optimization":{"mode":"custom", "color":{"downsample":false,"compression":"retain"}, "gray":{"downsample":false,"compression":"zip"}}}
        ]}
    });
    let mut invalid = task.clone();
    invalid["output_plan"]["steps"][1]["format"] = json!("txt");
    assert!(a.call("hot_folder_configure", &json!({"config":"invalid.json","settings":{"tasks":[invalid]}})).is_err());
    assert!(!root.path().join("invalid.json").exists());
    call(&mut a, "hot_folder_configure", json!({"config":"tasks.json","settings":{"tasks":[task]}}));
    let config = call(&mut a, "hot_folder_status", json!({"config":"tasks.json"}));
    assert_eq!(config["settings"]["tasks"][0]["output_plan"]["steps"][0]["optimization"]["mode"], "off");
    assert_eq!(config["settings"]["tasks"][0]["originals"], "keep");
    call(&mut a, "hot_folder_poll", json!({"config":"tasks.json"}));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let state = call(&mut a, "hot_folder_poll", json!({"config":"tasks.json"}));
    assert_eq!(state["tasks"]["optimize"]["completed"], 1, "{state}");
    let report = &state["tasks"]["optimize"]["last_optimization"];
    assert_eq!(report["documents"], 3);
    assert_eq!(report["changed"], 3);
    assert_eq!(report["bytes_before"].as_u64().unwrap(), source.len() as u64 * 3);
    let mut after = 0;
    for name in ["lossless", "reduce", "custom"] {
        let path = format!("out/{name}.pdf");
        let bytes = std::fs::read(root.path().join(&path)).unwrap();
        assert!(bytes.len() < source.len(), "{name}");
        after += bytes.len() as u64;
        let doc = call(&mut a, "doc_open", json!({"path":path}))["doc"].clone();
        assert!(call(&mut a, "text_extract", json!({"doc":doc})).to_string().contains("Searchable text"));
    }
    assert_eq!(report["bytes_after"].as_u64().unwrap(), after);
    assert_eq!(std::fs::read(root.path().join("out/unchanged.pdf")).unwrap(), source);
    assert_eq!(std::fs::read(root.path().join("in/source.pdf")).unwrap(), source);
    call(&mut a, "hot_folder_retry", json!({"config":"tasks.json","task":"optimize"}));
    call(&mut a, "hot_folder_poll", json!({"config":"tasks.json"}));
    assert_eq!(std::fs::read_dir(root.path().join("out")).unwrap().count(), 4);
    assert_eq!(std::fs::read(root.path().join("in/source.pdf")).unwrap(), source);
}

#[test]
fn hot_folder_analysis_only_preserves_pages_and_skips_processing_options() {
    let root = tempfile::tempdir().unwrap();
    for folder in ["in", "out"] {
        std::fs::create_dir(root.path().join(folder)).unwrap();
    }
    let session = pdfcraft_engine::Session::new();
    let text = session.create_from_text("source", "Analyze existing words without changing them").unwrap();
    let blank = session.create_blank(100.0, 100.0, 1).unwrap();
    let source = session.combine(&[("text.pdf".into(), text), ("blank.pdf".into(), blank)]).unwrap();
    std::fs::write(root.path().join("in/source.pdf"), source.as_slice()).unwrap();
    let mut a = Automation::new().with_root(root.path()).unwrap();
    let task = json!({"id":"analysis","name":"Analysis","input":"in","output":"out","analysis_only":true,"remove_blank_pages":true,"enabled":true,"interval_seconds":1,"settle_seconds":1,"preprocessing":{"invert":true,"split_spreads":true},"output_plan":{"steps":[{"format":"pdf"}]}});
    call(&mut a, "hot_folder_configure", json!({"config":"tasks.json","settings":{"tasks":[task]}}));
    call(&mut a, "hot_folder_poll", json!({"config":"tasks.json"}));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let state = call(&mut a, "hot_folder_poll", json!({"config":"tasks.json"}));
    assert_eq!(state["tasks"]["analysis"]["completed"], 1, "{state}");
    assert_eq!(std::fs::read_dir(root.path().join("out")).unwrap().count(), 1);
    let report: Value = serde_json::from_slice(&std::fs::read(root.path().join("out/source.json")).unwrap()).unwrap();
    assert_eq!(report["format"], "pdfcraft-analysis");
    assert_eq!(report["ocr_performed"], false);
    assert_eq!(report["pages"].as_array().unwrap().len(), 2);
    assert_eq!(report["pages"][0]["content"], "existing_text");
    assert_eq!(report["pages"][1]["content"], "empty_content");
    assert_eq!(std::fs::read(root.path().join("in/source.pdf")).unwrap(), *source);
    let config = call(&mut a, "hot_folder_status", json!({"config":"tasks.json"}));
    assert_eq!(config["settings"]["tasks"][0]["settings"]["recognize"], true);
    assert_eq!(config["settings"]["tasks"][0]["output_plan"]["steps"][0]["format"], "pdf");
}

#[test]
fn hot_folder_project_sources_preserve_order_and_ack_only_after_all_outputs() {
    let root = std::env::temp_dir().join(format!("pdfcraft-hot-folder-project-tools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for folder in ["sources", "stage", "out"] {
        std::fs::create_dir_all(root.join(folder)).unwrap();
    }
    let session = pdfcraft_engine::Session::new();
    for (name, text) in [("z.pdf", "Zebra comes first"), ("a.pdf", "Apple comes second")] {
        std::fs::write(root.join("sources").join(name), session.create_from_text("source", text).unwrap().as_slice()).unwrap();
    }
    let mut a = Automation::new().with_root(&root).unwrap();
    let project = call(&mut a, "hot_folder_project", json!({"path":"sources/project.json","files":["sources/z.pdf","sources/a.pdf"]}));
    assert_eq!(project["project"]["files"], json!(["z.pdf", "a.pdf"]));
    assert!(a.call("hot_folder_project", &json!({"path":"../escaped.json","files":["sources/z.pdf"]})).is_err());
    assert!(a.call("hot_folder_project", &json!({"path":"sources/project.json","files":["sources/z.pdf"]})).is_err());
    let task = json!({"id":"project","name":"Project","input":"stage","output":"out","enabled":true,"interval_seconds":1,"settle_seconds":1,"settings":{"language":"de","recognize":false,"auto_rotate":false,"deskew":false},"source":{"kind":"project","manifest":"sources/project.json","action":{"kind":"delete"}},"output_plan":{"steps":[{"format":"pdf","grouping":"all","name_template":"combined"},{"format":"txt","grouping":"all","name_template":"combined"},{"format":"xlsx","grouping":"all","name_template":"combined"},{"format":"epub","grouping":"all","name_template":"combined"}]}});
    call(&mut a, "hot_folder_configure", json!({"config":"tasks.json","settings":{"tasks":[task]}}));
    let mut overlapping = call(&mut a, "hot_folder_status", json!({"config":"tasks.json"}))["settings"].clone();
    overlapping["tasks"].as_array_mut().unwrap().push(json!({"id":"other","name":"Other","input":"sources","output":"out","enabled":true}));
    assert!(a.call("hot_folder_configure", &json!({"config":"overlap.json","settings":overlapping})).unwrap_err().to_string().contains("overlap"));
    call(&mut a, "hot_folder_poll", json!({"config":"tasks.json"}));
    assert!(root.join("sources/z.pdf").exists());
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let state = call(&mut a, "hot_folder_poll", json!({"config":"tasks.json"}));
    assert_eq!(state["tasks"]["project"]["completed"], 2, "{state}");
    assert!(!root.join("sources/z.pdf").exists());
    assert!(!root.join("sources/a.pdf").exists());
    let text = std::fs::read_to_string(root.join("out/combined.txt")).unwrap();
    assert!(text.find("Zebra").unwrap() < text.find("Apple").unwrap(), "{text}");
    call(&mut a, "hot_folder_retry", json!({"config":"tasks.json","task":"project"}));
    call(&mut a, "hot_folder_poll", json!({"config":"tasks.json"}));
    assert_eq!(std::fs::read_dir(root.join("out")).unwrap().count(), 4);
    for extension in ["xlsx", "epub"] {
        assert!(std::fs::read(root.join(format!("out/combined.{extension}"))).unwrap().starts_with(b"PK\x03\x04"));
    }
    let error = a.call("hot_folder_source", &json!({"config":"tasks.json","task":"project","action":"credential_status"})).unwrap_err();
    assert!(error.to_string().contains("no authenticated"));
}

#[test]
fn hot_folder_tools_configure_poll_stop_and_confine_all_nested_paths() {
    let root = std::env::temp_dir().join(format!("pdfcraft-hot-folder-tools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("in")).unwrap();
    std::fs::create_dir_all(root.join("out")).unwrap();
    let mut a = Automation::new().with_root(&root).unwrap();
    let task =
        json!({"id":"incoming", "name":"Incoming scans", "input":"in", "output":"out", "enabled":true, "settle_seconds":1, "interval_seconds":1});
    call(&mut a, "hot_folder_configure", json!({"config":"tasks.json","settings":{"tasks":[task.clone()]}}));
    let state = call(&mut a, "hot_folder_poll", json!({"config":"tasks.json"}));
    assert_eq!(state["tasks"]["incoming"]["completed"], 0);
    let status = call(&mut a, "hot_folder_status", json!({"config":"tasks.json"}));
    assert_eq!(status["settings"]["tasks"][0]["enabled"], true);
    let mut stopped = task.clone();
    stopped["enabled"] = json!(false);
    call(&mut a, "hot_folder_configure", json!({"config":"tasks.json","settings":{"tasks":[stopped]}}));
    call(&mut a, "hot_folder_retry", json!({"config":"tasks.json","task":"incoming"}));
    assert!(call(&mut a, "hot_folder_status", json!({"config":"tasks.json"}))["state"]["tasks"].as_object().unwrap().is_empty());
    for field in ["input", "output"] {
        let mut escaped = task.clone();
        escaped[field] = json!("../outside");
        assert!(a.call("hot_folder_configure", &json!({"config":"bad.json","settings":{"tasks":[escaped]}})).is_err());
    }
    assert!(!root.join("bad.json").exists());
}

#[test]
fn hot_folder_tools_produce_searchable_output_and_preserve_source() {
    if !pdfcraft_engine::ocr::available() {
        return;
    }
    let root = std::env::temp_dir().join(format!("pdfcraft-hot-folder-ocr-tools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("in")).unwrap();
    std::fs::create_dir_all(root.join("out")).unwrap();
    let mut a = Automation::new().with_root(&root).unwrap();
    let source = pdfcraft_engine::Session::new().create_from_text("source", "Hot folder searchable output").unwrap();
    std::fs::write(root.join("in/source.pdf"), source.as_slice()).unwrap();
    call(
        &mut a,
        "hot_folder_configure",
        json!({"config":"tasks.json","settings":{"tasks":[{"id":"test","name":"Test","input":"in","output":"out","enabled":true,"interval_seconds":1,"settle_seconds":1}]}}),
    );
    call(&mut a, "hot_folder_poll", json!({"config":"tasks.json"}));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let result = call(&mut a, "hot_folder_poll", json!({"config":"tasks.json"}));
    assert_eq!(result["tasks"]["test"]["completed"], 1, "{result}");
    assert_eq!(std::fs::read(root.join("in/source.pdf")).unwrap(), *source);
    let doc = call(&mut a, "doc_open", json!({"path":"out/source.pdf"}))["doc"].clone();
    assert!(call(&mut a, "text_extract", json!({"doc":doc})).to_string().contains("searchable"));
}

#[test]
fn hot_folder_tools_apply_page_templates_blank_archives_and_multiple_outputs() {
    let root = std::env::temp_dir().join(format!("pdfcraft-hot-folder-pipeline-tools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for folder in ["in", "out", "blank"] {
        std::fs::create_dir_all(root.join(folder)).unwrap();
    }
    let session = pdfcraft_engine::Session::new();
    let text = session.create_from_text("source", "Pipeline output remains searchable").unwrap();
    let blank = session.create_blank(100.0, 100.0, 1).unwrap();
    let source = session.combine(&[("text.pdf".into(), text), ("blank.pdf".into(), blank)]).unwrap();
    std::fs::write(root.join("in/source.pdf"), source.as_slice()).unwrap();
    let mut a = Automation::new().with_root(&root).unwrap();
    let task = json!({"id":"pipeline","name":"Pipeline","input":"in","output":"out","blank_archive":"blank","remove_blank_pages":true,"enabled":true,"interval_seconds":1,"settle_seconds":1,"settings":{"recognize":false,"auto_rotate":false,"deskew":false},"preprocessing":{"pages":"1-2"},"output_plan":{"steps":[{"format":"pdf"},{"format":"txt"}]}});
    let mut escaped = task.clone();
    escaped["output_plan"]["steps"][1]["folder"] = json!("../outside");
    assert!(a.call("hot_folder_configure", &json!({"config":"bad.json","settings":{"tasks":[escaped]}})).is_err());
    call(&mut a, "hot_folder_configure", json!({"config":"tasks.json","settings":{"tasks":[task]}}));
    call(&mut a, "hot_folder_poll", json!({"config":"tasks.json"}));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let result = call(&mut a, "hot_folder_poll", json!({"config":"tasks.json"}));
    assert_eq!(result["tasks"]["pipeline"]["completed"], 1, "{result}");
    assert!(root.join("out/source.pdf").exists());
    assert!(root.join("out/source.txt").exists());
    assert!(root.join("blank/source_blank_pages.pdf").exists());
    assert!(std::fs::read_to_string(root.join("out/source.txt")).unwrap().contains("Pipeline"));
    assert_eq!(std::fs::read(root.join("in/source.pdf")).unwrap(), *source);
}

#[test]
fn hot_folder_refuses_split_roots_before_writing_configuration() {
    use pdfcraft_automation::file_access::AutomationRoots;
    let root = tempfile::tempdir().unwrap();
    for folder in ["in", "out"] {
        std::fs::create_dir(root.path().join(folder)).unwrap();
    }
    let config = root.path().join("tasks.json");
    let args = json!({"config":config,"settings":{"tasks":[{
        "id":"incoming","name":"Incoming","input":root.path().join("in"),
        "output":root.path().join("out"),"enabled":false
    }]}});
    // The arguments are valid in the supported single-root mode.
    let mut single = Automation::new().with_root(root.path()).unwrap();
    call(&mut single, "hot_folder_configure", args.clone());
    std::fs::remove_file(&config).unwrap();
    for (read, write) in [(true, true), (true, false), (false, true)] {
        let roots = AutomationRoots::new(read.then_some(root.path()), write.then_some(root.path())).unwrap();
        let mut a = Automation::new().with_automation_roots(roots);
        let error = a.call("hot_folder_configure", &args).unwrap_err();
        assert!(error.to_string().contains("does not support separate automation read/write roots"), "{error}");
        assert!(!config.exists());
    }
}
