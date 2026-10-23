//! Scan & OCR ▸ From scanner: bed or feeder, a preview, and a region.
//!
//! Listing scanners does not feed paper. [`PdfCraftApp::start_scan`] is the only path that calls
//! [`pdfcraft_platform::scan::acquire`], and it does that on a background thread.

use std::sync::{Arc, Mutex};

use std::sync::mpsc::{self, Receiver};

use craft_ui::capability::Capability;
use craft_ui::scanning as shared;
use egui::{Align, Color32, Layout, Pos2, Rect, Sense, Vec2};
use pdfcraft_engine::DocId;
use pdfcraft_platform::scan::{
    self, CancelToken, Color, Connection, HandlingStatus, ImageChoices, Intent, InterruptKind, Page, PageSize, ProgressToken, Region, Rotation,
    ScanSettings, Scanner, ScannerRef, Source, ValueSpan, estimated_bmp_bytes, interrupt_kind, preview_dpi, region_from_inches, region_from_preview,
    region_pixels,
};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// What the dialog's buttons asked for this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanClick {
    None,
    Close,
    /// Stop an in-flight transfer without closing the dialog.
    Stop,
    Preview,
    Create,
    Insert,
    /// Second pass of manual duplex: reverse sides after the stack was flipped.
    ReverseSides,
    /// After a scan: the next feeder load (or bed page) as more fronts, after the last one.
    MoreFronts,
}

#[derive(Clone, Debug)]
pub struct PreviewImage {
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
    pub rgba: Vec<u8>,
}

/// Choices the dialog keeps. Auto (feeder if a sheet is waiting, else the bed) is the default.
#[derive(Clone, Debug)]
pub struct ScanDraft {
    pub device: String,
    pub source: Source,
    pub dpi: u32,
    pub dpi_y: Option<u32>,
    pub color: Color,
    pub pages: u32,
    pub brightness: Option<i32>,
    pub contrast: Option<i32>,
    pub threshold: Option<i32>,
    pub rotation: Option<Rotation>,
    pub format: Option<String>,
    pub compression: Option<String>,
    pub intent: Option<Intent>,
    pub page_size: Option<PageSize>,
    /// How the page length is decided (feeder only): the driver's own, the chosen page size, the scanner's
    /// detection, or a guess from the image. See [`final_settings`] for the mapping to the engine.
    pub page_length: shared::PageLength,
    pub auto_deskew: Option<bool>,
    pub orientation: Option<i32>,
    pub photometric: Option<i32>,
    pub invert: Option<bool>,
    pub mirror: Option<bool>,
    pub lamp: Option<bool>,
    pub duplex: bool,
    /// Two-pass reverse-side scan when the feeder has no hardware duplex.
    pub manual_duplex: bool,
    /// Create PDF: `0` is one document; `N` opens a new document every `N` finished pages.
    pub pages_per_file: u32,
    pub left_in: f64,
    pub top_in: f64,
    pub width_in: f64,
    pub height_in: f64,
    pub preview: Option<PreviewImage>,
    /// Drag rectangle in preview pixels. The far edge is exclusive. `None` is the full area.
    pub selection: Option<(u32, u32, u32, u32)>,
    anchor: Option<(u32, u32)>,
    pub error: Option<String>,
    snapped_device: String,
    snapped_source: Source,
    /// Last bed/feeder [`effective_source`] used for snapping choices (never Auto after a snap).
    snapped_resolved: Source,
}

impl Default for ScanDraft {
    fn default() -> Self {
        Self {
            device: String::new(),
            source: Source::Auto,
            dpi: 300,
            dpi_y: None,
            color: Color::Gray,
            // Whole feeder (0). The bed still takes one page in final_settings.
            pages: 0,
            brightness: None,
            contrast: None,
            threshold: None,
            rotation: None,
            format: None,
            compression: None,
            intent: None,
            page_size: None,
            page_length: shared::PageLength::Driver,
            auto_deskew: None,
            orientation: None,
            photometric: None,
            invert: None,
            mirror: None,
            lamp: None,
            duplex: false,
            manual_duplex: false,
            pages_per_file: 0,
            left_in: 0.0,
            top_in: 0.0,
            width_in: 0.0,
            height_in: 0.0,
            preview: None,
            selection: None,
            anchor: None,
            error: None,
            snapped_device: String::new(),
            snapped_source: Source::Auto,
            snapped_resolved: Source::Auto,
        }
    }
}

impl ScanDraft {
    /// The chosen page size names a length (A4, Letter, Legal, business card). `Auto` and `Custom` do not.
    pub fn page_size_names_a_length(&self) -> bool {
        self.page_size.is_some_and(|size| size.extent_mils().is_some())
    }

    /// Bed or feeder. The preview belongs to the previous source, so it goes away.
    pub fn set_source(&mut self, source: Source) {
        if self.source == source {
            return;
        }
        self.source = source;
        // Feeder default is the whole stack; the bed is always one page at acquire time.
        self.pages = if source == Source::Glass { 1 } else { 0 };
        self.clear_preview();
        self.brightness = None;
        self.contrast = None;
        self.threshold = None;
        self.rotation = None;
        self.format = None;
        self.compression = None;
        self.intent = None;
        self.page_size = None;
        self.page_length = shared::PageLength::Driver;
        self.auto_deskew = None;
        self.orientation = None;
        self.photometric = None;
        self.invert = None;
        self.mirror = None;
        self.lamp = None;
        self.dpi_y = None;
        self.snapped_device.clear();
    }

    /// Drop the preview image and the region that was chosen on it.
    pub fn clear_preview(&mut self) {
        self.preview = None;
        self.clear_region();
    }

    /// Drop the drag and the inch fields. The preview stays; the next scan covers the whole area.
    pub fn clear_region(&mut self) {
        self.selection = None;
        self.anchor = None;
        self.left_in = 0.0;
        self.top_in = 0.0;
        self.width_in = 0.0;
        self.height_in = 0.0;
    }

    /// 300 dpi gray with text intent and neutral brightness and contrast, when offered.
    pub fn apply_text_ocr(&mut self, choices: &ImageChoices) {
        self.apply_preset(choices, Color::Gray, preferred_dpi(&choices.dpi), &[Intent::Text, Intent::Gray]);
    }

    /// 300 dpi colour with colour intent — mixed text and pictures on a page.
    pub fn apply_color_document(&mut self, choices: &ImageChoices) {
        self.apply_preset(choices, Color::Rgb, preferred_dpi(&choices.dpi), &[Intent::Color, Intent::MaximizeQuality]);
    }

    /// Higher-resolution colour for photographs, when the driver lists it.
    pub fn apply_photo(&mut self, choices: &ImageChoices) {
        self.apply_preset(choices, Color::Rgb, photo_dpi(&choices.dpi), &[Intent::MaximizeQuality, Intent::Color]);
    }

    /// 300 dpi black-and-white with text intent — line art and printed text.
    pub fn apply_black_and_white(&mut self, choices: &ImageChoices) {
        self.apply_preset(choices, Color::BlackAndWhite, preferred_dpi(&choices.dpi), &[Intent::Text, Intent::Gray]);
        if let Some(span) = choices.threshold {
            self.threshold = Some(if span.contains(128) { 128 } else { span.min });
        }
    }

    /// Set colour, resolution, intent, brightness, contrast and rotation to the preset's values where the scanner
    /// offers them (the nearest it does offer otherwise), and clear the region.
    fn apply_preset(&mut self, choices: &ImageChoices, color: Color, dpi: Option<u32>, intents: &[Intent]) {
        if let Some(dpi) = dpi {
            self.dpi = dpi;
        }
        if choices.colors.contains(&color) {
            self.color = color;
        } else if let Some(fallback) = choices.colors.first().copied() {
            self.color = fallback;
        }
        self.intent = intents.iter().copied().find(|intent| choices.intents.contains(intent));
        self.brightness = choices.brightness.map(|span| if span.contains(0) { 0 } else { span.min });
        self.contrast = choices.contrast.map(|span| if span.contains(0) { 0 } else { span.min });
        self.threshold = None;
        self.rotation = if choices.rotation.contains(&Rotation::Portrait) { Some(Rotation::Portrait) } else { choices.rotation.first().copied() };
        self.clear_region();
    }

    /// The window to scan, in thousandths of an inch. A drag wins over the inch fields.
    /// An empty selection is the full area.
    pub fn region(&self) -> Option<Region> {
        if let (Some(preview), Some((x0, y0, x1, y1))) = (&self.preview, self.selection) {
            return region_from_preview(x0, y0, x1, y1, preview.dpi).ok().flatten();
        }
        if self.width_in <= 0.0 || self.height_in <= 0.0 {
            return None;
        }
        region_from_inches(self.left_in, self.top_in, self.width_in, self.height_in).ok()
    }

    /// Keep a drag as the selection (ignoring one under 2 pixels either way) and copy its region into the inch
    /// fields; with no selection the fields are cleared.
    fn remember_selection(&mut self, selection: Option<(u32, u32, u32, u32)>) {
        self.selection = selection.filter(|(x0, y0, x1, y1)| x1.saturating_sub(*x0) >= 2 && y1.saturating_sub(*y0) >= 2);
        if let Some(region) = self.region() {
            self.left_in = f64::from(region.left) / 1000.0;
            self.top_in = f64::from(region.top) / 1000.0;
            self.width_in = f64::from(region.width) / 1000.0;
            self.height_in = f64::from(region.height) / 1000.0;
        } else if self.selection.is_none() {
            self.left_in = 0.0;
            self.top_in = 0.0;
            self.width_in = 0.0;
            self.height_in = 0.0;
        }
    }
}

/// The shared scanner picker plus what only this app needs from the driver: the full capabilities of
/// the selected scanner, which the preview, region, advanced options and page handling read.
#[derive(Default)]
pub struct Devices {
    state: shared::State,
    /// Written by the details worker just before it answers, read once the shared state is `Ready`.
    details: Arc<Mutex<Option<Scanner>>>,
}

impl Devices {
    /// The two workers the shared component runs. Listing opens nothing; details are read for the
    /// selected scanner only. Both run on their own thread.
    fn workers(&self) -> (shared::Discover, shared::LoadChoices) {
        let discover: shared::Discover =
            Box::new(|| worker("pdfcraft-scan-list", || scan::scanner_list().map_err(|error| error.to_string()).and_then(|list| list_result(&list))));
        let known = Arc::clone(&self.details);
        let load: shared::LoadChoices = Box::new(move |id| {
            let id = id.to_string();
            let known = Arc::clone(&known);
            worker("pdfcraft-scan-details", move || {
                let scanner = scan::scanner_details(&id).map_err(|error| error.to_string())?;
                let choices = choices_of(&scanner);
                if let Ok(mut slot) = known.lock() {
                    *slot = Some(scanner);
                }
                Ok(choices)
            })
        });
        (discover, load)
    }

    /// Whether the scanner list is being read.
    pub fn loading(&self) -> bool {
        self.state.loading()
    }

    /// Whether the selected scanner's details are being read.
    pub fn checking(&self) -> bool {
        self.state.checking()
    }

    /// What the driver really reported for `id`; `None` while it is being read or when it could not be.
    fn scanner_read(&self, id: &str) -> Option<Scanner> {
        if self.state.settings.scanner != id || !matches!(self.state.details(), shared::DetailsState::Ready(_)) || self.state.choices().is_none() {
            return None;
        }
        self.details.lock().ok()?.clone().filter(|scanner| scanner.id == id)
    }

    /// The scanner to draw and scan with: the real details, or conservative defaults when they could
    /// not be read (bed and feeder both offered, no ranges, no duplex). `None` while the details are
    /// still loading, so nothing is scanned with a guess that is about to be replaced.
    pub fn scanner_selected(&self, id: &str) -> Option<Scanner> {
        if let Some(scanner) = self.scanner_read(id) {
            return Some(scanner);
        }
        let item = self.state.selected().filter(|item| item.id == id)?;
        (self.state.choices().is_some() && self.state.details_error().is_some()).then(|| fallback_scanner(item))
    }
}

/// Run `work` on a named thread and hand back its answer. A thread that cannot start answers with an
/// error, so the shared state never waits forever.
fn worker<T: Send + 'static>(name: &str, work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Receiver<Result<T, String>> {
    let (tx, rx) = mpsc::channel();
    let failed = tx.clone();
    let started = spawn_named(name, move || {
        let _ = tx.send(work());
    });
    if !started {
        let _ = failed.send(Err(tl!("The scanner list could not be refreshed.").to_string()));
    }
    rx
}

/// The devices of a listing. When no device was found and a backend failed, say why.
fn list_result(list: &scan::Listing<ScannerRef>) -> Result<Vec<shared::Scanner>, String> {
    match (list.items.is_empty(), list.errors.first()) {
        (true, Some(failure)) => Err(failure.error.to_string()),
        _ => Ok(scanners_of(&list.items)),
    }
}

/// The scanner list in the shared component's form, with the connection kind of each device.
fn scanners_of(list: &[ScannerRef]) -> Vec<shared::Scanner> {
    list.iter()
        .map(|item| shared::Scanner {
            id: item.id.clone(),
            name: item.name.clone(),
            connection: match item.connection {
                Connection::Usb => shared::Connection::Usb,
                Connection::Wsd => shared::Connection::Network,
                Connection::Unknown | Connection::Other => shared::Connection::Unknown,
            },
        })
        .collect()
}

/// Conservative scanner for a device whose details could not be read: bed and feeder offered, no duplex and no
/// ranges.
fn fallback_scanner(item: &shared::Scanner) -> Scanner {
    Scanner {
        id: item.id.clone(),
        name: item.name.clone(),
        connection: match item.connection {
            shared::Connection::Usb => Connection::Usb,
            shared::Connection::Network => Connection::Wsd,
            shared::Connection::Unknown => Connection::Unknown,
        },
        port: None,
        flatbed: true,
        feeder: true,
        duplex: false,
        feeder_ready: None,
        handling_status: HandlingStatus::default(),
        feeder_capacity: None,
        optical_x: None,
        optical_y: None,
        pages: None,
        bed: None,
        sheet: None,
        glass: None,
        feeder_choices: None,
        reported: Default::default(),
    }
}

/// The shared component's source for the engine's source.
fn shared_source(source: Source) -> shared::Source {
    match source {
        Source::Auto => shared::Source::Auto,
        Source::Glass => shared::Source::Glass,
        Source::Feeder => shared::Source::Feeder,
    }
}

/// The engine's source for the shared component's source.
fn source_of(source: shared::Source) -> Source {
    match source {
        shared::Source::Auto => Source::Auto,
        shared::Source::Glass => Source::Glass,
        shared::Source::Feeder => Source::Feeder,
    }
}

/// The shared component's colour mode for the engine's.
fn shared_color(color: Color) -> shared::Color {
    match color {
        Color::Gray => shared::Color::Gray,
        Color::Rgb => shared::Color::Color,
        Color::BlackAndWhite => shared::Color::BlackAndWhite,
    }
}

/// The engine's colour mode for the shared component's.
fn color_of(color: shared::Color) -> Color {
    match color {
        shared::Color::Gray => Color::Gray,
        shared::Color::Color => Color::Rgb,
        shared::Color::BlackAndWhite => Color::BlackAndWhite,
    }
}

/// What the shared component needs to know of a scanner. It has one list per setting, while the
/// driver lists them per source, so resolutions and colours are the union of the bed's and the
/// feeder's; [`snap_choices`] narrows them to the source in use.
///
/// "All pages" is always offered: the engine treats a feeder page count of `0` as "until empty"
/// whether or not the driver's range lists it.
#[allow(clippy::field_reassign_with_default)] // `Choices` is non_exhaustive: it is built from its default
fn choices_of(scanner: &Scanner) -> shared::Choices {
    let read = units_read(scanner);
    let glass = scanner.flatbed || scanner.glass.is_some();
    let feeder = scanner.feeder || scanner.feeder_choices.is_some();
    let lists: Vec<&ImageChoices> = [scanner.glass.as_ref(), scanner.feeder_choices.as_ref()].into_iter().flatten().collect();
    let mut dpi: Vec<u32> = Vec::new();
    let mut colors: Vec<shared::Color> = Vec::new();
    for list in &lists {
        for value in &list.dpi {
            if !dpi.contains(value) {
                dpi.push(*value);
            }
        }
        for color in &list.colors {
            let color = shared_color(*color);
            if !colors.contains(&color) {
                colors.push(color);
            }
        }
    }
    if lists.len() > 1 {
        dpi.sort_unstable();
    }
    let (pages, _) = page_values(scanner.pages);
    let mut choices = shared::Choices::default();
    choices.feeder = shared::presence(unit_answer(feeder, read));
    choices.glass = shared::presence(unit_answer(glass, read));
    choices.duplex = shared::presence(unit_answer(scanner.duplex, read));
    // craft-scan keeps a missing list and an empty one apart nowhere, so a list can only be
    // supported (it has values) or unknown (it does not); never unsupported.
    choices.dpi = Capability::from_option_list((!dpi.is_empty()).then_some(dpi.as_slice()));
    choices.colors = Capability::from_option_list((!colors.is_empty()).then_some(colors.as_slice()));
    choices.max_pages = Capability::from_option_range(scanner.pages.and(pages.last().copied()).map(|max| (1, max)));
    choices.all_pages = shared::presence(Some(true));
    // Only the feeder row shows page length, so the feeder item's answer is the one that counts. A scanner whose
    // properties were read without detection is "absent" (disabled with a reason); one that was not read is unknown.
    choices.detect_length = match scanner.length_support(Source::Feeder).device_detect {
        scan::Presence::Present => shared::presence(Some(true)),
        scan::Presence::Absent => shared::presence(Some(false)),
        scan::Presence::Unknown => shared::presence(None),
    };
    choices
}

/// Whether the driver's document-handling bits were evidently read. craft-scan reports a failed read as
/// all-false, the same as a scanner with no units, so "nothing at all is present" is taken as "not read".
fn units_read(scanner: &Scanner) -> bool {
    scanner.feeder || scanner.flatbed || scanner.glass.is_some() || scanner.feeder_choices.is_some()
}

/// A unit the driver lists is present; one it does not list is absent only when the bits were read.
fn unit_answer(present: bool, read: bool) -> Option<bool> {
    if present {
        Some(true)
    } else if read {
        Some(false)
    } else {
        None
    }
}

/// Device status the dialog warns about. It never disables a control.
fn warnings_of(scanner: &Scanner, source: Source, after_empty_feeder: bool) -> Vec<shared::Warning> {
    let mut out = Vec::new();
    let feeder = scanner.feeder || scanner.feeder_choices.is_some();
    if feeder && source == Source::Feeder && scanner.feeder_ready == Some(false) {
        out.push(shared::Warning::FeederEmpty);
    }
    if scanner.handling_status.cover_open || scanner.handling_status.path_cover_open {
        out.push(shared::Warning::CoverOpen);
    }
    if feeder && scanner.feeder_ready == Some(true) && after_empty_feeder {
        out.push(shared::Warning::FeederReadyButEmpty);
    }
    out
}

/// Put the dialog's choices where the shared component reads them. The preset it last applied stays.
fn push_settings(draft: &ScanDraft, settings: &mut shared::Settings) {
    settings.scanner.clone_from(&draft.device);
    settings.source = shared_source(draft.source);
    settings.color = shared_color(draft.color);
    settings.dpi = draft.dpi;
    settings.pages = if draft.pages == 0 { shared::Pages::All } else { shared::Pages::Count(draft.pages) };
    settings.duplex = draft.duplex;
    settings.page_length = draft.page_length;
    settings.page_size_chosen = draft.page_size_names_a_length();
}

/// Take back what the shared component changed. A new scanner or source drops the preview, as it
/// always did. True when the scanner or source changed, which makes an open advanced edit stale.
fn pull_settings(draft: &mut ScanDraft, settings: &shared::Settings) -> bool {
    let mut reset = false;
    if settings.scanner != draft.device {
        draft.device.clone_from(&settings.scanner);
        draft.clear_preview();
        draft.snapped_device.clear();
        reset = true;
    }
    let source = source_of(settings.source);
    if source != draft.source {
        // Also restores the page count that belongs to the source.
        draft.set_source(source);
        reset = true;
    } else {
        draft.pages = match settings.pages {
            shared::Pages::All => 0,
            shared::Pages::Count(n) => n,
        };
    }
    draft.dpi = settings.dpi;
    draft.color = color_of(settings.color);
    draft.duplex = settings.duplex;
    draft.page_length = settings.page_length;
    reset
}

/// The app's own words for the shared component's labels.
fn labels() -> shared::Labels<'static> {
    shared::Labels {
        scanner: tl!("Scanner"),
        select_scanner: "—",
        refresh: tl!("Refresh"),
        refresh_hint: tl!("Refresh lists scanners. It does not feed paper."),
        source: tl!("Source"),
        source_auto: tl!("Auto"),
        source_feeder: tl!("Feeder"),
        source_glass: tl!("Bed"),
        color: tl!("Color"),
        color_gray: tl!("Gray"),
        color_color: tl!("Color"),
        color_black_and_white: tl!("Black and white"),
        resolution: tl!("Resolution"),
        dpi: "dpi",
        pages: tl!("Pages"),
        all_pages: tl!("All pages"),
        duplex: tl!("Duplex"),
        duplex_both_sides: tl!("Both sides"),
        usb: tl!("USB"),
        network: tl!("Network"),
        checking: tl!("Checking the scanner…"),
        try_again: tl!("Try again"),
        preset_text: tl!("Text"),
        preset_color_document: tl!("Color document"),
        preset_black_and_white: tl!("Black and white"),
        preset_photo: tl!("Photo"),
        also_usb: tl!("The same printer is also available over USB."),
        also_network: tl!("The same printer is also available over the network."),
        // English only in this test build: plain literals keep the catalog coverage tests untouched.
        unsupported_reason: "This scanner does not offer this.",
        unconfirmed_note: "Not confirmed by the scanner.",
        reasons: shared::Reasons {
            feeder: Some("This scanner has no document feeder."),
            glass: Some("This scanner has no flatbed."),
            duplex: Some("This scanner has no duplex unit."),
            pages: Some("The scanner does not let you choose a page count."),
            ..shared::Reasons::default()
        },
        length: shared::LengthLabels {
            heading: "Page length",
            driver: "Driver default",
            match_size: "Match page size",
            detect: "Detect (scanner)",
            infer: "Detect from image (approximate)",
            no_page_size: "Choose a page size in Advanced first.",
            no_detection: "This scanner does not detect the page length.",
            infer_note: "Estimated from the scanned image, so it can be wrong. This is not a scanner feature.",
        },
        warning_feeder_empty: "The feeder reports no paper.",
        warning_cover_open: "A cover on the scanner is open.",
        warning_feeder_ready_but_empty: "The feeder reported ready, but the last scan found it empty.",
    }
}

/// Source and size snapping only follow capabilities that were really read.
fn follow_capabilities(app: &mut PdfCraftApp) {
    let Some(scanner) = app.scan.devices.scanner_read(&app.scan.draft.device) else { return };
    let glass = scanner.flatbed || scanner.glass.is_some();
    let feeder = scanner.feeder || scanner.feeder_choices.is_some();
    let before = app.scan.draft.source;
    if app.scan.draft.source == Source::Glass && !glass && feeder {
        app.scan.draft.set_source(Source::Feeder);
    } else if app.scan.draft.source == Source::Feeder && !feeder && glass {
        app.scan.draft.set_source(Source::Glass);
    } else if app.scan.draft.source == Source::Auto && !(glass && feeder) {
        app.scan.draft.set_source(if feeder { Source::Feeder } else { Source::Glass });
    }
    if app.scan.draft.source != before {
        app.scan.advanced = None;
    }
    let choices = source_choices(&scanner, effective_source(&app.scan.draft, &scanner));
    snap_choices(&mut app.scan.draft, &scanner, &choices);
}

