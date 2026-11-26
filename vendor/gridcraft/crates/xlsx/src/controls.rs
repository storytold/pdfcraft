//! Form controls (buttons, scroll bars, spin buttons) in xlsx, from the published ECMA-376 and
//! MS-XLSX formats. A control is written three ways, as spreadsheet applications expect:
//!
//! - a control properties part (`xl/ctrlProps/ctrlPropN.xml`, `formControlPr`) with its type,
//!   linked cell, limits and value;
//! - a `<control>` in the sheet's `<controls>`, inside `mc:AlternateContent` (it needs the 2010
//!   extensions), naming the properties part, the VML shape and the anchor;
//! - a legacy VML shape with `x:ClientData`, in the sheet's VML drawing (shared with notes).
//!
//! Reading prefers the properties part and falls back to the VML. Values are clamped to the
//! 0..30000 whole numbers sliders and spin buttons take. Controls of other kinds (check boxes,
//! list boxes, drop-downs, ActiveX controls) are left to pass-through (`passthrough.rs`), which
//! keeps them unchanged; a sheet can hold both, in one `<controls>` element and one VML drawing.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use gridcraft_core::CellRef;
use gridcraft_model::{Anchor, AnchorMode, CONTROL_MAX, ControlAction, ControlKind, FormControl, MacroInput, Sheet};

use crate::package::Rel;
use crate::read::Ctx;
use crate::tables::EMU_PER_PX;
use crate::write::{Out, Rels};
use crate::xml::{El, esc, esc_attr};

pub const CT_CTRL_PROP: &str = "application/vnd.ms-excel.controlproperties+xml";
/// The worksheet relationship to a control properties part (MS-XLSX), as Excel writes it.
pub const REL_CTRL_PROP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/ctrlProp";
const NS_X14: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const NS_XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";

/// The most controls read from one sheet.
const MAX_CONTROLS: usize = 10_000;

fn object_type(k: ControlKind) -> &'static str {
    match k {
        ControlKind::Button => "Button",
        ControlKind::Slider => "Scroll",
        ControlKind::Spin => "Spin",
    }
}

fn kind_of(t: &str) -> Option<ControlKind> {
    match t.trim().to_ascii_lowercase().as_str() {
        "button" => Some(ControlKind::Button),
        "scroll" => Some(ControlKind::Slider),
        "spin" => Some(ControlKind::Spin),
        _ => None,
    }
}

/// The macro a button runs, as Excel writes it: `[0]!Name` (this workbook).
fn macro_ref(c: &FormControl) -> Option<String> {
    match &c.action {
        Some(ControlAction::Macro { name, .. }) if !name.trim().is_empty() => Some(format!("[0]!{}", name.trim())),
        _ => None,
    }
}

/// The macro name in `[0]!Name`, `Book1.xlsm!Name` or `Name`.
fn macro_name(text: &str) -> Option<String> {
    let name = text.rsplit_once('!').map_or(text, |(_, n)| n).trim().trim_matches('\'');
    (!name.is_empty() && name.chars().count() <= gridcraft_model::MAX_MACRO_NAME).then(|| name.to_string())
}

/// The cell under a sheet position and the offset into it: (cell, dx, dy).
fn cell_at(sheet: &Sheet, x: f64, y: f64) -> (CellRef, f64, f64) {
    let (col, row) = (sheet.col_at(x), sheet.row_at(y));
    (CellRef::new(row, col), x - sheet.col_left(col), y - sheet.row_top(row))
}

/// The two corners of a control: ((from, dx, dy), (to, dx, dy)), offsets in sheet pixels.
fn corners(sheet: &Sheet, a: &Anchor) -> ((CellRef, f64, f64), (CellRef, f64, f64)) {
    let x1 = sheet.col_left(a.cell.col) + a.dx.max(0.0) as f64;
    let y1 = sheet.row_top(a.cell.row) + a.dy.max(0.0) as f64;
    let from = (a.cell, a.dx.max(0.0) as f64, a.dy.max(0.0) as f64);
    (from, cell_at(sheet, x1 + a.width.max(0.0) as f64, y1 + a.height.max(0.0) as f64))
}

