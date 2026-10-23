//! Page length on a WIA item: what the item publishes about sheet-length detection, and what to do about a
//! [`PageLength`] request. No COM here, so it builds and is tested on every platform; the backend reads the item's
//! properties into a [`PropTable`] and applies the [`Decision`].
//!
//! Policy, in priority order (see `craft-scan`'s `length` module for the vocabulary):
//!
//! 1. **Device detection**, only when the item *publishes* it: `WIA_IPS_AUTO_CROP` accepting
//!    `WIA_AUTO_CROP_SINGLE`, the `WIA_IPS_PAGE_SIZE` list holding `WIA_PAGE_AUTO`, or `WIA_IPS_LONG_DOCUMENT`
//!    accepting `WIA_LONG_DOCUMENT_ENABLED`. Support is three-state: [`Presence::Absent`] needs a table that was
//!    actually read.
//! 2. **An explicit page size**: the page size is written when the item lists it (the existing page-size plan and
//!    its `E_INVALIDARG` tolerance); otherwise the length is written as the vertical extent, inside the item's
//!    published range. A size the item does not list is never written as an unlisted `WIA_IPS_PAGE_SIZE` value.
//!    A length outside the range (beyond one step) is a clear error, not a silent clamp.
//! 3. **Inference** ([`PageLength::AutoInfer`], feeder only): request the item's longest vertical extent and let the
//!    page builder cut at the sheet end.
//!
//! An explicit region always wins: it already says which area to scan.

use craft_scan::{PageLength, PageLengthSupport, PageSize, Presence, ValueSpan, align_to_span, mils_of, rows_for_mils};

/// `WIA_IPS_AUTO_CROP` (checked against the `windows` crate in the Windows build's tests).
pub const WIA_IPS_AUTO_CROP: u32 = 4170;
/// `WIA_IPS_LONG_DOCUMENT`.
pub const WIA_IPS_LONG_DOCUMENT: u32 = 4166;
/// `WIA_IPS_PAGE_SIZE`.
pub const WIA_IPS_PAGE_SIZE: u32 = 3097;
/// `WIA_IPS_YEXTENT`.
pub const WIA_IPS_YEXTENT: u32 = 6152;
/// `WIA_IPS_MAX_VERTICAL_SIZE` (thousandths of an inch).
pub const WIA_IPS_MAX_VERTICAL_SIZE: u32 = 6166;
/// `WIA_AUTO_CROP_SINGLE`.
pub const WIA_AUTO_CROP_SINGLE: i32 = 1;
/// `WIA_PAGE_AUTO`, a value of the page-size list.
pub const WIA_PAGE_AUTO: i32 = 100;
/// `WIA_LONG_DOCUMENT_ENABLED`.
pub const WIA_LONG_DOCUMENT_ENABLED: i32 = 1;

/// One property of an item as the driver publishes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prop {
    /// Only the current value, no list or range.
    Value {
        value: i32,
        writable: bool,
    },
    List {
        values: Vec<i32>,
        writable: bool,
    },
    Range {
        span: ValueSpan,
        writable: bool,
    },
}

impl Prop {
    /// Whether the driver allows writing this property.
    pub fn writable(&self) -> bool {
        match self {
            Self::Value { writable, .. } | Self::List { writable, .. } | Self::Range { writable, .. } => *writable,
        }
    }

    /// The driver offers `value`: writable, and listed (list), in range (range) or already current (value).
    pub fn accepts(&self, value: i32) -> bool {
        match self {
            Self::Value { value: current, writable } => *writable && *current == value,
            Self::List { values, writable } => *writable && values.contains(&value),
            Self::Range { span, writable } => *writable && span.contains(value),
        }
    }
}

/// The properties of one item the length policy cares about. `read` says the item's property storage was
/// actually read: only then does a missing property mean "the device does not publish it".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PropTable {
    read: bool,
    props: Vec<(u32, Prop)>,
}

impl PropTable {
    /// A table for an item whose property storage was read (`true`) or could not be (`false`).
    pub fn new(read: bool) -> Self {
        Self { read, props: Vec::new() }
    }

    /// Add the property to the table, replacing any earlier entry with the same id.
    pub fn with(mut self, id: u32, prop: Prop) -> Self {
        self.props.retain(|(known, _)| *known != id);
        self.props.push((id, prop));
        self
    }

