use std::path::Path;
use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind, inspect};

use crate::cli::{AuditArgs, CombineArgs, EditArgs, EditStep, ExtractArgs, InfoArgs, Move, RenderArgs, Rotate, SplitArgs, TextArgs};
use crate::error::CliError;
use crate::{Io, stdout_line};

pub(crate) fn read(path: &Path) -> Result<Arc<Vec<u8>>, CliError> {
    std::fs::read(path).map(Arc::new).map_err(|e| CliError::file(path, e))
}

fn text_of(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn document_error(path: &Path, source: impl std::error::Error + Send + Sync + 'static) -> CliError {
    CliError::Document { path: path.to_path_buf(), source: Box::new(source) }
}

pub(crate) fn info(args: &InfoArgs, io: &mut Io) -> Result<(), CliError> {
    let path = args.file.as_path();
    let bytes = read(path)?;
    let password = args.password.password.as_deref();
    // The renderer takes only the user password of R2–R4 files; `Session::open` takes either.
    let render_password = pdfcraft_engine::render_password_for(bytes.clone(), password);
    let mut info = inspect(bytes.clone(), render_password.as_deref()).map_err(|e| CliError::Render(e.to_string()))?;
    let pdf_pages = info.pages.len();
    let acroform_fields = info.fields.len();
    // Dynamic XFA forms: say what laying the template out gives (what the app shows).
    let xfa_layout = (info.xfa == Some(pdfcraft_render::Xfa::Dynamic))
        .then(|| {
            let mut s = pdfcraft_engine::Session::new();
            let id = s.open("info.pdf", None, bytes.clone(), password).ok()?;
            let d = s.get(id)?;
            d.xfa.as_ref().map(|x| {
                info = d.info.clone();
                // The in-memory layout is an appended revision, not the file being inspected.
                info.file_size = bytes.len();
                serde_json::json!({ "pages": x.pages, "fields": x.fields, "warnings": x.warnings })
            })
        })
        .flatten();
    // What the document declares as a standard, read the way the Standards panel reads it, so
    // `info` and the app agree. A file that declares nothing leaves these null and empty.
    let declared =
        pdfcraft_cos::Document::open_with_password(bytes.clone(), password).map(|doc| pdfcraft_preflight::declared(&doc)).unwrap_or_default();
    let json = serde_json::json!({
        "file": text_of(path),
        "pdf_version": info.pdf_version,
        "file_size": info.file_size,
        "pages": info.pages.len(),
        "pdf_pages": pdf_pages, "acroform_fields": acroform_fields,
        "first_page_pt": info.pages.first().map(|p| [p.width, p.height]),
        "title": info.title, "author": info.author, "producer": info.producer, "creator": info.creator,
        "encrypted": info.encrypted, "tagged": info.tagged, "javascript": info.has_javascript,
        "pdfa": declared.pdfa.as_ref().map(|(part, conformance)| format!("PDF/A-{part}{}", conformance.to_lowercase())),
        "pdfua": declared.pdfua,
        "output_intents": declared.output_intents,
        "xfa": info.xfa.map(|x| match x { pdfcraft_render::Xfa::Static => "static", pdfcraft_render::Xfa::Dynamic => "dynamic" }),
        "xfa_layout": xfa_layout,
        "bookmarks": info.outline.len(), "annotations": info.annotations.len(), "fields": info.fields.len(),
        "links": info.links.len(), "layers": info.layers.len(), "attachments": info.attachments.len(),
        "transitions": info.transitions.len(),
        "custom_properties": info.custom.len(),
        "page_labels": info.pages.iter().take(8).map(|p| p.label.clone()).collect::<Vec<_>>(),
        "warnings": info.warnings,
    });
    stdout_line(io, format_args!("{}", serde_json::to_string_pretty(&json).unwrap_or_default()))
}

pub(crate) fn text(args: &TextArgs, io: &mut Io) -> Result<(), CliError> {
    let path = args.file.as_path();
    let selected_page = args.page.map(|page| page.get() - 1);
    let password = args.password.password.as_deref();
    let mut bytes = read(path)?;
    // The renderer takes only the user password of R2–R4 files; `Session::open` takes either.
    // Worked out from the file as read: `bytes` may become the session's display copy below.
    let render_password = pdfcraft_engine::render_password_for(bytes.clone(), password);
    // Printing the text is copying it, so ask what `text_extract` asks (#1645): without the
    // permissions password, a document that doesn't allow copying gives no text.
    let mut session = pdfcraft_engine::Session::new();
    if let Some(doc) = session.open(text_of(path), None, bytes.clone(), password).ok().and_then(|id| session.get(id)) {
        // The engine's hint is about editing in the app; on the command line the way in is
        // `--password`.
        doc.check_copying().map_err(|_| CliError::CopyingNotAllowed)?;
        if doc.xfa.is_some() {
            bytes = doc.display.clone();
        }
    }
    let mut r = PageRenderer::new(bytes.clone(), RenderConfig { password: render_password.as_deref().map(Arc::from), ..Default::default() });
    // A document the renderer could not open (wrong or missing password, unparseable file) has
    // no pages, and "extract every page" of nothing would print nothing and exit 0 (#132). Ask
    // `inspect` why instead, so a protected file fails the same way `info` does.
    if r.page_count() == 0 {
        let info = inspect(bytes.clone(), render_password.as_deref()).map_err(|e| CliError::command("text", e.to_string()))?;
        if !info.pages.is_empty() {
            return Err(CliError::NotParsed);
        }
    }
    let pages: Vec<usize> = match selected_page {
        Some(page) => vec![page],
        None => (0..r.page_count()).collect(),
    };
    let mut failed: Vec<usize> = Vec::new();
    for (n, p) in pages.iter().enumerate() {
        let out = r.render(RenderRequest { page: *p, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0, preview: false });
        if let Some(e) = out.error {
            let _ = writeln!(io.err, "page {}: {e}", p + 1);
            failed.push(p + 1);
            continue;
        }
        if n > 0 {
            stdout_line(io, format_args!("\u{c}"))?;
        }
        stdout_line(io, format_args!("{}", out.text.map(|t| t.plain_text()).unwrap_or_default()))?;
    }
    if failed.is_empty() { Ok(()) } else { Err(CliError::PagesUnreadable(failed)) }
}

/// Apply page and metadata edits through the engine and save (incrementally unless `--full`).
///
/// Every option was parsed before this runs, in the order given: an option this command does not
/// know (a typo like `--rotat`) is a usage error, not something to skip — skipping it would write
/// an unchanged copy and exit 0, which looks like a successful edit (#133).
pub(crate) fn edit(args: &EditArgs, steps: Vec<EditStep>) -> Result<(), CliError> {
    use pdfcraft_engine::{Edit, Session};
    let path = args.file.as_path();
    let edits = steps
        .into_iter()
        .map(|step| match step {
            EditStep::Rotate(Rotate { pages, degrees }) => Edit::RotatePages { pages, degrees },
            EditStep::Delete(pages) => Edit::DeletePages { pages },
            EditStep::Move(Move { pages, to }) => Edit::MovePages { pages, to },
            EditStep::InsertBlank(at) => Edit::InsertBlankPage { at, width: 612.0, height: 792.0 },
            EditStep::Title(value) => Edit::SetInfo { key: "Title".into(), value },
            EditStep::Author(value) => Edit::SetInfo { key: "Author".into(), value },
        })
        .collect::<Vec<_>>();
    let mut session = Session::new();
    let id = session
        .open(text_of(path), Some(text_of(path)), read(path)?, args.password.password.as_deref())
        .map_err(|e| CliError::Engine(e.to_string()))?;
    for e in edits {
        // A destination past the end must fail like `page_move` / `page_insert_blank` do, not
        // silently become an append (#1330). Checked per edit: earlier edits change the count.
        let n = session.get(id).map_or(0, |d| d.info.pages.len());
        let destination = match &e {
            Edit::MovePages { to, .. } => Some(("--move target", *to)),
            Edit::InsertBlankPage { at, .. } => Some(("--insert-blank", *at)),
            _ => None,
        };
        if let Some((option, at)) = destination.filter(|(_, at)| *at > n) {
            return Err(CliError::PositionPastEnd { option, at: at.saturating_add(1), max: n.saturating_add(1), count: n });
        }
        // Pages count from the document as the earlier options left it, so a page past the end is
        // reported with its own option and the current page count (#1550).
        let listed = match &e {
            Edit::RotatePages { pages, .. } => Some(("--rotate", pages)),
            Edit::DeletePages { pages } => Some(("--delete", pages)),
            Edit::MovePages { pages, .. } => Some(("--move", pages)),
            _ => None,
        };
        if let Some((option, bad)) = listed.and_then(|(option, pages)| pages.iter().find(|p| **p >= n).map(|p| (option, *p))) {
            return Err(CliError::PageMissing { option, page: bad.saturating_add(1), count: n });
        }
        session.apply(id, e).map_err(|e| CliError::Engine(e.to_string()))?;
    }
    let bytes = if args.full { session.save_full_bytes(id) } else { session.save_bytes(id) }.map_err(|e| CliError::Engine(e.to_string()))?;
    // As the app's Save: the file must reopen with its pages before it replaces anything (#1698).
    session.check_save(id, &bytes).map_err(|e| CliError::Engine(e.to_string()))?;
    write_pdf(&args.out, &bytes)
}

/// Write a PDF a command produced, through a staging file beside the target (#2166): a write
/// that fails, or `--out` naming the input, never leaves a truncated file where a good one was.
fn write_pdf(out: &Path, bytes: &[u8]) -> Result<(), CliError> {
    use pdfcraft_platform::staging::{StagingName, write_atomically};
    write_atomically(out, bytes, StagingName::SuffixThenTag).map_err(|e| CliError::file(out, e))
}

fn file_stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| text_of(path))
}

