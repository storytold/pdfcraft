use super::options::{
    COMBINE_OPTIONS, EXTRACT_OPTIONS, INFO_OPTIONS, RENDER_OPTIONS, SPLIT_OPTIONS, TEXT_OPTIONS, can_be_path_value, flag, needs_value, positional,
    validate_options,
};
use super::{Arc, CliError, ImageFormat, PageRenderer, Path, PathBuf, RenderConfig, RenderRequest, RequestKind, Write, inspect, stdout_line};

pub(crate) fn read(path: &str) -> Result<Arc<Vec<u8>>, String> {
    std::fs::read(path).map(Arc::new).map_err(|e| format!("{path}: {e}"))
}

pub(crate) fn info(args: &[String]) -> Result<(), CliError> {
    validate_options("info", args, INFO_OPTIONS)?;
    let path = *positional(args, INFO_OPTIONS).first().ok_or("info: missing file")?;
    let bytes = read(path)?;
    let password = flag(args, "--password");
    // The renderer takes only the user password of R2–R4 files; `Session::open` takes either.
    let render_password = pdfcraft_engine::render_password_for(bytes.clone(), password);
    let mut info = inspect(bytes.clone(), render_password.as_deref()).map_err(|e| e.to_string())?;
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
        "file": path,
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
    stdout_line(format_args!("{}", serde_json::to_string_pretty(&json).unwrap_or_default()))?;
    Ok(())
}

pub(crate) fn text(args: &[String]) -> Result<(), CliError> {
    validate_options("text", args, TEXT_OPTIONS)?;
    let path = *positional(args, TEXT_OPTIONS).first().ok_or("text: missing file")?;
    let mut selected_page = None;
    // Validate every supplied value before reading, retaining the first valid selection.
    let mut options = args.iter();
    while let Some(option) = options.next() {
        if option == "--page" {
            let page = options
                .next()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|page| *page > 0)
                .ok_or("bad --page: expected a positive page number")?;
            selected_page.get_or_insert(page - 1);
        } else if option.starts_with("--") {
            // Match positional(): an option's operand is not itself another option.
            options.next();
        }
    }
    let password = flag(args, "--password");
    let mut bytes = read(path)?;
    // The renderer takes only the user password of R2–R4 files; `Session::open` takes either.
    // Worked out from the file as read: `bytes` may become the session's display copy below.
    let render_password = pdfcraft_engine::render_password_for(bytes.clone(), password);
    // Printing the text is copying it, so ask what `text_extract` asks (#1645): without the
    // permissions password, a document that doesn't allow copying gives no text.
    let mut session = pdfcraft_engine::Session::new();
    if let Some(doc) = session.open(path, None, bytes.clone(), password).ok().and_then(|id| session.get(id)) {
        // The engine's hint is about editing in the app; on the command line the way in is
        // `--password`.
        doc.check_copying()
            .map_err(|_| "text: the document's permissions don't allow copying its text; pass the owner password with --password to extract it")?;
        if doc.xfa.is_some() {
            bytes = doc.display.clone();
        }
    }
    let mut r = PageRenderer::new(bytes.clone(), RenderConfig { password: render_password.as_deref().map(Arc::from), ..Default::default() });
    // A document the renderer could not open (wrong or missing password, unparseable file) has
    // no pages, and "extract every page" of nothing would print nothing and exit 0 (#132). Ask
    // `inspect` why instead, so a protected file fails the same way `info` does.
    if r.page_count() == 0 {
        let info = inspect(bytes.clone(), render_password.as_deref()).map_err(|e| format!("text: {e}"))?;
        if !info.pages.is_empty() {
            return Err("text: the document could not be parsed".into());
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
            let _ = writeln!(std::io::stderr().lock(), "page {}: {e}", p + 1);
            failed.push(p + 1);
            continue;
        }
        if n > 0 {
            stdout_line(format_args!("\u{c}"))?;
        }
        stdout_line(format_args!("{}", out.text.map(|t| t.plain_text()).unwrap_or_default()))?;
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("text: page(s) {} could not be read", failed.iter().map(usize::to_string).collect::<Vec<_>>().join(", ")).into())
    }
}

/// The most pages one `A-B` range may name: a bound on what a typed range can allocate.
const MAX_RANGE: usize = 1_000_000;

