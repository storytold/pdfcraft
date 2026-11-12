use super::setups::{self, PrinterSetup};
use super::{Binding, BookletSubset, Content, Layout, Orientation, PageOrder, SizeMode, Subset, print, spool};
use pdfcraft_engine::print::PAPERS;

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
    /// Whether the user has explicitly chosen or overridden the paper size.
    pub paper_overridden: bool,
    /// The list of papers available for selection (standard PAPERS plus any detected from the driver).
    pub papers: Vec<(String, (f64, f64))>,
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
    /// The printer whose papers and default paper are being read in the background.
    pub papers_loading: Option<String>,
    /// A printer change requested reading papers; the app starts the read on the next frame.
    pub read_papers_for: Option<String>,
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
            paper_overridden: false,
            papers: default_papers(),
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
            papers_loading: None,
            read_papers_for: None,
            two_sided: Default::default(),
            setups: Vec::new(),
        }
    }
}

/// Standard fallback paper presets.
pub fn default_papers() -> Vec<(String, (f64, f64))> {
    PAPERS.iter().map(|(n, s)| (n.to_string(), *s)).collect()
}

impl PrintDraft {
    /// Find the index of a paper in `self.papers` matching `size`, allowing for orientation.
    pub fn matching_paper(&self, size: (f64, f64)) -> Option<usize> {
        print::matching_paper(size).or_else(|| {
            self.papers.iter().position(|(_, p)| {
                ((p.0 - size.0).abs() <= 0.5 && (p.1 - size.1).abs() <= 0.5) || ((p.0 - size.1).abs() <= 0.5 && (p.1 - size.0).abs() <= 0.5)
            })
        })
    }

    /// Whether automatic paper detection for the selected printer is in progress and unresolved.
    ///
    /// When true, the user has not explicitly chosen or overridden the paper, and the selected
    /// printer's media sizes and default paper are still being read in the background off the GUI thread.
    pub fn papers_loading(&self) -> bool {
        let Some(p) = self.printer.as_deref() else { return false };
        !self.paper_overridden && (self.papers_loading.as_deref() == Some(p) || self.read_papers_for.as_deref() == Some(p))
    }

    /// Reset an automatically selected custom paper to a standard preset on printer change.
    ///
    /// If the currently selected paper does not match any standard preset and `paper_overridden` is false,
    /// resets `paper` to 0 (the standard default preset) so printer A's custom paper does not
    /// carry over when switching to printer B. Also resets `paper_before_label` if it refers to
    /// an unoverridden custom paper. Explicit user overrides (`paper_overridden == true`) are preserved.
    pub fn reset_unoverridden_custom_paper(&mut self) {
        if !self.paper_overridden {
            if print::matching_paper(self.paper_size()).is_none() {
                self.paper = 0;
            }
            if let Some(idx) = self.paper_before_label {
                let is_custom = self.papers.get(idx).is_none_or(|(_, s)| print::matching_paper(*s).is_none());
                if is_custom {
                    self.paper_before_label = Some(0);
                }
            }
        }
    }

    /// Refresh `self.papers` with driver-reported media sizes, preserving the current selection.
    pub fn update_printer_papers(&mut self) {
        self.update_printer_papers_with(None);
    }

    /// Refresh `self.papers` with driver-reported media sizes from an optional source or the driver.
    pub fn update_printer_papers_with(&mut self, source: Option<&super::PrinterPapersSource>) {
        let detected = if let Some(ref printer) = self.printer {
            if let Some(source) = source { source(printer) } else { spool::printer_papers(printer) }
        } else {
            Vec::new()
        };
        self.apply_printer_papers(detected);
    }

