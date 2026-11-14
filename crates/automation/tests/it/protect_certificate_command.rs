//! The registry command `protect.certificate` runs through its headless tool.

use pdfcraft_automation::{Automation, Content};
use pdfcraft_engine::Session;
use serde_json::{Value, json};

fn json_of(mut content: Vec<Content>) -> Value {
    match content.remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

#[test]
fn protect_certificate_command_runs_its_tool() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-protect-certificate-command-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let session = Session::new();
    let bytes = session.create_blank(200.0, 300.0, 1).unwrap();
    std::fs::write(dir.join("a.pdf"), bytes.as_slice()).unwrap();
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/rsa-aes.p12"), dir.join("me.p12")).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap().with_clock(|| 1_700_000_000);
    let doc = json_of(a.call("doc_open", &json!({ "path": "a.pdf" })).unwrap())["doc"].as_u64().unwrap();

    let list = json_of(a.call("command_list", &json!({ "doc": doc, "filter": "protect.certificate" })).unwrap());
    let entry = list["commands"].as_array().unwrap().iter().find(|c| c["id"] == "protect.certificate").expect("the command is registered");
    assert_eq!(entry["tool"], "doc_protect_certificates", "{entry}");

    let result = json_of(
        a.call("command_run", &json!({ "id": "protect.certificate", "params": { "doc": doc, "certificates": ["me.p12"], "password": "test" } }))
            .expect("the command reaches its tool"),
    );
    assert_eq!(result["recipients"].as_array().map(Vec::len), Some(1), "{result}");
    let _ = std::fs::remove_dir_all(&dir);
}
