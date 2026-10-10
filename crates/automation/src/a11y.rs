//! Accessibility tools: the full check, the accessibility report, and the automatic fixes.

use pdfcraft_engine::a11y::{Category, Options, Report, Rule, Status};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, child, failed, unused, write_atomic};

fn status_id(s: Status) -> &'static str {
    match s {
        Status::Passed => "passed",
        Status::Failed => "failed",
        Status::Manual => "manual",
        Status::Skipped => "skipped",
    }
}

fn category_id(c: Category) -> &'static str {
    match c {
        Category::Document => "document",
        Category::PageContent => "page_content",
        Category::Forms => "forms",
        Category::AlternateText => "alternate_text",
        Category::Tables => "tables",
        Category::Lists => "lists",
        Category::Headings => "headings",
    }
}

pub(crate) fn report_json(r: &Report) -> Value {
    let results: Vec<Value> = r
        .results
        .iter()
        .map(|x| {
            let findings: Vec<Value> = x.findings.iter().map(|f| json!({ "page": f.page.map(|p| p + 1), "message": f.message })).collect();
            json!({ "rule": x.rule.id(), "name": x.rule.name(), "category": category_id(x.rule.category()), "status": status_id(x.status), "findings": findings })
        })
        .collect();
    json!({
        "passed": r.count(Status::Passed),
        "failed": r.count(Status::Failed),
        "manual": r.count(Status::Manual),
        "skipped": r.count(Status::Skipped),
        "results": results,
    })
}

impl Automation {
    fn a11y_options(&self, a: &Args) -> Result<Options> {
        let mut o = Options::default();
        if a.opt_bool("all")?.unwrap_or(false) {
            o.rules = Rule::ALL.into_iter().collect();
        }
        if a.get("rules").is_some() {
            o.rules = a
                .strs("rules")?
                .iter()
                .map(|id| Rule::from_id(id).ok_or_else(|| ToolError::InvalidArgs(format!("unknown rule {id:?}"))))
                .collect::<Result<_>>()?;
        }
        if a.get("categories").is_some() {
            let cats: Vec<&str> = a.strs("categories")?;
            for c in &cats {
                if !Category::ALL.iter().any(|k| category_id(*k) == *c) {
                    return Err(ToolError::InvalidArgs(format!("unknown category {c:?}")));
                }
            }
            o.rules.retain(|r| cats.contains(&category_id(r.category())));
        }
        if a.opt_ints("pages")?.is_some() {
            o.pages = Some(self.pages(a, "pages")?);
        }
        Ok(o)
    }

    pub(crate) fn a11y_check(&self, a: &Args) -> Result<Value> {
        let o = self.a11y_options(a)?;
        let r = self.doc(a)?.accessibility_check(&o).ok_or_else(|| failed("the document can't be read for checking"))?;
        Ok(report_json(&r))
    }

    pub(crate) fn a11y_report(&self, a: &Args) -> Result<Value> {
        let o = self.a11y_options(a)?;
        let doc = self.doc(a)?;
        let r = doc.accessibility_check(&o).ok_or_else(|| failed("the document can't be read for checking"))?;
        let path = self.resolve(a.str("path")?, true)?;
        let (y, m, d) = self.session.today();
        let html = pdfcraft_engine::a11y::report_html(&r, &doc.name, &format!("{y}-{m:02}-{d:02}"));
        write_atomic(&path, html.as_bytes())?;
        let mut out = report_json(&r);
        out["path"] = json!(path.to_string_lossy());
        Ok(out)
    }

    pub(crate) fn a11y_fix(&mut self, a: &Args) -> Result<Value> {
        let id = a.str("rule")?;
        let rule = Rule::from_id(id).ok_or_else(|| ToolError::InvalidArgs(format!("unknown rule {id:?}")))?;
        let edit = self.doc(a)?.accessibility_fix(rule, a.opt_str("value")?).map_err(failed)?;
        let doc = self.doc(a)?.id;
        self.session.apply(doc, edit).map_err(failed)?;
        let r =
            self.doc(a)?.accessibility_check(&Options { rules: [rule].into(), pages: None }).ok_or_else(|| failed("the document can't be read"))?;
        Ok(json!({ "rule": id, "status": status_id(r.results.iter().find(|x| x.rule == rule).map_or(Status::Skipped, |x| x.status)) }))
    }