/// The device row, the settings and the presets (all from the shared component), then the
/// sentences the shared state asks the app to word. The draft carries the result.
fn devices_block(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, busy: bool) {
    let (discover, load) = app.scan.devices.workers();
    push_settings(&app.scan.draft, &mut app.scan.devices.state.settings);
    let before = app.scan.devices.state.settings.clone();
    let labels = labels();
    let warnings = app
        .scan
        .devices
        .scanner_read(&app.scan.draft.device)
        .map(|scanner| {
            let after_empty =
                app.scan.interrupted.as_ref().is_some_and(|i| matches!(i.kind, InterruptKind::FeederEmpty | InterruptKind::BusyOrFeederEmpty));
            warnings_of(&scanner, effective_source(&app.scan.draft, &scanner), after_empty)
        })
        .unwrap_or_default();
    app.scan.devices.state.set_warnings(warnings);
    let mut response = shared::Response::default();
    ui.add_enabled_ui(!busy, |ui| {
        response = app.scan.devices.state.show(ui, egui::Id::new("scan-devices"), &labels, &discover, &load);
    });
    let after = app.scan.devices.state.settings.clone();
    if pull_settings(&mut app.scan.draft, &after) {
        app.scan.advanced = None;
    }
    if after.preset != before.preset
        && let Some(preset) = after.preset
        && let Some(scanner) = app.scan.devices.scanner_selected(&app.scan.draft.device)
    {
        let choices = source_choices(&scanner, effective_source(&app.scan.draft, &scanner));
        match preset {
            shared::Preset::DocumentText => app.scan.draft.apply_text_ocr(&choices),
            shared::Preset::ColorDocument => app.scan.draft.apply_color_document(&choices),
            shared::Preset::BlackAndWhite => app.scan.draft.apply_black_and_white(&choices),
            shared::Preset::Photo => app.scan.draft.apply_photo(&choices),
        }
    }
    if response.changed {
        follow_capabilities(app);
        ui.ctx().request_repaint();
    }
    let red = Color32::from_rgb(0xB0, 0x30, 0x30);
    match app.scan.devices.state.feedback() {
        Some(shared::Feedback::FindingScanners) => {
            ui.label(egui::RichText::new(tl!("Listing scanners…")).small().color(t.text_muted));
        }
        Some(shared::Feedback::NoScanners) => {
            ui.label(tl!("No scanner is connected."));
        }
        Some(shared::Feedback::DiscoveryFailed(error)) => {
            ui.label(egui::RichText::new(error).color(red));
        }
        Some(shared::Feedback::WorkerStopped) => {
            ui.label(egui::RichText::new(tl!("The scanner list could not be refreshed.")).color(red));
        }
        Some(shared::Feedback::DetailsFailed(error)) => {
            let text = if error.is_empty() { tl!("The scanner list could not be refreshed.").to_string() } else { error };
            ui.label(egui::RichText::new(text).color(red));
        }
        // The shared widget shows its own "checking" line beside the device row.
        Some(shared::Feedback::Checking) | None => {}
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScanKind {
    Preview,
    Create,
    Insert(DocId),
    /// Reverse-side pass; fronts are already held in [`ScanUi::duplex`].
    ReverseSides,
}

/// Fronts of the first manual-duplex pass. They are already in the document (contiguous from
/// `first`); the reverse-side pass inserts each back after its front as it arrives.
#[derive(Clone, Copy, Debug)]
struct DuplexAwait {
    kind: ScanKind,
    /// Bed or feeder used for the fronts; the reverse pass uses the same source.
    source: Source,
    /// Document the fronts went into.
    doc: DocId,
    /// Page index of the first front in `doc`.
    first: usize,
    /// Fronts inserted so far.
    fronts: usize,
    /// Reverse sides inserted so far; fewer than the fronts after an interruption.
    backs: usize,
    /// The person ticked Reverse sides before scanning. Otherwise the fronts are just the pages of the scan that
    /// ended, offered for reverse sides or more pages after the fact; closing the dialog then finishes the scan.
    requested: bool,
    /// The pages came from the scanner's duplex unit and already hold both sides: only more pages or Finish are
    /// offered, never reverse sides.
    two_sided: bool,
}

struct JobSlot {
    /// Set when [`scan::acquire`] returns. Create/Insert pages are applied earlier from [`ScanJob::live`].
    result: Option<Result<Vec<Page>, String>>,
    /// The backend's own cause when the scan failed (a text match is only the fallback).
    error_kind: Option<InterruptKind>,
}

pub struct ScanJob {
    kind: ScanKind,
    /// Document Create opened from the first finished page.
    create_doc: Option<DocId>,
    /// Pages already in [`Self::create_doc`] when splitting Create into multiple files.
    pages_in_create: u32,
    /// Next Create file index (`0` → Scan.pdf, `1` → Scan (2).pdf, …).
    create_file_index: u32,
    /// Create PDF split: `None` keeps one document; `Some(n)` starts a new one every `n` pages.
    pages_per_file: Option<u32>,
    /// Requested page count when known (`0` / whole feeder is unknown).
    expected: Option<u32>,
    /// Source locked for the first (fronts) pass of a manual duplex (never Auto).
    duplex_source: Option<Source>,
    /// Pages scanned for this scan so far, over every Continue.
    received: u32,
    /// Pages wanted in all, over every Continue; `None` for the whole feeder or hardware duplex.
    expected_total: Option<u32>,
    /// Insert: the page index the next scanned page goes to.
    insert_cursor: Option<usize>,
    /// Bed or feeder the job resolved to.
    source: Source,
    /// The source the user asked for (Auto stays Auto; [`Self::source`] is where the job went).
    requested_source: Source,
    /// The scanner's display name, for the Scan interrupted dialog. Never an id, address or port.
    device_name: String,
    /// The scanner's duplex unit scanned both sides in this pass: the pages already alternate front and back.
    two_sided: bool,
    /// Continue scanning: the pages go after the last page of the scan before, wherever the selection is.
    continues: bool,
    live: ProgressToken,
    progress: Arc<Mutex<JobSlot>>,
    pub cancel: CancelToken,
}

/// Working copy while the Advanced scanner settings modal is open.
#[derive(Clone, Debug)]
struct AdvancedEdit {
    draft: ScanDraft,
}

#[derive(Default)]
pub struct ScanUi {
    pub draft: ScanDraft,
    pub devices: Devices,
    pub job: Option<ScanJob>,
    advanced: Option<AdvancedEdit>,
    duplex: Option<DuplexAwait>,
    /// A scan that stopped early; it waits for Continue or Finish ([`crate::Dialog::ScanInterrupted`]).
    pub(crate) interrupted: Option<Interrupted>,
    /// Documents opened by the Create job that just finished (for the toast).
    last_create_files: u32,
    /// How the last scanned page got its length, for the status line and the toast.
    last_length: Option<scan::LengthRecord>,
}

/// Which pass of a scan an interruption cut short.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pass {
    /// Pages go straight into the document.
    Direct,
    /// Manual duplex, first pass: the fronts go into the document as they arrive.
    Fronts,
    /// Manual duplex, second pass: each back goes in after its front.
    Backs,
}

/// What the user can choose in the Scan interrupted dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InterruptAction {
    /// Scan on from where the job stopped.
    Continue,
    /// Nothing was scanned yet: start again with the same settings.
    TryAgain,
    /// Manual duplex: go to the flip step with the fronts scanned so far.
    ReverseNow,
    /// Keep what was scanned and close.
    Finish,
}

/// A scan that stopped early (cover open, jam, empty feeder, offline). It keeps what a Continue
/// needs: which document, where the next page goes, and how many pages are still wanted.
#[derive(Clone, Debug)]
pub(crate) struct Interrupted {
    kind: InterruptKind,
    /// The engine's own sentence.
    message: String,
    /// Pages scanned so far in this scan, over every Continue.
    received: u32,
    /// Pages wanted in all; `None` when the whole feeder was asked for.
    expected: Option<u32>,
    scan_kind: ScanKind,
    pass: Pass,
    /// Create: the document the first pages went into.
    doc: Option<DocId>,
    pages_in_create: u32,
    create_file_index: u32,
    pages_per_file: Option<u32>,
    /// Insert: the page index the next scanned page goes to.
    insert_at: Option<usize>,
    /// Bed or feeder the job used.
    source: Source,
    /// The source the user asked for; Auto when the job resolved it.
    requested_source: Source,
    /// Which scan Auto resolved to, when the scanner reported it before the stop.
    resolved_source: Option<Source>,
    /// The scanner's display name. Never an id, address or port.
    device_name: String,
}

/// What a Continue hands to the next acquire.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Resume {
    scan_kind: ScanKind,
    /// Pages still wanted when a fixed count was asked for.
    pages: Option<u32>,
    /// Pages already scanned before this leg.
    carried: u32,
    expected_total: Option<u32>,
    create_doc: Option<DocId>,
    pages_in_create: u32,
    create_file_index: u32,
    pages_per_file: Option<u32>,
    insert_at: Option<usize>,
    /// Source to keep when pages already came in (Auto would pick the bed once the tray is empty).
    source: Option<Source>,
    feeder: bool,
}

impl Interrupted {
    /// "Fake WSD scanner (ADF tray 5, letter) · Auto → glass": which scanner and which source. The
    /// display name only; no id, address or port reaches this line.
    fn device_line(&self) -> String {
        let source = match (self.requested_source, self.resolved_source) {
            (Source::Auto, Some(Source::Glass)) => "Auto → glass",
            (Source::Auto, Some(Source::Feeder)) => "Auto → feeder",
            (Source::Auto, _) => "Auto",
            (Source::Feeder, _) => "Feeder",
            (Source::Glass, _) => "Glass",
        };
        let name = self.device_name.trim();
        if name.is_empty() { source.to_string() } else { format!("{name} · {source}") }
    }

    /// Pages still wanted: expected minus received, for a fixed count.
    fn remaining(&self) -> Option<u32> {
        self.expected.map(|expected| expected.saturating_sub(self.received))
    }

    /// Whether any page of this scan was received.
    fn has_pages(&self) -> bool {
        self.received > 0
    }

    /// The engine's rule for what can be done about this stop (`pdfcraft_scan::recovery`).
    fn recovery(&self) -> scan::Recovery {
        scan::recovery(usize::try_from(self.received).unwrap_or(usize::MAX), self.expected)
    }

    /// False once every wanted page is in: there is nothing left to continue with.
    #[cfg(test)]
    fn can_continue(&self) -> bool {
        let recovery = self.recovery();
        recovery.retryable || recovery.resumable
    }

    /// The primary action: Try again when nothing was scanned, otherwise Continue.
    fn primary(&self) -> Option<InterruptAction> {
        let recovery = self.recovery();
        if recovery.resumable {
            Some(InterruptAction::Continue)
        } else if recovery.retryable {
            Some(InterruptAction::TryAgain)
        } else {
            None
        }
    }

    /// The buttons, primary first.
    fn actions(&self) -> Vec<InterruptAction> {
        let mut actions: Vec<InterruptAction> = self.primary().into_iter().collect();
        if self.pass == Pass::Fronts && self.has_pages() {
            actions.push(InterruptAction::ReverseNow);
        }
        actions.push(InterruptAction::Finish);
        actions
    }

    /// The button that starts the next leg of this interrupted scan: the reverse-sides pass, an insert, or a
    /// create.
    fn click(&self) -> ScanClick {
        match (self.pass, self.scan_kind) {
            (Pass::Backs, _) => ScanClick::ReverseSides,
            (_, ScanKind::Insert(_)) => ScanClick::Insert,
            _ => ScanClick::Create,
        }
    }

    /// What the next leg carries over: the scan kind, the pages still wanted, the pages already kept and where they
    /// go.
    fn resume(&self) -> Resume {
        Resume {
            scan_kind: self.scan_kind,
            pages: self.remaining().filter(|left| *left > 0),
            carried: self.received,
            expected_total: self.expected,
            create_doc: self.doc,
            pages_in_create: self.pages_in_create,
            create_file_index: self.create_file_index,
            pages_per_file: self.pages_per_file,
            insert_at: self.insert_at,
            source: (self.received > 0 && self.source != Source::Auto).then_some(self.source),
            feeder: self.source == Source::Feeder,
        }
    }

    /// The sentence for the cause of the stop.
    fn headline(&self) -> &'static str {
        match self.kind {
            InterruptKind::CoverOpen => tl!("A cover on the scanner is open."),
            InterruptKind::Jam => tl!("The scanner has a paper jam."),
            InterruptKind::PaperProblem => tl!("The scanner reported a paper problem (a jam or several sheets fed together)."),
            InterruptKind::FeederEmpty => tl!("The feeder ran out of paper."),
            InterruptKind::MultiFeed => tl!("The feeder took more than one sheet."),
            InterruptKind::Offline => tl!("The scanner is offline or needs attention."),
            InterruptKind::Attention => tl!("The scanner needs attention."),
            InterruptKind::Busy => tl!("The scanner is busy."),
            InterruptKind::BusyOrFeederEmpty => tl!("The scanner is busy, or the feeder has no paper."),
            InterruptKind::DeviceError => tl!("The scanner reported a lamp or device error."),
            InterruptKind::Cancelled => tl!("The scan was cancelled at the scanner."),
            // `Other`, and any cause a newer craft-scan adds (the enum is non-exhaustive).
            _ => tl!("The scan was interrupted."),
        }
    }

    /// "2 of 3 pages were scanned." (a fixed count) or "2 pages were scanned." (all pages).
    fn count_line(&self) -> String {
        let n = self.received.to_string();
        match (self.received, self.expected) {
            (0, _) => tl!("No pages were scanned.").to_string(),
            (_, Some(m)) => crate::i18n::fmt(tl!("{n} of {m} pages were scanned."), &[("n", &n), ("m", &m.to_string())]),
            (1, None) => tl!("1 page was scanned.").to_string(),
            (_, None) => crate::i18n::fmt(tl!("{n} pages were scanned."), &[("n", &n)]),
        }
    }

    /// Where the pages already scanned are.
    fn kept_line(&self) -> Option<&'static str> {
        if !self.has_pages() {
            return None;
        }
        Some(match self.pass {
            Pass::Direct => tl!("The pages already scanned were kept in the document."),
            Pass::Fronts => tl!("The fronts already scanned are kept for the reverse-side pass."),
            Pass::Backs => tl!("The reverse sides already scanned are kept."),
        })
    }

    /// The one instruction: kind-specific, or the engine's sentence when it named nothing known.
    fn instruction_text(&self) -> String {
        let retry = self.primary() == Some(InterruptAction::TryAgain);
        match instruction(self.kind, retry) {
            Some(text) => text.to_string(),
            None if self.pass == Pass::Backs && self.has_pages() => {
                format!("{} {}", self.message, tl!("Scan the missing reverse sides, then choose Finish."))
            }
            None => self.message.clone(),
        }
    }

    /// The label of the primary button, by action and pass.
    fn primary_label(&self) -> &'static str {
        match (self.primary(), self.pass) {
            (Some(InterruptAction::TryAgain), _) => tl!("Try again"),
            (_, Pass::Direct) => tl!("Continue scanning"),
            (_, Pass::Fronts) => tl!("Continue scanning fronts"),
            (_, Pass::Backs) => tl!("Continue scanning reverse sides"),
        }
    }

    /// The label of the button that scans the reverse sides now, with the number of fronts scanned.
    fn reverse_now_label(&self) -> String {
        if self.received == 1 {
            tl!("Scan reverse side now (1 front)").to_string()
        } else {
            crate::i18n::fmt(tl!("Scan reverse sides now ({n} fronts)"), &[("n", &self.received.to_string())])
        }
    }
}

/// The instruction for a cause. `retry` is true when nothing was scanned, so the button reads
/// Try again. `None` for a cause the engine did not name: the dialog shows the engine's sentence.
fn instruction(kind: InterruptKind, retry: bool) -> Option<&'static str> {
    Some(match (kind, retry) {
        (InterruptKind::CoverOpen, false) => tl!("Close the feeder cover and check the paper, then choose Continue."),
        (InterruptKind::CoverOpen, true) => tl!("Close the feeder cover and check the paper, then choose Try again."),
        (InterruptKind::Jam, false) => tl!("Clear the jam and load the remaining sheets squarely, then choose Continue."),
        (InterruptKind::Jam, true) => tl!("Clear the jam and load the sheets squarely, then choose Try again."),
        (InterruptKind::FeederEmpty, false) => tl!("Load more sheets, then choose Continue; or choose Finish to keep what was scanned."),
        (InterruptKind::FeederEmpty, true) => tl!("Load sheets in the feeder, then choose Try again."),
        (InterruptKind::MultiFeed, false) => tl!("Separate the sheets and load the remaining ones, then choose Continue."),
        (InterruptKind::MultiFeed, true) => tl!("Separate the sheets and load them again, then choose Try again."),
        (InterruptKind::PaperProblem, false) => {
            tl!("Open the cover, clear any jam or sheets fed together, load the remaining sheets squarely, then choose Continue.")
        }
        (InterruptKind::PaperProblem, true) => {
            tl!("Open the cover, clear any jam or sheets fed together, load the sheets squarely, then choose Try again.")
        }
        (InterruptKind::Offline, false) => tl!("Check the scanner is on and connected, then choose Continue."),
        (InterruptKind::Offline, true) => tl!("Check the scanner is on and connected, then choose Try again."),
        (InterruptKind::Attention, false) => {
            tl!("Look at the scanner for a message or an open door or cover, fix what it shows, then choose Continue.")
        }
        (InterruptKind::Attention, true) => {
            tl!("Look at the scanner for a message or an open door or cover, fix what it shows, then choose Try again.")
        }
        (InterruptKind::Busy, false) => tl!("Wait for the scanner to finish, or close any other program using it, then choose Continue."),
        (InterruptKind::Busy, true) => tl!("Wait for the scanner to finish, or close any other program using it, then choose Try again."),
        (InterruptKind::BusyOrFeederEmpty, false) => {
            tl!("Check that the feeder has paper, or wait for the scanner to finish, then choose Continue.")
        }
        (InterruptKind::BusyOrFeederEmpty, true) => {
            tl!("Check that the feeder has paper, or wait for the scanner to finish, then choose Try again.")
        }
        (InterruptKind::DeviceError, false) => tl!("Switch the scanner off and on again, wait until it is ready, then choose Continue."),
        (InterruptKind::DeviceError, true) => tl!("Switch the scanner off and on again, wait until it is ready, then choose Try again."),
        // `Other`, `Cancelled` and any cause a newer craft-scan adds: the dialog shows the engine's sentence.
        _ => return None,
    })
}

/// Reverse sides still to scan: one per front, less those already in hand (at least one).
fn reverse_pages(fronts: usize, backs: usize) -> u32 {
    u32::try_from(fronts.saturating_sub(backs)).unwrap_or(u32::MAX).max(1)
}

/// Where the `back_index`-th reverse side scanned goes, as a page index, when the `n_fronts` fronts
/// sit contiguously from `first` and every back arriving so far has been inserted. A flipped stack
/// (`reversed`, the normal case) delivers the back of the LAST front first, so each back lands right
/// after its front and nothing already placed moves it. In scan order, earlier backs shift it down.
/// `None` when there is no front left for it.
fn back_position(first: usize, n_fronts: usize, back_index: usize, reversed: bool) -> Option<usize> {
    if back_index >= n_fronts {
        return None;
    }
    let (front, placed_before) = if reversed { (n_fronts - 1 - back_index, 0) } else { (back_index, back_index) };
    Some(first.saturating_add(front).saturating_add(1).saturating_add(placed_before))
}

impl ScanJob {
    /// Follow the source the engine reports it is transferring from. Auto is a guess at start;
    /// when the feeder turns out empty the engine scans the bed instead, one page, so the job's
    /// source, the manual-duplex source and the page counts are corrected to match.
    fn adopt_source(&mut self, used: Source) {
        if used == Source::Auto || used == self.source {
            return;
        }
        let was_feeder = self.source == Source::Feeder;
        self.source = used;
        if self.duplex_source.is_some() {
            self.duplex_source = Some(used);
        }
        if was_feeder && used == Source::Glass {
            self.expected = Some(1);
            self.expected_total = Some(self.received.saturating_add(1));
        }
    }

    /// The pass this job is in: reverse sides, fronts of a manual-duplex scan, or a direct scan.
    fn pass(&self) -> Pass {
        if matches!(self.kind, ScanKind::ReverseSides) {
            Pass::Backs
        } else if self.duplex_source.is_some() {
            Pass::Fronts
        } else {
            Pass::Direct
        }
    }

    /// What this job stopped with, for the dialog and for a Continue.
    #[cfg(test)]
    fn interruption(&self, message: &str) -> Interrupted {
        self.interruption_of(message, None)
    }

    /// As [`ScanJob::interruption`], with the cause the backend reported when there is one.
    fn interruption_of(&self, message: &str, cause: Option<InterruptKind>) -> Interrupted {
        Interrupted {
            kind: cause.unwrap_or_else(|| interrupt_kind(message)),
            message: message.to_string(),
            received: self.received,
            expected: self.expected_total,
            scan_kind: self.kind,
            pass: self.pass(),
            doc: self.create_doc,
            pages_in_create: self.pages_in_create,
            create_file_index: self.create_file_index,
            pages_per_file: self.pages_per_file,
            insert_at: self.insert_cursor,
            source: self.source,
            requested_source: self.requested_source,
            resolved_source: self.live.source(),
            device_name: self.device_name.clone(),
        }
    }
}

/// What the scanner dialog shows instead of its controls where there is no scanner backend
/// (`None` when scanning is available). Scanning is Windows/WIA only in this version; other
/// platforms build the same dialog and answer here, at run time, without listing devices.
fn unavailable_notice(supported: bool) -> Option<&'static str> {
    (!supported).then(|| tl!("Scanning needs Windows (WIA) in this version."))
}

/// The scanner dialog's content. Returns the button the user pressed; where scanning is not available it shows
/// a notice and only offers Close.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> ScanClick {
    if let Some(notice) = unavailable_notice(scan::supported()) {
        ui.label(egui::RichText::new(tl!("From scanner…")).font(theme::semibold(18.0)));
        ui.add_space(6.0);
        ui.label(egui::RichText::new(notice).color(t.text_muted));
        ui.add_space(10.0);
        let mut click = ScanClick::None;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Close"), true).clicked() {
                click = ScanClick::Close;
            }
        });
        return click;
    }
    let busy = app.scan.job.is_some();
    let has_doc = app.active.is_some();
    follow_capabilities(app);
    if busy {
        app.scan.advanced = None;
    }

    // Nested settings dialog: edits a copy; Done merges advanced knobs back.
    if app.scan.advanced.is_some() {
        advanced_modal(ui.ctx(), app, t, busy);
    }

    ui.label(egui::RichText::new(tl!("From scanner…")).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    let mut click = ScanClick::None;
    if !busy && let Some(pending) = app.scan.duplex.as_ref() {
        // Persistent: the fronts are done (or stopped early) and the stack has to be turned over.
        let fronts = pending.fronts.to_string();
        let text = match pending.source {
            // Both sides are in already: say how many pages, nothing to turn over.
            _ if pending.two_sided => crate::i18n::fmt(tl!("{n} pages were scanned."), &[("n", &fronts)]),
            Source::Glass => {
                crate::i18n::fmt(tl!("Fronts scanned: {n}. Flip the sheet on the bed, then choose Scan reverse sides."), &[("n", &fronts)])
            }
            _ => crate::i18n::fmt(
                tl!("Fronts scanned: {n}. Flip the stack so the last sheet is on top, reload it, then choose Scan reverse sides."),
                &[("n", &fronts)],
            ),
        };
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(text).color(t.text));
        });
        ui.add_space(6.0);
    }
    egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
        devices_block(ui, app, t, busy);
        let scanner = app.scan.devices.scanner_selected(&app.scan.draft.device);
        if let Some(scanner) = scanner.as_ref() {
            source_notes(ui, &app.scan.draft, t, scanner);
            let effective = effective_source(&app.scan.draft, scanner);
            handling_notes(ui, &mut app.scan.draft, t, scanner, effective, busy);
            ui.add_space(4.0);
            if ui.add_enabled(!busy, egui::Button::new(tl!("Advanced…"))).clicked() {
                let mut draft = app.scan.draft.clone();
                draft.preview = None;
                draft.selection = None;
                draft.anchor = None;
                app.scan.advanced = Some(AdvancedEdit { draft });
            }
            ui.add_space(4.0);
            ui.label(egui::RichText::new(size_line(&app.scan.draft, scanner)).small().color(t.text_muted));
        }
        ui.add_space(6.0);
        let uses_feeder = app
            .scan
            .devices
            .scanner_selected(&app.scan.draft.device)
            .map(|scanner| effective_source(&app.scan.draft, &scanner) == Source::Feeder)
            .unwrap_or(false);
        preview_block(ui, &mut app.scan.draft, t, busy, uses_feeder);
        if let Some(error) = app.scan.draft.error.clone() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(error).color(Color32::from_rgb(0xB0, 0x30, 0x30)));
        }
        if let Some(note) = app.scan.last_length.as_ref().and_then(length_note) {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(note).small().color(t.text_muted));
        }
        if busy {
            ui.add_space(4.0);
            if let Some(job) = app.scan.job.as_ref() {
                let label = scan_status_label(job);
                let fraction = scan_bar_fraction(job.live.pages_done(), job.expected, job.live.page_percent());
                ui.label(label);
                match fraction {
                    // Buffer / page fill (or multi-page count) — never a made-up asymptotic %.
                    Some(f) => {
                        ui.add(egui::ProgressBar::new(f).desired_width(ui.available_width()).animate(true));
                    }
                    None => {
                        scan_indeterminate_bar(ui, t);
                    }
                }
            } else {
                ui.label(tl!("Scanning…"));
            }
        }
    });
    ui.add_space(8.0);
    let selected_scanner = app.scan.devices.scanner_selected(&app.scan.draft.device);
    let uses_feeder = selected_scanner.as_ref().map(|scanner| effective_source(&app.scan.draft, scanner) == Source::Feeder).unwrap_or(false);
    // Fallback defaults carry no handling status, so a device that could not be read never blocks here.
    let handling_blocks = selected_scanner.as_ref().and_then(|scanner| scanner.handling_status.acquire_block_reason()).is_some();
    let checking = app.scan.devices.checking();
    let awaiting_flip = app.scan.duplex.is_some();
    if !awaiting_flip {
        ui.horizontal(|ui| {
            let mut multi = app.scan.draft.pages_per_file > 0;
            if ui.add_enabled(!busy, crate::widgets::checkbox(&mut multi, tl!("Multiple files"))).changed() {
                app.scan.draft.pages_per_file = if multi { 1 } else { 0 };
            }
            if multi {
                ui.label(tl!("Pages per file"));
                let mut n = app.scan.draft.pages_per_file.max(1);
                if ui.add_enabled(!busy, egui::DragValue::new(&mut n).range(1..=999)).changed() {
                    app.scan.draft.pages_per_file = n;
                }
            }
        });
        ui.add_space(4.0);
    }
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if awaiting_flip {
                // The bed scans one sheet at a time, so the reverse sides of several bed pages cannot be scanned in one go.
                let reverse_possible = app.scan.duplex.is_some_and(|pending| pending.source == Source::Feeder || pending.fronts == 1);
                let two_sided = app.scan.duplex.is_some_and(|pending| pending.two_sided);
                // A duplex unit scanned the backs already: there are no reverse sides to add.
                if !two_sided
                    && ui
                        .add_enabled_ui(!busy && !checking && !handling_blocks && reverse_possible, |ui| {
                            widgets::pill_button(ui, tl!("Scan reverse sides"), true)
                        })
                        .inner
                        .clicked()
                {
                    click = ScanClick::ReverseSides;
                }
                let more = if two_sided { tl!("Continue scanning") } else { tl!("Continue scanning fronts") };
                if ui.add_enabled_ui(!busy && !checking && !handling_blocks, |ui| widgets::pill_button(ui, more, two_sided)).inner.clicked() {
                    click = ScanClick::MoreFronts;
                }
            } else {
                if ui.add_enabled_ui(!busy && !checking && has_doc, |ui| widgets::pill_button(ui, tl!("Insert scanned pages"), true)).inner.clicked()
                {
                    click = ScanClick::Insert;
                }
                if ui.add_enabled_ui(!busy && !checking && !handling_blocks, |ui| widgets::pill_button(ui, tl!("Create PDF"), true)).inner.clicked() {
                    click = ScanClick::Create;
                }
            }
            // Preview frames a region on the bed. The feeder would pull a sheet and still not
            // show where the next page lands, so the button stays off for that source.
            let mut preview = ui
                .add_enabled_ui(!busy && !checking && !uses_feeder && !awaiting_flip && !handling_blocks, |ui| {
                    widgets::pill_button(ui, tl!("Preview"), false)
                })
                .inner;
            if uses_feeder {
                preview = preview.on_disabled_hover_text(tl!("Preview uses the bed. Switch the source to Bed, or scan without a preview."));
            }
            if preview.clicked() {
                click = ScanClick::Preview;
            }
            let cancel = if busy {
                tl!("Stop")
            } else if awaiting_flip {
                // Keeps every page scanned; only the reverse sides are not scanned.
                tl!("Finish")
            } else {
                tl!("Cancel")
            };
            if widgets::pill_button(ui, cancel, false).clicked() {
                click = if busy {
                    ScanClick::Stop
                } else if awaiting_flip {
                    app.cancel_reverse_sides();
                    ScanClick::None
                } else {
                    ScanClick::Close
                };
            }
        });
    });
    click
}

