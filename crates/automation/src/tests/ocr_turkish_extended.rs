//! Independent profile acceptance. Real OCR is explicit, ignored, and fails closed.
//! Frozen holdout rasters/references stay outside the repository and model selection.
use super::*;
use sha2::{Digest, Sha256};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const HOLDOUT_SHA256: &str = "4ce950401a4d67bd6e24da3430faf4f1d67f7df3989dcefd3143f273c509cebe";
const HOLDOUT_V2_SHA256: &str = "25e532575bc6ce112af18a4d7c912a9da86ea5a8739d91b8c62bed1ac0e41797";
const HOLDOUT_V3_SHA256: &str = "9eaf900efad7df4b6c5db7f86a7c9e568a0dc34591c1e80692c063fce6ff7225";
const NFD_SENTINEL: &str = "C\u{327}ag\u{306}rı I\u{307}stanbul i\u{302} I\u{302} u\u{302} U\u{302}";

fn settings(payload: &Value) -> pdfcraft_engine::ocr::OcrSettings {
    Automation::new().ocr_settings(&crate::json::Args(payload)).unwrap()
}

#[test]
fn legacy_payload_and_explicit_standard_keep_the_standard_profile() {
    for payload in [json!({}), json!({"language":"tr"}), json!({"language":"tr","engine":"auto"}), json!({"model_profile":"standard"})] {
        assert_eq!(settings(&payload).model_profile.id(), "standard", "{payload}");
    }
}

#[test]
fn unknown_wrong_type_and_raw_model_expressions_are_rejected() {
    for profile in [
        json!("unknown"),
        json!("TurkishExtended"),
        json!("turkish-extended"),
        json!(" turkish_extended "),
        json!("script/Latin+tur"),
        json!(true),
        json!(42),
        json!({}),
        json!(null),
        json!(["standard"]),
    ] {
        let payload = json!({"language":"tr","model_profile":profile});
        assert!(Automation::new().ocr_settings(&crate::json::Args(&payload)).is_err(), "{payload}");
        let error = Automation::new().call("ocr_status", &payload).unwrap_err();
        assert!(matches!(error, crate::ToolError::InvalidArgs(_)), "status must reject the explicit profile before inventory discovery: {error}");
    }
}

#[test]
fn extended_accepts_only_turkish_tesseract_or_turkish_auto() {
    for engine in ["tesseract", "auto"] {
        let payload = json!({"language":"tr","engine":engine,"model_profile":"turkish_extended"});
        let options = settings(&payload);
        assert_eq!(options.model_profile.id(), "turkish_extended");
        assert!(pdfcraft_engine::ocr::validate_model_profile(&options).is_ok());
    }
    for payload in [
        json!({"language":"en","engine":"tesseract","model_profile":"turkish_extended"}),
        json!({"language":"tr","engine":"ocrs","model_profile":"turkish_extended"}),
        json!({"model_profile":"turkish_extended"}),
    ] {
        let error = Automation::new().call("ocr_status", &payload).unwrap_err().to_string();
        assert!(error.contains("profile") || error.contains("Turkish") || error.contains("Tesseract"), "{payload}: {error}");
    }
}

#[test]
fn frozen_extended_requires_only_latin_while_standard_turkish_still_requires_tur() {
    let extended = settings(&json!({"language":"tr","engine":"auto","model_profile":"turkish_extended"}));
    assert_eq!(extended.model_profile.required_packs("tr").unwrap(), vec!["script/Latin"]);
    let standard = settings(&json!({"language":"tr","engine":"tesseract","model_profile":"standard"}));
    assert_eq!(standard.model_profile.required_packs("tr").unwrap(), vec!["tur"]);
}