pub(crate) fn combine(args: &CombineArgs) -> Result<(), CliError> {
    let sources = args.inputs.iter().map(|p| Ok((file_stem(p), read(p)?))).collect::<Result<Vec<_>, CliError>>()?;
    let bytes = pdfcraft_engine::Session::new().combine(&sources).map_err(|e| CliError::Engine(e.to_string()))?;
    write_pdf(&args.out, &bytes)
}

pub(crate) fn extract(args: &ExtractArgs) -> Result<(), CliError> {
    let path = args.file.as_path();
    let mut session = pdfcraft_engine::Session::new();
    let id = session.open(text_of(path), None, read(path)?, args.password.password.as_deref()).map_err(|e| CliError::Engine(e.to_string()))?;
    let bytes = session.extract(id, &args.pages.0).map_err(|e| CliError::Engine(e.to_string()))?;
    write_pdf(&args.out, &bytes)
}

pub(crate) fn split(args: &SplitArgs, io: &mut Io) -> Result<(), CliError> {
    use pdfcraft_engine::SplitBy;
    let path = args.file.as_path();
    // The parser's group allows exactly one of the two.
    let by = match (args.every, &args.before) {
        (Some(count), _) => SplitBy::PageCount(count.get()),
        (None, Some(pages)) => SplitBy::Before(pages.0.clone()),
        (None, None) => return Err(CliError::command("split", "give either --every N or --before PAGES")),
    };
    let dir = args.out_dir.as_path();
    let mut session = pdfcraft_engine::Session::new();
    let id = session.open(text_of(path), None, read(path)?, args.password.password.as_deref()).map_err(|e| CliError::Engine(e.to_string()))?;
    let stem = file_stem(path);
    let parts = session.split(id, &by).map_err(|e| CliError::Engine(e.to_string()))?;
    // An output folder that does not exist yet is created (#249); one that can't be is reported
    // as the folder's problem, not as the first part's.
    std::fs::create_dir_all(dir).map_err(|e| CliError::command("split", format!("--out-dir {}: {e}", dir.display())))?;
    for (a, b, bytes) in parts {
        let name = dir.join(if a == b { format!("{stem}-p{a}.pdf") } else { format!("{stem}-p{a}-{b}.pdf") });
        write_pdf(&name, &bytes)?;
        stdout_line(io, format_args!("{}", name.display()))?;
    }
    Ok(())
}

