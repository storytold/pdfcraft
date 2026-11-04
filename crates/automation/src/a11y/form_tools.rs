use super::{Args, Automation, Result, ToolError, Value, failed, json, write_atomic};

/// A script's request to the viewer as the JSON that running a script reports; a side panel
/// goes by its fixed id.
fn request_json(r: &pdfcraft_engine::js::Request) -> Value {
    use pdfcraft_engine::js::Request as R;
    match r {
        R::Reset(n) => json!({ "reset": n }),
        R::Print => json!({ "print": true }),
        R::GoToPage(p) => json!({ "page": p + 1 }),
        R::LaunchUrl(u) => json!({ "url": u }),
        R::Submit(u) => json!({ "submit": u }),
        R::Focus(f) => json!({ "focus": f }),
        R::Beep => json!({ "beep": true }),
        R::SaveAs => json!({ "save_as": true }),
        R::ShowPanel(p) => json!({ "panel": p.id() }),
    }
}

/// A message a script showed, as `js_run` reports it: its text, title (null when it gave none),
/// icon and buttons.
fn alert_json(m: &pdfcraft_engine::js::Message) -> Value {
    json!({ "message": m.text, "title": m.title, "icon": m.icon.name(), "buttons": m.buttons.name() })
}

impl Automation {
    /// `js_run`: run `script` in the document, or as button `field`'s script, and report its
    /// messages, console lines, requests, error, the failing document scripts' errors and result.
    pub(crate) fn js_run(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        let o = self.session.run_javascript(id, a.str("script")?, a.opt_str("field")?).map_err(failed)?;
        let reqs: Vec<Value> = o.requests.iter().map(request_json).collect();
        let alerts: Vec<Value> = o.alerts.iter().map(alert_json).collect();
        Ok(json!({ "alerts": alerts, "console": o.console, "requests": reqs, "error": o.error, "document_errors": o.doc_errors, "result": o.result }))
    }

    pub(crate) fn js_document_scripts(&self, a: &Args) -> Result<Value> {
        let list: Vec<Value> = self.doc(a)?.document_scripts().into_iter().map(|(n, s)| json!({ "name": n, "script": s })).collect();
        Ok(json!({ "scripts": list }))
    }

    pub(crate) fn js_set_document_script(&mut self, a: &Args) -> Result<Value> {
        let edit = pdfcraft_engine::Edit::SetDocumentScript { name: a.str("name")?.into(), script: a.opt_str("script")?.map(str::to_string) };
        let id = self.doc(a)?.id;
        self.session.apply(id, edit).map_err(failed)?;
        self.js_document_scripts(a)
    }

    pub(crate) fn form_set_script(&mut self, a: &Args) -> Result<Value> {
        let edit = pdfcraft_engine::Edit::SetFieldScript {
            name: a.str("field")?.into(),
            event: a.str("event")?.into(),
            script: a.opt_str("script")?.map(str::to_string),
        };
        let id = self.doc(a)?.id;
        self.session.apply(id, edit).map_err(failed)?;
        let out = self.session.take_js_output(id);
        Ok(json!({ "field": a.str("field")?, "event": a.str("event")?, "console": out.console, "errors": out.errors }))
    }

    pub(crate) fn form_merge_data(&mut self, a: &Args) -> Result<Value> {
        let mut files = Vec::new();
        for p in a.strs("paths")? {
            let path = self.resolve(p, false)?;
            files.push((p.to_string(), crate::read_input_bytes(&path).map_err(|e| failed(format!("{p}: {e}")))?));
        }
        let csv = pdfcraft_engine::merge_data_files(&files).map_err(failed)?;
        let target = self.resolve(a.str("path")?, true)?;
        write_atomic(&target, csv.as_bytes())?;
        let columns = csv.lines().next().map_or(0, |h| h.split(',').count());
        Ok(json!({ "path": target.to_string_lossy(), "rows": files.len(), "columns": columns }))
    }

    pub(crate) fn form_detection_status(&self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::form_detection::{MODEL_FILE, model};
        let directory = self.form_model_directory(a, false)?;
        Ok(json!({"available": model::available(&directory).map_err(failed)?, "path": directory.join(MODEL_FILE),
            "download_url": model::DOWNLOAD_URL, "download_bytes": model::MODEL_BYTES}))
    }