/// Body of [`crate::Dialog::ScanInterrupted`]: what stopped, how far it got, what to do next.
/// Returns the chosen action. Closing the dialog counts as Finish (the caller maps that).
pub(crate) fn interrupted_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> Option<InterruptAction> {
    let Some(state) = app.scan.interrupted.clone() else { return Some(InterruptAction::Finish) };
    ui.label(egui::RichText::new(tl!("Scan interrupted")).font(theme::semibold(18.0)));
    // English only in this test build, like the other new strings: plain literals keep the catalogs alone.
    ui.label(egui::RichText::new(state.device_line()).color(t.text_muted));
    ui.add_space(8.0);
    ui.label(egui::RichText::new(state.headline()).font(theme::semibold(14.0)).color(Color32::from_rgb(0xB0, 0x30, 0x30)));
    ui.add_space(4.0);
    ui.label(state.count_line());
    if let Some(kept) = state.kept_line() {
        ui.label(kept);
    }
    ui.add_space(8.0);
    ui.label(egui::RichText::new(state.instruction_text()).color(t.text));
    if let Some(error) = app.scan.draft.error.clone() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(error).color(Color32::from_rgb(0xB0, 0x30, 0x30)));
    }
    ui.add_space(10.0);
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            for action in state.actions() {
                let (label, primary) = match action {
                    InterruptAction::Continue | InterruptAction::TryAgain => (state.primary_label().to_string(), true),
                    InterruptAction::ReverseNow => (state.reverse_now_label(), false),
                    InterruptAction::Finish => (tl!("Finish").to_string(), false),
                };
                if widgets::pill_button(ui, &label, primary).clicked() {
                    chosen = Some(action);
                }
            }
        });
    });
    chosen
}

/// The advanced settings window for the selected scanner. Closes itself when the scanner is not known.
fn advanced_modal(ctx: &egui::Context, app: &mut PdfCraftApp, t: &Tokens, busy: bool) {
    let Some(scanner) = app.scan.devices.scanner_selected(&app.scan.draft.device) else {
        app.scan.advanced = None;
        return;
    };
    let mut close = false;
    let mut apply = false;
    egui::Modal::new(egui::Id::new("scan-advanced")).show(ctx, |ui| {
        ui.set_width(480.0);
        ui.label(egui::RichText::new(tl!("Scanner settings")).font(theme::semibold(16.0)));
        ui.label(
            egui::RichText::new(tl!("Settings this source offers. Leave a control alone to keep the driver’s value.")).small().color(t.text_muted),
        );
        ui.add_space(10.0);
        let effective = effective_source(&app.scan.draft, &scanner);
        let choices = source_choices(&scanner, effective);
        if let Some(edit) = app.scan.advanced.as_mut() {
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                more_options(ui, &mut edit.draft, t, &choices, &scanner, busy);
            });
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::pill_button(ui, tl!("Done"), true).clicked() {
                    apply = true;
                    close = true;
                }
                if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                    close = true;
                }
            });
        });
    });
    if apply && let Some(edit) = app.scan.advanced.take() {
        apply_advanced(&edit.draft, &mut app.scan.draft);
    } else if close {
        app.scan.advanced = None;
    }
}

/// Copy knobs that belong in Advanced; leave device, source, dpi, color, pages, duplex, preview alone.
fn apply_advanced(from: &ScanDraft, to: &mut ScanDraft) {
    to.dpi_y = from.dpi_y;
    to.brightness = from.brightness;
    to.contrast = from.contrast;
    to.threshold = from.threshold;
    to.rotation = from.rotation;
    to.format = from.format.clone();
    to.compression = from.compression.clone();
    to.intent = from.intent;
    to.page_size = from.page_size;
    to.auto_deskew = from.auto_deskew;
    to.orientation = from.orientation;
    to.photometric = from.photometric;
    to.invert = from.invert;
    to.mirror = from.mirror;
    to.lamp = from.lamp;
}

/// What the shared source row cannot say: which source Auto will use, and what the driver reports.
fn source_notes(ui: &mut egui::Ui, draft: &ScanDraft, t: &Tokens, scanner: &Scanner) {
    let glass = scanner.flatbed || scanner.glass.is_some();
    let feeder = scanner.feeder || scanner.feeder_choices.is_some();
    if draft.source == Source::Auto && glass && feeder {
        let line = if effective_source(draft, scanner) == Source::Feeder { tl!("Will use the feeder.") } else { tl!("Will use the bed.") };
        ui.label(egui::RichText::new(line).small().color(t.text_muted));
    }
    if scanner.feeder_ready == Some(true) && matches!(draft.source, Source::Feeder | Source::Auto) {
        ui.label(
            egui::RichText::new(tl!("The driver reports a sheet waiting. The bit can stay on after the tray is empty.")).small().color(t.text_muted),
        );
    }
    let status = scanner.handling_status;
    if status.jam {
        ui.label(egui::RichText::new(tl!("The driver reports a paper jam.")).color(Color32::from_rgb(0xB0, 0x30, 0x30)));
    }
    // A cover that is open is a status warning drawn by the shared settings (see `warnings_of`).
}

/// Manual duplex belongs to the document flow, not to the shared settings: it is offered where the
/// scanner has no duplex of its own. The notes explain what the chosen page handling does.
fn handling_notes(ui: &mut egui::Ui, draft: &mut ScanDraft, t: &Tokens, scanner: &Scanner, effective: Source, busy: bool) {
    if !scanner.duplex && matches!(effective, Source::Feeder | Source::Glass) {
        ui.add_enabled(!busy, crate::widgets::checkbox(&mut draft.manual_duplex, tl!("Reverse sides")));
    }
    if draft.manual_duplex && !scanner.duplex {
        let note = if effective == Source::Glass {
            tl!("Scan the front, flip the sheet on the bed, then scan the reverse side.")
        } else {
            tl!("Scan the fronts, flip the stack (last sheet on top), then scan the reverse sides. Pages are interleaved front, back, …")
        };
        ui.label(egui::RichText::new(note).small().color(t.text_muted));
    } else if effective == Source::Feeder {
        let note = if draft.pages == 0 {
            tl!("All pages asks the driver for the whole stack.")
        } else {
            tl!("A page count stops the feeder after that many sheets. Cancel ends the transfer early; pages already received stay in the document.")
        };
        ui.label(egui::RichText::new(note).small().color(t.text_muted));
    }
}

/// Extra options the source offers (formats, compression, intents, page size, deskew, orientation, photometric,
/// invert, mirror, lamp) with notes about the connection and optical resolution.
fn more_options(ui: &mut egui::Ui, draft: &mut ScanDraft, t: &Tokens, choices: &ImageChoices, scanner: &Scanner, busy: bool) {
    let mut notes = Vec::new();
    if let (Some(x), Some(y)) = (scanner.optical_x, scanner.optical_y) {
        notes.push(crate::i18n::fmt(tl!("Optical resolution {x} × {y} dpi."), &[("x", &x.to_string()), ("y", &y.to_string())]));
    }
    match scanner.connection {
        pdfcraft_platform::scan::Connection::Usb => notes.push(tl!("Connected by USB.").to_string()),
        pdfcraft_platform::scan::Connection::Wsd => {
            notes.push(tl!("Connected over the network (Web Services). Options come from that driver, not the USB one.").to_string())
        }
        pdfcraft_platform::scan::Connection::Unknown | pdfcraft_platform::scan::Connection::Other => {}
    }
    if choices.formats.len() == 1 {
        notes.push(crate::i18n::fmt(tl!("Transfer format: {format}."), &[("format", choices.formats.first().map(String::as_str).unwrap_or("bmp"))]));
    }
    if !scanner.duplex {
        notes.push(tl!("Duplex is not offered.").to_string());
    }
    if !notes.is_empty() {
        ui.label(egui::RichText::new(notes.join(" ")).small().color(t.text_muted));
        ui.add_space(10.0);
    }

    let orientation_turns: Vec<Rotation> = choices.orientation.iter().filter_map(|value| Rotation::from_driver(*value)).collect();
    let orientation_matches_rotation = !choices.rotation.is_empty()
        && orientation_turns.len() == choices.orientation.len()
        && orientation_turns.iter().all(|turn| choices.rotation.contains(turn));

    egui::Grid::new("scan-more").num_columns(2).spacing([16.0, 14.0]).min_col_width(100.0).show(ui, |ui| {
        if !choices.dpi_y.is_empty() && choices.dpi_y != choices.dpi {
            ui.label(tl!("Vertical resolution"));
            let current = draft.dpi_y.unwrap_or(draft.dpi);
            egui::ComboBox::from_id_salt("scan-dpi-y").selected_text(format!("{current} dpi")).width(220.0).show_ui(ui, |ui| {
                for value in &choices.dpi_y {
                    ui.selectable_value(&mut draft.dpi_y, Some(*value), format!("{value} dpi"));
                }
            });
            ui.end_row();
        }
        if let Some(span) = choices.brightness {
            ui.label(tl!("Brightness"));
            span_slider(ui, span, &mut draft.brightness, busy);
            ui.end_row();
        }
        if let Some(span) = choices.contrast {
            ui.label(tl!("Contrast"));
            span_slider(ui, span, &mut draft.contrast, busy);
            ui.end_row();
        }
        if draft.color == Color::BlackAndWhite
            && let Some(span) = choices.threshold
        {
            ui.label(tl!("Threshold"));
            span_slider(ui, span, &mut draft.threshold, busy);
            ui.end_row();
        }
        if choices.formats.len() > 1 {
            ui.label(tl!("Format"));
            let current = draft.format.clone().unwrap_or_else(|| choices.formats.first().cloned().unwrap_or_default());
            egui::ComboBox::from_id_salt("scan-format").selected_text(&current).width(220.0).show_ui(ui, |ui| {
                for format in &choices.formats {
                    ui.selectable_value(&mut draft.format, Some(format.clone()), format);
                }
            });
            ui.end_row();
        }
        if choices.compression.len() > 1 {
            ui.label(tl!("Compression"));
            let current = draft.compression.clone().unwrap_or_else(|| choices.compression.first().cloned().unwrap_or_default());
            egui::ComboBox::from_id_salt("scan-compression").selected_text(&current).width(220.0).show_ui(ui, |ui| {
                for name in &choices.compression {
                    ui.selectable_value(&mut draft.compression, Some(name.clone()), name);
                }
            });
            ui.end_row();
        }
        if !choices.intents.is_empty() {
            ui.label(tl!("Intent"));
            let current = draft.intent.map(|intent| intent.name().to_string()).unwrap_or_else(|| tl!("Driver default").to_string());
            egui::ComboBox::from_id_salt("scan-intent").selected_text(current).width(220.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut draft.intent, None, tl!("Driver default"));
                for intent in &choices.intents {
                    ui.selectable_value(&mut draft.intent, Some(*intent), intent.name());
                }
            });
            ui.end_row();
        }
        if choices.page_sizes.len() > 1 {
            ui.label(tl!("Page size"));
            let current = draft.page_size.map(|size| size.name().to_string()).unwrap_or_default();
            egui::ComboBox::from_id_salt("scan-page-size").selected_text(current).width(220.0).show_ui(ui, |ui| {
                for size in &choices.page_sizes {
                    ui.selectable_value(&mut draft.page_size, Some(*size), size.name());
                }
            });
            ui.end_row();
        }
        if choices.auto_deskew {
            ui.label(tl!("Auto-deskew"));
            let mut on = draft.auto_deskew.unwrap_or(false);
            if ui.add_enabled(!busy, crate::widgets::checkbox(&mut on, tl!("Straighten the page"))).changed() {
                draft.auto_deskew = Some(on);
            }
            ui.end_row();
        }
        if choices.photometric.len() > 1 {
            ui.label(tl!("Photometric"));
            let current = draft.photometric.map(|value| value.to_string()).unwrap_or_default();
            egui::ComboBox::from_id_salt("scan-photometric").selected_text(current).width(220.0).show_ui(ui, |ui| {
                for value in &choices.photometric {
                    ui.selectable_value(&mut draft.photometric, Some(*value), value.to_string());
                }
            });
            ui.end_row();
        }
        if choices.invert {
            ui.label(tl!("Invert"));
            let mut on = draft.invert.unwrap_or(false);
            if ui.add_enabled(!busy, crate::widgets::checkbox(&mut on, tl!("Invert tones"))).changed() {
                draft.invert = Some(on);
            }
            ui.end_row();
        }
        if choices.mirror {
            ui.label(tl!("Mirror"));
            let mut on = draft.mirror.unwrap_or(false);
            if ui.add_enabled(!busy, crate::widgets::checkbox(&mut on, tl!("Mirror the page"))).changed() {
                draft.mirror = Some(on);
            }
            ui.end_row();
        }
        if choices.lamp {
            ui.label(tl!("Lamp"));
            let mut on = draft.lamp.unwrap_or(true);
            if ui.add_enabled(!busy, crate::widgets::checkbox(&mut on, tl!("Lamp on"))).changed() {
                draft.lamp = Some(on);
            }
            ui.end_row();
        }
    });

    // Wide control: keep it out of the two-column grid so the thumbs don't crush other rows.
    if !choices.rotation.is_empty() {
        ui.add_space(14.0);
        ui.label(tl!("Rotation"));
        ui.add_space(4.0);
        ui.add_enabled_ui(!busy, |ui| {
            rotation_examples(ui, t, &choices.rotation, draft.rotation, |turn| {
                draft.rotation = Some(turn);
                if orientation_matches_rotation && choices.orientation.contains(&turn.driver()) {
                    draft.orientation = Some(turn.driver());
                }
            });
        });
        ui.label(egui::RichText::new(tl!("The driver may accept a turn and leave the picture as it was.")).small().color(t.text_muted));
    } else if !choices.orientation.is_empty() {
        ui.add_space(14.0);
        ui.label(tl!("Orientation"));
        ui.add_space(4.0);
        if orientation_turns.len() == choices.orientation.len() {
            let selected = draft.orientation.and_then(Rotation::from_driver);
            ui.add_enabled_ui(!busy, |ui| {
                rotation_examples(ui, t, &orientation_turns, selected, |turn| draft.orientation = Some(turn.driver()));
            });
        } else {
            let current = draft.orientation.map(|value| value.to_string()).unwrap_or_default();
            egui::ComboBox::from_id_salt("scan-orientation").selected_text(current).width(220.0).show_ui(ui, |ui| {
                for value in &choices.orientation {
                    ui.selectable_value(&mut draft.orientation, Some(*value), value.to_string());
                }
            });
        }
    }
}

/// Slider with the span’s ends labelled so the control reads as adjustable, not a lone number.
fn span_slider(ui: &mut egui::Ui, span: ValueSpan, slot: &mut Option<i32>, busy: bool) {
    let value = slot.get_or_insert(if span.contains(0) { 0 } else { span.min });
    *value = (*value).clamp(span.min, span.max);
    let step = if span.step <= 0 { 1.0 } else { f64::from(span.step) };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(egui::RichText::new(span.min.to_string()).small().color(ui.visuals().weak_text_color()));
        ui.add_enabled(
            !busy,
            egui::Slider::new(value, span.min..=span.max).step_by(step).clamping(egui::SliderClamping::Always).min_decimals(0).max_decimals(0),
        );
        ui.label(egui::RichText::new(span.max.to_string()).small().color(ui.visuals().weak_text_color()));
    });
}

/// The preview and the region fields. With the feeder there is no preview to drag on, so only the inch fields
/// apply.
fn preview_block(ui: &mut egui::Ui, draft: &mut ScanDraft, t: &Tokens, busy: bool, feeder: bool) {
    if feeder {
        ui.label(
            egui::RichText::new(tl!("Preview is for the bed. Set a region in inches below, or leave it empty for the whole sheet."))
                .small()
                .color(t.text_muted),
        );
    } else {
        ui.label(
            egui::RichText::new(tl!("Drag on the preview to choose a region. An empty selection scans the whole area.")).small().color(t.text_muted),
        );
    }
    if let Some(preview) = draft.preview.clone() {
        paint_preview(ui, draft, t, &preview, busy);
    }
    ui.label(egui::RichText::new(tl!("Region in inches. Leave the width or the height at zero to scan the whole area.")).small().color(t.text_muted));
    ui.horizontal(|ui| {
        inch_field(ui, tl!("Left"), &mut draft.left_in, &mut draft.selection, busy);
        inch_field(ui, tl!("Top"), &mut draft.top_in, &mut draft.selection, busy);
        inch_field(ui, tl!("Width"), &mut draft.width_in, &mut draft.selection, busy);
        inch_field(ui, tl!("Height"), &mut draft.height_in, &mut draft.selection, busy);
        let has_region = draft.selection.is_some() || (draft.width_in > 0.0 && draft.height_in > 0.0);
        if ui.add_enabled(!busy && has_region, egui::Button::new(tl!("Clear region"))).clicked() {
            draft.clear_region();
        }
    });
}

/// One region field in inches (0 to 30). Editing it discards a region that was dragged.
fn inch_field(ui: &mut egui::Ui, label: &str, value: &mut f64, selection: &mut Option<(u32, u32, u32, u32)>, busy: bool) {
    ui.label(label);
    let response = ui.add_enabled(!busy, egui::DragValue::new(value).range(0.0..=30.0).speed(0.01).suffix(" in"));
    if response.changed() {
        *selection = None;
    }
}

/// Draw the preview scaled to fit, let a drag select a region of it (not while busy) and outline the current
/// region.
fn paint_preview(ui: &mut egui::Ui, draft: &mut ScanDraft, t: &Tokens, preview: &PreviewImage, busy: bool) {
    let max = Vec2::new(360.0, 420.0);
    let native = Vec2::new(preview.width.max(1) as f32, preview.height.max(1) as f32);
    let scale = (max.x / native.x).min(max.y / native.y).min(1.0);
    let size = native * scale;
    let sense = if busy { Sense::hover() } else { Sense::drag() };
    let (response, painter) = ui.allocate_painter(size, sense);
    if preview.rgba.len() == preview.width as usize * preview.height as usize * 4 {
        let image = egui::ColorImage::from_rgba_unmultiplied([preview.width as usize, preview.height as usize], &preview.rgba);
        let texture = ui.ctx().load_texture("pdfcraft-scan-preview", image, egui::TextureOptions::LINEAR);
        painter.image(texture.id(), response.rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    }
    if !busy && let Some(pointer) = response.interact_pointer_pos() {
        let pixel = pixel_at(response.rect, preview.width, preview.height, pointer);
        if response.drag_started() {
            draft.anchor = Some(pixel);
        }
        if response.dragged()
            && let Some(anchor) = draft.anchor
        {
            draft.remember_selection(Some(pixel_rect(anchor, pixel, preview.width, preview.height)));
        }
    }
    if response.drag_stopped() {
        draft.anchor = None;
    }
    if let Some(sel) = overlay(draft, preview) {
        let rect = widget_rect(response.rect, preview.width, preview.height, sel);
        painter.rect_stroke(rect, egui::CornerRadius::ZERO, egui::Stroke::new(1.5, t.accent), egui::StrokeKind::Middle);
    }
}

/// Bed or feeder the next acquire will use. Auto follows [`Scanner::feeder_ready`].
fn effective_source(draft: &ScanDraft, scanner: &Scanner) -> Source {
    let has_feeder = scanner.feeder || scanner.feeder_choices.is_some();
    let has_glass = scanner.flatbed || scanner.glass.is_some();
    draft.source.resolve(scanner.feeder_ready, has_feeder, has_glass).unwrap_or(Source::Glass)
}

/// The size of the next scan: the area in inches, its pixels at the chosen resolution and the estimated bitmap
/// size. "Full area" alone when the size is not known.
fn size_line(draft: &ScanDraft, scanner: &Scanner) -> String {
    let region = draft.region();
    let (width, height, full) = if let Some(region) = region {
        (region.width, region.height, false)
    } else {
        match effective_source(draft, scanner) {
            Source::Glass | Source::Auto => match scanner.bed {
                Some((w, h)) => (w, h, true),
                None => return tl!("Full area").to_string(),
            },
            Source::Feeder => match scanner.sheet {
                Some((w, h)) => (w, h, true),
                None => return tl!("Full area").to_string(),
            },
        }
    };
    let Some(px) = region_pixels(Region { left: 0, top: 0, width, height }, draft.dpi).ok() else {
        return tl!("Full area").to_string();
    };
    let Some(bytes) = estimated_bmp_bytes(px.width, px.height, draft.color.depth()) else {
        return tl!("Full area").to_string();
    };
    let size = usize::try_from(bytes).map(crate::panels::human_size).unwrap_or_else(|_| format!("{bytes} bytes"));
    let mut line = crate::i18n::fmt(
        tl!("{w} × {h} in, {pxw} × {pxh} pixels, {size}"),
        &[
            ("w", &format!("{:.2}", f64::from(width) / 1000.0)),
            ("h", &format!("{:.2}", f64::from(height) / 1000.0)),
            ("pxw", &px.width.to_string()),
            ("pxh", &px.height.to_string()),
            ("size", &size),
        ],
    );
    if full {
        line = format!("{} — {line}", tl!("Full area"));
    }
    if bytes >= 64 * 1024 * 1024 {
        line.push(' ');
        line.push_str(tl!("Large scan"));
    }
    line
}

/// The region to outline on the preview in preview pixels: the drag, or else the inch fields converted at the
/// preview's resolution.
fn overlay(draft: &ScanDraft, preview: &PreviewImage) -> Option<(u32, u32, u32, u32)> {
    if let Some(selection) = draft.selection {
        return Some(selection);
    }
    let region = draft.region()?;
    let px = region_pixels(region, preview.dpi).ok()?;
    Some((px.x, px.y, px.x.saturating_add(px.width).min(preview.width), px.y.saturating_add(px.height).min(preview.height)))
}

/// The preview pixel under a pointer position, clamped to the image.
fn pixel_at(rect: Rect, width: u32, height: u32, pos: Pos2) -> (u32, u32) {
    if width == 0 || height == 0 || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return (0, 0);
    }
    let x = ((pos.x - rect.min.x) / rect.width()).clamp(0.0, 0.999);
    let y = ((pos.y - rect.min.y) / rect.height()).clamp(0.0, 0.999);
    let px = (x * width as f32) as u32;
    let py = (y * height as f32) as u32;
    (px.min(width - 1), py.min(height - 1))
}

/// The rectangle between two pixels, both included, as `(x0, y0, x1, y1)` with the far edge exclusive and
/// clamped to the image.
fn pixel_rect(a: (u32, u32), b: (u32, u32), width: u32, height: u32) -> (u32, u32, u32, u32) {
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0).saturating_add(1).min(width));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1).saturating_add(1).min(height));
    (x0, y0, x1, y1)
}

/// The on-screen rectangle that a preview-pixel rectangle covers inside the drawn image.
fn widget_rect(image: Rect, width: u32, height: u32, sel: (u32, u32, u32, u32)) -> Rect {
    let sx = image.width() / width.max(1) as f32;
    let sy = image.height() / height.max(1) as f32;
    Rect::from_min_max(image.min + Vec2::new(sel.0 as f32 * sx, sel.1 as f32 * sy), image.min + Vec2::new(sel.2 as f32 * sx, sel.3 as f32 * sy))
}

