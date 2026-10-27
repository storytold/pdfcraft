use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Orientation {
    #[default]
    Auto,
    Portrait,
    Landscape,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum SizeMode {
    #[default]
    Fit,
    Actual,
    Shrink,
    /// Percent.
    Custom(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PageOrder {
    #[default]
    Horizontal,
    HorizontalReversed,
    Vertical,
    VerticalReversed,
    /// Single-sided sheets: cut into cell piles, then stack them in row-major order.
    CutStack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BookletSubset {
    #[default]
    BothSides,
    FrontOnly,
    BackOnly,
}

/// How a booklet goes through the printer (#2707): it decides two-sided printing itself, as
/// Acrobat does (its Print dialog hides Two-sided while Booklet is chosen; Using Acrobat X Pro,
/// Print dialog box options: "Acrobat automatically enables duplex printing, if available, for
/// booklet printing"). Both sides flip on the short edge, the fold of a 2-up sheet turned
/// sideways; front side only or back side only is one pass of a manual duplex and prints
/// single-sided.
pub fn booklet_duplex(subset: BookletSubset) -> spool::Duplex {
    match subset {
        BookletSubset::BothSides => spool::Duplex::ShortEdge,
        BookletSubset::FrontOnly | BookletSubset::BackOnly => spool::Duplex::Off,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Binding {
    #[default]
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Layout {
    Size(SizeMode),
    Multiple {
        cols: usize,
        rows: usize,
        /// Source pages on each sheet, at most `cols × rows`: three per sheet uses a 2 × 2 grid
        /// with one cell left blank.
        per_sheet: usize,
        order: PageOrder,
        border: bool,
        auto_rotate: bool,
    },
    Booklet {
        subset: BookletSubset,
        binding: Binding,
    },
    /// Tile scale (percent), overlap (points), cut marks.
    Poster {
        scale: f64,
        overlap: f64,
        cut_marks: bool,
    },
    /// Shipping label: each page cut to its [`Settings::crops`] box, turned so its text reads
    /// upright on the sheet and fitted with [`LABEL_MARGIN`]; `flip` turns it a further 180°.
    Label {
        flip: bool,
    },
}

impl Default for Layout {
    fn default() -> Self {
        Layout::Size(SizeMode::Fit)
    }
}

impl Layout {
    /// Acrobat's pages-per-sheet choices as a grid (columns × rows on a portrait sheet).
    pub fn multiple(per_sheet: usize) -> Layout {
        let (cols, rows) = match per_sheet {
            2 => (1, 2),
            4 => (2, 2),
            6 => (2, 3),
            9 => (3, 3),
            16 => (4, 4),
            n => {
                let c = (n as f64).sqrt().ceil().max(1.0) as usize;
                (c, n.div_ceil(c).max(1))
            }
        };
        Layout::Multiple { cols, rows, per_sheet: per_sheet.max(1), order: PageOrder::Horizontal, border: false, auto_rotate: true }
    }
}

/// Acrobat's Comments & Forms choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Content {
    /// Page content and form fields.
    Document,
    /// Page content, form fields and every printable comment.
    #[default]
    DocumentAndMarkups,
    /// Page content, form fields and stamps.
    DocumentAndStamps,
    /// Form fields only.
    FormFieldsOnly,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Source pages (0-based) in print order; see [`select_pages`].
    pub pages: Vec<usize>,
    /// Paper (width, height) in points, portrait.
    pub paper: (f64, f64),
    pub orientation: Orientation,
    pub layout: Layout,
    pub content: Content,
    /// For [`Layout::Label`], per source page: the area to keep (display space) and whether its
    /// text runs bottom-to-top (so a quarter turn clockwise makes it upright). `None` or a
    /// missing entry keeps the whole page.
    pub crops: Vec<Option<Crop>>,
    /// Print as image (Acrobat's Advanced Print Setup): every page rendered to a picture at this
    /// resolution before it is placed on the paper. The engine does the rendering; `impose` is
    /// then given the image-only document.
    pub raster: Option<Raster>,
}

/// How pages are rendered when printing as image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Raster {
    /// Dots per inch, 72–600 (Acrobat offers 150, 300 and 600).
    pub dpi: f64,
    /// Render the pictures in grey, so the print-ready PDF itself is grayscale.
    pub grayscale: bool,
}

impl Default for Raster {
    fn default() -> Self {
        Raster { dpi: 300.0, grayscale: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub rect: [f64; 4],
    pub text_up: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            pages: Vec::new(),
            paper: PAPERS[0].1,
            orientation: Orientation::Auto,
            layout: Layout::default(),
            content: Content::default(),
            raster: None,
            crops: Vec::new(),
        }
    }
}

/// Common paper sizes (points, portrait).
pub const PAPERS: [(&str, (f64, f64)); 21] = [
    ("US Letter", (612.0, 792.0)),
    ("US Legal", (612.0, 1008.0)),
    ("Tabloid", (792.0, 1224.0)),
    ("A3", (841.89, 1190.55)),
    ("A4", (595.28, 841.89)),
    ("A5", (419.53, 595.28)),
    ("A6", (297.64, 419.53)),
    ("ARCH A (9 x 12 in)", (648.0, 864.0)),
    ("ARCH B (12 x 18 in)", (864.0, 1296.0)),
    ("ARCH C (18 x 24 in)", (1296.0, 1728.0)),
    ("ARCH D (24 x 36 in)", (1728.0, 2592.0)),
    ("ARCH E1 (30 x 42 in)", (2160.0, 3024.0)),
    ("ARCH E (36 x 48 in)", (2592.0, 3456.0)),
    ("ANSI C (17 x 22 in)", (1224.0, 1584.0)),
    ("ANSI D (22 x 34 in)", (1584.0, 2448.0)),
    ("ANSI E (34 x 44 in)", (2448.0, 3168.0)),
    ("A2", (1190.55, 1683.78)),
    ("A1", (1683.78, 2383.94)),
    ("A0", (2383.94, 3370.39)),
    ("4 x 6 in label (101.6 x 152.4 mm)", (288.0, 432.0)),
    ("US #10 Envelope (4.125 x 9.5 in)", (297.0, 684.0)),
];

/// `PAPERS` index of the 4 × 6 in shipping label.
pub const LABEL_PAPER: usize = 19;

/// The CUPS media name for `PAPERS[index]`, so the spooler feeds the sheet the PDF was laid out
/// on rather than the queue's default; `None` only past the end of `PAPERS`. The common sizes
/// use their PPD keywords (`w288h432` is the usual 4×6 label, `Env10` the US #10 envelope).
/// The rest use PWG 5101.1 names (ARCH E1 is its `wide-format` size), spelled exactly as the
/// spec and the IANA registry have them: a driverless printer may take only the names it
/// advertises. They carry their dimensions, so CUPS feeds the PPD size of those dimensions
/// whatever the driver calls it (`ARCHD`, `AnsiD`) (#3752).
pub fn cups_media(index: usize) -> Option<&'static str> {
    Some(match PAPERS.get(index)?.0 {
        "US Letter" => "Letter",
        "US Legal" => "Legal",
        "Tabloid" => "Tabloid",
        "A3" => "A3",
        "A4" => "A4",
        "A5" => "A5",
        "A6" => "iso_a6_105x148mm",
        "ARCH A (9 x 12 in)" => "na_arch-a_9x12in",
        "ARCH B (12 x 18 in)" => "na_arch-b_12x18in",
        "ARCH C (18 x 24 in)" => "na_arch-c_18x24in",
        "ARCH D (24 x 36 in)" => "na_arch-d_24x36in",
        "ARCH E1 (30 x 42 in)" => "na_wide-format_30x42in",
        "ARCH E (36 x 48 in)" => "na_arch-e_36x48in",
        "ANSI C (17 x 22 in)" => "na_c_17x22in",
        "ANSI D (22 x 34 in)" => "na_d_22x34in",
        "ANSI E (34 x 44 in)" => "na_e_34x44in",
        "A2" => "iso_a2_420x594mm",
        "A1" => "iso_a1_594x841mm",
        "A0" => "iso_a0_841x1189mm",
        "US #10 Envelope (4.125 x 9.5 in)" => "Env10",
        _ if index == LABEL_PAPER => "w288h432",
        // A size added to PAPERS without a name here: its tests fail, and meanwhile the queue's
        // paper stands, as it did for every size before #1583.
        _ => return None,
    })
}

/// The job option that feeds `PAPERS[index]` (`media`, see [`cups_media`]), for the Print
/// dialog's jobs and `doc_print`'s alike.
pub fn media_option(index: usize) -> Option<(String, String)> {
    cups_media(index).map(|m| ("media".to_string(), m.to_string()))
}
