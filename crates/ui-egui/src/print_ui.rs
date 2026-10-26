//! The Print dialog (Acrobat's File ▸ Print, execution plan M10.5): printer, copies, grayscale;
//! pages to print (all, current, range with labels, the selected pages; odd/even, reverse); page sizing & handling
//! (Size, Poster, Multiple, Booklet); orientation; comments & forms; and a live preview of the
//! sheets. Printing sends the print-ready PDF to the system spooler; "Save as PDF" writes it.

use egui::{Color32, Pos2, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::print::{self, Binding, BookletSubset, Content, Layout, Orientation, PAPERS, PageOrder, SizeMode, Subset, spool};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// The preview pane, and the paper inside it (12 pt inset on each side).
const PREVIEW_W: f32 = 320.0;
const PREVIEW_H: f32 = 380.0;
const PREVIEW_INSET: f32 = 12.0;
pub(crate) const PREVIEW_BOX: (f32, f32) = (PREVIEW_W - 2.0 * PREVIEW_INSET, PREVIEW_H - 2.0 * PREVIEW_INSET);
/// Label printing reads the printed area of at most this many pages (each is a quick render on
/// the UI thread); further pages print whole.
pub(crate) const LABEL_CROP_PAGES: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    All,
    Current,
    Range,
    /// The pages picked in the Pages panel or the organize grid when the dialog opened.
    Selected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handling {
    Size,
    Poster,
    Multiple,
    Booklet,
    /// Shipping label: cut to the printed area, turned upright, one per sheet.
    Label,
}

/// The sizes a custom paper may have, in points: from 1 in (the least the engine lays out on) to
/// PDF's largest page, 200 in.
pub const CUSTOM_PAPER_RANGE: std::ops::RangeInclusive<f64> = 72.0..=14_400.0;

/// The most characters the Paper list's search keeps.
const PAPER_QUERY_MAX: usize = 64;

/// The sections of the Paper list, in the order shown (see [`paper_group`]).
const PAPER_GROUPS: [&str; 8] =
    ["ISO A", "ISO B", "Print sheets (RA, SRA)", "North American", "Envelopes", "Japan and China", "Photo, cards and labels", "Other"];

/// The [`PAPER_GROUPS`] section a `PAPERS` name belongs to.
fn paper_group(name: &str) -> usize {
    let first = name.split(' ').next().unwrap_or(name);
    let numbered = |prefix: &str| first.strip_prefix(prefix).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    if name.contains("Envelope") {
        4
    } else if numbered("A") || first == "2A0" {
        0
    } else if numbered("B") {
        1
    } else if numbered("RA") || numbered("SRA") {
        2
    } else if ["US", "Tabloid", "ARCH", "ANSI", "Executive", "Statement", "Government", "Foolscap", "Super"].contains(&first) {
        3
    } else if ["JIS", "Hagaki", "16K"].contains(&first) {
        5
    } else if ["Photo", "Index"].contains(&first) || name.contains("label") {
        6
    } else {
        7
    }
}

/// The most paper sizes the user can save.
pub const SAVED_PAPERS_MAX: usize = 200;

/// The most characters a saved paper's name or category keeps.
const SAVED_PAPER_TEXT_MAX: usize = 80;

/// The category a saved paper gets when none is typed.
const SAVED_PAPER_CATEGORY: &str = "My sizes";

/// A paper size the user named, put in a category and saved (Print ▸ Custom ▸ Save size…). It is
/// listed with the papers and kept with the settings.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedPaper {
    pub name: String,
    pub category: String,
    /// Points, width × height as typed; within [`CUSTOM_PAPER_RANGE`].
    pub size: (f64, f64),
}

impl SavedPaper {
    /// A saved paper from what was typed: the name and category trimmed and cut to
    /// [`SAVED_PAPER_TEXT_MAX`] characters, a blank category becoming [`SAVED_PAPER_CATEGORY`].
    /// `None` for a blank name or a size outside [`CUSTOM_PAPER_RANGE`].
    pub fn new(name: &str, category: &str, size: (f64, f64)) -> Option<Self> {
        let text = |s: &str| s.trim().chars().filter(|c| !c.is_control()).take(SAVED_PAPER_TEXT_MAX).collect::<String>();
        let (name, category) = (text(name), text(category));
        let fits = CUSTOM_PAPER_RANGE.contains(&size.0) && CUSTOM_PAPER_RANGE.contains(&size.1);
        (!name.is_empty() && fits).then(|| SavedPaper {
            name,
            category: if category.is_empty() { SAVED_PAPER_CATEGORY.to_string() } else { category },
            size,
        })
    }
}

/// A paper in the Paper list: one of `PAPERS`, or one of the user's saved papers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaperRef {
    Listed(usize),
    Saved(usize),
}

/// A section heading in the Paper list: one of [`PAPER_GROUPS`], or a category of the user's.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Heading {
    Group(usize),
    Category(String),
}

/// The papers whose name has `query` in it (ignoring case and spaces, so "sra3" finds
/// "SRA3 (320 x 450 mm)" and "320x450" its size), by section, largest paper first within each.
/// The user's own categories come first (alphabetically), then [`PAPER_GROUPS`] in order; a
/// saved paper whose category names one of those goes in with the listed papers. A saved paper
/// is also found by its category. Every paper when `query` is blank.
fn paper_sections(query: &str, saved: &[SavedPaper]) -> Vec<(Heading, Vec<PaperRef>)> {
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect::<String>();
    let query = squash(query);
    let mut sections: Vec<(Heading, Vec<PaperRef>)> = Vec::new();
    let mut add = |heading: Heading, paper: PaperRef| match sections.iter_mut().find(|(h, _)| *h == heading) {
        Some((_, items)) => items.push(paper),
        None => sections.push((heading, vec![paper])),
    };
    for (i, (name, _)) in PAPERS.iter().enumerate() {
        if squash(name).contains(&query) || squash(tl!(name)).contains(&query) {
            add(Heading::Group(paper_group(name)), PaperRef::Listed(i));
        }
    }
    for (i, paper) in saved.iter().enumerate() {
        if squash(&paper.name).contains(&query) || squash(&paper.category).contains(&query) {
            let group = PAPER_GROUPS.iter().position(|g| g.eq_ignore_ascii_case(&paper.category) || tl!(g).eq_ignore_ascii_case(&paper.category));
            add(group.map_or_else(|| Heading::Category(paper.category.clone()), Heading::Group), PaperRef::Saved(i));
        }
    }
    sections.sort_by(|(a, _), (b, _)| match (a, b) {
        (Heading::Category(a), Heading::Category(b)) => a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)),
        (Heading::Category(_), Heading::Group(_)) => std::cmp::Ordering::Less,
        (Heading::Group(_), Heading::Category(_)) => std::cmp::Ordering::Greater,
        (Heading::Group(a), Heading::Group(b)) => a.cmp(b),
    });
    let area = |p: &PaperRef| match p {
        PaperRef::Listed(i) => PAPERS.get(*i).map_or(0.0, |(_, (w, h))| w * h),
        PaperRef::Saved(i) => saved.get(*i).map_or(0.0, |s| s.size.0 * s.size.1),
    };
    let order = |p: &PaperRef| match p {
        PaperRef::Listed(i) => (0, *i),
        PaperRef::Saved(i) => (1, *i),
    };
    for (_, items) in &mut sections {
        items.sort_by(|a, b| area(b).total_cmp(&area(a)).then(order(a).cmp(&order(b))));
    }
    sections
}

/// The Save size… form under the Paper row: the name and category being typed.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SavePaperForm {
    pub name: String,
    pub category: String,
    /// The category is being typed rather than picked from the existing ones.
    pub new_category: bool,
    /// The name field takes the keyboard on the next frame (the form was just opened).
    pub focus_name: bool,
    /// Why the last Save was refused.
    pub error: Option<&'static str>,
}

/// The units sizes are shown and typed in (mm, cm, in, pt); the same choice as Preferences ▸
/// Units, which the app takes back from the draft.
fn unit_combo(ui: &mut egui::Ui, units: &mut crate::units::Unit) {
    egui::ComboBox::from_id_salt("print-units")
        .selected_text(units.label())
        .width(56.0)
        .show_ui(ui, |ui| {
            for u in crate::units::Unit::ALL {
                ui.selectable_value(units, u, u.label());
            }
        })
        .response
        .on_hover_text(tl!("Units"));
}

