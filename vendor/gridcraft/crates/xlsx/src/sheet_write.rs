//! Writing worksheet parts and the parts they own (tables, comments, drawings, charts).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;

use craft_xlsx::{
    Break as CraftBreak, Cell as CraftCell, CellError as CraftCellError, CellFormula as CraftFormula, CellRef as CraftCellRef,
    CellValue as CraftValue, Column as CraftColumn, ConditionalFormatting as CraftCf, DataValidations as CraftDvs, Features as CraftFeatures,
    FormulaKind as CraftFormulaKind, HeaderFooter as CraftHf, Hyperlink as CraftHyperlink, PageMargins as CraftMargins, PageSetup as CraftPageSetup,
    Pane as CraftPane, Print as CraftPrint, PrintOptions as CraftPrintOptions, Range as CraftRange, Row as CraftRow, Selection as CraftSelection,
    SheetFormat as CraftSheetFormat, SheetPart as CraftSheetPart, SheetView as CraftSheetView,
};
use gridcraft_core::{CellError, CellRef, MAX_COLS, MAX_ROWS, RangeRef, Value};
use gridcraft_model::{
    Cell, CfOperator, CfRule, CfValueKind, Color, CondFormat, ErrorStyle, FilterCriterion, Formula, LineInfo, NoteBox, NoteCorner, Orientation,
    PageOrder, Sheet, SparklineKind, Style, Table, TotalsFn, Validation, ValidationKind, Workbook, default_icon_thresholds, icon_count,
};

use crate::drawing::{Obj, image_content_type, image_ext};
use crate::fmla::text_to_file;
use crate::styles::{color_attrs, dxf_id, dxf_xml};
use crate::tables::{paper_id, px_to_col_width, px_to_pt};
use crate::write::{
    CT_CHART, CT_COMMENTS, CT_DRAWING, CT_TABLE, CT_THREADED_COMMENTS, NS_MAIN, NS_REL, NS_THREADED, Out, REL_THREADED_COMMENT, Rels, XML_DECL,
    x14_cf_rule_guid,
};
use crate::xml::{encode_xstring, esc, esc_attr, num};

/// The worksheet extension holding Excel 2010 conditional formats.
const EXT_X14_CF: &str = "{78C0D931-6437-407d-A8EE-F0AAD7539E65}";

mod parts;
mod rules;

pub(crate) use parts::password_attrs;
use parts::{autofilter_xml, comments_xml, protected_ranges_xml, table_columns, table_name, table_xml, thread_ids, threaded_xml, vml_xml};
use rules::{cf_rule_xml, solid_bar_x14, validation_xml, x14_cf_rule_xml};

fn sqref(ranges: &[RangeRef]) -> String {
    ranges.iter().map(|r| r.a1()).collect::<Vec<_>>().join(" ")
}

fn to_craft_cell_ref(c: CellRef) -> CraftCellRef {
    CraftCellRef::new(c.row, c.col)
}

fn to_craft_range(r: RangeRef) -> CraftRange {
    CraftRange::new(to_craft_cell_ref(r.start), to_craft_cell_ref(r.end))
}

fn to_craft_error(e: CellError) -> CraftCellError {
    match e {
        CellError::Null => CraftCellError::Null,
        CellError::Div0 => CraftCellError::Div0,
        CellError::Value => CraftCellError::Value,
        CellError::Ref => CraftCellError::Ref,
        CellError::Name => CraftCellError::Name,
        CellError::Num => CraftCellError::Num,
        CellError::NA => CraftCellError::NA,
        CellError::GettingData => CraftCellError::GettingData,
        CellError::Spill => CraftCellError::Spill,
        CellError::Calc => CraftCellError::Calc,
        _ => CraftCellError::Value,
    }
}

fn parse_craft_fragment(xml: &str) -> Option<craft_xml::El> {
    let opts = craft_xml::Options::default().with_limits(craft_xml::Limits {
        max_input: crate::package::MAX_PART as usize,
        max_depth: crate::xml::MAX_DEPTH,
        max_nodes: crate::xml::MAX_XML_NODES,
        ..craft_xml::Limits::DEFAULT
    });
    opts.parse(xml.as_bytes()).ok()
}

fn push_fragment_kids(root: &mut craft_xml::El, xml: &str) {
    if xml.is_empty() {
        return;
    }
    let wrapped = format!("<root xmlns:r=\"{NS_REL}\">{xml}</root>");
    if let Some(parsed) = parse_craft_fragment(&wrapped) {
        root.children.extend(parsed.children);
    }
}

