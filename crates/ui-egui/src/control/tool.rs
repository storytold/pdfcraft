//! `tool.call`: automation tools run on the window's own documents (see [`super`]).

use super::{Reply, Value};

/// Whether `name` with `args` is, or wraps, a signing call. Signing is the person's act.
pub(super) fn agent_signing_call(name: &str, args: &Value, depth: usize) -> bool {
    if depth > 16 || name.starts_with("sign_") || (name.starts_with("sign.") && name != "sign.fill.text") {
        return true;
    }
    if matches!(name, "fill_sign_add" | "sign.fill.text") && matches!(args["type"].as_str(), Some("signature" | "initials")) {
        return true;
    }
    match name {
        "command_run" => args["id"].as_str().is_some_and(|id| agent_signing_call(id, args.get("params").unwrap_or(&Value::Null), depth + 1)),
        "command_batch" => args["steps"].as_array().is_some_and(|steps| steps.iter().any(|step| agent_signing_call("command_run", step, depth + 1))),
        "tool_call" => args["name"].as_str().is_some_and(|n| agent_signing_call(n, args.get("arguments").unwrap_or(&Value::Null), depth + 1)),
        _ => false,
    }
}

/// The tools that only read, so an Ask Agent run may call them without holding the edit turn.
pub(super) fn agent_read_only_tool(name: &str) -> bool {
    matches!(
        name,
        "doc_list"
            | "doc_info"
            | "doc_inspect"
            | "render_preview"
            | "command_list"
            | "text_extract"
            | "text_find"
            | "page_render"
            | "form_fields"
            | "comment_list"
            | "bookmark_list"
            | "form_detect_fields"
    )
}

impl crate::PdfCraftApp {
    pub(crate) fn run_tool(&mut self, name: &str, args: &Value, root: Option<&str>) -> Reply {
        // An agent must not overwrite an editor's draft, save an older value, or leave stale
        // field/object indexes after changing the document. The person commits typing first.
        if self.dialog.is_some()
            || self.password_prompt.is_some()
            || self.bookmark_rename.is_some()
            || self.views.iter().any(|v| {
                v.comments.composer.is_some()
                    || !v.comments.reply.is_empty()
                    || v.comments.editing.is_some()
                    || !v.comments.add_box.is_empty()
                    || v.pending_edit.is_some()
                    || v.content.draft.is_some()
                    || v.forms.committed.is_some()
                    || v.forms
                        .focus
                        .as_ref()
                        .is_some_and(|f| self.session.get(v.id).is_some_and(|d| crate::forms_ui::draft_edit(f, &d.form).is_some()))
                    || v.fill_text.as_ref().is_some_and(|t| !t.text.trim().is_empty())
                    || v.line_editor.as_ref().is_some_and(|ed| ed.has_unsaved_text())
            })
        {
            return Err(
                "tool.call: finish typing and close the open dialog in the window before running a document tool; your draft has been kept".into()
            );
        }
        let Some(runner) = self.tool_runner.as_mut() else {
            return Err("tool.call: this build of PdfCraft can't run automation tools in the window".into());
        };
        let before: Vec<(pdfcraft_engine::DocId, u64)> = self.session.docs().iter().map(|d| (d.id, d.edit_generation())).collect();
        let result = runner(&mut self.session, name, args, root.map(std::path::Path::new));
        self.show_tool_changes(&before);
        result
    }

    /// Show what a tool run through `tool.call` changed: documents it edited are drawn again, the
    /// ones it opened or made get a tab (behind the person's, which stays active), and tabs whose
    /// document it closed go. `before` is each document's edit generation before the tool ran.
    fn show_tool_changes(&mut self, before: &[(pdfcraft_engine::DocId, u64)]) {
        let active = self.active_ids().map(|(_, id)| id);
        for view in &mut self.views {
            let Some(doc) = self.session.get(view.id) else { continue };
            if before.iter().any(|(id, generation)| *id == view.id && *generation != doc.edit_generation()) {
                view.document_replaced(&doc.info);
            }
        }
        for doc in self.session.docs() {
            if !before.iter().any(|(id, _)| *id == doc.id) && !self.views.iter().any(|v| v.id == doc.id) {
                self.views.push(crate::DocView::new(doc.id, &doc.info, self.view_defaults));
            }
        }
        let session = &self.session;
        self.views.retain(|v| session.get(v.id).is_some() || !before.iter().any(|(id, _)| *id == v.id));
        // The person's tab stays active; Home (no active tab) stays Home.
        self.active = match active {
            Some(id) => self.views.iter().position(|v| v.id == id).or_else(|| self.views.len().checked_sub(1)),
            None => None,
        };
    }
}