/// The row under Paper while a custom size is in use: Save size… (and Delete for a saved
/// paper), or the form that names the size and puts it in a category.
fn save_paper_row(ui: &mut egui::Ui, d: &mut PrintDraft, t: &Tokens) {
    let categories = d.paper_categories();
    let Some(form) = d.save_form.as_mut() else {
        if ui.button(tl!("Save size…")).on_hover_text(tl!("Name this size and keep it in the Paper list.")).clicked() {
            let saved = d.custom_name.as_ref().and_then(|n| d.saved_papers.iter().find(|p| &p.name == n));
            d.save_form = Some(SavePaperForm {
                name: saved.map(|p| p.name.clone()).unwrap_or_default(),
                category: saved.map_or_else(|| SAVED_PAPER_CATEGORY.to_string(), |p| p.category.clone()),
                focus_name: true,
                ..SavePaperForm::default()
            });
        }
        if let Some(name) = d.custom_name.clone()
            && ui.button(tl!("Delete")).on_hover_text(crate::i18n::fmt(tl!("Remove {name} from the Paper list."), &[("name", &name)])).clicked()
        {
            d.delete_saved_paper();
        }
        return;
    };
    let name = ui.add(widgets::line(&mut form.name).hint_text(tl!("Name")).desired_width(170.0)).on_hover_text(tl!("Name"));
    if std::mem::take(&mut form.focus_name) {
        name.request_focus();
    }
    if form.new_category {
        ui.add(widgets::line(&mut form.category).hint_text(tl!("New category")).desired_width(150.0)).on_hover_text(tl!("Category"));
    } else {
        egui::ComboBox::from_id_salt("paper-category").selected_text(tl!(form.category.as_str())).width(150.0).show_ui(ui, |ui| {
            for c in &categories {
                if ui.selectable_label(form.category == *c, tl!(c.as_str())).clicked() {
                    form.category = c.clone();
                }
            }
            ui.separator();
            if ui.selectable_label(false, tl!("New category…")).clicked() {
                form.new_category = true;
                form.category.clear();
            }
        });
    }
    let named = !form.name.trim().is_empty();
    let enter = name.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let save = ui.add_enabled(named, egui::Button::new(tl!("Save"))).clicked() || (enter && named);
    let cancel = ui.button(tl!("Cancel")).clicked();
    if let Some(error) = form.error {
        ui.label(egui::RichText::new(tl!(error)).small().color(t.text_muted));
    }
    let (typed_name, typed_category) = (form.name.clone(), form.category.clone());
    if save
        && let Err(error) = d.save_paper(&typed_name, &typed_category)
        && let Some(form) = d.save_form.as_mut()
    {
        form.error = Some(error);
    }
    if cancel {
        d.save_form = None;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PrintDraft {
    pub printers: Vec<spool::Printer>,
    /// The unit the sheet size is shown in (Preferences).
    pub units: crate::units::Unit,
    /// The printers are still being listed in the background (see [`PrinterSource`]).
    pub printers_loading: bool,
    /// The user chose the printer, so a list arriving late doesn't move it to the default.
    pub printer_picked: bool,
    /// `None` = Save as PDF.
    pub printer: Option<String>,
    pub copies: u32,
    pub collate: bool,
    pub grayscale: bool,
    /// `None` = the printer's own default (its Properties), as Acrobat does.
    pub duplex: Option<spool::Duplex>,
    pub which: Which,
    pub range: String,
    /// The pages `Which::Selected` prints (0-based, in page order); empty when none were picked.
    pub selected: Vec<usize>,
    pub subset: Subset,
    pub reverse: bool,
    pub handling: Handling,
    pub size: SizeMode,
    pub custom_scale: f64,
    pub per_sheet: usize,
    pub order: PageOrder,
    pub border: bool,
    pub auto_rotate: bool,
    pub booklet_subset: BookletSubset,
    pub binding: Binding,
    pub poster_scale: f64,
    pub overlap: f64,
    pub cut_marks: bool,
    pub orientation: Orientation,
    pub content: Content,
    /// Print as image (Acrobat's Advanced Print Setup) at `dpi`.
    pub as_image: bool,
    pub dpi: f64,
    pub paper: usize,
    /// A paper of the user's own size (points, as typed: width × height), used instead of
    /// `PAPERS[paper]` while set. Always within [`CUSTOM_PAPER_RANGE`].
    pub custom_paper: Option<(f64, f64)>,
    /// What is typed in the Paper list's search field.
    pub paper_query: String,
    /// The paper sizes the user saved (see [`SavedPaper`]), at most [`SAVED_PAPERS_MAX`].
    pub saved_papers: Vec<SavedPaper>,
    /// The saved paper `custom_paper` came from, while its size is unchanged.
    pub custom_name: Option<String>,
    /// The Save size… form, while it is open.
    pub save_form: Option<SavePaperForm>,
    /// The paper chosen before Label put label stock in, so leaving Label gives it back (#3564).
    pub paper_before_label: Option<usize>,
    /// The sheet shown in the preview (0-based).
    pub sheet: usize,
    pub current_page: usize,
    /// The printer driver's own options and the printer they were read for (Properties…, CUPS).
    pub driver_options: Option<(String, Vec<spool::PrinterOption>)>,
    /// Driver options set away from the printer's defaults, for `driver_options`' printer.
    pub driver_choices: std::collections::BTreeMap<String, String>,
    pub show_driver_options: bool,
    /// Why the printer's preferences window didn't open (Windows).
    pub driver_error: Option<String>,
    /// Label: turn a further 180°.
    pub label_flip: bool,
    /// Label: each page's printed area and text direction, read when Label is chosen
    /// ([`PdfCraftApp::read_label_crops`]); empty until then.
    pub crops: Vec<Option<print::Crop>>,
    /// The pages `crops` were read for, so a changed page selection reads them again.
    pub crop_pages: Option<Vec<usize>>,
    /// The printer whose options are being read in the background (CUPS runs `lpoptions`,
    /// which a stuck CUPS can hold up; #1818). The panel says so meanwhile.
    pub driver_options_loading: Option<String>,
    /// Properties… asked for this printer's options; the app starts the read on the next frame.
    pub read_options_for: Option<String>,
}

impl Default for PrintDraft {
    fn default() -> Self {
        PrintDraft {
            printers: Vec::new(),
            units: Default::default(),
            printers_loading: false,
            printer_picked: false,
            printer: None,
            copies: 1,
            collate: true,
            grayscale: false,
            duplex: None,
            which: Which::All,
            range: String::new(),
            selected: Vec::new(),
            subset: Subset::All,
            reverse: false,
            handling: Handling::Size,
            size: SizeMode::Fit,
            custom_scale: 100.0,
            per_sheet: 2,
            order: PageOrder::Horizontal,
            border: false,
            auto_rotate: true,
            booklet_subset: BookletSubset::BothSides,
            binding: Binding::Left,
            poster_scale: 200.0,
            overlap: 18.0,
            cut_marks: true,
            orientation: Orientation::Auto,
            content: Content::DocumentAndMarkups,
            as_image: false,
            dpi: 300.0,
            paper: 0,
            custom_paper: None,
            paper_query: String::new(),
            saved_papers: Vec::new(),
            custom_name: None,
            save_form: None,
            paper_before_label: None,
            sheet: 0,
            current_page: 0,
            driver_options: None,
            driver_choices: Default::default(),
            show_driver_options: false,
            driver_error: None,
            label_flip: false,
            crops: Vec::new(),
            crop_pages: None,
            driver_options_loading: None,
            read_options_for: None,
        }
    }
}

impl PrintDraft {
    /// Choose Page Sizing & Handling. Label loads label stock and remembers the paper it
    /// replaced; leaving Label gives that paper back, unless another one was picked meanwhile.
    pub fn set_handling(&mut self, handling: Handling) {
        let leaving_label = self.handling == Handling::Label && handling != Handling::Label;
        self.handling = handling;
        self.sheet = 0;
        if handling == Handling::Label {
            // A label goes on label stock, not on a paper of the user's own size.
            self.custom_paper = None;
            self.custom_name = None;
            self.save_form = None;
            if self.paper != print::LABEL_PAPER {
                self.paper_before_label = Some(self.paper);
                self.paper = print::LABEL_PAPER;
            }
        } else if leaving_label
            && let Some(paper) = self.paper_before_label.take()
            && self.paper == print::LABEL_PAPER
        {
            self.paper = paper;
        }
    }

    /// The paper the sheets are laid out on (points, portrait as listed or as typed).
    pub fn paper_size(&self) -> (f64, f64) {
        self.custom_paper.unwrap_or(PAPERS[self.paper.min(PAPERS.len() - 1)].1)
    }

    /// Use a paper of the user's own size, held to [`CUSTOM_PAPER_RANGE`]; `None` when a size
    /// is not a number.
    pub fn set_custom_paper(&mut self, size: (f64, f64)) -> Option<(f64, f64)> {
        let clamp = |v: f64| v.is_finite().then(|| v.clamp(*CUSTOM_PAPER_RANGE.start(), *CUSTOM_PAPER_RANGE.end()));
        let size = (clamp(size.0)?, clamp(size.1)?);
        self.custom_paper = Some(size);
        // A changed size is no longer the saved paper it came from.
        self.custom_name = None;
        self.paper_before_label = None;
        self.sheet = 0;
        Some(size)
    }

    /// Use one of the saved papers (its size, shown by its name).
    pub fn use_saved_paper(&mut self, index: usize) {
        let Some(paper) = self.saved_papers.get(index).cloned() else { return };
        if self.handling == Handling::Label {
            self.set_handling(Handling::Size);
        }
        if self.set_custom_paper(paper.size).is_some() {
            self.custom_name = Some(paper.name);
        }
    }

    /// Save the custom size in use as `name` in `category`, replacing a saved paper of that name
    /// (ignoring case). Refused without a custom size, with a blank name, or when
    /// [`SAVED_PAPERS_MAX`] papers are already saved.
    pub fn save_paper(&mut self, name: &str, category: &str) -> Result<(), &'static str> {
        let size = self.custom_paper.ok_or("Tick Custom and type the size first.")?;
        let paper = SavedPaper::new(name, category, size).ok_or("Give the paper size a name.")?;
        let full = self.saved_papers.len() >= SAVED_PAPERS_MAX;
        match self.saved_papers.iter_mut().find(|p| p.name.to_lowercase() == paper.name.to_lowercase()) {
            Some(existing) => *existing = paper.clone(),
            None if full => return Err("Too many saved paper sizes. Delete one first."),
            None => self.saved_papers.push(paper.clone()),
        }
        self.custom_name = Some(paper.name);
        self.save_form = None;
        Ok(())
    }

    /// Delete the saved paper in use; its size stays as a custom size.
    pub fn delete_saved_paper(&mut self) {
        if let Some(name) = self.custom_name.take() {
            self.saved_papers.retain(|p| p.name != name);
        }
    }

    /// The categories a saved paper can go in: the user's own (alphabetically), then
    /// [`PAPER_GROUPS`].
    pub fn paper_categories(&self) -> Vec<String> {
        let mut own: Vec<String> = Vec::new();
        for p in &self.saved_papers {
            if !PAPER_GROUPS.iter().any(|g| g.eq_ignore_ascii_case(&p.category)) && !own.iter().any(|c| c.eq_ignore_ascii_case(&p.category)) {
                own.push(p.category.clone());
            }
        }
        own.sort_by_key(|c| c.to_lowercase());
        own.into_iter().chain(PAPER_GROUPS.iter().map(|g| g.to_string())).collect()
    }

    /// The engine settings for this draft (page count and labels from the document).
    pub fn settings(&self, count: usize, labels: &[String]) -> Result<print::Settings, String> {
        if self.cut_and_stack() && matches!(self.duplex, Some(spool::Duplex::LongEdge | spool::Duplex::ShortEdge)) {
            return Err("Cut and stack needs Two-sided: Off. Print single-sided sheets.".into());
        }
        let range = match self.which {
            Which::All | Which::Selected | Which::Current => None,
            Which::Range => Some(self.range.clone()),
        };
        // Selected and Current are positions, not a typed range: a range would read numbers as
        // page labels first.
        let pages = match self.which {
            Which::Selected => print::select_listed(count, &self.selected, self.subset, self.reverse),
            Which::Current => print::select_listed(count, &[self.current_page], self.subset, self.reverse),
            Which::All | Which::Range => print::select_pages(count, range.as_deref(), labels, self.subset, self.reverse),
        }
        // What the person got wrong about what to print is said in their language: the engine hands
        // the wording over whole and the page numbers apart from it.
        .map_err(|e| e.said(&crate::i18n::words))?;
        let layout = match self.handling {
            Handling::Size => Layout::Size(match self.size {
                SizeMode::Custom(_) => SizeMode::Custom(self.custom_scale),
                m => m,
            }),
            Handling::Multiple => match Layout::multiple(self.per_sheet) {
                Layout::Multiple { cols, rows, per_sheet, .. } => {
                    Layout::Multiple { cols, rows, per_sheet, order: self.order, border: self.border, auto_rotate: self.auto_rotate }
                }
                other => other,
            },
            Handling::Booklet => Layout::Booklet { subset: self.booklet_subset, binding: self.binding },
            Handling::Poster => Layout::Poster { scale: self.poster_scale, overlap: self.overlap, cut_marks: self.cut_marks },
            Handling::Label => Layout::Label { flip: self.label_flip },
        };
        Ok(print::Settings {
            pages,
            paper: self.paper_size(),
            orientation: self.orientation,
            layout,
            content: self.content,
            raster: self.as_image.then_some(print::Raster { dpi: self.dpi, grayscale: self.grayscale }),
            crops: self.crops.clone(),
        })
    }

    fn cut_and_stack(&self) -> bool {
        self.handling == Handling::Multiple && self.order == PageOrder::CutStack
    }

    /// The spooler job for this print: the dialog's copies, two-sided and colour settings, the
    /// driver options changed from the printer's defaults, and the chosen paper.
    pub fn job(&self, title: &str) -> spool::Job {
        spool::Job {
            printer: self.printer.clone(),
            copies: self.copies.max(1),
            collate: self.collate,
            // Cut and stack needs single-sided sheets, whatever the printer's default; a booklet
            // decides two-sided itself (#2707), leaving the Two-sided choice as it was.
            duplex: if self.cut_and_stack() {
                Some(spool::Duplex::Off)
            } else if self.handling == Handling::Booklet {
                Some(print::booklet_duplex(self.booklet_subset))
            } else {
                self.duplex
            },
            scaling: spool::Scaling::Exact,
            grayscale: self.grayscale,
            title: title.to_string(),
            // Only for the printer the options were read from.
            options: match (&self.printer, &self.driver_options) {
                (Some(p), Some((read_for, _))) if p == read_for => self.driver_choices.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                _ => Vec::new(),
            }
            .into_iter()
            // The sheet the PDF was laid out on, not the queue's default paper. A size of the
            // user's own goes to CUPS as its custom size (points); Windows matches the driver's
            // paper to the sheet size itself.
            .chain(match self.custom_paper {
                Some((w, h)) => Some(("media".to_string(), format!("Custom.{w:.2}x{h:.2}"))),
                None => print::media_option(self.paper),
            })
            .collect(),
        }
    }
}

/// Lists the printers off the UI thread (the desktop app sets [`PdfCraftApp::printer_source`]):
/// on Windows each driver's capabilities are read for its two-sided support, and CUPS' driverless discovery can wait on
/// the network. Without a source the dialog reads them as it opens, so tests stay deterministic.
pub type PrinterSource = std::sync::Arc<dyn Fn() -> Vec<spool::Printer> + Send + Sync>;

/// Where a printer's driver options come from: the spooler's (`spool::printer_options_in`, in the
/// UI language) unless a test stands in. Called off the GUI thread.
pub type OptionsSource = std::sync::Arc<dyn Fn(&str) -> Vec<spool::PrinterOption> + Send + Sync>;

/// The system spooler's printers, as the desktop app's [`PrinterSource`].
pub fn system_printers() -> PrinterSource {
    std::sync::Arc::new(spool::printers)
}

/// The Print dialog's work in the background: the printer list being read, and the jobs being
/// spooled, oldest first.
#[derive(Default)]
pub(crate) struct PrintTasks {
    #[cfg(not(target_arch = "wasm32"))]
    printers: Option<std::sync::mpsc::Receiver<Vec<spool::Printer>>>,
    #[cfg(not(target_arch = "wasm32"))]
    jobs: Vec<PrintJob>,
    /// The progress card is showing a print job (it is shared with other long jobs).
    #[cfg(not(target_arch = "wasm32"))]
    showing_progress: bool,
    /// The driver options being read, per printer (`read_options_in_background`).
    #[cfg(not(target_arch = "wasm32"))]
    options: Vec<(String, std::sync::mpsc::Receiver<Vec<spool::PrinterOption>>)>,
    /// The browser print being drawn, a sheet a frame (`web_print`).
    #[cfg(target_arch = "wasm32")]
    web_job: Option<crate::web_print::WebPrintJob>,
    /// What went wrong after the browser print page was handed over, with the print-ready PDF
    /// to download instead.
    #[cfg(target_arch = "wasm32")]
    web_failed: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// The last browser print's PDF and name, for that fallback.
    #[cfg(target_arch = "wasm32")]
    web_last: Option<(String, std::sync::Arc<Vec<u8>>)>,
}

/// The printers to offer when no [`PrinterSource`] is set: the spooler's, or in a browser its
/// print dialog.
fn listed_printers() -> Vec<spool::Printer> {
    #[cfg(target_arch = "wasm32")]
    {
        vec![spool::Printer { name: crate::web_print::BROWSER_PRINTER.to_string(), default: true, two_sided: None, description: String::new() }]
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        spool::printers()
    }
}

/// The first list is still being read and the user hasn't chosen yet (#160): no printer is
/// selected only because none is known yet, so the dialog says it is looking rather than
/// offering Save as PDF, and its button waits. Printing then would have saved a PDF.
fn still_looking(d: &PrintDraft) -> bool {
    d.printers_loading && d.printer.is_none() && !d.printer_picked
}

/// A printer's name as the dialog shows it.
fn printer_label(name: &str) -> String {
    if name == crate::web_print::BROWSER_PRINTER { crate::web_print::browser_printer_label().to_string() } else { name.to_string() }
}

/// A job being spooled: its printer, the sheets sent so far of how many, and its worker's news.
#[cfg(not(target_arch = "wasm32"))]
struct PrintJob {
    printer: String,
    sheets: Option<(usize, usize)>,
    news: std::sync::mpsc::Receiver<JobNews>,
}

#[cfg(not(target_arch = "wasm32"))]
enum JobNews {
    /// `(sent, sheets)`: the spooler has this many of the job's sheets.
    Sent(usize, usize),
    /// The spooler's answer.
    Done(Result<String, String>),
}

/// The printer the dialog preselects: the system default, else the first.
fn default_printer(printers: &[spool::Printer]) -> Option<String> {
    printers.iter().find(|p| p.default).or(printers.first()).map(|p| p.name.clone())
}

/// Whether `printer` can still be chosen from `printers`: Save as PDF (`None`) always can.
fn offered(printer: Option<&str>, printers: &[spool::Printer]) -> bool {
    printer.is_none_or(|name| printers.iter().any(|p| p.name == name))
}

/// Asks for a frame when a worker thread ends, also when it panics: [`PdfCraftApp::poll_print`]
/// then sees the answer, or the disconnected channel of a worker that died, without waiting
/// for some other event to draw (#2108 review). Declared before the worker's sender, so the
/// sender is dropped first and the frame finds the channel closed.
#[cfg(not(target_arch = "wasm32"))]
struct WakeOnExit(Option<egui::Context>);

#[cfg(not(target_arch = "wasm32"))]
impl Drop for WakeOnExit {
    fn drop(&mut self) {
        if let Some(ctx) = &self.0 {
            ctx.request_repaint();
        }
    }
}

impl PdfCraftApp {
    pub fn open_print(&mut self) {
        let Some((i, _)) = self.active_ids() else { return };
        let (printers, printers_loading) = match self.printer_source.clone() {
            // The list from the last time the dialog opened, until the fresh one arrives.
            #[cfg(not(target_arch = "wasm32"))]
            Some(source) => {
                self.list_printers_in_background(source);
                (self.print_draft.printers.clone(), true)
            }
            _ => (listed_printers(), false),
        };
        let default = default_printer(&printers);
        let current = self.views[i].current;
        let selected: Vec<usize> = self.views[i].selected.iter().copied().collect();
        let keep = std::mem::take(&mut self.print_draft);
        // The printer picked last time, Save as PDF included, while it is still listed (#3537).
        let picked = keep.printer_picked && offered(keep.printer.as_deref(), &printers);
        let printer = if picked { keep.printer.clone() } else { default };
        // Picked pages are what Print is for (Acrobat's "Selected pages"); without any, a choice
        // left over from another document falls back to the whole document.
        let which = match (selected.is_empty(), keep.which) {
            (false, _) => Which::Selected,
            (true, Which::Selected) => Which::All,
            (true, other) => other,
        };
        self.print_draft = PrintDraft {
            crops: Vec::new(),
            crop_pages: None,
            printers,
            printers_loading,
            printer_picked: picked,
            units: self.units,
            printer,
            current_page: current,
            sheet: 0,
            selected,
            which,
            ..keep
        };
        self.dialog = Some(crate::Dialog::Print);
    }

    /// Label printing: find each printed page's printed area (a quick low-resolution render, as
    /// Fit Visible does) and which way its text runs. Only the pages being printed are read, at
    /// most [`LABEL_CROP_PAGES`] of them (the rest print whole); read again when they change.
    pub(crate) fn read_label_crops(&mut self, id: pdfcraft_engine::DocId) {
        let Some(doc) = self.session.get(id) else { return };
        let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
        let mut pages = self.print_draft.settings(doc.info.pages.len(), &labels).map(|s| s.pages).unwrap_or_default();
        pages.sort_unstable();
        pages.dedup();
        pages.truncate(LABEL_CROP_PAGES);
        if self.print_draft.crop_pages.as_ref() == Some(&pages) {
            return;
        }
        let up = self.session.print_text_up(id, &pages);
        let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        self.print_draft.crops = doc
            .info
            .pages
            .iter()
            .enumerate()
            .map(|(page, p)| {
                let (w, h) = (f64::from(p.width), f64::from(p.height));
                if pages.binary_search(&page).is_err() || w <= 0.0 || h <= 0.0 {
                    return None;
                }
                let out = r.render(pdfcraft_render::RenderRequest {
                    page,
                    kind: pdfcraft_render::RequestKind::Pixels,
                    tile: None,
                    scale: (1000.0 / w.max(h) as f32).clamp(0.1, 4.0),
                    tag: 0,
                    preview: false,
                });
                let [u0, v0, u1, v1] = crate::zoom_snap::ink_bounds(out.width as usize, out.height as usize, &out.rgba)?;
                // Image rows run down from the top; display space runs up. Half a millimetre of slack.
                let pad = 1.5;
                let rect = [f64::from(u0) * w - pad, (1.0 - f64::from(v1)) * h - pad, f64::from(u1) * w + pad, (1.0 - f64::from(v0)) * h + pad];
                Some(print::Crop { rect, text_up: up.get(page).copied().unwrap_or(false) })
            })
            .collect();
        self.print_draft.crop_pages = Some(pages);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn list_printers_in_background(&mut self, source: PrinterSource) {
        // A list already being read is as fresh as a new one would be.
        if self.print_tasks.printers.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = WakeOnExit(self.ctx.clone());
        std::thread::spawn(move || {
            let _wake = wake;
            let tx = tx;
            // The receiver may be gone (the app quit): nothing to report to then.
            let _ = tx.send(source());
        });
        self.print_tasks.printers = Some(rx);
    }

    /// Read `printer`'s driver options off the GUI thread (#1818); `poll_print` picks them up.
    /// A read already under way for the same printer is left to finish.
    #[cfg(not(target_arch = "wasm32"))]
    fn read_options_in_background(&mut self, printer: String) {
        if self.print_tasks.options.iter().any(|(p, _)| *p == printer) {
            return;
        }
        let source: OptionsSource = self.options_source.clone().unwrap_or_else(|| {
            // The driver's own names in the UI language, as it is when Properties… is clicked.
            let lang = crate::i18n::current().code();
            std::sync::Arc::new(move |p: &str| spool::printer_options_in(p, lang))
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = WakeOnExit(self.ctx.clone());
        let name = printer.clone();
        let spawned = std::thread::Builder::new().name("printer options".into()).spawn(move || {
            let _wake = wake;
            let tx = tx;
            // The receiver may be gone (the app quit): nothing to report to then.
            let _ = tx.send(source(&name));
        });
        match spawned {
            Ok(_) => self.print_tasks.options.push((printer, rx)),
            // No thread to read them on: the printer offers nothing this time.
            Err(_) => self.receive_options(printer, Vec::new()),
        }
    }

    /// The options read for `printer` arrived, or the read died. A read that a later click
    /// superseded (another printer's is awaited) is dropped.
    #[cfg(not(target_arch = "wasm32"))]
    fn receive_options(&mut self, printer: String, options: Vec<spool::PrinterOption>) {
        let d = &mut self.print_draft;
        if d.driver_options_loading.as_deref() == Some(printer.as_str()) {
            d.driver_options_loading = None;
            d.driver_options = Some((printer, options));
        }
    }

    /// Pick up a printer list, driver options or a spooled job that finished (each frame).
    pub(crate) fn poll_print(&mut self) {
        #[cfg(target_arch = "wasm32")]
        self.poll_web_print();
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(printer) = self.print_draft.read_options_for.take() {
                self.read_options_in_background(printer);
            }
            let mut arrived = Vec::new();
            self.print_tasks.options.retain(|(printer, rx)| match rx.try_recv() {
                Ok(options) => {
                    arrived.push((printer.clone(), options));
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
                // A read that died offers nothing; the panel says so rather than wait.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    arrived.push((printer.clone(), Vec::new()));
                    false
                }
            });
            for (printer, options) in arrived {
                self.receive_options(printer, options);
            }
            if let Some(rx) = &self.print_tasks.printers {
                let printers = match rx.try_recv() {
                    Ok(p) => Some(p),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    // A source that died lists nothing new; the dialog keeps what it has.
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(self.print_draft.printers.clone()),
                };
                if let Some(printers) = printers {
                    self.print_tasks.printers = None;
                    let d = &mut self.print_draft;
                    // A picked printer that is no longer listed falls back to the default too.
                    if !d.printer_picked || !offered(d.printer.as_deref(), &printers) {
                        d.printer_picked = false;
                        d.printer = default_printer(&printers);
                    }
                    d.printers = printers;
                    d.printers_loading = false;
                }
            }
            let mut finished = Vec::new();
            self.print_tasks.jobs.retain_mut(|job| {
                loop {
                    match job.news.try_recv() {
                        Ok(JobNews::Sent(sent, sheets)) => job.sheets = Some((sent, sheets)),
                        Ok(JobNews::Done(r)) => {
                            finished.push((job.printer.clone(), r));
                            break false;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break true,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            finished.push((job.printer.clone(), Err("the print job stopped unexpectedly".into())));
                            break false;
                        }
                    }
                }
            });
            self.show_print_progress();
            for (printer, result) in finished {
                self.report_print_job(&printer, result);
            }
        }
    }

    /// The oldest job being spooled on the progress card, counting its sheets as they reach the
    /// spooler, while no other long job holds the card.
    #[cfg(not(target_arch = "wasm32"))]
    fn show_print_progress(&mut self) {
        let Some(job) = self.print_tasks.jobs.first() else {
            if std::mem::take(&mut self.print_tasks.showing_progress) {
                self.progress_notice = None;
            }
            return;
        };
        if self.progress_notice.is_some() && !self.print_tasks.showing_progress {
            return;
        }
        let printer = job.printer.as_str();
        let (label, fraction) = match job.sheets {
            Some((sent, sheets)) if sheets > 0 => {
                // The sheet on its way: one past those sent, until the last is.
                let sheet = sent.saturating_add(1).min(sheets).to_string();
                let label = crate::i18n::fmt(
                    tl!("Sending sheet {s} of {n} to {printer}…"),
                    &[("s", &sheet), ("n", &sheets.to_string()), ("printer", printer)],
                );
                (label, sent as f32 / sheets as f32)
            }
            _ => (crate::i18n::fmt(tl!("Sending to {printer}…"), &[("printer", printer)]), 0.0),
        };
        self.progress_notice = Some(crate::widgets::ProgressNotice { label, fraction, cancellable: false });
        self.print_tasks.showing_progress = true;
        // The bar's sweep keeps moving between sheets.
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn report_print_job(&mut self, printer: &str, result: Result<String, String>) {
        match result {
            Ok(msg) if msg.is_empty() => self.notify(crate::i18n::fmt(tl!("Sent to {printer}"), &[("printer", printer)])),
            Ok(msg) => self.notify(crate::i18n::fmt(tl!("Sent to {printer}: {msg}"), &[("printer", printer), ("msg", &msg)])),
            Err(e) => self.notify_error(e),
        }
    }

    /// Print (or save) with the dialog's settings. Returns `true` on success. Save as PDF without
    /// a preset path returns `true` once the save picker is showing; the file is written on a
    /// later frame, when the user has chosen where.
    pub fn print_now(&mut self) -> bool {
        // What's typed in a form field is part of what's printed (#166).
        if !self.commit_form_typing() {
            return false;
        }
        let Some((_, id)) = self.active_ids() else { return false };
        let Some(doc) = self.session.get(id) else { return false };
        let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
        let settings = match self.print_draft.settings(doc.info.pages.len(), &labels) {
            Ok(s) => s,
            Err(e) => {
                self.notify_tr(&e);
                return false;
            }
        };
        let name = doc.name.clone();
        let bytes = match self.session.print_pdf(id, &settings) {
            Ok(b) => b,
            Err(e) => {
                self.notify_error(e);
                return false;
            }
        };
        match self.print_draft.printer.clone() {
            #[cfg(target_arch = "wasm32")]
            Some(printer) if printer == crate::web_print::BROWSER_PRINTER => {
                let sizes: Vec<(f64, f64)> = doc.info.pages.iter().map(|p| (f64::from(p.width), f64::from(p.height))).collect();
                match print::layout(&sizes, &settings) {
                    Ok(sheets) => self.print_in_browser(name, bytes, sheets.iter().map(|s| s.size).collect()),
                    Err(e) => {
                        self.notify_error(e);
                        false
                    }
                }
            }
            Some(printer) => self.send_to_printer(printer, bytes, self.print_draft.job(&name)),
            None => self.save_print_pdf(&name, bytes),
        }
    }

    /// Spool a job off the UI thread: on Windows every sheet is rendered at the printer's
    /// resolution first, which can take minutes for a long document. The dialog closes at once,
    /// the progress card counts the sheets as they reach the spooler, and the spooler's answer
    /// arrives as a notice.
    #[cfg(not(target_arch = "wasm32"))]
    fn send_to_printer(&mut self, printer: String, bytes: Vec<u8>, job: spool::Job) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let repaint = || {
                if let Some(ctx) = &ctx {
                    ctx.request_repaint();
                }
            };
            // The receiver may be gone (the app quit): nothing to report to then.
            let result = spool::submit_with_progress(&bytes, &job, &mut |sent, sheets| {
                let _ = tx.send(JobNews::Sent(sent, sheets));
                repaint();
            });
            let _ = tx.send(JobNews::Done(result.map_err(|e| e.to_string())));
            repaint();
        });
        self.print_tasks.jobs.push(PrintJob { printer, sheets: None, news: rx });
        self.show_print_progress();
        true
    }

    /// A browser has no spooler: `submit` answers at once.
    #[cfg(target_arch = "wasm32")]
    fn send_to_printer(&mut self, printer: String, bytes: Vec<u8>, job: spool::Job) -> bool {
        let result = spool::submit(&bytes, &job).map_err(|e| e.to_string());
        let ok = result.is_ok();
        self.report_print_job(&printer, result);
        ok
    }

    /// Print through the browser's print dialog: start drawing `sheets` (the pages of the
    /// print-ready `bytes`), a sheet a frame in [`PdfCraftApp::poll_print`]. A print still being
    /// prepared is replaced. The dialog closes at once.
    #[cfg(target_arch = "wasm32")]
    fn print_in_browser(&mut self, name: String, bytes: Vec<u8>, sheets: Vec<(f64, f64)>) -> bool {
        match crate::web_print::WebPrintJob::new(bytes, sheets, name) {
            Ok(job) => {
                let n = job.total().to_string();
                self.print_tasks.web_job = Some(job);
                self.notify_fmt("Preparing {n} sheets for printing…", &[("n", &n)]);
                if let Some(ctx) = &self.ctx {
                    ctx.request_repaint();
                }
                true
            }
            Err(e) => {
                self.notify_error(e);
                false
            }
        }
    }

    /// Draw the next sheet of the browser print, and hand the finished page to the browser.
    #[cfg(target_arch = "wasm32")]
    fn poll_web_print(&mut self) {
        let failed: Vec<String> = self.print_tasks.web_failed.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        if let Some(e) = failed.into_iter().next() {
            self.web_print_failed(&e);
        }
        let Some(job) = self.print_tasks.web_job.as_mut() else { return };
        if !job.finished() {
            if let Err(e) = job.step(&mut crate::web_print::blob_url) {
                self.print_tasks.web_job = None;
                self.notify_error(e);
                return;
            }
            let (done, total) = (job.done().to_string(), job.total().to_string());
            self.notify_fmt("Preparing sheet {done} of {total} for printing…", &[("done", &done), ("total", &total)]);
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint();
            }
            return;
        }
        let Some(mut job) = self.print_tasks.web_job.take() else { return };
        let html = job.html();
        let urls = job.take_urls();
        self.print_tasks.web_last = Some((job.name.clone(), job.pdf.clone()));
        match crate::web_print::print_in_iframe(&html, urls, self.print_tasks.web_failed.clone(), self.ctx.clone()) {
            Ok(()) => self.notify_tr("Opening the browser's print dialog…"),
            Err(e) => self.web_print_failed(&e),
        }
    }

    /// The browser wouldn't print (a sandboxed frame, a picture it couldn't decode): offer the
    /// print-ready PDF as a download instead, so the work isn't lost.
    #[cfg(target_arch = "wasm32")]
    fn web_print_failed(&mut self, error: &str) {
        let Some((name, pdf)) = self.print_tasks.web_last.take() else {
            self.notify_error(error);
            return;
        };
        let stem = name.trim_end_matches(".pdf").trim_end_matches(".PDF");
        let file = format!("{stem} (print).pdf");
        match crate::editing::download(&file, &pdf) {
            Ok(()) => self.notify_fmt("Couldn't print from the browser ({e}); downloaded {name} to print instead", &[("e", error), ("name", &file)]),
            Err(_) => self.notify_error(error),
        }
    }

    /// Print ▸ Save as PDF on the desktop: write to `save_override` (tests and automation), or
    /// ask where and write on a later frame. Returns `true` once written, or once the save picker
    /// is showing.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_print_pdf(&mut self, name: &str, bytes: Vec<u8>) -> bool {
        let write = move |app: &mut Self, path: std::path::PathBuf| match crate::editing::write_atomically(&path.to_string_lossy(), &bytes) {
            Ok(()) => {
                app.notify_fmt("Saved the print-ready PDF to {path}", &[("path", &path.display().to_string())]);
                true
            }
            Err(e) => {
                app.notify_fmt("Could not save: {e}", &[("e", &e.to_string())]);
                false
            }
        };
        match self.save_override.clone() {
            Some(p) => write(self, p.into()),
            None => {
                let stem = name.trim_end_matches(".pdf").trim_end_matches(".PDF");
                let dialog = rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(format!("{stem} (print).pdf"));
                self.ask_one(crate::pickers::Ask::Save(dialog), None, move |app, path| {
                    write(app, path);
                })
            }
        }
    }

    /// Print ▸ Save as PDF in a browser: download it, as Save does (#170). There is no folder to
    /// choose and no file system to write to, so `save_override` doesn't apply.
    #[cfg(target_arch = "wasm32")]
    fn save_print_pdf(&mut self, name: &str, bytes: Vec<u8>) -> bool {
        let stem = name.trim_end_matches(".pdf").trim_end_matches(".PDF");
        let file = format!("{stem} (print).pdf");
        match crate::editing::download(&file, &bytes) {
            Ok(()) => {
                self.notify_fmt("Downloaded {name}", &[("name", &file)]);
                true
            }
            Err(e) => {
                self.notify_fmt("Couldn't download {name}: {e}", &[("name", &file), ("e", &e)]);
                false
            }
        }
    }
}

