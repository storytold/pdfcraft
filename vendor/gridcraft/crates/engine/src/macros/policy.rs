//! What a macro step may run. Macros run commands, so this keeps a macro from saving, opening or
//! closing files, driving the app, running other macros, or reading and writing files without the
//! person knowing. It goes by command id and the `path` parameter; the runner also runs every
//! step except an approved file step inside the scripts' file sandbox, so a command that touches a
//! file under another parameter can't either.

use serde_json::Value as Json;

use crate::Trust;

/// Whether a command may run as a macro step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Policy {
    Allow,
    /// Allowed once the person approves the macro; the text says why it asks ("came with a
    /// workbook you opened").
    Ask(String),
    /// Reads or writes files: the macro must list the command in its permissions, and the person
    /// approves the macro before it runs. The text says what it does ("reads a file").
    NeedsPermission(String),
    /// Never runs in a macro; the text says why.
    Deny(String),
}

/// Prefixes of commands that never run in a macro, with the reason.
const DENIED_PREFIXES: &[(&str, &str)] = &[
    ("file.", "macros don't open, save, close or create workbooks"),
    ("app.", "macros don't change the app"),
    ("window.", "macros don't switch windows"),
    ("automate.", "macros don't record or run scripts or other macros"),
    ("macro.", "macros don't record or run other macros"),
];

/// Single commands that never run in a macro.
const DENIED: &[(&str, &str)] = &[
    ("edit.undo", "a macro is undone as one step after it runs"),
    ("edit.redo", "a macro is undone as one step after it runs"),
    ("view.newWindow", "macros don't open, hide or show windows"),
    ("view.hideWindow", "macros don't open, hide or show windows"),
    ("view.unhideWindow", "macros don't open, hide or show windows"),
    ("control.press", "macros don't press form controls"),
];

/// Commands under a denied prefix that only change the workbook.
const ALLOWED: &[&str] = &["file.properties"];

/// Commands that read a file when given a `path`.
const READS_FILES: &[&str] = &["data.getData", "data.fromTextCsv", "insert.picture", "pageLayout.background"];

/// File commands that only produce bytes unless given a `path` (then they write a file).
const EXPORTS: &[&str] = &["file.exportPdf", "file.exportCsv", "file.exportHtml", "file.print", "file.printPreview", "file.saveBytes"];

/// Whether command `id` with `params` writes a file (an export or print given a `path`).
pub fn writes_file(id: &str, params: &Json) -> bool {
    EXPORTS.contains(&id) && params.get("path").is_some_and(|p| !p.is_null())
}

/// The policy for running command `id` with `params` as a macro step in a document with `trust`.
///
/// Any `path` other than `null` counts as touching a file: a token such as `{"$ref": "Input"}`
/// becomes a path when the step runs, and exports write to `""` and `" "` as file names. The
/// runner checks the parameters both as written and as resolved.
pub fn macro_policy(id: &str, params: &Json, trust: Trust) -> Policy {
    let has_path = params.get("path").is_some_and(|p| !p.is_null());
    let base = if EXPORTS.contains(&id) {
        if has_path { Policy::NeedsPermission("writes a file".into()) } else { Policy::Allow }
    } else if ALLOWED.contains(&id) {
        Policy::Allow
    } else if let Some((_, why)) = DENIED.iter().find(|(d, _)| *d == id) {
        Policy::Deny((*why).into())
    } else if let Some((_, why)) = DENIED_PREFIXES.iter().find(|(p, _)| id.starts_with(p)) {
        Policy::Deny((*why).into())
    } else if READS_FILES.contains(&id) && has_path {
        Policy::NeedsPermission("reads a file".into())
    } else {
        Policy::Allow
    };
    match base {
        Policy::Allow if !trust.allows_code() => Policy::Ask("came with a workbook you opened".into()),
        other => other,
    }
}

/// Whether the footprint dry run can preview a command. Solver can take long, so a dry run stops
/// before it and reports the footprint as partial.
pub fn previewable(id: &str) -> bool {
    id != "data.solver"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_deny_and_permission_lists() {
        let none = json!({});
        let path = json!({"path": "/tmp/x"});
        for id in [
            "file.saveAs",
            "file.save",
            "file.open",
            "file.close",
            "file.new",
            "app.commands",
            "window.activate",
            "automate.runScript",
            "automate.recordActions",
            "macro.run",
            "macro.record",
            "edit.undo",
            "edit.redo",
            "view.newWindow",
            "view.hideWindow",
            "view.unhideWindow",
            "control.press",
        ] {
            assert!(matches!(macro_policy(id, &none, Trust::Local), Policy::Deny(_)), "{id}");
            assert!(matches!(macro_policy(id, &path, Trust::FromFile), Policy::Deny(_)), "{id}");
        }
        for id in ["data.getData", "data.fromTextCsv", "insert.picture", "pageLayout.background", "file.exportPdf", "file.print", "file.exportCsv"] {
            assert!(matches!(macro_policy(id, &path, Trust::Local), Policy::NeedsPermission(_)), "{id}");
            assert!(matches!(macro_policy(id, &path, Trust::FromFile), Policy::NeedsPermission(_)), "{id}");
        }
        // Without a path they touch no file.
        for id in ["data.getData", "insert.picture", "file.exportPdf", "file.saveBytes", "file.printPreview"] {
            assert_eq!(macro_policy(id, &none, Trust::Local), Policy::Allow, "{id}");
            assert_eq!(macro_policy(id, &json!({"path": null}), Trust::Local), Policy::Allow, "{id}");
        }
        // A path that is blank or a token still touches a file once the step runs.
        for path in [json!(""), json!(" "), json!({"$ref": "Input"}), json!("{{Input}}.csv"), json!(1)] {
            for id in ["file.exportCsv", "data.fromTextCsv"] {
                let p = json!({ "path": path });
                assert!(matches!(macro_policy(id, &p, Trust::Local), Policy::NeedsPermission(_)), "{id} {p}");
            }
        }
        for id in ["cell.set", "home.bold", "view.macros", "edit.copy", "file.properties"] {
            assert_eq!(macro_policy(id, &none, Trust::Local), Policy::Allow, "{id}");
            assert!(matches!(macro_policy(id, &none, Trust::FromFile), Policy::Ask(_)), "{id}");
        }
        assert!(!previewable("data.solver") && previewable("cell.set"));
    }

    #[test]
    fn every_listed_command_exists() {
        for id in ALLOWED.iter().chain(READS_FILES).chain(EXPORTS).chain(DENIED.iter().map(|(id, _)| id)).chain(&["data.solver"]) {
            assert!(crate::find_command(id).is_some(), "{id}");
        }
    }
}
