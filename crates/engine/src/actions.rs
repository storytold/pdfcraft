//! Action Wizard: named sequences of steps run over files (Acrobat's guided actions), the
//! built-in actions, and running an action over many files (batch processing).

use std::sync::Arc;

use crate::{DocId, Edit, EditError, Session};

/// One step of an action.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Scan & OCR ▸ Recognize text (pages without text).
    RecognizeText,
    /// Reduce file size.
    ReduceFileSize,
    /// Remove hidden information (every category).
    RemoveHiddenInformation,
    /// Sanitize document.
    Sanitize,
    FlattenComments,
    FlattenFields,
    /// Add a text watermark (diagonal, 50% opaque).
    AddWatermark(String),
    /// Add a footer with this centre text (tokens such as `<<Page 1 of n>>` work).
    AddFooter(String),
    /// Add a header with this centre text.
    AddHeader(String),
    /// Set the document title.
    SetTitle(String),
    /// Prepare a form: detect fields.
    DetectFormFields,
    /// Run a JavaScript (console context).
    RunJavaScript(String),
}

impl Step {
    /// Every step, with an empty argument where it takes one (for the step picker).
    pub fn all() -> Vec<Step> {
        vec![
            Step::RecognizeText,
            Step::ReduceFileSize,
            Step::RemoveHiddenInformation,
            Step::Sanitize,
            Step::FlattenComments,
            Step::FlattenFields,
            Step::AddWatermark(String::new()),
            Step::AddHeader(String::new()),
            Step::AddFooter(String::new()),
            Step::SetTitle(String::new()),
            Step::DetectFormFields,
            Step::RunJavaScript(String::new()),
        ]
    }

    /// A stable id (agents, saved actions).
    pub fn id(&self) -> &'static str {
        match self {
            Step::RecognizeText => "recognize_text",
            Step::ReduceFileSize => "reduce_file_size",
            Step::RemoveHiddenInformation => "remove_hidden_information",
            Step::Sanitize => "sanitize",
            Step::FlattenComments => "flatten_comments",
            Step::FlattenFields => "flatten_fields",
            Step::AddWatermark(_) => "add_watermark",
            Step::AddHeader(_) => "add_header",
            Step::AddFooter(_) => "add_footer",
            Step::SetTitle(_) => "set_title",
            Step::DetectFormFields => "detect_form_fields",
            Step::RunJavaScript(_) => "run_javascript",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Step::RecognizeText => "Recognize text",
            Step::ReduceFileSize => "Reduce file size",
            Step::RemoveHiddenInformation => "Remove hidden information",
            Step::Sanitize => "Sanitize document",
            Step::FlattenComments => "Flatten comments",
            Step::FlattenFields => "Flatten form fields",
            Step::AddWatermark(_) => "Add watermark",
            Step::AddHeader(_) => "Add header",
            Step::AddFooter(_) => "Add footer",
            Step::SetTitle(_) => "Set document title",
            Step::DetectFormFields => "Detect form fields",
            Step::RunJavaScript(_) => "Run a JavaScript",
        }
    }

    /// The step's text argument, if it takes one.
    pub fn arg(&self) -> Option<&str> {
        match self {
            Step::AddWatermark(s) | Step::AddHeader(s) | Step::AddFooter(s) | Step::SetTitle(s) | Step::RunJavaScript(s) => Some(s),
            _ => None,
        }
    }

    pub fn arg_mut(&mut self) -> Option<&mut String> {
        match self {
            Step::AddWatermark(s) | Step::AddHeader(s) | Step::AddFooter(s) | Step::SetTitle(s) | Step::RunJavaScript(s) => Some(s),
            _ => None,
        }
    }

    /// Build a step from its id and argument.
    pub fn from_id(id: &str, arg: &str) -> Option<Step> {
        let mut s = Step::all().into_iter().find(|s| s.id() == id)?;
        if let Some(a) = s.arg_mut() {
            *a = arg.to_string();
        }
        Some(s)
    }
}

/// A named action.
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub name: String,
    pub description: String,
    pub steps: Vec<Step>,
    /// One of PdfCraft's own actions (can't be edited or deleted).
    pub builtin: bool,
}

