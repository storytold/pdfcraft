//! Authorization, execution and request advancement are one operation on the document owner.
use super::target;
use crate::{
    PdfCraftApp,
    control::{Reply, tool},
};
use pdfcraft_engine::DocId;
use serde_json::{Value, json};

impl PdfCraftApp {
    pub(crate) fn ask_agent_reply(&mut self, connection: u64, params: &Value) -> Reply {
        let hub = self.ask_agent.hub.clone();
        let value = hub
            .handle(connection, "agent.reply", params, || true, |expected| target::current(&self.session, expected))
            .ok_or("agent.reply: unknown method")?;
        if value["ok"] == true {
            Ok(value["result"].clone())
        } else {
            Err(format!(
                "{}: {}",
                value["errorCode"].as_str().unwrap_or("INVALID_PARAMS"),
                value["error"].as_str().unwrap_or("agent response refused")
            ))
        }
    }

    pub(crate) fn ask_agent_tool(&mut self, connection: u64, request: Option<u64>, name: &str, args: &Value, root: Option<&str>) -> Reply {
        if tool::agent_signing_call(name, args, 0) {
            return Err("HUMAN_SIGNATURE_REQUIRED: signing is person-only".into());
        }
        let hub = self.ask_agent.hub.clone();
        if tool::agent_read_only_tool(name, args) {
            return self.run_connected_tool(connection, name, args, root);
        }
        let Some(request) = request else {
            // Ordinary connected commands retain their capabilities. Only a document actually
            // held by an open Ask Agent request is unavailable without its owning claim.
            if let Some(open) = &self.ask_agent.open {
                let doc = args.get("doc").or_else(|| args.get("params").and_then(|p| p.get("doc"))).and_then(Value::as_u64);
                if (doc.is_none() || doc == Some(open.doc_id.0))
                    && let Some(current) = target::for_doc(&self.session, open.doc_id)
                {
                    hub.invalidate(&current);
                    hub.may_edit(connection, None, &current, |_| false).map_err(|e| format!("{}: {}", e.code, e.message))?;
                }
            }
            return self.run_connected_tool(connection, name, args, root);
        };
        let resolver = self.tool_resolver.as_mut().ok_or("UNPROVEN_TARGET: this window has no mutation target resolver")?;
        let proof = resolver(&self.session, connection, name, args)?;
        let doc = DocId(proof["doc"].as_u64().ok_or("UNPROVEN_TARGET: no document identity")?);
        let before = target::for_doc(&self.session, doc).ok_or("STALE_TARGET: document closed")?;
        if proof["revision"].as_u64() != Some(before.revision) {
            return Err("STALE_TARGET: the mutation proof is out of date".into());
        }
        let agent = hub.snapshot(request).and_then(|snapshot| snapshot.agent);
        let mut selection = Value::Null;
        hub.may_edit(connection, Some(request), &before, |captured| {
            selection = captured.clone();
            target::contains(captured, &proof)
        })
        .map_err(|e| format!("{}: {}", e.code, e.message))?;
        if let Some(open) = self.ask_agent.open.as_mut().filter(|open| open.id == request)
            && agent.is_some()
        {
            open.last_agent = agent;
        }
        let depth = self.session.get(doc).map_or(0, |d| d.undo_depth());
        let mut args = args.clone();
        if proof["tool"].as_str().is_some_and(|tool| tool.starts_with("comment_")) {
            let comment_args = if name == "command_run" { args.get_mut("params") } else { Some(&mut args) };
            if let Some(object) = comment_args.and_then(Value::as_object_mut)
                && !object.contains_key("author")
                && let Some(author) = self.ask_agent.author(request)
            {
                object.insert("author".into(), json!(author));
            }
        }
        let result = self.run_connected_tool(connection, name, &args, root);
        // An error can follow successful earlier steps. Count actual history, including partial
        // failure; a successful Save that adds no step contributes zero.
        let after = target::for_doc(&self.session, doc);
        let after_depth = self.session.get(doc).map_or(0, |d| d.undo_depth());
        let created = after_depth.saturating_sub(depth);
        if let Some(open) = self.ask_agent.open.as_mut().filter(|o| o.id == request) {
            open.edits_count = open.edits_count.saturating_add(created);
            if before.revision != open.expected_generation
                || after_depth < depth
                || (after.as_ref().is_some_and(|after| after.revision != before.revision) && created == 0 && proof["tool"] != "doc_save")
            {
                open.undo_safe = false;
            }
            if let Some(after) = &after {
                open.expected_generation = after.revision;
            }
        }
        if let Some(after) = after
            && after.revision != before.revision
        {
            let next = self.tool_resolver.as_mut().ok_or("UNPROVEN_TARGET: resolver unavailable")?.as_mut()(&self.session, connection, name, &args);
            match next {
                Ok(proof) => {
                    let selection = target::after(&selection, &proof);
                    if let Err(error) = hub.advance(connection, request, &before, after.clone(), selection.clone()) {
                        if let Some(open) = self.ask_agent.open.as_mut().filter(|open| open.id == request) {
                            open.scope_valid = false;
                        }
                        hub.invalidate(&after);
                        return Err(format!("{}: {}; the tool's earlier edits remain available to review", error.code, error.message));
                    }
                    if let Some(open) = self.ask_agent.open.as_mut().filter(|o| o.id == request) {
                        open.target = selection;
                    }
                }
                Err(_) => {
                    if let Some(open) = self.ask_agent.open.as_mut().filter(|open| open.id == request) {
                        open.scope_valid = false;
                    }
                    hub.invalidate(&after);
                    return Err("STALE_TARGET: the tool edited its target but the new selection cannot be proved; review its existing edits with Keep or Undo".into());
                }
            }
        }
        result
    }
}

pub(super) fn undo_matches(session: &pdfcraft_engine::Session, open: &super::Open) -> bool {
    open.undo_safe
        && session.get(open.doc_id).is_some_and(|doc| {
            doc.edit_generation() == open.expected_generation && doc.undo_depth() == open.initial_depth.saturating_add(open.edits_count)
        })
}