    /// The property with this id, if the table has it.
    pub fn get(&self, id: u32) -> Option<&Prop> {
        self.props.iter().find(|(known, _)| *known == id).map(|(_, prop)| prop)
    }

    /// Whether the item's property storage was read. A missing property only means "not published" when this is
    /// true.
    pub fn was_read(&self) -> bool {
        self.read
    }
}

/// How an item can detect the sheet length itself, in the order they are preferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceDetect {
    /// Write `WIA_IPS_AUTO_CROP = WIA_AUTO_CROP_SINGLE`.
    AutoCrop,
    /// Write `WIA_IPS_PAGE_SIZE = WIA_PAGE_AUTO`.
    PageSizeAuto,
    /// Write `WIA_IPS_LONG_DOCUMENT = WIA_LONG_DOCUMENT_ENABLED`.
    LongDocument,
}

impl DeviceDetect {
    /// The short name used in logs and the page-length report.
    pub fn name(self) -> &'static str {
        match self {
            Self::AutoCrop => "auto-crop",
            Self::PageSizeAuto => "page-size-auto",
            Self::LongDocument => "long-document",
        }
    }

    /// The `(property, value)` the backend writes.
    pub fn write(self) -> (u32, i32) {
        match self {
            Self::AutoCrop => (WIA_IPS_AUTO_CROP, WIA_AUTO_CROP_SINGLE),
            Self::PageSizeAuto => (WIA_IPS_PAGE_SIZE, WIA_PAGE_AUTO),
            Self::LongDocument => (WIA_IPS_LONG_DOCUMENT, WIA_LONG_DOCUMENT_ENABLED),
        }
    }
}

/// The detection methods the item publishes, preferred first. Empty for an item that publishes none (or whose
/// table was not read).
pub fn device_methods(table: &PropTable) -> Vec<DeviceDetect> {
    [DeviceDetect::AutoCrop, DeviceDetect::PageSizeAuto, DeviceDetect::LongDocument]
        .into_iter()
        .filter(|method| {
            let (id, value) = method.write();
            table.get(id).is_some_and(|prop| prop.accepts(value))
        })
        .collect()
}

/// What the item says about page length: detection three-state, and the longest page it can scan.
pub fn support(table: &PropTable) -> PageLengthSupport {
    let device_detect = if !table.was_read() {
        Presence::Unknown
    } else if device_methods(table).is_empty() {
        Presence::Absent
    } else {
        Presence::Present
    };
    let max_length_mils = match table.get(WIA_IPS_MAX_VERTICAL_SIZE) {
        Some(Prop::Value { value, .. }) => u32::try_from(*value).ok().filter(|mils| *mils > 0),
        _ => None,
    };
    PageLengthSupport { device_detect, max_length_mils }
}

/// What a scan request asks of the length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub mode: PageLength,
    pub page_size: Option<PageSize>,
    /// The caller gave an explicit region.
    pub region: bool,
    /// The item that transfers is the feeder.
    pub feeder: bool,
}

/// What to do about the length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Leave the extent to the driver.
    Driver,
    /// Ask the device to detect it.
    Device(DeviceDetect),
    /// Fix the length of this named size: page size if listed, else the vertical extent.
    Size { size: PageSize, height_mils: u32 },
    /// Request the longest extent and cut at the sheet end.
    Infer,
}

/// An [`Action`] and the sentence that says why (for the log and the trace).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub action: Action,
    pub why: &'static str,
}

/// Apply the policy to one request and one item.
pub fn decide(request: &Request, table: &PropTable) -> Decision {
    let driver = |why| Decision { action: Action::Driver, why };
    if request.mode == PageLength::Driver {
        return driver("page length not requested");
    }
    if request.region {
        return driver("an explicit region sets the scan area");
    }
    if request.mode != PageLength::Size
        && let Some(method) = device_methods(table).first()
    {
        return Decision { action: Action::Device(*method), why: "the scanner publishes length detection" };
    }
    if let Some(size) = request.page_size
        && let Some((_, height_mils)) = size.extent_mils()
    {
        return Decision { action: Action::Size { size, height_mils }, why: "extents from the chosen page size" };
    }
    if request.mode == PageLength::Size {
        return driver("no page size that names a length was chosen");
    }
    if request.mode.may_infer() {
        return if request.feeder {
            Decision { action: Action::Infer, why: "no detection or page size: infer the sheet end from the image (approximate)" }
        } else {
            driver("inference applies to the feeder only")
        };
    }
    driver("the scanner publishes no length detection and no page size was chosen")
}