/// The sheet's controls written out: the `<control>` entries of the worksheet's `<controls>`
/// element (each in its own `mc:AlternateContent`, with the namespaces it needs, so it can also
/// join a kept `<controls>` element) and the VML shapes for its drawing.
pub struct Written {
    pub entries: String,
    pub vml: String,
}

/// The `<controls>` element for `entries` written by [`write`].
pub fn controls_element(entries: &str) -> String {
    format!(
        "<mc:AlternateContent xmlns:mc=\"{NS_MC}\"><mc:Choice xmlns:x14=\"{NS_X14}\" Requires=\"x14\"><controls>{entries}</controls></mc:Choice></mc:AlternateContent>"
    )
}

/// Writes the properties parts and relationships of the sheet's controls. `shape_id(i)` is
/// control `i`'s VML shape id.
pub fn write(out: &mut Out, rels: &mut Rels, sheet: &Sheet, shape_id: impl Fn(usize) -> u64) -> Written {
    let mut xml = String::new();
    let mut vml = String::new();
    for (i, c) in sheet.controls.iter().enumerate() {
        // Kept properties parts of another sheet may hold the next name.
        let n = loop {
            out.ctrl_props = out.ctrl_props.saturating_add(1);
            if out.kept.reserve(&format!("xl/ctrlProps/ctrlProp{}.xml", out.ctrl_props)) || out.ctrl_props == u32::MAX {
                break out.ctrl_props;
            }
        };
        let part = format!("xl/ctrlProps/ctrlProp{n}.xml");
        out.part(&part, Some(CT_CTRL_PROP), ctrl_prop(c).into_bytes());
        let rid = rels.add_uri(REL_CTRL_PROP, &format!("../ctrlProps/ctrlProp{n}.xml"));
        let id = shape_id(i);
        let ((from, fdx, fdy), (to, tdx, tdy)) = corners(sheet, &c.anchor);
        let emu = |v: f64| (v.max(0.0) * EMU_PER_PX).round() as i64;
        let marker = |tag: &str, cell: CellRef, dx: f64, dy: f64| {
            format!(
                "<{tag}><xdr:col>{}</xdr:col><xdr:colOff>{}</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>{}</xdr:rowOff></{tag}>",
                cell.col,
                emu(dx),
                cell.row,
                emu(dy)
            )
        };
        let mut pr = String::from(" defaultSize=\"0\" autoPict=\"0\"");
        if !c.print {
            pr.push_str(" print=\"0\"");
        }
        if !c.locked {
            pr.push_str(" locked=\"0\"");
        }
        if let Some(m) = macro_ref(c) {
            let _ = write!(pr, " macro=\"{}\"", esc_attr(&m));
        }
        let (move_cells, size_cells) = match c.anchor.mode {
            AnchorMode::MoveAndSize => (true, true),
            AnchorMode::MoveOnly => (true, false),
            AnchorMode::Absolute => (false, false),
        };
        let name = esc_attr(&c.display_name());
        let _ = write!(
            xml,
            "<mc:AlternateContent xmlns:mc=\"{NS_MC}\"><mc:Choice xmlns:x14=\"{NS_X14}\" Requires=\"x14\"><control shapeId=\"{id}\" r:id=\"{rid}\" name=\"{name}\"><controlPr{pr}><anchor{}{} xmlns:xdr=\"{NS_XDR}\">{}{}</anchor></controlPr></control></mc:Choice><mc:Fallback><control shapeId=\"{id}\" r:id=\"{rid}\" name=\"{name}\"/></mc:Fallback></mc:AlternateContent>",
            if move_cells { " moveWithCells=\"1\"" } else { "" },
            if size_cells { " sizeWithCells=\"1\"" } else { "" },
            marker("from", from, fdx, fdy),
            marker("to", to, tdx, tdy),
        );
        vml.push_str(&vml_shape(sheet, c, id, i));
    }
    Written { entries: xml, vml }
}

