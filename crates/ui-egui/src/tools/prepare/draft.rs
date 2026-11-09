use super::{Calculate, FieldLook, FieldProps, FormField, FormFieldKind, Format, Validate};

// ───────────────────────────────────────────────────────────────────────── Field Properties

impl crate::PdfCraftApp {
    /// Open Field Properties for a field of the active document.
    pub fn open_field_props(&mut self, name: &str, widget: usize) {
        let Some((index, id)) = self.active_ids() else { return };
        let names = self.views[index].prepare.names();
        if names.len() > 1 && names.iter().any(|n| n == name) {
            let draft = self.session.get(id).map_or(Err("The document is no longer open."), |doc| crate::bulk_fields::Draft::new(doc, names));
            match draft {
                Ok(draft) => {
                    self.field_props = None;
                    self.bulk_field_props = Some(draft);
                    self.dialog = Some(crate::Dialog::BulkFieldProps);
                }
                Err(why) => self.notify_tr(why),
            }
            return;
        }
        self.bulk_field_props = None;
        let Some(f) = self.session.get(id).and_then(|d| d.form.iter().find(|f| f.name == name).cloned()) else { return };
        let mut d = FieldDraft::new(&f, widget);
        d.look = self.session.get(id).and_then(|doc| doc.field_look(name));
        d.check_style = self.session.get(id).and_then(|doc| doc.field_check_style(name));
        if let Some(o) = d.original.as_mut() {
            o.look = d.look;
            o.check_style = d.check_style;
        }
        let others: Vec<String> =
            self.session.get(id).map(|doc| doc.form.iter().filter(|x| x.name != name).map(|x| x.name.clone()).collect()).unwrap_or_default();
        d.others = others.clone();
        d.actions = self.session.get(id).map(|doc| doc.field_actions(name)).unwrap_or_default();
        d.chained = self.session.get(id).map(|doc| doc.field_action_chains(name)).unwrap_or_default();
        if let Some(o) = d.original.as_mut() {
            o.others = others;
            o.actions = d.actions.clone();
        }
        self.field_props = Some(d);
        self.dialog = Some(crate::Dialog::FieldProps);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldTab {
    General,
    Appearance,
    Position,
    Options,
    Format,
    Validate,
    Calculate,
    Actions,
}

/// Actions tab: the "Select Action" choices.
pub const ACTION_KINDS: [&str; 8] = [
    "Run a JavaScript",
    "Open a web link",
    "Reset a form",
    "Execute a menu item",
    "Go to a page view",
    "Show a field",
    "Hide a field",
    "Submit a form",
];

/// Actions tab: the action being added.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionDraft {
    pub trigger: pdfcraft_engine::FieldTrigger,
    pub kind: usize,
    /// The script, URL, field names (comma-separated), menu item or page number.
    pub text: String,
}

impl Default for ActionDraft {
    fn default() -> Self {
        ActionDraft { trigger: pdfcraft_engine::FieldTrigger::MouseUp, kind: 0, text: String::new() }
    }
}

impl ActionDraft {
    pub fn action(&self) -> Result<pdfcraft_engine::FieldAction, String> {
        use pdfcraft_engine::FieldAction as A;
        let t = self.text.trim();
        let names = || t.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>();
        Ok(match self.kind {
            0 => A::JavaScript(self.text.clone()),
            1 if !t.is_empty() => A::Uri(t.to_string()),
            2 => A::Reset(names()),
            3 => A::Named(if t.is_empty() { "Print".into() } else { t.to_string() }),
            4 => A::GoTo(t.parse::<usize>().ok().filter(|p| *p >= 1).ok_or("Enter a page number")? - 1),
            5 if !t.is_empty() => A::ShowHide { fields: names(), hide: false },
            6 if !t.is_empty() => A::ShowHide { fields: names(), hide: true },
            7 if !t.is_empty() => A::Submit(t.to_string()),
            1 | 7 => return Err("Enter a URL".into()),
            _ => return Err("Enter the field names".into()),
        })
    }
}

/// The Field Properties dialog's working copy.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldDraft {
    pub field: String,
    pub kind: FormFieldKind,
    pub widget: usize,
    pub tab: FieldTab,
    pub name: String,
    pub tooltip: String,
    pub caption: String,
    pub read_only: bool,
    pub required: bool,
    pub multiline: bool,
    pub limit: bool,
    pub max_len: usize,
    pub options: Vec<String>,
    pub new_option: String,
    /// `/Ff` as edited on the Options tab.
    pub flags: u32,
    /// Field Properties ▸ General ▸ Locked.
    pub locked: bool,
    /// `/Q`: 0 left, 1 centre, 2 right.
    pub quadding: i64,
    /// `/DV` (text and choices), or the default state (buttons: empty = off).
    pub default: String,
    /// Check boxes and radio buttons: this widget's "on" state (export value).
    pub on_state: String,
    pub export_value: String,
    /// 0 = auto.
    pub font_size: f64,
    /// Left, bottom, width, height in points.
    pub position: [f64; 4],
    /// `/MK /R`: 0, 90, 180 or 270 degrees counterclockwise.
    pub rotation: i64,
    pub look: Option<FieldLook>,
    /// Check boxes and radio buttons: the mark when on (Options tab).
    pub check_style: Option<pdfcraft_engine::CheckStyle>,
    pub format: Format,
    pub validate: Validate,
    pub calculate: Calculate,
    /// Every other field's name (the Calculate tab picks from them).
    pub others: Vec<String>,
    /// Actions tab: each trigger's action, and the one being added.
    pub actions: Vec<(pdfcraft_engine::FieldTrigger, pdfcraft_engine::FieldAction)>,
    pub new_action: ActionDraft,
    /// Actions tab: the trigger whose script `new_action` is editing (Edit), if any.
    pub editing: Option<pdfcraft_engine::FieldTrigger>,
    /// Actions tab: how many actions the document chains after each trigger's first one. They are
    /// kept when that first action is replaced.
    pub chained: Vec<(pdfcraft_engine::FieldTrigger, usize)>,
    pub(super) original: Box<Option<FieldDraft>>,
}