pub(crate) fn render(args: &RenderArgs, io: &mut Io) -> Result<(), CliError> {
    let path = args.file.as_path();
    let page = args.page.get();
    let dpi = args.dpi;
    let out = args.out.path.as_path();
    // The app renders a display copy that draws comments missing their /AP streams; the CLI
    // must agree with it (and with independent viewers), while keeping the file it reads intact.
    let bytes = read(path)?;
    let password = args.password.password.as_deref();
    // The renderer takes only the user password of R2–R4 files; `Session::open` and the display
    // copy take either.
    let render_password = pdfcraft_engine::render_password_for(bytes.clone(), password);
    let display = if inspect(bytes.clone(), render_password.as_deref()).is_ok_and(|info| info.xfa == Some(pdfcraft_render::Xfa::Dynamic)) {
        let mut session = pdfcraft_engine::Session::new();
        let id = session.open(text_of(path), None, bytes, password).map_err(|e| CliError::Engine(e.to_string()))?;
        session.get(id).ok_or_else(|| CliError::command("render", "the document could not be opened"))?.display.clone()
    } else {
        pdfcraft_engine::display_bytes_for(bytes, password)
    };
    let mut r =
        PageRenderer::new(display, RenderConfig { password: render_password.as_deref().map(Arc::from), reject_oversize: true, ..Default::default() });
    let p = r.render(RenderRequest { page: page - 1, kind: RequestKind::Pixels, tile: None, scale: dpi / 72.0, tag: 0, preview: false });
    if let Some(e) = p.error {
        return Err(CliError::Render(e));
    }
    let bytes = match args.out.format {
        // With `reject_oversize` the page is drawn at `--dpi` or not at all, so the file can say so.
        Some(f) => pdfcraft_engine::export::encode_image(p.width, p.height, &p.rgba, f, f64::from(dpi)).map_err(CliError::Render)?,
        None => {
            // PAM (netpbm RGB_ALPHA): the raw premultiplied pixels, for tools that read it.
            let mut pam = format!("P7\nWIDTH {}\nHEIGHT {}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n", p.width, p.height).into_bytes();
            pam.extend_from_slice(&p.rgba);
            pam
        }
    };
    std::fs::write(out, bytes).map_err(|e| CliError::file(out, e))?;
    let _ = writeln!(io.err, "rendered page {page} at {dpi} dpi: {}×{} px in {} ms", p.width, p.height, p.millis);
    for warning in &p.warnings {
        match warning {
            pdfcraft_render::RenderWarning::ContentTruncated => {
                let _ = writeln!(io.err, "warning: page {page}: content past the page's safety budget was skipped");
            }
            pdfcraft_render::RenderWarning::ColourFallback => {
                let _ = writeln!(io.err, "warning: page {page}: a colour that could not be computed was painted in a fallback colour");
            }
        }
    }
    Ok(())
}

