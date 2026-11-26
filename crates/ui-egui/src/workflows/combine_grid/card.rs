//! The tiles of the Combine grid and the page shown large: a file's tile (a stack of sheets when it
//! takes more than one page), a page tile of an expanded file, and the preview over the window.

use std::sync::{Arc, Mutex};

use craft::behavior::IconId;
use craft::behavior::kurbo::Size;
use craft::pages::ui::{PagePreview, PageSource};
use craft::theme::variant::Tone;
use craft::ui::contract::CommandSet;
use craft::ui::list::ThumbAspect;
use craft::ui::primitives::textures::{Priority, Revision, ThumbPaint, ThumbRender, ThumbRequest, ThumbSource, Thumbnails};
use craft::ui::style::DataColor;
use craft::ui::tile_grid::{BadgeKind, BadgeSlot, StackLook, Tile, TileBadge, TileStatus};

use super::source::{CombineThumbs, OpenDocuments, page_job, page_size};
use super::{MAX_EXPAND, PdfCraftApp};
use crate::combine_ui::{CombineFile, CombineKey, Lock};

/// The actions a tile offers in its band, and the rows of its menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, CommandSet)]
#[command(prefix = "combineTile")]
pub(super) enum TileCommand {
    #[command(label = "Preview")]
    Preview,
    #[command(label = "Expand")]
    Expand,
    #[command(label = "Collapse")]
    Collapse,
    #[command(label = "Remove")]
    Remove,
}

/// One of a tile's own actions. A tile lists the ones that apply, in this order, and the grid reports a
/// click as the index into that list: [`file_acts`] and [`page_acts`] are the one place it is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Act {
    /// Show it large.
    Preview,
    /// A file as its pages.
    Expand,
    /// An expanded file's pages as the file's tile.
    Collapse,
    /// Take it out of the list: the file, or the page.
    Remove,
}

impl Act {
    fn command(self) -> TileCommand {
        match self {
            Act::Preview => TileCommand::Preview,
            Act::Expand => TileCommand::Expand,
            Act::Collapse => TileCommand::Collapse,
            Act::Remove => TileCommand::Remove,
        }
    }

    /// Its row in the tile's menu, as the band's button names it.
    pub(super) fn label(self) -> &'static str {
        match self {
            Act::Preview => tl!("Preview"),
            Act::Expand => tl!("Expand"),
            Act::Collapse => tl!("Collapse"),
            Act::Remove => tl!("Remove"),
        }
    }

    pub(super) fn icon(self) -> IconId {
        IconId::new(match self {
            Act::Preview => "search",
            Act::Expand => "chevron_down",
            Act::Collapse => "chevron_up",
            Act::Remove => "trash",
        })
    }
}

/// The page a file's tile shows, when one can be drawn: the first the range takes, in the file, with a size.
fn file_picture(f: &CombineFile) -> Option<(f32, f32)> {
    let page = f.first_taken().filter(|p| *p < f.pages)?;
    page_size(&f.sizes, page)
}

/// The actions a file's tile (or the row for it) offers. `check` is what its range takes.
pub(super) fn file_acts(f: &CombineFile, check: &Result<usize, String>) -> Vec<Act> {
    let readable = f.lock.is_none() && f.problem.is_none() && check.is_ok();
    let takes = check.as_ref().map_or(f.pages, |k| *k);
    let mut acts = Vec::new();
    if readable && file_picture(f).is_some() {
        acts.push(Act::Preview);
    }
    // A file that adds more pages than one can be spread out into them.
    if readable && takes > 1 && takes <= MAX_EXPAND {
        acts.push(Act::Expand);
    }
    acts.push(Act::Remove);
    acts
}

/// The actions the tile of page `page` (0-based, in the file) offers.
pub(super) fn page_acts(f: &CombineFile, page: usize) -> Vec<Act> {
    let mut acts = Vec::new();
    if page < f.pages && page_size(&f.sizes, page).is_some() {
        acts.push(Act::Preview);
    }
    acts.push(Act::Collapse);
    acts.push(Act::Remove);
    acts
}

/// Where a tile's picture comes from.
#[derive(Clone, Copy)]
pub(super) struct Pictures<'a> {
    pub(super) thumbs: &'a Thumbnails<CombineKey>,
    pub(super) source: &'a CombineThumbs,
}

/// The shape a page of `size` points is drawn in.
fn aspect_of((w, h): (f32, f32)) -> ThumbAspect {
    let side = |v: f32| v.round().clamp(1.0, f32::from(u16::MAX)) as u16;
    ThumbAspect::new(side(w), side(h))
}