    pub(crate) fn accessibility_figures(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let list: Vec<Value> = doc
            .figures()
            .iter()
            .map(|f| {
                // Top-left-origin points on the displayed page.
                let rect = f.page.zip(f.bbox).and_then(|(p, b)| {
                    let info = doc.info.pages.get(p)?;
                    let (u, v) = (info.user_to_view(b[0] as f32, b[1] as f32), info.user_to_view(b[2] as f32, b[3] as f32));
                    let r = |x: f32| (x as f64 * 100.0).round() / 100.0;
                    Some([r(u[0].min(v[0])), r(u[1].min(v[1])), r(u[0].max(v[0])), r(u[1].max(v[1]))])
                });
                json!({ "figure": f.obj.num, "page": f.page.map(|p| p + 1), "alt": f.alt, "rect": rect })
            })
            .collect();
        Ok(json!({ "count": list.len(), "figures": list }))
    }

    pub(crate) fn accessibility_set_alt(&mut self, a: &Args) -> Result<Value> {
        let figure = u32::try_from(a.int("figure")?)
            .map_err(|_| ToolError::InvalidArgs("figure must be a figure number from accessibility_figures".into()))?;
        let edit = if a.opt_bool("decorative")?.unwrap_or(false) {
            pdfcraft_engine::Edit::MarkDecorative { figure }
        } else {
            pdfcraft_engine::Edit::SetAltText { figure, alt: a.opt_str("alt")?.map(str::to_owned) }
        };
        let id = self.doc(a)?.id;
        self.session.apply(id, edit).map_err(failed)?;
        self.accessibility_figures(a)
    }
}

impl Automation {
    fn ocr_settings(&self, a: &Args) -> Result<pdfcraft_engine::ocr::OcrSettings> {
        let mut settings = pdfcraft_engine::ocr::OcrSettings::default();
        if let Some(d) = a.opt_num("dpi")? {
            settings.dpi = d.clamp(72.0, 600.0) as f32;
        }
        if let Some(l) = a.opt_str("language")? {
            if !pdfcraft_engine::ocr::LANGUAGES.iter().any(|x| x.0 == l) {
                return Err(ToolError::InvalidArgs(format!("unsupported language {l:?}")));
            }
            settings.language = l.into();
        }
        if let Some(s) = a.opt_bool("skip_text_pages")? {
            settings.skip_text_pages = s;
        }
        Ok(settings)
    }

    pub(crate) fn ocr_recognize_files(&mut self, a: &Args) -> Result<Value> {
        let settings = self.ocr_settings(a)?;
        let folder = self.resolve(a.str("folder")?, true)?;
        // A path that leaves the root refuses the whole call before any work starts, like the
        // other tools (the write check passes missing files, which stay per-file errors below).
        let paths = a.strs("paths")?;
        for p in &paths {
            self.resolve(p, true)?;
        }
        std::fs::create_dir_all(&folder).map_err(|e| failed(e.to_string()))?;
        let ocr = pdfcraft_engine::ocr::engine().map_err(failed)?;
        let mut out = Vec::new();
        for p in paths {
            let name = std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "document.pdf".into());
            let result =
                self.resolve(p, false).map_err(|e| e.to_string()).and_then(|src| std::fs::read(&src).map_err(|e| e.to_string())).and_then(|bytes| {
                    pdfcraft_engine::ocr::recognize_file(&name, std::sync::Arc::new(bytes), None, settings.clone(), &ocr, |_, _| true)
                });
            out.push(match result {
                Ok(r) => {
                    let target = child(&folder, &name);
                    write_atomic(&target, &r.bytes)?;
                    let skipped: Vec<usize> = r.pages.iter().filter(|p| p.skipped.is_some()).map(|p| p.page + 1).collect();
                    json!({ "path": p, "output": target.to_string_lossy(), "words": r.words(), "skipped_pages": skipped })
                }
                Err(e) => json!({ "path": p, "error": e }),
            });
        }
        Ok(json!({ "files": out }))
    }

    pub(crate) fn ocr_recognize(&mut self, a: &Args) -> Result<Value> {
        let settings = self.ocr_settings(a)?;
        let pages = if a.opt_ints("pages")?.is_some() { self.pages(a, "pages")? } else { Vec::new() };
        let id = self.doc(a)?.id;
        let found = self.session.recognize_text(id, &pages, settings).map_err(failed)?;
        let list: Vec<Value> = found
            .iter()
            .map(|p| match &p.skipped {
                Some(why) => json!({ "page": p.page + 1, "skipped": why }),
                None => json!({ "page": p.page + 1, "words": p.words.len(), "text": p.text() }),
            })
            .collect();
        Ok(json!({ "words": found.iter().map(|p| p.words.len()).sum::<usize>(), "pages": list }))
    }

    pub(crate) fn ocr_status(&self) -> Result<Value> {
        use pdfcraft_engine::ocr;
        let dirs: Vec<String> = ocr::Models::search_dirs().iter().map(|d| d.to_string_lossy().into_owned()).collect();
        let langs: Vec<Value> = ocr::LANGUAGES.iter().map(|(c, n)| json!({ "code": c, "name": n })).collect();
        Ok(json!({ "available": ocr::available(), "search_dirs": dirs, "languages": langs }))
    }
}

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
    }
}

