//! Writing a workbook as an XLSX package.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use gridcraft_core::RangeRef;
use gridcraft_formula::quote_sheet;
use gridcraft_model::{CalcMode, CalcSettings, PhoneticString, ShowObjects, Style, Visibility, Workbook, WorkbookView};
use sha2::Digest as _;

use crate::package::ZipOut;
use crate::xml::{esc, esc_attr};
use crate::{FunctionInfo, IoError, XlsxKind};

pub const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
pub const NS_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

pub const CT_WORKSHEET: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
pub const CT_DRAWING: &str = "application/vnd.openxmlformats-officedocument.drawing+xml";
pub const CT_CHART: &str = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";
pub const CT_TABLE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.table+xml";
pub const CT_COMMENTS: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml";
pub const CT_VBA: &str = "application/vnd.ms-office.vbaProject";
pub const CT_THREADED_COMMENTS: &str = "application/vnd.ms-excel.threadedcomments+xml";
pub const CT_PERSONS: &str = "application/vnd.ms-excel.person+xml";
pub const REL_THREADED_COMMENT: &str = "http://schemas.microsoft.com/office/2017/10/relationships/threadedComment";
pub const REL_PERSON: &str = "http://schemas.microsoft.com/office/2017/10/relationships/person";
/// Namespace of the threaded comments and persons parts ([MS-XLSX]).
pub const NS_THREADED: &str = "http://schemas.microsoft.com/office/spreadsheetml/2018/threadedcomments";

/// A part's relationships.
#[derive(Default)]
pub struct Rels {
    items: Vec<(String, String, String, bool)>,
}

