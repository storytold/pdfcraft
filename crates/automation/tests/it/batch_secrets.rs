//! Tool results and errors from the folder tools carry no known test password.
//!
//! The passwords are set on the document and passed to the tool, and must never come back out: a
//! result that echoes one has leaked it into logs, transcripts and MCP callers.

use pdfcraft_automation::{Automation, Content};
use pdfcraft_engine::{Changes, Edit, Protection, Session};
use serde_json::{Value, json};

/// The known test passwords; neither may appear in a result or an error.
const OPEN_PASSWORD: &str = "PLANT-OPEN-5f2c9a";
const OWNER_PASSWORD: &str = "PLANT-OWNER-71de0b";

/// A one-page PDF locked with the known test passwords.
///
/// The open password is the only way in. The batch bates tool rewrites page content, and
/// `Changes::None` forbids that, so this fixture permits every change except extracting pages;
/// the restrictions stay real, since the open password never lifts them.
fn protected_pdf() -> Vec<u8> {
    let mut session = Session::new();
    let bytes = session.create_from_text("Batch secrets", "Planted password test").unwrap();
    let id = session.open("locked.pdf", None, bytes, None).unwrap();
    session
        .apply(
            id,
            Edit::Protect(Protection {
                open_password: Some(OPEN_PASSWORD.into()),
                permissions_password: Some(OWNER_PASSWORD.into()),
                changes: Changes::AnyExceptExtract,
                ..Default::default()
            }),
        )
        .unwrap();
    session.save_bytes(id).unwrap().as_ref().clone()
}

fn json_result(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args) {
        Ok(mut content) => match content.remove(0) {
            Content::Json(value) => value,
            other => panic!("{tool}: expected JSON, got {other:?}"),
        },
        Err(error) => panic!("{tool} {args}: {error}"),
    }
}

#[test]
fn results_and_errors_hold_no_planted_password_for_bates_or_batch() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("locked.pdf"), protected_pdf()).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();

    // The right open password: numbering succeeds and reports the numbers only.
    let numbered =
        json_result(&mut a, "doc_bates_multi", json!({"files":[{"input":"locked.pdf","output":"locked-numbered.pdf","password":OPEN_PASSWORD}]}));
    assert_eq!(numbered["completed"], 1, "{numbered}");
    assert_eq!(numbered["files"][0]["status"], "ok", "{numbered}");
    // The report carries the per-file entry the Bates tool builds, so a field added to that entry
    // lands in the text checked below.
    assert_eq!(numbered["files"][0]["first"], 1, "{numbered}");
    assert!(numbered["files"][0]["output"].as_str().is_some_and(|path| path.ends_with("locked-numbered.pdf")), "{numbered}");
    assert!(dir.path().join("locked-numbered.pdf").exists(), "the numbered copy is written");

    // The wrong open password: the tool refuses the file with an error.
    let refused = match a
        .call("doc_bates_multi", &json!({"files":[{"input":"locked.pdf","output":"locked-refused.pdf","password":"not-the-open-password"}]}))
    {
        Err(error) => error.to_string(),
        Ok(value) => panic!("a wrong open password must be refused: {value:?}"),
    };

    // The folder job refuses the encrypted file without a password, in its per-file report.
    let batch = json_result(&mut a, "doc_batch", json!({"paths":["locked.pdf"],"steps":["optimize"],"out_dir":"out"}));
    assert_eq!(batch["failed"], 1, "{batch}");
    assert_eq!(batch["files"][0]["status"], "error", "{batch}");
    assert!(batch["files"][0]["error"].as_str().is_some_and(|text| !text.is_empty()), "{batch}");
    assert!(!dir.path().join("out/locked.pdf").exists(), "a refused file publishes nothing");

    for report in [serde_json::to_string(&numbered).unwrap(), refused, serde_json::to_string(&batch).unwrap()] {
        for secret in [OPEN_PASSWORD, OWNER_PASSWORD] {
            assert!(!report.contains(secret), "a result or error echoed a planted password: {report}");
        }
    }
}
