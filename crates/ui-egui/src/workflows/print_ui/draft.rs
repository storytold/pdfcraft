use super::{Binding, BookletSubset, Content, Layout, Orientation, PAPERS, PageOrder, SizeMode, Subset, print, spool};

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
    /// Pages per sheet: Custom, `cols` across by `rows` down instead of `per_sheet`.
    pub custom_grid: bool,
    pub cols: usize,
    pub rows: usize,
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
            custom_grid: false,
            cols: 2,
            rows: 1,
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
        }
    }
}

/// Standard fallback paper presets.
pub fn default_papers() -> Vec<(String, (f64, f64))> {
    PAPERS.iter().map(|(n, s)| (n.to_string(), *s)).collect()
}

impl PrintDraft {
    /// The display name of the currently selected paper.
    pub fn paper_name(&self) -> &str {
        self.papers.get(self.paper).map(|p| p.0.as_str()).or_else(|| PAPERS.get(self.paper).map(|p| p.0)).unwrap_or(PAPERS[0].0)
    }

    /// The dimensions in points `(width, height)` of the currently selected paper.
    pub fn paper_size(&self) -> (f64, f64) {
        self.papers.get(self.paper).map(|p| p.1).or_else(|| PAPERS.get(self.paper).map(|p| p.1)).unwrap_or(PAPERS[0].1)
    }

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
        let before_label = self.paper_before_label.and_then(|idx| self.papers.get(idx).cloned());
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
        let current_name = self.paper_name().to_string();
        let current_size = self.paper_size();
        self.papers = papers;
        if let Some(pos) =
            self.papers.iter().position(|(n, s)| n == &current_name && (s.0 - current_size.0).abs() <= 0.5 && (s.1 - current_size.1).abs() <= 0.5)
        {
            self.paper = pos;
        } else if let Some(pos) = self.papers.iter().position(|(_, s)| (s.0 - current_size.0).abs() <= 0.5 && (s.1 - current_size.1).abs() <= 0.5) {
            self.paper = pos;
        } else {
            self.papers.push((current_name, current_size));
            self.paper = self.papers.len() - 1;
        }
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

    /// Whether multiple pages per sheet should be cut and stacked into sorted piles.
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
            // The sheet the PDF was laid out on, not the queue's default paper.
            .chain(Some(print::paper_media_option(Some(self.paper_name()), self.paper_size())))
            .collect(),
        }
    }
}