pub fn write_sheet(wb: &Workbook, si: usize, selected: bool, out: &mut Out) -> (String, Rels) {
    let mut rels = Rels::default();
    let Some(sheet) = wb.sheet(si) else { return (String::new(), rels) };
    let nstyles = wb.styles.len() as u32;
    let sid = |s: gridcraft_model::StyleId| if s.0 < nstyles { s.0 } else { 0 };

    let folded: Vec<Table> = match &sheet.autofilter {
        Some(af) if !af.criteria.is_empty() => sheet
            .tables
            .iter()
            .cloned()
            .map(|mut t| {
                if t.header_row
                    && t.filter_button
                    && t.filter.is_empty()
                    && t.range.contains_range(&af.range)
                    && t.range.start.col == af.range.start.col
                {
                    t.filter = af.criteria.clone();
                }
                t
            })
            .collect(),
        _ => sheet.tables.clone(),
    };
    let mut overrides: BTreeMap<CellRef, String> = BTreeMap::new();
    let tables: Vec<(&Table, Vec<String>)> = folded
        .iter()
        .filter(|t| t.range.height() > t.header_row as u32 + t.totals_row as u32 || (t.header_row && t.range.height() > t.totals_row as u32))
        .map(|t| (t, table_columns(t)))
        .collect();
    for (t, cols) in &tables {
        if t.header_row {
            for (i, name) in cols.iter().enumerate() {
                overrides.insert(CellRef::new(t.range.start.row, t.range.start.col + i as u32), name.clone());
            }
        }
    }
    let dynamic: BTreeMap<CellRef, RangeRef> = sheet
        .spill_ranges
        .iter()
        .filter(|(a, r)| {
            !r.is_single() && sheet.cells.get(**a).is_some_and(|c| c.py.is_some() || c.formula.as_ref().is_some_and(|f| f.array.is_none()))
        })
        .map(|(a, r)| (*a, *r))
        .collect();
    let mut extra: BTreeMap<CellRef, Value> = BTreeMap::new();
    for (a, r) in sheet.spill_ranges.iter().filter(|(_, r)| !r.is_single()) {
        for c in r.iter().take(1_000_000) {
            if c != *a
                && let Some(v) = sheet.spill.get(&c)
                && sheet.cells.get(c).is_none_or(|x| x.formula.is_none() && x.value.is_empty())
            {
                extra.insert(c, v.clone());
            }
        }
    }

    let mut part = CraftSheetPart { root: craft_xml::El::new("worksheet"), ..Default::default() };
    // sheetPr
    let fit = sheet.print.fit_to.is_some();
    let summary = !sheet.summary_below || !sheet.summary_right;
    if sheet.tab_color.is_some() || fit || summary {
        let mut pr = craft_xml::El::new("sheetPr");
        if let Some(c) = &sheet.tab_color {
            let xml = format!("<tabColor {}/>", color_attrs(c));
            if let Some(el) = parse_craft_fragment(&xml) {
                pr.push(el);
            }
        }
        if summary {
            let mut outline = craft_xml::El::new("outlinePr");
            if !sheet.summary_below {
                outline.set_attr("summaryBelow", "0");
            }
            if !sheet.summary_right {
                outline.set_attr("summaryRight", "0");
            }
            pr.push(outline);
        }
        if fit {
            pr.push(craft_xml::El::new("pageSetUpPr").with_attr("fitToPage", "1"));
        }
        part.root.push(pr);
    }

    let max_row_outline = sheet.rows.values().map(|r| r.outline).max().unwrap_or(0);
    let max_col_outline = sheet.cols.values().map(|r| r.outline).max().unwrap_or(0);
    part.features.format = Some(CraftSheetFormat {
        default_col_width: ((sheet.default_col_width - gridcraft_model::DEFAULT_COL_WIDTH).abs() > 0.01)
            .then(|| px_to_col_width(sheet.default_col_width)),
        default_row_height: Some(px_to_pt(sheet.default_row_height)),
        custom_height: ((sheet.default_row_height - gridcraft_model::DEFAULT_ROW_HEIGHT).abs() > 0.01).then_some(true),
        outline_level_row: (max_row_outline > 0).then_some(u32::from(max_row_outline)),
        outline_level_col: (max_col_outline > 0).then_some(u32::from(max_col_outline)),
        ..Default::default()
    });

    part.features.views = vec![build_craft_view(sheet, selected)];
    part.columns = build_craft_cols(sheet, &sid);
    part.merges = sheet.merges.iter().filter(|m| !m.is_single()).copied().map(to_craft_range).collect();

    let rows = build_craft_rows(SheetCellData { workbook: wb, index: si, sheet, overrides: &overrides, extra: &extra, dynamic: &dynamic }, &sid, out);

    let (rules_frag, x14_cf) = build_sheet_rules(sheet, &mut part.features, &mut rels, out);
    push_fragment_kids(&mut part.root, &rules_frag);

    let kept = build_sheet_printing(sheet, &mut part.features.print, &mut rels, out);
    part.features.drawing = write_sheet_drawing_rel(wb, si, sheet, &mut rels, out);
    let (legacy_rid, notes_frag) = write_sheet_notes_frag(sheet, si, &kept, &mut rels, out);
    part.features.legacy_drawing = legacy_rid;

    if !tables.is_empty() {
        let mut used_names: HashSet<String> = HashSet::new();
        for (t, cols) in &tables {
            out.tables += 1;
            out.table_id += 1;
            let n = out.tables;
            let name = table_name(&t.name, &mut used_names, out.table_id);
            let xml = table_xml(t, cols, &name, out.table_id, out);
            out.part(&format!("xl/tables/table{n}.xml"), Some(CT_TABLE), xml.into_bytes());
            part.features.table_parts.push(rels.add("table", &format!("../tables/table{n}.xml")));
        }
    }

    let mut ext_xml = String::new();
    write_sheet_extensions(&mut ext_xml, sheet, &x14_cf);
    push_fragment_kids(&mut part.root, &ext_xml);

    let mut xml = String::from_utf8(part.to_xml(&rows)).unwrap_or_default();
    if !notes_frag.is_empty() {
        let after_data = xml
            .find("</sheetData>")
            .map(|p| p + "</sheetData>".len())
            .or_else(|| xml.find("<sheetData/>").map(|p| p + "<sheetData/>".len()))
            .unwrap_or(0);
        let pos = ["<webPublishItems", "<tableParts", "<extLst", "</worksheet>"]
            .iter()
            .filter_map(|tag| xml[after_data..].find(tag).map(|p| after_data + p))
            .min()
            .unwrap_or(xml.len());
        xml.insert_str(pos, &notes_frag);
    }
    (xml, rels)
}