#[test]
fn unavailable_explicit_program_cannot_fall_back_to_standard_or_ocrs() {
    let dir = tempfile::tempdir().unwrap();
    for engine in ["tesseract", "auto"] {
        let mut options = settings(&json!({"language":"tr","engine":engine,"model_profile":"turkish_extended"}));
        options.tesseract_path = Some(dir.path().join("missing-tesseract.exe").to_string_lossy().into_owned());
        let error = pdfcraft_engine::ocr::recognizers(&options).err().expect("an unavailable explicit engine must fail");
        assert!(error.to_lowercase().contains("tesseract"), "{error}");
    }
}

#[cfg(unix)]
#[test]
fn tur_only_installation_refuses_extended_before_recognition_and_output() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("tur-only-tesseract");
    let marker = dir.path().join("recognition-must-not-run");
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"--list-langs\" ]; then printf 'List of available languages (1):\\ntur\\n'; exit 0; fi\nprintf 'called' > '{}'\nexit 79\n",
        marker.display()
    );
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    for engine in ["auto", "tesseract"] {
        let mut options = settings(&json!({"language":"tr","engine":engine,"model_profile":"turkish_extended"}));
        options.tesseract_path = Some(program.to_string_lossy().into_owned());
        let error = pdfcraft_engine::ocr::recognizers(&options).err().expect("tur alone is not the extended profile");
        assert!(error.contains("script/Latin") || error.contains("turkish_extended"), "missing profile must be named: {error}");
        assert!(!marker.exists(), "no recognition/fallback process may start");
    }
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn required_path(name: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("set {name} explicitly; missing setup is a failure, not a skip")));
    assert!(path.is_absolute(), "{name} must be an absolute path");
    path
}

fn exact_tokens(actual: &[String], expected: &[String], sample: &Value) {
    assert_eq!(actual, expected, "{}: ordered tokens must match; no correction or case folding", sample["id"]);
    for occurrence in sample["target_occurrences"].as_array().unwrap() {
        let index = usize::try_from(occurrence["token_index_zero_based"].as_u64().unwrap()).unwrap();
        let offset = usize::try_from(occurrence["character_offset"].as_u64().unwrap()).unwrap();
        let token = actual.get(index).expect("target token must exist in its original position");
        assert_eq!(token, occurrence["token"].as_str().unwrap());
        assert_eq!(token.chars().nth(offset).unwrap().to_string(), occurrence["expected"].as_str().unwrap());
    }
}

#[test]
fn presence_only_or_misplaced_characters_cannot_satisfy_the_acceptance_oracle() {
    let sample =
        json!({"id":"oracle-control","target_occurrences":[{"token_index_zero_based":0,"character_offset":4,"token":"millî","expected":"î"}]});
    let expected = vec!["millî".to_owned(), "Û".to_owned()];
    exact_tokens(&expected, &expected, &sample);
    for wrong in [vec!["milli".to_owned(), "îÎûÛ".to_owned()], vec!["Û".to_owned(), "millî".to_owned()]] {
        assert!(std::panic::catch_unwind(|| exact_tokens(&wrong, &expected, &sample)).is_err());
    }
}

// The old Python oracle is imported by hash, rather than adding a normalization
// dependency or silently changing NFC/Unicode-whitespace behavior in this test.
fn check_acceptance(actual: &[String], expected: &[String], sample: &Value, strict_v1: bool) {
    if strict_v1 {
        exact_tokens(actual, expected, sample);
        return;
    }
    let python = required_path("PDFCRAFT_EXTENDED_PYTHON");
    let metric = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packaging/windows/measure-turkish-ocr.py");
    assert_eq!(sha256(&std::fs::read(&metric).unwrap()), "8be0e932e453f6873de9d5643def0f4fffebec432bb763ed8c8ec084fe4b0539");
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.json");
    let output = dir.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&json!({"actual":actual,"expected":expected,"sample":sample,"metric":metric})).unwrap()).unwrap();
    const ORACLE: &str = r#"import hashlib,importlib.util,json,sys