/// The page counts a scanner offers from its range (at most 64), and whether it accepts 0 for all pages.
/// Without a range only 1 is offered.
fn page_values(span: Option<ValueSpan>) -> (Vec<u32>, bool) {
    let Some(span) = span else {
        return (vec![1], false);
    };
    let step = span.step.max(1);
    let mut values = Vec::new();
    let mut n = span.min.max(1);
    while n <= span.max && values.len() < 64 {
        if span.contains(n)
            && let Ok(page) = u32::try_from(n)
        {
            values.push(page);
        }
        let Some(next) = n.checked_add(step) else { break };
        n = next;
    }
    if values.is_empty() {
        values.push(1);
    }
    (values, span.contains(0))
}

/// 300 dpi if listed, else 200, else the first listed value.
fn preferred_dpi(listed: &[u32]) -> Option<u32> {
    [300, 200].into_iter().find(|dpi| listed.contains(dpi)).or_else(|| listed.first().copied())
}

/// Prefer 600 dpi for photographs, then 400, then the usual document dpi.
fn photo_dpi(listed: &[u32]) -> Option<u32> {
    [600, 400, 300]
        .into_iter()
        .find(|dpi| listed.contains(dpi))
        .or_else(|| listed.iter().copied().filter(|dpi| *dpi <= 600).max())
        .or_else(|| listed.first().copied())
}

/// Bring the draft in line with what the scanner offers for the resolved source: resolution, colour and the
/// adjustments that are not listed are reset or dropped. Runs once per scanner and source, and drops the
/// preview when Auto changes between bed and feeder.
fn snap_choices(draft: &mut ScanDraft, scanner: &Scanner, choices: &ImageChoices) {
    let resolved = effective_source(draft, scanner);
    if draft.snapped_device == scanner.id && draft.snapped_source == draft.source && draft.snapped_resolved == resolved {
        return;
    }
    if draft.snapped_device == scanner.id
        && draft.source == Source::Auto
        && draft.snapped_resolved != Source::Auto
        && draft.snapped_resolved != resolved
    {
        draft.clear_preview();
    }
    draft.snapped_device = scanner.id.clone();
    draft.snapped_source = draft.source;
    draft.snapped_resolved = resolved;
    if !choices.dpi.is_empty() && !choices.dpi.contains(&draft.dpi) {
        draft.dpi = preferred_dpi(&choices.dpi).unwrap_or(draft.dpi);
    }
    if !choices.colors.is_empty() && !choices.colors.contains(&draft.color) {
        draft.color = if choices.colors.contains(&Color::Gray) { Color::Gray } else { choices.colors.first().copied().unwrap_or(Color::Gray) };
    }
    draft.brightness = choices.brightness.map(|span| if span.contains(0) { 0 } else { span.min });
    draft.contrast = choices.contrast.map(|span| if span.contains(0) { 0 } else { span.min });
    draft.threshold = choices.threshold.map(|span| if span.contains(128) { 128 } else { span.min });
    draft.rotation = if choices.rotation.contains(&Rotation::Portrait) { Some(Rotation::Portrait) } else { choices.rotation.first().copied() };
    if choices.formats.len() > 1 {
        if draft.format.as_ref().is_none_or(|format| !choices.formats.contains(format)) {
            draft.format = choices.formats.first().cloned();
        }
    } else {
        draft.format = None;
    }
    if choices.compression.len() > 1 {
        if draft.compression.as_ref().is_none_or(|name| !choices.compression.contains(name)) {
            draft.compression = choices.compression.first().cloned();
        }
    } else {
        draft.compression = None;
    }
    if !choices.page_sizes.is_empty() {
        if draft.page_size.is_none_or(|size| !choices.page_sizes.contains(&size)) {
            draft.page_size = choices.page_sizes.first().copied();
        }
    } else {
        draft.page_size = None;
    }
    if !choices.auto_deskew {
        draft.auto_deskew = None;
    }
    if draft.intent.is_some_and(|intent| !choices.intents.is_empty() && !choices.intents.contains(&intent)) {
        draft.intent = None;
    }
    if draft.orientation.is_some_and(|value| !choices.orientation.is_empty() && !choices.orientation.contains(&value)) {
        draft.orientation = choices.orientation.first().copied();
    }
    if choices.orientation.is_empty() {
        draft.orientation = None;
    }
    if draft.photometric.is_some_and(|value| !choices.photometric.is_empty() && !choices.photometric.contains(&value)) {
        draft.photometric = choices.photometric.first().copied();
    }
    if choices.photometric.is_empty() {
        draft.photometric = None;
    }
    if !choices.invert {
        draft.invert = None;
    }
    if !choices.mirror {
        draft.mirror = None;
    }
    if !choices.lamp {
        draft.lamp = None;
    }
    if draft.dpi_y.is_some_and(|dpi| !choices.dpi_y.is_empty() && !choices.dpi_y.contains(&dpi)) {
        draft.dpi_y = None;
    }
    if choices.dpi_y.is_empty() || choices.dpi_y == choices.dpi {
        draft.dpi_y = None;
    }
    if !scanner.duplex {
        draft.duplex = false;
    } else {
        draft.manual_duplex = false;
    }
    let (values, all) = page_values(scanner.pages);
    if draft.pages != 0 && scanner.pages.is_some() && !values.contains(&draft.pages) {
        draft.pages = values.last().copied().unwrap_or(1);
    } else if draft.pages == 0 && !all && draft.source == Source::Glass {
        // The bed is always one page; feeder `0` still means "until empty" in `scan::acquire`.
        // Only when the user chose the bed: under Auto the source can resolve to the bed just
        // because the feeder looked empty when the list loaded, and this write is sticky (the
        // Pages list has no "All pages" entry to get back to), so a later feeder scan would
        // ask for one sheet. `final_settings` already sends one page for any bed scan.
        draft.pages = 1;
    }
}

/// The choices of the bed (also for Auto) or the feeder; empty when the scanner lists none.
fn source_choices(scanner: &Scanner, source: Source) -> ImageChoices {
    match source {
        Source::Glass | Source::Auto => scanner.glass.clone(),
        Source::Feeder => scanner.feeder_choices.clone(),
    }
    .unwrap_or_default()
}

/// The label for a turn.
fn rotation_label(turn: Rotation) -> String {
    match turn {
        Rotation::Portrait => tl!("Portrait").to_string(),
        Rotation::Landscape => tl!("Landscape").to_string(),
        Rotation::UpsideDown => tl!("Upside down").to_string(),
        Rotation::Left => tl!("Turned left").to_string(),
    }
}

/// Selectable page sketches for the four turns the driver offers.
fn rotation_examples(ui: &mut egui::Ui, t: &Tokens, turns: &[Rotation], selected: Option<Rotation>, mut choose: impl FnMut(Rotation)) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for &turn in turns {
            let on = selected == Some(turn);
            let response = orientation_thumb(ui, t, turn, on);
            if response.clicked() {
                choose(turn);
            }
        }
    });
}