/// Why a length does not fit the item's vertical range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitError {
    /// Shorter than the item's smallest vertical extent (rows).
    TooShort { min_rows: u32 },
    /// Longer than the item's largest vertical extent (rows).
    TooLong { max_rows: u32 },
}

impl FitError {
    /// The sentence for a `ScanError`.
    pub fn sentence(self, requested_mils: u32, dpi_y: u32) -> String {
        let inches = |mils: u32| f64::from(mils) / 1000.0;
        match self {
            Self::TooShort { min_rows } => format!(
                "The scanner cannot scan a page as short as {:.2} in at {dpi_y} dpi (its shortest is {:.2} in).",
                inches(requested_mils),
                inches(mils_of(min_rows, dpi_y).unwrap_or(0))
            ),
            Self::TooLong { max_rows } => format!(
                "The scanner cannot scan a page {:.2} in long at {dpi_y} dpi (its longest is {:.2} in).",
                inches(requested_mils),
                inches(mils_of(max_rows, dpi_y).unwrap_or(0))
            ),
        }
    }
}

/// The vertical extent in rows for `height_mils` at `dpi_y`, inside the item's published range at that
/// resolution (`span`, in rows; `None` when no range is published, then the value is used as computed). A value
/// at most one step outside the range is snapped to its end; further out is an error.
pub fn fit_height(height_mils: u32, dpi_y: u32, span: Option<ValueSpan>) -> Result<u32, FitError> {
    let rows = rows_for_mils(height_mils, dpi_y);
    let Some(span) = span else { return Ok(rows) };
    let slack = u32::try_from(span.step.max(1)).unwrap_or(1);
    let (min, max) = (u32::try_from(span.min.max(1)).unwrap_or(1), u32::try_from(span.max.max(1)).unwrap_or(1));
    if rows.saturating_add(slack) < min {
        return Err(FitError::TooShort { min_rows: min });
    }
    if rows > max.saturating_add(slack) {
        return Err(FitError::TooLong { max_rows: max });
    }
    let fitted = align_to_span(i32::try_from(rows).unwrap_or(i32::MAX), span, true).map_err(|_| FitError::TooLong { max_rows: max })?;
    Ok(u32::try_from(fitted).unwrap_or(rows))
}