/// The longest side, in pixels, of one page's print-preview raster (also kept within the GPU's
/// `max_texture_side`): an Actual-size print of a large page would otherwise ask for a raster
/// bigger than a texture can be.
pub(crate) const PREVIEW_MAX_SIDE: f32 = 4096.0;
/// The most bytes of print-preview rasters kept at once. The current sheet comes first, then the
/// next and the previous one; a neighbour that doesn't fit isn't rendered ahead.
pub(crate) const PREVIEW_BYTES: f64 = 128.0 * 1024.0 * 1024.0;

/// Pages on the current sheet and its neighbours, each with the device pixels per point that
/// fill the preview pane (so the picture is not a stretched thumbnail), nearest sheet first and
/// within [`PREVIEW_MAX_SIDE`] (and `max_side`, the GPU's texture limit) and [`PREVIEW_BYTES`].
pub(crate) fn preview_rasters(d: &PrintDraft, sizes: &[(f64, f64)], labels: &[String], ppp: f32, max_side: f32) -> Vec<(usize, f32)> {
    let Ok(settings) = d.settings(sizes.len(), labels) else { return Vec::new() };
    let Ok(sheets) = print::layout(sizes, &settings) else { return Vec::new() };
    if sheets.is_empty() {
        return Vec::new();
    }
    let ppp = if ppp.is_finite() { ppp.max(1.0) } else { 1.0 };
    let side = if max_side.is_finite() && max_side >= 1.0 { max_side.min(PREVIEW_MAX_SIDE) } else { PREVIEW_MAX_SIDE };
    let i = d.sheet.min(sheets.len() - 1);
    // Nearest first: the sheet on screen, then the next one, then the previous one.
    let order = [Some(i), i.checked_add(1).filter(|n| *n < sheets.len()), i.checked_sub(1)];
    let mut out: Vec<(usize, f32, f64)> = Vec::new();
    let mut bytes = 0.0f64;
    for sheet in order.into_iter().flatten().filter_map(|s| sheets.get(s)) {
        let (sw, sh) = (sheet.size.0 as f32, sheet.size.1 as f32);
        if !(sw.is_finite() && sh.is_finite()) || sw < 1.0 || sh < 1.0 {
            continue;
        }
        let k = (PREVIEW_BOX.0 / sw).min(PREVIEW_BOX.1 / sh);
        for pl in &sheet.placed {
            let [a, b, _, _, _, _] = pl.matrix.0;
            let placed = (a.hypot(b) as f32) * k * ppp;
            let Some(&(pw, ph)) = sizes.get(pl.page) else { continue };
            let long_pt = pw.max(ph) as f32;
            if !placed.is_finite() || placed < 0.05 || !long_pt.is_finite() || long_pt < 1.0 {
                continue;
            }
            let scale = placed.min(64.0).min(side / long_pt);
            let cost = (pw * f64::from(scale)).ceil() * (ph * f64::from(scale)).ceil() * 4.0;
            if let Some(slot) = out.iter_mut().find(|(page, _, _)| *page == pl.page) {
                if scale > slot.1 && bytes - slot.2 + cost <= PREVIEW_BYTES {
                    bytes += cost - slot.2;
                    (slot.1, slot.2) = (scale, cost);
                }
            } else if out.len() < 48 && (out.is_empty() || bytes + cost <= PREVIEW_BYTES) {
                bytes += cost;
                out.push((pl.page, scale, cost));
            }
        }
    }
    out.into_iter().map(|(page, scale, _)| (page, scale)).collect()
}

