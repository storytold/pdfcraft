//! Shared command/inspection/preview entry points, using the existing confined tool path.
use serde_json::{Value, json};

use crate::{Args, Automation, Content, Result, ToolError, encode_png, failed, info, tools};
use pdfcraft_render::{RenderRequest, RequestKind};

/// The most steps one `command_batch` runs.
pub(crate) const MAX_BATCH_STEPS: usize = 1000;

impl Automation {
    pub(crate) fn command_run(&mut self, args: &Args) -> Result<Vec<Content>> {
        let id = args.str("id")?;
        let tool = tools::tool_for_command(id).ok_or_else(|| failed(format!("command {id:?} has no headless tool; see command_list")))?;
        let def = tools::find(tool).ok_or_else(|| failed("command tool is unavailable"))?;
        let mut params = match args.get("params") {
            None | Some(Value::Null) => serde_json::Map::new(),
            Some(Value::Object(params)) => params.clone(),
            Some(_) => return Err(ToolError::InvalidArgs("params must be an object".into())),
        };
        let props = def.input_schema.get("properties").and_then(Value::as_object).ok_or_else(|| failed("tool has no parameter schema"))?;
        let accepted = props.keys().map(String::as_str).collect::<Vec<_>>().join(", ");
        let mut warnings = Vec::new();
        params.retain(|key, _| {
            if props.contains_key(key) {
                return true;
            }
            warnings.push(format!("unknown param {key:?} for {id} (accepted: {accepted})"));
            false
        });
        // This preserves the task tool's validation, atomic writes and --root confinement.
        let mut content = self.call(tool, &Value::Object(params))?;
        if !warnings.is_empty() {
            if let Some(object) = content.iter_mut().find_map(|c| match c {
                Content::Json(v) => v.as_object_mut(),
                _ => None,
            }) {
                let mut all = match object.remove("warnings") {
                    Some(Value::Array(old)) => old,
                    Some(old) => vec![old],
                    None => Vec::new(),
                };
                all.extend(warnings.into_iter().map(Value::String));
                object.insert("warnings".into(), Value::Array(all));
            } else {
                content.push(Content::Json(json!({"warnings":warnings})));
            }
        }
        Ok(content)
    }

    pub(crate) fn command_batch(&mut self, args: &Args) -> Result<Value> {
        let steps = args.get("steps").and_then(Value::as_array).ok_or_else(|| ToolError::InvalidArgs("steps must be an array".into()))?;
        if steps.len() > MAX_BATCH_STEPS {
            return Err(ToolError::InvalidArgs(format!("a batch has at most {MAX_BATCH_STEPS} steps ({} given); split it", steps.len())));
        }
        // Check every envelope before applying any edit.
        for step in steps {
            let a = Args(step);
            a.str("id")?;
            if a.get("params").is_some_and(|p| !p.is_object() && !p.is_null()) {
                return Err(ToolError::InvalidArgs("step params must be an object".into()));
            }
        }
        let stop = args.opt_bool("stop_on_error")?.unwrap_or(true);
        let (mut completed, mut failed, mut results) = (0usize, 0usize, Vec::new());
        for step in steps {
            match self.command_run(&Args(step)) {
                Ok(content) => {
                    completed += 1;
                    let result = content.into_iter().find_map(|c| match c {
                        Content::Json(v) => Some(v),
                        _ => None,
                    });
                    results.push(json!({"ok":true,"result":result}));
                }
                Err(e) => {
                    failed += 1;
                    results.push(json!({"ok":false,"error":e.to_string()}));
                    if stop {
                        break;
                    }
                }
            }
        }
        Ok(json!({"completed":completed,"failed":failed,"results":results}))
    }

    pub(crate) fn doc_inspect(&self, args: &Args) -> Result<Value> {
        if args.opt_int("doc")?.is_some() {
            return Ok(info(self.doc(args)?));
        }
        Ok(json!({"documents":self.session.docs().iter().map(info).collect::<Vec<_>>()}))
    }

    pub(crate) fn render_preview(&mut self, args: &Args) -> Result<Content> {
        let id = if args.opt_int("doc")?.is_some() {
            self.doc(args)?.id
        } else {
            match self.session.docs() {
                [doc] => doc.id,
                _ => return Err(failed("give doc from doc_list (a single open document is selected automatically)")),
            }
        };
        let page = args.opt_int("page")?.unwrap_or(1);
        let max = args.opt_int("max_side")?.unwrap_or(1024);
        if !(1..=4096).contains(&max) {
            return Err(ToolError::InvalidArgs("max_side must be between 1 and 4096".into()));
        }
        let page = usize::try_from(page).ok().and_then(|p| p.checked_sub(1)).ok_or_else(|| failed("page must be at least 1"))?;
        let doc = self.session.get(id).ok_or_else(|| failed("no such document"))?;
        let size = doc.info.pages.get(page).ok_or_else(|| failed("page is out of range"))?;
        let side = size.width.max(size.height);
        if !side.is_finite() || side <= 0.0 {
            return Err(failed("page has invalid dimensions"));
        }
        let scale = (max as f32 / side).min(1.0);
        let out = self.renderer(id)?.render(RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale, tag: 0 });
        if let Some(error) = out.error {
            return Err(failed(error));
        }
        let data = encode_png(out.width, out.height, &out.rgba)?;
        Ok(Content::Png { data, width: out.width, height: out.height })
    }
}
