//! Custom stamps (Add a stamp ▸ Custom stamps ▸ Create): a PDF page or an image saved in the
//! user's stamp library under a category and a name, then placed like any stamp. Also the Stamp
//! tool itself, which places a built-in or a custom stamp with one click.

use std::borrow::Cow;
use std::sync::Arc;

use craft::behavior::{CancelReason, Cursor, CursorIcon, DisplayOrder, ItemSource, Selection, SelectionChange};
use craft::host::{PickRequest, Picked};
use craft::ui::canvas::{CanvasTool, ToolAction, ToolContext, ToolPath, ToolPointer};
use craft::ui::contract::{Bind, CollectionEvent, Entry, Props as _, SelectMode, Show as _, ShowCollection as _, ShowContainer as _};
use craft::ui::dialog::DialogId;
use craft::ui::list::{List, ListRow};
use craft::ui::menu::Menu;
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog, Param, ParamDialog, ParamKind};
use craft::ui::pickers::ComboBox;
use craft::ui::primitives::Draw;
use craft::ui::sections::Section;
use craft::ui::text::TextField;
use egui::Pos2;
use pdfcraft_engine::{Edit, MarkFile, NewAnnotation, Shape, StampGroup, StampKind, Style};
use serde::{Deserialize, Serialize};

// PLAN: assumed `PageMap::to_user(&self, page: usize, at: Point) -> [f64; 2]` and `crate::comments::PageMap<'a>` (pdf-comments-w7, A-1)
use crate::comments::PageMap;

use crate::{PdfCraftApp, QuickTool};

/// Stamp files larger than this aren't kept in the library (it lives in the app's settings).
pub const MAX_STAMP_BYTES: usize = 4 << 20;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomStamp {
    pub category: String,
    pub name: String,
    /// The source file's name (its extension tells images from PDFs).
    pub file: String,
    /// The page of a PDF (0-based).
    #[serde(default)]
    pub page: usize,
    #[serde(with = "b64")]
    pub data: Arc<Vec<u8>>,
    /// A line under the picture, filled in when the stamp is placed (#1741): `{name}`,
    /// `{date}` and `{time}` become the author and now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
}

/// The line a new dynamic custom stamp offers: what the built-in dynamic stamps write.
pub const DEFAULT_LINE: &str = "By {name} at {time}, {date}";

/// The key of the Create Custom Stamp dialog, which names it to the dialog host.
const DIALOG_KEY: &str = "comment.stamp";
/// The dialog's field keys.
const CATEGORY: &str = "category";
const NAME: &str = "name";
const DYNAMIC: &str = "dynamic";
const LINE: &str = "line";

mod b64 {
    use std::sync::Arc;

    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Arc<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v.as_slice()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<Vec<u8>>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD.decode(s).map(Arc::new).map_err(serde::de::Error::custom)
    }
}

/// Which stamp the Stamp tool places.
#[derive(Clone, Debug)]
pub enum StampChoice {
    /// One of the built-in stamps.
    Builtin(StampKind),
    /// A stamp from the user's library.
    Custom(CustomStamp),
}

/// A press the tool owns: the page it landed on, where, and the same point in that page's user space. The page
/// tool is built each pass, so the session keeps it from the press to its release.
pub(crate) struct StampPress {
    page: usize,
    pos: Pos2,
    user: [f64; 2],
}

/// The Stamp tool: a click on a page places the chosen stamp centred on it, upright as the page is shown. The
/// canvas takes the edit with [`StampTool::placed`] and goes back to selecting.
pub struct StampTool<'a> {
    choice: StampChoice,
    by_line: String,
    line: Option<String>,
    author: String,
    map: &'a PageMap<'a>,
    press: &'a mut Option<StampPress>,
    placed: Option<Edit>,
}

impl<'a> StampTool<'a> {
    /// A tool placing `choice`. `by_line` is the built-in dynamic stamps' line, `line` a custom stamp's own, both
    /// already filled in for now; `map` holds the pages on screen and the document's page geometry, which lays out a
    /// built-in stamp in view space, and takes a pointer to the page's user space, whatever the view's rotation.
    /// `press` is the session's slot for the tool's press.
    pub(crate) fn new(
        choice: StampChoice,
        by_line: String,
        line: Option<String>,
        author: String,
        map: &'a PageMap<'a>,
        press: &'a mut Option<StampPress>,
    ) -> Self {
        Self { choice, by_line, line, author, map, press, placed: None }
    }

    /// The edit a click placed, once; the canvas applies it.
    pub fn placed(&mut self) -> Option<Edit> {
        self.placed.take()
    }

