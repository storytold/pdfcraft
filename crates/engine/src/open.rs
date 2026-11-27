//! Opening a document: preparing it away from the session, then registering it.

use std::sync::{Arc, OnceLock};

use pdfcraft_cos::{SaveOptions, write_incremental};
use pdfcraft_render::{DocInfo, OpenError, RenderConfig, RenderPool, inspect};

use crate::history::{AsOpened, Editor, Keys, TextBlocksCache};
use crate::{
    AppearanceCache, DocId, Document, Session, StatusCaptions, TrustStore, XFA_SCRIPTS_OFF, XfaLayout, display_bytes, display_config,
    editor_signatures, guard, info_fields, js, note_warnings, print_preview_layers, recovered_render_password, render_threads, same_pages, xfa,
    xfa_layout, xfa_values_from_datasets,
};

/// Immutable session state needed while a document is prepared away from its owning thread.
#[derive(Clone)]
pub struct OpenContext {
    now: Option<i64>,
    js_off: bool,
    trust: Arc<TrustStore>,
    highlight: bool,
    captions: Arc<StatusCaptions>,
    xfa_time_scale: u32,
}

/// A fully prepared document that has not yet been registered in a session.
pub struct PreparedDocument {
    document: Document,
}

struct DocumentOpenData<'a> {
    name: String,
    path: Option<String>,
    bytes: Arc<Vec<u8>>,
    info: DocInfo,
    structure: Result<Result<pdfcraft_cos::Document, pdfcraft_cos::CosError>, Box<dyn std::any::Any + Send>>,
    render_password: Option<String>,
    password: Option<&'a str>,
    xfa: Option<XfaLayout>,
    /// The file as read, when opening appended revisions to it ([`crate::history::AsOpened`]).
    as_opened: Option<AsOpened>,
}

impl OpenContext {
    /// This context with Highlight fields `on`, which the document's scripts read as
    /// `app.runtimeHighlight` while it opens. A new document opens with the remembered setting
    /// while the session keeps the one of the document in front, which its scripts read.
    pub fn with_highlight(mut self, on: bool) -> Self {
        self.highlight = on;
        self
    }

    /// Prepare a document without mutating the session that supplied this context.
    pub fn prepare_open(
        &self,
        name: impl Into<String>,
        path: Option<String>,
        bytes: Arc<Vec<u8>>,
        password: Option<&str>,
    ) -> Result<PreparedDocument, OpenError> {
        let name = name.into();
        guard(|| Session::prepare_open_unguarded(self, name, path, bytes, password))
            .unwrap_or_else(|m| Err(OpenError::Invalid(format!("reading it failed unexpectedly ({m})"))))
    }
}

impl Session {
    /// Snapshot the session state that affects open-time scripts, dates and signature trust.
    pub fn open_context(&self) -> OpenContext {
        OpenContext {
            now: self.now(),
            js_off: self.js_off,
            trust: self.trust.clone(),
            highlight: self.field_highlight,
            captions: self.captions.clone(),
            xfa_time_scale: self.xfa_time_scale.unwrap_or(1),
        }
    }

    /// Prepare a document without adding it to this session.
    pub fn prepare_open(
        &self,
        name: impl Into<String>,
        path: Option<String>,
        bytes: Arc<Vec<u8>>,
        password: Option<&str>,
    ) -> Result<PreparedDocument, OpenError> {
        self.open_context().prepare_open(name, path, bytes, password)
    }

    /// Register a prepared document. This is the only open step that mutates the session.
    pub fn commit_open(&mut self, prepared: PreparedDocument) -> DocId {
        self.next_id += 1;
        let id = DocId(self.next_id);
        let mut document = prepared.document;
        document.id = id;
        // Trust or the interface language may have changed while it was prepared on another
        // thread.
        if !Arc::ptr_eq(&document.trust, &self.trust) {
            document.captions = self.captions.clone();
            Self::revalidate(&mut document, self.trust.clone());
        } else if *document.captions != *self.captions {
            Self::draw_captions(&mut document, self.captions.clone());
        }
        self.docs.push(document);
        self.run_open_scripts(id);
        id
    }