/// What a tile shows on its picture.
enum Face {
    /// The page, drawn at this size in points.
    Page((f32, f32)),
    /// Why there is no page: an icon.
    Icon(&'static str),
}

impl<'a> Pictures<'a> {
    /// `tile` showing `face`: the picture of `key`, or the icon.
    fn put(self, tile: Tile<'a>, key: CombineKey, face: Face) -> Tile<'a> {
        match face {
            Face::Icon(icon) => tile.icon(IconId::new(icon)),
            Face::Page(size) => tile.thumb(aspect_of(size), move |cx, rect| {
                let ctx = cx.painter.ctx().clone();
                let request = ThumbRequest::for_cell(rect.width().max(rect.height()), cx.painter.pixels_per_point());

                let state = self.thumbs.state(&ctx, self.source, &key, &request, Priority::VISIBLE);
                // A page is white paper in either theme.
                self.thumbs.paint(cx, rect, &state, &ThumbPaint::default().paper(DataColor::new(egui::Color32::WHITE)));
            }),
        }
    }
}

/// The tile of a file shown as one: its first page (a stack of sheets when it takes several), its name,
/// the pages it takes, and what is wrong with it. `check` is what its range takes; `twice` that the same
/// file is in the list again.
pub(super) fn file_tile<'a>(f: &'a CombineFile, check: &Result<usize, String>, twice: bool, pictures: Pictures<'a>) -> Tile<'a> {
    let takes = check.as_ref().map_or(f.pages, |k| *k);
    let mut tile = Tile::new(f.name.clone());
    // What a screen reader hears after the name: everything the picture and the detail line only show.
    let mut says: Vec<String> = Vec::new();
    let (face, detail) = if let Some(lock) = f.lock {
        let reason = match lock {
            Lock::Open => tl!("Password-protected"),
            Lock::Permissions => tl!("Its security settings don't allow copying pages"),
        };
        says.push(reason.to_string());
        (Face::Icon("lock"), Some(reason.to_string()))
    } else if let Some(problem) = &f.problem {
        tile = tile.status(TileStatus::Unreadable);
        says.push(problem.clone());
        (Face::Icon("error"), Some(problem.clone()))
    } else if let Err(why) = check {
        tile = tile.status(TileStatus::Failed);
        says.push(why.clone());
        // Page ranges are typed in the list.
        says.push(tl!("Switch to List view to change the pages it takes").to_string());
        (Face::Icon("error"), Some(tl!("Check the pages").to_string()))
    } else {
        let detail = (!f.range.trim().is_empty())
            .then(|| crate::i18n::fmt(tl!("Pages: {k} of {n}"), &[("k", &takes.to_string()), ("n", &f.pages.to_string())]));
        match file_picture(f) {
            Some(size) => (Face::Page(size), detail),
            // The pages couldn't be counted, or the page's size is unknown (some passwords open a file
            // for combining but not for reading it here): nothing is drawn (the engine decides at
            // Combine), and the tile says so rather than look blank.
            None => {
                says.push(tl!("No preview").to_string());
                (Face::Icon("eye_off"), detail.or_else(|| Some(tl!("No preview").to_string())))
            }
        }
    };
    let readable = f.lock.is_none() && f.problem.is_none() && check.is_ok();
    // A warning that doesn't stop it being combined.
    if readable && (!f.notes.is_empty() || twice) {
        let mut lines: Vec<String> = f.notes.clone();
        if twice {
            lines.push(tl!("Added more than once").to_string());
        }
        says.extend(lines.iter().cloned());
        tile = tile.badge(TileBadge::new(BadgeSlot::EndTop, BadgeKind::Glyph(IconId::new("warning")), Tone::Warn, Some(lines.join("\n").into())));
    }
    if takes > 1 {
        tile = tile.stack(takes).stack_look(StackLook::Sheets);
    }
    if let Some(detail) = detail {
        tile = tile.detail(detail);
    }
    // The whole name alone for a file with nothing to say: the tile is found and read by it.
    let accessible = if says.is_empty() { f.name.clone() } else { format!("{}. {}", f.name, says.join(". ")) };
    tile = pictures.put(tile.accessible_name(accessible), CombineKey::File(f.id), face);
    for act in file_acts(f, check) {
        tile = tile.action(act.icon(), act.command());
    }
    tile
}

/// The tile of page `page` (0-based, in the file) of an expanded file, `nth` among the pages it adds:
/// "Page 3" and the file's name under it.
pub(super) fn page_tile<'a>(f: &'a CombineFile, nth: usize, page: usize, pictures: Pictures<'a>) -> Tile<'a> {
    let label = crate::i18n::fmt(tl!("Page {label}"), &[("label", &page.saturating_add(1).to_string())]);
    let size = (page < f.pages).then(|| page_size(&f.sizes, page)).flatten();
    let face = size.map_or(Face::Icon("eye_off"), Face::Page);
    // Named by its page and its file, so it is found and read as one of that file's pages.
    let tile = Tile::new(label.clone()).detail(f.name.clone()).accessible_name(format!("{label} · {}", f.name));
    let mut tile = pictures.put(tile, CombineKey::Page(f.id, nth), face);
    for act in page_acts(f, page) {
        tile = tile.action(act.icon(), act.command());
    }
    tile
}

/// One Combine file's pages as the preview's page source: its pictures render through the same job path
/// as the grid's, on the tab's open documents.
struct FilePages {
    bytes: Arc<Vec<u8>>,
    auth: u64,
    password: Option<Arc<str>>,
    sizes: Arc<[(f32, f32)]>,
    count: usize,
    documents: Arc<Mutex<OpenDocuments>>,
}

impl ThumbSource<usize> for FilePages {
    fn revision(&self, page: &usize, _: &ThumbRequest) -> Revision {
        Revision::of(&(self.auth, *page))
    }