    /// Apply driver-reported media sizes to `self.papers`, preserving the current selection.
    pub fn apply_printer_papers(&mut self, detected: Vec<(String, (f64, f64))>) {
        let custom_paper_idx = self.paper.checked_sub(self.papers.len());
        let before_label_custom = self.paper_before_label.and_then(|idx| idx.checked_sub(self.papers.len()));
        let before_label = if before_label_custom.is_some() { None } else { self.paper_before_label.and_then(|idx| self.papers.get(idx).cloned()) };
        let current_name = self.paper_name().to_string();
        let current_size = self.paper_size();
        let mut papers = default_papers();
        for (name, size) in detected {
            if !(size.0 >= 72.0 && size.1 >= 72.0 && size.0.is_finite() && size.1.is_finite()) {
                continue;
            }
            let norm = (size.0.min(size.1), size.0.max(size.1));
            if !papers.iter().any(|(_, s)| {
                let s_norm = (s.0.min(s.1), s.0.max(s.1));
                (s_norm.0 - norm.0).abs() <= 0.5 && (s_norm.1 - norm.1).abs() <= 0.5
            }) {
                papers.push((name, size));
            }
        }
        self.papers = papers;
        if let Some(c_idx) = custom_paper_idx {
            if c_idx < self.custom_papers.len() {
                self.paper = self.papers.len() + c_idx;
            } else {
                self.paper = 0;
            }
        } else if let Some(pos) =
            self.papers.iter().position(|(n, s)| n == &current_name && (s.0 - current_size.0).abs() <= 0.5 && (s.1 - current_size.1).abs() <= 0.5)
        {
            self.paper = pos;
        } else if let Some(pos) = self.papers.iter().position(|(_, s)| (s.0 - current_size.0).abs() <= 0.5 && (s.1 - current_size.1).abs() <= 0.5) {
            self.paper = pos;
        } else {
            self.papers.push((current_name, current_size));
            self.paper = self.papers.len() - 1;
        }
        if let Some(c_idx) = before_label_custom {
            if c_idx < self.custom_papers.len() {
                self.paper_before_label = Some(self.papers.len() + c_idx);
            } else {
                self.paper_before_label = None;
            }
        } else {
            self.paper_before_label = before_label.map(|(name, size)| {
                if let Some(pos) = self.papers.iter().position(|(n, s)| n == &name && (s.0 - size.0).abs() <= 0.5 && (s.1 - size.1).abs() <= 0.5) {
                    pos
                } else if let Some(pos) = self.papers.iter().position(|(_, s)| (s.0 - size.0).abs() <= 0.5 && (s.1 - size.1).abs() <= 0.5) {
                    pos
                } else {
                    self.papers.push((name, size));
                    self.papers.len() - 1
                }
            });
        }
    }

    /// Auto-select the printer's configured default paper (from DEVMODE / driver settings).
    pub fn auto_select_default_paper(&mut self) {
        self.auto_select_default_paper_with(None);
    }

    /// Auto-select the printer's default paper using an optional source (or DEVMODE / driver settings).
    pub fn auto_select_default_paper_with(&mut self, source: Option<&super::DefaultPaperSource>) {
        if let Some(ref printer) = self.printer {
            let detected = if let Some(source) = source { source(printer) } else { spool::printer_default_paper(printer) };
            self.apply_default_paper(detected);
        }
    }

    /// Apply the detected default paper.
    pub fn apply_default_paper(&mut self, detected: Option<(String, (f64, f64))>) {
        if let Some((name, size)) = detected
            && let Some(pos) = self.find_or_add_paper(name, size)
        {
            if self.handling == Handling::Label {
                if !self.paper_overridden {
                    self.paper_before_label = Some(pos);
                }
            } else {
                self.paper = pos;
            }
        }
    }

    /// Locate `(name, size)` in `self.papers`, or append it if missing. Returns the paper index.
    pub fn find_or_add_paper(&mut self, name: String, size: (f64, f64)) -> Option<usize> {
        if !(size.0 >= 72.0 && size.1 >= 72.0 && size.0.is_finite() && size.1.is_finite()) {
            return None;
        }
        if let Some(pos) = print::matching_paper(size) {
            Some(pos)
        } else if !name.is_empty() {
            let norm = (size.0.min(size.1), size.0.max(size.1));
            if let Some(pos) = self.papers.iter().position(|(n, s)| {
                n.eq_ignore_ascii_case(&name) && {
                    let s_norm = (s.0.min(s.1), s.0.max(s.1));
                    (s_norm.0 - norm.0).abs() <= 1.0 && (s_norm.1 - norm.1).abs() <= 1.0
                }
            }) {
                Some(pos)
            } else if let Some(pos) = self.matching_paper(size) {
                Some(pos)
            } else {
                self.papers.push((name, size));
                Some(self.papers.len() - 1)
            }
        } else if let Some(pos) = self.matching_paper(size) {
            Some(pos)
        } else {
            let custom_name = format!("{:.1} x {:.1} pt", size.0, size.1);
            self.papers.push((custom_name, size));
            Some(self.papers.len() - 1)
        }
    }

