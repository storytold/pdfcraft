//! Reading the workbook part and everything hanging off it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gridcraft_core::{DateSystem, RangeRef};
use gridcraft_model::{
    CalcMode, CellPicture, DefinedName, PhoneticString, Sheet, ShowObjects, Style, StyleId, StyleTable, Visibility, Workbook, WorkbookView,
};

use crate::package::{Package, Rel};
use crate::styles::{StylesIn, read_styles};
use crate::xml::{self, El};
use crate::{IoError, ReadReport};

/// State shared while reading one package.
pub struct Ctx<'a> {
    pub pkg: Package<'a>,
    pub report: ReadReport,
    pub sst: Vec<Arc<str>>,
    /// Phonetic (furigana) data of the shared strings, one entry per base text.
    pub phonetics: Vec<PhoneticString>,
    /// Cell format index → model style.
    pub xf_map: Vec<StyleId>,
    pub dxfs: Vec<Style>,
    pub palette: Vec<u32>,
    pub date_system: DateSystem,
    pub styles: StyleTable,
    /// Threaded-comment person id → display name.
    pub persons: std::collections::HashMap<String, String>,
    pub next_id: u32,
    /// Custom number formats from the styles part, by id.
    pub num_fmts: std::collections::HashMap<u32, String>,
    /// Value metadata blocks, indexed by the worksheet's one-based `vm` attribute.
    pub cell_pictures: Vec<Option<Arc<CellPicture>>>,
    /// x14 conditional-format ids of the current sheet's data bars, with the index of the rule in `cond_formats`.
    pub cf_ext_ids: Vec<(String, usize)>,
    /// Cells of the worksheet being read whose formulas lack dynamic-array metadata (legacy
    /// formulas, evaluated with implicit intersection), collected per sheet.
    pub legacy_formulas: Vec<gridcraft_core::CellRef>,
    /// Bytes charged so far for shared formulas expanded into dependent cells (see
    /// [`crate::sheet_read::MAX_SHARED_FORMULA_BYTES`]).
    pub shared_formula_bytes: u64,
    /// Whether GridCraft calculates a function, when the caller says: cell formulas wrapped by
    /// Google Sheets' export are then read as the formula inside (see [`crate::read_xlsx_with`]).
    pub known_function: Option<&'a dyn Fn(&str) -> bool>,
    /// The code of the workbook's Python cells, from its Python part.
    pub python: crate::python_part::ScriptsIn,
    /// Cell elements read so far, across every sheet (capped by `sheet_read::MAX_CELLS`).
    pub cells: u64,
}

/// The most worksheets read from one workbook (as the XLS reader allows).
const MAX_SHEETS: usize = 4096;
/// The most shared strings read from one workbook (as the XLS reader allows): each is an
/// `Arc<str>` of its own, so a hundred million tiny `<si/>` would ask for gigabytes.
const MAX_STRINGS: usize = 4_000_000;

/// Sheet names taken while reading, compared as `Workbook::sheet_index` compares them (ASCII case
/// folded), with the next `(n)` suffix to try for each base, so renaming duplicates stays linear.
#[derive(Default)]
pub(crate) struct SheetNames {
    taken: HashSet<String>,
    next_suffix: HashMap<String, usize>,
}

impl SheetNames {
    /// Records `name` as taken.
    pub(crate) fn insert(&mut self, name: &str) {
        self.taken.insert(name.to_ascii_lowercase());
    }

    /// A valid name for the `i`-th sheet based on `name`, not yet taken; it is recorded as taken.
    pub(crate) fn unique(&mut self, name: &str, i: usize) -> String {
        let mut base: String = name.chars().filter(|c| !matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']')).take(31).collect();
        if base.trim().is_empty() {
            base = format!("Sheet{}", i + 1);
        }
        if self.taken.insert(base.to_ascii_lowercase()) {
            return base;
        }
        let key: String = base.chars().take(25).collect::<String>().to_ascii_lowercase();
        let mut n = self.next_suffix.get(&key).copied().unwrap_or(2);
        loop {
            // 25 characters of the base, fewer once the suffix grows past ` (999)`, so the name
            // stays within 31 characters however many duplicates there are.
            let suffix = format!(" ({n})");
            let short: String = base.chars().take(31usize.saturating_sub(suffix.len()).min(25)).collect();
            let candidate = format!("{short}{suffix}");
            n = n.saturating_add(1);
            if self.taken.insert(candidate.to_ascii_lowercase()) {
                self.next_suffix.insert(key, n);
                return candidate;
            }
            if n == usize::MAX {
                return base;
            }
        }
    }
}