sys.dont_write_bytecode=True
from pathlib import Path
p=json.loads(Path(sys.argv[1]).read_text('utf-8'))
metric=Path(p['metric'])
assert hashlib.sha256(metric.read_bytes()).hexdigest()=='8be0e932e453f6873de9d5643def0f4fffebec432bb763ed8c8ec084fe4b0539'
spec=importlib.util.spec_from_file_location('unchanged_corpus_oracle',metric)
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
a=m.normalized(' '.join(p['actual']));e=m.normalized(' '.join(p['expected']))
counts=m.edit_counts(e,a)
targets=[]
for x in p['sample']['target_occurrences']:
 i=x['token_index_zero_based'];j=x['character_offset']
 token=p['actual'][i] if i<len(p['actual']) else ''
 targets.append({'expected':x,'actual_token':token,'correct':token==x['token'] and j<len(token) and token[j]==x['expected']})
r={'normalized_cer':counts,'target_positions':targets,'pass':counts['cer']<=0.01 and all(x['correct'] for x in targets),'post_correction':False}
Path(sys.argv[2]).write_text(json.dumps(r,ensure_ascii=False),'utf-8')
"#;
    let mut child = Command::new(python)
        .args(["-X", "utf8", "-c", ORACLE])
        .arg(&input)
        .arg(&output)
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(dir.path().join("stderr.txt")).unwrap())
        .spawn()
        .expect("start the unchanged external metric oracle");
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("metric oracle wait: {error}");
            }
        }
        if started.elapsed() >= Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("metric oracle timeout");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "metric oracle failed: {}", std::fs::read_to_string(dir.path().join("stderr.txt")).unwrap());
    let result: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(result["pass"], true, "V2 independent acceptance failed: {result}");
}

fn reference_tokens(reference: &str) -> Vec<String> {
    reference.split_whitespace().map(str::to_owned).collect()
}

fn extracted_tokens(a: &mut Automation, id: DocId, page: usize) -> Vec<String> {
    let value = json(a, "text_extract", json!({"doc":id.0,"pages":[page]}));
    reference_tokens(value["pages"][0]["text"].as_str().unwrap())
}

fn assert_nfd(a: &mut Automation, id: DocId) {
    let value = json(a, "text_extract", json!({"doc":id.0,"pages":[2]}));
    assert_eq!(
        value["pages"][0]["text"].as_str().unwrap().trim_matches(['\r', '\n']),
        NFD_SENTINEL,
        "existing source NFD scalar order must stay exact"
    );
}

fn assert_boxes(result: &Value, page_size: [f64; 2]) -> Vec<String> {
    assert_eq!(result["pages"].as_array().unwrap().len(), 1);
    assert_eq!(result["pages"][0]["page"], 1);
    let words = result["pages"][0]["words"].as_array().unwrap();
    assert!(!words.is_empty());
    words
        .iter()
        .map(|word| {
            let rect = word["box"].as_array().unwrap();
            assert_eq!(rect.len(), 4);
            let rect: Vec<f64> = rect.iter().map(|x| x.as_f64().unwrap()).collect();
            assert!(rect.iter().all(|x| x.is_finite()) && rect[0] >= 0.0 && rect[1] >= 0.0 && rect[2] > 0.0 && rect[3] > 0.0, "{word}");
            assert!(rect[0] + rect[2] <= page_size[0] + 0.01 && rect[1] + rect[3] <= page_size[1] + 0.01, "word outside page: {word}");
            assert_eq!(word["source"], "tesseract", "Turkish auto must not merge OCRS output");
            let confidence = word["confidence"].as_f64().unwrap();
            assert!((0.0..=100.0).contains(&confidence));
            word["text"].as_str().unwrap().to_owned()
        })
        .collect()
}