    /// Synchronous convenience API used by the CLI and automation.
    pub fn open(&mut self, name: impl Into<String>, path: Option<String>, bytes: Arc<Vec<u8>>, password: Option<&str>) -> Result<DocId, OpenError> {
        let prepared = self.prepare_open(name, path, bytes, password)?;
        Ok(self.commit_open(prepared))
    }

    /// What [`OpenContext::prepare_open`] runs inside its panic guard: read, decrypt and lay out
    /// the document, and run an XFA form's opening scripts.
    fn prepare_open_unguarded(
        context: &OpenContext,
        name: String,
        path: Option<String>,
        bytes: Arc<Vec<u8>>,
        password: Option<&str>,
    ) -> Result<PreparedDocument, OpenError> {
        let cos = std::panic::catch_unwind(|| pdfcraft_cos::Document::open_with_password(bytes.clone(), password));
        // The renderer authenticates on its own. It cannot use the owner password of R2–R4
        // files, so give it the user password that owner authentication recovers.
        let (info, render_password) = match inspect(bytes.clone(), password) {
            Ok(info) => (info, password.map(str::to_owned)),
            Err(OpenError::WrongPassword) => {
                let user = match &cos {
                    Ok(Ok(d)) => recovered_render_password(d),
                    _ => None,
                };
                let user = user.ok_or(OpenError::WrongPassword)?;
                (inspect(bytes.clone(), Some(&user))?, Some(user))
            }
            Err(e) => return Err(e),
        };
        // A dynamic XFA form is a shell around an XML template; lay the template out into real
        // pages and fields so the rest of the engine works on it. The original bytes stay: the
        // laid-out form is one appended revision.
        let opts = SaveOptions { mod_date: context.now.map(pdfcraft_cos::pdf_date), ..SaveOptions::default() };
        // Write `work` as one more revision and reopen it: what the document then is. The notes
        // about damage repaired while reading the file stay with it (Document Properties), once.
        let rebase = |work: &pdfcraft_cos::Document| -> Result<(Arc<Vec<u8>>, DocInfo, pdfcraft_cos::Document), String> {
            let new_bytes = write_incremental(work, &opts).map(Arc::new).map_err(|e| e.to_string())?;
            let new_info = inspect(new_bytes.clone(), render_password.as_deref()).map_err(|e| e.to_string())?;
            let mut new_cos = pdfcraft_cos::Document::open_with_password(new_bytes.clone(), password).map_err(|e| e.to_string())?;
            for line in work.repair_log() {
                if !new_cos.repair_log().contains(line) {
                    new_cos.note_repair(line.clone());
                }
            }
            Ok((new_bytes, new_info, new_cos))
        };
        // The file as read, for its signatures: opening an XFA form may append revisions below.
        let as_read = match (&info.xfa, &cos) {
            (Some(_), Ok(Ok(c))) => Some(AsOpened { cos: c.clone(), bytes: bytes.clone() }),
            _ => None,
        };
        let (bytes, info, cos, xfa) = match (info.xfa, cos) {
            (Some(pdfcraft_render::Xfa::Dynamic), Ok(Ok(cos))) if pdfcraft_xfa::existing_layout(&cos).is_none() => {
                let mut work = cos.clone();
                let laid_out = guard(|| xfa_layout(&mut work))
                    .unwrap_or_else(|m| Err(format!("laying it out failed unexpectedly ({m})")))
                    .and_then(|report| rebase(&work).map(|(b, i, c)| (b, i, c, report)));
                match laid_out {
                    Ok((b, i, c, report)) => (b, i, Ok(Ok(c)), Some(report)),
                    Err(e) => {
                        let mut info = info;
                        info.warnings.push(format!("This dynamic XFA form could not be laid out: {e}"));
                        (bytes, info, Ok(Ok(cos)), None)
                    }
                }
            }
            // A static XFA form, or a dynamic one laid out earlier: its datasets may hold values
            // filled in by another viewer since; give the fields those values.
            (Some(_), Ok(Ok(cos))) => {
                let report = pdfcraft_xfa::existing_layout(&cos);
                let mut work = cos.clone();
                let synced =
                    guard(|| xfa_values_from_datasets(&mut work)).unwrap_or_else(|m| Err(format!("reading its data failed unexpectedly ({m})")));
                match synced {
                    Ok(names) if names.is_empty() => (bytes, info, Ok(Ok(cos)), report),
                    Ok(names) => match rebase(&work) {
                        Ok((b, mut i, c)) => {
                            // The fields were rewritten from the XFA data: say so, and which.
                            let shown: Vec<&str> = names.iter().take(5).map(String::as_str).collect();
                            let more = if names.len() > shown.len() { format!(" and {} more", names.len() - shown.len()) } else { String::new() };
                            i.warnings.push(format!(
                                "{} form field value{} were taken from this form's XFA data (filled in by another viewer): {}{more}",
                                names.len(),
                                if names.len() == 1 { "" } else { "s" },
                                shown.join(", ")
                            ));
                            (b, i, Ok(Ok(c)), report)
                        }
                        Err(e) => {
                            let mut info = info;
                            info.warnings.push(format!("The values in this form's XFA data could not be applied: {e}"));
                            (bytes, info, Ok(Ok(cos)), report)
                        }
                    },
                    Err(e) => {
                        let mut info = info;
                        info.warnings.push(format!("The values in this form's XFA data could not be applied: {e}"));
                        (bytes, info, Ok(Ok(cos)), report)
                    }
                }
            }
            (_, cos) => (bytes, info, cos, None),
        };
        // A laid-out XFA form: its initialize and calculate scripts run now, as on opening in
        // Acrobat; what they change is one more revision, still nothing to save.
        let mut script_output = js::JsOutput::default();
        let mut ran_away = false;
        let xfa_template = match (&xfa, &cos) {
            (Some(_), Ok(Ok(c))) => xfa::template(c),
            _ => None,
        };
        let (bytes, info, cos) = match (&xfa, cos) {
            (Some(_), Ok(Ok(cos))) if !context.js_off => match xfa_template.clone() {
                Some(tpl) => {
                    let mut work = cos.clone();
                    let pages = info.pages.len();
                    let ran = guard(|| {
                        let mut run = xfa::XfaRun {
                            page_count: pages,
                            out: &mut script_output,
                            datasets: None,
                            ran_away: false,
                            highlight: context.highlight,
                            time_scale: context.xfa_time_scale,
                            relaid: false,
                        };
                        let changes = xfa::on_open(&mut work, &tpl, &mut run);
                        if run.ran_away {
                            ran_away = true;
                        }
                        changes
                    })
                    .unwrap_or_else(|m| Err(format!("its scripts failed unexpectedly ({m})")));
                    match ran {
                        Ok(changes) if work.is_modified() => match rebase(&work) {
                            Ok((b, mut i, c)) => {
                                // The file as saved differs from what opening shows: say so.
                                if !changes.is_empty() {
                                    i.warnings.push(format!(
                                        "This form's scripts changed it on opening ({}); saving keeps those changes",
                                        changes.describe()
                                    ));
                                }
                                (b, i, Ok(Ok(c)))
                            }
                            Err(e) => {
                                script_output.errors.push(format!("The form's scripts could not be applied: {e}"));
                                (bytes, info, Ok(Ok(cos)))
                            }
                        },
                        Ok(_) => (bytes, info, Ok(Ok(cos))),
                        Err(e) => {
                            script_output.errors.push(format!("The form's scripts could not run: {e}"));
                            (bytes, info, Ok(Ok(cos)))
                        }
                    }
                }
                None => (bytes, info, Ok(Ok(cos))),
            },
            (_, cos) => (bytes, info, cos),
        };
        // The document model's page tree leads to fewer pages than the reader shows: line it up
        // with them as one more revision ([`repair_page_tree`]), or edits and saves address only
        // the pages the model reaches.
        let (bytes, info, cos) = match cos {
            Ok(Ok(cos)) if !info.pages.is_empty() => match repair_page_tree(context, &cos, &bytes, &info, render_password.as_deref(), &rebase) {
                // The warnings opening noted so far (an XFA form's scripts changing it, say) stay,
                // beside those of the repaired file's inspection.
                Ok((b, mut i, c)) => {
                    for w in info.warnings {
                        if !i.warnings.contains(&w) {
                            i.warnings.push(w);
                        }
                    }
                    (b, i, Ok(Ok(c)))
                }
                Err(refused) => {
                    let mut info = info;
                    info.warnings.extend(refused);
                    (bytes, info, Ok(Ok(cos)))
                }
            },
            cos => (bytes, info, cos),
        };
        // XFA notes survive the document being re-read after edits.
        let xfa_warnings: Vec<String> = if info.xfa.is_some() { info.warnings.clone() } else { Vec::new() };
        let as_opened = as_read.filter(|o| !Arc::ptr_eq(&o.bytes, &bytes));
        let mut document =
            Self::build_document(context, DocumentOpenData { name, path, bytes, info, structure: cos, render_password, password, xfa, as_opened });
        // A script that ran away on open turns the form's scripts off for this document.
        document.xfa_template = if ran_away { None } else { xfa_template };
        document.js_output.append(script_output);
        if ran_away {
            document.xfa_scripts_off = true;
            document.js_output.errors.push(XFA_SCRIPTS_OFF.into());
        }
        note_warnings(&mut document.xfa_warnings, &xfa_warnings);
        Ok(PreparedDocument { document })
    }