/// A selectable sketch of a page turned as the driver would turn it. Returns the response for the click.
fn orientation_thumb(ui: &mut egui::Ui, t: &Tokens, turn: Rotation, selected: bool) -> egui::Response {
    let size = Vec2::new(52.0, 64.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let response = response.on_hover_text(rotation_label(turn));
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter_at(rect);
    let fill = if selected { t.accent_soft } else { t.field };
    painter.rect_filled(rect, 4.0, fill);
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(if selected { 1.5 } else { 1.0 }, if selected { t.accent } else { t.border }),
        egui::StrokeKind::Inside,
    );
    let page = Rect::from_center_size(rect.center(), Vec2::new(26.0, 36.0));
    painter.rect_filled(page, 2.0, Color32::WHITE);
    painter.rect_stroke(page, 2.0, egui::Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
    // Header bar and two text lines in page space, then turned to match the driver code.
    let center = page.center();
    let header = [
        Pos2::new(page.min.x + 4.0, page.min.y + 5.0),
        Pos2::new(page.max.x - 4.0, page.min.y + 5.0),
        Pos2::new(page.max.x - 4.0, page.min.y + 11.0),
        Pos2::new(page.min.x + 4.0, page.min.y + 11.0),
    ]
    .map(|p| turn_point(p, center, turn));
    painter.add(egui::Shape::convex_polygon(header.to_vec(), t.text_muted.gamma_multiply(0.55), egui::Stroke::NONE));
    for (i, y) in [16.0_f32, 22.0, 28.0].into_iter().enumerate() {
        let inset = if i == 2 { 10.0 } else { 4.0 };
        let a = turn_point(Pos2::new(page.min.x + 4.0, page.min.y + y), center, turn);
        let b = turn_point(Pos2::new(page.max.x - inset, page.min.y + y), center, turn);
        painter.line_segment([a, b], egui::Stroke::new(1.5, t.text_muted.gamma_multiply(0.7)));
    }
    response
}

/// Turn a point about `center` by the turn's rotation.
fn turn_point(point: Pos2, center: Pos2, turn: Rotation) -> Pos2 {
    let v = point - center;
    let (x, y) = match turn {
        Rotation::Portrait => (v.x, v.y),
        Rotation::Landscape => (-v.y, v.x),
        Rotation::UpsideDown => (-v.x, -v.y),
        Rotation::Left => (v.y, -v.x),
    };
    center + Vec2::new(x, y)
}

/// Settings for a preview: one low-resolution page from the bed, in the dialog's colour where the bed offers
/// it, with a preview or matching intent. No region and no adjustments.
fn preview_settings(draft: &ScanDraft, scanner: &Scanner) -> ScanSettings {
    // Preview always uses the bed. Callers must refuse the feeder before reaching here.
    // Color follows the dialog so a Color choice is not previewed as gray; dpi stays low.
    let choices = source_choices(scanner, Source::Glass);
    let color = if choices.colors.is_empty() || choices.colors.contains(&draft.color) {
        draft.color
    } else if choices.colors.contains(&Color::Gray) {
        Color::Gray
    } else {
        choices.colors.first().copied().unwrap_or(Color::Gray)
    };
    let intent = if choices.intents.contains(&Intent::BestPreview) {
        Some(Intent::BestPreview)
    } else {
        match color {
            Color::Rgb if choices.intents.contains(&Intent::Color) => Some(Intent::Color),
            Color::Gray if choices.intents.contains(&Intent::Gray) => Some(Intent::Gray),
            Color::BlackAndWhite if choices.intents.contains(&Intent::Text) => Some(Intent::Text),
            _ => None,
        }
    };
    ScanSettings {
        device: draft.device.clone(),
        source: Source::Glass,
        dpi: preview_dpi(&choices.dpi),
        dpi_y: None,
        color,
        pages: 1,
        brightness: None,
        contrast: None,
        threshold: if color == Color::BlackAndWhite { draft.threshold } else { None },
        rotation: None,
        region: None,
        format: None,
        compression: None,
        intent,
        page_size: None,
        page_length: scan::PageLength::Driver,
        auto_deskew: None,
        orientation: None,
        photometric: None,
        invert: None,
        mirror: None,
        lamp: None,
        duplex: false,
        cancel: None,
        progress: None,
    }
}

/// The engine's page-length request for the dialog's choice: Match page size is `Size` (the chosen size's length,
/// whatever the scanner could detect), Detect is `Auto` (the scanner's detection, else the size), and the image
/// guess is `AutoInfer`. Only the feeder has a length to decide.
fn engine_page_length(choice: shared::PageLength, effective: Source) -> scan::PageLength {
    if effective != Source::Feeder {
        return scan::PageLength::Driver;
    }
    match choice {
        shared::PageLength::Driver => scan::PageLength::Driver,
        shared::PageLength::MatchSize => scan::PageLength::Size,
        shared::PageLength::Detect => scan::PageLength::Auto,
        shared::PageLength::Infer => scan::PageLength::AutoInfer,
    }
}

/// One sentence about how the last scanned page got its length, `None` for the driver's own length (nothing to say).
fn length_note(record: &scan::LengthRecord) -> Option<String> {
    let inches = record.length_inches();
    match (record.source, inches) {
        (scan::LengthSource::Driver, _) => None,
        (scan::LengthSource::Device, Some(len)) => Some(format!("Page length {len:.2} in, detected by the scanner.")),
        (scan::LengthSource::Device, None) => Some("Page length detected by the scanner.".to_string()),
        (scan::LengthSource::Size, Some(len)) => Some(format!("Page length {len:.2} in, from the chosen page size.")),
        (scan::LengthSource::Size, None) => Some("Page length from the chosen page size.".to_string()),
        (scan::LengthSource::Inferred, Some(len)) => Some(format!("Page length about {len:.2} in, estimated from the scanned image.")),
        (scan::LengthSource::Inferred, None) => Some("Page length estimated from the scanned image.".to_string()),
    }
}

/// Settings for the final scan from the draft. The bed always takes one page, duplex applies only to a duplex
/// feeder, and the threshold only to black and white.
fn final_settings(draft: &ScanDraft, scanner: &Scanner, cancel: CancelToken) -> ScanSettings {
    let effective = effective_source(draft, scanner);
    ScanSettings {
        device: draft.device.clone(),
        source: draft.source,
        dpi: draft.dpi,
        dpi_y: draft.dpi_y,
        color: draft.color,
        pages: if effective == Source::Glass { 1 } else { draft.pages },
        brightness: draft.brightness,
        contrast: draft.contrast,
        threshold: if draft.color == Color::BlackAndWhite { draft.threshold } else { None },
        rotation: draft.rotation,
        region: draft.region(),
        format: draft.format.clone(),
        compression: draft.compression.clone(),
        intent: draft.intent,
        page_size: draft.page_size,
        page_length: engine_page_length(draft.page_length, effective),
        auto_deskew: draft.auto_deskew,
        orientation: draft.orientation,
        photometric: draft.photometric,
        invert: draft.invert,
        mirror: draft.mirror,
        lamp: draft.lamp,
        duplex: draft.duplex && scanner.duplex && effective == Source::Feeder,
        cancel: Some(cancel),
        progress: None,
    }
}

/// The progress sentence of a running scan: which page or side, of how many when known, and the share of the
/// page received.
fn scan_status_label(job: &ScanJob) -> String {
    let done = job.live.pages_done();
    let page_pct = job.live.page_percent();
    let received = job.live.bytes_received();
    let bytes_expected = job.live.bytes_expected();
    // Unknown stack or a one-page job: the buffer fill is the story, not a made-up overall %.
    let page_focused = job.expected.map(|total| total <= 1).unwrap_or(true);
    let prefix = match job.kind {
        ScanKind::ReverseSides => tl!("Scanning reverse side {n}…"),
        _ if job.duplex_source.is_some() => tl!("Scanning front {n}…"),
        _ => tl!("Scanning page {n}…"),
    };
    let mut base = match job.expected {
        Some(total) if total > 0 && matches!(job.kind, ScanKind::ReverseSides) => {
            crate::i18n::fmt(tl!("Scanning reverse side {n} of {t}…"), &[("n", &done.max(1).to_string()), ("t", &total.to_string())])
        }
        Some(total) if total > 0 && job.duplex_source.is_some() => {
            crate::i18n::fmt(tl!("Scanning front {n} of {t}…"), &[("n", &done.max(1).to_string()), ("t", &total.to_string())])
        }
        Some(total) if total > 0 => crate::i18n::fmt(tl!("Scanning page {n} of {t}…"), &[("n", &done.max(1).to_string()), ("t", &total.to_string())]),
        _ if done > 0 => crate::i18n::fmt(prefix, &[("n", &done.to_string())]),
        _ => tl!("Scanning…").to_string(),
    };
    // Digits only — no new i18n key. Buffer size first when that is the progress we have.
    // WSD JFIF: expected may be unknown mid-stream — still show received bytes + soft %.
    if page_focused {
        if received > 0 || page_pct.is_some() {
            let r = crate::panels::human_size(usize::try_from(received).unwrap_or(usize::MAX));
            base = match (bytes_expected, page_pct) {
                (Some(expected), Some(p)) => {
                    let e = crate::panels::human_size(usize::try_from(expected).unwrap_or(usize::MAX));
                    format!("{} · {r} / {e} ({p}%)", base.trim_end_matches('…'))
                }
                (Some(expected), None) => {
                    let e = crate::panels::human_size(usize::try_from(expected).unwrap_or(usize::MAX));
                    format!("{} · {r} / {e}", base.trim_end_matches('…'))
                }
                (None, Some(p)) if received > 0 => {
                    format!("{} · {r} ({p}%)", base.trim_end_matches('…'))
                }
                (None, Some(p)) => format!("{} {}%", base.trim_end_matches('…'), p),
                (None, None) if received > 0 => format!("{} · {r}", base.trim_end_matches('…')),
                _ => base,
            };
        }
    } else {
        if let Some(p) = page_pct {
            base = format!("{} {}%", base.trim_end_matches('…'), p);
        }
        if received > 0 {
            let r = crate::panels::human_size(usize::try_from(received).unwrap_or(usize::MAX));
            base = match bytes_expected {
                Some(expected) => {
                    let e = crate::panels::human_size(usize::try_from(expected).unwrap_or(usize::MAX));
                    format!("{base} · {r} / {e}")
                }
                None => format!("{base} · {r}"),
            };
        }
    }
    base
}

struct CreateApplyState {
    create_doc: Option<DocId>,
    pages_in_create: u32,
    create_file_index: u32,
}

/// Name for Create PDF document `index` (`0` → Scan.pdf, `1` → Scan (2).pdf).
fn scan_create_name(index: u32) -> String {
    if index == 0 { "Scan.pdf".into() } else { format!("Scan ({}).pdf", index + 1) }
}

/// Pages that still fit in the current multi-file document before a new one must open.
fn pages_remaining(per: u32, already: u32, has_doc: bool) -> u32 {
    if !has_doc {
        return per.max(1);
    }
    per.saturating_sub(already)
}

/// Honest bar fill. Multi-page with a known count uses pages + in-page fill; a single page or an
/// unknown stack uses only the page buffer (`None` until the first band/status arrives).
fn scan_bar_fraction(done: usize, expected: Option<u32>, page_percent: Option<u8>) -> Option<f32> {
    let page = page_percent.map(|p| f32::from(p) / 100.0);
    match expected {
        Some(total) if total > 1 => Some((((done as f32) + page.unwrap_or(0.0)) / (total as f32)).clamp(0.0, 0.99)),
        _ => page.map(|p| p.clamp(0.0, 0.99)),
    }
}

/// Walking band while we wait for the first buffer/status update (no honest fraction yet).
fn scan_indeterminate_bar(ui: &mut egui::Ui, t: &Tokens) {
    let width = ui.available_width();
    let (track, _) = ui.allocate_exact_size(egui::vec2(width, 6.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(track, egui::CornerRadius::same(3), t.text_muted.gamma_multiply(0.25));
    let now = ui.ctx().input(|i| i.time);
    let band = (track.width() * 0.28).clamp(36.0, 96.0);
    let travel = track.width() + band;
    let x = track.left() - band + ((now * 140.0) % f64::from(travel.max(1.0))) as f32;
    let clip = painter.with_clip_rect(track);
    clip.rect_filled(
        egui::Rect::from_min_size(egui::pos2(x, track.top()), egui::vec2(band, track.height())),
        egui::CornerRadius::same(3),
        t.accent.gamma_multiply(0.85),
    );
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

/// BMP (1, 4, 8, 24, or 32 bits uncompressed; 4 and 8 bits also RLE-compressed) as top-down RGBA.
/// Row 0 is the top of the page. Any other layout (bit fields, JPEG/PNG payloads, 16 bits) gives
/// `None`, and so does a truncated file, so the caller falls back to its own decoder. The header
/// rules and the decoder are `pdfcraft_scan::bmp`, shared with the scan crate's page builder.
pub fn bmp_rgba(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    scan::bmp::to_rgba(bytes)
}

/// Run `work` on a new named thread; `false` when the thread could not start. On wasm it runs at once on the
/// caller.
fn spawn_named(name: &str, work: impl FnOnce() + Send + 'static) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        work();
        true
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::Builder::new().name(name.into()).spawn(work).is_ok()
    }
}

impl PdfCraftApp {
    /// The selected scanner's display name (empty when its details are not known); never its id.
    fn scan_device_name(&self) -> String {
        self.scan.devices.scanner_selected(&self.scan.draft.device).map(|scanner| scanner.name).unwrap_or_default()
    }

    /// Start a scan for the button pressed. Does nothing while a scan is running.
    pub fn start_scan(&mut self, click: ScanClick) {
        self.start_scan_with(click, None);
    }

    /// [`Self::start_scan`], or the next leg of an interrupted scan. True when a job started.
    fn start_scan_with(&mut self, click: ScanClick, resume: Option<Resume>) -> bool {
        if matches!(click, ScanClick::Stop) {
            if let Some(job) = &self.scan.job {
                job.cancel.request();
            }
            return false;
        }
        if self.scan.job.is_some() || matches!(click, ScanClick::None | ScanClick::Close) {
            return false;
        }
        let Some(scanner) = self.scan.devices.scanner_selected(&self.scan.draft.device) else {
            let message = if self.scan.devices.checking() { tl!("Checking the scanner…") } else { tl!("No scanner is connected.") };
            self.scan.draft.error = Some(message.to_string());
            return false;
        };
        if matches!(click, ScanClick::Preview) && effective_source(&self.scan.draft, &scanner) == Source::Feeder {
            self.scan.draft.error = Some(tl!("Preview uses the bed. Switch the source to Bed, or scan without a preview.").to_string());
            return false;
        }
        if matches!(click, ScanClick::ReverseSides) && self.scan.duplex.is_none_or(|pending| pending.two_sided) {
            self.scan.draft.error = Some(tl!("Scan the fronts before the reverse sides.").to_string());
            return false;
        }
        let cancel = CancelToken::new();
        let live = ProgressToken::new();
        let mut settings = match click {
            ScanClick::Preview => {
                let mut settings = preview_settings(&self.scan.draft, &scanner);
                settings.cancel = Some(cancel.clone());
                settings.progress = Some(live.clone());
                settings
            }
            ScanClick::Create | ScanClick::Insert | ScanClick::ReverseSides | ScanClick::MoreFronts => {
                let mut settings = final_settings(&self.scan.draft, &scanner, cancel.clone());
                settings.progress = Some(live.clone());
                settings
            }
            ScanClick::None | ScanClick::Close | ScanClick::Stop => return false,
        };
        if matches!(click, ScanClick::MoreFronts) {
            // More fronts come from where the first ones did, into the same document after the last one.
            let Some(pending) = self.scan.duplex.filter(|pending| self.views.iter().any(|view| view.id == pending.doc)) else {
                self.scan.duplex = None;
                self.scan.draft.error = Some(tl!("The document is no longer open.").to_string());
                return false;
            };
            settings.source = pending.source;
        }
        if let Some(r) = &resume {
            // Once pages came in, keep the bed or feeder they came from (Auto would pick the bed
            // when the tray is now empty) and ask only for the pages still missing.
            if settings.source == Source::Auto
                && let Some(source) = r.source
            {
                settings.source = source;
            }
            if r.feeder
                && !matches!(click, ScanClick::ReverseSides)
                && let Some(pages) = r.pages
            {
                settings.pages = pages;
            }
        }
        if matches!(click, ScanClick::ReverseSides)
            && let Some(pending) = self.scan.duplex
            && !self.views.iter().any(|view| view.id == pending.doc)
        {
            // Never reached through the UI (closing the document cancels the reverse sides), but never scan for it.
            self.scan.duplex = None;
            self.scan.draft.error = Some(tl!("The document is no longer open.").to_string());
            return false;
        }
        if matches!(click, ScanClick::ReverseSides)
            && let Some(pending) = &self.scan.duplex
        {
            let n = u32::try_from(pending.fronts).unwrap_or(0);
            if n == 0 {
                self.scan.draft.error = Some(tl!("Scan the fronts before the reverse sides.").to_string());
                return false;
            }
            // Same source as the fronts. Forcing the feeder here broke bed reverse-side scans.
            settings.source = pending.source;
            settings.pages = if pending.source == Source::Glass { 1 } else { reverse_pages(pending.fronts, pending.backs) };
            if pending.source == Source::Glass && n != 1 {
                self.scan.draft.error =
                    Some(tl!("The bed scans one page at a time. Scan one front, flip it, then scan its reverse side.").to_string());
                return false;
            }
        }
        if let Err(error) = settings.checked() {
            self.scan.draft.error = Some(error.to_string());
            return false;
        }
        if let Some(region) = settings.region {
            let max = match effective_source(&self.scan.draft, &scanner) {
                Source::Glass | Source::Auto => scanner.bed,
                Source::Feeder => scanner.sheet,
            };
            if let Err(error) = scan::fit_region(region, max).and_then(|fitted| region_pixels(fitted, settings.dpi)) {
                self.scan.draft.error = Some(error.to_string());
                return false;
            }
        }
        let kind = match click {
            ScanClick::Preview => ScanKind::Preview,
            ScanClick::Create => ScanKind::Create,
            ScanClick::Insert => match resume.as_ref().map(|r| r.scan_kind) {
                Some(ScanKind::Insert(id)) => {
                    if !self.views.iter().any(|view| view.id == id) {
                        self.scan.draft.error = Some(tl!("The document is no longer open.").to_string());
                        return false;
                    }
                    ScanKind::Insert(id)
                }
                _ => {
                    let Some((_, id)) = self.active_ids() else { return false };
                    ScanKind::Insert(id)
                }
            },
            ScanClick::ReverseSides => ScanKind::ReverseSides,
            ScanClick::MoreFronts => match self.scan.duplex {
                Some(pending) => ScanKind::Insert(pending.doc),
                None => return false,
            },
            ScanClick::None | ScanClick::Close | ScanClick::Stop => return false,
        };
        let duplex_source = match effective_source(&self.scan.draft, &scanner) {
            Source::Auto => Source::Glass, // acquire will resolve; bed is the empty-tray case
            source => source,
        };
        // Prefer the source acquire will actually use when Auto is chosen.
        let duplex_source = if self.scan.draft.source == Source::Auto {
            if scanner.feeder_ready == Some(true) && (scanner.feeder || scanner.feeder_choices.is_some()) { Source::Feeder } else { Source::Glass }
        } else {
            duplex_source
        };
        let duplex_source = match (click, self.scan.duplex) {
            (ScanClick::MoreFronts, Some(pending)) => pending.source,
            _ => duplex_source,
        };
        let duplex_first_pass = (self.scan.draft.manual_duplex || matches!(click, ScanClick::MoreFronts))
            && !scanner.duplex
            && matches!(kind, ScanKind::Create | ScanKind::Insert(_))
            && matches!(duplex_source, Source::Feeder | Source::Glass);
        let expected = match kind {
            ScanKind::Preview => Some(1),
            ScanKind::ReverseSides => Some(settings.pages.max(1)),
            ScanKind::Create | ScanKind::Insert(_) if settings.pages > 0 => Some(settings.pages),
            ScanKind::Create | ScanKind::Insert(_) if duplex_first_pass && duplex_source == Source::Glass => Some(1),
            _ => None,
        };
        self.scan.draft.error = None;
        let hardware_duplex = settings.duplex;
        let slot = Arc::new(Mutex::new(JobSlot { result: None, error_kind: None }));
        let progress = Arc::clone(&slot);
        // acquire lights the lamp or pulls a sheet. It never runs on this thread.
        let started = spawn_named("pdfcraft-scan", move || {
            let outcome = scan::acquire(&settings);
            let error_kind = outcome.as_ref().err().map(|error| error.interrupt_kind());
            let result = outcome.map(|acquired| acquired.pages).map_err(|error| error.to_string());
            if let Ok(mut guard) = progress.lock() {
                guard.error_kind = error_kind;
                guard.result = Some(result);
            }
        });
        if !started {
            self.scan.draft.error = Some(tl!("The scan could not be started.").to_string());
            return false;
        }
        // Every kind leaves the dialog so the document can show each page as it arrives; a manual
        // duplex reopens it for the flip step once the fronts are in.
        if matches!(kind, ScanKind::Create | ScanKind::Insert(_) | ScanKind::ReverseSides) {
            self.dialog = None;
        }
        // The backs find their fronts by position, so the fronts of a manual duplex stay in one document.
        let pages_per_file = match kind {
            ScanKind::Create if self.scan.draft.pages_per_file > 0 && !duplex_first_pass => Some(self.scan.draft.pages_per_file),
            _ => None,
        };
        let carried = resume.as_ref().map_or(0, |r| r.carried);
        let expected_total = match (kind, expected) {
            _ if hardware_duplex => None,
            (ScanKind::Preview, _) => None,
            (_, Some(leg)) => Some(carried.saturating_add(leg)),
            (_, None) => None,
        };
        let (create_doc, pages_in_create, create_file_index, pages_per_file) = match &resume {
            // A document the user closed since is not appended to: the next page opens a new one.
            Some(r) => match r.create_doc.filter(|id| self.views.iter().any(|view| view.id == *id)) {
                Some(id) => (Some(id), r.pages_in_create, r.create_file_index, r.pages_per_file.or(pages_per_file)),
                None => (None, 0, r.create_file_index, r.pages_per_file.or(pages_per_file)),
            },
            None => (None, 0, 0, pages_per_file),
        };
        // The next page goes after the last one this scan already added.
        let insert_cursor = match kind {
            ScanKind::Insert(id) => match resume.as_ref().and_then(|r| r.insert_at) {
                Some(at) => Some(at),
                None => self.insert_cursor_for(id),
            },
            _ => None,
        };
        if resume.is_some() {
            let target = match kind {
                ScanKind::Insert(id) => insert_cursor.map(|at| (id, at)),
                ScanKind::Create => create_doc.map(|id| (id, self.session.get(id).map_or(0, |doc| doc.info.pages.len()))),
                ScanKind::Preview | ScanKind::ReverseSides => None,
            };
            if let Some((id, at)) = target
                && let Some(view) = self.views.iter_mut().find(|view| view.id == id)
            {
                view.insert_at = Some(at);
            }
        } else {
            self.scan.last_create_files = 0;
            self.scan.last_length = None;
            self.scan.interrupted = None;
            if duplex_first_pass {
                // A new first pass starts a new set of fronts.
                self.scan.duplex = None;
            }
        }
        self.scan.job = Some(ScanJob {
            kind,
            create_doc,
            pages_in_create,
            create_file_index,
            pages_per_file,
            expected,
            duplex_source: if duplex_first_pass { Some(duplex_source) } else { None },
            received: carried,
            expected_total,
            insert_cursor,
            source: duplex_source,
            requested_source: self.scan.draft.source,
            device_name: scanner.name.clone(),
            two_sided: hardware_duplex,
            continues: matches!(click, ScanClick::MoreFronts),
            live,
            progress: slot,
            cancel,
        });
        self.poll_scan();
        true
    }

    /// Collect the running scan's progress: adopt the source it used and take its finished result, if any, into the
    /// document, the preview or the interruption dialog.
    pub(crate) fn poll_scan(&mut self) {
        let Some(job) = &self.scan.job else { return };
        let kind = job.kind;
        let live = job.live.clone();
        if let Some(used) = live.source()
            && let Some(job) = self.scan.job.as_mut()
        {
            job.adopt_source(used);
        }
        let Some(job) = &self.scan.job else { return };
        let expected = job.expected;
        let progress = Arc::clone(&job.progress);
        let Ok(mut slot) = progress.lock() else {
            self.scan.job = None;
            self.clear_job_progress(crate::widgets::ProgressOwner::Scan);
            return;
        };
        let finished = slot.result.take();
        let error_kind = slot.error_kind.take();
        drop(slot);

        // Preview waits for the full transfer; other kinds apply each finished page now.
        if !matches!(kind, ScanKind::Preview) {
            let pages = live.take_pages();
            if !pages.is_empty() {
                self.apply_scanned_pages(kind, pages);
            }
        }

        let Some(result) = finished else {
            let done = live.pages_done();
            let page_percent = live.page_percent();
            let label = self.scan.job.as_ref().map(scan_status_label).unwrap_or_else(|| tl!("Scanning…").to_string());
            let cancelling = self.scan.job.as_ref().is_some_and(|job| job.cancel.is_requested());
            let show_notice =
                matches!(kind, ScanKind::Create | ScanKind::Insert(_) | ScanKind::ReverseSides) || self.dialog != Some(crate::Dialog::Scanner);
            if show_notice {
                let notice = crate::widgets::ProgressNotice {
                    label: if cancelling { tl!("Cancelling…").to_string() } else { label },
                    // No honest fraction yet (an unknown stack before the first update) rests at the start.
                    fraction: scan_bar_fraction(done, expected, page_percent).unwrap_or(0.0),
                    cancellable: !cancelling,
                };
                self.show_job_progress(crate::widgets::ProgressOwner::Scan, notice);
            }
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            return;
        };

        self.clear_job_progress(crate::widgets::ProgressOwner::Scan);
        match (kind, result) {
            (ScanKind::Preview, Ok(pages)) => {
                self.scan.job = None;
                self.take_preview(pages);
            }
            (ScanKind::Create | ScanKind::Insert(_), Ok(_)) => {
                let rest = live.take_pages();
                if !rest.is_empty() {
                    self.apply_scanned_pages(kind, rest);
                }
                let streamed = self.scan.job.as_ref().is_some_and(|job| job.create_doc.is_some() || job.received > 0) || live.pages_done() > 0;
                if matches!(kind, ScanKind::Create) {
                    self.scan.last_create_files = self.scan.job.as_ref().map(|job| job.create_file_index).unwrap_or(0).max(1);
                }
                let fronts_done = self.scan.duplex.is_some();
                self.scan.job = None;
                if fronts_done {
                    // The pages are in the document. The dialog stays on the next step: their reverse sides, more
                    // pages, or Finish, whether or not Reverse sides was ticked before the scan.
                    self.dialog = Some(crate::Dialog::Scanner);
                } else if streamed {
                    self.finish_scan_job(kind);
                }
            }
            (ScanKind::ReverseSides, Ok(_)) => {
                let rest = live.take_pages();
                if !rest.is_empty() {
                    self.apply_scanned_pages(kind, rest);
                }
                self.scan.job = None;
                self.finish_manual_duplex();
            }
            (ScanKind::Preview, Err(error)) => {
                self.scan.job = None;
                self.scan.draft.error = Some(error.clone());
                if self.dialog != Some(crate::Dialog::Scanner) {
                    self.notify(error);
                }
            }
            (_, Err(error)) => self.scan_interrupted(kind, error, error_kind, live.take_pages()),
        }
    }

    /// A Create/Insert/reverse-side job ended with an error. Whatever pages arrived are kept in
    /// the document (the fronts and backs of a manual duplex included), and the Scan interrupted
    /// dialog says what stopped and what to do. A scan the user stopped is not an interruption.
    fn scan_interrupted(&mut self, kind: ScanKind, error: String, error_kind: Option<InterruptKind>, leftover: Vec<Page>) {
        let cancelled = self.scan.job.as_ref().is_some_and(|job| job.cancel.is_requested());
        if !leftover.is_empty() {
            self.apply_scanned_pages(kind, leftover);
        }
        let Some(job) = self.scan.job.take() else {
            self.scan.draft.error = Some(error);
            return;
        };
        let state = job.interruption_of(&error, error_kind);
        if cancelled {
            // The user pressed Stop: keep the pages, say nothing more than before.
            if state.pass == Pass::Direct {
                self.scan.draft.error = Some(error.clone());
                if self.dialog != Some(crate::Dialog::Scanner) {
                    self.notify(error);
                }
            } else {
                if self.scan.duplex.is_none() {
                    self.scan.draft.error = Some(error);
                }
                self.dialog = Some(crate::Dialog::Scanner);
            }
            return;
        }
        self.scan.draft.error = None;
        self.scan.interrupted = Some(state);
        self.dialog = Some(crate::Dialog::ScanInterrupted);
    }

    /// Where the next page of an Insert goes: the gap chosen, else after the selection.
    fn insert_cursor_for(&self, id: DocId) -> Option<usize> {
        let view = self.views.iter().find(|view| view.id == id)?;
        let count = self.session.get(id).map_or(0, |doc| doc.info.pages.len());
        let after = view.target_pages().last().map_or(0, |page| page.saturating_add(1));
        Some(view.insert_at.unwrap_or(after).min(count))
    }

    /// The user's choice in the Scan interrupted dialog.
    pub(crate) fn resolve_interrupt(&mut self, action: InterruptAction) {
        match action {
            InterruptAction::Continue | InterruptAction::TryAgain => self.resume_interrupted(),
            InterruptAction::ReverseNow => {
                self.scan.interrupted = None;
                // The fronts are held in `duplex`: the Scanner dialog shows the flip step.
                self.dialog = self.scan.duplex.is_some().then_some(crate::Dialog::Scanner);
            }
            InterruptAction::Finish => self.finish_interrupted(),
        }
    }

    /// Continue (or Try again): the next acquire carries on with the same settings and the same
    /// document, for the pages still missing.
    fn resume_interrupted(&mut self) {
        let Some(state) = self.scan.interrupted.take() else { return };
        if self.dialog == Some(crate::Dialog::ScanInterrupted) {
            self.dialog = None;
        }
        self.scan.draft.error = None;
        if self.start_scan_with(state.click(), Some(state.resume())) {
            return;
        }
        // It could not start (no scanner, document closed): stay in the dialog with the reason.
        self.scan.interrupted = Some(state);
        self.dialog = Some(crate::Dialog::ScanInterrupted);
    }

    /// Finish: keep everything scanned and close. Every page scanned is already in the document
    /// (fronts, and the reverse sides so far beside their fronts); fronts without a reverse side
    /// stay single pages.
    fn finish_interrupted(&mut self) {
        let Some(state) = self.scan.interrupted.take() else {
            if self.dialog == Some(crate::Dialog::ScanInterrupted) {
                self.dialog = None;
            }
            return;
        };
        if self.dialog == Some(crate::Dialog::ScanInterrupted) {
            self.dialog = None;
        }
        self.scan.draft.error = None;
        let kind = match state.pass {
            Pass::Direct => state.scan_kind,
            Pass::Fronts | Pass::Backs => match self.scan.duplex.take() {
                Some(pending) => pending.kind,
                None => return,
            },
        };
        if state.has_pages() {
            if matches!(kind, ScanKind::Create) {
                let files = if state.pass == Pass::Direct { state.create_file_index } else { self.scan.last_create_files };
                self.scan.last_create_files = files.max(1);
            }
            self.finish_scan_job(kind);
        }
    }

    /// A document was closed. A scan that was putting pages into it stops, a stopped scan waiting to go on into it is
    /// dropped, and a manual duplex waiting for its reverse sides is cancelled, so the scanner never pulls paper for a
    /// document that is gone. Called by [`crate::PdfCraftApp::close_tab`].
    pub(crate) fn scan_document_closed(&mut self, id: DocId) {
        let duplex_here = self.scan.duplex.is_some_and(|pending| pending.doc == id);
        let mut stopped = false;
        if let Some(job) = &self.scan.job {
            let into_it = match job.kind {
                ScanKind::Insert(doc) => doc == id,
                ScanKind::Create => job.create_doc == Some(id),
                ScanKind::ReverseSides => duplex_here,
                ScanKind::Preview => false,
            };
            if into_it {
                job.cancel.request();
                stopped = true;
            }
        }
        if let Some(state) = &self.scan.interrupted {
            let into_it =
                state.doc == Some(id) || matches!(state.scan_kind, ScanKind::Insert(doc) if doc == id) || (state.pass != Pass::Direct && duplex_here);
            if into_it {
                self.scan.interrupted = None;
                if self.dialog == Some(crate::Dialog::ScanInterrupted) {
                    self.dialog = None;
                }
                stopped = true;
            }
        }
        if duplex_here {
            self.scan.duplex = None;
            self.scan.draft.error = None;
            self.notify_tr("The reverse sides were cancelled: their document was closed.");
        } else if stopped {
            self.notify_tr("The scan was stopped: its document was closed.");
        }
    }

    /// The scanner dialog was closed. Pages offered for reverse sides only after the fact (Reverse sides was not
    /// ticked) are finished, so the offer does not come back the next time the dialog opens. A requested manual
    /// duplex keeps waiting: the person may have closed the dialog to turn the stack over.
    pub(crate) fn scan_dialog_closed(&mut self) {
        if self.scan.job.is_none() && self.scan.duplex.is_some_and(|pending| !pending.requested) {
            self.cancel_reverse_sides();
        }
    }

    /// The user gave up on the reverse sides: the document keeps the fronts as they are.
    pub(crate) fn cancel_reverse_sides(&mut self) {
        let Some(pending) = self.scan.duplex.take() else { return };
        self.scan.draft.error = None;
        if matches!(pending.kind, ScanKind::Create) {
            self.scan.last_create_files = self.scan.last_create_files.max(1);
        }
        self.finish_scan_job(pending.kind);
    }

    /// Insert scanned pages as they arrive. The first front of a Create opens the document.
    fn apply_scanned_pages(&mut self, kind: ScanKind, pages: Vec<Page>) {
        if matches!(kind, ScanKind::ReverseSides) {
            self.apply_back_pages(pages);
            return;
        }
        let count = pages.len();
        if let Some(page) = pages.last() {
            self.scan.last_length = Some(page.length);
        }
        let mut first_hint = 0;
        let mut fronts_pass = false;
        if let Some(job) = &mut self.scan.job {
            first_hint = job.insert_cursor.unwrap_or(0);
            // A continuation is placed like the fronts of a manual duplex: after the last page scanned so far.
            fronts_pass = job.duplex_source.is_some() || job.continues;
            job.received = job.received.saturating_add(u32::try_from(count).unwrap_or(u32::MAX));
            job.insert_cursor = job.insert_cursor.map(|at| at.saturating_add(count));
        }
        match kind {
            ScanKind::Create => {
                let (create_doc, pages_in, file_index, per) = self
                    .scan
                    .job
                    .as_ref()
                    .map(|job| (job.create_doc, job.pages_in_create, job.create_file_index, job.pages_per_file))
                    .unwrap_or((None, 0, 0, None));
                let per = if fronts_pass {
                    None
                } else {
                    per.or_else(|| {
                        let n = self.scan.draft.pages_per_file;
                        (n > 0).then_some(n)
                    })
                };
                if fronts_pass
                    && let Some(pending) = self.scan.duplex
                    && Some(pending.doc) == create_doc
                {
                    self.place_next_insert(pending.doc, pending.first.saturating_add(pending.fronts));
                }
                let state = self.apply_create_pages(pages, create_doc, pages_in, file_index, per);
                if let Some(job) = &mut self.scan.job {
                    job.create_doc = state.create_doc;
                    job.pages_in_create = state.pages_in_create;
                    job.create_file_index = state.create_file_index;
                }
                self.scan.last_create_files = state.create_file_index.max(self.scan.last_create_files);
                // Every scan into one document is remembered as fronts, so its reverse sides or more pages can still
                // be scanned after the fact. A scan split into several files cannot have them.
                if fronts_pass || per.is_none() {
                    self.note_fronts(kind, state.create_doc, 0, count);
                }
            }
            ScanKind::Insert(id) => {
                if fronts_pass {
                    let at = match self.scan.duplex {
                        Some(pending) if pending.doc == id => pending.first.saturating_add(pending.fronts),
                        _ => first_hint,
                    };
                    self.place_next_insert(id, at);
                }
                self.insert_scanned_pdf(id, &pages);
                self.note_fronts(kind, Some(id), first_hint, count);
            }
            ScanKind::Preview | ScanKind::ReverseSides => {}
        }
    }

    /// The next insert into `id` goes to page index `at`, whatever is selected.
    fn place_next_insert(&mut self, id: DocId, at: usize) {
        let count = self.session.get(id).map_or(0, |doc| doc.info.pages.len());
        if let Some(view) = self.views.iter_mut().find(|view| view.id == id) {
            view.insert_at = Some(at.min(count));
        }
    }

    /// Remember where the fronts of this manual duplex are: `count` more in `doc`, the first of the
    /// whole set at `first` (Insert) or page 0 (Create).
    fn note_fronts(&mut self, kind: ScanKind, doc: Option<DocId>, first: usize, count: usize) {
        let Some(doc) = doc.filter(|id| self.views.iter().any(|view| view.id == *id)) else { return };
        let first = if matches!(kind, ScanKind::Create) { 0 } else { first };
        let requested = self.scan.job.as_ref().is_some_and(|job| job.duplex_source.is_some());
        let two_sided = self.scan.job.as_ref().is_some_and(|job| job.two_sided);
        // The source the fronts came from: the one locked for a requested duplex, else the one the scan used.
        let source = self.scan.job.as_ref().map(|job| job.duplex_source.unwrap_or(job.source)).unwrap_or(Source::Glass);
        match &mut self.scan.duplex {
            Some(pending) if pending.doc == doc => {
                pending.fronts = pending.fronts.saturating_add(count);
                pending.requested |= requested;
                pending.two_sided |= two_sided;
            }
            _ => self.scan.duplex = Some(DuplexAwait { kind, source, doc, first, fronts: count, backs: 0, requested, two_sided }),
        }
    }

    /// Reverse sides arrive one by one: each goes right after its front (see [`back_position`]).
    fn apply_back_pages(&mut self, pages: Vec<Page>) {
        for page in pages {
            let Some(pending) = self.scan.duplex else { return };
            if !self.views.iter().any(|view| view.id == pending.doc) {
                let message = tl!("The document is no longer open.").to_string();
                self.scan.draft.error = Some(message.clone());
                self.notify(message);
                return;
            }
            // The stack was flipped as a whole, so the first back belongs to the last front.
            let Some(at) = back_position(pending.first, pending.fronts, pending.backs, true) else { return };
            self.place_next_insert(pending.doc, at);
            self.insert_scanned_pdf(pending.doc, std::slice::from_ref(&page));
            if let Some(held) = &mut self.scan.duplex {
                held.backs = held.backs.saturating_add(1);
            }
            if let Some(job) = &mut self.scan.job {
                job.received = job.received.saturating_add(1);
            }
        }
    }

    /// Apply Create pages, optionally splitting into a new document every `per` pages.
    fn apply_create_pages(
        &mut self,
        pages: Vec<Page>,
        mut create_doc: Option<DocId>,
        mut pages_in_create: u32,
        mut create_file_index: u32,
        per: Option<u32>,
    ) -> CreateApplyState {
        if pages.is_empty() {
            return CreateApplyState { create_doc, pages_in_create, create_file_index };
        }
        let Some(per) = per.filter(|n| *n > 0) else {
            if let Some(id) = create_doc {
                self.insert_scanned_pdf(id, &pages);
                pages_in_create = pages_in_create.saturating_add(pages.len() as u32);
            } else {
                let Some((first, rest)) = pages.split_first() else {
                    return CreateApplyState { create_doc, pages_in_create, create_file_index };
                };
                self.open_scanned_pdf_named(&scan_create_name(create_file_index), std::slice::from_ref(first));
                create_file_index = create_file_index.saturating_add(1);
                create_doc = self.active_ids().map(|(_, id)| id);
                pages_in_create = 1;
                if let Some(id) = create_doc
                    && !rest.is_empty()
                {
                    self.insert_scanned_pdf(id, rest);
                    pages_in_create = pages_in_create.saturating_add(rest.len() as u32);
                }
            }
            return CreateApplyState { create_doc, pages_in_create, create_file_index };
        };

        let mut rest = pages;
        while !rest.is_empty() {
            let room = pages_remaining(per, pages_in_create, create_doc.is_some());
            if room == 0 {
                create_doc = None;
                pages_in_create = 0;
                continue;
            }
            let take = (room as usize).min(rest.len());
            let chunk: Vec<Page> = rest.drain(..take).collect();
            if let Some(id) = create_doc {
                self.insert_scanned_pdf(id, &chunk);
                pages_in_create = pages_in_create.saturating_add(chunk.len() as u32);
            } else {
                let Some((first, more)) = chunk.split_first() else { break };
                self.open_scanned_pdf_named(&scan_create_name(create_file_index), std::slice::from_ref(first));
                create_file_index = create_file_index.saturating_add(1);
                create_doc = self.active_ids().map(|(_, id)| id);
                pages_in_create = 1;
                if let Some(id) = create_doc
                    && !more.is_empty()
                {
                    self.insert_scanned_pdf(id, more);
                    pages_in_create = pages_in_create.saturating_add(more.len() as u32);
                }
            }
            if pages_in_create >= per {
                create_doc = None;
                pages_in_create = 0;
            }
        }
        CreateApplyState { create_doc, pages_in_create, create_file_index }
    }

    /// The reverse-side pass ended: done when every front has its back, otherwise offer the rest.
    fn finish_manual_duplex(&mut self) {
        let Some(pending) = self.scan.duplex else {
            self.scan.draft.error = Some(tl!("Scan the fronts before the reverse sides.").to_string());
            return;
        };
        if pending.backs < pending.fronts {
            let message = format!(
                "{} ({} {}, {} {})",
                tl!("Manual duplex needs one reverse side for each front."),
                pending.fronts,
                tl!("front(s)"),
                pending.backs,
                tl!("reverse side(s)"),
            );
            // Short: the backs are in place; offer Continue for the missing ones, or Finish.
            let state = Interrupted {
                kind: InterruptKind::Other,
                message,
                received: u32::try_from(pending.backs).unwrap_or(u32::MAX),
                expected: u32::try_from(pending.fronts).ok(),
                scan_kind: ScanKind::ReverseSides,
                pass: Pass::Backs,
                doc: None,
                pages_in_create: 0,
                create_file_index: 0,
                pages_per_file: None,
                insert_at: None,
                source: pending.source,
                requested_source: pending.source,
                resolved_source: None,
                device_name: self.scan_device_name(),
            };
            self.scan.draft.error = None;
            self.scan.interrupted = Some(state);
            self.dialog = Some(crate::Dialog::ScanInterrupted);
            return;
        }
        self.scan.duplex = None;
        self.notify(tl!("Reverse sides interleaved.").to_string());
        if matches!(pending.kind, ScanKind::Create) {
            self.scan.last_create_files = self.scan.last_create_files.max(1);
        }
        self.finish_scan_job(pending.kind);
    }

    /// After Create/Insert finishes: close the scanner dialog and leave Create a PDF / Scan & OCR
    /// so the document is the focus. Does not open Organize — rotation stays a choice, as with
    /// Enhance camera image's optional next steps.
    fn finish_scan_job(&mut self, kind: ScanKind) {
        if self.dialog == Some(crate::Dialog::Scanner) {
            self.dialog = None;
        }
        if matches!(self.left, crate::LeftPanel::Tool("create" | "scan")) {
            self.left = crate::LeftPanel::AllTools;
        }
        match kind {
            ScanKind::Create => {
                let n = self.scan.last_create_files.max(1);
                if n > 1 {
                    self.notify_fmt("Created {n} scanned files", &[("n", &n.to_string())]);
                } else {
                    self.notify_tr("Created Scan.pdf");
                }
            }
            ScanKind::Insert(_) => self.notify_tr("Inserted scanned pages"),
            ScanKind::Preview | ScanKind::ReverseSides => {}
        }
        // The length is only worth a word when something other than the driver decided it.
        if matches!(kind, ScanKind::Create | ScanKind::Insert(_))
            && let Some(note) = self.scan.last_length.as_ref().and_then(length_note)
            && let Some((text, _)) = &self.toast
        {
            let text = format!("{text} {note}");
            self.notify(text);
        }
    }

    /// Show the first page of a preview scan. A page that cannot be decoded sets an error instead.
    fn take_preview(&mut self, pages: Vec<Page>) {
        let Some(page) = pages.into_iter().next() else {
            self.scan.draft.error = Some(tl!("The preview could not be shown.").to_string());
            return;
        };
        match bmp_rgba(&page.bytes) {
            Some((width, height, rgba)) => {
                self.scan.draft.selection = None;
                self.scan.draft.anchor = None;
                self.scan.draft.preview = Some(PreviewImage { width, height, dpi: page.dpi.max(1), rgba });
                self.scan.draft.error = None;
            }
            None => self.scan.draft.error = Some(tl!("The preview could not be shown.").to_string()),
        }
    }

    /// Build a PDF from the scanned pages, sized by the first page's resolution. No pages is an error.
    fn scanned_pdf(&self, pages: &[Page]) -> Result<Vec<u8>, String> {
        let Some(first) = pages.first() else {
            return Err(tl!("The scan produced no pages.").to_string());
        };
        let images =
            pages.iter().enumerate().map(|(index, page)| (format!("scan-{}.{}", index + 1, page.format), page.bytes.clone())).collect::<Vec<_>>();
        let pdf = self
            .session
            .create_from_images_with_resolution(&images, pdfcraft_engine::ImageResolution::Dpi(f64::from(first.dpi)))
            .map_err(|error| error.to_string())?;
        Ok(pdf.as_ref().clone())
    }

    /// Open the scanned pages as a new document called `name`, closing the scanner dialog; failures are shown in
    /// the dialog.
    fn open_scanned_pdf_named(&mut self, name: &str, pages: &[Page]) {
        match self.scanned_pdf(pages) {
            Ok(bytes) => {
                if let Err(error) = self.open_bytes(name, None, bytes) {
                    self.scan.draft.error = Some(error);
                } else if self.dialog == Some(crate::Dialog::Scanner) {
                    // Dialog can close as pages stream in; Create/Scan tools stay until the job ends.
                    self.dialog = None;
                }
            }
            Err(error) => self.scan.draft.error = Some(error),
        }
    }

    /// Insert the scanned pages into the document `id`. When it was closed the pages are dropped, with a message
    /// unless the scan was stopped.
    fn insert_scanned_pdf(&mut self, id: DocId, pages: &[Page]) {
        let Some(index) = self.views.iter().position(|view| view.id == id) else {
            // Its document was closed and the scan was stopped then (with a message): the pages still in flight
            // are dropped quietly.
            if self.scan.job.as_ref().is_some_and(|job| job.cancel.is_requested()) {
                return;
            }
            let message = tl!("The document is no longer open.").to_string();
            self.scan.draft.error = Some(message.clone());
            self.notify(message);
            return;
        };
        let bytes = match self.scanned_pdf(pages) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.scan.draft.error = Some(error);
                return;
            }
        };
        self.active = Some(index);
        self.insert_pages_from("Scan.pdf", bytes);
        if self.dialog == Some(crate::Dialog::Scanner) {
            self.dialog = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PreviewImage, ScanDraft, bmp_rgba};

    /// Runs on every platform: the gate decides from a plain bool, so both answers are checked.
    #[test]
    fn the_dialog_shows_a_notice_only_where_scanning_is_unsupported() {
        assert_eq!(super::unavailable_notice(true), None);
        assert_eq!(super::unavailable_notice(false), Some("Scanning needs Windows (WIA) in this version."));
    }
    use egui::Pos2;
    use pdfcraft_platform::scan::Source;

    #[test]
    fn page_length_defaults_to_the_driver_and_resets_with_the_source() {
        use craft_ui::scanning::PageLength as Choice;
        let mut draft = ScanDraft::default();
        assert_eq!(draft.page_length, Choice::Driver);
        draft.page_length = Choice::Infer;
        draft.set_source(Source::Glass);
        assert_eq!(draft.page_length, Choice::Driver, "changing the source starts the advanced choices over");
    }

    #[test]
    fn the_dialog_choice_maps_to_the_engine_request_for_the_feeder_only() {
        use super::engine_page_length;
        use craft_ui::scanning::PageLength as Choice;
        use pdfcraft_platform::scan::PageLength as Engine;
        assert_eq!(engine_page_length(Choice::Driver, Source::Feeder), Engine::Driver);
        assert_eq!(engine_page_length(Choice::MatchSize, Source::Feeder), Engine::Size);
        assert_eq!(engine_page_length(Choice::Detect, Source::Feeder), Engine::Auto);
        assert_eq!(engine_page_length(Choice::Infer, Source::Feeder), Engine::AutoInfer);
        for choice in Choice::ALL {
            assert_eq!(engine_page_length(choice, Source::Glass), Engine::Driver, "the bed has a fixed length");
        }
    }

    #[test]
    fn final_settings_carry_the_page_length_and_the_chosen_size() {
        use super::{choices_of, final_settings, length_note};
        use craft_ui::scanning::PageLength as Choice;
        use pdfcraft_platform::scan::{CancelToken, LengthRecord, LengthSource, PageLength, PageLengthSupport, Presence};
        let mut scanner = super::fallback_scanner(&craft_ui::scanning::Scanner::default());
        scanner.feeder = true;
        scanner.flatbed = true;
        let draft = ScanDraft {
            device: scanner.id.clone(),
            source: Source::Feeder,
            page_size: Some(pdfcraft_platform::scan::PageSize::Legal),
            page_length: Choice::MatchSize,
            ..ScanDraft::default()
        };
        assert!(draft.page_size_names_a_length());
        let settings = final_settings(&draft, &scanner, CancelToken::new());
        assert_eq!((settings.page_length, settings.page_size), (PageLength::Size, Some(pdfcraft_platform::scan::PageSize::Legal)));
        let custom = ScanDraft { page_size: Some(pdfcraft_platform::scan::PageSize::Custom), ..draft.clone() };
        assert!(!custom.page_size_names_a_length(), "custom names no length");
        // Detection is read from the feeder item: present, absent and not read are three different rows.
        scanner.reported.feeder_length = PageLengthSupport { device_detect: Presence::Present, max_length_mils: None };
        assert!(choices_of(&scanner).detect_length.is_supported());
        scanner.reported.feeder_length = PageLengthSupport { device_detect: Presence::Absent, max_length_mils: None };
        assert!(choices_of(&scanner).detect_length.is_unsupported());
        scanner.reported.feeder_length = PageLengthSupport::default();
        let unknown = choices_of(&scanner).detect_length;
        assert!(!unknown.is_known() && unknown.is_enabled());
        // The status sentence says how the length was obtained; the driver's own length says nothing.
        assert_eq!(length_note(&LengthRecord::from_delivered(2800, 200)), None);
        assert_eq!(
            length_note(&LengthRecord::with_source(2146, 200, LengthSource::Inferred)).as_deref(),
            Some("Page length about 10.73 in, estimated from the scanned image.")
        );
        assert_eq!(
            length_note(&LengthRecord::with_source(2338, 200, LengthSource::Size)).as_deref(),
            Some("Page length 11.69 in, from the chosen page size.")
        );
        assert_eq!(length_note(&LengthRecord::default()), None);
        assert_eq!(
            length_note(&LengthRecord { source: LengthSource::Device, length_mils: None, approximate: false }).as_deref(),
            Some("Page length detected by the scanner.")
        );
    }

    #[test]
    fn the_dialog_defaults_to_auto_300_dpi_gray() {
        let draft = ScanDraft::default();
        assert_eq!(draft.source, Source::Auto);
        assert_eq!(draft.dpi, 300);
        assert_eq!(draft.color, pdfcraft_platform::scan::Color::Gray);
        assert_eq!(draft.pages, 0, "feeder/auto defaults to the whole stack");
    }

    #[test]
    fn snap_choices_keeps_feeder_all_pages_when_wia_omits_zero_in_range() {
        use super::snap_choices;
        use pdfcraft_platform::scan::{Color, Connection, ImageChoices, Scanner, ValueSpan};

        let feeder_choices = ImageChoices { dpi: vec![100, 200, 300], colors: vec![Color::Gray], ..ImageChoices::default() };
        let scanner = Scanner {
            id: "wsd-1".into(),
            name: "Network scanner (WSD)".into(),
            feeder: true,
            flatbed: true,
            duplex: false,
            feeder_ready: Some(true),
            handling_status: Default::default(),
            feeder_capacity: Some(30),
            connection: Connection::Wsd,
            port: Some("http://192.0.2.10/wsd".into()),
            optical_x: None,
            optical_y: None,
            pages: Some(ValueSpan { min: 1, max: 30, step: 1 }),
            bed: Some((8500, 11000)),
            sheet: None,
            glass: Some(ImageChoices::default()),
            feeder_choices: Some(feeder_choices.clone()),
            reported: Default::default(),
        };
        let mut draft = ScanDraft { device: scanner.id.clone(), source: Source::Feeder, pages: 0, ..ScanDraft::default() };
        draft.snapped_device.clear();
        snap_choices(&mut draft, &scanner, &feeder_choices);
        assert_eq!(draft.pages, 0, "All pages on the feeder must stay 0 when the driver range is 1..30 (scan::acquire whole-stack)");
    }

    #[test]
    fn snap_choices_clamps_glass_all_pages_to_one_without_driver_zero() {
        use super::snap_choices;
        use pdfcraft_platform::scan::{Color, Connection, ImageChoices, Scanner, ValueSpan};

        let glass = ImageChoices { dpi: vec![100, 200, 300], colors: vec![Color::Gray], ..ImageChoices::default() };
        let scanner = Scanner {
            id: "wsd-1".into(),
            name: "Network scanner (WSD)".into(),
            feeder: true,
            flatbed: true,
            duplex: false,
            feeder_ready: None,
            handling_status: Default::default(),
            feeder_capacity: None,
            connection: Connection::Wsd,
            port: None,
            optical_x: None,
            optical_y: None,
            pages: Some(ValueSpan { min: 1, max: 1, step: 1 }),
            bed: Some((8500, 11000)),
            sheet: None,
            glass: Some(glass.clone()),
            feeder_choices: Some(ImageChoices::default()),
            reported: Default::default(),
        };
        let mut draft = ScanDraft { device: scanner.id.clone(), source: Source::Glass, pages: 0, ..ScanDraft::default() };
        draft.snapped_device.clear();
        snap_choices(&mut draft, &scanner, &glass);
        assert_eq!(draft.pages, 1, "the bed is always one page");
    }

    /// Regression probe for the "only the first sheet from the feeder" report: with Source::Auto,
    /// an empty/not-ready feeder makes the draft resolve to the bed, `snap_choices` clamps
    /// "All pages" (0) to 1, and that clamp is sticky when the feeder later becomes ready.
    #[test]
    fn auto_source_does_not_keep_one_page_after_the_feeder_becomes_ready() {
        use super::{effective_source, final_settings, snap_choices};
        use pdfcraft_platform::scan::{CancelToken, Connection, ImageChoices, Scanner, ValueSpan};

        let choices = ImageChoices { dpi: vec![300], ..ImageChoices::default() };
        let mut scanner = Scanner {
            id: "wsd-1".into(),
            name: "Network scanner (WSD)".into(),
            feeder: true,
            flatbed: true,
            duplex: false,
            feeder_ready: Some(false),
            handling_status: Default::default(),
            feeder_capacity: None,
            connection: Connection::Wsd,
            port: None,
            optical_x: None,
            optical_y: None,
            pages: Some(ValueSpan { min: 1, max: 30, step: 1 }),
            bed: None,
            sheet: None,
            glass: Some(choices.clone()),
            feeder_choices: Some(choices.clone()),
            reported: Default::default(),
        };
        let mut draft = ScanDraft { device: scanner.id.clone(), ..ScanDraft::default() };
        assert_eq!((draft.source, draft.pages), (Source::Auto, 0));
        snap_choices(&mut draft, &scanner, &choices);
        assert_eq!(effective_source(&draft, &scanner), Source::Glass);
        // Paper is loaded and the scanner list refreshes.
        scanner.feeder_ready = Some(true);
        snap_choices(&mut draft, &scanner, &choices);
        assert_eq!(effective_source(&draft, &scanner), Source::Feeder);
        let settings = final_settings(&draft, &scanner, CancelToken::new());
        assert_eq!(settings.pages, 0, "whole feeder expected; got {} page(s) for source Auto -> feeder", settings.pages);
    }

    #[test]
    fn final_settings_passes_feeder_all_pages_to_acquire() {
        use super::final_settings;
        use pdfcraft_platform::scan::{CancelToken, Connection, ImageChoices, Scanner, ValueSpan};

        let scanner = Scanner {
            id: "wsd-1".into(),
            name: "Network scanner".into(),
            feeder: true,
            flatbed: true,
            duplex: false,
            feeder_ready: Some(true),
            handling_status: Default::default(),
            feeder_capacity: Some(5),
            connection: Connection::Wsd,
            port: None,
            optical_x: None,
            optical_y: None,
            pages: Some(ValueSpan { min: 1, max: 30, step: 1 }),
            bed: None,
            sheet: None,
            glass: Some(ImageChoices::default()),
            feeder_choices: Some(ImageChoices::default()),
            reported: Default::default(),
        };
        let draft = ScanDraft { device: scanner.id.clone(), source: Source::Feeder, pages: 0, ..ScanDraft::default() };
        let settings = final_settings(&draft, &scanner, CancelToken::new());
        assert_eq!(settings.pages, 0);
        assert_eq!(settings.source, Source::Feeder);
    }

    #[test]
    fn text_ocr_picks_300_dpi_gray_and_clears_the_region() {
        use pdfcraft_platform::scan::{Color, ImageChoices, Rotation};

        let mut draft = ScanDraft {
            dpi: 1200,
            color: Color::Rgb,
            brightness: Some(20),
            contrast: Some(-10),
            selection: Some((0, 0, 10, 10)),
            width_in: 2.0,
            height_in: 1.0,
            ..ScanDraft::default()
        };
        let choices = ImageChoices {
            dpi: vec![100, 200, 300, 600],
            colors: vec![Color::Gray, Color::Rgb],
            brightness: Some(pdfcraft_platform::scan::ValueSpan { min: -50, max: 50, step: 1 }),
            contrast: Some(pdfcraft_platform::scan::ValueSpan { min: -50, max: 50, step: 1 }),
            threshold: None,
            rotation: vec![Rotation::Portrait, Rotation::Landscape],
            intents: vec![pdfcraft_platform::scan::Intent::Text],
            ..ImageChoices::default()
        };
        draft.apply_text_ocr(&choices);
        assert_eq!(draft.dpi, 300);
        assert_eq!(draft.color, Color::Gray);
        assert_eq!(draft.intent, Some(pdfcraft_platform::scan::Intent::Text));
        assert_eq!(draft.brightness, Some(0));
        assert_eq!(draft.contrast, Some(0));
        assert_eq!(draft.rotation, Some(Rotation::Portrait));
        assert!(draft.selection.is_none());
        assert!(draft.region().is_none());
    }

    #[test]
    fn color_document_photo_and_black_and_white_presets() {
        use pdfcraft_platform::scan::{Color, ImageChoices, Intent, Rotation, ValueSpan};

        let choices = ImageChoices {
            dpi: vec![100, 200, 300, 600, 1200],
            colors: vec![Color::BlackAndWhite, Color::Gray, Color::Rgb],
            brightness: Some(ValueSpan { min: -50, max: 50, step: 1 }),
            contrast: Some(ValueSpan { min: -50, max: 50, step: 1 }),
            threshold: Some(ValueSpan { min: 0, max: 255, step: 1 }),
            rotation: vec![Rotation::Portrait],
            intents: vec![Intent::Color, Intent::Gray, Intent::Text, Intent::MaximizeQuality],
            ..ImageChoices::default()
        };
        let mut draft = ScanDraft { dpi: 100, color: Color::Gray, ..ScanDraft::default() };
        draft.apply_color_document(&choices);
        assert_eq!(draft.dpi, 300);
        assert_eq!(draft.color, Color::Rgb);
        assert_eq!(draft.intent, Some(Intent::Color));

        draft.apply_photo(&choices);
        assert_eq!(draft.dpi, 600);
        assert_eq!(draft.color, Color::Rgb);
        assert_eq!(draft.intent, Some(Intent::MaximizeQuality));

        draft.apply_black_and_white(&choices);
        assert_eq!(draft.dpi, 300);
        assert_eq!(draft.color, Color::BlackAndWhite);
        assert_eq!(draft.intent, Some(Intent::Text));
        assert_eq!(draft.threshold, Some(128));
    }

    #[test]
    fn advanced_settings_do_not_overwrite_the_main_dpi_and_source() {
        use super::apply_advanced;
        let mut main = ScanDraft { source: Source::Feeder, dpi: 300, color: pdfcraft_platform::scan::Color::Gray, pages: 2, ..ScanDraft::default() };
        let advanced = ScanDraft {
            source: Source::Glass,
            dpi: 600,
            color: pdfcraft_platform::scan::Color::Rgb,
            pages: 1,
            brightness: Some(10),
            format: Some("jpeg".into()),
            invert: Some(true),
            ..ScanDraft::default()
        };
        apply_advanced(&advanced, &mut main);
        assert_eq!(main.source, Source::Feeder);
        assert_eq!(main.dpi, 300);
        assert_eq!(main.color, pdfcraft_platform::scan::Color::Gray);
        assert_eq!(main.pages, 2);
        assert_eq!(main.brightness, Some(10));
        assert_eq!(main.format.as_deref(), Some("jpeg"));
        assert_eq!(main.invert, Some(true));
    }

    #[test]
    fn turn_point_rotates_around_the_page_centre() {
        let c = Pos2::new(10.0, 10.0);
        let top = Pos2::new(10.0, 0.0);
        assert_eq!(super::turn_point(top, c, pdfcraft_platform::scan::Rotation::Portrait), top);
        assert_eq!(super::turn_point(top, c, pdfcraft_platform::scan::Rotation::Landscape), Pos2::new(20.0, 10.0));
        assert_eq!(super::turn_point(top, c, pdfcraft_platform::scan::Rotation::UpsideDown), Pos2::new(10.0, 20.0));
        assert_eq!(super::turn_point(top, c, pdfcraft_platform::scan::Rotation::Left), Pos2::new(0.0, 10.0));
    }

    #[test]
    fn preview_follows_the_chosen_color() {
        use pdfcraft_platform::scan::{Color, ImageChoices, Intent, Scanner};

        let scanner = Scanner {
            id: "test".into(),
            name: "test".into(),
            feeder: false,
            flatbed: true,
            duplex: false,
            feeder_ready: None,
            handling_status: Default::default(),
            feeder_capacity: None,
            connection: pdfcraft_platform::scan::Connection::Usb,
            port: Some(r"\\.\Usbscan0".into()),
            optical_x: Some(1200),
            optical_y: Some(1200),
            pages: None,
            bed: Some((8500, 11000)),
            sheet: None,
            glass: Some(ImageChoices {
                dpi: vec![100, 150, 200, 300],
                colors: vec![Color::BlackAndWhite, Color::Gray, Color::Rgb],
                intents: vec![Intent::Color, Intent::Gray, Intent::Text, Intent::BestPreview],
                ..ImageChoices::default()
            }),
            feeder_choices: None,
            reported: Default::default(),
        };
        let mut draft = ScanDraft { color: Color::Rgb, ..ScanDraft::default() };
        let settings = super::preview_settings(&draft, &scanner);
        assert_eq!(settings.color, Color::Rgb);
        assert_eq!(settings.source, pdfcraft_platform::scan::Source::Glass);
        assert_eq!(settings.dpi, 100);
        draft.color = Color::BlackAndWhite;
        let bw = super::preview_settings(&draft, &scanner);
        assert_eq!(bw.color, Color::BlackAndWhite);
    }

    #[test]
    fn scan_bar_uses_page_buffer_when_total_is_unknown_or_one() {
        assert_eq!(super::scan_bar_fraction(0, None, None), None);
        assert_eq!(super::scan_bar_fraction(3, None, None), None, "finished pages alone are not a made-up %");
        assert_eq!(super::scan_bar_fraction(0, None, Some(42)), Some(0.42));
        assert_eq!(super::scan_bar_fraction(0, Some(1), Some(75)), Some(0.75));
        assert!((super::scan_bar_fraction(1, Some(4), Some(50)).unwrap() - 0.375).abs() < 1e-4);
    }

    #[test]
    fn switching_source_clears_the_preview_and_the_region() {
        let mut draft = ScanDraft::default();
        assert_eq!(draft.source, Source::Auto);
        draft.preview = Some(PreviewImage { width: 200, height: 100, dpi: 100, rgba: Vec::new() });
        draft.selection = Some((0, 0, 200, 100));
        assert!(draft.region().is_some_and(|region| region.left == 0 && region.top == 0 && region.width >= 2000 && region.height >= 1000));
        draft.set_source(Source::Feeder);
        assert!(draft.preview.is_none());
        assert!(draft.selection.is_none());
        assert!(draft.region().is_none());
        assert_eq!(draft.source, Source::Feeder);
        draft.left_in = 1.0;
        draft.top_in = 0.0;
        draft.width_in = 2.0;
        draft.height_in = 1.0;
        assert!(draft.region().is_some());
        draft.set_source(Source::Glass);
        assert!(draft.region().is_none());
    }

    #[test]
    fn a_one_pixel_bmp_decodes_with_the_top_at_row_zero() {
        let mut bytes = vec![0u8; 58];
        bytes[0] = b'B';
        bytes[1] = b'M';
        bytes[10] = 54;
        bytes[14] = 40;
        bytes[18] = 1;
        bytes[22] = 1;
        bytes[26] = 1;
        bytes[28] = 24;
        bytes[54] = 0;
        bytes[55] = 0;
        bytes[56] = 255;
        assert!(bmp_rgba(&bytes).is_some_and(|(width, height, rgba)| width == 1 && height == 1 && rgba == [255, 0, 0, 255]));
    }

    /// A BMP with a 4-colour palette (black, red, green, blue) at indices 0..4.
    fn palette_bmp(bits: u16, compression: u32, width: i32, height: i32, body: &[u8]) -> Vec<u8> {
        let colours = 1usize << bits;
        let pixel_at = 54 + colours * 4;
        let mut bytes = vec![0u8; pixel_at];
        bytes[0] = b'B';
        bytes[1] = b'M';
        bytes[10..14].copy_from_slice(&u32::try_from(pixel_at).unwrap_or(0).to_le_bytes());
        bytes[14] = 40;
        bytes[18..22].copy_from_slice(&width.to_le_bytes());
        bytes[22..26].copy_from_slice(&height.to_le_bytes());
        bytes[26] = 1;
        bytes[28..30].copy_from_slice(&bits.to_le_bytes());
        bytes[30..34].copy_from_slice(&compression.to_le_bytes());
        // BGRA palette entries.
        for (i, bgr) in [[0u8, 0, 0], [0, 0, 255], [0, 255, 0], [255, 0, 0]].iter().enumerate() {
            bytes[54 + i * 4..54 + i * 4 + 3].copy_from_slice(bgr);
        }
        bytes.extend_from_slice(body);
        bytes
    }

    const BLACK: [u8; 4] = [0, 0, 0, 255];
    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// The pixels of an RGBA buffer as four-byte groups.
    fn pixels(rgba: &[u8]) -> Vec<[u8; 4]> {
        rgba.chunks(4).map(|px| [px[0], px[1], px[2], px[3]]).collect()
    }

    #[test]
    fn a_four_bit_palette_bmp_decodes_both_nibbles_and_row_order() {
        // 3x2, bottom-up: the first stored row is the bottom one. Rows pad to 4 bytes.
        let bottom = [0x12, 0x30, 0, 0]; // indices 1,2,3
        let top = [0x01, 0x20, 0, 0]; // indices 0,1,2
        let body = [bottom, top].concat();
        let (width, height, rgba) = bmp_rgba(&palette_bmp(4, 0, 3, 2, &body)).expect("4-bit");
        assert_eq!((width, height), (3, 2));
        assert_eq!(pixels(&rgba), vec![BLACK, RED, GREEN, RED, GREEN, BLUE]);
        // The same data top-down (negative height) keeps the stored order.
        let (_, _, down) = bmp_rgba(&palette_bmp(4, 0, 3, -2, &body)).expect("top-down");
        assert_eq!(pixels(&down), vec![RED, GREEN, BLUE, BLACK, RED, GREEN]);
    }

    #[test]
    fn an_eight_bit_palette_bmp_still_decodes() {
        let body = [1, 2, 3, 0, 0, 1, 0, 0];
        let (_, _, rgba) = bmp_rgba(&palette_bmp(8, 0, 3, 2, &body)).expect("8-bit");
        assert_eq!(pixels(&rgba), vec![BLACK, RED, BLACK, RED, GREEN, BLUE]);
    }

    #[test]
    fn rle8_bitmaps_decode_runs_absolute_runs_and_deltas() {
        // 4x2 bottom-up. Bottom row: a run of four index-1 pixels. Top row: an absolute run of
        // three (padded to 4 bytes), then one encoded pixel.
        let body = [4, 1, 0, 0, 0, 3, 2, 3, 1, 0, 1, 2, 0, 0, 0, 1];
        let (_, _, rgba) = bmp_rgba(&palette_bmp(8, 1, 4, 2, &body)).expect("rle8");
        assert_eq!(pixels(&rgba), vec![GREEN, BLUE, RED, GREEN, RED, RED, RED, RED]);
        // A delta skips one pixel right and leaves it at index 0: 3x1.
        let skip = [0, 2, 1, 0, 1, 3, 0, 1];
        let (_, _, rgba) = bmp_rgba(&palette_bmp(8, 1, 3, 1, &skip)).expect("delta");
        assert_eq!(pixels(&rgba), vec![BLACK, BLUE, BLACK]);
    }

    #[test]
    fn rle4_bitmaps_decode_nibble_runs() {
        // 4x1: an encoded run of four pixels alternating indices 1 and 2.
        let (_, _, rgba) = bmp_rgba(&palette_bmp(4, 2, 4, 1, &[4, 0x12, 0, 1])).expect("rle4");
        assert_eq!(pixels(&rgba), vec![RED, GREEN, RED, GREEN]);
        // An absolute run of three nibbles (two bytes, already even): indices 3,2,1.
        let (_, _, rgba) = bmp_rgba(&palette_bmp(4, 2, 3, 1, &[0, 3, 0x32, 0x10, 0, 1])).expect("rle4 absolute");
        assert_eq!(pixels(&rgba), vec![BLUE, GREEN, RED]);
    }

    #[test]
    fn unsupported_or_broken_bmps_fall_back_without_panicking() {
        // Bit fields, JPEG and PNG payloads, 16 bits, and RLE on the wrong depth.
        for (bits, compression) in [(32u16, 3u32), (24, 4), (24, 5), (16, 0), (4, 1), (8, 2), (1, 1)] {
            let mut bytes = palette_bmp(4, 0, 2, 2, &[0; 8]);
            bytes[28..30].copy_from_slice(&bits.to_le_bytes());
            bytes[30..34].copy_from_slice(&compression.to_le_bytes());
            assert!(bmp_rgba(&bytes).is_none(), "{bits} bits, compression {compression}");
        }
        // A truncated RLE stream, a truncated absolute run, and a truncated plain body.
        assert!(bmp_rgba(&palette_bmp(8, 1, 4, 1, &[4])).is_none());
        assert!(bmp_rgba(&palette_bmp(8, 1, 4, 1, &[0, 4, 1, 2])).is_none());
        assert!(bmp_rgba(&palette_bmp(4, 0, 8, 2, &[0x11])).is_none());
        // A run past the right edge is clipped, not an error.
        let (_, _, rgba) = bmp_rgba(&palette_bmp(8, 1, 2, 1, &[5, 1, 0, 1])).expect("clipped");
        assert_eq!(pixels(&rgba), vec![RED, RED]);
        // Junk and a short buffer.
        assert!(bmp_rgba(b"BM").is_none());
        assert!(bmp_rgba(&[]).is_none());
    }

    #[test]
    fn hostile_bmp_headers_neither_allocate_absurdly_nor_panic() {
        let started = std::time::Instant::now();
        // 100000 x 100000 pixels claimed by a file of a few hundred bytes, every depth, both row orders, RLE too.
        for (bits, compression) in [(4u16, 0u32), (8, 0), (4, 2), (8, 1)] {
            for height in [100_000, -100_000] {
                assert!(bmp_rgba(&palette_bmp(bits, compression, 100_000, height, &[0; 32])).is_none(), "{bits} bits {height}");
            }
        }
        let mut huge = palette_bmp(8, 0, 2, 1, &[1, 2, 0, 0]);
        huge[18..22].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(bmp_rgba(&huge).is_none());
        // A palette count of four billion is clamped to what 8 bits can hold; the picture still decodes.
        let mut counted = palette_bmp(8, 0, 2, 1, &[1, 2, 0, 0]);
        counted[46..50].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(bmp_rgba(&counted).map(|(_, _, rgba)| pixels(&rgba)), Some(vec![RED, GREEN]));
        // bfOffBits of 0 means "after the palette"; one inside the headers is refused.
        let mut zero = palette_bmp(8, 0, 2, 1, &[1, 2, 0, 0]);
        zero[10..14].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(bmp_rgba(&zero).map(|(_, _, rgba)| pixels(&rgba)), Some(vec![RED, GREEN]));
        zero[10..14].copy_from_slice(&20u32.to_le_bytes());
        assert!(bmp_rgba(&zero).is_none());
        // An index past the stored table paints black instead of dropping the preview.
        let past = palette_bmp(8, 0, 2, 1, &[200, 1, 0, 0]);
        assert_eq!(bmp_rgba(&past).map(|(_, _, rgba)| pixels(&rgba)), Some(vec![BLACK, RED]));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    /// A 2x2 24-bit bitmap as a scanned page.
    fn tiny_bmp_page() -> pdfcraft_platform::scan::Page {
        // 2x2 24-bit BMP (rows padded to 8 bytes).
        let mut bytes = vec![0u8; 54 + 16];
        bytes[0] = b'B';
        bytes[1] = b'M';
        bytes[2] = 70;
        bytes[10] = 54;
        bytes[14] = 40;
        bytes[18] = 2;
        bytes[22] = 2;
        bytes[26] = 1;
        bytes[28] = 24;
        bytes[34] = 16;
        pdfcraft_platform::scan::Page {
            bytes,
            format: "bmp",
            width: 2,
            height: 2,
            dpi: 300,
            dpi_y: 300,
            requested_dpi: 300,
            thresholded: false,
            length: Default::default(),
        }
    }

    /// GUI bookkeeping: pages that stream in over several poll batches all land in the one
    /// document opened by the first batch.
    #[test]
    fn create_keeps_every_page_across_poll_batches() {
        for batches in [vec![3usize], vec![1, 1, 1], vec![1, 2], vec![2, 1], vec![1, 1, 1, 1, 1]] {
            let mut app = crate::PdfCraftApp::new();
            let (mut doc, mut in_doc, mut index) = (None, 0u32, 0u32);
            for n in &batches {
                let pages: Vec<_> = (0..*n).map(|_| tiny_bmp_page()).collect();
                let state = app.apply_create_pages(pages, doc, in_doc, index, None);
                (doc, in_doc, index) = (state.create_doc, state.pages_in_create, state.create_file_index);
            }
            let total: usize = batches.iter().sum();
            assert_eq!(app.views.len(), 1, "batches {batches:?} opened extra documents");
            let id = doc.expect("create doc");
            let count = app.session.get(id).map(|d| d.info.pages.len());
            assert_eq!(count, Some(total), "batches {batches:?}");
            assert_eq!(in_doc as usize, total);
        }
    }

    #[test]
    fn create_file_names_and_remaining_capacity() {
        assert_eq!(super::scan_create_name(0), "Scan.pdf");
        assert_eq!(super::scan_create_name(1), "Scan (2).pdf");
        assert_eq!(super::scan_create_name(2), "Scan (3).pdf");
        assert_eq!(super::pages_remaining(3, 0, false), 3);
        assert_eq!(super::pages_remaining(3, 2, true), 1);
        assert_eq!(super::pages_remaining(3, 3, true), 0);
        assert_eq!(super::pages_remaining(1, 0, true), 1);
    }

    #[test]
    fn job_follows_the_source_the_engine_used() {
        let mut job = super::ScanJob { duplex_source: Some(Source::Feeder), ..test_job(ScanKind::Create, 0, None) };
        job.adopt_source(Source::Auto);
        assert_eq!((job.source, job.duplex_source), (Source::Feeder, Some(Source::Feeder)));
        job.adopt_source(Source::Feeder);
        assert_eq!(job.expected, None);
        // Auto picked the feeder, the engine fell back to the bed: one page, bed source everywhere.
        job.adopt_source(Source::Glass);
        assert_eq!((job.source, job.duplex_source), (Source::Glass, Some(Source::Glass)));
        assert_eq!((job.expected, job.expected_total), (Some(1), Some(1)));
        // A single direct job keeps no duplex source.
        let mut direct = test_job(ScanKind::Create, 2, Some(5));
        direct.adopt_source(Source::Glass);
        assert_eq!((direct.source, direct.duplex_source, direct.expected_total), (Source::Glass, None, Some(3)));
    }

    #[test]
    fn multiple_files_defaults_off() {
        assert_eq!(ScanDraft::default().pages_per_file, 0);
    }

    // ---- Scan interrupted: pure state machine, no scanner ----

    use super::{InterruptAction as A, InterruptKind as K, Pass, ScanKind};
    use pdfcraft_engine::DocId;

    /// A job of `kind` that has received `received` pages out of `expected_total`, on a feeder.
    fn test_job(kind: ScanKind, received: u32, expected_total: Option<u32>) -> super::ScanJob {
        super::ScanJob {
            kind,
            create_doc: None,
            pages_in_create: 0,
            create_file_index: 0,
            pages_per_file: None,
            expected: expected_total,
            duplex_source: None,
            received,
            expected_total,
            insert_cursor: None,
            source: Source::Feeder,
            requested_source: Source::Auto,
            device_name: "Fake WSD scanner (ADF tray 5, letter)".to_string(),
            two_sided: false,
            continues: false,
            live: pdfcraft_platform::scan::ProgressToken::new(),
            progress: std::sync::Arc::new(std::sync::Mutex::new(super::JobSlot { result: None, error_kind: None })),
            cancel: pdfcraft_platform::scan::CancelToken::new(),
        }
    }

    /// Give the job a failed result with `error`, make it the app's current job and poll it.
    fn finish_with(app: &mut crate::PdfCraftApp, job: super::ScanJob, error: &str) {
        if let Ok(mut slot) = job.progress.lock() {
            slot.result = Some(Err(error.to_string()));
        }
        app.scan.job = Some(job);
        app.poll_scan();
    }

    #[test]
    fn create_interruption_keeps_the_document_and_asks_for_the_rest() {
        let mut job = test_job(ScanKind::Create, 2, Some(5));
        job.create_doc = Some(DocId(7));
        job.pages_in_create = 2;
        job.create_file_index = 1;
        job.pages_per_file = Some(4);
        let state = job.interruption("The scanner has a paper jam.");
        assert_eq!(state.kind, K::Jam);
        assert_eq!(state.remaining(), Some(3));
        assert_eq!(state.actions(), vec![A::Continue, A::Finish]);
        assert_eq!(state.click(), super::ScanClick::Create);
        let resume = state.resume();
        assert_eq!((resume.pages, resume.carried, resume.expected_total), (Some(3), 2, Some(5)));
        assert_eq!((resume.create_doc, resume.pages_in_create, resume.create_file_index, resume.pages_per_file), (Some(DocId(7)), 2, 1, Some(4)));
        assert_eq!(resume.source, Some(Source::Feeder), "pages came in, so the feeder stays the source");
        assert_eq!(state.count_line(), "2 of 5 pages were scanned.");
        assert_eq!(state.primary_label(), "Continue scanning");
    }

    #[test]
    fn insert_interruption_keeps_the_position() {
        let mut job = test_job(ScanKind::Insert(DocId(3)), 2, Some(4));
        job.insert_cursor = Some(6);
        let state = job.interruption("A cover on the scanner is open.");
        assert_eq!(state.kind, K::CoverOpen);
        assert_eq!(state.click(), super::ScanClick::Insert);
        let resume = state.resume();
        assert_eq!((resume.scan_kind, resume.insert_at, resume.pages), (ScanKind::Insert(DocId(3)), Some(6), Some(2)));
    }

    #[test]
    fn all_pages_interruption_has_no_expected_count() {
        let state = test_job(ScanKind::Create, 2, None).interruption("The feeder has no paper.");
        assert_eq!(state.kind, K::FeederEmpty);
        assert_eq!((state.expected, state.remaining()), (None, None));
        assert_eq!(state.actions(), vec![A::Continue, A::Finish]);
        assert_eq!(state.resume().pages, None, "the next leg asks for all the pages again");
        assert_eq!(state.count_line(), "2 pages were scanned.");
        assert_eq!(test_job(ScanKind::Create, 1, None).interruption("x").count_line(), "1 page was scanned.");
    }

    #[test]
    fn nothing_left_to_scan_offers_only_finish() {
        let state = test_job(ScanKind::Create, 3, Some(3)).interruption("The scanner is offline.");
        assert!(!state.can_continue());
        assert_eq!(state.actions(), vec![A::Finish]);
    }

    #[test]
    fn preflight_refusal_offers_try_again() {
        let mut app = crate::PdfCraftApp::new();
        finish_with(&mut app, test_job(ScanKind::Create, 0, Some(3)), "A cover on the scanner is open.");
        let state = app.scan.interrupted.clone().expect("interrupted");
        assert_eq!(app.dialog, Some(crate::Dialog::ScanInterrupted));
        assert_eq!(state.actions(), vec![A::TryAgain, A::Finish]);
        assert_eq!(state.primary_label(), "Try again");
        assert_eq!(state.instruction_text(), "Close the feeder cover and check the paper, then choose Try again.");
        assert_eq!(state.count_line(), "No pages were scanned.");
        assert!(state.kept_line().is_none());
        assert_eq!(state.resume().pages, Some(3), "start again with the original count");
        assert_eq!(state.resume().source, None, "nothing came in, so Auto stays Auto");
    }

    /// Every text the Scan interrupted dialog draws, from a headless egui run.
    fn dialog_texts(app: &mut crate::PdfCraftApp) -> Vec<String> {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let t = crate::theme::Tokens::get(ui.ctx());
            let _ = super::interrupted_body(ui, app, &t);
        });
        let texts = output
            .shapes
            .iter()
            .filter_map(|shape| if let egui::Shape::Text(text) = &shape.shape { Some(text.galley.job.text.clone()) } else { None })
            .collect();
        output.drop_without_applying_deltas();
        texts
    }

    #[test]
    fn the_interrupted_dialog_names_the_scanner_and_the_source_but_never_an_id() {
        let mut app = crate::PdfCraftApp::new();
        let mut job = test_job(ScanKind::Insert(DocId(3)), 0, Some(5));
        job.requested_source = Source::Auto;
        job.live.set_source(Source::Glass);
        job.device_name = "Fake WSD scanner (ADF tray 5, letter)".to_string();
        // The device id the draft holds must never be drawn.
        app.scan.draft.device = "wia:{6BDD1FC6-810F-11D0-BEC7-08002BE2092F}\\0005".to_string();
        finish_with(&mut app, job, "The scanner did not accept the page size setting (The parameter is incorrect.)");
        let state = app.scan.interrupted.clone().expect("interrupted");
        assert_eq!(state.device_line(), "Fake WSD scanner (ADF tray 5, letter) · Auto → glass");
        let shown = dialog_texts(&mut app);
        assert!(shown.iter().any(|text| text == "Fake WSD scanner (ADF tray 5, letter) · Auto → glass"), "{shown:?}");
        let all = shown.join(" | ");
        for secret in ["6BDD1FC6", "wia:", "urn:uuid", "0005", "ScannerService", "127.0.0.1"] {
            assert!(!all.contains(secret), "{secret} in {all}");
        }
    }

    #[test]
    fn the_source_line_says_auto_until_the_scanner_reported_where_auto_went() {
        let mut state = test_job(ScanKind::Create, 0, Some(1)).interruption("x");
        state.requested_source = Source::Auto;
        state.resolved_source = None;
        assert!(state.device_line().ends_with("· Auto"), "{}", state.device_line());
        state.resolved_source = Some(Source::Feeder);
        assert!(state.device_line().ends_with("· Auto → feeder"));
        state.requested_source = Source::Feeder;
        assert!(state.device_line().ends_with("· Feeder"));
        state.requested_source = Source::Glass;
        assert!(state.device_line().ends_with("· Glass"));
        state.device_name = String::new();
        assert_eq!(state.device_line(), "Glass");
    }

    #[test]
    fn each_kind_selects_its_instruction() {
        let say = |kind, retry| super::instruction(kind, retry);
        assert!(say(K::CoverOpen, false).is_some_and(|text| text.starts_with("Close the feeder cover") && text.ends_with("choose Continue.")));
        assert!(say(K::Jam, false).is_some_and(|text| text.starts_with("Clear the jam") && text.contains("squarely")));
        assert!(say(K::FeederEmpty, false).is_some_and(|text| text.starts_with("Load more sheets") && text.contains("Finish")));
        assert!(say(K::MultiFeed, false).is_some_and(|text| text.starts_with("Separate the sheets")));
        assert!(say(K::PaperProblem, false).is_some_and(|text| text.contains("any jam or sheets fed together") && !text.contains("Clear the jam")));
        assert!(say(K::Offline, false).is_some_and(|text| text.starts_with("Check the scanner is on")));
        assert!(say(K::Attention, false).is_some_and(|text| text.starts_with("Look at the scanner")));
        assert!(say(K::Busy, false).is_some_and(|text| text.starts_with("Wait for the scanner")));
        assert!(say(K::DeviceError, false).is_some_and(|text| text.starts_with("Switch the scanner off and on")));
        assert_eq!(say(K::Other, false), None);
        assert_eq!(say(K::Other, true), None);
        assert!(say(K::BusyOrFeederEmpty, false).is_some_and(|text| text.starts_with("Check that the feeder has paper")));
        let all = [
            K::CoverOpen,
            K::Jam,
            K::PaperProblem,
            K::FeederEmpty,
            K::MultiFeed,
            K::Offline,
            K::Attention,
            K::Busy,
            K::BusyOrFeederEmpty,
            K::DeviceError,
        ];
        for retry in [false, true] {
            let texts: Vec<_> = all.iter().filter_map(|kind| say(*kind, retry)).collect();
            assert_eq!(texts.len(), all.len());
            let distinct: std::collections::BTreeSet<_> = texts.iter().collect();
            assert_eq!(distinct.len(), all.len(), "every cause has its own instruction");
            assert!(texts.iter().all(|text| text.contains(if retry { "Try again" } else { "Continue" })));
        }
        // Other shows the engine's own sentence.
        let other = test_job(ScanKind::Create, 1, None).interruption("The scan stopped before all pages arrived.");
        assert_eq!(other.instruction_text(), "The scan stopped before all pages arrived.");
        // The engine's sentences reach the right kind end to end.
        for (message, kind) in [
            ("Only 1 of 3 pages were scanned: the feeder reported no more paper.", K::FeederEmpty),
            ("The feeder took more than one sheet.", K::MultiFeed),
            ("The scanner has a paper problem.", K::PaperProblem),
            ("The scanner needs attention. Check it, then try again.", K::Attention),
            ("The scanner is busy. Close any other program that is using it, or wait for the current scan to finish, then try again.", K::Busy),
            ("The scanner is busy, or the feeder has no paper.", K::BusyOrFeederEmpty),
        ] {
            assert_eq!(test_job(ScanKind::Create, 1, Some(3)).interruption(message).kind, kind, "{message}");
        }
        // The backend's own cause wins over the wording of its sentence.
        let lamp = test_job(ScanKind::Create, 1, Some(3)).interruption_of("The scanner lamp is off.", Some(K::DeviceError));
        assert_eq!(lamp.kind, K::DeviceError);
    }

    /// A 1-pixel-high BMP `w` pixels wide; at 72 dpi the page is `w` points wide, so `w` tags a page.
    fn tagged(w: u32) -> pdfcraft_platform::scan::Page {
        let row = (w * 3).div_ceil(4) * 4;
        let mut bytes = vec![0u8; 54 + row as usize];
        bytes[0] = b'B';
        bytes[1] = b'M';
        bytes[10] = 54;
        bytes[14] = 40;
        bytes[18..22].copy_from_slice(&w.to_le_bytes());
        bytes[22] = 1;
        bytes[26] = 1;
        bytes[28] = 24;
        bytes[34..38].copy_from_slice(&row.to_le_bytes());
        pdfcraft_platform::scan::Page {
            bytes,
            format: "bmp",
            width: w,
            height: 1,
            dpi: 72,
            dpi_y: 72,
            requested_dpi: 72,
            thresholded: false,
            length: Default::default(),
        }
    }

    /// The rounded widths of the pages of document `id`.
    fn widths(app: &crate::PdfCraftApp, id: DocId) -> Vec<u32> {
        app.session.get(id).map(|doc| doc.info.pages.iter().map(|page| page.width.round() as u32).collect()).unwrap_or_default()
    }

    /// A job of the front pass of a manual-duplex scan.
    fn fronts_job(kind: ScanKind, received: u32) -> super::ScanJob {
        let mut job = test_job(kind, received, None);
        job.duplex_source = Some(Source::Feeder);
        job
    }

    /// Fail the app's current job with `error` and poll it.
    fn fail_current(app: &mut crate::PdfCraftApp, error: &str) {
        if let Some(job) = &app.scan.job
            && let Ok(mut slot) = job.progress.lock()
        {
            slot.result = Some(Err(error.to_string()));
        }
        app.poll_scan();
    }

    /// Finish the app's current job with no further pages and poll it.
    fn succeed_current(app: &mut crate::PdfCraftApp) {
        if let Some(job) = &app.scan.job
            && let Ok(mut slot) = job.progress.lock()
        {
            slot.result = Some(Ok(Vec::new()));
        }
        app.poll_scan();
    }

    /// Scan three fronts (10 11 12) into a new document the way a live Create does; returns its id.
    fn create_three_fronts(app: &mut crate::PdfCraftApp) -> DocId {
        app.scan.draft.manual_duplex = true;
        app.scan.job = Some(fronts_job(ScanKind::Create, 0));
        app.apply_scanned_pages(ScanKind::Create, vec![tagged(10)]);
        app.apply_scanned_pages(ScanKind::Create, vec![tagged(11), tagged(12)]);
        app.scan.duplex.expect("fronts are noted").doc
    }

    /// The reverse-side pass over `backs` (in scan order), one page per poll like a live scan.
    fn scan_backs(app: &mut crate::PdfCraftApp, received: u32, backs: &[u32]) {
        app.scan.job = Some(test_job(ScanKind::ReverseSides, received, None));
        for back in backs {
            app.apply_scanned_pages(ScanKind::ReverseSides, vec![tagged(*back)]);
        }
    }

    /// Merge by the same rule the live path uses, on plain numbers.
    fn merged(mut doc: Vec<u32>, first: usize, fronts: usize, backs: &[u32], reversed: bool) -> Vec<u32> {
        for (index, back) in backs.iter().enumerate() {
            let at = super::back_position(first, fronts, index, reversed).expect("a front is left");
            doc.insert(at, *back);
        }
        doc
    }

    #[test]
    fn back_position_follows_the_flipped_stack_or_scan_order() {
        // Fronts 1 2 3 4; the stack was flipped as a whole: the backs of 4, 3, 2, 1 arrive.
        assert_eq!(merged(vec![1, 2, 3, 4], 0, 4, &[14, 13, 12, 11], true), vec![1, 11, 2, 12, 3, 13, 4, 14]);
        // Not flipped: the backs arrive in front order and each shifts the rest down.
        assert_eq!(merged(vec![1, 2, 3, 4], 0, 4, &[11, 12, 13, 14], false), vec![1, 11, 2, 12, 3, 13, 4, 14]);
        // Fewer backs than fronts: they belong to the LAST fronts.
        assert_eq!(merged(vec![1, 2, 3, 4], 0, 4, &[14, 13], true), vec![1, 2, 3, 13, 4, 14]);
        assert_eq!(merged(vec![1, 2, 3, 4], 0, 4, &[11, 12], false), vec![1, 11, 2, 12, 3, 4]);
        assert_eq!(merged(vec![1, 2], 0, 2, &[], true), vec![1, 2]);
        // Fronts that start part-way into the document.
        assert_eq!(merged(vec![7, 8, 1, 2, 9], 2, 2, &[12, 11], true), vec![7, 8, 1, 11, 2, 12, 9]);
        assert_eq!(super::back_position(0, 2, 0, true), Some(2));
        assert_eq!(super::back_position(0, 2, 1, true), Some(1));
        assert_eq!(super::back_position(0, 2, 2, true), None, "no front is left for a third back");
        assert_eq!(super::back_position(0, 0, 0, true), None);
        assert_eq!(super::reverse_pages(0, 0), 1, "never asks the scanner for zero pages");
        assert_eq!(super::reverse_pages(3, 1), 2);
    }

    #[test]
    fn fronts_enter_the_document_as_they_arrive() {
        let mut app = crate::PdfCraftApp::new();
        app.scan.draft.manual_duplex = true;
        app.scan.job = Some(fronts_job(ScanKind::Create, 0));
        app.apply_scanned_pages(ScanKind::Create, vec![tagged(10)]);
        let held = app.scan.duplex.expect("the first front is noted");
        assert_eq!((held.first, held.fronts, held.backs, held.source), (0, 1, 0, Source::Feeder));
        assert_eq!(widths(&app, held.doc), vec![10], "the document already shows the first front");
        app.apply_scanned_pages(ScanKind::Create, vec![tagged(11), tagged(12)]);
        assert_eq!(widths(&app, held.doc), vec![10, 11, 12]);
        assert_eq!(app.scan.duplex.map(|pending| pending.fronts), Some(3));
        assert_eq!(app.scan.job.as_ref().map(|job| job.received), Some(3));
    }

    #[test]
    fn the_first_pass_ending_opens_the_flip_step_and_keeps_the_fronts() {
        let mut app = crate::PdfCraftApp::new();
        let id = create_three_fronts(&mut app);
        app.dialog = None;
        succeed_current(&mut app);
        assert!(app.scan.job.is_none());
        assert_eq!(app.dialog, Some(crate::Dialog::Scanner));
        assert_eq!(app.scan.duplex.map(|pending| pending.fronts), Some(3));
        assert_eq!(widths(&app, id), vec![10, 11, 12]);
        // The user never scans the backs: the fronts stay, nothing is added or lost.
        app.cancel_reverse_sides();
        assert!(app.scan.duplex.is_none());
        assert_eq!(app.dialog, None);
        assert_eq!(widths(&app, id), vec![10, 11, 12]);
        assert!(app.scan.last_create_files >= 1);
    }

    /// A plain Create of `widths` from the feeder, Reverse sides not ticked, the way a live scan delivers it.
    fn create_without_reverse_sides(app: &mut crate::PdfCraftApp, widths: &[u32]) -> DocId {
        app.scan.draft.manual_duplex = false;
        app.scan.job = Some(test_job(ScanKind::Create, 0, None));
        for width in widths {
            app.apply_scanned_pages(ScanKind::Create, vec![tagged(*width)]);
        }
        let id = app.scan.duplex.expect("the pages are remembered as fronts").doc;
        succeed_current(app);
        id
    }

    #[test]
    fn a_scan_without_reverse_sides_ticked_still_offers_them_afterwards() {
        let mut app = crate::PdfCraftApp::new();
        let id = create_without_reverse_sides(&mut app, &[10, 11, 12]);
        assert!(app.scan.job.is_none());
        assert_eq!(app.dialog, Some(crate::Dialog::Scanner), "the dialog stays on the next step instead of closing");
        let pending = app.scan.duplex.expect("offered after the fact");
        assert_eq!((pending.fronts, pending.backs, pending.requested, pending.source), (3, 0, false, Source::Feeder));
        // The person turns the stack over and scans the backs: they land as if Reverse sides had been ticked.
        scan_backs(&mut app, 0, &[112, 111, 110]);
        assert_eq!(widths(&app, id), vec![10, 110, 11, 111, 12, 112]);
    }

    #[test]
    fn finish_keeps_every_page_and_closes() {
        let mut app = crate::PdfCraftApp::new();
        let id = create_without_reverse_sides(&mut app, &[10, 11]);
        app.cancel_reverse_sides();
        assert!(app.scan.duplex.is_none());
        assert_eq!(app.dialog, None);
        assert_eq!(widths(&app, id), vec![10, 11]);
    }

    #[test]
    fn closing_the_dialog_finishes_an_offer_but_keeps_a_requested_duplex_waiting() {
        let mut app = crate::PdfCraftApp::new();
        create_without_reverse_sides(&mut app, &[10]);
        app.dialog = None;
        app.scan_dialog_closed();
        assert!(app.scan.duplex.is_none(), "the offer does not come back next time");

        let mut app = crate::PdfCraftApp::new();
        create_three_fronts(&mut app);
        succeed_current(&mut app);
        app.dialog = None;
        app.scan_dialog_closed();
        assert!(app.scan.duplex.is_some_and(|pending| pending.requested), "the stack may be turned over with the dialog closed");
    }

    #[test]
    fn more_fronts_go_after_the_last_one_and_the_backs_still_find_them() {
        let mut app = crate::PdfCraftApp::new();
        let id = create_without_reverse_sides(&mut app, &[10, 11]);
        // Continue scanning fronts: the next feeder load is an insert into the same document as a fronts pass.
        app.scan.job = Some(fronts_job(ScanKind::Insert(id), 0));
        app.apply_scanned_pages(ScanKind::Insert(id), vec![tagged(12), tagged(13)]);
        succeed_current(&mut app);
        assert_eq!(widths(&app, id), vec![10, 11, 12, 13]);
        assert_eq!(app.scan.duplex.map(|pending| pending.fronts), Some(4));
        scan_backs(&mut app, 0, &[113, 112, 111, 110]);
        assert_eq!(widths(&app, id), vec![10, 110, 11, 111, 12, 112, 13, 113]);
    }

    #[test]
    fn a_duplex_unit_scan_is_offered_more_pages_and_finish_but_never_reverse_sides() {
        let mut app = crate::PdfCraftApp::new();
        // Two sheets through the duplex unit in one pass: front, back, front, back.
        app.scan.job = Some(super::ScanJob { two_sided: true, ..test_job(ScanKind::Create, 0, None) });
        app.apply_scanned_pages(ScanKind::Create, vec![tagged(10), tagged(110), tagged(11), tagged(111)]);
        let id = app.scan.duplex.expect("the pages are remembered").doc;
        succeed_current(&mut app);
        assert_eq!(app.dialog, Some(crate::Dialog::Scanner));
        let pending = app.scan.duplex.expect("offered more pages");
        assert!(pending.two_sided && !pending.requested);
        assert_eq!(pending.fronts, 4);
        // Reverse sides are refused: the backs are already in.
        assert!(!app.start_scan_with(super::ScanClick::ReverseSides, None));
        assert_eq!(widths(&app, id), vec![10, 110, 11, 111]);
        // Continue scanning: the next sheets, two-sided again, go after the last page wherever the selection is.
        if let Some(view) = app.views.iter_mut().find(|view| view.id == id) {
            view.insert_at = Some(0);
        }
        app.scan.job = Some(super::ScanJob { two_sided: true, continues: true, ..test_job(ScanKind::Insert(id), 0, None) });
        app.apply_scanned_pages(ScanKind::Insert(id), vec![tagged(12), tagged(112)]);
        succeed_current(&mut app);
        assert_eq!(widths(&app, id), vec![10, 110, 11, 111, 12, 112]);
        assert_eq!(app.scan.duplex.map(|pending| (pending.fronts, pending.two_sided)), Some((6, true)));
        app.cancel_reverse_sides();
        assert_eq!(app.dialog, None);
        assert_eq!(widths(&app, id), vec![10, 110, 11, 111, 12, 112]);
    }

    #[test]
    fn a_scan_split_into_several_files_is_not_offered_reverse_sides() {
        let mut app = crate::PdfCraftApp::new();
        app.scan.draft.pages_per_file = 1;
        app.scan.job = Some(test_job(ScanKind::Create, 0, None));
        app.apply_scanned_pages(ScanKind::Create, vec![tagged(10), tagged(11)]);
        assert!(app.scan.duplex.is_none(), "the pages are in two documents");
        succeed_current(&mut app);
        assert_eq!(app.dialog, None);
    }

    #[test]
    fn closing_the_document_cancels_the_reverse_sides_it_was_waiting_for() {
        let mut app = crate::PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let id = create_three_fronts(&mut app);
        succeed_current(&mut app);
        assert!(app.scan.duplex.is_some(), "the flip step waits for the reverse sides");
        let index = app.views.iter().position(|view| view.id == id).unwrap();
        app.close_tab(index);
        assert!(app.scan.duplex.is_none(), "nothing is left to continue into");
        assert!(app.scan.job.is_none());
    }

    #[test]
    fn closing_the_document_stops_the_reverse_pass_that_is_scanning_into_it() {
        let mut app = crate::PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let id = create_three_fronts(&mut app);
        app.scan.job = Some(test_job(ScanKind::ReverseSides, 0, Some(3)));
        app.apply_scanned_pages(ScanKind::ReverseSides, vec![tagged(112)]);
        let index = app.views.iter().position(|view| view.id == id).unwrap();
        app.close_tab(index);
        assert!(app.scan.job.as_ref().is_some_and(|job| job.cancel.is_requested()), "the scanner is told to stop");
        assert!(app.scan.duplex.is_none());
        // A back still in flight is dropped without a second message.
        app.scan.draft.error = None;
        app.apply_scanned_pages(ScanKind::ReverseSides, vec![tagged(111)]);
        assert!(app.scan.draft.error.is_none());
    }

    #[test]
    fn closing_another_document_leaves_the_reverse_sides_waiting() {
        let mut app = crate::PdfCraftApp::new();
        let id = create_three_fronts(&mut app);
        succeed_current(&mut app);
        app.scan_document_closed(pdfcraft_engine::DocId(u64::MAX));
        assert_eq!(app.scan.duplex.map(|pending| pending.doc), Some(id));
    }

    #[test]
    fn reverse_sides_land_after_their_fronts_as_they_arrive() {
        let mut app = crate::PdfCraftApp::new();
        let id = create_three_fronts(&mut app);
        app.scan.job = Some(test_job(ScanKind::ReverseSides, 0, Some(3)));
        app.apply_scanned_pages(ScanKind::ReverseSides, vec![tagged(112)]);
        assert_eq!(widths(&app, id), vec![10, 11, 12, 112], "the first back belongs to the last front");
        app.apply_scanned_pages(ScanKind::ReverseSides, vec![tagged(111), tagged(110)]);
        assert_eq!(widths(&app, id), vec![10, 110, 11, 111, 12, 112]);
        assert_eq!(app.scan.duplex.map(|pending| pending.backs), Some(3));
        succeed_current(&mut app);
        assert!(app.scan.duplex.is_none() && app.scan.interrupted.is_none());
        assert_eq!(app.dialog, None);
        assert_eq!(widths(&app, id), vec![10, 110, 11, 111, 12, 112]);
    }

    #[test]
    fn an_extra_reverse_side_is_not_inserted() {
        let mut app = crate::PdfCraftApp::new();
        let id = create_three_fronts(&mut app);
        scan_backs(&mut app, 0, &[112, 111, 110, 999]);
        assert_eq!(widths(&app, id), vec![10, 110, 11, 111, 12, 112]);
    }

    #[test]
    fn a_fronts_interruption_keeps_the_inserted_fronts() {
        let mut app = crate::PdfCraftApp::new();
        let id = create_three_fronts(&mut app);
        fail_current(&mut app, "The scanner has a paper jam.");
        let state = app.scan.interrupted.clone().expect("interrupted");
        assert_eq!((state.pass, state.received), (Pass::Fronts, 3));
        assert_eq!(state.actions(), vec![A::Continue, A::ReverseNow, A::Finish]);
        assert_eq!(app.dialog, Some(crate::Dialog::ScanInterrupted));
        assert_eq!(widths(&app, id), vec![10, 11, 12]);
        let resume = state.resume();
        assert_eq!((resume.carried, resume.create_doc), (3, Some(id)));
        // A resumed first pass carries on after the last front, wherever the selection moved to.
        app.scan.interrupted = None;
        app.scan.job = Some({
            let mut job = fronts_job(ScanKind::Create, 3);
            job.create_doc = Some(id);
            job.pages_in_create = 3;
            job
        });
        app.views.iter_mut().for_each(|view| view.select_pages(&[0]));
        app.apply_scanned_pages(ScanKind::Create, vec![tagged(13)]);
        assert_eq!(widths(&app, id), vec![10, 11, 12, 13]);
        assert_eq!(app.scan.duplex.map(|pending| (pending.first, pending.fronts)), Some((0, 4)));
    }

    #[test]
    fn finish_after_a_fronts_interruption_keeps_the_fronts_as_pages() {
        let mut app = crate::PdfCraftApp::new();
        let id = create_three_fronts(&mut app);
        fail_current(&mut app, "The feeder has no paper.");
        app.resolve_interrupt(A::Finish);
        assert!(app.scan.duplex.is_none() && app.scan.interrupted.is_none());
        assert_eq!(app.dialog, None);
        assert_eq!(widths(&app, id), vec![10, 11, 12]);
    }

    #[test]
    fn reverse_now_after_a_fronts_interruption_opens_the_flip_step() {
        let mut app = crate::PdfCraftApp::new();
        let id = create_three_fronts(&mut app);
        fail_current(&mut app, "The feeder has no paper.");
        app.resolve_interrupt(A::ReverseNow);
        assert_eq!(app.dialog, Some(crate::Dialog::Scanner));
        assert!(app.scan.interrupted.is_none());
        assert_eq!(app.scan.duplex.map(|pending| pending.fronts), Some(3));
        assert_eq!(widths(&app, id), vec![10, 11, 12]);
    }

    #[test]
    fn a_reverse_pass_interruption_keeps_the_backs_and_resumes_at_the_right_front() {
        let mut app = crate::PdfCraftApp::new();
        let id = create_three_fronts(&mut app);
        scan_backs(&mut app, 0, &[112]);
        if let Some(job) = &mut app.scan.job {
            job.expected_total = Some(3);
        }
        fail_current(&mut app, "The scanner has a paper jam.");
        let state = app.scan.interrupted.clone().expect("interrupted");
        assert_eq!((state.pass, state.received, state.remaining()), (Pass::Backs, 1, Some(2)));
        assert_eq!(state.actions(), vec![A::Continue, A::Finish]);
        assert_eq!(state.click(), super::ScanClick::ReverseSides);
        assert_eq!(state.resume().pages, Some(2));
        assert_eq!(app.scan.duplex.map(|pending| pending.backs), Some(1));
        assert_eq!(widths(&app, id), vec![10, 11, 12, 112]);
        // The next leg continues with the back of the second-to-last front.
        app.scan.interrupted = None;
        scan_backs(&mut app, 1, &[111, 110]);
        assert_eq!(widths(&app, id), vec![10, 110, 11, 111, 12, 112]);
        succeed_current(&mut app);
        assert!(app.scan.duplex.is_none());
    }

    #[test]
    fn finish_after_too_few_reverse_sides_pairs_the_last_fronts() {
        let mut app = crate::PdfCraftApp::new();
        let id = create_three_fronts(&mut app);
        scan_backs(&mut app, 0, &[112]);
        succeed_current(&mut app);
        let state = app.scan.interrupted.clone().expect("interrupted");
        assert_eq!((state.pass, state.received, state.remaining()), (Pass::Backs, 1, Some(2)));
        assert_eq!(state.kind, K::Other);
        assert!(state.instruction_text().contains("Manual duplex needs one reverse side for each front."));
        assert!(state.instruction_text().ends_with("Scan the missing reverse sides, then choose Finish."));
        assert_eq!(app.dialog, Some(crate::Dialog::ScanInterrupted));
        app.resolve_interrupt(A::Finish);
        assert!(app.scan.duplex.is_none());
        assert_eq!(app.dialog, None);
        assert_eq!(widths(&app, id), vec![10, 11, 12, 112]);
    }

    #[test]
    fn insert_puts_fronts_at_the_chosen_gap_and_backs_after_them() {
        let mut app = crate::PdfCraftApp::new();
        app.scan.draft.manual_duplex = true;
        let state = app.apply_create_pages(vec![tagged(1), tagged(2), tagged(3), tagged(4)], None, 0, 0, None);
        let id = state.create_doc.expect("document");
        assert_eq!(widths(&app, id), vec![1, 2, 3, 4]);
        if let Some(view) = app.views.iter_mut().find(|view| view.id == id) {
            view.insert_at = Some(2);
        }
        let cursor = app.insert_cursor_for(id);
        assert_eq!(cursor, Some(2));
        let mut job = fronts_job(ScanKind::Insert(id), 0);
        job.insert_cursor = cursor;
        app.scan.job = Some(job);
        app.apply_scanned_pages(ScanKind::Insert(id), vec![tagged(20)]);
        assert_eq!(widths(&app, id), vec![1, 2, 20, 3, 4]);
        app.apply_scanned_pages(ScanKind::Insert(id), vec![tagged(21), tagged(22)]);
        assert_eq!(widths(&app, id), vec![1, 2, 20, 21, 22, 3, 4]);
        let held = app.scan.duplex.expect("fronts noted");
        assert_eq!((held.doc, held.first, held.fronts, held.kind), (id, 2, 3, ScanKind::Insert(id)));
        succeed_current(&mut app);
        assert_eq!(app.dialog, Some(crate::Dialog::Scanner));
        scan_backs(&mut app, 0, &[122, 121, 120]);
        assert_eq!(widths(&app, id), vec![1, 2, 20, 120, 21, 121, 22, 122, 3, 4]);
        succeed_current(&mut app);
        assert!(app.scan.duplex.is_none());
        assert_eq!(widths(&app, id), vec![1, 2, 20, 120, 21, 121, 22, 122, 3, 4]);
    }

    #[test]
    fn a_resumed_insert_pass_continues_at_the_cursor() {
        let mut app = crate::PdfCraftApp::new();
        let state = app.apply_create_pages(vec![tagged(1), tagged(2)], None, 0, 0, None);
        let id = state.create_doc.expect("document");
        let mut job = fronts_job(ScanKind::Insert(id), 0);
        job.insert_cursor = Some(1);
        app.scan.job = Some(job);
        app.apply_scanned_pages(ScanKind::Insert(id), vec![tagged(20), tagged(21)]);
        fail_current(&mut app, "The scanner has a paper jam.");
        let state = app.scan.interrupted.clone().expect("interrupted");
        assert_eq!((state.pass, state.received, state.insert_at), (Pass::Fronts, 2, Some(3)));
        assert_eq!(widths(&app, id), vec![1, 20, 21, 2]);
        // Resume: the job starts from the interrupted cursor.
        let mut job = fronts_job(ScanKind::Insert(id), 2);
        job.insert_cursor = state.resume().insert_at;
        app.scan.job = Some(job);
        app.apply_scanned_pages(ScanKind::Insert(id), vec![tagged(22)]);
        assert_eq!(widths(&app, id), vec![1, 20, 21, 22, 2]);
        assert_eq!(app.scan.duplex.map(|pending| (pending.first, pending.fronts)), Some((1, 3)));
    }

    #[test]
    fn stopping_a_scan_is_not_an_interruption() {
        let mut app = crate::PdfCraftApp::new();
        let job = test_job(ScanKind::Create, 0, Some(3));
        job.cancel.request();
        finish_with(&mut app, job, "the scan was cancelled");
        assert!(app.scan.interrupted.is_none());
        assert_ne!(app.dialog, Some(crate::Dialog::ScanInterrupted));
        assert_eq!(app.scan.draft.error.as_deref(), Some("the scan was cancelled"));
    }

    #[test]
    fn a_continue_that_cannot_start_returns_to_the_dialog() {
        let mut app = crate::PdfCraftApp::new();
        finish_with(&mut app, test_job(ScanKind::Create, 1, Some(3)), "The scanner has a paper jam.");
        app.resolve_interrupt(A::Continue);
        // No scanner is listed in a test app, so the next acquire cannot start.
        assert!(app.scan.job.is_none());
        assert!(app.scan.interrupted.is_some());
        assert_eq!(app.dialog, Some(crate::Dialog::ScanInterrupted));
        assert!(app.scan.draft.error.is_some());
        app.resolve_interrupt(A::Finish);
        assert!(app.scan.interrupted.is_none());
        assert_eq!(app.dialog, None);
    }
}

#[cfg(test)]
mod host_tests {
    use super::*;

    /// Choices listing these resolutions and colours.
    fn choices(dpi: &[u32], colors: &[Color]) -> ImageChoices {
        ImageChoices { dpi: dpi.to_vec(), colors: colors.to_vec(), ..ImageChoices::default() }
    }

    /// A scanner with the given choices for the bed and the feeder; both sources are present.
    fn scanner(glass: Option<ImageChoices>, feeder: Option<ImageChoices>, duplex: bool) -> Scanner {
        Scanner { glass, feeder_choices: feeder, duplex, flatbed: true, feeder: true, ..fallback_scanner(&shared::Scanner::default()) }
    }

    #[test]
    fn the_shared_choices_are_the_union_of_what_the_bed_and_the_feeder_list() {
        let bed = choices(&[300, 100, 600], &[Color::Rgb, Color::Gray]);
        let feeder = choices(&[200, 300], &[Color::Gray, Color::BlackAndWhite]);
        let mapped = choices_of(&scanner(Some(bed), Some(feeder), true));
        assert!(mapped.feeder.is_supported() && mapped.glass.is_supported());
        assert_eq!(mapped.dpi.list(), [100, 200, 300, 600]);
        assert_eq!(mapped.colors.list(), [shared::Color::Color, shared::Color::Gray, shared::Color::BlackAndWhite]);
        assert!(mapped.duplex.is_supported() && mapped.all_pages.is_supported());
    }

    #[test]
    fn one_list_keeps_the_drivers_order_and_a_scanner_without_lists_is_unknown_not_unsupported() {
        let only = choices(&[600, 300], &[Color::Gray]);
        let mapped = choices_of(&scanner(Some(only), None, false));
        assert_eq!(mapped.dpi.list(), [600, 300]);
        let bare = choices_of(&scanner(None, None, false));
        assert!(bare.dpi == Capability::Unknown && bare.colors == Capability::Unknown, "enabled with a note, not disabled");
        assert!(bare.dpi.is_enabled() && bare.colors.is_enabled());
    }

    #[test]
    fn no_feeder_and_no_duplex_unit_are_unsupported_when_the_driver_was_read() {
        let mut s = scanner(Some(choices(&[300], &[Color::Gray])), None, false);
        s.feeder = false;
        let mapped = choices_of(&s);
        assert!(mapped.feeder.is_unsupported() && mapped.duplex.is_unsupported());
        assert!(mapped.glass.is_supported());
        // Auto then uses the glass.
        assert_eq!(mapped.resolve(shared::Source::Auto), shared::Source::Glass);
        let mut settings =
            shared::Settings { source: shared::Source::Feeder, pages: shared::Pages::Count(5), duplex: true, ..shared::Settings::default() };
        settings.clamp(&mapped);
        assert_eq!(settings.source, shared::Source::Auto);
        let request = mapped.request(&settings);
        assert_eq!(request.source.value(), Some(&shared::Source::Glass));
        assert!(request.duplex.value().is_none() && request.pages.value().is_none(), "nothing is sent for a unit that is not there");
    }

    #[test]
    fn a_scanner_with_nothing_present_means_the_bits_were_not_read_so_everything_is_unknown() {
        let mut s = scanner(None, None, false);
        s.feeder = false;
        s.flatbed = false;
        let mapped = choices_of(&s);
        assert_eq!((mapped.feeder, mapped.glass, mapped.duplex), (Capability::Unknown, Capability::Unknown, Capability::Unknown));
    }

    #[test]
    fn the_page_limit_follows_the_drivers_range_and_is_unknown_without_one() {
        let mut s = scanner(None, None, false);
        assert_eq!(choices_of(&s).max_pages, Capability::Unknown);
        s.pages = Some(ValueSpan { min: 0, max: 50, step: 1 });
        assert_eq!(choices_of(&s).max_pages, Capability::from_option_range(Some((1, 50))));
    }

    #[test]
    fn status_warns_and_never_changes_the_capabilities() {
        let mut s = scanner(Some(choices(&[300], &[Color::Gray])), Some(choices(&[300], &[Color::Gray])), true);
        let calm = choices_of(&s);
        s.feeder_ready = Some(false);
        s.handling_status.cover_open = true;
        assert_eq!(warnings_of(&s, Source::Feeder, false), vec![shared::Warning::FeederEmpty, shared::Warning::CoverOpen]);
        assert_eq!(warnings_of(&s, Source::Glass, false), vec![shared::Warning::CoverOpen], "an idle feeder is not a warning for the bed");
        s.feeder_ready = Some(true);
        s.handling_status.cover_open = false;
        assert_eq!(warnings_of(&s, Source::Auto, true), vec![shared::Warning::FeederReadyButEmpty]);
        assert!(warnings_of(&s, Source::Auto, false).is_empty());
        assert_eq!(choices_of(&s), calm, "status is not capability");
    }

    #[test]
    fn a_missing_page_range_is_not_clamped_to_one_page() {
        let s = scanner(Some(ImageChoices::default()), Some(ImageChoices::default()), false);
        let mut draft = ScanDraft { device: s.id.clone(), source: Source::Feeder, pages: 5, ..ScanDraft::default() };
        draft.snapped_device = "other".into();
        snap_choices(&mut draft, &s, &ImageChoices::default());
        assert_eq!(draft.pages, 5, "unknown is a hint, not a limit of one");
    }

    #[test]
    fn the_draft_and_the_shared_settings_round_trip() {
        let mut draft =
            ScanDraft { device: "a".into(), source: Source::Feeder, dpi: 200, color: Color::Rgb, pages: 3, duplex: true, ..ScanDraft::default() };
        let mut settings = shared::Settings::default();
        push_settings(&draft, &mut settings);
        assert_eq!(settings.pages, shared::Pages::Count(3));
        assert_eq!(settings.color, shared::Color::Color);
        settings.pages = shared::Pages::All;
        settings.dpi = 600;
        settings.color = shared::Color::BlackAndWhite;
        assert!(!pull_settings(&mut draft, &settings));
        assert_eq!((draft.pages, draft.dpi, draft.color, draft.duplex), (0, 600, Color::BlackAndWhite, true));
    }

    #[test]
    fn a_new_scanner_or_source_drops_the_preview_and_the_snap() {
        let mut draft =
            ScanDraft { device: "a".into(), preview: Some(PreviewImage { width: 1, height: 1, dpi: 75, rgba: vec![0; 4] }), ..ScanDraft::default() };
        let mut settings = shared::Settings { scanner: "b".into(), ..shared::Settings::default() };
        assert!(pull_settings(&mut draft, &settings));
        assert_eq!(draft.device, "b");
        assert!(draft.preview.is_none());
        settings.source = shared::Source::Glass;
        assert!(pull_settings(&mut draft, &settings));
        assert_eq!((draft.source, draft.pages), (Source::Glass, 1), "the bed takes one page");
    }

    #[test]
    fn listed_scanners_keep_their_connection() {
        let list = [ScannerRef::new("1".into(), "A".into(), Some("Usbscan0".into())), ScannerRef::new("2".into(), "A".into(), None)];
        let mapped = scanners_of(&list);
        assert_eq!(mapped[0].connection, shared::Connection::Usb);
        assert_eq!(mapped[1].connection, shared::Connection::Unknown);
    }
}