fn build_craft_cols(sheet: &Sheet, sid: &dyn Fn(gridcraft_model::StyleId) -> u32) -> Vec<CraftColumn> {
    let mut cols = Vec::new();
    let mut run: Option<(u32, u32, LineInfo)> = None;
    let push_run = |cols: &mut Vec<CraftColumn>, (a, b, i): (u32, u32, LineInfo)| {
        let w = i.size.unwrap_or(sheet.default_col_width);
        cols.push(CraftColumn {
            first: a,
            last: b,
            width: Some(px_to_col_width(w)),
            custom_width: i.size.is_some(),
            best_fit: false,
            hidden: i.hidden,
            outline_level: i.outline.min(7),
            collapsed: i.collapsed,
            style: i.style.map(sid),
        });
    };
    for (&c, info) in &sheet.cols {
        if c >= gridcraft_core::MAX_COLS {
            continue;
        }
        run = match run {
            Some((a, b, i)) if b + 1 == c && i == *info => Some((a, c, i)),
            Some(r) => {
                push_run(&mut cols, r);
                Some((c, c, *info))
            }
            None => Some((c, c, *info)),
        };
    }
    if let Some(r) = run {
        push_run(&mut cols, r);
    }
    cols
}

fn build_craft_view(sheet: &Sheet, selected: bool) -> CraftSheetView {
    let active = sheet.view_active.a1();
    let (pane, selections) = match sheet.freeze {
        Some((rows, cols)) if rows > 0 || cols > 0 => {
            let pane_name = match (rows > 0, cols > 0) {
                (true, true) => "bottomRight",
                (true, false) => "bottomLeft",
                _ => "topRight",
            };
            let top_left = CellRef::new(
                rows.max(sheet.view_top_left.row).min(gridcraft_core::MAX_ROWS - 1),
                cols.max(sheet.view_top_left.col).min(gridcraft_core::MAX_COLS - 1),
            )
            .a1();
            let pane = CraftPane {
                x_split: (cols > 0).then_some(f64::from(cols)),
                y_split: (rows > 0).then_some(f64::from(rows)),
                top_left_cell: Some(top_left),
                active_pane: Some(pane_name.to_string()),
                state: Some("frozen".to_string()),
                ..Default::default()
            };
            let sel = CraftSelection {
                pane: Some(pane_name.to_string()),
                active_cell: Some(active),
                active_cell_id: None,
                sqref: vec![to_craft_range(RangeRef::cell(sheet.view_active))],
                ..Default::default()
            };
            (Some(pane), vec![sel])
        }
        _ => {
            let sel = CraftSelection {
                pane: None,
                active_cell: Some(active),
                active_cell_id: None,
                sqref: vec![to_craft_range(RangeRef::cell(sheet.view_active))],
                ..Default::default()
            };
            (None, vec![sel])
        }
    };
    CraftSheetView {
        tab_selected: selected.then_some(true),
        show_grid_lines: (!sheet.show_gridlines).then_some(false),
        show_row_col_headers: (!sheet.show_headings).then_some(false),
        show_formulas: sheet.show_formulas.then_some(true),
        show_zeros: (!sheet.show_zeros).then_some(false),
        show_outline_symbols: (!sheet.show_outline_symbols).then_some(false),
        right_to_left: sheet.right_to_left.then_some(true),
        view: (sheet.view != gridcraft_model::SheetViewMode::Normal).then(|| sheet.view.as_str().to_string()),
        zoom_scale: (sheet.zoom != 100 && (10..=400).contains(&sheet.zoom)).then_some(u32::from(sheet.zoom)),
        zoom_scale_normal: (sheet.zoom != 100 && (10..=400).contains(&sheet.zoom)).then_some(u32::from(sheet.zoom)),
        top_left_cell: (sheet.view_top_left != CellRef::default() && sheet.freeze.is_none()).then(|| sheet.view_top_left.a1()),
        workbook_view_id: Some(0),
        pane,
        selections,
        ..Default::default()
    }
}