fn direct_tsv(program: &Path, model_dir: &Path, image: &Path, output: &Path, expression: &str, sample: &Value) -> (Vec<String>, Value) {
    let base = output.join("recognized");
    let argv = vec![
        image.to_string_lossy().into_owned(),
        base.to_string_lossy().into_owned(),
        "--tessdata-dir".into(),
        model_dir.to_string_lossy().into_owned(),
        "-l".into(),
        expression.into(),
        "--dpi".into(),
        "300".into(),
        "--oem".into(),
        "1".into(),
        "--psm".into(),
        "3".into(),
        "-c".into(),
        "tessedit_create_txt=1".into(),
        "-c".into(),
        "tessedit_create_tsv=1".into(),
    ];
    let mut child = Command::new(program)
        .args(&argv)
        .env("OMP_THREAD_LIMIT", "2")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(output.join("stdout.txt")).unwrap())
        .stderr(std::fs::File::create(output.join("stderr.txt")).unwrap())
        .spawn()
        .expect("start real Tesseract");
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("wait for Tesseract: {error}");
            }
        }
        if started.elapsed() >= Duration::from_secs(180) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("real Tesseract exceeded the 180 second timeout");
        }
        if std::fs::metadata(output.join("stdout.txt")).unwrap().len() > 16 * 1024 * 1024
            || std::fs::metadata(output.join("stderr.txt")).unwrap().len() > 1024 * 1024
        {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Tesseract output exceeded the bounded capture");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let invocation = json!({"program":program,"argv":argv,"exit_code":status.code(),"seconds":started.elapsed().as_secs_f64(),"timeout_seconds":180,"OMP_THREAD_LIMIT":2});
    std::fs::write(output.join("invocation.json"), serde_json::to_vec_pretty(&invocation).unwrap()).unwrap();
    assert!(status.success(), "{invocation}; stderr: {}", std::fs::read_to_string(output.join("stderr.txt")).unwrap());
    let tsv = std::fs::read_to_string(base.with_extension("tsv")).expect("real TSV output is required");
    assert!(tsv.starts_with("level\tpage_num\t"));
    let (width, height) = (sample["pixels"][0].as_u64().unwrap() as f64, sample["pixels"][1].as_u64().unwrap() as f64);
    let mut words = Vec::new();
    for line in tsv.lines().skip(1) {
        let row: Vec<&str> = line.splitn(12, '\t').collect();
        assert_eq!(row.len(), 12, "malformed TSV row: {line}");
        if row[0] != "5" {
            continue;
        }
        let confidence: f64 = row[10].parse().unwrap();
        if confidence < 0.0 {
            continue;
        }
        assert!(confidence.is_finite() && confidence <= 100.0);
        let rect: Vec<f64> = row[6..10].iter().map(|x| x.parse().unwrap()).collect();
        assert!(rect.iter().all(|x| x.is_finite()) && rect[0] >= 0.0 && rect[1] >= 0.0 && rect[2] > 0.0 && rect[3] > 0.0);
        assert!(rect[0] + rect[2] <= width && rect[1] + rect[3] <= height, "{line}");
        assert!(!row[11].is_empty());
        words.push(row[11].to_owned());
    }
    assert_eq!(
        reference_tokens(&std::fs::read_to_string(base.with_extension("txt")).unwrap()),
        words,
        "real TXT/TSV content must agree in original order"
    );
    (words, invocation)
}
fn assert_metadata(result: &Value, required: &[&str]) {
    assert_eq!(result["requested_model_profile"], "turkish_extended");
    assert_eq!(result["effective_model_profile"], "turkish_extended");
    assert_eq!(result["model_engine"], "tesseract");
    assert_eq!(result["required_models"], json!(required));
    let installed = result["installed_models"].as_array().unwrap();
    for pack in required {
        assert!(installed.contains(&json!(pack)), "{result}");
    }
}

/// Run only after P4's old-corpus selection is frozen. Requires explicit real engine,
/// model directory, pinned holdout, selection evidence and a NEW report directory.
/// PDFs remain temporary; retained evidence is TSV/TXT/JSON only.
#[test]
#[ignore = "root-only real Tesseract acceptance: explicit engine/models, frozen holdout and candidate-selection evidence required"]
fn real_tesseract_extended_holdout_raw_tsv_searchable_editable_nfd_and_undo() {
    run_real_holdout(HOLDOUT_SHA256, true);
}