/// 1-based pages ("1,3,5-8": single pages and inclusive ranges, in the order given) → 0-based
/// indices (#1923).
fn page_list(s: &str) -> Result<Vec<usize>, String> {
    let number = |t: &str, item: &str| t.trim().parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| format!("bad page number {item:?}"));
    let mut out = Vec::new();
    for item in s.split(',').map(str::trim) {
        match item.split_once('-') {
            Some((from, to)) => {
                let (from, to) = (number(from, item)?, number(to, item)?);
                if to < from {
                    return Err(format!("bad page range {item:?}: it ends before it starts"));
                }
                if to - from >= MAX_RANGE {
                    return Err(format!("bad page range {item:?}: more than {MAX_RANGE} pages"));
                }
                out.extend(from - 1..to);
            }
            None => out.push(number(item, item)? - 1),
        }
    }
    Ok(out)
}

/// The options `edit` understands, for its "unknown option" message.
const EDIT_OPTIONS: &str = "--out, --password, --full, --rotate, --delete, --move, --insert-blank, --title, --author";

/// Apply page and metadata edits through the engine and save (incrementally unless `--full`).
///
/// Every argument is parsed before the file is opened: an option this command does not know
/// (a typo like `--rotat`) is an error, not something to skip — skipping it would write an
/// unchanged copy and exit 0, which looks like a successful edit (#133).
pub(crate) fn edit(args: &[String]) -> Result<(), CliError> {
    use pdfcraft_engine::{Edit, Session};
    let mut path: Option<&str> = None;
    let mut out: Option<&str> = None;
    let mut password: Option<&str> = None;
    let mut full = false;
    let mut edits = Vec::new();
    let mut i = 0;
    while let Some(arg) = args.get(i).map(String::as_str) {
        // Options that take no value, and the input file.
        match arg {
            "--full" => {
                full = true;
                i += 1;
                continue;
            }
            a if a.starts_with("--") => {}
            a => {
                if path.is_some() {
                    return Err(format!("edit: unexpected argument {a:?} (one input file, then options)").into());
                }
                path = Some(a);
                i += 1;
                continue;
            }
        }
        // `edit` parses its own options (they run in order), so `--out` is checked here in the
        // same way: it wrote a PDF to a file called `--full`.
        let value = args.get(i + 1).map(String::as_str);
        if !can_be_path_value(arg, value) {
            return Err(needs_value("edit", arg, value).into());
        }
        let value = value.ok_or_else(|| format!("edit: {arg} needs a value"))?;
        match arg {
            "--out" => out = Some(value),
            "--password" => password = Some(value),
            "--rotate" => {
                let (pages, deg) = value.split_once(':').ok_or("--rotate PAGES:DEGREES")?;
                let degrees: i64 = deg.parse().map_err(|_| format!("bad --rotate degrees: expected a whole number of degrees, got {deg:?}"))?;
                // The same rule `rotate_pages` enforces, checked here so a bad angle is named
                // with its option before any file is opened.
                if degrees % 90 != 0 {
                    return Err(format!("bad --rotate degrees: expected a multiple of 90 (such as 90, -90 or 180), got {degrees}").into());
                }
                edits.push(Edit::RotatePages { pages: page_list(pages)?, degrees });
            }
            "--delete" => edits.push(Edit::DeletePages { pages: page_list(value)? }),
            "--move" => {
                let (pages, to) = value.split_once(':').ok_or("--move PAGES:TO")?;
                let target: usize = to
                    .parse()
                    .ok()
                    .filter(|target| *target > 0)
                    .ok_or_else(|| format!("bad --move target: expected a page number of at least 1, got {to:?}"))?;
                edits.push(Edit::MovePages { pages: page_list(pages)?, to: target - 1 });
            }
            "--insert-blank" => {
                let position: usize = value
                    .parse()
                    .ok()
                    .filter(|position| *position > 0)
                    .ok_or_else(|| format!("bad --insert-blank position: expected a page position of at least 1, got {value:?}"))?;
                edits.push(Edit::InsertBlankPage { at: position - 1, width: 612.0, height: 792.0 });
            }
            "--title" => edits.push(Edit::SetInfo { key: "Title".into(), value: value.into() }),
            "--author" => edits.push(Edit::SetInfo { key: "Author".into(), value: value.into() }),
            other => return Err(format!("edit: unknown option {other:?} (expected one of: {EDIT_OPTIONS})").into()),
        }
        i += 2;
    }
    let path = path.ok_or("edit: missing file")?;
    let out = out.ok_or("edit: missing --out")?;
    let mut session = Session::new();
    let id = session.open(path, Some(path.to_string()), read(path)?, password).map_err(|e| e.to_string())?;
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
            return Err(format!("{option} {} is past the end: it must be between 1 and {} (the document has {n} pages)", at + 1, n + 1).into());
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
            return Err(format!("{option}: page {} does not exist; after the earlier options the document has {n} pages", bad + 1).into());
        }
        session.apply(id, e).map_err(|e| e.to_string())?;
    }
    let bytes = if full { session.save_full_bytes(id) } else { session.save_bytes(id) }.map_err(|e| e.to_string())?;
    // As the app's Save: the file must reopen with its pages before it replaces anything (#1698).
    session.check_save(id, &bytes).map_err(|e| e.to_string())?;
    write_pdf(Path::new(out), &bytes)
}