    /// The edit that places the stamp at `user` on `page`.
    fn edit_at(&self, page: usize, user: [f64; 2]) -> Option<Edit> {
        match &self.choice {
            StampChoice::Builtin(kind) => {
                let info = self.map.info.pages.get(page)?;
                // Laid out in view space, so the stamp is upright as the page is shown.
                let [vx, vy] = info.user_to_view(user[0] as f32, user[1] as f32);
                let (w, h) = kind.size();
                let a = info.view_to_user((f64::from(vx) - w / 2.0) as f32, (f64::from(vy) - h / 2.0) as f32);
                let b = info.view_to_user((f64::from(vx) + w / 2.0) as f32, (f64::from(vy) + h / 2.0) as f32);
                let rect = [f64::from(a[0].min(b[0])), f64::from(a[1].min(b[1])), f64::from(a[0].max(b[0])), f64::from(a[1].max(b[1]))];
                let by = (kind.group() == StampGroup::Dynamic).then(|| self.by_line.clone());
                let shape = Shape::Stamp { rect, stamp: *kind, by };
                Some(Edit::AddAnnotation(NewAnnotation {
                    page,
                    style: Style::default_for(&shape),
                    shape,
                    contents: String::new(),
                    author: self.author.clone(),
                }))
            }
            StampChoice::Custom(stamp) => Some(Edit::AddCustomStamp {
                page,
                rect: [user[0], user[1], user[0], user[1]],
                name: stamp.name.clone(),
                file: MarkFile { name: stamp.file.clone(), bytes: stamp.data.clone(), page: stamp.page },
                author: self.author.clone(),
                line: self.line.clone(),
            }),
        }
    }
}

impl CanvasTool for StampTool<'_> {
    fn press(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) -> ToolAction {
        let Some(placed) = self.map.pages.iter().find(|p| p.rect.contains(pointer.pos)) else { return ToolAction::Ignored };
        let at = placed.to_page(pointer.pos);
        let user = self.map.to_user(placed.page, at);
        *self.press = Some(StampPress { page: placed.page, pos: pointer.pos, user });
        ToolAction::Claimed
    }

    fn drag(&mut self, _pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {}

    /// A release within the click distance of the press places the stamp; a drag past it places nothing.
    fn release(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        let Some(press) = self.press.take() else { return };
        if (pointer.pos - press.pos).length() <= cx.slop {
            self.placed = self.edit_at(press.page, press.user);
        }
    }

    fn cancel(&mut self, _reason: CancelReason, _cx: &mut ToolContext<'_>) {
        *self.press = None;
    }

    fn paint(&self, _draw: &Draw<'_>, _cx: &ToolContext<'_>) {}

    fn cursor(&self, _cx: &ToolContext<'_>) -> Cursor {
        Cursor::System(CursorIcon::Crosshair)
    }

    fn command_form(&self) -> ToolPath {
        ToolPath::Command("the Add a stamp command")
    }
}

/// The Create Custom Stamp dialog the app keeps while it is open: the dialog's id and the file it makes a stamp of.
/// The category, name and line live in the dialog's own fields.
pub struct CreateStampDialog {
    /// The hosted dialog's id.
    pub id: DialogId,
    /// The chosen file's name.
    pub file: String,
    /// The chosen file's bytes.
    pub data: Arc<Vec<u8>>,
}

/// The categories of the library, sorted and without repeats.
fn categories(stamps: &[CustomStamp]) -> Vec<String> {
    let mut c: Vec<String> = stamps.iter().map(|s| s.category.clone()).collect();
    c.sort();
    c.dedup();
    c
}

/// The dialog's rows over `file` and the existing `categories`.
fn create_params(file: &str, categories: &[String]) -> Vec<Param> {
    let categories = categories.to_vec();
    vec![
        Param::read_only_text("file", "", crate::i18n::fmt(tl!("From {file}"), &[("file", file)])).full_width(),
        Param::custom(CATEGORY, tl!("Category"), move |ui, fields, _| {
            let entries: Vec<Entry<String>> = categories.iter().map(|c| Entry::new(c.clone(), c.clone())).collect();
            let mut value = fields.text(CATEGORY).unwrap_or_default().to_owned();
            let shown = ComboBox::new(tl!("Category"), Bind::bound(&mut value), &entries).placeholder(tl!("My stamps")).show(ui);
            fields.set(CATEGORY, value);
            shown.edit.response
        }),
        Param::new(NAME, tl!("Name"), ParamKind::text("")),
        // A dynamic stamp (#1741): a line under the picture, filled in when it is placed.
        Param::new(DYNAMIC, tl!("Add a line filled in when the stamp is placed"), ParamKind::bool(false)),
        Param::custom(LINE, tl!("Line"), |ui, fields, _| {
            let mut value = fields.text(LINE).unwrap_or_default().to_owned();
            let shown = TextField::new(tl!("Line"), Bind::bound(&mut value)).limit(pdfcraft_engine::MAX_STAMP_LINE).show(ui);
            fields.set(LINE, value);
            shown.response
        })
        .visible_when(DYNAMIC, true),
        Param::read_only_text("hint", "", tl!("{name}, {date} and {time} become your name, the date and the time."))
            .full_width()
            .visible_when(DYNAMIC, true),
    ]
}