struct CellWriteInput<'a> {
    reference: CellRef,
    cell: Option<&'a Cell>,
    picture: Option<&'a std::sync::Arc<gridcraft_model::CellPicture>>,
    header_override: Option<&'a String>,
    spill_value: Option<&'a Value>,
    dynamic_range: Option<&'a RangeRef>,
}

fn write_sheet_extensions(s: &mut String, sheet: &Sheet, x14_cf: &str) {
    // Excel 2010 extensions: conditional formats, then sparklines.
    if !x14_cf.is_empty() || !sheet.sparklines.is_empty() {
        s.push_str("<extLst>");
    }
    if !x14_cf.is_empty() {
        let _ = write!(s, "<ext uri=\"{EXT_X14_CF}\" xmlns:x14=\"{X14_NS}\"><x14:conditionalFormattings>{x14_cf}</x14:conditionalFormattings></ext>");
    }
    if !sheet.sparklines.is_empty() {
        let _ = write!(s, "<ext uri=\"{{05C60535-1F16-4fd2-B633-F4F36F0B64E0}}\" xmlns:x14=\"{X14_NS}\"><x14:sparklineGroups xmlns:xm=\"{XM_NS}\">");
        for sp in &sheet.sparklines {
            let ty = match sp.kind {
                SparklineKind::Line => "",
                SparklineKind::Column => " type=\"column\"",
                SparklineKind::WinLoss => " type=\"stacked\"",
            };
            let src =
                if sp.source.contains('!') { sp.source.clone() } else { format!("{}!{}", gridcraft_formula::quote_sheet(&sheet.name), sp.source) };
            let color = if sp.color == Color::Auto { "rgb=\"FF376092\"".to_string() } else { color_attrs(&sp.color) };
            let _ = write!(
                s,
                "<x14:sparklineGroup displayEmptyCellsAs=\"gap\"{ty}{}><x14:colorSeries {color}/><x14:sparklines><x14:sparkline><xm:f>{}</xm:f><xm:sqref>{}</xm:sqref></x14:sparkline></x14:sparklines></x14:sparklineGroup>",
                if sp.markers { " markers=\"1\"" } else { "" },
                esc(&src),
                sp.cell.a1()
            );
        }
        s.push_str("</x14:sparklineGroups></ext>");
    }
    if !x14_cf.is_empty() || !sheet.sparklines.is_empty() {
        s.push_str("</extLst>");
    }
}

fn write_sheet_notes_frag(
    sheet: &Sheet,
    si: usize,
    kept: &crate::passthrough::SheetKept<'_>,
    rels: &mut Rels,
    out: &mut Out,
) -> (Option<String>, String) {
    let mut controls = String::new();
    let legacy_rid = if !sheet.comments.is_empty() || !sheet.controls.is_empty() {
        let shapes = (sheet.comments.len() as u64).saturating_add(sheet.controls.len() as u64);
        let blocks = u32::try_from(shapes.saturating_add(1).div_ceil(1024)).unwrap_or(u32::MAX).max(1);
        let first = out.kept.notes_block(out.vml_block.saturating_add(1));
        out.vml_block = out.vml_block.saturating_add(blocks);
        let idmap = (first..first.saturating_add(blocks)).map(|b| b.to_string()).collect::<Vec<_>>().join(",");
        let base = kept.shape_base().unwrap_or(u64::from(first) * 1024);
        let notes = sheet.comments.len() as u64;
        let mut control_shapes = String::new();
        if !sheet.controls.is_empty() {
            let w = crate::controls::write(out, rels, sheet, |i| base + 1 + notes + i as u64);
            controls = w.entries;
            control_shapes = format!("{}{}", crate::controls::SHAPETYPE, w.vml);
        }
        if !sheet.comments.is_empty() {
            out.comments += 1;
            let n = out.comments;
            let ids = thread_ids(sheet, si);
            out.part(&format!("xl/comments{n}.xml"), Some(CT_COMMENTS), comments_xml(sheet, &ids).into_bytes());
            if !ids.is_empty() {
                let x = threaded_xml(sheet, &ids, &mut out.persons);
                out.part(&format!("xl/threadedComments/threadedComment{n}.xml"), Some(CT_THREADED_COMMENTS), x.into_bytes());
                rels.add_uri(REL_THREADED_COMMENT, &format!("../threadedComments/threadedComment{n}.xml"));
            }
            rels.add("comments", &format!("../comments{n}.xml"));
        }
        out.vmls += 1;
        let v = out.vmls;
        out.default_type("vml", "application/vnd.openxmlformats-officedocument.vmlDrawing");
        let vml_name = format!("xl/drawings/vmlDrawing{v}.vml");
        let vml = kept.notes_vml(vml_xml(sheet, &idmap, base, &control_shapes), &vml_name, out);
        out.part(&vml_name, None, vml.into_bytes());
        Some(rels.add("vmlDrawing", &format!("../drawings/vmlDrawing{v}.vml")))
    } else {
        kept.lone_vml(rels, out)
    };
    (legacy_rid, kept.elements(&controls))
}