/// Audit a document for faux redactions. Prints the findings as JSON; fails when any are
/// found so scripts and CI can gate on it, and when the audit itself cannot run.
pub(crate) fn audit(args: &AuditArgs, io: &mut Io) -> Result<(), CliError> {
    let path = args.file.as_path();
    let bytes = read(path)?;
    let password = args.password.password.as_deref();
    let doc = pdfcraft_cos::Document::open_with_password(bytes.clone(), password).map_err(|e| document_error(path, e))?;
    let findings = pdfcraft_audit::audit_redactions(&doc).map_err(|e| document_error(path, e))?;
    // Rectangles in points from the top-left of the displayed page, as `doc_audit_redactions`
    // reports them; the page geometry is read only when there is something to place.
    let info = if findings.is_empty() {
        None
    } else {
        let render_password = pdfcraft_engine::render_password_for(bytes.clone(), password);
        Some(inspect(bytes, render_password.as_deref()).map_err(|e| document_error(path, e))?)
    };
    let items: Vec<serde_json::Value> = findings
        .into_iter()
        .map(|f| {
            let rect = f.rect.map(|v| v as f32);
            serde_json::json!({
                "page": f.page + 1,
                "kind": f.kind.id(),
                "rect": info.as_ref().map_or(rect, |i| pdfcraft_automation::view_rect(i, f.page, rect)),
                "covered_text": f.covered_text,
            })
        })
        .collect();
    stdout_line(io, format_args!("{}", serde_json::json!({ "file": text_of(path), "findings": items, "count": items.len() })))?;
    if items.is_empty() { Ok(()) } else { Err(CliError::FauxRedactions(items.len())) }
}