    fn render(&self, page: &usize, request: &ThumbRequest) -> ThumbRender {
        let Some((w, h)) = page_size(&self.sizes, *page) else { return ThumbRender::Blank };
        let scale = request.side as f32 / w.max(h);
        if !(scale.is_finite() && scale > 0.0) {
            return ThumbRender::Blank;
        }
        ThumbRender::Later(page_job(self.documents.clone(), self.bytes.clone(), self.auth, self.password.clone(), *page, scale))
    }
}

impl PageSource for FilePages {
    fn page_count(&self) -> usize {
        self.count
    }

    /// A page whose size is unknown is drawn as Letter paper, and nothing is rendered for it.
    fn page_size(&self, page: usize) -> Size {
        let (w, h) = page_size(&self.sizes, page).unwrap_or((612.0, 792.0));
        Size::new(f64::from(w), f64::from(h))
    }
}

/// The pages a file adds as the preview's whole document: it counts and numbers them 1..n as the production
/// preview does ("Page 1 of 3" for a range of three), where `pages` maps each one to its page in the file.
struct RangePages<'a> {
    inner: &'a FilePages,
    pages: &'a [usize],
}

impl ThumbSource<usize> for RangePages<'_> {
    fn revision(&self, page: &usize, request: &ThumbRequest) -> Revision {
        match self.pages.get(*page) {
            Some(real) => self.inner.revision(real, request),
            None => Revision::of(&u8::MAX),
        }
    }

    fn render(&self, page: &usize, request: &ThumbRequest) -> ThumbRender {
        match self.pages.get(*page) {
            Some(real) => self.inner.render(real, request),
            None => ThumbRender::Blank,
        }
    }
}

impl PageSource for RangePages<'_> {
    fn page_count(&self) -> usize {
        self.pages.len()
    }

    fn page_size(&self, page: usize) -> Size {
        self.pages.get(page).map_or_else(|| Size::new(612.0, 792.0), |real| self.inner.page_size(*real))
    }
}

/// The file a tile's Preview opened, shown large over the window: one of the pages it adds at a time,
/// stepped with the buttons or the keys; Escape or Close closes it. It closes by itself when the file
/// leaves the list, is locked again or has nothing to show.
pub(crate) fn preview(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let Some(p) = app.combine_tab.preview.as_mut() else { return };
    let Some(f) = app.combine_draft.iter().find(|f| f.id == p.file).filter(|f| f.lock.is_none() && f.problem.is_none()) else {
        app.combine_tab.preview = None;
        return;
    };
    // The pages it adds, worked out again only when its range changes.
    let pages = match &p.pages {
        Some(((range, count), pages)) if *range == f.range && *count == f.pages => pages.clone(),
        _ => match f.pages_taken() {
            Ok(pages) => {
                let pages: Arc<[usize]> = Arc::from(pages);
                p.pages = Some(((f.range.clone(), f.pages), pages.clone()));
                pages
            }
            Err(_) => {
                app.combine_tab.preview = None;
                return;
            }
        },
    };
    p.at = p.at.min(pages.len().saturating_sub(1));
    let Some(&page) = pages.get(p.at) else {
        app.combine_tab.preview = None;
        return;
    };
    p.page = Some(page);
    let name = f.name.clone();
    let source = FilePages {
        bytes: f.bytes.clone(),
        auth: f.auth,
        password: f.password().map(Arc::from),
        sizes: f.sizes.clone(),
        count: f.pages,
        documents: app.combine_tab.open_documents.clone(),
    };
    let mut at = p.at;
    let numbered: Vec<usize> = (0..pages.len()).collect();
    let ranged = RangePages { inner: &source, pages: &pages };
    let shown = PagePreview::new(&ranged, &numbered, &mut at, DataColor::new(egui::Color32::WHITE)).title(name).show(ui);
    p.at = at;
    p.page = pages.get(at).copied();
    if shown.closed {
        app.combine_tab.preview = None;
    }
}
