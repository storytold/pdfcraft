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
    /// Place the resolved line over the image's lower edge. Older saved stamps keep the band.
    #[serde(default)]
    pub line_over_picture: bool,
    /// A date pattern for `{date}`; `None` keeps the original stamp date style.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_format: Option<String>,
    /// Overlay text anchor in normalized picture coordinates, from top-left.
    #[serde(default = "default_line_position")]
    pub line_position: [f64; 2],
}

fn default_line_position() -> [f64; 2] {
    [0.5, 0.9]
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
const LINE_OVER_PICTURE: &str = "line_over_picture";
const DATE_FORMAT: &str = "date_format";
const LINE_X: &str = "line_x";
const LINE_Y: &str = "line_y";

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
                line_over_picture: stamp.line_over_picture,
                line_position: stamp.line_position,
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

#[derive(Clone)]
struct StampPreviewDate {
    fallback: String,
    today: (i64, u32, u32),
    language: String,
}

/// The dialog's rows over `file` and the existing `categories`.
fn create_params(
    file: &str,
    categories: &[String],
    preview: Option<egui::TextureHandle>,
    preview_size: [usize; 2],
    author: String,
    date: StampPreviewDate,
    time: String,
) -> Vec<Param> {
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
        Param::new(LINE_OVER_PICTURE, tl!("Place line over picture"), ParamKind::bool(false)).visible_when(DYNAMIC, true),
        Param::custom(LINE_X, tl!("Horizontal position"), |ui, fields, _| {
            let mut value = fields.text(LINE_X).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.5).clamp(0.0, 1.0);
            let response = ui.add(egui::Slider::new(&mut value, 0.0..=1.0).text(tl!("Horizontal position")));
            fields.set(LINE_X, value.to_string());
            response
        })
        .visible_when(LINE_OVER_PICTURE, true),
        Param::custom(LINE_Y, tl!("Vertical position"), |ui, fields, _| {
            let mut value = fields.text(LINE_Y).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.9).clamp(0.0, 1.0);
            let response = ui.add(egui::Slider::new(&mut value, 0.0..=1.0).text(tl!("Vertical position")));
            fields.set(LINE_Y, value.to_string());
            response
        })
        .visible_when(LINE_OVER_PICTURE, true),
        Param::custom(DATE_FORMAT, tl!("Date format"), |ui, fields, _| {
            let mut value = fields.text(DATE_FORMAT).unwrap_or("mmm d, yyyy").to_owned();
            let shown = TextField::new(tl!("Date format"), Bind::bound(&mut value)).limit(pdfcraft_engine::dates::MAX_DATE_FORMAT_CHARS).show(ui);
            fields.set(DATE_FORMAT, value);
            shown.response
        })
        .visible_when(DYNAMIC, true),
        Param::read_only_text("hint", "", tl!("{name}, {date} and {time} become your name, the date and the time."))
            .full_width()
            .visible_when(DYNAMIC, true),
        Param::custom("preview", tl!("Preview"), move |ui, fields, _| {
            let Some(texture) = preview.as_ref() else { return ui.label(tl!("Preview unavailable for this picture")) };
            let dynamic = fields.bool(DYNAMIC).unwrap_or(false);
            let overlay = dynamic && fields.bool(LINE_OVER_PICTURE).unwrap_or(false);
            let template = fields.text(LINE).unwrap_or_default();
            let format = fields.text(DATE_FORMAT).unwrap_or("mmm d, yyyy");
            let formatted_date = pdfcraft_engine::dates::check_date_format(format)
                .ok()
                .and_then(|fmt| {
                    pdfcraft_engine::dates::date_language(&date.language).map(|lang| pdfcraft_engine::dates::format_day(fmt, date.today, lang))
                })
                .unwrap_or_else(|| date.fallback.clone());
            let filled = pdfcraft_engine::Session::stamp_line_with_date_time(template, &author, &formatted_date, &time);
            let source = egui::vec2(preview_size[0] as f32, preview_size[1] as f32);
            let natural = if dynamic && !overlay {
                let (w, h) = pdfcraft_engine::custom_stamp_size((f64::from(source.x), f64::from(source.y)), Some(&filled));
                egui::vec2(w as f32, h as f32)
            } else {
                source
            };
            let scale = (280.0 / natural.x).min(160.0 / natural.y);
            let picture = source * scale;
            let canvas = natural * scale;
            let band = if dynamic { canvas.y * pdfcraft_engine::CUSTOM_LINE_BAND as f32 } else { 0.0 };
            let picture_height = canvas.y - if overlay { 0.0 } else { band };
            let (rect, response) = ui.allocate_exact_size(canvas, if overlay { egui::Sense::drag() } else { egui::Sense::hover() });
            let picture_rect =
                egui::Rect::from_min_size(egui::pos2(rect.center().x - picture.x / 2.0, rect.min.y + (picture_height - picture.y) / 2.0), picture);
            let painter = ui.painter().with_clip_rect(rect);
            painter.image(texture.id(), picture_rect, egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            if dynamic {
                let mut text_size = band * pdfcraft_engine::CUSTOM_LINE_TEXT as f32;
                let width = painter.layout_no_wrap(filled.clone(), egui::FontId::proportional(text_size), egui::Color32::from_gray(51)).size().x;
                if width > canvas.x - 4.0 && width > 0.0 {
                    text_size *= ((canvas.x - 4.0) / width).max(0.0);
                }
                let mut position = [0.5_f32, 0.9_f32];
                if overlay {
                    if let Some(x) = fields.text(LINE_X).and_then(|v| v.parse::<f32>().ok()) {
                        position[0] = x.clamp(0.0, 1.0);
                    }
                    if let Some(y) = fields.text(LINE_Y).and_then(|v| v.parse::<f32>().ok()) {
                        position[1] = y.clamp(0.0, 1.0);
                    }
                    if response.dragged()
                        && let Some(pointer) = response.interact_pointer_pos()
                    {
                        position =
                            [((pointer.x - rect.min.x) / rect.width()).clamp(0.0, 1.0), ((pointer.y - rect.min.y) / rect.height()).clamp(0.0, 1.0)];
                        fields.set(LINE_X, position[0].to_string());
                        fields.set(LINE_Y, position[1].to_string());
                    }
                }
                let at = if overlay {
                    egui::pos2(rect.min.x + rect.width() * position[0], rect.min.y + rect.height() * position[1])
                } else {
                    egui::pos2(rect.center().x, rect.max.y - band / 2.0)
                };
                painter.text(at, egui::Align2::CENTER_CENTER, &filled, egui::FontId::proportional(text_size), egui::Color32::from_gray(51));
                if overlay {
                    ui.label(tl!("Drag the date on the preview to position it"));
                }
                ui.label(crate::i18n::fmt(tl!("Preview: {line}"), &[("line", &filled)]));
            }
            response
        })
        .full_width(),
    ]
}