impl Ctx<'_> {
    pub fn warn(&mut self, m: impl Into<String>) {
        self.report.warn(m);
    }
    /// Stores the phonetic data read from `xl/sharedStrings.xml`.
    ///
    /// One entry per base text: the writer rebuilds the shared string table from the values the
    /// sheets use and holds each text once (the format's `uniqueCount`), so only one reading can be
    /// written per text. A later `<si>` with a text that is already there is dropped — with a
    /// warning when its data differs, so nothing is lost silently.
    pub fn keep_phonetics(&mut self, list: Vec<PhoneticString>) {
        let mut kept: Vec<PhoneticString> = Vec::new();
        let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut conflicts = 0usize;
        for p in list {
            match index.get(p.text.as_str()) {
                Some(&i) => {
                    if kept.get(i).is_some_and(|k| *k != p) {
                        conflicts += 1;
                    }
                }
                None => {
                    index.insert(p.text.clone(), kept.len());
                    kept.push(p);
                }
            }
        }
        if conflicts > 0 {
            self.warn(format!("shared strings: {conflicts} phonetic readings were dropped because another <si> with the same text already had one"));
        }
        self.phonetics = kept;
    }
    /// A fresh id for a chart, picture, shape or table.
    pub fn next_object_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }
    pub fn style(&self, xf: Option<u32>) -> StyleId {
        xf.and_then(|i| self.xf_map.get(i as usize).copied()).unwrap_or_default()
    }
    /// Reads and parses an optional part: problems other than size limits become warnings.
    pub fn optional_xml(&mut self, part: &str) -> Result<Option<El>, IoError> {
        match self.pkg.read_xml(part) {
            Ok(Some(e)) => Ok(Some(e)),
            Ok(None) => {
                self.warn(format!("missing part {part}"));
                Ok(None)
            }
            Err(IoError::TooLarge(m)) => Err(IoError::TooLarge(m)),
            Err(e) => {
                self.warn(format!("skipped {part}: {e}"));
                Ok(None)
            }
        }
    }
}

pub fn find_rel<'r>(rels: &'r [Rel], kind: &str) -> Option<&'r Rel> {
    rels.iter().find(|r| r.kind == kind)
}

fn read_shared_strings(cx: &mut Ctx<'_>, wb_rels: &[Rel]) -> Result<(), IoError> {
    // Shared strings, streamed item by item. Phonetic (furigana) runs are kept next to the text
    // they annotate, so an open + save does not delete them.
    if let Some(r) = find_rel(wb_rels, "sharedStrings").cloned() {
        match cx.pkg.read(&r.target)? {
            Some(b) => {
                let mut sst = Vec::new();
                let mut phonetics = Vec::new();
                let mut dropped = 0usize;
                let res = xml::parse_streaming(&b, &["si"], &mut |si| {
                    if sst.len() >= MAX_STRINGS {
                        return Err(IoError::TooLarge(format!("the workbook has more than {MAX_STRINGS} shared strings")));
                    }
                    let text = xml::rich_text(&si);
                    let raw = si.kids("rPh").count();
                    let (runs, props) = xml::phonetic_data(&si, &text);
                    dropped += raw.saturating_sub(runs.len());
                    if !runs.is_empty() || props.is_some() {
                        phonetics.push(PhoneticString { text: text.clone(), runs, props });
                    }
                    sst.push(Arc::from(text));
                    Ok(())
                });
                match res {
                    Ok(_) => {}
                    Err(IoError::TooLarge(m)) => return Err(IoError::TooLarge(m)),
                    Err(e) => cx.warn(format!("shared strings are damaged ({e}); kept {} strings", sst.len())),
                }
                if dropped > 0 {
                    cx.warn(format!("shared strings: {dropped} phonetic runs were dropped (bad sb/eb or no reading)"));
                }
                cx.sst = sst;
                cx.keep_phonetics(phonetics);
            }
            None => cx.warn(format!("missing part {}", r.target)),
        }
    }

    Ok(())
}