/// Properties… on CUPS: the driver's options for this print, grouped as the driver groups them.
/// Only choices away from the printer's defaults are kept, and sent with the job.
fn driver_options_panel(ui: &mut egui::Ui, d: &mut PrintDraft, t: &Tokens) {
    let PrintDraft { driver_options: Some((_, options)), driver_choices: choices, show_driver_options: show, .. } = d else { return };
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tl!("Printer properties")).font(theme::semibold(13.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tl!("Done")).clicked() {
                    *show = false;
                }
                if ui.add_enabled(!choices.is_empty(), egui::Button::new(tl!("Reset to printer defaults"))).clicked() {
                    choices.clear();
                }
            });
        });
        if options.is_empty() {
            ui.label(egui::RichText::new(tl!("This printer offers no settings of its own.")).small().color(t.text_muted));
            return;
        }
        egui::ScrollArea::vertical().id_salt("driver-options").max_height(240.0).show(ui, |ui| {
            for (n, chunk) in options.chunk_by(|a, b| a.group == b.group).enumerate() {
                let group = chunk.first().map_or("", |o| o.group.as_str());
                if !group.is_empty() {
                    ui.add_space(4.0);
                    // Option and group names are the driver's own text.
                    ui.label(egui::RichText::new(group).small().color(t.text_muted));
                }
                egui::Grid::new(("driver-group", n)).num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                    for o in chunk {
                        let current = choices.get(&o.key).cloned().unwrap_or_else(|| o.default.clone());
                        let label_of = |c: &str| o.choices.iter().find(|(k, _)| k == c).map_or_else(|| c.to_string(), |(_, l)| l.clone());
                        let mut pick = current.clone();
                        let option_label = ui.label(&o.label);
                        egui::ComboBox::from_id_salt(("driver-option", &o.key))
                            .selected_text(label_of(&current))
                            .width(200.0)
                            .show_ui(ui, |ui| {
                                for (k, l) in &o.choices {
                                    ui.selectable_value(&mut pick, k.clone(), l.as_str());
                                }
                            })
                            .response
                            .labelled_by(option_label.id);
                        if pick != current {
                            if pick == o.default {
                                choices.remove(&o.key);
                            } else {
                                choices.insert(o.key.clone(), pick);
                            }
                        }
                        ui.end_row();
                    }
                });
            }
        });
    });
}