    fn form_model_directory(&self, a: &Args, install: bool) -> Result<std::path::PathBuf> {
        use crate::file_access::Access;
        use pdfcraft_engine::form_detection::{MODEL_FILE, model};
        let directory = match a.opt_str("directory")? {
            Some(path) => std::path::PathBuf::from(path),
            None => model::directory().map_err(failed)?,
        };
        let shown = directory.to_string_lossy();
        let directory = self.resolve_for(&shown, Access::Read, true)?;
        if install {
            self.resolve(&shown, true)?;
        }
        // Check the nested files too: an installed file may be a link outside the read root.
        for name in [MODEL_FILE, "LICENSE-APACHE", "ATTRIBUTION.txt"] {
            let path = directory.join(name);
            self.resolve_for(&path.to_string_lossy(), Access::Read, true)?;
            if install {
                self.resolve(&path.to_string_lossy(), true)?;
            }
        }
        Ok(directory)
    }

    pub(crate) fn form_detection_install(&self, a: &Args) -> Result<Value> {
        if a.opt_bool("confirm_download")? != Some(true) {
            return Err(ToolError::InvalidArgs("confirm_download must be true to download the 121 MB form detection model from GitHub".into()));
        }
        let directory = self.form_model_directory(a, true)?;
        let path =
            pdfcraft_engine::guard(|| pdfcraft_engine::form_detection::model::install(&directory, |_, _| true)).map_err(failed)?.map_err(failed)?;
        Ok(json!({"available":true, "path":path}))
    }