/// The control properties part.
fn ctrl_prop(c: &FormControl) -> String {
    let mut s = format!("{}<formControlPr xmlns=\"{NS_X14}\" objectType=\"{}\"", crate::write::XML_DECL, object_type(c.kind));
    match c.kind {
        ControlKind::Button => {
            s.push_str(" lockText=\"1\"");
        }
        ControlKind::Slider | ControlKind::Spin => {
            s.push_str(" dx=\"22\"");
            if let Some(l) = &c.link {
                let _ = write!(s, " fmlaLink=\"{}\"", esc_attr(l));
            }
            if c.kind == ControlKind::Slider && c.horizontal {
                s.push_str(" horiz=\"1\"");
            }
            let _ = write!(s, " inc=\"{}\" max=\"{}\" min=\"{}\"", c.step, c.max, c.min);
            if c.kind == ControlKind::Slider {
                let _ = write!(s, " page=\"{}\"", c.page);
            }
            let _ = write!(s, " val=\"{}\"", c.value);
        }
    }
    s.push_str("/>");
    s
}

/// A control's legacy VML shape.
fn vml_shape(sheet: &Sheet, c: &FormControl, id: u64, z: usize) -> String {
    let ((from, fdx, fdy), (to, tdx, tdy)) = corners(sheet, &c.anchor);
    let pt = |px: f64| px * 0.75;
    let x = sheet.col_left(from.col) + fdx;
    let y = sheet.row_top(from.row) + fdy;
    let mut data = format!(
        "<x:ClientData ObjectType=\"{}\">{}{}<x:Anchor>{}, {}, {}, {}, {}, {}, {}, {}</x:Anchor>",
        object_type(c.kind),
        // These two elements mean the object does *not* move (size) with its cells.
        if c.anchor.mode == AnchorMode::Absolute { "<x:MoveWithCells/>" } else { "" },
        if c.anchor.mode == AnchorMode::MoveAndSize { "" } else { "<x:SizeWithCells/>" },
        from.col,
        fdx.round() as i64,
        from.row,
        fdy.round() as i64,
        to.col,
        tdx.round() as i64,
        to.row,
        tdy.round() as i64
    );
    if !c.print {
        data.push_str("<x:PrintObject>False</x:PrintObject>");
    }
    if !c.locked {
        data.push_str("<x:Locked>False</x:Locked>");
    }
    data.push_str("<x:AutoFill>False</x:AutoFill>");
    match c.kind {
        ControlKind::Button => {
            if let Some(m) = macro_ref(c) {
                let _ = write!(data, "<x:FmlaMacro>{}</x:FmlaMacro>", esc(&m));
            }
            data.push_str("<x:TextHAlign>Center</x:TextHAlign><x:TextVAlign>Center</x:TextVAlign>");
        }
        ControlKind::Slider | ControlKind::Spin => {
            if let Some(l) = &c.link {
                let _ = write!(data, "<x:FmlaLink>{}</x:FmlaLink>", esc(l));
            }
            let _ = write!(data, "<x:Val>{}</x:Val><x:Min>{}</x:Min><x:Max>{}</x:Max><x:Inc>{}</x:Inc>", c.value, c.min, c.max, c.step);
            if c.kind == ControlKind::Slider {
                let _ = write!(data, "<x:Page>{}</x:Page>", c.page);
                if c.horizontal {
                    data.push_str("<x:Horiz/>");
                }
            }
            data.push_str("<x:Dx>22</x:Dx>");
        }
    }
    data.push_str("</x:ClientData>");
    let text = if c.kind == ControlKind::Button {
        format!("<v:textbox style=\"mso-direction-alt:auto\" o:singleclick=\"f\"><div style=\"text-align:center\">{}</div></v:textbox>", esc(&c.text))
    } else {
        String::new()
    };
    format!(
        "<v:shape id=\"_x0000_s{id}\" type=\"#_x0000_t201\" style=\"position:absolute;margin-left:{:.2}pt;margin-top:{:.2}pt;width:{:.2}pt;height:{:.2}pt;z-index:{}\"{} o:insetmode=\"auto\"><o:lock v:ext=\"edit\" rotation=\"t\"/>{text}{data}</v:shape>",
        pt(x),
        pt(y),
        pt(c.anchor.width.max(0.0) as f64),
        pt(c.anchor.height.max(0.0) as f64),
        z + 1,
        if c.kind == ControlKind::Button { " o:button=\"t\"" } else { " stroked=\"f\" filled=\"f\"" },
    )
}