    /// The last step of opening: build the document record and register it.
    fn build_document(context: &OpenContext, data: DocumentOpenData<'_>) -> Document {
        let DocumentOpenData { name, path, bytes, mut info, structure: cos, render_password, password, xfa, as_opened } = data;
        let mut config = RenderConfig { password: render_password.as_deref().map(Arc::from), ..Default::default() };
        let (mut editor, read_only_reason) = match cos {
            Ok(Ok(cos)) => {
                let keys = Keys { render: render_password.clone(), reopen: password.map(str::to_owned) };
                let saved = cos.bytes().clone();
                (
                    Some(Editor {
                        cos,
                        saved,
                        saved_keys: keys.clone(),
                        unsaved_file: false,
                        undo: Vec::new(),
                        redo: Vec::new(),
                        keys,
                        appearances: AppearanceCache::default(),
                        field_appearances: pdfcraft_forms::FieldAppearanceCache::default(),
                        text_blocks: TextBlocksCache::default(),
                        as_opened,
                    }),
                    None,
                )
            }
            Ok(Err(e)) => (None, Some(e.to_string())),
            Err(_) => (None, Some("the document structure could not be read for editing".into())),
        };
        let sig_cache = Arc::new(pdfcraft_sign::DigestCache::default());
        let mut signatures = editor.as_ref().map(|e| editor_signatures(e, &bytes, &context.trust, &sig_cache)).unwrap_or_default();
        // Only a signed file needs keeping as it was read.
        if !signatures.iter().any(|s| s.signed)
            && let Some(e) = editor.as_mut()
        {
            e.as_opened = None;
        }
        // The inspection reads only `/Info`; a file stating its title or author only in XMP still
        // shows them (#2373).
        if let Some(e) = &editor {
            for (field, value) in info_fields(&mut info, &e.cos) {
                if field.is_none() {
                    *field = value;
                }
            }
        }
        // The print preview draws the layers as they print.
        config.print_layers = print_preview_layers(editor.as_ref().map(|e| &e.cos), &info.layers);
        let display = display_bytes(editor.as_mut(), &bytes, &mut signatures, &context.captions);
        let renderer = RenderPool::new(display.clone(), render_threads(), display_config(&config));
        let (form, adopted) = editor
            .as_ref()
            .map(|e| {
                let mut fields = pdfcraft_forms::fields_with_adopted(&e.cos);
                xfa::mark_script_buttons(&e.cos, &mut fields.items);
                (fields.items, fields.adopted_page_fields)
            })
            .unwrap_or_default();
        // Fields the file lists only as page widgets are adopted leniently; the file says so.
        if adopted > 0
            && let Some(e) = &mut editor
        {
            let line = if adopted == 1 {
                "the form's /Fields list omits 1 field; it was read from the page annotations".to_string()
            } else {
                format!("the form's /Fields list omits {adopted} fields; they were read from the page annotations")
            };
            e.cos.note_repair(line);
        }
        Document {
            id: DocId(0),
            name,
            path,
            bytes,
            display,
            info,
            renderer,
            password: render_password,
            dirty: false,
            generation: 0,
            read_only_reason,
            form: Arc::new(form),
            marker_scan: OnceLock::new(),
            links: OnceLock::new(),
            signatures: Arc::new(signatures),
            trust: context.trust.clone(),
            captions: context.captions.clone(),
            sig_cache,
            editor,
            config,
            js_output: Default::default(),
            js_runtime: Default::default(),
            js_zoom: 100.0,
            js_page: 0,
            live: Default::default(),
            xfa,
            xfa_template: None,
            xfa_scripts_off: false,
            xfa_exited: Default::default(),
            fonts_stale: false,
            xfa_warnings: Vec::new(),
            edit_notices: Vec::new(),
        }
    }
}