fn read_workbook_styles(cx: &mut Ctx<'_>, wb: &mut Workbook, wb_rels: &[Rel]) -> Result<(), IoError> {
    // Theme, then styles: a font's link to the theme's body font is read against it.
    if let Some(r) = find_rel(wb_rels, "theme").cloned()
        && let Some(x) = cx.optional_xml(&r.target)?
    {
        wb.theme = crate::theme::read_theme(&x);
    }
    // Cell checkboxes live in the feature property bags, which styles refer to.
    let mut checkboxes = Vec::new();
    if let Some(r) = find_rel(wb_rels, "FeaturePropertyBag").cloned()
        && let Some(x) = cx.optional_xml(&r.target)?
    {
        checkboxes = crate::feature_bags::checkbox_complements(&x);
    }
    if let Some(r) = find_rel(wb_rels, "styles").cloned() {
        if let Some(x) = cx.optional_xml(&r.target)? {
            let StylesIn { xfs, dxfs, palette, named, num_fmts } = read_styles(&x, &checkboxes, &wb.theme.minor_font);
            cx.num_fmts = num_fmts;
            // Cell format 0 is the Normal cell format: everything on it (font, number format,
            // fill, borders, alignment, protection) is what a cell without its own format shows
            // (#1013).
            if let Some(normal) = xfs.first() {
                cx.styles = StyleTable::with_default(normal.clone());
            }
            cx.xf_map = xfs.into_iter().enumerate().map(|(i, s)| if i == 0 { StyleId::DEFAULT } else { cx.styles.intern(s) }).collect();
            cx.dxfs = dxfs;
            cx.palette = palette;
            wb.cell_styles = named;
        }
    } else {
        cx.warn("no styles part");
    }
    Ok(())
}

/// Read the workbook XML selected by the package's root relationships.
fn workbook_xml(pkg: &mut Package<'_>, part: &str) -> Result<El, IoError> {
    let xml = match pkg.read_xml(part)? {
        Some(xml) => xml,
        None => {
            if pkg.has("xl/workbook.xml") && part != "xl/workbook.xml" {
                pkg.read_xml("xl/workbook.xml")?.ok_or_else(|| IoError::Format("no workbook part".into()))?
            } else if pkg.has("content.xml") {
                return Err(IoError::Format("this is an OpenDocument file, not XLSX".into()));
            } else {
                return Err(IoError::Format("no workbook part".into()));
            }
        }
    };
    if xml.name != "workbook" {
        return Err(IoError::Format(format!("unexpected root element <{}> in workbook part", xml.name)));
    }
    Ok(xml)
}

/// Read workbook-level settings before processing the individual sheets.
fn read_workbook_properties(cx: &mut Ctx<'_>, wb: &mut Workbook, xml: &El) {
    if let Some(p) = xml.child("workbookPr") {
        if p.flag("date1904", false) {
            cx.date_system = DateSystem::D1904;
        }
        wb.remove_personal_info = p.flag("filterPrivacy", false);
        wb.show_objects = match p.attr("showObjects") {
            Some("none") => ShowObjects::None,
            Some("placeholders") => ShowObjects::Placeholders,
            _ => ShowObjects::All,
        };
    }
    wb.date_system = cx.date_system;
    if let Some(c) = xml.child("calcPr") {
        wb.calc.mode = match c.attr("calcMode") {
            Some("manual") => CalcMode::Manual,
            Some("autoNoTable") => CalcMode::AutomaticExceptTables,
            _ => CalcMode::Automatic,
        };
        wb.calc.iterative = c.flag("iterate", false);
        if let Some(n) = c.attr_u32("iterateCount") {
            wb.calc.set_max_iterations(n);
        }
        if let Some(d) = c.attr_f64("iterateDelta") {
            wb.calc.set_max_change(d);
        }
        wb.calc.precision_as_displayed = !c.flag("fullPrecision", true);
        wb.calc.r1c1 = c.attr("refMode") == Some("R1C1");
    }
    if let Some(p) = xml.child("workbookProtection") {
        wb.protected_structure = p.flag("lockStructure", false);
        wb.structure_password = if wb.protected_structure { crate::sheet_read::read_password(p, "workbook") } else { None };
    }
}