/// Write a PDF a command produced, through a staging file beside the target (#2166): a write
/// that fails, or `--out` naming the input, never leaves a truncated file where a good one was.
fn write_pdf(out: &Path, bytes: &[u8]) -> Result<(), CliError> {
    use pdfcraft_platform::staging::{StagingName, write_atomically};
    write_atomically(out, bytes, StagingName::SuffixThenTag).map_err(|e| format!("{}: {e}", out.display()).into())
}

fn file_stem(path: &str) -> String {
    Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

pub(crate) fn combine(args: &[String]) -> Result<(), CliError> {
    validate_options("combine", args, COMBINE_OPTIONS)?;
    let out = flag(args, "--out").ok_or("combine: missing --out")?;
    let inputs = positional(args, COMBINE_OPTIONS);
    if inputs.len() < 2 {
        return Err("combine: give at least two input files".into());
    }
    let sources = inputs.iter().map(|p| Ok((file_stem(p), read(p)?))).collect::<Result<Vec<_>, String>>()?;
    let bytes = pdfcraft_engine::Session::new().combine(&sources).map_err(|e| e.to_string())?;
    write_pdf(Path::new(out), &bytes)
}

pub(crate) fn extract(args: &[String]) -> Result<(), CliError> {
    validate_options("extract", args, EXTRACT_OPTIONS)?;
    let path = *positional(args, EXTRACT_OPTIONS).first().ok_or("extract: missing file")?;
    let out = flag(args, "--out").ok_or("extract: missing --out")?;
    let pages = page_list(flag(args, "--pages").ok_or("extract: missing --pages")?)?;
    let mut session = pdfcraft_engine::Session::new();
    let id = session.open(path, None, read(path)?, flag(args, "--password")).map_err(|e| e.to_string())?;
    let bytes = session.extract(id, &pages).map_err(|e| e.to_string())?;
    write_pdf(Path::new(out), &bytes)
}

pub(crate) fn split(args: &[String]) -> Result<(), CliError> {
    validate_options("split", args, SPLIT_OPTIONS)?;
    use pdfcraft_engine::SplitBy;
    let path = *positional(args, SPLIT_OPTIONS).first().ok_or("split: missing file")?;
    let by = match (flag(args, "--every"), flag(args, "--before")) {
        (Some(n), None) => {
            // A count the splitter cannot hold is refused here, before the file is opened,
            // rather than silently clamped to 1 further down.
            let count = n
                .parse::<usize>()
                .ok()
                .filter(|count| *count > 0)
                .ok_or_else(|| format!("bad --every: expected a positive whole number of pages, got {n:?}"))?;
            SplitBy::PageCount(count)
        }
        (None, Some(list)) => SplitBy::Before(page_list(list)?),
        _ => return Err("split: give either --every N or --before PAGES".into()),
    };
    let dir = PathBuf::from(flag(args, "--out-dir").unwrap_or("."));
    let mut session = pdfcraft_engine::Session::new();
    let id = session.open(path, None, read(path)?, flag(args, "--password")).map_err(|e| e.to_string())?;
    let stem = file_stem(path);
    let parts = session.split(id, &by).map_err(|e| e.to_string())?;
    // An output folder that does not exist yet is created (#249); one that can't be is reported
    // as the folder's problem, not as the first part's.
    std::fs::create_dir_all(&dir).map_err(|e| format!("split: --out-dir {}: {e}", dir.display()))?;
    for (a, b, bytes) in parts {
        let name = dir.join(if a == b { format!("{stem}-p{a}.pdf") } else { format!("{stem}-p{a}-{b}.pdf") });
        write_pdf(&name, &bytes)?;
        stdout_line(format_args!("{}", name.display()))?;
    }
    Ok(())
}

/// A `--dpi` the renderer can be asked for: a positive, finite number. `0`, a negative value
/// or `NaN` parse as an `f32` but reach the renderer as a scale it refuses with its own
/// internal message (and in `check`, passed on as text, they would turn every file into a
/// child that printed nothing), so the CLI names the option and the value instead.
pub(crate) fn parse_dpi(value: &str) -> Result<f32, String> {
    value
        .parse::<f32>()
        .ok()
        .filter(|dpi| dpi.is_finite() && *dpi > 0.0)
        .ok_or_else(|| format!("bad --dpi: expected a positive number of dots per inch, got {value:?}"))
}

pub(crate) fn render(args: &[String]) -> Result<(), CliError> {
    validate_options("render", args, RENDER_OPTIONS)?;
    let path = *positional(args, RENDER_OPTIONS).first().ok_or("render: missing file")?;
    let page: usize =
        flag(args, "--page").unwrap_or("1").parse().ok().filter(|page| *page > 0).ok_or("bad --page: expected a positive page number")?;
    let page_index = page - 1;
    let dpi = parse_dpi(flag(args, "--dpi").unwrap_or("96"))?;
    let out = flag(args, "--out").ok_or("render: missing --out (.png, .jpg, .tif or .pam)")?;
    // The file is what its name says (#248): a `.png` used to get a netpbm PAM stream.
    let format = match Path::new(out).extension().and_then(std::ffi::OsStr::to_str).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => Some(ImageFormat::Png),
        Some("jpg" | "jpeg") => Some(ImageFormat::Jpeg { quality: 90 }),
        Some("tif" | "tiff") => Some(ImageFormat::Tiff),
        Some("pam") => None,
        _ => return Err(format!("render: --out {out}: use a .png, .jpg, .tif or .pam name").into()),
    };
    // The app renders a display copy that draws comments missing their /AP streams; the CLI
    // must agree with it (and with independent viewers), while keeping the file it reads intact.
    let bytes = read(path)?;
    let password = flag(args, "--password");
    // The renderer takes only the user password of R2–R4 files; `Session::open` and the display
    // copy take either.
    let render_password = pdfcraft_engine::render_password_for(bytes.clone(), password);
    let display = if inspect(bytes.clone(), render_password.as_deref()).is_ok_and(|info| info.xfa == Some(pdfcraft_render::Xfa::Dynamic)) {
        let mut session = pdfcraft_engine::Session::new();
        let id = session.open(path, None, bytes, password).map_err(|e| e.to_string())?;
        session.get(id).ok_or("render: the document could not be opened")?.display.clone()
    } else {
        pdfcraft_engine::display_bytes_for(bytes, password)
    };
    let mut r =
        PageRenderer::new(display, RenderConfig { password: render_password.as_deref().map(Arc::from), reject_oversize: true, ..Default::default() });
    let p = r.render(RenderRequest { page: page_index, kind: RequestKind::Pixels, tile: None, scale: dpi / 72.0, tag: 0, preview: false });
    if let Some(e) = p.error {
        return Err(e.into());
    }
    let bytes = match format {
        // With `reject_oversize` the page is drawn at `--dpi` or not at all, so the file can say so.
        Some(f) => pdfcraft_engine::export::encode_image(p.width, p.height, &p.rgba, f, f64::from(dpi))?,
        None => {
            // PAM (netpbm RGB_ALPHA): the raw premultiplied pixels, for tools that read it.
            let mut pam = format!("P7\nWIDTH {}\nHEIGHT {}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n", p.width, p.height).into_bytes();
            pam.extend_from_slice(&p.rgba);
            pam
        }
    };
    std::fs::write(out, bytes).map_err(|e| format!("{out}: {e}"))?;
    let _ = writeln!(std::io::stderr().lock(), "rendered page {page} at {dpi} dpi: {}×{} px in {} ms", p.width, p.height, p.millis);
    for warning in &p.warnings {
        match warning {
            pdfcraft_render::RenderWarning::ContentTruncated => {
                let _ = writeln!(std::io::stderr().lock(), "warning: page {page}: content past the page's safety budget was skipped");
            }
            pdfcraft_render::RenderWarning::ColourFallback => {
                let _ =
                    writeln!(std::io::stderr().lock(), "warning: page {page}: a colour that could not be computed was painted in a fallback colour");
            }
        }
    }
    Ok(())
}