/// Whether the dialog's values make a stamp: a category and a name, and a line when one is asked for.
fn fields_ok(fields: &DialogFields) -> bool {
    let filled = |key: &str| fields.text(key).is_some_and(|t| !t.trim().is_empty());
    filled(CATEGORY) && filled(NAME) && (!fields.bool(DYNAMIC).unwrap_or(false) || filled(LINE))
}

/// A row of the palette's list: a category heading or a stamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PaletteKey {
    /// The category at this position of the sorted categories.
    Heading(usize),
    /// The stamp at this index of the library.
    Stamp(usize),
}

/// The palette's rows: each category's heading followed by its stamps.
struct PaletteRows {
    rows: Vec<(PaletteKey, String)>,
}

impl PaletteRows {
    fn of(stamps: &[CustomStamp]) -> Self {
        let mut rows = Vec::new();
        for (c, category) in categories(stamps).into_iter().enumerate() {
            rows.push((PaletteKey::Heading(c), category.clone()));
            rows.extend(stamps.iter().enumerate().filter(|(_, s)| s.category == category).map(|(i, s)| (PaletteKey::Stamp(i), s.name.clone())));
        }
        Self { rows }
    }
}

impl DisplayOrder<PaletteKey> for PaletteRows {
    fn len(&self) -> usize {
        self.rows.len()
    }

    fn key(&self, at: usize) -> PaletteKey {
        self.rows.get(at).map_or(PaletteKey::Heading(usize::MAX), |(key, _)| *key)
    }

    fn is_section(&self, at: usize) -> bool {
        matches!(self.rows.get(at), Some((PaletteKey::Heading(_), _)))
    }

    fn text(&self, at: usize) -> Option<Cow<'_, str>> {
        self.rows.get(at).map(|(_, name)| Cow::Borrowed(name.as_str()))
    }
}

impl ItemSource<PaletteKey> for PaletteRows {}

/// The Custom stamps section of the stamps palette.
pub(crate) fn palette_section(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let rows = PaletteRows::of(&app.custom_stamps);
    let mut selection = match app.quick_tool {
        QuickTool::CustomStamp(i) => Selection::single(PaletteKey::Stamp(i)),
        _ => Selection::default(),
    };
    let (mut choose, mut remove) = (None, None);
    Section::fixed(tl!("Custom")).id_salt("custom-stamps").show(ui, |ui, _| {
        let out = List::new(tl!("Custom stamps")).natural_height(true).select(SelectMode::Single).selection(Bind::bound(&mut selection)).show_items(
            ui,
            &rows,
            &mut |key, item| match key {
                PaletteKey::Heading(_) => ListRow::new(rows.text(item.at).unwrap_or_default().into_owned()).section_heading().state(item.row.clone()),
                PaletteKey::Stamp(_) => ListRow::new(rows.text(item.at).unwrap_or_default().into_owned()).state(item.row.clone()),
            },
        );
        for event in out.events {
            match event {
                CollectionEvent::Selection(SelectionChange::Set(keys)) => {
                    if let Some(PaletteKey::Stamp(i)) = keys.last() {
                        choose = Some(*i);
                    }
                }
                CollectionEvent::Selection(SelectionChange::Toggle(PaletteKey::Stamp(i))) => choose = Some(i),
                _ => {}
            }
        }
        let stamp = out.menu.as_ref().and_then(|m| match m.key {
            PaletteKey::Stamp(i) => Some(i),
            PaletteKey::Heading(_) => None,
        });
        if let Some(i) = stamp {
            let picked = Menu::context(tl!("Custom stamp"), &out.response).pick(ui, &[Entry::new(i, tl!("Delete stamp"))]);
            if picked.picked.is_some() {
                remove = Some(i);
            }
        }
    });
    if let Some(i) = choose {
        app.quick_tool = QuickTool::CustomStamp(i);
    }
    if let Some(i) = remove.filter(|i| *i < app.custom_stamps.len()) {
        app.custom_stamps.remove(i);
        app.quick_tool = QuickTool::Select;
    }
    if craft::ui::buttons::Button::new(tl!("Create custom stamp…")).tooltip(tl!("From a PDF page or an image")).show(ui).clicked() {
        app.pick_stamp_file();
    }
}