/// The single window modelled by GridCraft: prefer the first visible window,
/// including the format's default visibility, and otherwise retain the first one.
fn selected_workbook_view(workbook: &El) -> Option<&El> {
    let views = workbook.child("bookViews")?;
    views.kids("workbookView").find(|view| view.attr("visibility").unwrap_or("visible") == "visible").or_else(|| views.child("workbookView"))
}

pub fn read_xlsx<'a>(bytes: &'a [u8], opts: crate::ReadOptions<'a>) -> Result<(Workbook, ReadReport), IoError> {
    let functions = opts.functions;
    let known_function = opts.known_function;
    let pkg = Package::open(bytes)?;
    let mut cx = Ctx {
        pkg,
        report: ReadReport::default(),
        sst: vec![],
        phonetics: vec![],
        xf_map: vec![],
        dxfs: vec![],
        palette: crate::tables::INDEXED.to_vec(),
        date_system: DateSystem::D1900,
        styles: StyleTable::default(),
        persons: Default::default(),
        next_id: 1,
        num_fmts: Default::default(),
        cell_pictures: vec![],
        cf_ext_ids: vec![],
        legacy_formulas: vec![],
        shared_formula_bytes: 0,
        known_function,
        python: Default::default(),
        cells: 0,
    };
    let root_rels = cx.pkg.rels("")?;
    let wb_part = find_rel(&root_rels, "officeDocument").map(|r| r.target.clone()).unwrap_or_else(|| "xl/workbook.xml".into());
    let wb_xml = workbook_xml(&mut cx.pkg, &wb_part)?;
    let wb_rels = cx.pkg.rels(&wb_part)?;
    let mut kept = crate::passthrough::KeptReader::new(&mut cx);
    cx.cell_pictures = match crate::richdata::read(&mut cx, &wb_rels) {
        Ok(pictures) => pictures,
        Err(error) => {
            cx.warn(format!("cell-picture data could not be read ({error}); scalar fallbacks were retained"));
            vec![]
        }
    };
    // The code of Python cells, which their formulas name by index.
    cx.python = crate::python_part::read(&mut cx, &wb_rels)?;
    let mut wb = Workbook::new();
    wb.sheets.clear();

    read_workbook_styles(&mut cx, &mut wb, &wb_rels)?;
    read_shared_strings(&mut cx, &wb_rels)?;

    if let Some(r) = find_rel(&wb_rels, "person").cloned()
        && let Ok(Some(x)) = cx.pkg.read_xml(&r.target)
    {
        for p in x.kids("person") {
            if let (Some(id), Some(n)) = (p.attr("id"), p.attr("displayName")) {
                cx.persons.insert(id.to_string(), n.to_string());
            }
        }
    }

    read_workbook_properties(&mut cx, &mut wb, &wb_xml);
    let view_el = selected_workbook_view(&wb_xml);
    let active_tab = view_el.and_then(|v| v.attr_u32("activeTab")).unwrap_or(0) as usize;
    if let Some(v) = view_el {
        // The window's display settings (Excel's "Display options for this workbook").
        // Absent attributes take the format's defaults, which is also what a workbook
        // without a <workbookView> gets.
        wb.view = WorkbookView {
            show_sheet_tabs: v.flag("showSheetTabs", true),
            show_horizontal_scroll: v.flag("showHorizontalScroll", true),
            show_vertical_scroll: v.flag("showVerticalScroll", true),
            tab_ratio: v.attr_u32("tabRatio").unwrap_or(600).min(WorkbookView::MAX_TAB_RATIO),
        };
    }

    // Sheets. `file_to_model[i]` maps the i-th <sheet> to a model sheet index.
    let mut file_to_model: Vec<Option<usize>> = vec![];
    // Legacy formula cells per model sheet, rewritten once names and tables are known.
    let mut legacy_formulas: Vec<(usize, Vec<gridcraft_core::CellRef>)> = vec![];
    let mut pivot_caches = crate::pivot::CachesIn::new(&wb_xml, &wb_rels);
    let sheet_els: Vec<&El> = wb_xml.child("sheets").map(|s| s.kids("sheet").collect()).unwrap_or_default();
    if sheet_els.len() > MAX_SHEETS {
        return Err(IoError::TooLarge(format!("the workbook has more than {MAX_SHEETS} sheets")));
    }
    let mut rel_by_id: HashMap<&str, &Rel> = HashMap::with_capacity(wb_rels.len());
    for r in &wb_rels {
        // The first of duplicate ids wins, as a linear search would find it.
        rel_by_id.entry(r.id.as_str()).or_insert(r);
    }
    let mut names = SheetNames::default();
    for (i, se) in sheet_els.iter().enumerate() {
        let name = se.attr("name").unwrap_or("").to_string();
        let rel = se.attr("id").and_then(|id| rel_by_id.get(id).copied()).cloned();
        let Some(rel) = rel else {
            cx.warn(format!("sheet \"{name}\" has no part; skipped"));
            file_to_model.push(None);
            continue;
        };
        if rel.kind == "chartsheet" {
            // Kept as it is and written back in place (see `passthrough`); nothing to model.
            file_to_model.push(None);
            continue;
        }
        if rel.kind != "worksheet" {
            cx.warn(format!("sheet \"{name}\" is a {} and was skipped", rel.kind));
            file_to_model.push(None);
            continue;
        }
        let name = names.unique(&name, i);
        cx.legacy_formulas.clear();
        let (mut sheet, read_controls) = match crate::sheet_read::read_sheet(&mut cx, &rel.target, &name) {
            Ok(s) => s,
            Err(IoError::TooLarge(m)) => return Err(IoError::TooLarge(m)),
            Err(e) => {
                cx.warn(format!("sheet \"{name}\" could not be read: {e}"));
                cx.legacy_formulas.clear();
                (Sheet::new(name.clone()), Default::default())
            }
        };
        legacy_formulas.push((wb.sheets.len(), std::mem::take(&mut cx.legacy_formulas)));
        match crate::pivot::read_pivots(&mut cx, &mut pivot_caches, &rel.target, &name) {
            Ok(p) => sheet.pivots = p,
            Err(IoError::TooLarge(m)) => return Err(IoError::TooLarge(m)),
            Err(e) => cx.warn(format!("PivotTables on sheet \"{name}\" could not be read: {e}")),
        }
        sheet.preserved = kept.sheet(&mut cx, &rel.target, &name, &read_controls);
        sheet.file_id = se.attr_u32("sheetId").unwrap_or(0);
        sheet.visibility = match se.attr("state") {
            Some("hidden") => Visibility::Hidden,
            Some("veryHidden") => Visibility::VeryHidden,
            _ => Visibility::Visible,
        };
        file_to_model.push(Some(wb.sheets.len()));
        wb.sheets.push(Arc::new(sheet));
    }
    if wb.sheets.is_empty() {
        cx.warn("the workbook has no worksheets; added an empty sheet");
        wb.sheets.push(Arc::new(Sheet::new("Sheet1")));
    }
    wb.set_active_sheet(file_to_model.get(active_tab).copied().flatten().unwrap_or(0));

    // Defined names. Chartex parts refer to their data through hidden `_xlchart.*` names.
    let mut chart_names: Vec<(String, String)> = vec![];
    if let Some(dn) = wb_xml.child("definedNames") {
        for d in dn.kids("definedName") {
            let Some(name) = d.attr("name") else { continue };
            let scope = match d.attr_u32("localSheetId") {
                Some(i) => match file_to_model.get(i as usize).copied().flatten() {
                    Some(m) => Some(m),
                    None => continue,
                },
                None => None,
            };
            let text = d.text.trim();
            let lname = name.to_ascii_lowercase();
            if lname == "_xlnm._filterdatabase" {
                continue;
            }
            if lname.starts_with("_xlchart.") {
                chart_names.push((lname, crate::fmla::from_file(&crate::chart::chart_ref_from_file(text))));
                continue;
            }
            if lname == "_xlnm.print_area" || lname == "_xlnm.print_titles" {
                if let Some(si) = scope
                    && let Some(sheet) = wb.sheet_mut(si)
                {
                    if lname == "_xlnm.print_area" {
                        sheet.print.print_area = parse_print_area(text);
                    } else {
                        let (rows, cols) = parse_print_titles(text);
                        sheet.print.title_rows = rows;
                        sheet.print.title_cols = cols;
                    }
                }
                continue;
            }
            wb.names.push(DefinedName {
                name: name.to_string(),
                scope,
                formula: crate::fmla::from_file(text),
                comment: d.attr("comment").unwrap_or("").to_string(),
                hidden: d.flag("hidden", false),
            });
        }
    }

    if !chart_names.is_empty() {
        let resolve = |f: &mut String| {
            if let Some((_, r)) = chart_names.iter().find(|(n, _)| f.eq_ignore_ascii_case(n)) {
                *f = r.clone();
            }
        };
        for sheet in wb.sheets.iter_mut().filter(|s| !s.charts.is_empty()) {
            for ch in Arc::make_mut(sheet).charts.iter_mut() {
                for s in ch.series.iter_mut() {
                    resolve(&mut s.values);
                    s.categories.iter_mut().for_each(&resolve);
                    s.name.iter_mut().for_each(&resolve);
                }
            }
        }
    }

    // Document properties.
    if let Some(r) = find_rel(&root_rels, "core-properties").cloned()
        && let Ok(Some(x)) = cx.pkg.read_xml(&r.target)
    {
        let get = |n: &str| x.child(n).map(|e| e.text.clone()).unwrap_or_default();
        wb.props.title = get("title");
        wb.props.subject = get("subject");
        wb.props.author = get("creator");
        wb.props.keywords = get("keywords");
        wb.props.description = get("description");
    }
    if let Some(r) = find_rel(&root_rels, "extended-properties").cloned()
        && let Ok(Some(x)) = cx.pkg.read_xml(&r.target)
    {
        wb.props.company = x.child("Company").map(|e| e.text.clone()).unwrap_or_default();
    }

    // A VBA project is carried through unchanged: GridCraft never runs macros, but preserving the
    // binary keeps macro-enabled workbooks from being silently stripped on save.
    if let Some(r) = find_rel(&wb_rels, "vbaProject").cloned() {
        match cx.pkg.read(&r.target) {
            Ok(Some(data)) => {
                wb.vba_project = Some(data.into());
                cx.warn("macros (VBA project) were kept but not executed");
            }
            Ok(None) => cx.warn("macros (VBA project) reference a missing part and were dropped"),
            Err(IoError::TooLarge(m)) => return Err(IoError::TooLarge(m)),
            Err(e) => cx.warn(format!("macros (VBA project) could not be read and were dropped: {e}")),
        }
    }
    wb.preserved = kept.workbook(&mut cx, &wb_part, &wb_xml, &file_to_model);
    if let Some(w) = cx.python.warning() {
        cx.warn(w);
    }
    // Calls of LAMBDA names are spelled like the names (the parser upper-cases them).
    wb.apply_name_call_case();
    // Legacy formulas read as dynamic-array formulas with Excel's implicit `@`.
    crate::array_formula::apply_legacy(&mut wb, legacy_formulas, functions);
    wb.styles = cx.styles;
    wb.phonetics = cx.phonetics;
    Ok((wb, cx.report))
}