impl Rels {
    /// Adds a relationship of type `kind` (last segment, e.g. `worksheet`); returns its id.
    pub fn add(&mut self, kind: &str, target: &str) -> String {
        let id = format!("rId{}", self.items.len() + 1);
        let ty = match kind {
            "core-properties" => "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties".to_string(),
            // The VBA project relationship lives in the Microsoft-documented namespace, not the
            // OpenXML one; Excel rejects the file if this type is wrong.
            "vbaProject" => "http://schemas.microsoft.com/office/2006/relationships/vbaProject".to_string(),
            k => format!("{NS_REL}/{k}"),
        };
        self.items.push((id.clone(), ty, target.to_string(), false));
        id
    }
    pub fn add_external(&mut self, kind: &str, target: &str) -> String {
        let id = format!("rId{}", self.items.len() + 1);
        self.items.push((id.clone(), format!("{NS_REL}/{kind}"), target.to_string(), true));
        id
    }
    /// Adds a Microsoft extension relationship with its complete namespace URI.
    pub fn add_uri(&mut self, ty: &str, target: &str) -> String {
        let id = format!("rId{}", self.items.len() + 1);
        self.items.push((id.clone(), ty.to_string(), target.to_string(), false));
        id
    }
    /// Adds a relationship with its complete type URI, internal or external (kept content).
    pub fn add_raw(&mut self, ty: &str, target: &str, external: bool) -> String {
        let id = format!("rId{}", self.items.len() + 1);
        self.items.push((id.clone(), crate::xml::esc_attr(ty), target.to_string(), external));
        id
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn xml(&self) -> String {
        let mut s = format!("{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">");
        for (id, ty, target, ext) in &self.items {
            let _ = write!(
                s,
                "<Relationship Id=\"{id}\" Type=\"{ty}\" Target=\"{}\"{}/>",
                esc_attr(target),
                if *ext { " TargetMode=\"External\"" } else { "" }
            );
        }
        s.push_str("</Relationships>");
        s
    }
}

/// Shared string table being built.
#[derive(Default)]
pub struct Sst {
    list: Vec<String>,
    map: HashMap<String, u32>,
    pub count: u64,
}

impl Sst {
    pub fn index(&mut self, s: &str) -> u32 {
        self.count += 1;
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        let i = self.list.len() as u32;
        self.list.push(s.to_string());
        self.map.insert(s.to_string(), i);
        i
    }
}

/// Package being assembled.
pub struct Out {
    parts: Vec<(String, Vec<u8>)>,
    overrides: Vec<(String, String)>,
    defaults: BTreeMap<String, String>,
    pub dxfs: Vec<Style>,
    /// Number format ids assigned so far (inline `x14:dxf` formats); `styles.xml` adds the rest.
    pub num_fmts: crate::styles::NumFmts,
    /// Excel 2010 conditional format rules written so far (each needs a unique id).
    pub x14_cf_rules: u32,
    pub sst: Sst,
    pub images: u32,
    /// Media parts written so far, by extension and SHA-256 of their bytes: a picture used again
    /// is found here instead of being compared byte by byte with every earlier one.
    media: HashMap<(String, [u8; 32]), String>,
    pub charts: u32,
    /// The workbook's file name, which a PivotChart's link to its PivotTable names in brackets.
    pub book_name: String,
    /// References of chartex parts, defined as hidden `_xlchart.v1.N` names.
    pub chart_names: Vec<String>,
    pub drawings: u32,
    pub tables: u32,
    pub comments: u32,
    /// VML drawings written, the last VML shape-id block used, and control properties parts.
    pub vmls: u32,
    pub vml_block: u32,
    pub ctrl_props: u32,
    /// Authors of threaded comments, in first-use order; written to `xl/persons/person.xml`.
    pub persons: Vec<String>,
    pub table_id: u32,
    pub dynamic_arrays: bool,
    pub needs_calc: bool,
    pub cell_pictures: crate::richdata::PicturesOut,
    /// Kept (pass-through) parts written so far.
    pub kept: crate::passthrough::KeptOut,
    /// Decides which formulas need dynamic-array metadata.
    pub functions: FunctionInfo,
    /// Defined names analysed for implicit intersection, shared by every formula written.
    pub(crate) names: crate::array_formula::NameCache,
    /// The code of the Python cells written so far, in order: script `i` of the Python part.
    pub python_scripts: Vec<String>,
}

impl Out {
    pub fn part(&mut self, name: &str, content_type: Option<&str>, data: Vec<u8>) {
        if let Some(ct) = content_type {
            self.overrides.push((format!("/{name}"), ct.to_string()));
        }
        // Every `xl/media/image*.ext` part is indexed as it is added, preserved ones included, so a
        // later identical picture finds it instead of writing a second copy.
        if let Some((ext, file)) = name.strip_prefix("xl/media/").filter(|f| f.starts_with("image")).and_then(|f| Some((f.rsplit_once('.')?.1, f))) {
            self.media.entry((ext.to_string(), sha2::Sha256::digest(&data).into())).or_insert_with(|| file.to_string());
        }
        self.parts.push((name.to_string(), data));
    }
    /// The `xl/media` file name (`imageN.ext`) holding `data`. A picture used on several sheets,
    /// or twice on one, is written once and every drawing points to that part, as in Excel.
    pub fn image_part(&mut self, ext: &str, data: &[u8]) -> String {
        let key = (ext.to_string(), sha2::Sha256::digest(data).into());
        if let Some(file) = self.media.get(&key) {
            return file.clone();
        }
        self.images += 1;
        let file = format!("image{}.{ext}", self.images);
        // Not through `part`: that would hash the same bytes again to index them.
        self.parts.push((format!("xl/media/{file}"), data.to_vec()));
        self.media.insert(key, file.clone());
        file
    }
    pub fn default_type(&mut self, ext: &str, ct: &str) {
        self.defaults.entry(ext.to_string()).or_insert_with(|| ct.to_string());
    }
}

/// Id of the i-th person of `xl/persons/person.xml`: a GUID-shaped, deterministic id (the
/// file only needs ids that are unique within it).
pub fn person_guid(i: usize) -> String {
    format!("{{00000000-0000-4000-8002-{:012X}}}", i as u64 & 0xFFFF_FFFF_FFFF)
}

/// Id of the i-th Excel 2010 conditional format rule of the package (deterministic and unique,
/// like [`person_guid`]).
pub fn x14_cf_rule_guid(i: u32) -> String {
    format!("{{00000000-0000-4000-8003-{i:012X}}}")
}

/// `$A$1:$D$20` (or `$1:$3` / `$A:$C`).
pub fn abs_range(r: &RangeRef) -> String {
    let c = |col: u32| gridcraft_core::col_to_letters(col);
    if r.is_full_rows() && !r.is_full_cols() {
        return format!("${}:${}", r.start.row + 1, r.end.row + 1);
    }
    if r.is_full_cols() && !r.is_full_rows() {
        return format!("${}:${}", c(r.start.col), c(r.end.col));
    }
    if r.is_single() {
        return format!("${}${}", c(r.start.col), r.start.row + 1);
    }
    format!("${}${}:${}${}", c(r.start.col), r.start.row + 1, c(r.end.col), r.end.row + 1)
}

/// Refuses sheet names XLSX cannot hold. The reader repairs such names (cuts, strips, renames
/// duplicates) but formulas keep the old spelling, so the saved file would turn into `#REF!`.
fn check_sheet_names(wb: &Workbook) -> Result<(), IoError> {
    let mut seen = std::collections::HashSet::new();
    for sheet in &wb.sheets {
        let name = &sheet.name;
        let bad = if name.is_empty() {
            Some("is blank")
        } else if name.chars().count() > 31 {
            Some("is longer than 31 characters")
        } else if name.chars().any(|c| matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']')) {
            Some("contains one of : \\ / ? * [ ]")
        } else if !seen.insert(name.to_ascii_lowercase()) {
            Some("is used by another sheet")
        } else {
            None
        };
        if let Some(why) = bad {
            return Err(IoError::Format(format!("sheet name \"{name}\" {why}; rename the sheet before saving as XLSX")));
        }
    }
    Ok(())
}

/// Workbook name written in a PivotChart's link when the file name isn't known.
const DEFAULT_BOOK_NAME: &str = "Workbook.xlsx";

pub fn write_xlsx(wb: &Workbook, kind: XlsxKind, functions: FunctionInfo) -> Result<Vec<u8>, IoError> {
    write_xlsx_named_with(wb, kind, None, functions)
}

/// The `<workbookView>` attributes beyond `activeTab`: the window's display settings
/// that differ from the format's defaults. An ordinary workbook gets none, so its file
/// is unchanged; a workbook Excel saved with the sheet tabs or scroll bars hidden
/// keeps them that way (#3057).
fn workbook_view_attrs(wb: &Workbook) -> String {
    let mut attrs = String::new();
    if !wb.view.show_sheet_tabs {
        attrs.push_str(" showSheetTabs=\"0\"");
    }
    if !wb.view.show_horizontal_scroll {
        attrs.push_str(" showHorizontalScroll=\"0\"");
    }
    if !wb.view.show_vertical_scroll {
        attrs.push_str(" showVerticalScroll=\"0\"");
    }
    let tab_ratio = wb.view.tab_ratio.min(WorkbookView::MAX_TAB_RATIO);
    if tab_ratio != 600 {
        let _ = write!(attrs, " tabRatio=\"{tab_ratio}\"");
    }
    attrs
}

/// Writes the workbook for the file `book_name` (a bare file name). A PivotChart is bound to its
/// PivotTable in Excel only when its link starts with the workbook in brackets.
pub fn write_xlsx_named_with(wb: &Workbook, kind: XlsxKind, book_name: Option<&str>, functions: FunctionInfo) -> Result<Vec<u8>, IoError> {
    if wb.sheets.is_empty() {
        return Err(IoError::Format("a workbook needs at least one sheet".into()));
    }
    if wb.sheets.iter().any(|sheet| sheet.cell_pictures.keys().any(|c| sheet.cell(*c).is_some_and(|cell| cell.formula.is_some()))) {
        return Err(IoError::Format("a static cell picture cannot also contain a formula".into()));
    }
    check_sheet_names(wb)?;
    let mut out = Out {
        parts: vec![],
        overrides: vec![],
        defaults: BTreeMap::new(),
        dxfs: vec![],
        num_fmts: Default::default(),
        x14_cf_rules: 0,
        sst: Sst::default(),
        images: 0,
        media: HashMap::new(),
        charts: 0,
        book_name: book_name.filter(|n| !n.is_empty() && !n.contains(['[', ']'])).unwrap_or(DEFAULT_BOOK_NAME).to_string(),
        chart_names: vec![],
        drawings: 0,
        tables: 0,
        comments: 0,
        vmls: 0,
        vml_block: 0,
        ctrl_props: 0,
        persons: vec![],
        table_id: 0,
        dynamic_arrays: false,
        needs_calc: false,
        cell_pictures: Default::default(),
        kept: Default::default(),
        functions,
        names: Default::default(),
        python_scripts: vec![],
    };
    out.kept.reserve_vml_blocks(wb);
    out.default_type("rels", "application/vnd.openxmlformats-package.relationships+xml");
    out.default_type("xml", "application/xml");

    // Which sheet is active (must be visible), and at least one visible sheet.
    let active = if wb.sheets.get(wb.active_sheet()).is_some_and(|s| s.visibility == Visibility::Visible) {
        wb.active_sheet()
    } else {
        wb.sheets.iter().position(|s| s.visibility == Visibility::Visible).unwrap_or(0)
    };

    let mut wb_rels = Rels::default();
    let (sheets_xml, kept, pivots) = write_sheets(wb, &mut out, active, &mut wb_rels);

    let names = write_defined_names(wb, &kept, &out.chart_names);

    let mut w = format!("{XML_DECL}<workbook xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_REL}\">");
    // A workbook read from a file keeps its <fileVersion> as it was: Excel trusts the saved values
    // only when appName is "xl" (#799). One without names GridCraft (#1699), and Excel recalculates it.
    let file_version = crate::passthrough::workbook_elements(wb, "fileVersion");
    w.push_str(if file_version.is_empty() { "<fileVersion appName=\"gridcraft\"/>" } else { &file_version });
    w.push_str("<workbookPr");
    match wb.show_objects {
        ShowObjects::All => {}
        ShowObjects::Placeholders => w.push_str(" showObjects=\"placeholders\""),
        ShowObjects::None => w.push_str(" showObjects=\"none\""),
    }
    if wb.date_system == gridcraft_core::DateSystem::D1904 {
        w.push_str(" date1904=\"1\"");
    }
    if wb.remove_personal_info {
        w.push_str(" filterPrivacy=\"1\"");
    }
    w.push_str("/>");
    if wb.protected_structure {
        let pw = wb.structure_password.as_ref().map(|h| crate::sheet_write::password_attrs(h, "workbook")).unwrap_or_default();
        let _ = write!(w, "<workbookProtection{pw} lockStructure=\"1\"/>");
    }
    let _ = write!(w, "<bookViews><workbookView activeTab=\"{}\"", kept.file_index(active));
    w.push_str(&workbook_view_attrs(wb));
    let _ = write!(w, "/></bookViews><sheets>{sheets_xml}</sheets>");
    w.push_str(&kept.elements);
    if !names.is_empty() {
        let _ = write!(w, "<definedNames>{names}</definedNames>");
    }
    w.push_str("<calcPr calcId=\"191029\"");
    match wb.calc.mode {
        CalcMode::Automatic => {}
        CalcMode::Manual => w.push_str(" calcMode=\"manual\""),
        CalcMode::AutomaticExceptTables => w.push_str(" calcMode=\"autoNoTable\""),
    }
    // CT_CalcPr defaults: iterate off, 100 iterations, 0.001 maximum change. The limits are kept
    // while iteration is off, as Options keeps them.
    let calc = &wb.calc;
    if calc.iterative {
        w.push_str(" iterate=\"1\"");
    }
    if calc.iterative || calc.max_iterations != CalcSettings::DEFAULT_ITERATIONS {
        let _ = write!(w, " iterateCount=\"{}\"", calc.max_iterations);
    }
    if calc.iterative || calc.max_change != CalcSettings::DEFAULT_CHANGE {
        let _ = write!(w, " iterateDelta=\"{}\"", crate::xml::num(calc.max_change));
    }
    if wb.calc.precision_as_displayed {
        w.push_str(" fullPrecision=\"0\"");
    }
    if wb.calc.r1c1 {
        w.push_str(" refMode=\"R1C1\"");
    }
    if out.needs_calc {
        w.push_str(" fullCalcOnLoad=\"1\"");
    }
    w.push_str("/>");
    w.push_str(&pivots.workbook_xml(&mut wb_rels));
    let exts = crate::passthrough::workbook_elements(wb, "ext");
    if !exts.is_empty() {
        let _ = write!(w, "<extLst>{exts}</extLst>");
    }
    w.push_str("</workbook>");

    // Workbook-level parts.
    let (styles, fmt_ids) = crate::styles::write_styles(wb, &out.dxfs, &pivots.number_formats(wb), std::mem::take(&mut out.num_fmts));
    pivots.finish(wb, &mut out, &|code: &str| crate::tables::builtin_id(code).or_else(|| fmt_ids.get(code).copied()));
    out.part("xl/styles.xml", Some("application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"), styles.into_bytes());
    wb_rels.add("styles", "styles.xml");
    if !out.python_scripts.is_empty() {
        use crate::python_part as py;
        let part = py::write(&out.python_scripts);
        out.part(py::PART, Some(py::CONTENT_TYPE), part.into_bytes());
        wb_rels.add_uri(py::REL, "python.xml");
    }
    if wb.styles.iter().any(|(_, st)| st.num_fmt.as_str() == "checkbox") {
        use crate::feature_bags as fpb;
        out.part(fpb::PART, Some(fpb::CONTENT_TYPE), fpb::checkbox_part().into_bytes());
        wb_rels.add_uri(fpb::REL, "featurePropertyBag/featurePropertyBag.xml");
    }
    out.part(
        "xl/theme/theme1.xml",
        Some("application/vnd.openxmlformats-officedocument.theme+xml"),
        crate::theme::write_theme(&wb.theme).into_bytes(),
    );
    wb_rels.add("theme", "theme/theme1.xml");
    // Phonetic (furigana) data is kept per string text: the table below holds each text once, so
    // the runs of the entry with that text are written next to it (`Workbook::phonetics`).
    let phonetics: HashMap<&str, &PhoneticString> = wb.phonetics.iter().map(|p| (p.text.as_str(), p)).collect();
    let mut sst = format!("{XML_DECL}<sst xmlns=\"{NS_MAIN}\" count=\"{}\" uniqueCount=\"{}\">", out.sst.count, out.sst.list.len());
    for t in &out.sst.list {
        let _ = write!(sst, "<si>{}</si>", crate::xml::si_body(t, phonetics.get(t.as_str()).copied()));
    }
    sst.push_str("</sst>");
    out.part("xl/sharedStrings.xml", Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"), sst.into_bytes());
    wb_rels.add("sharedStrings", "sharedStrings.xml");
    if !out.cell_pictures.is_empty() {
        crate::richdata::write(&mut out, &mut wb_rels)?;
    } else if out.dynamic_arrays {
        out.part(
            "xl/metadata.xml",
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml"),
            METADATA.as_bytes().to_vec(),
        );
        wb_rels.add("sheetMetadata", "metadata.xml");
    }
    if !out.persons.is_empty() {
        let mut x = format!("{XML_DECL}<personList xmlns=\"{NS_THREADED}\" xmlns:x=\"{NS_MAIN}\">");
        for (i, name) in out.persons.iter().enumerate() {
            let n = esc_attr(name);
            let _ = write!(x, "<person displayName=\"{n}\" id=\"{}\" userId=\"{n}\" providerId=\"None\"/>", person_guid(i));
        }
        x.push_str("</personList>");
        out.part("xl/persons/person.xml", Some(CT_PERSONS), x.into_bytes());
        wb_rels.add_uri(REL_PERSON, "persons/person.xml");
    }
    out.part("xl/workbook.xml", Some(kind.main_content_type()), w.into_bytes());
    // A macro-free package can't hold a VBA project (Excel refuses such a file).
    if let Some(data) = wb.vba_project.as_ref().filter(|_| kind.is_macro_enabled()) {
        wb_rels.add("vbaProject", "vbaProject.bin");
        out.part("xl/vbaProject.bin", Some(CT_VBA), data.to_vec());
    }
    out.part("xl/_rels/workbook.xml.rels", None, wb_rels.xml().into_bytes());

    // Document properties.
    let p = &wb.props;
    // "Remove personal information on save": the author and company are not saved (#3089).
    let private = |v: &String| if wb.remove_personal_info { String::new() } else { v.clone() };
    let (author, company) = (private(&p.author), private(&p.company));
    let mut core = format!(
        "{XML_DECL}<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:dcmitype=\"http://purl.org/dc/dcmitype/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">"
    );
    for (tag, v) in [
        ("dc:title", &p.title),
        ("dc:subject", &p.subject),
        ("dc:creator", &author),
        ("cp:keywords", &p.keywords),
        ("dc:description", &p.description),
    ] {
        if !v.is_empty() {
            let _ = write!(core, "<{tag}>{}</{tag}>", esc(v));
        }
    }
    core.push_str("</cp:coreProperties>");
    out.part("docProps/core.xml", Some("application/vnd.openxmlformats-package.core-properties+xml"), core.into_bytes());
    let mut app = String::from(XML_DECL);
    app.push_str("<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\" xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\"><Application>GridCraft</Application>");
    if !company.is_empty() {
        let _ = write!(app, "<Company>{}</Company>", esc(&company));
    }
    app.push_str("</Properties>");
    out.part("docProps/app.xml", Some("application/vnd.openxmlformats-officedocument.extended-properties+xml"), app.into_bytes());
    let mut root = Rels::default();
    root.add("officeDocument", "xl/workbook.xml");
    root.add("core-properties", "docProps/core.xml");
    root.add("extended-properties", "docProps/app.xml");
    crate::passthrough::write_package_rels(&mut root, &mut out);
    out.part("_rels/.rels", None, root.xml().into_bytes());

    // Content types first, then the parts in creation order.
    let mut ct = format!("{XML_DECL}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">");
    for (ext, t) in &out.defaults {
        // Kept parts bring their content types (and extensions) from the input file: escape them.
        let _ = write!(ct, "<Default Extension=\"{}\" ContentType=\"{}\"/>", esc_attr(ext), esc_attr(t));
    }
    for (part, t) in &out.overrides {
        let _ = write!(ct, "<Override PartName=\"{}\" ContentType=\"{}\"/>", esc_attr(part), esc_attr(t));
    }
    ct.push_str("</Types>");
    let mut zip = ZipOut::new();
    zip.add("[Content_Types].xml", ct.as_bytes())?;
    // Excel's order: rels, workbook rels, workbook, the rest, document properties last. libmagic (PHP
    // `finfo`, `file`) recognises an OOXML package only when an `xl/` entry name appears within the
    // first few KB, so no user-sized part (document properties, a long description) may precede it.
    let rank = |n: &str| match n {
        "_rels/.rels" => 0,
        "xl/_rels/workbook.xml.rels" => 1,
        "xl/workbook.xml" => 2,
        n if n.starts_with("docProps/") => 4,
        _ => 3,
    };
    let mut parts = std::mem::take(&mut out.parts);
    parts.sort_by_key(|(n, _)| rank(n));
    for (name, data) in &parts {
        zip.add(name, data)?;
    }
    zip.finish()
}

fn write_sheets(
    wb: &Workbook,
    out: &mut Out,
    active: usize,
    wb_rels: &mut Rels,
) -> (String, crate::passthrough::WorkbookKept, crate::pivot::PivotWriter) {
    // Kept content first: the chart sheets among the worksheets need their relationship ids.
    let kept = crate::passthrough::write_workbook(wb, wb_rels, out);
    let mut pivots = crate::pivot::PivotWriter::default();
    let mut sheets_xml = String::new();
    let any_visible = wb.sheets.iter().any(|s| s.visibility == Visibility::Visible);
    let mut ids = crate::passthrough::SheetIds::new(wb, &kept);
    for (i, sheet) in wb.sheets.iter().enumerate() {
        sheets_xml.push_str(&kept.sheet_entries(i, false, &mut ids));
        let (xml, mut rels) = crate::sheet_write::write_sheet(wb, i, i == active, out);
        pivots.add_sheet(wb, i, out, &mut rels);
        let name = format!("xl/worksheets/sheet{}.xml", i + 1);
        out.part(&name, Some(CT_WORKSHEET), xml.into_bytes());
        if !rels.is_empty() {
            out.part(&format!("xl/worksheets/_rels/sheet{}.xml.rels", i + 1), None, rels.xml().into_bytes());
        }
        let rid = wb_rels.add("worksheet", &format!("worksheets/sheet{}.xml", i + 1));
        let state = match sheet.visibility {
            Visibility::Visible => "",
            _ if !any_visible && i == active => "",
            Visibility::Hidden => " state=\"hidden\"",
            Visibility::VeryHidden => " state=\"veryHidden\"",
        };
        let name: String = sheet.name.chars().take(31).collect();
        let sheet_id = ids.take(sheet.file_id);
        let _ = write!(sheets_xml, "<sheet name=\"{}\" sheetId=\"{sheet_id}\"{state} r:id=\"{rid}\"/>", esc_attr(&name));
    }
    sheets_xml.push_str(&kept.sheet_entries(wb.sheets.len(), true, &mut ids));

    (sheets_xml, kept, pivots)
}

fn write_defined_names(wb: &Workbook, kept: &crate::passthrough::WorkbookKept, chart_names: &[String]) -> String {
    // Defined names, then print areas and titles.
    let mut names = String::new();
    let mut seen = std::collections::HashSet::new();
    for n in &wb.names {
        if n.name.is_empty() || !seen.insert((n.name.to_ascii_lowercase(), n.scope)) {
            continue;
        }
        if let Some(sc) = n.scope
            && sc >= wb.sheets.len()
        {
            continue;
        }
        let _ = write!(names, "<definedName name=\"{}\"", esc_attr(&n.name));
        if let Some(sc) = n.scope {
            let _ = write!(names, " localSheetId=\"{}\"", kept.file_index(sc));
        }
        if n.hidden {
            names.push_str(" hidden=\"1\"");
        }
        if !n.comment.is_empty() {
            let _ = write!(names, " comment=\"{}\"", esc_attr(&n.comment));
        }
        let _ = write!(names, ">{}</definedName>", esc(&crate::fmla::text_to_file(&n.formula)));
    }
    for (i, f) in chart_names.iter().enumerate() {
        let _ = write!(
            names,
            "<definedName name=\"{}{i}\" hidden=\"1\">{}</definedName>",
            crate::chartex::NAME_PREFIX,
            esc(&crate::fmla::text_to_file(f))
        );
    }
    for (i, sheet) in wb.sheets.iter().enumerate() {
        let q = quote_sheet(&sheet.name);
        if let Some(pa) = &sheet.print.print_area {
            let _ = write!(
                names,
                "<definedName name=\"_xlnm.Print_Area\" localSheetId=\"{}\">{}!{}</definedName>",
                kept.file_index(i),
                esc(&q),
                abs_range(pa)
            );
        }
        let mut titles = vec![];
        if let Some((c0, c1)) = sheet.print.title_cols {
            titles.push(format!("{q}!{}", abs_range(&RangeRef::cols(c0.min(c1), c0.max(c1)))));
        }
        if let Some((r0, r1)) = sheet.print.title_rows {
            titles.push(format!("{q}!{}", abs_range(&RangeRef::rows(r0.min(r1), r0.max(r1)))));
        }
        if !titles.is_empty() {
            let _ = write!(
                names,
                "<definedName name=\"_xlnm.Print_Titles\" localSheetId=\"{}\">{}</definedName>",
                kept.file_index(i),
                esc(&titles.join(","))
            );
        }
    }

    names
}

/// Cell metadata declaring dynamic-array formulas (MS-XLSX `XLDAPR`).
const METADATA: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<metadata xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:xda=\"http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray\"><metadataTypes count=\"1\"><metadataType name=\"XLDAPR\" minSupportedVersion=\"120000\" copy=\"1\" pasteAll=\"1\" pasteValues=\"1\" merge=\"1\" splitFirst=\"1\" rowColShift=\"1\" clearFormats=\"1\" clearComments=\"1\" assign=\"1\" coerce=\"1\" cellMeta=\"1\"/></metadataTypes><futureMetadata name=\"XLDAPR\" count=\"1\"><bk><extLst><ext uri=\"{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}\"><xda:dynamicArrayProperties fDynamic=\"1\" fCollapsed=\"0\"/></ext></extLst></bk></futureMetadata><cellMetadata count=\"1\"><bk><rc t=\"1\" v=\"0\"/></bk></cellMetadata></metadata>";
/// Writes a workbook with a caller-supplied file name and the default function facts.
pub fn write_xlsx_named(wb: &Workbook, kind: XlsxKind, book_name: Option<&str>) -> Result<Vec<u8>, IoError> {
    write_xlsx_named_with(wb, kind, book_name, FunctionInfo::default())
}