/// Draw a combo box whose accessible name does not depend on the selected value.
fn combo<T: PartialEq + Copy>(ui: &mut egui::Ui, id: &str, name: &str, value: &mut T, choices: &[(T, &str)], width: f32) {
    let shown = choices.iter().find(|c| c.0 == *value).map_or("", |c| c.1);
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(width)
        .show_ui(ui, |ui| {
            for (v, label) in choices {
                ui.selectable_value(value, *v, *label);
            }
        })
        .response
        .accessible_name(name);
}

/// Draw the dialog. `thumb` gives a page's thumbnail texture when there is one. Returns
/// (print, cancel).
pub(crate) fn body(
    ui: &mut egui::Ui,
    d: &mut PrintDraft,
    t: &Tokens,
    sizes: &[(f64, f64)],
    labels: &[String],
    thumb: &mut dyn FnMut(usize) -> Option<egui::TextureId>,
) -> (bool, bool) {
    ui.set_width(820.0);
    ui.label(egui::RichText::new(tl!("Print")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let settings = d.settings(sizes.len(), labels);
    let sheets = settings.as_ref().ok().and_then(|s| print::layout(sizes, s).ok()).unwrap_or_default();
    d.sheet = d.sheet.min(sheets.len().saturating_sub(1));
    ui.horizontal_top(|ui| {
        // Settings.
        ui.vertical(|ui| {
            ui.set_width(470.0);
            egui::Grid::new("print-top").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                let printer_caption = ui.label(tl!("Printer:"));
                let before = d.printer.clone();
                // The browser's print dialog sets copies, two-sided printing and colour itself.
                let browser = d.printer.as_deref() == Some(crate::web_print::BROWSER_PRINTER);
                ui.horizontal(|ui| {
                    let shown = match d.printer.as_deref() {
                        Some(p) => printer_label(p),
                        None if still_looking(d) => tl!("Looking for printers…").to_string(),
                        None => tl!("Save as PDF").to_string(),
                    };
                    egui::ComboBox::from_id_salt("printer").selected_text(shown).width(260.0).show_ui(ui, |ui| {
                        if d.printers_loading {
                            ui.add_enabled(false, egui::Label::new(tl!("Looking for printers…")));
                        }
                        for p in &d.printers {
                            let name = printer_label(&p.name);
                            let label = if p.default && p.name != crate::web_print::BROWSER_PRINTER {
                                crate::i18n::fmt(tl!("{name} (default)"), &[("name", &name)])
                            } else {
                                name
                            };
                            ui.selectable_value(&mut d.printer, Some(p.name.clone()), label);
                        }
                        // Chosen even when it was already the selection, so the list arriving
                        // later doesn't replace it with the default printer.
                        if ui.selectable_value(&mut d.printer, None, tl!("Save as PDF")).clicked() {
                            d.printer_picked = true;
                        }
                    })
                    .response
                    .labelled_by(printer_caption.id);
                    let tip = if spool::HAS_PRINTER_PREFERENCES {
                        tl!("Opens the printer driver's preferences. They are saved for every print from this computer; the copies, two-sided, grayscale and paper chosen here still apply.")
                    } else {
                        tl!("The printer driver's own settings for this print: paper tray, paper type, finishing and more.")
                    };
                    let properties = ui.add_enabled(d.printer.is_some() && !browser, egui::Button::new(tl!("Properties…"))).on_hover_text(tip);
                    if properties.clicked()
                        && let Some(p) = d.printer.clone()
                    {
                        if spool::HAS_PRINTER_PREFERENCES {
                            d.driver_error = spool::open_printer_preferences(&p).err().map(|e| e.to_string());
                        } else {
                            if d.driver_options.as_ref().is_none_or(|(read_for, _)| *read_for != p) && d.driver_options_loading.as_deref() != Some(p.as_str()) {
                                // Read off the GUI thread (#1818): `poll_print` starts it next frame.
                                d.driver_options = None;
                                d.driver_choices.clear();
                                d.driver_options_loading = Some(p.clone());
                                d.read_options_for = Some(p);
                            }
                            d.show_driver_options = !d.show_driver_options;
                        }
                    }
                });
                if d.printer != before {
                    d.printer_picked = true;
                    d.show_driver_options = false;
                    d.driver_error = None;
                }
                ui.end_row();
                // The spooler answered with no printer at all (#160): say so, rather than
                // leaving Save as PDF looking like the only thing PdfCraft can do.
                if !d.printers_loading && d.printers.is_empty() {
                    ui.label("");
                    ui.label(
                        egui::RichText::new(tl!("No printers were found. Check that the printer is installed and the print spooler is running, then open Print again."))
                            .small()
                            .color(t.text_muted),
                    );
                    ui.end_row();
                }
                let copies_label = ui.label(tl!("Copies:"));
                ui.add_enabled_ui(!browser, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut d.copies).range(1..=999)).labelled_by(copies_label.id);
                        ui.add(widgets::checkbox(&mut d.collate, tl!("Collate")));
                        ui.add(widgets::checkbox(&mut d.grayscale, tl!("Print in grayscale")));
                    });
                });
                ui.end_row();
                // A booklet decides two-sided itself (#2707), so the choice is hidden meanwhile, as
                // in Acrobat's Print dialog; it is kept for the other layouts.
                if d.handling != Handling::Booklet {
                    ui.label(tl!("Two-sided:"));
                    ui.add_enabled_ui(!browser, |ui| {
                        combo(
                            ui,
                            "duplex",
                            tl!("Two-sided:"),
                            &mut d.duplex,
                            &[
                                (None, tl!("Printer default")),
                                (Some(spool::Duplex::Off), tl!("Off")),
                                (Some(spool::Duplex::LongEdge), tl!("Flip on long edge")),
                                (Some(spool::Duplex::ShortEdge), tl!("Flip on short edge")),
                            ],
                            160.0,
                        );
                    });
                    ui.end_row();
                }
                let paper_label = ui.label(tl!("Paper:"));
                ui.horizontal(|ui| {
                    // A saved paper shows its own name, as typed (not translated).
                    let paper_name: String = match (&d.custom_name, d.custom_paper) {
                        (Some(name), Some(_)) => name.clone(),
                        (None, Some(_)) => tl!("Custom size").to_string(),
                        _ => tl!(PAPERS.get(d.paper).map_or("US Letter", |p| p.0)).to_string(),
                    };
                    let before = d.paper;
                    let mut picked = None;
                    let combo_id = ui.make_persistent_id("paper");
                    // What is typed while the list is open narrows it (shown on top), then the
                    // papers in sections; Enter takes the first match.
                    egui::ComboBox::from_id_salt("paper")
                        .selected_text(paper_name)
                        .width(260.0)
                        .height(460.0)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .show_ui(ui, |ui| {
                            // The keys are read here, not by a text field: a focused field in the
                            // list turns Windows' IME on, which closes the list as it opens. The
                            // list button gives up the keyboard, so Space types instead of closing it.
                            ui.memory_mut(|m| m.surrender_focus(combo_id));
                            let enter = ui.input_mut(|i| {
                                for event in &i.events {
                                    match event {
                                        egui::Event::Text(text) => {
                                            for c in text.chars().filter(|c| !c.is_control()) {
                                                if d.paper_query.chars().count() < PAPER_QUERY_MAX {
                                                    d.paper_query.push(c);
                                                }
                                            }
                                        }
                                        egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. } => {
                                            d.paper_query.pop();
                                        }
                                        _ => {}
                                    }
                                }
                                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                            });
                            egui::Frame::new()
                                .fill(t.field)
                                .stroke(Stroke::new(1.0, t.border))
                                .corner_radius(t.radius)
                                .inner_margin(egui::Margin::symmetric(8, 4))
                                .show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    if d.paper_query.is_empty() {
                                        ui.label(egui::RichText::new(tl!("Type to search paper sizes…")).color(t.text_faint));
                                    } else {
                                        ui.label(format!("{}|", d.paper_query));
                                    }
                                });
                            let sections = paper_sections(&d.paper_query, &d.saved_papers);
                            egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                                if sections.is_empty() {
                                    ui.label(egui::RichText::new(tl!("No paper size matches. Tick Custom to type its size.")).small().color(t.text_muted));
                                }
                                for (heading, items) in &sections {
                                    ui.add_space(4.0);
                                    let heading = match heading {
                                        Heading::Group(g) => tl!(PAPER_GROUPS.get(*g).copied().unwrap_or("Other")).to_string(),
                                        Heading::Category(c) => c.clone(),
                                    };
                                    ui.label(egui::RichText::new(heading).small().strong().color(t.text_muted));
                                    for &paper in items {
                                        let (selected, name) = match paper {
                                            PaperRef::Listed(i) => {
                                                let Some((name, _)) = PAPERS.get(i) else { continue };
                                                (d.custom_paper.is_none() && d.paper == i, tl!(name).to_string())
                                            }
                                            PaperRef::Saved(i) => {
                                                let Some(saved) = d.saved_papers.get(i) else { continue };
                                                let size = d.units.size(saved.size.0, saved.size.1);
                                                (d.custom_name.as_deref() == Some(saved.name.as_str()), format!("{} ({size})", saved.name))
                                            }
                                        };
                                        if ui.selectable_label(selected, name).clicked() {
                                            picked = Some(paper);
                                        }
                                    }
                                }
                            });
                            if enter && picked.is_none() {
                                picked = sections.first().and_then(|(_, items)| items.first().copied());
                            }
                            if picked.is_some() {
                                ui.close();
                            }
                        })
                        .response
                        .labelled_by(paper_label.id);
                    match picked {
                        Some(PaperRef::Listed(i)) => {
                            // A listed paper replaces a size of the user's own.
                            d.paper = i;
                            d.custom_paper = None;
                            d.custom_name = None;
                            d.save_form = None;
                            d.paper_query.clear();
                            d.sheet = 0;
                        }
                        Some(PaperRef::Saved(i)) => {
                            d.use_saved_paper(i);
                            d.save_form = None;
                            d.paper_query.clear();
                        }
                        None => {}
                    }
                    // Label stock gets the label layout (cut, turned upright, fitted).
                    if d.paper != before && d.paper == print::LABEL_PAPER {
                        d.paper_before_label = Some(before);
                        d.handling = Handling::Label;
                        d.sheet = 0;
                    }
                    // A paper of the user's own size, next to the list rather than in it.
                    let mut custom = d.custom_paper.is_some();
                    if ui
                        .add(widgets::checkbox(&mut custom, tl!("Custom")))
                        .on_hover_text(tl!("Lay the pages out on a paper of your own size. The printer's driver must have a paper of that size."))
                        .changed()
                    {
                        if custom {
                            if d.handling == Handling::Label {
                                d.set_handling(Handling::Size);
                            }
                            d.set_custom_paper(d.paper_size());
                        } else {
                            d.custom_paper = None;
                            d.custom_name = None;
                            d.save_form = None;
                            d.sheet = 0;
                        }
                    }
                    if let Some((w, h)) = d.custom_paper {
                        let unit = d.units;
                        let k = unit.per_point();
                        let range = CUSTOM_PAPER_RANGE.start() * k..=CUSTOM_PAPER_RANGE.end() * k;
                        let (mut width, mut height) = (w * k, h * k);
                        fn field(value: &mut f64, range: std::ops::RangeInclusive<f64>, unit: crate::units::Unit) -> egui::DragValue<'_> {
                            egui::DragValue::new(value).range(range).speed(unit.speed()).max_decimals(unit.decimals())
                        }
                        let a = ui.add(field(&mut width, range.clone(), unit)).on_hover_text(tl!("Width"));
                        ui.label("×");
                        let b = ui.add(field(&mut height, range, unit)).on_hover_text(tl!("Height"));
                        if a.changed() || b.changed() {
                            d.set_custom_paper((width / k, height / k));
                        }
                        unit_combo(ui, &mut d.units);
                    }
                });
                ui.end_row();
                // Saving the custom size under a name and a category, and deleting a saved one.
                if d.custom_paper.is_some() {
                    ui.label("");
                    ui.horizontal(|ui| save_paper_row(ui, d, t));
                    ui.end_row();
                }
            });
            // Windows says which drivers can't print two-sided as they're asked; they keep their
            // own setting, even when Two-sided is Off.
            if let Some(p) = d.printers.iter().find(|p| d.printer.as_ref() == Some(&p.name))
                && p.two_sided == Some(false)
            {
                ui.label(egui::RichText::new(tl!("This printer's driver doesn't take two-sided settings from other programs, so its own setting applies. Change it in Properties…")).small().color(t.text_muted));
            }
            if d.printer.as_deref() == Some(crate::web_print::BROWSER_PRINTER) {
                ui.label(egui::RichText::new(tl!("Copies, two-sided printing and colour are chosen in the browser's print dialog.")).small().color(t.text_muted));
            }
            if let Some(e) = &d.driver_error {
                ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Couldn't open the printer's preferences: {e}"), &[("e", e)])).small().color(t.text_muted));
            }
            if d.show_driver_options {
                if d.driver_options.is_none() && d.driver_options_loading.is_some() {
                    ui.label(egui::RichText::new(tl!("Reading the printer's settings…")).small().color(t.text_muted));
                } else {
                    driver_options_panel(ui, d, t);
                }
            }
            if let Some(&source) = sizes.get(d.current_page) {
                ui.horizontal_wrapped(|ui| {
                    ui.label(crate::i18n::fmt(tl!("PDF page: {size}"), &[("size", &d.units.size(source.0, source.1))]));
                    let matched = print::matching_paper(source);
                    // A page of no listed size gets a paper of its own size (portrait, as listed).
                    let own = (source.0.min(source.1), source.0.max(source.1));
                    let fits = CUSTOM_PAPER_RANGE.contains(&own.0) && CUSTOM_PAPER_RANGE.contains(&own.1);
                    if ui.add_enabled(matched.is_some() || fits, egui::Button::new(tl!("Match page at 100%"))).clicked() {
                        match matched {
                            Some(paper) => {
                                d.paper = paper;
                                d.custom_paper = None;
                                d.custom_name = None;
                                d.save_form = None;
                            }
                            None => {
                                d.set_custom_paper(own);
                            }
                        }
                        // A paper picked outright: nothing for leaving Label to give back.
                        d.paper_before_label = None;
                        d.handling = Handling::Size;
                        d.size = SizeMode::Actual;
                        d.orientation = Orientation::Auto;
                        d.sheet = 0;
                    }
                    // With Custom ticked the units sit next to its size instead.
                    if d.custom_paper.is_none() {
                        unit_combo(ui, &mut d.units);
                    }
                });
            }
            ui.add_space(6.0);
            widgets::section_title(ui, tl!("Pages to Print"));
            ui.horizontal(|ui| {
                widgets::radio_value(ui, &mut d.which, Which::All, tl!("All"));
                widgets::radio_value(ui, &mut d.which, Which::Current, tl!("Current page"));
                let pages = widgets::radio_value(ui, &mut d.which, Which::Range, tl!("Pages"));
                let r = ui.add_enabled(
                    d.which == Which::Range,
                    crate::widgets::line(&mut d.range).hint_text(format!("1-{}", sizes.len())).desired_width(110.0),
                ).labelled_by(pages.id);
                if r.gained_focus() {
                    d.which = Which::Range;
                }
            });
            // Offered only when pages were picked before the dialog opened.
            if !d.selected.is_empty() {
                let label = format!("{} ({})", tl!("Selected pages"), d.selected.len());
                widgets::radio_value(ui, &mut d.which, Which::Selected, label);
            }
            ui.horizontal(|ui| {
                ui.label(tl!("More options:"));
                combo(
                    ui,
                    "subset",
                    tl!("More options:"),
                    &mut d.subset,
                    &[(Subset::All, tl!("All pages in range")), (Subset::Odd, tl!("Odd pages only")), (Subset::Even, tl!("Even pages only"))],
                    150.0,
                );
                ui.add(widgets::checkbox(&mut d.reverse, tl!("Reverse pages")));
            });
            ui.add_space(6.0);
            widgets::section_title(ui, tl!("Page Sizing & Handling"));
            ui.horizontal_wrapped(|ui| {
                for (h, label) in [
                    (Handling::Size, tl!("Size")),
                    (Handling::Poster, tl!("Poster")),
                    (Handling::Multiple, tl!("Multiple")),
                    (Handling::Booklet, tl!("Booklet")),
                    (Handling::Label, tl!("Label")),
                ] {
                    if widgets::mode_tab(ui, label, d.handling == h).clicked() {
                        d.set_handling(h);
                    }
                }
            });
            ui.add_space(4.0);
            match d.handling {
                Handling::Size => {
                    ui.horizontal(|ui| {
                        widgets::radio_value(ui, &mut d.size, SizeMode::Fit, tl!("Fit"));
                        widgets::radio_value(ui, &mut d.size, SizeMode::Actual, tl!("Actual size"));
                        widgets::radio_value(ui, &mut d.size, SizeMode::Shrink, tl!("Shrink oversized pages"));
                    });
                    ui.horizontal(|ui| {
                        let custom = matches!(d.size, SizeMode::Custom(_));
                        if ui.add(widgets::radio(custom, tl!("Custom scale:"))).clicked() {
                            d.size = SizeMode::Custom(d.custom_scale);
                        }
                        ui.add_enabled(custom, egui::DragValue::new(&mut d.custom_scale).range(1.0..=1000.0).suffix(" %"))
                            .accessible_name(tl!("Custom scale"));
                    });
                }
                Handling::Label => {
                    ui.add(widgets::checkbox(&mut d.label_flip, tl!("Turn upside down (180°)")));
                    ui.label(
                        egui::RichText::new(tl!("Cuts each page to its printed area, turns it upright and fits it to the sheet without stretching. One label per sheet."))
                            .small()
                            .color(t.text_muted),
                    );
                }
                Handling::Poster => {
                    ui.horizontal(|ui| {
                        let scale_label = ui.label(tl!("Tile scale:"));
                        ui.add(egui::DragValue::new(&mut d.poster_scale).range(10.0..=1000.0).suffix(" %")).labelled_by(scale_label.id);
                        let overlap_label = ui.label(tl!("Overlap:"));
                        ui.add(egui::DragValue::new(&mut d.overlap).range(0.0..=144.0).suffix(" pt")).labelled_by(overlap_label.id);
                        ui.add(widgets::checkbox(&mut d.cut_marks, tl!("Cut marks")));
                    });
                }
                Handling::Multiple => {
                    ui.horizontal(|ui| {
                        ui.label(tl!("Pages per sheet:"));
                        combo(ui, "per-sheet", tl!("Pages per sheet:"), &mut d.per_sheet, &[(2, "2"), (4, "4"), (6, "6"), (9, "9"), (16, "16")], 60.0);
                        ui.label(tl!("Page order:"));
                        combo(
                            ui,
                            "order",
                            tl!("Page order:"),
                            &mut d.order,
                            &[
                                (PageOrder::Horizontal, tl!("Horizontal")),
                                (PageOrder::HorizontalReversed, tl!("Horizontal reversed")),
                                (PageOrder::Vertical, tl!("Vertical")),
                                (PageOrder::VerticalReversed, tl!("Vertical reversed")),
                                (PageOrder::CutStack, tl!("Cut and stack")),
                            ],
                            150.0,
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.add(widgets::checkbox(&mut d.border, tl!("Print page border")));
                        ui.add(widgets::checkbox(&mut d.auto_rotate, tl!("Auto-rotate pages")));
                    });
                    if d.order == PageOrder::CutStack {
                        ui.label(tl!(
                            "Print single-sided. Keep the sheets in order, cut at the marks, then stack the piles left to right, top to bottom."
                        ));
                    }
                }
                Handling::Booklet => {
                    ui.horizontal(|ui| {
                        ui.label(tl!("Booklet subset:"));
                        combo(
                            ui,
                            "booklet",
                            tl!("Booklet subset:"),
                            &mut d.booklet_subset,
                            &[
                                (BookletSubset::BothSides, tl!("Both sides")),
                                (BookletSubset::FrontOnly, tl!("Front side only")),
                                (BookletSubset::BackOnly, tl!("Back side only")),
                            ],
                            130.0,
                        );
                        ui.label(tl!("Binding:"));
                        combo(ui, "binding", tl!("Binding:"), &mut d.binding, &[(Binding::Left, tl!("Left")), (Binding::Right, tl!("Right"))], 80.0);
                    });
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(tl!("Orientation:"));
                widgets::radio_value(ui, &mut d.orientation, Orientation::Auto, tl!("Auto portrait/landscape"));
                widgets::radio_value(ui, &mut d.orientation, Orientation::Portrait, tl!("Portrait"));
                widgets::radio_value(ui, &mut d.orientation, Orientation::Landscape, tl!("Landscape"));
            });
            ui.add_space(6.0);
            widgets::section_title(ui, tl!("Comments & Forms"));
            combo(
                ui,
                "content",
                tl!("Comments & Forms"),
                &mut d.content,
                &[
                    (Content::Document, tl!("Document")),
                    (Content::DocumentAndMarkups, tl!("Document and markups")),
                    (Content::DocumentAndStamps, tl!("Document and stamps")),
                    (Content::FormFieldsOnly, tl!("Form fields only")),
                ],
                220.0,
            );
            // Advanced (Acrobat's Advanced Print Setup): print as image. Folded unless it is on.
            ui.add_space(6.0);
            egui::CollapsingHeader::new(egui::RichText::new(crate::i18n::upper_label("Advanced")).font(theme::semibold(10.5)).color(t.text_faint))
                .id_salt("print-advanced")
                .default_open(d.as_image)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(widgets::checkbox(&mut d.as_image, tl!("Print as image")));
                        ui.add_enabled_ui(d.as_image, |ui| {
                            ui.label(tl!("Resolution:"));
                            let mut dpi = d.dpi.round() as u32;
                            combo(ui, "print-dpi", tl!("Resolution:"), &mut dpi, &[(150, "150 dpi"), (300, "300 dpi"), (600, "600 dpi")], 100.0);
                            d.dpi = f64::from(dpi);
                        });
                    });
                    ui.label(
                        egui::RichText::new(tl!(
                            "Use it when a printer skips text, shading or transparency. The print is no longer searchable text; with Print in grayscale, the pictures are grey."
                        ))
                        .small()
                        .color(t.text_muted),
                    );
                });
        });
        ui.add_space(12.0);
        // Preview.
        ui.vertical(|ui| {
            ui.set_width(PREVIEW_W);
            let (area, _) = ui.allocate_exact_size(vec2(PREVIEW_W, PREVIEW_H), egui::Sense::hover());
            ui.painter().rect_filled(area, 6.0, t.hover);
            match (&settings, sheets.get(d.sheet)) {
                (Err(e), _) => {
                    ui.put(area.shrink(16.0), egui::Label::new(egui::RichText::new(tl!(e)).color(t.text_muted)).wrap());
                }
                (Ok(_), Some(sheet)) => {
                    let k = (PREVIEW_BOX.0 / sheet.size.0 as f32).min(PREVIEW_BOX.1 / sheet.size.1 as f32);
                    let paper = Rect::from_center_size(area.center(), vec2(sheet.size.0 as f32 * k, sheet.size.1 as f32 * k));
                    ui.painter().rect_filled(paper, 0.0, Color32::WHITE);
                    ui.painter().rect_stroke(paper, 0.0, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
                    let to_screen = |x: f64, y: f64| -> Pos2 { pos2(paper.left() + x as f32 * k, paper.bottom() - y as f32 * k) };
                    let painter = ui.painter().with_clip_rect(paper);
                    for pl in &sheet.placed {
                        let (dw, dh) = sizes[pl.page];
                        let [x0, y0, x1, y1] = pl.clip;
                        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| {
                            let (sx, sy) = pl.matrix.apply(x, y);
                            to_screen(sx, sy)
                        });
                        // UVs of the visible part (texture y runs down from the page top).
                        let uv = |x: f64, y: f64| pos2((x / dw) as f32, (1.0 - y / dh) as f32);
                        let uvs = [uv(x0, y0), uv(x1, y0), uv(x1, y1), uv(x0, y1)];
                        match thumb(pl.page) {
                            Some(tex) => {
                                let mut mesh = egui::Mesh::with_texture(tex);
                                for (p, u) in corners.iter().zip(uvs) {
                                    mesh.vertices.push(egui::epaint::Vertex {
                                        pos: *p,
                                        uv: u,
                                        color: if d.grayscale { Color32::from_gray(235) } else { Color32::WHITE },
                                    });
                                }
                                mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
                                painter.add(egui::Shape::mesh(mesh));
                            }
                            None => {
                                painter.add(egui::Shape::convex_polygon(
                                    corners.to_vec(),
                                    Color32::from_gray(245),
                                    Stroke::new(0.5, Color32::from_gray(180)),
                                ));
                                let c = corners.iter().fold(vec2(0.0, 0.0), |a, p| a + p.to_vec2()) / 4.0;
                                painter.text(
                                    c.to_pos2(),
                                    egui::Align2::CENTER_CENTER,
                                    (pl.page + 1).to_string(),
                                    theme::regular(11.0),
                                    Color32::from_gray(120),
                                );
                            }
                        }
                    }
                    for b in &sheet.borders {
                        painter.rect_stroke(
                            Rect::from_two_pos(to_screen(b[0], b[1]), to_screen(b[2], b[3])),
                            0.0,
                            Stroke::new(0.6, Color32::BLACK),
                            egui::StrokeKind::Middle,
                        );
                    }
                    for l in &sheet.lines {
                        painter.line_segment([to_screen(l[0], l[1]), to_screen(l[2], l[3])], Stroke::new(0.6, Color32::BLACK));
                    }
                }
                _ => {}
            }
            ui.horizontal(|ui| {
                let n = sheets.len();
                if ui.add_enabled(d.sheet > 0, egui::Button::new("‹")).on_hover_text(tl!("Previous sheet")).clicked() {
                    d.sheet -= 1;
                }
                ui.label(if n == 0 {
                    tl!("No sheets").to_string()
                } else {
                    crate::i18n::fmt(tl!("Sheet {s} of {n}"), &[("s", &(d.sheet + 1).to_string()), ("n", &n.to_string())])
                });
                if ui.add_enabled(d.sheet + 1 < n, egui::Button::new("›")).on_hover_text(tl!("Next sheet")).clicked() {
                    d.sheet += 1;
                }
            });
            if let Some(s) = sheets.get(d.sheet) {
                ui.label(egui::RichText::new(d.units.size(s.size.0, s.size.1)).small().color(t.text_muted));
                if d.handling == Handling::Size
                    && let Some(p) = s.placed.first()
                {
                    let scale = p.matrix.0[0].hypot(p.matrix.0[1]) * 100.0;
                    ui.label(crate::i18n::fmt(tl!("PDF scale: {scale}%"), &[("scale", &format!("{scale:.2}"))]));
                    ui.label(egui::RichText::new(tl!("Printer paper and scaling may differ. Check driver settings.")).small().color(t.text_muted));
                    if (scale - 100.0).abs() > 0.01 {
                        ui.label(egui::RichText::new(tl!("Drawing scale changes. Use Actual size for full-scale plans.")).small().color(t.text_muted));
                    }
                }
            }
        });
    });
    ui.add_space(12.0);
    let (mut go, mut cancel) = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let looking = still_looking(d);
        let label = if d.printer.is_some() || looking { tl!("Print") } else { tl!("Save as PDF") };
        if ui.add_enabled_ui(settings.is_ok() && !sheets.is_empty() && !looking, |ui| widgets::pill_button(ui, label, true)).inner.clicked() {
            go = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (go, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_page_preview_uses_positions_and_selection_guards() {
        for labels in [["i", "ii", "iii", "1", "2"], ["1", "1", "1", "1", "1"], ["1", "2", "3", "4", "5"]] {
            let labels: Vec<String> = labels.into_iter().map(str::to_owned).collect();
            for current_page in 0..5 {
                let draft = PrintDraft { which: Which::Current, current_page, ..Default::default() };
                let rasters = preview_rasters(&draft, &[(200.0, 300.0); 5], &labels, 1.0, 8192.0);
                assert_eq!(rasters.iter().map(|(page, _)| *page).collect::<Vec<_>>(), vec![current_page]);
            }
        }
        let labels: Vec<String> = ["i", "ii", "iii", "1", "2"].into_iter().map(str::to_owned).collect();
        let mut draft = PrintDraft { which: Which::Selected, selected: vec![0, 3], ..Default::default() };
        assert_eq!(draft.settings(5, &labels).unwrap().pages, vec![0, 3]);
        draft.which = Which::All;
        assert_eq!(draft.settings(5, &labels).unwrap().pages, vec![0, 1, 2, 3, 4]);
        draft.which = Which::Current;
        draft.reverse = true;
        draft.subset = Subset::Odd;
        assert_eq!(draft.settings(5, &labels).unwrap().pages, vec![0]);
        draft.subset = Subset::Even;
        assert!(draft.settings(5, &labels).is_err());
        draft.subset = Subset::All;
        for (count, current) in [(0, 0), (5, 5), (5, usize::MAX)] {
            draft.current_page = current;
            assert!(draft.settings(count, &labels).is_err());
        }
    }

    /// The Paper list shows every paper once, in its section, largest first; typing narrows it
    /// by name or size, ignoring case and spaces.
    #[test]
    fn the_paper_list_is_sectioned_and_searchable() {
        // Nothing is saved here, so every paper is a listed one (a saved one would index past the end).
        let listed = |p: &PaperRef| match p {
            PaperRef::Listed(i) => *i,
            PaperRef::Saved(_) => usize::MAX,
        };
        let names = |q: &str| paper_sections(q, &[]).into_iter().flat_map(|(_, items)| items).map(|p| PAPERS[listed(&p)].0).collect::<Vec<_>>();
        let all = paper_sections("", &[]);
        let mut seen: Vec<usize> = all.iter().flat_map(|(_, items)| items.iter().map(listed)).collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..PAPERS.len()).collect::<Vec<_>>(), "every paper once");
        let groups: Vec<usize> = all
            .iter()
            .map(|(h, _)| match h {
                Heading::Group(g) => Some(*g),
                Heading::Category(_) => None,
            })
            .collect::<Option<Vec<_>>>()
            .unwrap();
        assert!(groups.windows(2).all(|w| w[0] < w[1]), "sections in order, none twice");
        // Only Folio is left for Other.
        assert_eq!(
            all.iter()
                .find(|(h, _)| *h == Heading::Group(PAPER_GROUPS.len() - 1))
                .map(|(_, items)| items.iter().map(|p| PAPERS[listed(p)].0).collect::<Vec<_>>()),
            Some(vec!["Folio (210 x 330 mm)"])
        );
        let iso_a = &all[0];
        assert_eq!(
            (&iso_a.0, PAPERS[listed(&iso_a.1[0])].0, PAPERS[listed(iso_a.1.last().unwrap())].0),
            (&Heading::Group(0), "2A0 (1189 x 1682 mm)", "A8 (52 x 74 mm)"),
            "largest first"
        );
        assert_eq!(names("sra3"), ["SRA3 (320 x 450 mm)"]);
        assert_eq!(names("  SRA 3 "), ["SRA3 (320 x 450 mm)"]);
        assert_eq!(names("320x450"), ["SRA3 (320 x 450 mm)"]);
        assert_eq!(names("a4"), ["A4", "SRA4 (225 x 320 mm)", "RA4 (215 x 305 mm)"]);
        assert!(names("envelope").iter().all(|n| n.contains("Envelope")) && names("envelope").len() == 11);
        assert!(names("no such paper").is_empty());
    }

    /// Saved papers are listed in their own categories ahead of the standard sections, or with
    /// the listed papers when their category names a section, and are found by name or category.
    #[test]
    fn saved_papers_are_listed_by_category_and_searchable() {
        let saved = vec![
            SavedPaper::new("Shipping labels", "Štítky", (907.09, 1360.63)).unwrap(),
            SavedPaper::new("SRA3+", "print sheets (ra, sra)", (935.43, 1380.47)).unwrap(),
            SavedPaper::new("Banner", "", (1000.0, 3000.0)).unwrap(),
        ];
        let all = paper_sections("", &saved);
        let headings: Vec<&Heading> = all.iter().map(|(h, _)| h).collect();
        assert_eq!(
            headings[..2],
            [&Heading::Category("My sizes".into()), &Heading::Category("Štítky".into())],
            "own categories first, alphabetically"
        );
        let sheets = all.iter().find(|(h, _)| *h == Heading::Group(2)).unwrap();
        let sra3 = PaperRef::Listed(PAPERS.iter().position(|p| p.0.starts_with("SRA3 ")).unwrap());
        let at = |p: PaperRef| sheets.1.iter().position(|q| *q == p).unwrap();
        assert_eq!(at(PaperRef::Saved(1)) + 1, at(sra3), "SRA3+ goes in with the print sheets, by size");
        let found = |q: &str| paper_sections(q, &saved).into_iter().flat_map(|(_, items)| items).collect::<Vec<_>>();
        assert_eq!(found("shipping"), [PaperRef::Saved(0)]);
        assert_eq!(found("štítky"), [PaperRef::Saved(0)], "by category");
        assert_eq!(found("sra3"), [PaperRef::Saved(1), PaperRef::Listed(PAPERS.iter().position(|p| p.0.starts_with("SRA3 ")).unwrap())]);
    }

    /// A custom size is saved under a name and category, used again by name, replaced by saving
    /// the same name, and deleted; blank names, sizes out of range and a full list are refused.
    #[test]
    fn a_custom_size_is_saved_used_replaced_and_deleted() {
        let mut d = PrintDraft::default();
        assert!(d.save_paper("Nálepky", "Štítky").is_err(), "nothing to save without Custom");
        d.set_custom_paper((907.09, 1360.63));
        assert!(d.save_paper("   ", "Štítky").is_err(), "a blank name");
        assert_eq!(d.save_paper("  Nálepky  ", "  Štítky "), Ok(()));
        assert_eq!(d.saved_papers, [SavedPaper { name: "Nálepky".into(), category: "Štítky".into(), size: (907.09, 1360.63) }]);
        assert_eq!(d.custom_name.as_deref(), Some("Nálepky"));
        assert_eq!(d.paper_categories()[0], "Štítky");
        assert!(d.paper_categories().contains(&"ISO A".to_string()));
        // Changing the size leaves the saved one as it was; saving the name again replaces it.
        d.set_custom_paper((500.0, 700.0));
        assert_eq!(d.custom_name, None);
        assert_eq!(d.save_paper("NÁLEPKY", ""), Ok(()));
        assert_eq!(d.saved_papers, [SavedPaper { name: "NÁLEPKY".into(), category: "My sizes".into(), size: (500.0, 700.0) }]);
        // Using it again, from another paper.
        d.custom_paper = None;
        d.custom_name = None;
        d.use_saved_paper(0);
        assert_eq!((d.custom_paper, d.custom_name.as_deref()), (Some((500.0, 700.0)), Some("NÁLEPKY")));
        d.use_saved_paper(99);
        assert_eq!(d.custom_name.as_deref(), Some("NÁLEPKY"), "no such paper changes nothing");
        d.delete_saved_paper();
        assert!(d.saved_papers.is_empty());
        assert_eq!((d.custom_paper, d.custom_name.as_deref()), (Some((500.0, 700.0)), None), "the size stays as a custom size");
        // A full list takes no more, but a saved name can still be replaced.
        d.saved_papers = (0..SAVED_PAPERS_MAX).map(|i| SavedPaper::new(&format!("p{i}"), "", (500.0, 700.0)).unwrap()).collect();
        assert!(d.save_paper("one more", "").is_err());
        assert_eq!(d.save_paper("p7", "Other"), Ok(()));
        assert_eq!(d.saved_papers.len(), SAVED_PAPERS_MAX);
        assert!(SavedPaper::new("x", "", (10.0, 500.0)).is_none(), "out of range");
        assert_eq!(SavedPaper::new(&"n".repeat(500), "", (500.0, 500.0)).unwrap().name.chars().count(), SAVED_PAPER_TEXT_MAX);
    }

    /// A paper of the user's own size lays the sheets out at that size, goes to CUPS as its
    /// custom size, is held to the allowed range, and gives way to label stock.
    #[test]
    fn a_custom_paper_is_used_held_in_range_and_replaced_by_label_stock() {
        let labels = vec!["1".to_string()];
        let mut draft = PrintDraft::default();
        assert_eq!(draft.paper_size(), PAPERS[0].1);
        assert_eq!(draft.set_custom_paper((907.09, 1360.63)), Some((907.09, 1360.63)));
        assert_eq!(draft.settings(1, &labels).unwrap().paper, (907.09, 1360.63));
        let media = draft.job("a.pdf").options.into_iter().find(|(k, _)| k == "media");
        assert_eq!(media, Some(("media".to_string(), "Custom.907.09x1360.63".to_string())));
        assert_eq!(draft.set_custom_paper((1.0, 99_999.0)), Some((72.0, 14_400.0)), "held to the range");
        assert_eq!(draft.set_custom_paper((f64::NAN, 500.0)), None, "not a number");
        assert_eq!(draft.custom_paper, Some((72.0, 14_400.0)), "a refused size changes nothing");
        draft.set_handling(Handling::Label);
        assert_eq!((draft.custom_paper, draft.paper), (None, print::LABEL_PAPER));
        assert_eq!(draft.paper_size(), PAPERS[print::LABEL_PAPER].1);
    }

    #[test]
    fn composed_sheets_keep_exact_scaling_and_the_dialogs_duplex_choice() {
        let draft = PrintDraft { duplex: Some(spool::Duplex::LongEdge), ..PrintDraft::default() };
        let job = draft.job("report.pdf");
        assert_eq!(job.scaling, spool::Scaling::Exact);
        assert_eq!(job.duplex, Some(spool::Duplex::LongEdge));
        let off = PrintDraft { duplex: Some(spool::Duplex::Off), ..PrintDraft::default() };
        assert_eq!(off.job("report.pdf").duplex, Some(spool::Duplex::Off));
    }

    /// Untouched, Two-sided sends nothing, so the duplex set in the printer's Properties applies
    /// (#2422); cut and stack still asks for single-sided sheets.
    #[test]
    fn two_sided_follows_the_printer_until_chosen() {
        assert_eq!(PrintDraft::default().duplex, None);
        assert_eq!(PrintDraft::default().job("report.pdf").duplex, None);
        let mut cut = PrintDraft { handling: Handling::Multiple, order: PageOrder::CutStack, ..PrintDraft::default() };
        assert_eq!(cut.job("report.pdf").duplex, Some(spool::Duplex::Off));
        assert!(cut.settings(4, &[]).is_ok(), "the printer's default doesn't block cut and stack");
        cut.duplex = Some(spool::Duplex::ShortEdge);
        assert!(cut.settings(4, &[]).is_err());
    }

    /// A job being spooled holds the progress card, counting its sheets as the spooler reports
    /// them; the card goes when the job ends and its answer arrives as a notice. A card another
    /// long job is showing is left alone.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_progress_card_counts_the_sheets_of_a_print_job() {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        app.print_tasks.jobs.push(PrintJob { printer: "Office".into(), sheets: None, news: rx });
        app.poll_print();
        assert_eq!(app.progress_notice.as_ref().map(|p| p.label.as_str()), Some("Sending to Office…"));

        tx.send(JobNews::Sent(0, 3)).unwrap();
        tx.send(JobNews::Sent(1, 3)).unwrap();
        app.poll_print();
        let shown = app.progress_notice.clone().unwrap();
        assert_eq!(shown.label, "Sending sheet 2 of 3 to Office…");
        assert!((shown.fraction - 1.0 / 3.0).abs() < 1e-4 && !shown.cancellable, "{shown:?}");

        tx.send(JobNews::Sent(3, 3)).unwrap();
        tx.send(JobNews::Done(Ok(String::new()))).unwrap();
        app.poll_print();
        assert!(app.progress_notice.is_none() && app.print_tasks.jobs.is_empty());
        assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("Sent to Office"));

        // Another job's card stays; a worker that vanished is reported, not left spinning.
        let other = crate::widgets::ProgressNotice { label: "Optimizing…".into(), fraction: 0.5, cancellable: true };
        app.progress_notice = Some(other.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        app.print_tasks.jobs.push(PrintJob { printer: "Office".into(), sheets: None, news: rx });
        tx.send(JobNews::Sent(1, 2)).unwrap();
        app.poll_print();
        assert_eq!(app.progress_notice, Some(other));
        drop(tx);
        app.poll_print();
        assert!(app.print_tasks.jobs.is_empty() && app.progress_notice.is_some());
        assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("the print job stopped unexpectedly"));
    }

    /// Issue #1308: Current page is the page on screen, even when a later page is labelled with
    /// its number. A typed range still reads labels first.
    #[test]
    fn current_page_is_the_viewed_page_not_a_label() {
        let labels: Vec<String> = ["i", "ii", "iii", "1", "2"].map(String::from).to_vec();
        for (current, want) in [(0, vec![0]), (1, vec![1]), (3, vec![3])] {
            let d = PrintDraft { which: Which::Current, current_page: current, ..PrintDraft::default() };
            assert_eq!(d.settings(5, &labels).map(|s| s.pages), Ok(want), "viewing page {}", current + 1);
        }
        let typed = PrintDraft { which: Which::Range, range: "1".into(), ..PrintDraft::default() };
        assert_eq!(typed.settings(5, &labels).map(|s| s.pages), Ok(vec![3]));
    }

    #[test]
    fn print_preview_rasters_stay_within_the_texture_limit() {
        // Actual size on a 14,400 pt (200 in) page would ask for a raster far past any texture.
        let d = PrintDraft { size: SizeMode::Actual, ..PrintDraft::default() };
        let sizes = vec![(14_400.0, 14_400.0); 2];
        let labels = vec!["1".into(), "2".into()];
        for max_side in [2048.0, 8192.0, 16384.0, f32::NAN, 0.0] {
            for (page, scale) in preview_rasters(&d, &sizes, &labels, 4.0, max_side) {
                let long = 14_400.0 * scale;
                let cap = if max_side.is_finite() && max_side >= 1.0 { max_side.min(PREVIEW_MAX_SIDE) } else { PREVIEW_MAX_SIDE };
                assert!(long <= cap + 1.0, "page {page} at {max_side}: {long} px");
            }
        }
    }

    #[test]
    fn print_preview_rasters_keep_the_current_sheet_within_the_byte_budget() {
        // Many large pages: the current sheet always gets its raster; neighbours only while the
        // budget lasts, and the total stays within it.
        let d = PrintDraft { sheet: 5, ..PrintDraft::default() };
        let sizes = vec![(2_000.0, 2_000.0); 12];
        let labels: Vec<String> = (1..=12).map(|n| n.to_string()).collect();
        let rasters = preview_rasters(&d, &sizes, &labels, 4.0, 16384.0);
        assert_eq!(rasters.first().map(|r| r.0), Some(5), "the sheet on screen comes first: {rasters:?}");
        let total: f64 = rasters.iter().map(|(_, s)| (2_000.0 * f64::from(*s)).ceil().powi(2) * 4.0).sum();
        assert!(total <= PREVIEW_BYTES || rasters.len() == 1, "{total} bytes for {rasters:?}");
    }

    #[test]
    fn print_preview_is_sharper_than_a_thumbnail_on_a_high_dpi_screen() {
        let d = PrintDraft::default();
        let sizes = vec![(612.0, 792.0); 3];
        let labels = vec!["1".into(), "2".into(), "3".into()];
        let rasters = preview_rasters(&d, &sizes, &labels, 2.0, 8192.0);
        let scale = rasters.iter().find(|(page, _)| *page == 0).map(|(_, s)| *s).unwrap();
        // A 132 pt thumbnail at 2× is about 0.43 px/pt. The preview pane needs roughly twice that.
        assert!(scale > 0.7, "letter page in the preview at 2 px/pt: {scale}");
        assert!(rasters.iter().any(|(page, _)| *page == 1), "the next sheet is ready");
        let mut later = d.clone();
        later.sheet = 2;
        let rasters = preview_rasters(&later, &sizes, &labels, 2.0, 8192.0);
        assert!(rasters.iter().any(|(page, _)| *page == 2));
    }
}