#[test]
#[ignore = "root-only V2: independently frozen new holdout, frozen candidate and existing NFC/whitespace CER oracle"]
fn real_tesseract_extended_holdout_v2_cer_targets_roundtrips_nfd_and_undo() {
    run_real_holdout(HOLDOUT_V2_SHA256, false);
}

#[test]
#[ignore = "root-only V3: four independently frozen pages, frozen candidate decision, unchanged CER/target-position oracle"]
fn real_tesseract_extended_holdout_v3_four_pages_cer_targets_roundtrips_nfd_and_undo() {
    run_real_holdout(HOLDOUT_V3_SHA256, false);
}

struct RealHoldout<'a> {
    manifest_path: &'a Path,
    program: &'a Path,
    model_dir: &'a Path,
    report_dir: &'a Path,
    required: &'a [&'static str],
    expression: &'a str,
    strict_v1: bool,
}

impl RealHoldout<'_> {
    fn run_sample(&self, sample: &Value) -> Value {
        let (manifest_path, program, model_dir, report_dir) = (self.manifest_path, self.program, self.model_dir, self.report_dir);
        let (required, expression, strict_v1) = (self.required, self.expression, self.strict_v1);
        let sid = sample["id"].as_str().unwrap();
        assert!(!sid.contains(['/', '\\']));
        let sample_dir = report_dir.join(sid);
        std::fs::create_dir(&sample_dir).unwrap();
        let holdout = manifest_path.parent().unwrap();
        let image = holdout.join(sample["png"].as_str().unwrap());
        let reference = holdout.join(sample["reference"].as_str().unwrap());
        let image_bytes = std::fs::read(&image).unwrap();
        let reference_bytes = std::fs::read(&reference).unwrap();
        assert_eq!(sha256(&image_bytes), sample["png_sha256"].as_str().unwrap());
        assert_eq!(sha256(&reference_bytes), sample["reference_sha256"].as_str().unwrap());
        let expected = reference_tokens(std::str::from_utf8(&reference_bytes).unwrap());
        assert_eq!(json!(expected), sample["expected_tokens"]);
        let (raw, invocation) = direct_tsv(program, model_dir, &image, &sample_dir, expression, sample);
        check_acceptance(&raw, &expected, sample, strict_v1);

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("scan.png"), &image_bytes).unwrap();
        let mut setup = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
        let id = DocId(json(&mut setup, "doc_create", json!({"from":"images","paths":["scan.png","scan.png"],"dpi":300}))["doc"].as_u64().unwrap());
        // This is pre-existing document text on a page we do not recognize. It is not a
        // substitute for recognition output, a review correction, or an OCR accuracy oracle.
        json(&mut setup, "page_add_text", json!({"doc":id.0,"page":2,"text":NFD_SENTINEL,"at":[20,25],"size":10}));
        assert_nfd(&mut setup, id);
        json(&mut setup, "doc_save", json!({"doc":id.0,"path":"Kaynak.pdf"}));
        let source_file = std::fs::read(dir.path().join("Kaynak.pdf")).unwrap();
        drop(setup);
        let mut a = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
        let source = DocId(json(&mut a, "doc_open", json!({"path":"Kaynak.pdf"}))["doc"].as_u64().unwrap());
        let before = a.session.save_bytes(source).unwrap();
        let pixels_before = a.render_pixels(source.0, 1, 1.0).unwrap();
        let words_args =
            json!({"doc":source.0,"pages":[1],"language":"tr","engine":"auto","model_profile":"turkish_extended","retain_review":true,"dpi":300});
        let words = json(&mut a, "ocr_words", words_args.clone());
        std::fs::write(sample_dir.join("ocr-words-editable.json"), serde_json::to_vec_pretty(&words).unwrap()).unwrap();
        assert_metadata(&words, required);
        let info = &a.session.get(source).unwrap().info.pages[0];
        let recognized_tokens = assert_boxes(&words, [f64::from(info.width), f64::from(info.height)]);
        check_acceptance(&recognized_tokens, &expected, sample, strict_v1);
        assert_eq!(a.session.save_bytes(source).unwrap(), before);
        assert_nfd(&mut a, source);
        let review = words["review"].as_u64().unwrap();
        std::fs::write(dir.path().join("existing.pdf"), b"existing-output-sentinel").unwrap();
        assert!(a.call("ocr_review_apply", &json!({"doc":source.0,"review":review,"output_mode":"editable","out":"existing.pdf"})).is_err());
        assert_eq!(std::fs::read(dir.path().join("existing.pdf")).unwrap(), b"existing-output-sentinel");
        assert!(a.ocr_reviews.contains_key(&source), "failed publication must keep the real review");
        json(&mut a, "ocr_review_apply", json!({"doc":source.0,"review":review,"output_mode":"editable","out":"Editable.pdf"}));
        assert!(!a.ocr_reviews.contains_key(&source));
        assert_eq!(a.session.save_bytes(source).unwrap(), before, "editable export must leave source bytes unchanged");
        assert!(a.render_pixels(source.0, 1, 1.0).unwrap() == pixels_before);
        assert_nfd(&mut a, source);
        assert!(a.call("edit_undo", &json!({"doc":source.0})).is_err(), "export must not create a source undo step");
        let mut editable = Automation::new().with_root(dir.path()).unwrap();
        let out = DocId(json(&mut editable, "doc_open", json!({"path":"Editable.pdf"}))["doc"].as_u64().unwrap());
        assert_eq!(extracted_tokens(&mut editable, out, 1), recognized_tokens, "editable output must preserve actual recognition exactly");
        check_acceptance(&extracted_tokens(&mut editable, out, 1), &expected, sample, strict_v1);
        assert!(editable.session.get(out).unwrap().page_images(0).is_empty(), "editable output must not hide missing glyphs behind the scan");
        let ink = editable.render_pixels(out.0, 1, 1.0).unwrap();
        assert!(ink.2.chunks_exact(4).any(|p| p[0] < 128 && p[1] < 128 && p[2] < 128), "editable text must draw visible ink");
        json(&mut editable, "doc_save", json!({"doc":out.0,"path":"Editable-again.pdf"}));
        let mut fresh = Automation::new().with_root(dir.path()).unwrap();
        let re = DocId(json(&mut fresh, "doc_open", json!({"path":"Editable-again.pdf"}))["doc"].as_u64().unwrap());
        assert_eq!(extracted_tokens(&mut fresh, re, 1), recognized_tokens, "second editable save/reopen must preserve actual recognition exactly");
        check_acceptance(&extracted_tokens(&mut fresh, re, 1), &expected, sample, strict_v1);
        assert!(fresh.render_pixels(re.0, 1, 1.0).unwrap() == ink);

        // A second actual reading exercises direct searchable recognition and its metadata.
        let recognized = json(
            &mut a,
            "ocr_recognize",
            json!({"doc":source.0,"pages":[1],"language":"tr","engine":"auto","model_profile":"turkish_extended","output_mode":"searchable","dpi":300}),
        );
        std::fs::write(sample_dir.join("ocr-recognize-searchable.json"), serde_json::to_vec_pretty(&recognized).unwrap()).unwrap();
        assert_metadata(&recognized, required);
        let searchable_tokens = reference_tokens(recognized["pages"][0]["text"].as_str().expect("actual searchable recognition text"));
        check_acceptance(&searchable_tokens, &expected, sample, strict_v1);
        assert_eq!(extracted_tokens(&mut a, source, 1), searchable_tokens, "searchable apply/redo must preserve actual recognition exactly");
        check_acceptance(&extracted_tokens(&mut a, source, 1), &expected, sample, strict_v1);
        assert_nfd(&mut a, source);
        json(&mut a, "doc_save", json!({"doc":source.0,"path":"Searchable.pdf"}));
        let mut searchable = Automation::new().with_root(dir.path()).unwrap();
        let re = DocId(json(&mut searchable, "doc_open", json!({"path":"Searchable.pdf"}))["doc"].as_u64().unwrap());
        assert_eq!(extracted_tokens(&mut searchable, re, 1), searchable_tokens, "searchable save/reopen must preserve actual recognition exactly");
        check_acceptance(&extracted_tokens(&mut searchable, re, 1), &expected, sample, strict_v1);
        assert_nfd(&mut searchable, re);
        json(&mut a, "edit_undo", json!({"doc":source.0}));
        assert!(extracted_tokens(&mut a, source, 1).is_empty());
        assert_nfd(&mut a, source);
        assert!(a.render_pixels(source.0, 1, 1.0).unwrap() == pixels_before);
        json(&mut a, "edit_redo", json!({"doc":source.0}));
        assert_eq!(extracted_tokens(&mut a, source, 1), searchable_tokens, "searchable apply/redo must preserve actual recognition exactly");
        check_acceptance(&extracted_tokens(&mut a, source, 1), &expected, sample, strict_v1);
        assert_nfd(&mut a, source);
        assert_eq!(std::fs::read(dir.path().join("Kaynak.pdf")).unwrap(), source_file);
        assert_eq!(std::fs::read(dir.path().join("scan.png")).unwrap(), image_bytes);
        let completed = json!({"id":sid,"status":"PASS","raw_tsv_tokens":raw,"invocation":invocation,"expected_target_occurrences":sample["target_occurrences"],"source_file_sha256":sha256(&source_file),"source_nfd_preserved":true,"source_undo_redo_preserved":true,"editable_and_searchable_reopened":true});
        // Keep completed sample evidence if a later sample fails the independent oracle.
        std::fs::write(sample_dir.join("sample-report.json"), serde_json::to_vec_pretty(&completed).unwrap()).unwrap();
        completed
    }
}