fn write_sheet_drawing_rel(wb: &Workbook, si: usize, sheet: &Sheet, rels: &mut Rels, out: &mut Out) -> Option<String> {
    if sheet.images.is_empty() && sheet.charts.is_empty() && sheet.shapes.is_empty() {
        return None;
    }
    out.drawings += 1;
    let dn = out.drawings;
    let mut drels = Rels::default();
    let mut objs: Vec<Obj<'_>> = vec![];
    for img in &sheet.images {
        if img.data.is_empty() {
            continue;
        }
        let ext = image_ext(&img.mime, &img.data);
        out.default_type(ext, image_content_type(ext));
        let file = out.image_part(ext, &img.data);
        let rid = drels.add("image", &format!("../media/{file}"));
        objs.push(Obj::Image(img, rid));
    }
    for ch in &sheet.charts {
        out.charts += 1;
        let rid = if crate::chartex::is_chartex(ch.kind) {
            let n = out.charts;
            let xml = crate::chartex::write_chartex(wb, si, ch, &mut out.chart_names);
            out.part(&format!("xl/charts/chartEx{n}.xml"), Some(crate::chartex::CT_CHARTEX), xml.into_bytes());
            out.part(&format!("xl/charts/style{n}.xml"), Some(crate::chartex::CT_STYLE), crate::chartex::style_xml().into_bytes());
            out.part(&format!("xl/charts/colors{n}.xml"), Some(crate::chartex::CT_COLORS), crate::chartex::colors_xml().into_bytes());
            let mut crels = Rels::default();
            crels.add_uri(crate::chartex::REL_STYLE, &format!("style{n}.xml"));
            crels.add_uri(crate::chartex::REL_COLORS, &format!("colors{n}.xml"));
            out.part(&format!("xl/charts/_rels/chartEx{n}.xml.rels"), None, crels.xml().into_bytes());
            drels.add_uri(crate::chartex::REL_CHARTEX, &format!("../charts/chartEx{n}.xml"))
        } else {
            let name = format!("xl/charts/chart{}.xml", out.charts);
            out.part(&name, Some(CT_CHART), crate::chart::write_chart(wb, si, ch, &out.book_name).into_bytes());
            drels.add("chart", &format!("../charts/chart{}.xml", out.charts))
        };
        objs.push(Obj::Chart(ch, rid));
    }
    for sh in &sheet.shapes {
        objs.push(Obj::Shape(sh));
    }
    out.part(&format!("xl/drawings/drawing{dn}.xml"), Some(CT_DRAWING), crate::drawing::write_drawing(sheet, &wb.theme, &objs).into_bytes());
    if !drels.is_empty() {
        out.part(&format!("xl/drawings/_rels/drawing{dn}.xml.rels"), None, drels.xml().into_bytes());
    }
    Some(rels.add("drawing", &format!("../drawings/drawing{dn}.xml")))
}