/// The bytes, inspection and document model of a document after one more revision.
type Rebased = (Arc<Vec<u8>>, DocInfo, pdfcraft_cos::Document);

/// Line the page tree of `cos` up with the pages `info` shows, where the document model's walk
/// reaches fewer of them, written as one more revision with `rebase`:
/// - the catalog leads to no page, but the reader found pages by scanning: the page objects are
///   gathered into a page tree (#1656);
/// - a page tree node could not be read (one bad `/Kids` entry makes the model reject the node,
///   where the reader skips the entry): the pages the reader shows under it are gathered into the
///   page tree, in the order shown.
///
/// Kept only when the reader then shows the same pages, and never on a signed document, where
/// writing the repair could rewrite the file under its signatures. `Err` leaves it as it is, with
/// a warning when pages are shown under an unreadable node but cannot be taken out from under it
/// without changing how they look.
fn repair_page_tree(
    context: &OpenContext,
    cos: &pdfcraft_cos::Document,
    bytes: &Arc<Vec<u8>>,
    info: &DocInfo,
    render_password: Option<&str>,
    rebase: &dyn Fn(&pdfcraft_cos::Document) -> Result<Rebased, String>,
) -> Result<Rebased, Option<String>> {
    let walked = pdfcraft_organize::page_count(cos).unwrap_or(0);
    if walked >= info.pages.len() {
        return Err(None);
    }
    let mut work = cos.clone();
    let uncarried = std::cell::Cell::new(false);
    let added = if walked == 0 {
        pdfcraft_organize::recover_page_tree(&mut work).map_err(|_| None)?
    } else {
        let shown = pdfcraft_render::page_objects(bytes.clone(), render_password).unwrap_or_default();
        // What the reader inherits from a node the model cannot read; the pages taken out from
        // under it carry that, or stay where they are.
        let node_attributes = |node| {
            let read = pdfcraft_render::page_tree_node_attributes(bytes.clone(), render_password, node);
            uncarried.set(uncarried.get() || read.is_none());
            read
        };
        pdfcraft_organize::recover_unreadable_page_tree_nodes(&mut work, &shown, &node_attributes).map_err(|_| None)?
    };
    if added == 0 {
        return Err(uncarried.get().then(|| {
            let hidden = info.pages.len().saturating_sub(walked);
            format!(
                "{hidden} of the {} pages shown are under a page tree node that could not be read, and could not be moved out of it \
                 without changing how they look; edits reach only the other {walked}",
                info.pages.len()
            )
        }));
    }
    // A signature field the form's field list leaves out is still found by its signature dictionary.
    if pdfcraft_sign::pdf::list(cos, bytes, &context.trust).iter().any(|s| s.signed) || pdfcraft_sign::has_signatures(cos) {
        return Err(None);
    }
    let (b, i, mut c) = rebase(&work).ok().filter(|(_, i, _)| same_pages(&i.pages, &info.pages)).ok_or(None)?;
    if walked == 0 {
        c.note_repair(format!("the page tree is missing; its {} pages were found by scanning the file", i.pages.len()));
    } else {
        let (pages, were) = if added == 1 { ("page", "was") } else { ("pages", "were") };
        c.note_repair(format!("a page tree node could not be read; the {added} {pages} shown under it {were} gathered into the page tree"));
    }
    Ok((b, i, c))
}