/// The VML shape type form controls use.
pub const SHAPETYPE: &str = "<v:shapetype id=\"_x0000_t201\" coordsize=\"21600,21600\" o:spt=\"201\" path=\"m,l,21600r21600,l21600,xe\"><v:stroke joinstyle=\"miter\"/><v:path shadowok=\"f\" o:extrusionok=\"f\" strokeok=\"f\" fillok=\"f\" o:connecttype=\"rect\"/><o:lock v:ext=\"edit\" shapetype=\"t\"/></v:shapetype>";

// ---------------------------------------------------------------- reading

/// What a control's properties say, from `formControlPr` or VML `x:ClientData`.
#[derive(Clone, Default)]
struct Props {
    kind: Option<ControlKind>,
    link: Option<String>,
    min: Option<i64>,
    max: Option<i64>,
    inc: Option<i64>,
    page: Option<i64>,
    val: Option<i64>,
    horiz: Option<bool>,
    macro_: Option<String>,
    text: String,
    print: Option<bool>,
    locked: Option<bool>,
}

fn props_from_ctrl_prop(e: &El) -> Props {
    let int = |k: &str| e.attr_f64(k).map(|v| v.round() as i64);
    Props {
        kind: e.attr("objectType").and_then(kind_of),
        link: e.attr("fmlaLink").map(str::to_string),
        min: int("min"),
        max: int("max"),
        inc: int("inc"),
        page: int("page"),
        val: int("val"),
        horiz: e.attr_bool("horiz"),
        macro_: None,
        text: String::new(),
        print: None,
        locked: None,
    }
}

fn props_from_vml(shape: &El) -> Option<Props> {
    let data = shape.child("ClientData")?;
    let kind = data.attr("ObjectType").and_then(kind_of)?;
    let text_of = |n: &str| data.child(n).map(|c| c.text.trim().to_string());
    let int = |n: &str| text_of(n).and_then(|t| t.parse::<f64>().ok()).filter(|v| v.is_finite()).map(|v| v.round() as i64);
    let truth = |n: &str| text_of(n).map(|t| !t.eq_ignore_ascii_case("false"));
    let text = shape.child("textbox").map(|t| all_text(t, 0)).unwrap_or_default().trim().to_string();
    Some(Props {
        kind: Some(kind),
        link: text_of("FmlaLink").filter(|t| !t.is_empty()),
        min: int("Min"),
        max: int("Max"),
        inc: int("Inc"),
        page: int("Page"),
        val: int("Val"),
        horiz: data.child("Horiz").map(|_| true),
        macro_: text_of("FmlaMacro"),
        text,
        print: truth("PrintObject"),
        locked: truth("Locked"),
    })
}

/// The text inside `e` and its descendants (a VML text box holds `<div>` and `<font>`), up to a
/// caption's length; `<br>` is a line break.
fn all_text(e: &El, depth: usize) -> String {
    let mut s = e.text.clone();
    if depth < 16 {
        for k in &e.children {
            if k.name == "br" {
                s.push('\n');
            }
            s.push_str(&all_text(k, depth + 1));
            if s.len() > 4_096 {
                break;
            }
        }
    }
    s
}