fn run_real_holdout(expected_manifest_sha256: &str, strict_v1: bool) {
    let manifest_path = required_path("PDFCRAFT_EXTENDED_HOLDOUT_MANIFEST");
    let manifest_bytes = std::fs::read(&manifest_path).unwrap();
    assert_eq!(sha256(&manifest_bytes), expected_manifest_sha256, "holdout must not change after model selection");
    let manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
    let selection = required_path("PDFCRAFT_EXTENDED_SELECTION");
    let selection_sha = std::env::var("PDFCRAFT_EXTENDED_SELECTION_SHA256").expect("pin the already-frozen P4 selection evidence");
    assert_eq!(selection_sha.len(), 64);
    assert_eq!(sha256(&std::fs::read(&selection).unwrap()), selection_sha);
    let program = required_path("PDFCRAFT_TESSERACT");
    let model_dir = required_path("TESSDATA_PREFIX");
    assert!(program.is_file() && model_dir.is_dir());
    let report_dir = required_path("PDFCRAFT_EXTENDED_REPORT_DIR");
    std::fs::create_dir(&report_dir).expect("report directory must be new; never replace earlier acceptance evidence");
    let options = settings(&json!({"language":"tr","engine":"auto","model_profile":"turkish_extended"}));
    let required = options.model_profile.required_packs("tr").unwrap();
    assert_eq!(required, vec!["script/Latin"], "use the frozen P4 winner, never reselect on holdout");
    let expression = required.join("+");
    let model_hashes: Vec<Value> = required
        .iter()
        .map(|pack| {
            let path = model_dir.join(format!("{pack}.traineddata"));
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("required model {}: {e}; no standard fallback", path.display()));
            json!({"pack":pack,"sha256":sha256(&bytes)})
        })
        .collect();
    std::fs::write(
        report_dir.join("started.json"),
        serde_json::to_vec_pretty(&json!({
            "status":"STARTED_NOT_PASS","holdout_sha256":expected_manifest_sha256,"selection":selection,"selection_sha256":selection_sha,
            "program":program,"program_sha256":sha256(&std::fs::read(&program).unwrap()),"model_expression":expression,"models":model_hashes,
            "post_correction":false,"reference_injection":false,"pdfs_retained":false,"strict_v1_full_tokens":strict_v1
        }))
        .unwrap(),
    )
    .unwrap();
    let samples = manifest["samples"].as_array().unwrap();
    // V1/V2 keep their original two-page contract. New manifests declare their
    // sample count inside the hash-verified payload; a missing page must fail.
    let declared_samples = manifest.get("declared_sample_count").map_or(2, |count| {
        usize::try_from(count.as_u64().expect("declared sample count must be a non-negative integer")).expect("declared sample count must fit usize")
    });
    assert!((1..=64).contains(&declared_samples), "declared sample count must be in 1..=64");
    assert_eq!(samples.len(), declared_samples);
    let context = RealHoldout {
        manifest_path: &manifest_path,
        program: &program,
        model_dir: &model_dir,
        report_dir: &report_dir,
        required: &required,
        expression: &expression,
        strict_v1,
    };
    let reports: Vec<Value> = samples.iter().map(|sample| context.run_sample(sample)).collect();
    assert_eq!(sha256(&std::fs::read(&manifest_path).unwrap()), expected_manifest_sha256);
    std::fs::write(report_dir.join("report.json"), serde_json::to_vec_pretty(&json!({"status":"PASS","holdout_sha256":expected_manifest_sha256,"samples":reports,"selection_sha256":selection_sha,"models":model_hashes,"post_correction":false,"reference_injection":false})).unwrap()).unwrap();
}