impl Automation {
    pub(crate) fn js_run(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        let o = self.session.run_javascript(id, a.str("script")?, a.opt_str("field")?).map_err(failed)?;
        let reqs: Vec<Value> = o.requests.iter().map(request_json).collect();
        Ok(json!({ "alerts": o.alerts, "console": o.console, "requests": reqs, "error": o.error, "result": o.result }))
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
            files.push((p.to_string(), std::fs::read(&path).map_err(|e| failed(format!("{p}: {e}")))?));
        }
        let csv = pdfcraft_engine::merge_data_files(&files).map_err(failed)?;
        let target = self.resolve(a.str("path")?, true)?;
        write_atomic(&target, csv.as_bytes())?;
        let columns = csv.lines().next().map_or(0, |h| h.split(',').count());
        Ok(json!({ "path": target.to_string_lossy(), "rows": files.len(), "columns": columns }))
    }

    pub(crate) fn doc_export_office(&self, a: &Args) -> Result<Value> {
        let path = self.resolve(a.str("path")?, true)?;
        let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let format = pdfcraft_engine::compare::OfficeFormat::from_extension(&ext)
            .ok_or_else(|| ToolError::InvalidArgs(format!("unsupported extension {ext:?} (docx, html or rtf)")))?;
        let bytes = self.doc(a)?.export_office(format);
        write_atomic(&path, &bytes)?;
        Ok(json!({ "path": path.to_string_lossy(), "bytes": bytes.len(), "format": format.extension() }))
    }

    fn pdfa_level(&self, a: &Args) -> Result<pdfcraft_engine::pdfa::Level> {
        let l = a.opt_str("level")?.unwrap_or("2b");
        pdfcraft_engine::pdfa::Level::from_id(l).ok_or_else(|| ToolError::InvalidArgs(format!("unknown PDF/A level {l:?} (2b or 3b)")))
    }

    pub(crate) fn pdfa_verify(&self, a: &Args) -> Result<Value> {
        let level = self.pdfa_level(a)?;
        let doc = self.doc(a)?;
        let issues: Vec<Value> = doc
            .pdfa_verify(level)
            .iter()
            .map(|i| json!({ "clause": i.clause, "message": i.message, "page": i.page.map(|p| p + 1), "fixable": i.fixable }))
            .collect();
        let d = doc.standards();
        Ok(json!({
            "level": level.label(),
            "compliant": issues.is_empty(),
            "issues": issues,
            "declared": { "pdfa": d.pdfa.map(|(p, c)| format!("PDF/A-{p}{}", c.to_lowercase())), "pdfua": d.pdfua, "output_intents": d.output_intents },
        }))
    }

    pub(crate) fn pdfa_convert(&mut self, a: &Args) -> Result<Value> {
        let level = self.pdfa_level(a)?;
        let id = self.doc(a)?.id;
        self.session.apply(id, pdfcraft_engine::Edit::ConvertPdfA { level }).map_err(failed)?;
        self.pdfa_verify(a)
    }

    pub(crate) fn action_list(&self) -> Result<Value> {
        use pdfcraft_engine::actions::{Step, builtin};
        let step_json = |s: &Step| json!({ "step": s.id(), "arg": s.arg() });
        let actions: Vec<Value> = builtin()
            .iter()
            .map(|a| json!({ "name": a.name, "description": a.description, "steps": a.steps.iter().map(step_json).collect::<Vec<_>>() }))
            .collect();
        let steps: Vec<Value> = Step::all().iter().map(|s| json!({ "step": s.id(), "label": s.label(), "takes_arg": s.arg().is_some() })).collect();
        Ok(json!({ "actions": actions, "steps": steps }))
    }

    pub(crate) fn action_run(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::actions::{Action, Step, builtin, run_on};
        let action = if let Some(name) = a.opt_str("action")? {
            builtin()
                .into_iter()
                .find(|x| x.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| ToolError::InvalidArgs(format!("no built-in action {name:?}")))?
        } else {
            let items = a.get("steps").and_then(Value::as_array).ok_or_else(|| ToolError::InvalidArgs("give action or steps".into()))?;
            let steps = items
                .iter()
                .map(|it| {
                    let id = it["step"].as_str().unwrap_or("");
                    Step::from_id(id, it["arg"].as_str().unwrap_or("")).ok_or_else(|| ToolError::InvalidArgs(format!("unknown step {id:?}")))
                })
                .collect::<Result<Vec<_>>>()?;
            Action { name: "Custom".into(), description: String::new(), steps, builtin: false }
        };
        let folder = self.resolve(a.str("folder")?, true)?;
        std::fs::create_dir_all(&folder).map_err(|e| failed(e.to_string()))?;
        let mut out = Vec::new();
        for p in a.strs("paths")? {
            let name = std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "document.pdf".into());
            let result = self
                .resolve(p, false)
                .map_err(|e| e.to_string())
                .and_then(|src| std::fs::read(&src).map_err(|e| e.to_string()))
                .and_then(|bytes| run_on(&action, &name, std::sync::Arc::new(bytes), |_, _| {}));
            out.push(match result {
                Ok(r) => {
                    let target = child(&folder, &name);
                    let target = if target.exists() {
                        let stem =
                            std::path::Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
                        unused(&folder, &stem)
                    } else {
                        target
                    };
                    if target.exists() {
                        return Err(failed(format!("{}: no unused output name available", target.display())));
                    }
                    write_atomic(&target, &r.bytes)?;
                    json!({ "path": p, "output": target.to_string_lossy(), "log": r.log })
                }
                Err(e) => json!({ "path": p, "error": e }),
            });
        }
        Ok(json!({ "action": action.name, "files": out }))
    }

    fn compare_ids(&self, a: &Args) -> Result<(pdfcraft_engine::DocId, pdfcraft_engine::DocId)> {
        let new = self.doc(a)?.id;
        let other = a.int("other")?;
        let old = u64::try_from(other)
            .ok()
            .and_then(|o| self.session.get(pdfcraft_engine::DocId(o)))
            .map(|d| d.id)
            .ok_or_else(|| ToolError::InvalidArgs(format!("no open document {other}")))?;
        Ok((old, new))
    }

    pub(crate) fn doc_compare(&self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::compare::Kind;
        let (old, new) = self.compare_ids(a)?;
        let c = self.session.compare(old, new).map_err(failed)?;
        let limit = a.opt_int("limit")?.unwrap_or(500).max(1) as usize;
        let r2 = |r: &[f64; 4]| r.map(|v| (v * 100.0).round() / 100.0);
        let side =
            |s: &pdfcraft_engine::compare::Side| json!({ "text": s.text, "page": s.page + 1, "rects": s.rects.iter().map(r2).collect::<Vec<_>>() });
        let list: Vec<Value> = c
            .changes
            .iter()
            .take(limit)
            .map(|ch| json!({ "kind": ch.kind.label().to_lowercase(), "old": side(&ch.old), "new": side(&ch.new) }))
            .collect();
        let visual: Vec<Value> = if a.opt_bool("visual")?.unwrap_or(false) {
            self.session.compare_visual(old, new, 72.0).map_err(failed)?.iter().map(|(p, r)| json!({ "page": p + 1, "rect": r2(r) })).collect()
        } else {
            Vec::new()
        };
        Ok(json!({
            "visual": visual,
            "identical": c.identical(),
            "replaced": c.count(Kind::Replaced),
            "inserted": c.count(Kind::Inserted),
            "deleted": c.count(Kind::Deleted),
            "old_words": c.old_words,
            "new_words": c.new_words,
            "changes": list,
        }))
    }

    pub(crate) fn doc_compare_report(&self, a: &Args) -> Result<Value> {
        let (old, new) = self.compare_ids(a)?;
        let bytes = self.session.compare_report(old, new).map_err(failed)?;
        let path = self.resolve(a.str("path")?, true)?;
        write_atomic(&path, &bytes)?;
        Ok(json!({ "path": path.to_string_lossy(), "bytes": bytes.len() }))
    }

    pub(crate) fn doc_compare_mark(&mut self, a: &Args) -> Result<Value> {
        let (old, new) = self.compare_ids(a)?;
        let n = self.session.mark_differences(old, new).map_err(failed)?;
        Ok(json!({ "comments": n }))
    }

    pub(crate) fn form_detect_fields(&mut self, a: &Args) -> Result<Value> {
        let pages = if a.opt_ints("pages")?.is_some() { self.pages(a, "pages")? } else { Vec::new() };
        let id = self.doc(a)?.id;
        let found = self.session.detect_fields(id, &pages);
        let list: Vec<Value> = found
            .iter()
            .map(|(p, c)| {
                let kind = match c.kind {
                    pdfcraft_engine::detect::Kind::Text => "text",
                    pdfcraft_engine::detect::Kind::CheckBox => "checkbox",
                };
                json!({ "page": p + 1, "kind": kind, "name": c.name, "rect": c.rect.map(|v| (v * 100.0).round() / 100.0) })
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