/// A control from what the file says, its numbers clamped to whole numbers 0..30000 that pass
/// the model's rules (a hostile or odd file can't give a control it couldn't have).
fn build(p: Props, kind: ControlKind, anchor: Anchor, name: Option<&str>, id: u32) -> FormControl {
    let clamp = |v: Option<i64>, d: u32| v.map_or(d, |v| v.clamp(0, CONTROL_MAX as i64) as u32);
    let mut c = FormControl { id, kind, anchor, ..FormControl::default() };
    c.min = clamp(p.min, 0);
    c.max = clamp(p.max, 100);
    if c.min >= c.max {
        if c.min >= CONTROL_MAX {
            c.min = CONTROL_MAX - 1;
        }
        c.max = c.min + 1;
    }
    c.step = clamp(p.inc, 1).clamp(1, c.max - c.min);
    c.page = clamp(p.page, 10).max(c.step);
    c.value = c.snap(clamp(p.val, c.min) as f64);
    // Only sliders have an orientation; `horiz` is left out for a vertical one.
    c.horizontal = kind != ControlKind::Slider || p.horiz.unwrap_or(false);
    c.link = p.link.map(|l| l.trim().trim_start_matches('=').to_string()).filter(|l| !l.is_empty() && l.chars().count() <= 300);
    c.text = p.text.chars().take(1_024).collect();
    if let Some(n) = name {
        c.name = n.chars().take(255).collect();
    }
    c.print = p.print.unwrap_or(true);
    c.locked = p.locked.unwrap_or(true);
    if kind == ControlKind::Button
        && let Some(m) = p.macro_.as_deref().and_then(macro_name)
    {
        c.action = Some(ControlAction::Macro { name: m, input: MacroInput::Selection, output: None });
    }
    c
}