pub(crate) fn da_size(da: &str) -> f64 {
    let mut previous = "";
    let mut size = 0.0;
    for token in da.split_whitespace() {
        if token == "Tf" {
            size = previous.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0);
        }
        previous = token;
    }
    size
}

pub(crate) fn da_font(da: &str) -> &str {
    let (mut two_back, mut previous, mut font) = ("", "", "Helv");
    for token in da.split_whitespace() {
        if token == "Tf" {
            font = two_back.trim_start_matches('/');
        }
        two_back = previous;
        previous = token;
    }
    font
}

impl FieldDraft {
    pub fn new(f: &FormField, widget: usize) -> Self {
        let r = f.widgets.get(widget).map(|w| w.rect).unwrap_or_default();
        let mut d = FieldDraft {
            field: f.name.clone(),
            kind: f.kind,
            widget,
            tab: FieldTab::General,
            // The full name: a dotted name moves the field under those parent fields (#2696).
            name: f.name.clone(),
            tooltip: f.tooltip.clone().unwrap_or_default(),
            caption: f.widgets.get(widget).and_then(|w| w.caption.clone()).unwrap_or_default(),
            read_only: f.read_only(),
            required: f.has(pdfcraft_engine::field_flags::REQUIRED),
            multiline: f.has(pdfcraft_engine::field_flags::MULTILINE),
            limit: f.max_len.is_some(),
            max_len: f.max_len.unwrap_or(0),
            options: f.options.iter().map(|(_, d)| d.clone()).collect(),
            new_option: String::new(),
            flags: f.flags,
            locked: f.locked(),
            quadding: f.quadding,
            default: f.default.first().cloned().unwrap_or_default(),
            on_state: f.widgets.get(widget).and_then(|w| w.on_state.clone()).unwrap_or_default(),
            export_value: f.export_of(widget).unwrap_or_default().to_string(),
            font_size: da_size(&f.da),
            position: [r[0], r[1], r[2] - r[0], r[3] - r[1]],
            rotation: f.widgets.get(widget).map(|w| w.rotation).unwrap_or(0),
            look: None,
            check_style: None,
            format: f.actions.format.clone(),
            validate: f.actions.validate.clone(),
            calculate: f.actions.calculate.clone(),
            others: Vec::new(),
            actions: Vec::new(),
            new_action: ActionDraft::default(),
            editing: None,
            chained: Vec::new(),
            original: Box::new(None),
        };
        d.original = Box::new(Some(d.clone()));
        d
    }

