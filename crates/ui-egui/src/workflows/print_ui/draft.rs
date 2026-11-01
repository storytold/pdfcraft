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
    /// Index into the Paper list: the standard sizes, then the printer's own.
    pub paper: usize,
    /// Whether `paper` was chosen, in the dialog or restored from the last session (#2156),
    /// rather than left at the first size: a chosen paper the printer has stays chosen when the
    /// printer's sizes are read, otherwise the printer's default size is taken.
    pub paper_picked: bool,
    /// The printer's own paper sizes and the printer they were read for
    /// ([`spool::printer_papers`]); `None` until a printer is chosen.
    pub printer_papers: Option<(String, Vec<spool::PrinterPaper>)>,
    /// Whether `printer_papers` was read since the dialog opened: a driver's stock (a label
    /// printer's one "User defined" size) changes between prints, so each opening reads again.
    pub papers_read: bool,
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
            paper_picked: false,
            printer_papers: None,
            papers_read: false,
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
            // The sheet the PDF was laid out on, not the queue's default paper.
            .chain(self.media())
            .collect(),
        }
    }

    /// The printer's own paper sizes, when they were read for the chosen printer.
    pub fn own_papers(&self) -> &[spool::PrinterPaper] {
        match (&self.printer, &self.printer_papers) {
            (Some(p), Some((read_for, papers))) if p == read_for => papers,
            _ => &[],
        }
    }

    /// The printer's own sizes the Paper list offers after the standard ones, as (index,
    /// label): all of them, each under the driver's name with its size in the preferences'
    /// unit (one that is a standard size too is listed again, so what the driver calls it, a
    /// label driver's "User defined", can be chosen and seen).
    pub fn own_papers_listed(&self) -> Vec<(usize, String)> {
        self.own_papers()
            .iter()
            .enumerate()
            .map(|(i, p)| (PAPERS.len() + i, format!("{} ({})", p.label, self.units.size(p.size.0, p.size.1))))
            .collect()
    }

    /// The Paper list index of the printer's default size, when its driver reports one.
    pub fn default_own_paper(&self) -> Option<usize> {
        self.own_papers().iter().position(|p| p.default).map(|i| PAPERS.len() + i)
    }

    /// The Paper list index to start on for the printer's default size: the standard size when it
    /// is one as the driver feeds it (so it shows under its translated name, and the job still
    /// names the printer's keyword, [`Self::media`]), else the printer's own entry.
    pub fn default_paper(&self) -> Option<usize> {
        let p = self.own_papers().iter().find(|p| p.default)?;
        let same = |s: (f64, f64)| (s.0 - p.size.0).abs() <= 0.5 && (s.1 - p.size.1).abs() <= 0.5;
        PAPERS.iter().position(|(_, s)| same(*s)).or_else(|| self.default_own_paper())
    }

    /// The paper to choose once the printer's sizes are read, `size` being the one chosen so
    /// far and `key` the printer's keyword for it when it was one of the printer's own: that
    /// entry when it was chosen (`paper_picked`) and the printer still lists it under that
    /// keyword and size (so a chosen `Letter.Fullbleed` stays itself rather than becoming the
    /// standard Letter), else that size when it was chosen and the printer lists it (or lists no
    /// sizes at all), else the printer's default size when its driver reports one (the stock a
    /// label printer is set up for, an office printer's tray paper), else that size when it is
    /// a standard size or one of the printer's, else the first standard size.
    pub fn paper_after_read(&self, size: (f64, f64), key: Option<&str>) -> usize {
        let own = self.own_papers();
        let same_entry = key.and_then(|k| own.iter().position(|p| p.key == k && spool::paper_of_size(std::slice::from_ref(p), size).is_some()));
        let kept = same_entry.map(|i| PAPERS.len() + i).or_else(|| self.paper_index(size));
        let listed = own.is_empty() || spool::paper_of_size(own, size).is_some();
        kept.filter(|_| self.paper_picked && listed).or_else(|| self.default_paper()).or(kept).unwrap_or(0)
    }

    /// The chosen paper's size in points: a standard size portrait, one of the printer's own as
    /// its driver feeds it (a label's width is across the feed); a stale index reads as the first size.
    pub fn paper_size(&self) -> (f64, f64) {
        match self.paper.checked_sub(PAPERS.len()) {
            None => PAPERS.get(self.paper).map_or(PAPERS[0].1, |p| p.1),
            Some(i) => self.own_papers().get(i).map_or(PAPERS[0].1, |p| p.size),
        }
    }

    /// The chosen paper as the dialog names it: a standard size translated, one of the
    /// printer's own as its driver names it, with its size.
    pub fn paper_name(&self) -> String {
        match self.paper.checked_sub(PAPERS.len()).map(|i| self.own_papers().get(i)) {
            Some(Some(p)) => format!("{} ({})", p.label, self.units.size(p.size.0, p.size.1)),
            Some(None) => tl!(PAPERS[0].0).to_string(),
            None => tl!(PAPERS.get(self.paper).map_or(PAPERS[0].0, |p| p.0)).to_string(),
        }
    }

    /// The index in the Paper list of a sheet of this size (points, either way round): a
    /// standard size, else one of the printer's own.
    pub fn paper_index(&self, size: (f64, f64)) -> Option<usize> {
        print::matching_paper(size).or_else(|| spool::paper_of_size(self.own_papers(), size).map(|i| PAPERS.len() + i))
    }

    /// The `media` option the job names, so the spooler feeds the sheet the PDF was laid out on
    /// rather than the queue's default: the printer's own keyword for the chosen size when it has
    /// one (its default when that is this size as fed, since a driver can offer one size under
    /// two keywords, `Letter` and `Letter.Fullbleed`), else the CUPS name of the standard size
    /// ([`print::media_option`]).
    pub fn media(&self) -> Option<(String, String)> {
        let own = self.own_papers();
        let index = match self.paper.checked_sub(PAPERS.len()) {
            Some(i) => Some(i),
            None => {
                let size = self.paper_size();
                let fed = |p: &spool::PrinterPaper| (p.size.0 - size.0).abs() <= 0.5 && (p.size.1 - size.1).abs() <= 0.5;
                own.iter().position(|p| p.default && fed(p)).or_else(|| spool::paper_of_size(own, size))
            }
        };
        index.and_then(|i| own.get(i)).map(|p| ("media".to_string(), p.key.clone())).or_else(|| print::media_option(self.paper))
    }
}