fn build_sheet_printing<'a>(sheet: &'a Sheet, print: &mut CraftPrint, rels: &mut Rels, out: &mut Out) -> crate::passthrough::SheetKept<'a> {
    let p = &sheet.print;
    if p.gridlines || p.headings || p.center_h || p.center_v {
        print.options = Some(CraftPrintOptions {
            horizontal_centered: p.center_h.then_some(true),
            vertical_centered: p.center_v.then_some(true),
            headings: p.headings.then_some(true),
            grid_lines: p.gridlines.then_some(true),
            grid_lines_set: None,
            ..Default::default()
        });
    }
    let kept = crate::passthrough::SheetKept::write(sheet, rels, out);
    let m = |i: usize| {
        let inches = p.margins.get(i).copied().unwrap_or(0.75);
        inches.to_string().parse::<f64>().unwrap_or(f64::from(inches))
    };
    print.margins = Some(CraftMargins {
        left: Some(m(0)),
        right: Some(m(1)),
        top: Some(m(2)),
        bottom: Some(m(3)),
        header: Some(m(4)),
        footer: Some(m(5)),
        ..Default::default()
    });
    print.setup = Some(CraftPageSetup {
        paper_size: paper_id(&p.paper).filter(|&id| id != 1),
        scale: (p.scale != 100 && (10..=400).contains(&p.scale)).then_some(u32::from(p.scale)),
        first_page_number: p.first_page_number,
        fit_to_width: p.fit_to.map(|(w, _)| u32::from(w)),
        fit_to_height: p.fit_to.map(|(_, h)| u32::from(h)),
        page_order: (p.page_order != PageOrder::DownThenOver).then(|| p.page_order.id().to_string()),
        orientation: Some(if p.orientation == Orientation::Landscape { "landscape" } else { "portrait" }.to_string()),
        use_first_page_number: p.first_page_number.map(|_| true),
        rel_id: kept.page_setup_id().map(str::to_string),
        ..Default::default()
    });
    if p.has_header_footer() || p.different_first || p.different_odd_even || !p.hf_scale_with_doc || !p.hf_align_with_margins {
        let opt = |t: &String| (!t.is_empty()).then(|| t.clone());
        print.header_footer = Some(CraftHf {
            different_odd_even: p.different_odd_even.then_some(true),
            different_first: p.different_first.then_some(true),
            scale_with_doc: (!p.hf_scale_with_doc).then_some(false),
            align_with_margins: (!p.hf_align_with_margins).then_some(false),
            odd_header: opt(&p.header),
            odd_footer: opt(&p.footer),
            even_header: opt(&p.even_header),
            even_footer: opt(&p.even_footer),
            first_header: opt(&p.first_header),
            first_footer: opt(&p.first_footer),
            ..Default::default()
        });
    }
    let to_breaks = |list: &[u32], max: u32| {
        list.iter().copied().filter(|&b| b > 0).map(|id| CraftBreak { id: Some(id), max: Some(max), man: Some(true), ..Default::default() }).collect()
    };
    print.row_breaks = to_breaks(&p.row_breaks, 16383);
    print.col_breaks = to_breaks(&p.col_breaks, 1048575);
    kept
}

fn build_sheet_rules(sheet: &Sheet, features: &mut CraftFeatures, rels: &mut Rels, out: &mut Out) -> (String, String) {
    let mut frag = String::new();
    if let Some(p) = &sheet.protection {
        frag.push_str("<sheetProtection sheet=\"1\" objects=\"1\" scenarios=\"1\"");
        for (attr, allowed) in [
            ("formatCells", p.format_cells),
            ("formatColumns", p.format_columns),
            ("formatRows", p.format_rows),
            ("insertColumns", p.insert_columns),
            ("insertRows", p.insert_rows),
            ("deleteColumns", p.delete_columns),
            ("deleteRows", p.delete_rows),
            ("sort", p.sort),
            ("autoFilter", p.autofilter),
        ] {
            if allowed {
                let _ = write!(frag, " {attr}=\"0\"");
            }
        }
        if !p.select_locked {
            frag.push_str(" selectLockedCells=\"1\"");
        }
        if !p.select_unlocked {
            frag.push_str(" selectUnlockedCells=\"1\"");
        }
        if let Some(h) = &p.password {
            frag.push_str(&password_attrs(h, ""));
        }
        frag.push_str("/>");
    }
    frag.push_str(&protected_ranges_xml(sheet));
    if let Some(af) = &sheet.autofilter
        && !sheet.tables.iter().any(|t| t.range.intersects(&af.range))
    {
        frag.push_str(&autofilter_xml(af.range, &af.criteria, out));
    }

    let mut order: Vec<usize> = (0..sheet.cond_formats.len()).collect();
    order.sort_by_key(|&i| sheet.cond_formats.get(i).map(|c| c.priority).unwrap_or(0));
    let mut prio = vec![0u32; sheet.cond_formats.len()];
    for (rank, &i) in order.iter().enumerate() {
        if let Some(p) = prio.get_mut(i) {
            *p = rank as u32 + 1;
        }
    }
    let mut x14_cf = String::new();
    for (i, cf) in sheet.cond_formats.iter().enumerate() {
        if cf.ranges.is_empty() {
            continue;
        }
        let priority = prio.get(i).copied().unwrap_or(1);
        let x14 = match x14_cf_rule_xml(cf, &sheet.name, priority, out) {
            Some(rule) => Some(rule),
            None => {
                let xml =
                    format!("<conditionalFormatting sqref=\"{}\">{}</conditionalFormatting>", sqref(&cf.ranges), cf_rule_xml(cf, priority, out));
                if let Some(el) = parse_craft_fragment(&xml) {
                    features.conditional_formats.push(CraftCf::read(&el));
                }
                solid_bar_x14(cf, priority)
            }
        };
        if let Some(rule) = x14 {
            let _ = write!(
                x14_cf,
                "<x14:conditionalFormatting xmlns:xm=\"{XM_NS}\">{rule}<xm:sqref>{}</xm:sqref></x14:conditionalFormatting>",
                sqref(&cf.ranges)
            );
        }
    }
    let vals: Vec<&Validation> = sheet.validations.iter().filter(|v| !v.ranges.is_empty()).collect();
    if !vals.is_empty() {
        let items = vals.iter().filter_map(|v| parse_craft_fragment(&validation_xml(v)).map(|el| craft_xlsx::DataValidation::read(&el))).collect();
        features.validations = Some(CraftDvs { items, ..Default::default() });
    }
    for (c, h) in &sheet.hyperlinks {
        let t = h.target.trim();
        if t.is_empty() {
            continue;
        }
        let external = h.external
            || t.contains("://")
            || t.to_ascii_lowercase().starts_with("mailto:")
            || t.starts_with("\\\\")
            || t.to_ascii_lowercase().starts_with("file:");
        let (rel_id, location, display, url) = if external {
            let (u, loc) = match t.split_once('#') {
                Some((u, l)) if !u.is_empty() => (u, Some(l.to_string())),
                _ => (t, None),
            };
            let id = rels.add_external("hyperlink", u);
            (Some(id), loc, None, Some(u.to_string()))
        } else {
            let loc = t.trim_start_matches('#').to_string();
            (None, Some(loc.clone()), Some(loc), None)
        };
        features.hyperlinks.push(CraftHyperlink {
            rel_id,
            location,
            tooltip: h.tooltip.clone(),
            display,
            range: to_craft_range(RangeRef::cell(*c)),
            url,
            ..Default::default()
        });
    }
    (frag, x14_cf)
}