/// Strips a sheet qualifier (`'My Sheet'!$A$1` → `$A$1`).
fn strip_sheet(s: &str) -> &str {
    match s.rfind('!') {
        Some(i) => s.get(i + 1..).unwrap_or(""),
        None => s,
    }
}

pub fn parse_print_area(text: &str) -> Option<RangeRef> {
    let first = text.split(',').next()?;
    let r = strip_sheet(first.trim()).replace('$', "");
    RangeRef::parse(&r)
}

/// A print-titles span: the first and last row (or column) repeated on each page.
type TitleSpan = Option<(u32, u32)>;

pub fn parse_print_titles(text: &str) -> (TitleSpan, TitleSpan) {
    let mut rows = None;
    let mut cols = None;
    for part in text.split(',') {
        let p = strip_sheet(part.trim()).replace('$', "");
        let Some((a, b)) = p.split_once(':') else { continue };
        if let (Ok(r0), Ok(r1)) = (a.parse::<u32>(), b.parse::<u32>()) {
            if r0 >= 1 && r1 >= r0 && r1 <= gridcraft_core::MAX_ROWS {
                rows = Some((r0 - 1, r1 - 1));
            }
        } else if let (Some(c0), Some(c1)) = (gridcraft_core::letters_to_col(a), gridcraft_core::letters_to_col(b))
            && c1 >= c0
        {
            cols = Some((c0, c1));
        }
    }
    (rows, cols)
}