    /// Apply paper size and orientation chosen in printer properties or page setup.
    pub fn apply_driver_paper(&mut self, name: String, size: (f64, f64)) {
        if let Some(pos) = self.find_or_add_paper(name, size) {
            self.paper_overridden = true;
            self.paper = pos;
            if size.0 > size.1 {
                self.orientation = Orientation::Landscape;
            } else if size.1 > size.0 {
                self.orientation = Orientation::Portrait;
            }
        }
    }

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
        let setup =
            PrinterSetup { paper: self.paper_name_at(paper).to_string(), size: self.size, custom_scale: self.custom_scale, duplex: self.duplex };
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
            // The paper this printer was last printed on is a choice, as one made in the dialog:
            // the driver's default paper, read in the background (#5861), must not replace it.
            self.paper_overridden = true;
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
        let raster = self.raster();
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
        if self.cut_and_stack() && matches!(self.chosen_duplex(), Some(spool::Duplex::LongEdge | spool::Duplex::ShortEdge)) {
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
        self.as_image.then_some(print::Raster { dpi: self.dpi, grayscale: self.grayscale, colour: None })
    }

    /// Whether multiple pages per sheet should be cut and stacked into sorted piles.
    fn cut_and_stack(&self) -> bool {
        self.handling == Handling::Multiple && self.order == PageOrder::CutStack
    }

    /// Whether the chosen printer's driver takes two-sided settings from other programs:
    /// `Some(false)` for one that keeps its own (no two-sided printing, or only manual two-sided in
    /// its Properties); `None` when unknown, and for Save as PDF. The printer list says when it
    /// knows; on Windows it lists the queues without asking their drivers (#3554), and the answer
    /// for the printer picked arrives in [`PrintDraft::two_sided`].
    pub fn printer_two_sided(&self) -> Option<bool> {
        let p = self.printers.iter().find(|p| self.printer.as_ref() == Some(&p.name))?;
        p.two_sided.or_else(|| self.two_sided.get(&p.name).copied().flatten())
    }

    /// The Two-sided choice as the chosen printer can take it: long or short edge is left out for
    /// a printer whose driver doesn't take two-sided settings from other programs, so its own
    /// setting applies instead of the job being refused (#3208). Off and the printer's default
    /// stay as chosen: one-sided is accepted by every driver.
    fn chosen_duplex(&self) -> Option<spool::Duplex> {
        match self.duplex {
            Some(spool::Duplex::LongEdge | spool::Duplex::ShortEdge) if self.printer_two_sided() == Some(false) => None,
            chosen => chosen,
        }
    }

    /// The spooler job for this print: the dialog's copies, two-sided and colour settings, the
    /// driver options changed from the printer's defaults, and the chosen paper.
    pub fn job(&self, title: &str) -> spool::Job {
        spool::Job {
            printer: self.printer.clone(),
            copies: self.copies.max(1),
            collate: self.collate,
            // Cut and stack needs single-sided sheets, whatever the printer's default; a booklet
            // decides two-sided itself (#2707), as far as the driver takes it (#5772), leaving the
            // Two-sided choice as it was.
            duplex: if self.cut_and_stack() {
                Some(spool::Duplex::Off)
            } else if self.handling == Handling::Booklet {
                print::booklet_job_duplex(self.booklet_subset, self.printer_two_sided())
            } else {
                self.chosen_duplex()
            },
            scaling: spool::Scaling::Exact,
            grayscale: print::driver_needs_grayscale(self.grayscale, self.raster().as_ref()),
            // Set when the copy sent is made grey (`spooler_copy`, #5806).
            grey_pages: false,
            title: title.to_string(),
            // Only for the printer the options were read from.
            options: match (&self.printer, &self.driver_options) {
                (Some(p), Some((read_for, _))) if p == read_for => self.driver_choices.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                _ => Vec::new(),
            }
            .into_iter()
            // The sheet the PDF was laid out on, not the queue's default paper.
            // A user's custom label is not a keyword from the driver's media catalog.
            .chain(Some(print::paper_media_option((!self.paper_is_custom()).then(|| self.paper_name()).as_deref(), self.paper_size())))
            .collect(),
        }
    }
}