struct SheetCellData<'a> {
    workbook: &'a Workbook,
    index: usize,
    sheet: &'a Sheet,
    overrides: &'a BTreeMap<CellRef, String>,
    extra: &'a BTreeMap<CellRef, Value>,
    dynamic: &'a BTreeMap<CellRef, RangeRef>,
}

fn build_craft_rows(data: SheetCellData<'_>, sid: &dyn Fn(gridcraft_model::StyleId) -> u32, out: &mut Out) -> Vec<CraftRow> {
    let SheetCellData { workbook, index, sheet, overrides, extra, dynamic } = data;
    let mut keys: Vec<(CellRef, Option<&Cell>)> = Vec::with_capacity(sheet.cells.len());
    keys.extend(sheet.cells.iter().map(|(c, cell)| (c, Some(cell))));
    if !sheet.cell_pictures.is_empty() || !extra.is_empty() || !overrides.is_empty() || !keys.is_sorted_by_key(|(c, _)| *c) {
        let mut all: BTreeSet<CellRef> = keys.iter().map(|(c, _)| *c).collect();
        all.extend(sheet.cell_pictures.keys().copied());
        all.extend(extra.keys().copied());
        all.extend(overrides.keys().copied());
        keys = all.into_iter().map(|c| (c, sheet.cells.get(c))).collect();
    }
    let mut rows = Vec::new();
    let mut row_infos = sheet.rows.iter().peekable();
    let mut next = 0;
    loop {
        let cell_row = keys.get(next).map(|(cell, _)| cell.row);
        let info_row = row_infos.peek().map(|(row, _)| **row);
        let row = match (cell_row, info_row) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => break,
        };
        let info = match row_infos.next_if(|(entry_row, _)| **entry_row == row) {
            Some((_, info)) => *info,
            None => LineInfo::default(),
        };
        let first = next;
        while keys.get(next).is_some_and(|(cell, _)| cell.row == row) {
            next += 1;
        }
        let cells = keys.get(first..next).unwrap_or_default();
        if cells.is_empty() && info == LineInfo::default() {
            continue;
        }
        let mut craft_cells = Vec::with_capacity(cells.len());
        for &(cell_ref, mut cell) in cells {
            let ov = overrides.get(&cell_ref);
            let ex = extra.get(&cell_ref);
            let mut legacy_cell = None;
            let array = dynamic.get(&cell_ref).copied().or_else(|| {
                let f = cell?.formula.as_ref().filter(|f| f.array.is_none())?;
                if f.dynamic {
                    return Some(RangeRef::cell(cell_ref));
                }
                match crate::array_formula::export(workbook, index, cell_ref, f, out.functions, &mut out.names) {
                    crate::array_formula::Export::Unchanged => None,
                    crate::array_formula::Export::Dynamic => Some(RangeRef::cell(cell_ref)),
                    crate::array_formula::Export::Legacy(e) => {
                        legacy_cell = cell.map(|x| Cell { formula: Some(std::sync::Arc::new(Formula::from_expr(e))), ..x.clone() });
                        None
                    }
                }
            });
            if legacy_cell.is_some() {
                cell = legacy_cell.as_ref();
            }
            craft_cells.push(build_craft_cell(
                CellWriteInput {
                    reference: cell_ref,
                    cell,
                    picture: sheet.cell_pictures.get(&cell_ref),
                    header_override: ov,
                    spill_value: ex,
                    dynamic_range: array.as_ref(),
                },
                sid,
                out,
            ));
        }
        rows.push(CraftRow {
            index: row,
            height: info.size.map(px_to_pt),
            custom_height: info.size.is_some() && info.custom,
            hidden: info.hidden,
            outline_level: info.outline.min(7),
            collapsed: info.collapsed,
            style: info.style.map(sid),
            cells: craft_cells,
            attrs: Vec::new(),
        });
    }
    rows
}