/// The `<control>` elements one child of a `<controls>` element holds: the child itself, or those
/// inside its `mc:AlternateContent` (the Choice, else the Fallback).
fn controls_of_child(k: &El) -> Vec<&El> {
    match k.name.as_str() {
        "control" => vec![k],
        "AlternateContent" => k
            .child("Choice")
            .filter(|c| c.child("control").is_some())
            .or_else(|| k.child("Fallback"))
            .map(|inner| inner.kids("control").collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The `<control>` elements under a `<controls>` element, looking through `mc:AlternateContent`
/// (the Choice, else the Fallback).
fn controls_in(e: &El) -> Vec<&El> {
    e.children.iter().flat_map(controls_of_child).collect()
}

/// The `<controls>` element of a worksheet child (`controls`, or one inside AlternateContent).
pub fn controls_of(e: &El) -> Option<&El> {
    match e.name.as_str() {
        "controls" => Some(e),
        "AlternateContent" => e.child("Choice").and_then(|c| c.child("controls")).or_else(|| e.child("Fallback").and_then(|c| c.child("controls"))),
        _ => None,
    }
}

/// The shape number of a VML shape id (`_x0000_s1025` → 1025) or a `shapeId` attribute.
pub(crate) fn shape_number(id: &str) -> Option<u64> {
    id.trim().trim_start_matches("_x0000_s").parse().ok()
}

/// A VML shape's number: from its `id` (`_x0000_s1025`), else from `o:spid`, where producers that
/// give the shape a named id (`id="Button 1"`) keep it.
pub(crate) fn number_of(shape: &El) -> Option<u64> {
    shape.attr("id").and_then(shape_number).or_else(|| shape.attr("spid").and_then(shape_number))
}

/// What [`read`] took from a sheet's form controls, for pass-through.
#[derive(Debug, Default)]
pub struct ReadControls {
    /// The VML shape numbers of the controls read into the model: GridCraft writes those, so they
    /// are left out of the kept content.
    pub modelled: HashSet<u64>,
    /// The VML drawing also holds a control that the sheet doesn't list and the model doesn't
    /// hold (a check box only the drawing has): the drawing is kept for it even when no
    /// `<controls>` element is left to keep.
    pub vml_only_unmodelled: bool,
}

/// Whether a VML shape the sheet doesn't list is a control that only pass-through can keep: it
/// has client data that is neither a note (GridCraft writes notes) nor an embedded object's
/// picture (`Pict`, which means nothing without its sheet entry).
fn unlisted_kept(shape: &El) -> bool {
    object_type_of(shape).is_some_and(|t| !t.eq_ignore_ascii_case("note") && !t.eq_ignore_ascii_case("pict"))
}

/// A VML shape's client data `ObjectType`, trimmed (`Note`, `Button`, `Checkbox`).
fn object_type_of(shape: &El) -> Option<&str> {
    shape.child("ClientData").and_then(|d| d.attr("ObjectType")).map(str::trim)
}

/// Whether a VML shape's client data is a note's: its `ObjectType` is `Note` in any case and with
/// spaces around it. Only that attribute counts, not text elsewhere in the shape (a caption).
pub(crate) fn is_note_data(data: &El) -> bool {
    data.attr("ObjectType").is_some_and(|t| t.trim().eq_ignore_ascii_case("note"))
}

/// Reads a sheet's buttons, scroll bars and spin buttons: each `<control>` with its properties
/// part, else its VML shape, and VML shapes the sheet doesn't list (files that only have VML).
/// The shapes come from the VML drawing with relationship id `vml_rid` (`<legacyDrawing r:id>`).
/// Returns the shape numbers of the controls it read, so pass-through keeps only the others.
pub fn read(cx: &mut Ctx<'_>, controls: Option<&El>, vml_rid: Option<&str>, rels: &[Rel], sheet: &mut Sheet) -> ReadControls {
    let mut read = HashSet::new();
    let vml = match vml_rid.and_then(|id| rels.iter().find(|r| r.id == id && r.kind == "vmlDrawing")) {
        Some(r) => cx.pkg.read_xml(&r.target).ok().flatten(),
        None => None,
    };
    let shapes: Vec<&El> = vml.as_ref().map(|v| v.kids("shape").collect()).unwrap_or_default();
    // Looked up once per listed control: indexes keep a sheet of many controls linear.
    let mut by_number: HashMap<u64, &El> = HashMap::with_capacity(shapes.len());
    for s in &shapes {
        if let Some(n) = number_of(s) {
            by_number.entry(n).or_insert(s);
        }
    }
    let shape_of = |n: u64| by_number.get(&n).copied();
    // The relationships likewise: one per control, looked up by id (the first of a repeated id wins,
    // as a scan would find it).
    let mut rel_by_id: HashMap<&str, &Rel> = HashMap::with_capacity(rels.len());
    for r in rels {
        rel_by_id.entry(r.id.as_str()).or_insert(r);
    }
    // Each properties part is read and parsed once, however many controls name it: every read is
    // charged to the package's budget, so a file whose controls all point at one part would
    // otherwise spend it on that part alone. A part that fails to read, or isn't a
    // `formControlPr`, is remembered as nothing and not read again.
    let mut props_by_part: HashMap<&str, Option<Props>> = HashMap::new();
    // Every shape the sheet lists, read here or not.
    let mut listed: HashSet<u64> = HashSet::new();
    for ctl in controls.map(controls_in).unwrap_or_default() {
        let Some(n) = ctl.attr("shapeId").and_then(shape_number) else { continue };
        listed.insert(n);
        if sheet.controls.len() >= MAX_CONTROLS {
            continue;
        }
        let shape = shape_of(n);
        let vml_props = shape.and_then(props_from_vml);
        let prop_part = ctl.attr("id").and_then(|id| rel_by_id.get(id)).map(|r| r.target.as_str());
        let file_props = match prop_part {
            Some(t) => props_by_part
                .entry(t)
                .or_insert_with(|| cx.pkg.read_xml(t).ok().flatten().filter(|e| e.name == "formControlPr").map(|e| props_from_ctrl_prop(&e)))
                .clone(),
            None => None,
        };
        let pr = ctl.child("controlPr");
        let mut p = match (file_props, vml_props) {
            (Some(mut f), v) => {
                // The properties part has no caption; the VML text box does.
                if let Some(v) = v {
                    f.text = v.text;
                    f.print = f.print.or(v.print);
                    f.locked = f.locked.or(v.locked);
                    f.macro_ = v.macro_;
                }
                f
            }
            (None, Some(v)) => v,
            (None, None) => continue,
        };
        if let Some(pr) = pr {
            if let Some(m) = pr.attr("macro").filter(|m| !m.trim().is_empty()) {
                p.macro_ = Some(m.to_string());
            }
            p.print = pr.attr_bool("print").or(p.print);
            p.locked = pr.attr_bool("locked").or(p.locked);
        }
        // Other kinds (a check box, a list box) stay with pass-through.
        let Some(kind) = p.kind else { continue };
        let anchor =
            pr.and_then(|pr| pr.child("anchor")).and_then(|a| anchor_from_markers(a, sheet)).or_else(|| shape.and_then(|s| vml_anchor(s, sheet)));
        let Some(anchor) = anchor else { continue };
        let id = cx.next_object_id();
        sheet.controls.push(build(p, kind, anchor, ctl.attr("name"), id));
        read.insert(n);
    }
    // Controls known only from the VML. The ones not read (a check box, or past the limit) stay
    // in the kept drawing.
    let mut vml_only_unmodelled = false;
    for s in &shapes {
        let n = number_of(s);
        if n.is_some_and(|n| listed.contains(&n)) {
            continue;
        }
        let modelled = n.filter(|_| sheet.controls.len() < MAX_CONTROLS).and_then(|n| {
            let p = props_from_vml(s)?;
            let (kind, anchor) = (p.kind?, vml_anchor(s, sheet)?);
            let id = cx.next_object_id();
            sheet.controls.push(build(p, kind, anchor, None, id));
            read.insert(n);
            Some(())
        });
        if modelled.is_none() && unlisted_kept(s) {
            vml_only_unmodelled = true;
        }
    }
    ReadControls { modelled: read, vml_only_unmodelled }
}

fn anchor_from_markers(a: &El, sheet: &Sheet) -> Option<Anchor> {
    let (cell, dx, dy) = crate::drawing::marker(a.child("from")?);
    let (to, tdx, tdy) = a.child("to").map(crate::drawing::marker).unwrap_or((cell, dx + 64.0, dy + 20.0));
    let x1 = sheet.col_left(cell.col) + dx as f64;
    let y1 = sheet.row_top(cell.row) + dy as f64;
    let x2 = sheet.col_left(to.col) + tdx as f64;
    let y2 = sheet.row_top(to.row) + tdy as f64;
    let mode = match (a.flag("moveWithCells", false), a.flag("sizeWithCells", false)) {
        (_, true) => AnchorMode::MoveAndSize,
        (true, false) => AnchorMode::MoveOnly,
        _ => AnchorMode::Absolute,
    };
    Some(Anchor { cell, dx, dy, width: (x2 - x1).clamp(1.0, 10_000.0) as f32, height: (y2 - y1).clamp(1.0, 10_000.0) as f32, mode })
}

/// A VML `x:Anchor` (`LeftColumn, LeftOffset, TopRow, TopOffset, RightColumn, RightOffset,
/// BottomRow, BottomOffset`, offsets in pixels).
fn vml_anchor(shape: &El, sheet: &Sheet) -> Option<Anchor> {
    let data = shape.child("ClientData")?;
    let nums: Vec<i64> = data.child("Anchor")?.text.split(',').filter_map(|t| t.trim().parse::<i64>().ok()).collect();
    let [c1, dx1, r1, dy1, c2, dx2, r2, dy2] = nums.as_slice() else { return None };
    let col = |c: i64| c.clamp(0, gridcraft_core::MAX_COLS as i64 - 1) as u32;
    let row = |r: i64| r.clamp(0, gridcraft_core::MAX_ROWS as i64 - 1) as u32;
    let off = |v: i64| v.clamp(0, 10_000) as f64;
    let cell = CellRef::new(row(*r1), col(*c1));
    let x1 = sheet.col_left(cell.col) + off(*dx1);
    let y1 = sheet.row_top(cell.row) + off(*dy1);
    let x2 = sheet.col_left(col(*c2)) + off(*dx2);
    let y2 = sheet.row_top(row(*r2)) + off(*dy2);
    // The elements are there when the object does *not* move (size) with its cells.
    let mode = match (data.child("MoveWithCells").is_none(), data.child("SizeWithCells").is_none()) {
        (_, true) => AnchorMode::MoveAndSize,
        (true, false) => AnchorMode::MoveOnly,
        _ => AnchorMode::Absolute,
    };
    Some(Anchor {
        cell,
        dx: off(*dx1) as f32,
        dy: off(*dy1) as f32,
        width: (x2 - x1).clamp(1.0, 10_000.0) as f32,
        height: (y2 - y1).clamp(1.0, 10_000.0) as f32,
        mode,
    })
}