#[test]
#[ignore = "root-only real tur-only model-directory refusal; run separately from the full-model acceptance process"]
fn real_missing_extended_pack_is_an_error_without_output_or_source_changes() {
    let program = required_path("PDFCRAFT_TESSERACT");
    let model_dir = required_path("TESSDATA_PREFIX");
    assert!(program.is_file() && model_dir.join("tur.traineddata").is_file());
    let options = settings(&json!({"language":"tr","engine":"tesseract","model_profile":"turkish_extended"}));
    let required = options.model_profile.required_packs("tr").unwrap();
    assert!(
        required.iter().any(|p| !model_dir.join(format!("{p}.traineddata")).is_file()),
        "negative control needs an actually missing required pack"
    );
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let id = DocId(json(&mut a, "doc_create", json!({"from":"blank"}))["doc"].as_u64().unwrap());
    let before = a.session.save_bytes(id).unwrap();
    let selection = json!({"language":"tr","engine":"tesseract","model_profile":"turkish_extended"});
    let status = json(&mut a, "ocr_status", selection);
    assert_eq!(status["requested_model_profile"], "turkish_extended");
    assert_eq!(status["effective_model_profile"], Value::Null, "missing model is not an effective Standard profile");
    assert_eq!(status["required_models"], json!(required));
    assert!(status["installed_models"].as_array().unwrap().contains(&json!("tur")), "real tur-only inventory must be visible: {status}");
    for tool in ["ocr_words", "ocr_recognize"] {
        let mut arguments = json!({"doc":id.0,"language":"tr","engine":"tesseract","model_profile":"turkish_extended"});
        if tool == "ocr_words" {
            arguments["retain_review"] = json!(true);
        } else {
            arguments["output_mode"] = json!("editable");
            arguments["out"] = json!("must-not-exist.pdf");
        }
        let error = a.call(tool, &arguments).unwrap_err().to_string();
        assert!(error.contains("turkish_extended") || required.iter().any(|pack| error.contains(pack)), "missing model must be named: {error}");
        assert!(!dir.path().join("must-not-exist.pdf").exists());
        assert_eq!(a.session.save_bytes(id).unwrap(), before);
        assert!(!a.ocr_reviews.contains_key(&id), "failed recognition must not create a successful review");
        assert!(a.call("edit_undo", &json!({"doc":id.0})).is_err());
    }
}