impl PdfCraftApp {
    /// Create ▸ choose a PDF or an image for a new custom stamp.
    pub(crate) fn pick_stamp_file(&mut self) {
        let request = PickRequest::open("stamp")
            .title(tl!("Select a file for the stamp"))
            .filter(tl!("PDF or image"), &["pdf", "png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx"]);
        self.pick_file(request, None, move |app, ctx, picked| match picked {
            Picked::Paths(paths) => {
                let Some(path) = paths.into_iter().next() else { return };
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        app.start_custom_stamp(ctx, name, bytes);
                    }
                    Err(e) => app.notify_fmt("Couldn't read {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
                }
            }
            Picked::Contents(files) => {
                if let Some((name, bytes)) = files.into_iter().next() {
                    app.start_custom_stamp(ctx, name, bytes);
                }
            }
            _ => {}
        });
    }

    /// Open the Create Custom Stamp dialog for a file.
    pub fn start_custom_stamp(&mut self, ctx: &egui::Context, file: String, bytes: Vec<u8>) {
        if bytes.len() > MAX_STAMP_BYTES {
            self.notify_fmt("{file} is too large for a stamp (at most {n} MB)", &[("file", &file), ("n", &(MAX_STAMP_BYTES >> 20).to_string())]);
            return;
        }

        let name = file.rsplit_once('.').map_or(file.as_str(), |(s, _)| s).to_string();
        let category = self.custom_stamps.last().map(|s| s.category.clone()).unwrap_or_else(|| "My stamps".into());
        let existing = categories(&self.custom_stamps);
        let fields = DialogFields::from_pairs([(CATEGORY, category), (NAME, name), (LINE, DEFAULT_LINE.to_owned())]);
        let shown_file = file.clone();
        let dialog = HostedParamDialog::new(ctx, DIALOG_KEY, fields, move |fields| {
            ParamDialog::new(DIALOG_KEY, tl!("Create Custom Stamp"), create_params(&shown_file, &existing))
                .ok_enabled(fields_ok(fields))
                .values(Bind::bound(fields))
        });
        let id = crate::dialogs::host::open(self, dialog);
        self.stamp_dialog = Some(CreateStampDialog { id, file, data: Arc::new(bytes) });
    }

    /// Takes the Create Custom Stamp dialog's answer: OK saves the stamp in the library and chooses it.
    pub(crate) fn create_stamp_pass(&mut self, _ctx: &egui::Context) {
        let Some(id) = self.stamp_dialog.as_ref().map(|d| d.id) else { return };
        // `DialogResult` is `#[non_exhaustive]`: whatever is not an OK closes the dialog without a stamp.
        let Some(answer) = self.dialogs.take_by_id::<HostedParamDialog>(id) else { return };
        match answer.ok() {
            Some(result) => {
                if let Some(d) = self.stamp_dialog.take() {
                    self.save_custom_stamp(d, &result.fields);
                }
            }
            None => self.stamp_dialog = None,
        }
    }

    /// Save the dialog's stamp in the library and choose it.
    fn save_custom_stamp(&mut self, d: CreateStampDialog, fields: &DialogFields) {
        let text = |key: &str| fields.text(key).unwrap_or_default().trim().to_string();
        let line = fields.bool(DYNAMIC).unwrap_or(false).then(|| text(LINE)).filter(|l| !l.is_empty());
        self.custom_stamps.push(CustomStamp { category: text(CATEGORY), name: text(NAME), file: d.file, page: 0, data: d.data, line });
        self.quick_tool = QuickTool::CustomStamp(self.custom_stamps.len() - 1);
    }
}

/// The custom stamps for `persist` (kept small: base64 of each file).
pub(crate) fn encode(stamps: &[CustomStamp]) -> serde_json::Value {
    serde_json::to_value(stamps).unwrap_or_default()
}

/// Read stamps back; malformed entries are dropped.
pub(crate) fn decode(v: &serde_json::Value) -> Vec<CustomStamp> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| serde_json::from_value::<CustomStamp>(x.clone()).ok()).filter(|s| s.data.len() <= MAX_STAMP_BYTES).collect())
        .unwrap_or_default()
}