    pub(crate) fn form_detect_fields(&mut self, a: &Args) -> Result<Value> {
        let pages = if a.opt_ints("pages")?.is_some() { self.pages(a, "pages")? } else { Vec::new() };
        let id = self.doc(a)?.id;
        let backend = a.opt_str("backend")?.unwrap_or("geometry");
        if !["geometry", "ffdetr"].contains(&backend) {
            return Err(ToolError::InvalidArgs("backend must be geometry or ffdetr".into()));
        }
        let model = a.opt_str("model")?.map(str::to_string);
        let model = if model.is_none() && backend == "ffdetr" {
            let directory = self.form_model_directory(a, false)?;
            if !pdfcraft_engine::form_detection::model::available(&directory).map_err(failed)? {
                return Err(failed(
                    "form detection is not installed or failed verification; use form_detection_status and form_detection_install with confirm_download:true",
                ));
            }
            Some(directory.join(pdfcraft_engine::form_detection::MODEL_FILE).to_string_lossy().into_owned())
        } else {
            model
        };
        if let Some(path) = model.as_deref() {
            let confidence = a.opt_num("confidence")?.unwrap_or(0.3);
            if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
                return Err(ToolError::InvalidArgs("confidence must be a finite number between 0 and 1".into()));
            }
            let confidence = confidence as f32;
            let add = a.opt_bool("add")?.unwrap_or(true);
            // Resolve before reading the model, including preview-only requests.
            let path = self.resolve(path, false)?;
            let detector = pdfcraft_engine::guard(|| pdfcraft_engine::form_detection::Detector::load(&path)).map_err(failed)?.map_err(failed)?;
            let job = self.session.form_detection_job(id, &pages).map_err(failed)?;
            let found = job.run(&detector, confidence, |_, _| true).map_err(failed)?;
            let list: Vec<Value> = found.fields().iter().map(|field| {
                use pdfcraft_engine::form_detection::Kind;
                let kind = match field.kind { Kind::Text => "text", Kind::CheckBox => "checkbox", Kind::Signature => "signature" };
                json!({ "page": field.page + 1, "kind": kind, "name": field.name, "rect": field.rect.map(|v| (v * 100.0).round() / 100.0), "confidence": field.confidence })
            }).collect();
            if add {
                self.session.apply_detected_fields(found).map_err(failed)?;
            }
            return Ok(json!({ "fields": list, "backend": "ffdetr" }));
        }
        if a.opt_num("confidence")?.is_some() {
            return Err(ToolError::InvalidArgs("confidence requires a local FFDetr model path".into()));
        }
        let found = self.session.detect_fields(id, &pages);
        let list: Vec<Value> = found
            .iter()
            .map(|(p, c)| {
                let (kind, comb) = match c.kind {
                    pdfcraft_engine::detect::Kind::Text => ("text", None),
                    pdfcraft_engine::detect::Kind::CheckBox => ("checkbox", None),
                    pdfcraft_engine::detect::Kind::Comb { cells } => ("text", Some(cells)),
                };
                let mut v = json!({ "page": p + 1, "kind": kind, "name": c.name, "rect": c.rect.map(|v| (v * 100.0).round() / 100.0) });
                if let Some(cells) = comb {
                    v["comb"] = json!(cells);
                }
                v
            })
            .collect();
        if a.opt_bool("add")?.unwrap_or(true) && !found.is_empty() {
            self.session.auto_detect_fields(id, &pages).map_err(failed)?;
        }
        Ok(json!({ "fields": list }))
    }

    pub(crate) fn form_actions(&self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::FieldAction as A;
        let list: Vec<Value> = self
            .doc(a)?
            .field_actions(a.str("field")?)
            .iter()
            .map(|(t, act)| {
                let mut v = match act {
                    A::JavaScript(j) => json!({ "javascript": j }),
                    A::Uri(u) => json!({ "url": u }),
                    A::Reset(f) => json!({ "reset": f }),
                    A::Named(n) => json!({ "menu": n }),
                    A::GoTo(p) => json!({ "page": p + 1 }),
                    A::ShowHide { fields, hide: true } => json!({ "hide": fields }),
                    A::ShowHide { fields, hide: false } => json!({ "show": fields }),
                    A::Submit(u) => json!({ "submit": u }),
                    A::Other(s) => json!({ "other": s }),
                };
                v["trigger"] = json!(t.id());
                v
            })
            .collect();
        Ok(json!({ "field": a.str("field")?, "actions": list }))
    }

    pub(crate) fn form_set_actions(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::{FieldAction as A, FieldTrigger as T};
        let items = a.get("actions").and_then(Value::as_array).ok_or_else(|| ToolError::InvalidArgs("actions must be an array".into()))?;
        let bad = |m: String| ToolError::InvalidArgs(m);
        let names = |v: &Value| -> Vec<String> {
            v.as_array().map(|x| x.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        let mut acts = Vec::new();
        for it in items {
            let tid = it["trigger"].as_str().unwrap_or("");
            let t = T::from_id(tid).ok_or_else(|| bad(format!("unknown trigger {tid:?}")))?;
            let act = if let Some(j) = it["javascript"].as_str() {
                A::JavaScript(j.into())
            } else if let Some(u) = it["url"].as_str() {
                A::Uri(u.into())
            } else if it.get("reset").is_some() {
                A::Reset(names(&it["reset"]))
            } else if let Some(m) = it["menu"].as_str() {
                A::Named(m.into())
            } else if let Some(p) = it["page"].as_u64() {
                A::GoTo((p.max(1) - 1) as usize)
            } else if it.get("show").is_some() {
                A::ShowHide { fields: names(&it["show"]), hide: false }
            } else if it.get("hide").is_some() {
                A::ShowHide { fields: names(&it["hide"]), hide: true }
            } else if let Some(u) = it["submit"].as_str() {
                A::Submit(u.into())
            } else {
                return Err(bad(format!("{tid}: give one action (javascript, url, reset, menu, page, show, hide or submit)")));
            };
            acts.push((t, act));
        }
        let name = a.str("field")?.to_string();
        let edit =
            pdfcraft_engine::Edit::SetFieldProps { name, props: Box::new(pdfcraft_engine::FieldProps { actions: Some(acts), ..Default::default() }) };
        let id = self.doc(a)?.id;
        self.session.apply(id, edit).map_err(failed)?;
        self.form_actions(a)
    }

    pub(crate) fn js_enabled(&mut self, a: &Args) -> Result<Value> {
        if let Some(on) = a.opt_bool("enabled")? {
            self.session.set_javascript(on);
        }
        Ok(json!({ "enabled": self.session.javascript() }))
    }
}