    pub fn title(&self) -> &'static str {
        match self.kind {
            FormFieldKind::Text => "Text Field Properties",
            FormFieldKind::CheckBox => "Check Box Properties",
            FormFieldKind::Radio => "Radio Button Properties",
            FormFieldKind::Combo => "Dropdown Properties",
            FormFieldKind::List => "List Box Properties",
            FormFieldKind::PushButton => "Button Properties",
            FormFieldKind::Signature => "Digital Signature Properties",
        }
    }

    pub(super) fn tabs(&self) -> Vec<(FieldTab, &'static str)> {
        let mut t = vec![(FieldTab::General, "General"), (FieldTab::Appearance, "Appearance"), (FieldTab::Position, "Position")];
        if self.kind != FormFieldKind::Signature {
            t.push((FieldTab::Options, "Options"));
        }
        if matches!(self.kind, FormFieldKind::Text | FormFieldKind::Combo) {
            t.extend([(FieldTab::Format, "Format"), (FieldTab::Validate, "Validate"), (FieldTab::Calculate, "Calculate")]);
        }
        t.push((FieldTab::Actions, "Actions"));
        t
    }

    /// The properties that changed (`None` when nothing did).
    pub fn props(&self) -> Option<FieldProps> {
        let o = self.original.as_ref().as_ref()?;
        let ch = |a: bool, b: bool| (a != b).then_some(a);
        let p = FieldProps {
            name: (self.name.trim() != o.name).then(|| self.name.trim().to_string()),
            tooltip: (self.tooltip != o.tooltip).then(|| self.tooltip.clone()),
            caption: (self.caption != o.caption).then(|| (self.widget, self.caption.clone())),
            read_only: ch(self.read_only, o.read_only),
            required: ch(self.required, o.required),
            multiline: ch(self.multiline, o.multiline),
            max_len: ((self.limit, self.max_len) != (o.limit, o.max_len)).then_some((self.limit && self.max_len > 0).then_some(self.max_len)),
            options: (self.options != o.options).then(|| self.options.clone()),
            export_value: (self.export_value != o.export_value).then(|| (self.widget, self.export_value.clone())),
            font_size: ((self.font_size - o.font_size).abs() > 1e-6).then_some(self.font_size),
            rect: (self.position != o.position).then(|| {
                let [x, y, w, h] = self.position;
                (self.widget, [x, y, x + w.max(4.0), y + h.max(4.0)])
            }),
            rotation: (self.rotation != o.rotation).then_some((self.widget, self.rotation)),
            look: (self.look != o.look).then_some(self.look).flatten(),
            appearance: None,
            check_style: (self.check_style != o.check_style).then_some(self.check_style).flatten(),
            format: (self.format != o.format).then(|| self.format.clone()),
            validate: (self.validate != o.validate).then(|| self.validate.clone()),
            calculate: (self.calculate != o.calculate).then(|| self.calculate.clone()),
            flags: OPTION_FLAGS.iter().filter(|b| (self.flags ^ o.flags) & **b != 0).map(|b| (*b, self.flags & b != 0)).collect(),
            quadding: (self.quadding != o.quadding).then_some(self.quadding),
            locked: ch(self.locked, o.locked),
            actions: (self.actions != o.actions).then(|| self.actions.clone()),
            default_value: (self.default != o.default).then(|| (!self.default.is_empty()).then(|| self.default.clone())),
        };
        (p != FieldProps::default()).then_some(p)
    }
}

/// The `/Ff` bits the Options tab edits.
const OPTION_FLAGS: [u32; 10] = {
    use pdfcraft_engine::field_flags as ff;
    [
        ff::DO_NOT_SCROLL,
        ff::RICH_TEXT,
        ff::PASSWORD,
        ff::FILE_SELECT,
        ff::DO_NOT_SPELL_CHECK,
        ff::COMB,
        ff::SORT,
        ff::EDIT,
        ff::MULTI_SELECT,
        ff::COMMIT_ON_SEL_CHANGE,
    ]
};