/// The built-in actions.
pub fn builtin() -> Vec<Action> {
    let a = |name: &str, description: &str, steps: Vec<Step>| Action { name: name.into(), description: description.into(), steps, builtin: true };
    vec![
        a(
            "Prepare for Distribution",
            "Remove hidden information and flatten comments and form fields, so recipients see only the pages.",
            vec![Step::RemoveHiddenInformation, Step::FlattenComments, Step::FlattenFields],
        ),
        a("Optimize Scanned Documents", "Make scans searchable, then make them smaller.", vec![Step::RecognizeText, Step::ReduceFileSize]),
        a("Add Page Numbers", "Number every page in the footer.", vec![Step::AddFooter("Page <<1>> of <<n>>".into())]),
        a("Mark as Confidential", "A diagonal CONFIDENTIAL watermark on every page.", vec![Step::AddWatermark("CONFIDENTIAL".into())]),
        a("Sanitize Documents", "Remove hidden information, metadata, scripts and embedded content, and rewrite the files.", vec![Step::Sanitize]),
    ]
}

/// What running an action on one file did.
#[derive(Clone, Debug, PartialEq)]
pub struct FileResult {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    /// One line per step.
    pub log: Vec<String>,
}

fn apply_step(s: &mut Session, id: DocId, step: &Step, log: &mut Vec<String>) -> Result<Option<DocId>, EditError> {
    let pages = s.get(id).map_or(0, |d| d.info.pages.len());
    let all: Vec<usize> = (0..pages).collect();
    let edit = match step {
        Step::RecognizeText => {
            let found = s.recognize_text(id, &[], crate::ocr::OcrSettings::default()).map_err(EditError::Invalid)?;
            log.push(format!("Recognize text: {} words", found.iter().map(|p| p.words.len()).sum::<usize>()));
            return Ok(None);
        }
        Step::ReduceFileSize => {
            let (bytes, _) = s.optimized_bytes(id, &crate::optimize::Settings::default(), &[])?;
            let before = s.get(id).map_or(0, |d| d.bytes.len());
            log.push(format!("Reduce file size: {} → {} bytes", before, bytes.len()));
            let name = s.get(id).map(|d| d.name.clone()).unwrap_or_default();
            s.close(id);
            let new = s.open(name, None, bytes, None).map_err(|e| EditError::Reopen(e.to_string()))?;
            return Ok(Some(new));
        }
        Step::RemoveHiddenInformation => Edit::RemoveHidden { which: crate::HIDDEN.to_vec() },
        Step::Sanitize => Edit::Sanitize,
        Step::FlattenComments => Edit::Flatten { comments: true, fields: false },
        Step::FlattenFields => Edit::Flatten { comments: false, fields: true },
        Step::AddWatermark(t) => {
            Edit::AddWatermark { pages: all, settings: crate::Watermark { text: t.clone(), ..Default::default() }, replace: false, file: None }
        }
        Step::AddHeader(t) | Step::AddFooter(t) => {
            let mut settings = crate::HeaderFooter::default();
            settings.text[if matches!(step, Step::AddHeader(_)) { 1 } else { 4 }] = t.clone();
            Edit::AddHeaderFooter { pages: all, settings, replace: false }
        }
        Step::SetTitle(t) => Edit::SetInfo { key: "Title".into(), value: t.clone() },
        Step::DetectFormFields => {
            let n = s.auto_detect_fields(id, &[])?.len();
            log.push(format!("Detect form fields: {n}"));
            return Ok(None);
        }
        Step::RunJavaScript(js) => {
            let o = s.run_javascript(id, js, None)?;
            log.push(match o.error {
                Some(e) => format!("Run a JavaScript: {e}"),
                None => "Run a JavaScript".into(),
            });
            return Ok(None);
        }
    };
    // Steps that find nothing to do (no comments to flatten, …) aren't failures.
    match s.apply(id, edit) {
        Ok(()) => log.push(step.label().to_string()),
        Err(e) => log.push(format!("{}: {e}", step.label())),
    }
    Ok(None)
}

/// Run `action` on one file. `progress(step, steps)` is called before each step.
pub fn run_on(action: &Action, name: &str, bytes: Arc<Vec<u8>>, mut progress: impl FnMut(usize, usize)) -> Result<FileResult, String> {
    let mut s = Session::new();
    let mut id = s.open(name, None, bytes, None).map_err(|e| e.to_string())?;
    if let Some(why) = s.get(id).and_then(|d| d.read_only_reason.clone()) {
        return Err(why);
    }
    let mut log = Vec::new();
    for (i, step) in action.steps.iter().enumerate() {
        progress(i, action.steps.len());
        if let Some(new) = apply_step(&mut s, id, step, &mut log).map_err(|e| format!("{}: {e}", step.label()))? {
            id = new;
        }
    }
    let d = s.get(id).ok_or("the document was lost")?;
    // A signed file keeps its signatures: incremental. Otherwise a clean full rewrite.
    let bytes = if d.is_signed() { s.save_bytes(id) } else { s.save_full_bytes(id) }.map_err(|e| e.to_string())?;
    Ok(FileResult { name: name.to_string(), bytes, log })
}