/// The longest extent to request for inference: the top of the item's vertical range (rows at the current
/// resolution), else the item's maximum vertical size converted at `dpi_y`. `None` when the item publishes neither.
pub fn longest_rows(span: Option<ValueSpan>, max_length_mils: Option<u32>, dpi_y: u32) -> Option<u32> {
    if let Some(span) = span
        && span.max > 0
    {
        return align_to_span(span.max, span, true).ok().and_then(|rows| u32::try_from(rows).ok());
    }
    max_length_mils.filter(|mils| *mils > 0).map(|mils| rows_for_mils(mils, dpi_y))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A feeder that lists only A4, with an adjustable vertical extent and a 14 in maximum: what a WIA driver that
    /// cannot detect the sheet length publishes.
    fn plain_feeder(dpi: u32) -> PropTable {
        let _ = dpi;
        PropTable::new(true)
            .with(WIA_IPS_PAGE_SIZE, Prop::List { values: vec![0], writable: true })
            .with(WIA_IPS_MAX_VERTICAL_SIZE, Prop::Value { value: 14_000, writable: false })
    }

    /// A range from `min` to `max` in steps of 1.
    fn span(min: i32, max: i32) -> ValueSpan {
        ValueSpan { min, max, step: 1 }
    }

    /// A length request of `mode` for a feeder or glass item, with `size` as the page size and no region.
    fn ask(mode: PageLength, size: Option<PageSize>, feeder: bool) -> Request {
        Request { mode, page_size: size, region: false, feeder }
    }

    #[test]
    fn a_feeder_that_publishes_nothing_is_absent_only_when_its_table_was_read() {
        let table = plain_feeder(200);
        assert!(device_methods(&table).is_empty());
        assert_eq!(support(&table).device_detect, Presence::Absent);
        assert_eq!(support(&table).max_length_mils, Some(14_000));
        assert_eq!(support(&PropTable::new(false)).device_detect, Presence::Unknown);
        assert_eq!(support(&PropTable::new(false)), PageLengthSupport::default());
    }

    #[test]
    fn auto_crop_is_present_only_when_it_accepts_single() {
        let listed = PropTable::new(true).with(WIA_IPS_AUTO_CROP, Prop::List { values: vec![0, 1, 2], writable: true });
        assert_eq!(device_methods(&listed), [DeviceDetect::AutoCrop]);
        assert_eq!(support(&listed).device_detect, Presence::Present);
        let multi_only = PropTable::new(true).with(WIA_IPS_AUTO_CROP, Prop::List { values: vec![0, 2], writable: true });
        assert!(device_methods(&multi_only).is_empty(), "multi-crop alone is not single-sheet detection");
        let read_only = PropTable::new(true).with(WIA_IPS_AUTO_CROP, Prop::List { values: vec![0, 1], writable: false });
        assert_eq!(support(&read_only).device_detect, Presence::Absent, "published but not writable");
        let value_only = PropTable::new(true).with(WIA_IPS_AUTO_CROP, Prop::Value { value: 0, writable: true });
        assert_eq!(support(&value_only).device_detect, Presence::Absent, "a bare current value of 0 does not offer 1");
        let range = PropTable::new(true).with(WIA_IPS_AUTO_CROP, Prop::Range { span: span(0, 2), writable: true });
        assert_eq!(device_methods(&range), [DeviceDetect::AutoCrop]);
    }

    #[test]
    fn page_size_auto_counts_only_when_the_list_holds_it() {
        let table = PropTable::new(true).with(WIA_IPS_PAGE_SIZE, Prop::List { values: vec![0, 1, 100], writable: true });
        assert_eq!(device_methods(&table), [DeviceDetect::PageSizeAuto]);
        assert_eq!(DeviceDetect::PageSizeAuto.write(), (WIA_IPS_PAGE_SIZE, WIA_PAGE_AUTO));
        assert_eq!(WIA_PAGE_AUTO, PageSize::Auto.driver());
        let without = PropTable::new(true).with(WIA_IPS_PAGE_SIZE, Prop::List { values: vec![0, 1], writable: true });
        assert!(device_methods(&without).is_empty());
    }

    #[test]
    fn long_document_counts_and_methods_keep_their_priority() {
        let all = PropTable::new(true)
            .with(WIA_IPS_LONG_DOCUMENT, Prop::List { values: vec![0, 1, 2], writable: true })
            .with(WIA_IPS_PAGE_SIZE, Prop::List { values: vec![0, 100], writable: true })
            .with(WIA_IPS_AUTO_CROP, Prop::List { values: vec![0, 1], writable: true });
        assert_eq!(device_methods(&all), [DeviceDetect::AutoCrop, DeviceDetect::PageSizeAuto, DeviceDetect::LongDocument]);
        let only = PropTable::new(true).with(WIA_IPS_LONG_DOCUMENT, Prop::List { values: vec![0, 1], writable: true });
        assert_eq!(device_methods(&only), [DeviceDetect::LongDocument]);
        assert_eq!(DeviceDetect::LongDocument.write(), (WIA_IPS_LONG_DOCUMENT, 1));
        assert_eq!(DeviceDetect::AutoCrop.write(), (WIA_IPS_AUTO_CROP, 1));
    }

    #[test]
    fn the_request_follows_the_priority_device_then_size_then_inference() {
        let plain = plain_feeder(200);
        let detecting = PropTable::new(true).with(WIA_IPS_AUTO_CROP, Prop::List { values: vec![0, 1], writable: true });
        // Driver: nothing, whatever is published.
        assert_eq!(decide(&ask(PageLength::Driver, Some(PageSize::Legal), true), &detecting).action, Action::Driver);
        // Device detection beats an explicit size.
        assert_eq!(decide(&ask(PageLength::Auto, Some(PageSize::Legal), true), &detecting).action, Action::Device(DeviceDetect::AutoCrop));
        // No detection: the size becomes extents.
        assert_eq!(
            decide(&ask(PageLength::Auto, Some(PageSize::Legal), true), &plain).action,
            Action::Size { size: PageSize::Legal, height_mils: 14_000 }
        );
        // Auto with nothing to go on falls back to the driver; infer does not apply to Auto.
        assert_eq!(decide(&ask(PageLength::Auto, None, true), &plain).action, Action::Driver);
        assert_eq!(decide(&ask(PageLength::Auto, Some(PageSize::Custom), true), &plain).action, Action::Driver);
        // AutoInfer infers only as the last resort, and only on the feeder.
        assert_eq!(decide(&ask(PageLength::AutoInfer, None, true), &plain).action, Action::Infer);
        assert_eq!(decide(&ask(PageLength::AutoInfer, None, false), &plain).action, Action::Driver);
        assert_eq!(
            decide(&ask(PageLength::AutoInfer, Some(PageSize::A4), true), &plain).action,
            Action::Size { size: PageSize::A4, height_mils: 11_692 }
        );
        assert_eq!(decide(&ask(PageLength::AutoInfer, None, true), &detecting).action, Action::Device(DeviceDetect::AutoCrop));
        // An unread table cannot offer detection but does not stop a size or inference.
        assert_eq!(decide(&ask(PageLength::AutoInfer, None, true), &PropTable::new(false)).action, Action::Infer);
    }

    #[test]
    fn size_mode_uses_the_size_even_when_the_scanner_detects_and_never_infers() {
        let detecting = PropTable::new(true).with(WIA_IPS_AUTO_CROP, Prop::List { values: vec![0, 1], writable: true });
        let plain = plain_feeder(200);
        for table in [&detecting, &plain] {
            assert_eq!(
                decide(&ask(PageLength::Size, Some(PageSize::Letter), true), table).action,
                Action::Size { size: PageSize::Letter, height_mils: 11_000 }
            );
            assert_eq!(decide(&ask(PageLength::Size, None, true), table).action, Action::Driver);
            assert_eq!(decide(&ask(PageLength::Size, Some(PageSize::Auto), true), table).action, Action::Driver);
        }
    }

    #[test]
    fn an_explicit_region_wins_over_every_length_request() {
        let request = Request { mode: PageLength::AutoInfer, page_size: Some(PageSize::A4), region: true, feeder: true };
        assert_eq!(decide(&request, &plain_feeder(200)).action, Action::Driver);
        let detecting = PropTable::new(true).with(WIA_IPS_AUTO_CROP, Prop::List { values: vec![0, 1], writable: true });
        assert_eq!(decide(&request, &detecting).action, Action::Driver);
    }

    #[test]
    fn extents_for_a4_letter_legal_and_custom_at_several_resolutions() {
        // Rows are the rounded length times the resolution; the item's range is 16 rows up to 14 in.
        let rows = |size: PageSize, dpi: u32| -> Result<u32, FitError> {
            let (_, mils) = size.extent_mils().ok_or(FitError::TooShort { min_rows: 0 })?;
            fit_height(mils, dpi, Some(span(16, 14 * dpi as i32)))
        };
        assert_eq!(rows(PageSize::A4, 200), Ok(2338));
        assert_eq!(rows(PageSize::A4, 100), Ok(1169));
        assert_eq!(rows(PageSize::A4, 300), Ok(3508));
        assert_eq!(rows(PageSize::A4, 600), Ok(7015));
        assert_eq!(rows(PageSize::Letter, 200), Ok(2200));
        assert_eq!(rows(PageSize::Letter, 300), Ok(3300));
        assert_eq!(rows(PageSize::Letter, 600), Ok(6600));
        assert_eq!(rows(PageSize::Legal, 200), Ok(2800));
        assert_eq!(rows(PageSize::Legal, 600), Ok(8400));
        // Custom and Auto have no height.
        assert!(PageSize::Custom.extent_mils().is_none() && PageSize::Auto.extent_mils().is_none());
        // A custom length is any number of thousandths of an inch (here 5.5 in).
        assert_eq!(fit_height(5500, 200, Some(span(16, 2800))), Ok(1100));
        assert_eq!(fit_height(5500, 150, None), Ok(825));
    }

    #[test]
    fn a_length_outside_the_published_range_is_a_clear_error_and_one_step_of_slack_snaps() {
        // 14 in at 200 dpi is exactly the top of 16..2800.
        assert_eq!(fit_height(14_000, 200, Some(span(16, 2800))), Ok(2800));
        // 14.005 in is 2801 rows: one row over, snapped to the end.
        assert_eq!(fit_height(14_005, 200, Some(span(16, 2800))), Ok(2800));
        assert_eq!(fit_height(14_020, 200, Some(span(16, 2800))), Err(FitError::TooLong { max_rows: 2800 }));
        // A scanner whose longest page is A4 cannot make Legal.
        let a4_only = span(16, 2338);
        assert_eq!(fit_height(14_000, 200, Some(a4_only)), Err(FitError::TooLong { max_rows: 2338 }));
        assert_eq!(fit_height(50, 200, Some(span(100, 2800))), Err(FitError::TooShort { min_rows: 100 }));
        assert_eq!(fit_height(500, 200, Some(span(16, 2800))), Ok(100));
        let text = FitError::TooLong { max_rows: 2338 }.sentence(14_000, 200);
        assert!(text.contains("14.00 in") && text.contains("11.69 in") && text.contains("200 dpi"), "{text}");
        // A step is honoured: rows snap down onto the grid (as regions do).
        let stepped = ValueSpan { min: 16, max: 2800, step: 8 };
        assert_eq!(fit_height(5500, 200, Some(stepped)), Ok(1096));
    }

    #[test]
    fn the_longest_extent_comes_from_the_range_first_then_the_maximum_size() {
        assert_eq!(longest_rows(Some(span(16, 2800)), Some(14_000), 200), Some(2800));
        assert_eq!(longest_rows(Some(span(16, 8400)), Some(14_000), 600), Some(8400));
        assert_eq!(longest_rows(None, Some(14_000), 300), Some(4200));
        assert_eq!(longest_rows(None, None, 300), None);
        assert_eq!(longest_rows(None, Some(0), 300), None);
    }

    #[test]
    fn the_constants_are_the_wia_ones() {
        assert_eq!(
            (WIA_IPS_AUTO_CROP, WIA_IPS_LONG_DOCUMENT, WIA_IPS_PAGE_SIZE, WIA_IPS_YEXTENT, WIA_IPS_MAX_VERTICAL_SIZE),
            (4170, 4166, 3097, 6152, 6166)
        );
        #[cfg(windows)]
        {
            use windows::Win32::Devices::ImageAcquisition as wia;
            assert_eq!(WIA_IPS_AUTO_CROP, wia::WIA_IPS_AUTO_CROP);
            assert_eq!(WIA_IPS_LONG_DOCUMENT, wia::WIA_IPS_LONG_DOCUMENT);
            assert_eq!(WIA_IPS_PAGE_SIZE, wia::WIA_IPS_PAGE_SIZE);
            assert_eq!(WIA_IPS_YEXTENT, wia::WIA_IPS_YEXTENT);
            assert_eq!(WIA_IPS_MAX_VERTICAL_SIZE, wia::WIA_IPS_MAX_VERTICAL_SIZE);
            assert_eq!(WIA_AUTO_CROP_SINGLE, wia::WIA_AUTO_CROP_SINGLE as i32);
            assert_eq!(WIA_PAGE_AUTO, wia::WIA_PAGE_AUTO as i32);
            assert_eq!(WIA_LONG_DOCUMENT_ENABLED, wia::WIA_LONG_DOCUMENT_ENABLED as i32);
        }
    }

    /// The saved real scan (200 dpi, 14 in extent): inference lands where the measured scanner ended the sheet.
    /// Set `PDFCRAFT_TEST_PADDED_SCAN` to its path; skipped when unset or missing. Read only, never stored in the
    /// repository.
    #[test]
    fn the_saved_scan_is_inferred_at_the_measured_sheet_end() {
        use craft_scan::{InferOptions, assess_sheet_end};
        let Some(path) = std::env::var_os("PDFCRAFT_TEST_PADDED_SCAN") else { return };
        let Ok(bytes) = std::fs::read(path) else { return };
        let Ok(found) = assess_sheet_end(&bytes, 200, &InferOptions::default()) else {
            panic!("the saved scan should be inferable");
        };
        let inches = f64::from(found.length_mils) / 1000.0;
        assert!((10.73..=10.78).contains(&inches), "{inches}");
    }
}