/// Whether the dialog's values make a stamp: a category and a name, and a line when one is asked for.
fn fields_ok(fields: &DialogFields) -> bool {
    let filled = |key: &str| fields.text(key).is_some_and(|t| !t.trim().is_empty());
    filled(CATEGORY)
        && filled(NAME)
        && (!fields.bool(DYNAMIC).unwrap_or(false)
            || (filled(LINE) && fields.text(DATE_FORMAT).is_some_and(|f| pdfcraft_engine::dates::check_date_format(f).is_ok())))
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
        let preview_image = file.to_ascii_lowercase().ends_with(".png").then(|| pdfcraft_engine::SignatureImage::from_bytes(&bytes).ok()).flatten();
        let preview_size = preview_image.as_ref().map_or([0, 0], pdfcraft_engine::SignatureImage::size);
        let preview = preview_image.map(|image| {
            ctx.load_texture(
                "custom-stamp-preview",
                egui::ColorImage::from_rgba_unmultiplied(image.size(), image.rgba()),
                egui::TextureOptions::LINEAR,
            )
        });
        let category = self.custom_stamps.last().map(|s| s.category.clone()).unwrap_or_else(|| "My stamps".into());
        let existing = categories(&self.custom_stamps);
        let fields = DialogFields::from_pairs([
            (CATEGORY, category),
            (NAME, name),
            (LINE, DEFAULT_LINE.to_owned()),
            (DATE_FORMAT, "mmm d, yyyy".to_owned()),
            (LINE_X, "0.5".to_owned()),
            (LINE_Y, "0.9".to_owned()),
        ]);
        let shown_file = file.clone();
        let author = self.comment_prefs.author.clone();
        let date = StampPreviewDate {
            fallback: self.session.stamp_line("{date}", ""),
            today: self.session.today(),
            language: self.session.date_language().unwrap_or(crate::i18n::current().code()).to_owned(),
        };
        let time = self.session.stamp_line("{time}", "");
        let dialog = HostedParamDialog::new(ctx, DIALOG_KEY, fields, move |fields| {
            ParamDialog::new(
                DIALOG_KEY,
                tl!("Create Custom Stamp"),
                create_params(&shown_file, &existing, preview.clone(), preview_size, author.clone(), date.clone(), time.clone()),
            )
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
        let line_over_picture = line.is_some() && fields.bool(LINE_OVER_PICTURE).unwrap_or(false);
        let date_format = line.as_ref().map(|_| text(DATE_FORMAT));
        let line_position = [LINE_X, LINE_Y].map(|key| fields.text(key).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.5).clamp(0.0, 1.0));
        self.custom_stamps.push(CustomStamp {
            category: text(CATEGORY),
            name: text(NAME),
            file: d.file,
            page: 0,
            data: d.data,
            line,
            line_over_picture,
            date_format,
            line_position,
        });
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