fn build_craft_cell(input: CellWriteInput<'_>, sid: &dyn Fn(gridcraft_model::StyleId) -> u32, out: &mut Out) -> CraftCell {
    let CellWriteInput { reference: c, cell, picture, header_override: ov, spill_value: extra, dynamic_range: dynamic } = input;
    let style = cell.map(|x| sid(x.style)).unwrap_or(0);
    let at = to_craft_cell_ref(c);
    if let Some(picture) = picture {
        let vm = out.cell_pictures.index(picture);
        return CraftCell { at, style, value: CraftValue::Error(CraftCellError::Value), value_metadata: Some(vm), ..Default::default() };
    }
    let formula = if ov.is_some() { None } else { cell.and_then(|x| x.formula.as_ref()) };
    let py_text = if ov.is_some() || formula.is_some() {
        None
    } else {
        cell.and_then(|x| x.py.as_ref()).map(|p| {
            out.python_scripts.push(p.code.clone());
            crate::python_part::file_formula(out.python_scripts.len().saturating_sub(1), p.object)
        })
    };
    let value: Value = match (ov, extra) {
        (Some(t), _) => Value::text(t.as_str()),
        (None, Some(v)) => v.clone(),
        _ => cell.map(|x| x.value.scalar()).unwrap_or(Value::Empty),
    };
    let value = match value {
        Value::Array(a) => a.get(0, 0).cloned().unwrap_or(Value::Empty),
        v => v,
    };
    let value = match value {
        Value::Error(CellError::Circ) => Value::Number(0.0),
        Value::Error(CellError::Spill | CellError::Calc | CellError::Python | CellError::Busy | CellError::Timeout | CellError::Blocked) => {
            Value::Error(CellError::Value)
        }
        v => v,
    };
    let own = formula.filter(|f| f.dynamic && f.array.is_none()).map(|_| RangeRef::cell(c));
    let dynamic = dynamic.or(own.as_ref());
    let cell_metadata = if dynamic.is_some() && (formula.is_some_and(|f| f.array.is_none()) || py_text.is_some()) {
        out.dynamic_arrays = true;
        Some(1)
    } else {
        None
    };
    let text = formula.map(|f| crate::fmla::to_file(f)).or(py_text);
    if let Some(text) = text {
        let (kind, range) = match (formula.and_then(|f| f.array), dynamic) {
            (Some(r), _) => (CraftFormulaKind::Array, Some(to_craft_range(r))),
            (None, Some(r)) => (CraftFormulaKind::Array, Some(to_craft_range(*r))),
            _ => (CraftFormulaKind::Normal, None),
        };
        let craft_val = match &value {
            Value::Number(n) => CraftValue::Number(*n),
            Value::Text(t) => CraftValue::Text(t.as_ref().into()),
            Value::Bool(b) => CraftValue::Bool(*b),
            Value::Error(e) => CraftValue::Error(to_craft_error(*e)),
            _ => {
                out.needs_calc = true;
                CraftValue::Empty
            }
        };
        return CraftCell {
            at,
            style,
            value: craft_val,
            formula: Some(CraftFormula { kind, text, shared_index: None, range, attrs: Vec::new() }),
            value_metadata: None,
            cell_metadata,
            phonetic: false,
        };
    }
    let craft_val = match &value {
        Value::Empty | Value::Array(_) => CraftValue::Empty,
        Value::Number(n) => CraftValue::Number(*n),
        Value::Text(t) if extra.is_some() => CraftValue::Text(t.as_ref().into()),
        Value::Text(t) => CraftValue::Shared(out.sst.index(t)),
        Value::Bool(b) => CraftValue::Bool(*b),
        Value::Error(e) => CraftValue::Error(to_craft_error(*e)),
    };
    CraftCell { at, style, value: craft_val, ..Default::default() }
}

const X14_NS: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
const XM_NS: &str = "http://schemas.microsoft.com/office/excel/2006/main";

/// The box a note made here gets: two columns right of its cell, four rows tall, 108 x 59.25 pt.
/// A box read from a file that equals it is kept as `None`, so a note's model survives a save.
pub(crate) const DEFAULT_NOTE_BOX: NoteBox = NoteBox {
    from: NoteCorner { col: 1, col_px: 15, row: 0, row_px: 2 },
    to: NoteCorner { col: 3, col_px: 15, row: 4, row_px: 16 },
    size_pt: Some(DEFAULT_NOTE_SIZE_PT),
};
const DEFAULT_NOTE_SIZE_PT: (f64, f64) = (108.0, 59.25);
