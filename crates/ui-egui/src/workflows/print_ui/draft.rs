use super::setups::{self, PrinterSetup};
use super::{Binding, BookletSubset, Content, Layout, Orientation, PageOrder, SizeMode, Subset, print, spool};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    All,
    Current,
    Range,
    /// The pages picked in the Pages panel or the organize grid when the dialog opened.
    Selected,
    /// Only the area chosen with Snapshot (`PrintDraft::area`).
    Area,
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

/// The percent a custom scale can be (the dialog's field and a saved setting).
pub const CUSTOM_SCALE: std::ops::RangeInclusive<f64> = 1.0..=1000.0;

/// The resolutions the Print dialog offers for Print as image, in dots per inch.
pub const IMAGE_DPI: [f64; 3] = [150.0, 300.0, 600.0];

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
    /// The area Snapshot's Print area… asked for: (page, rectangle in display space, points, y up).
    pub area: Option<(usize, [f64; 4])>,
    pub subset: Subset,
    pub reverse: bool,
    pub handling: Handling,
    pub size: SizeMode,
    pub custom_scale: f64,
    pub per_sheet: usize,
    /// Pages per sheet: Custom, `cols` across by `rows` down instead of `per_sheet`.
    pub custom_grid: bool,
    pub cols: usize,
    pub rows: usize,
    pub order: PageOrder,
    pub border: bool,
    pub auto_rotate: bool,
    pub booklet_subset: BookletSubset,
    pub binding: Binding,
    /// Booklet creep compensation, points per sheet.
    pub creep: f64,
    pub poster_scale: f64,
    pub overlap: f64,
    pub cut_marks: bool,
    pub orientation: Orientation,
    pub content: Content,
    /// Print as image (Acrobat's Advanced Print Setup) at `dpi`.
    pub as_image: bool,
    pub dpi: f64,
    /// Index into the paper list: the standard sizes, then `custom_papers`.
    pub paper: usize,
    /// Paper sizes added with Custom…, kept with the settings.
    pub custom_papers: Vec<crate::print_paper::CustomPaper>,
    /// The custom paper being added: name, width and height as typed (the preferences' unit).
    pub custom_new: Option<(String, String, String)>,
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
    /// Whether each printer's driver takes two-sided settings, as read in the background for
    /// the printer picked (see [`TwoSidedSource`](super::TwoSidedSource)); `None` when it couldn't be read.
    pub two_sided: std::collections::BTreeMap<String, Option<bool>>,
    /// The paper, sizing and two-sided choice each printer was last printed with, the printer
    /// printed to last first; kept across restarts (#2001).
    pub setups: Vec<(String, PrinterSetup)>,
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
            area: None,
            subset: Subset::All,
            reverse: false,
            handling: Handling::Size,
            size: SizeMode::Fit,
            custom_scale: 100.0,
            per_sheet: 2,
            custom_grid: false,
            cols: 2,
            rows: 1,
            order: PageOrder::Horizontal,
            border: false,
            auto_rotate: true,
            booklet_subset: BookletSubset::BothSides,
            binding: Binding::Left,
            creep: 0.0,
            poster_scale: 200.0,
            overlap: 18.0,
            cut_marks: true,
            orientation: Orientation::Auto,
            content: Content::DocumentAndMarkups,
            as_image: false,
            dpi: 300.0,
            paper: 0,
            custom_papers: Vec::new(),
            custom_new: None,
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
            two_sided: Default::default(),
            setups: Vec::new(),
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
            // A label goes on label stock.
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

    /// Remember this draft's paper, sizing and two-sided choice for its printer, after printing
    /// to it (#2001). Save as PDF has none to remember.
    pub fn remember_setup(&mut self) {
        let Some(printer) = self.printer.clone() else { return };
        // Label's stock is that job's; the printer keeps the paper Label replaced.
        let paper = if self.handling == Handling::Label { self.paper_before_label.unwrap_or(self.paper) } else { self.paper };
        // By name, so a custom paper is found again however the list changes.
        let setup = PrinterSetup { paper: self.paper_name_at(paper), size: self.size, custom_scale: self.custom_scale, duplex: self.duplex };
        setups::record(&mut self.setups, &printer, setup);
    }

    /// Bring back what the chosen printer was last printed with, when it has been printed to
    /// (#2001); a printer never printed to keeps the dialog's choices. While Label holds the
    /// label stock, the printer's paper is what leaving Label gives back. A custom paper taken
    /// off the list since leaves the paper as it is.
    pub fn apply_setup(&mut self) {
        let Some(printer) = self.printer.as_deref() else { return };
        let Some(setup) = self.setups.iter().find(|(name, _)| name == printer).map(|(_, s)| s.clone()) else { return };
        self.size = setup.size;
        self.custom_scale = setup.custom_scale;
        self.duplex = setup.duplex;
        if let Some(paper) = self.paper_index(&setup.paper) {
            if self.handling == Handling::Label {
                self.paper_before_label = Some(paper);
            } else {
                self.paper = paper;
            }
        }
        self.sheet = 0;
    }

    /// The printer printed to last, while `printers` still lists it (#2001).
    pub fn last_printer<'a>(&self, printers: &'a [spool::Printer]) -> Option<&'a spool::Printer> {
        let (last, _) = self.setups.first()?;
        printers.iter().find(|p| p.name == *last)
    }

    /// The engine settings for this draft (page count and labels from the document).
    pub fn settings(&self, count: usize, labels: &[String]) -> Result<print::Settings, String> {
        let size = Layout::Size(match self.size {
            SizeMode::Custom(_) => SizeMode::Custom(self.custom_scale),
            m => m,
        });
        let paper = self.paper_size();
        let raster = self.as_image.then_some(print::Raster { dpi: self.dpi, grayscale: self.grayscale });
        // A selected area prints alone, its one page sized like a page of its own: Size is its
        // only handling (the dialog offers no other while it is chosen).
        if self.which == Which::Area {
            let Some((page, area)) = self.area.filter(|(p, _)| *p < count) else {
                return Err("The selected area is no longer on a page of this document.".into());
            };
            return Ok(print::Settings {
                pages: vec![page],
                paper,
                orientation: self.orientation,
                layout: size,
                content: self.content,
                raster,
                crops: Vec::new(),
                area: Some(area),
            });
        }
        if self.cut_and_stack() && matches!(self.duplex, Some(spool::Duplex::LongEdge | spool::Duplex::ShortEdge)) {
            return Err("Cut and stack needs Two-sided: Off. Print single-sided sheets.".into());
        }
        let range = match self.which {
            Which::All | Which::Selected | Which::Current | Which::Area => None,
            Which::Range => Some(self.range.clone()),
        };
        // Selected and Current are positions, not a typed range: a range would read numbers as
        // page labels first.
        let pages = match self.which {
            Which::Selected => print::select_listed(count, &self.selected, self.subset, self.reverse),
            Which::Current => print::select_listed(count, &[self.current_page], self.subset, self.reverse),
            Which::All | Which::Range | Which::Area => print::select_pages(count, range.as_deref(), labels, self.subset, self.reverse),
        }
        // What the person got wrong about what to print is said in their language: the engine hands
        // the wording over whole and the page numbers apart from it.
        .map_err(|e| e.said(&crate::i18n::words))?;
        let layout = match self.handling {
            Handling::Size => size,
            Handling::Multiple => {
                let grid = if self.custom_grid {
                    Layout::grid(self.cols, self.rows).ok_or_else(|| tl!("Columns and rows must each be 1 to 99.").to_string())?
                } else {
                    Layout::multiple(self.per_sheet)
                };
                match grid {
                    Layout::Multiple { cols, rows, per_sheet, custom, .. } => {
                        Layout::Multiple { cols, rows, per_sheet, order: self.order, border: self.border, auto_rotate: self.auto_rotate, custom }
                    }
                    other => other,
                }
            }
            Handling::Booklet => Layout::Booklet { subset: self.booklet_subset, binding: self.binding, creep: self.creep },
            Handling::Poster => Layout::Poster { scale: self.poster_scale, overlap: self.overlap, cut_marks: self.cut_marks },
            Handling::Label => Layout::Label { flip: self.label_flip },
        };
        Ok(print::Settings {
            pages,
            paper,
            orientation: self.orientation,
            layout,
            content: self.content,
            raster: self.raster(),
            crops: self.crops.clone(),
            area: None,
        })
    }

    /// Print as image: pictures at the chosen resolution, grey when grayscale is asked for.
    fn raster(&self) -> Option<print::Raster> {
        self.as_image.then_some(print::Raster { dpi: self.dpi, grayscale: self.grayscale })
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
            grayscale: print::driver_needs_grayscale(self.grayscale, self.raster()),
            title: title.to_string(),
            // Only for the printer the options were read from.
            options: match (&self.printer, &self.driver_options) {
                (Some(p), Some((read_for, _))) if p == read_for => self.driver_choices.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                _ => Vec::new(),
            }
            .into_iter()
            // The sheet the PDF was laid out on, not the queue's default paper.
            .chain(print::media_option(self.paper))
            .collect(),
        }
    }
}
