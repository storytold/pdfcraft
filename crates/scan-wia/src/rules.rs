//! What the WIA backend knows about WIA, kept free of COM so it builds and is tested on every
//! platform: the HRESULT name and kind tables, the sentences WIA and the Windows WSD driver are
//! known to produce, the document-handling status bits, and the rules that sharpen a vague
//! transfer failure with a status read. `craft-scan` knows only [`NativeCode`] and
//! [`InterruptKind`]; this module is what the Windows backend does with them.

use craft_scan::{HandlingStatus, InterruptKind, NativeCode, PageSize, ScanError, Source, interrupt_kind};

/// The sentence for a feeder transfer that answered busy while nothing names a cause.
pub const BUSY_OR_FEEDER_EMPTY: &str = craft_scan::BUSY_OR_FEEDER_EMPTY;

/// HRESULTs with a name Microsoft documents (the WIA error code list), by that exact name. Codes
/// classified but not listed by Microsoft (0x80210010 to 0x80210014: the Windows WSD driver was
/// measured returning 0x80210010 for a cover and 0x80210011 for the lamp) have no name here.
pub const CODE_NAMES: [(u32, &str); 22] = [
    (0x8021_0001, "WIA_ERROR_GENERAL_ERROR"),
    (0x8021_0002, "WIA_ERROR_PAPER_JAM"),
    (0x8021_0003, "WIA_ERROR_PAPER_EMPTY"),
    (0x8021_0004, "WIA_ERROR_PAPER_PROBLEM"),
    (0x8021_0005, "WIA_ERROR_OFFLINE"),
    (0x8021_0006, "WIA_ERROR_BUSY"),
    (0x8021_0007, "WIA_ERROR_WARMING_UP"),
    (0x8021_0008, "WIA_ERROR_USER_INTERVENTION"),
    (0x8021_0009, "WIA_ERROR_ITEM_DELETED"),
    (0x8021_000A, "WIA_ERROR_DEVICE_COMMUNICATION"),
    (0x8021_000B, "WIA_ERROR_INVALID_COMMAND"),
    (0x8021_000C, "WIA_ERROR_INCORRECT_HARDWARE_SETTING"),
    (0x8021_000D, "WIA_ERROR_DEVICE_LOCKED"),
    (0x8021_000E, "WIA_ERROR_EXCEPTION_IN_DRIVER"),
    (0x8021_000F, "WIA_ERROR_INVALID_DRIVER_RESPONSE"),
    (0x8021_0015, "WIA_S_NO_DEVICE_AVAILABLE"),
    (0x8021_0016, "WIA_ERROR_COVER_OPEN"),
    (0x8021_0017, "WIA_ERROR_LAMP_OFF"),
    (0x8021_0020, "WIA_ERROR_MULTI_FEED"),
    (0x8021_0021, "WIA_ERROR_MAXIMUM_PRINTER_ENDORSER_COUNTER"),
    (0x8000_4005, "E_FAIL"),
    (0x8007_0057, "E_INVALIDARG"),
];

pub fn code_name(code: u32) -> Option<&'static str> {
    CODE_NAMES.iter().find(|(known, _)| *known == code).map(|(_, name)| *name)
}

/// The kind each classified HRESULT stands for.
pub const KIND_BY_CODE: [(u32, InterruptKind); 19] = [
    (0x8021_0002, InterruptKind::Jam),
    (0x8021_0004, InterruptKind::PaperProblem),
    (0x8021_0010, InterruptKind::CoverOpen),
    (0x8021_0014, InterruptKind::MultiFeed),
    (0x8021_0003, InterruptKind::FeederEmpty),
    (0x8021_0005, InterruptKind::Offline),
    (0x8021_000A, InterruptKind::Offline),
    (0x8021_0008, InterruptKind::Attention),
    (0x8021_0001, InterruptKind::Offline),
    (0x8021_0006, InterruptKind::Busy),
    (0x8021_0007, InterruptKind::Busy),
    (0x8021_000D, InterruptKind::Busy),
    (0x8021_0011, InterruptKind::DeviceError),
    (0x8021_000E, InterruptKind::DeviceError),
    (0x8021_0013, InterruptKind::Busy),
    (0x8021_0015, InterruptKind::Offline),
    (0x8021_0016, InterruptKind::CoverOpen),
    (0x8021_0017, InterruptKind::DeviceError),
    (0x8021_0020, InterruptKind::MultiFeed),
];

pub fn kind_for_hresult(code: u32) -> Option<InterruptKind> {
    KIND_BY_CODE.iter().find(|(known, _)| *known == code).map(|(_, kind)| *kind)
}

/// Plain sentence for a WIA HRESULT.
pub fn wia_error_message(code: u32) -> Option<&'static str> {
    match code {
        0x8021_0001 => Some("The scanner could not be opened. If it is a network scanner, remove it in Windows and add it again while it is on."),
        0x8021_0002 => Some("The scanner has a paper jam."),
        0x8021_0003 => Some("The feeder has no paper."),
        0x8021_0004 => Some("The scanner has a paper problem."),
        0x8021_0005 => Some(
            "The scanner is offline. Check that it is powered on and connected. For a network scanner, remove it in Windows and add it again while it is on.",
        ),
        0x8021_0006 => Some("The scanner is busy. Close any other program that is using it, or wait for the current scan to finish, then try again."),
        0x8021_0007 => Some("The scanner is warming up. Try again in a moment."),
        0x8021_0008 => Some("The scanner needs attention. Check it, then try again."),
        0x8021_0009 => Some("The scanner item was removed."),
        0x8021_000A => Some("The computer could not talk to the scanner. Check the cable and power, then try again."),
        0x8021_000B => Some("The scanner rejected that command."),
        0x8021_000C => Some("A scanner setting was not accepted."),
        0x8021_000D => Some("The scanner is locked. Close any other program that is using it, then try again."),
        0x8021_000E => Some("The scanner driver failed."),
        0x8021_000F => Some("The scanner driver gave an unexpected response."),
        0x8021_0010 | 0x8021_0016 => Some("A cover on the scanner is open."),
        0x8021_0011 | 0x8021_0017 => Some("The scanner lamp is off."),
        0x8021_0012 => Some("The scan could not be saved to that destination."),
        0x8021_0013 => Some("The network scanner could not be reserved."),
        0x8021_0014 | 0x8021_0020 => Some("The feeder took more than one sheet."),
        0x8021_0015 => Some("No scanner is available. Check that it is powered on and connected."),
        0x8021_0021 => Some("The printer endorser counter is at its maximum."),
        _ => None,
    }
}

/// The kind of a WIA sentence: the table first, then the core's neutral fallback.
pub fn wia_interrupt_kind(message: &str) -> InterruptKind {
    if message.contains(BUSY_OR_FEEDER_EMPTY) {
        return InterruptKind::BusyOrFeederEmpty;
    }
    for (code, kind) in KIND_BY_CODE {
        if let Some(sentence) = wia_error_message(code)
            && message.contains(sentence)
        {
            return kind;
        }
    }
    interrupt_kind(message)
}

/// Map WS-Scan SOAP text or driver messages to the same sentences as [`wia_error_message`].
fn transfer_fault_sentence(message: &str) -> Option<&'static str> {
    let lower = message.to_ascii_lowercase();
    if lower.contains("mediajam") || lower.contains("clienterrormediajam") {
        return wia_error_message(0x8021_0002);
    }
    if lower.contains("notacceptingjobs") || lower.contains("servererrornotacceptingjobs") {
        return wia_error_message(0x8021_0005);
    }
    if lower.contains("noimagesavailable") || lower.contains("clienterrornoimagesavailable") {
        return wia_error_message(0x8021_0003);
    }
    if lower.contains("coveropen") || lower.contains("clienterrorcoveropen") {
        return wia_error_message(0x8021_0010);
    }
    if lower.contains("multifeed") || lower.contains("clienterrordocumentfeederror") {
        return wia_error_message(0x8021_0014);
    }
    None
}

/// Text for a Windows error from WIA. Known codes become sentences; unknown ones keep the hex.
pub fn describe_windows_error(code: u32, message: &str) -> String {
    if code == 0x8021_0004
        && let Some(sentence) = transfer_fault_sentence(message.trim())
    {
        return sentence.to_string();
    }
    if let Some(text) = wia_error_message(code) {
        return text.to_string();
    }
    let trimmed = message.trim();
    if !trimmed.is_empty()
        && !trimmed.eq_ignore_ascii_case("ok")
        && trimmed != "The operation completed successfully."
        && !trimmed.eq_ignore_ascii_case("Unspecified error")
    {
        if let Some(sentence) = transfer_fault_sentence(trimmed) {
            return sentence.to_string();
        }
        return trimmed.to_string();
    }
    if code == 0x8000_4005 {
        return "The scan was interrupted. Check for a paper jam, an open cover, or that the scanner stayed online.".into();
    }
    if (0x8021_0000..=0x8021_FFFF).contains(&code) {
        return format!("The scanner reported an error ({code:#010x}).");
    }
    format!("The scan failed ({code:#010x}).")
}

/// An error for an HRESULT, with the sentence for it and the cause the code stands for. The sentence
/// decides only when the code is unknown or is the vague `PAPER_PROBLEM`, which the driver's own
/// text can sharpen into a jam or a double feed.
pub fn from_hresult(code: u32, driver_message: &str) -> ScanError {
    let message = describe_windows_error(code, driver_message);
    let from_message = wia_interrupt_kind(&message);
    let kind = match kind_for_hresult(code) {
        Some(_) if code == 0x8021_0004 => from_message,
        Some(kind) => kind,
        None => from_message,
    };
    ScanError::native_error(kind, NativeCode::hresult(code, code_name(code)), message)
}

/// Bits from `WIA_DPS_DOCUMENT_HANDLING_STATUS`.
pub fn status_from_bits(bits: i32) -> HandlingStatus {
    let bits = u32::try_from(bits).unwrap_or(0);
    HandlingStatus {
        feeder_ready: Some(bits & 0x1 != 0),
        flat_ready: bits & 0x2 != 0,
        duplex_ready: bits & 0x4 != 0,
        cover_open: bits & 0x8 != 0,
        path_cover_open: bits & 0x10 != 0,
        jam: bits & 0x20 != 0,
        multiple_feed: bits & 0x200 != 0,
        attention: bits & 0x400 != 0,
        lamp_error: bits & 0x800 != 0,
    }
}

/// WIA document-handling capability bits: feeder, flatbed, duplex.
pub fn handling_caps(bits: i32) -> (bool, bool, bool) {
    (bits & 0x1 != 0, bits & 0x2 != 0, bits & 0x4 != 0)
}

/// WIA's wording for the status bits that block a scan.
pub fn acquire_block_reason(status: HandlingStatus) -> Option<&'static str> {
    if status.jam {
        return wia_error_message(0x8021_0002);
    }
    if status.cover_open || status.path_cover_open {
        return wia_error_message(0x8021_0010);
    }
    if status.multiple_feed {
        return wia_error_message(0x8021_0014);
    }
    if status.lamp_error {
        return wia_error_message(0x8021_0011);
    }
    None
}

/// The sentence for a paper-path fault the bits name, in the order jam, cover, double feed.
pub fn paper_path_reason(status: HandlingStatus) -> Option<&'static str> {
    if status.jam {
        return wia_error_message(0x8021_0002);
    }
    if status.cover_open || status.path_cover_open {
        return wia_error_message(0x8021_0010);
    }
    if status.multiple_feed {
        return wia_error_message(0x8021_0014);
    }
    None
}

/// The same failure restated as `sentence` by a status read; the native code stays.
fn restated(error: &ScanError, sentence: &str) -> ScanError {
    error.restated(wia_interrupt_kind(sentence), sentence)
}

/// The sentence [`after_transfer`] gives a feeder transfer that delivered nothing and named no fault.
pub fn no_usable_page(error: &ScanError) -> bool {
    error.to_string().starts_with("The scanner did not deliver a usable page.")
}

/// A Windows failure that does not name the fault. A later status read can.
pub fn generic_scan_failure(error: &ScanError) -> bool {
    let text = error.to_string();
    text == "The scanner reported an error."
        || text.starts_with("The scanner reported an error (")
        || text.starts_with("The scan failed (")
        || text == "Unspecified error"
        || text == "The parameter is incorrect."
        || text.starts_with("The scan was interrupted.")
}

fn is_busy(error: &ScanError) -> bool {
    wia_error_message(0x8021_0006).is_some_and(|sentence| error.to_string().contains(sentence))
}

fn is_paper_problem(error: &ScanError) -> bool {
    wia_error_message(0x8021_0004).is_some_and(|sentence| error.to_string().contains(sentence))
}

/// When Download returns a generic HRESULT, prefer a jam, cover, double-feed, lamp, or attention
/// bit read after the call; on the feeder a clear `FEED_READY` with no block bit is an empty tray.
pub fn explain_transfer_failure(
    error: ScanError,
    status: HandlingStatus,
    feeder: bool,
    pages_transferred: usize,
    pages_requested: usize,
) -> ScanError {
    if feeder && is_busy(&error) {
        // An empty feeder is invisible to the WIA status: the driver starts the scan, then answers
        // BUSY. A bit that names a cause wins; with none set, say both.
        let named = acquire_block_reason(status).or_else(|| status.attention.then(|| wia_error_message(0x8021_0008)).flatten());
        return restated(&error, named.unwrap_or(BUSY_OR_FEEDER_EMPTY));
    }
    if is_paper_problem(&error) {
        return match paper_path_reason(status) {
            Some(reason) => restated(&error, reason),
            None => error,
        };
    }
    if !generic_scan_failure(&error) {
        return error;
    }
    if let Some(reason) = acquire_block_reason(status) {
        return restated(&error, reason);
    }
    if feeder && pages_requested == 0 && pages_transferred > 0 {
        return restated(&error, wia_error_message(0x8021_0003).unwrap_or("The feeder has no paper."));
    }
    if status.attention
        && let Some(reason) = wia_error_message(0x8021_0008)
    {
        // A door interlock or a full device store clears `FEED_READY` too; the sentence does not
        // guess which one.
        return restated(&error, reason);
    }
    if feeder && status.feeder_ready != Some(true) {
        let empty_tray = pages_transferred == 0
            || (pages_requested > 0 && pages_transferred >= pages_requested)
            || (pages_requested == 0 && pages_transferred > 0);
        if empty_tray {
            return restated(&error, wia_error_message(0x8021_0003).unwrap_or("The feeder has no paper."));
        }
    }
    error
}

/// Banded transfer is the wrong mode when the driver rejects it without naming a device fault.
pub fn retry_with_file_transfer(error: &ScanError) -> bool {
    generic_scan_failure(error)
}

/// Whether feeder acquire should refuse before transfer when `FEED_READY` is clear. Inbox WSD
/// drivers often omit the bit even when the feeder has sheets loaded.
pub fn feeder_empty_blocks_acquire(source: Source, feeder_ready: bool, wsd_port: bool) -> bool {
    source == Source::Feeder && !feeder_ready && !wsd_port
}

/// WIA `IPS_PAGES` / `DPS_PAGES` for inbox WSD when the user asked for the whole feeder (`0`).
pub fn wsd_feeder_ticket_pages(page_count: i32) -> i32 {
    if page_count == 0 { i32::try_from(craft_scan::MAX_PAGES).unwrap_or(i32::MAX) } else { page_count }
}

/// [`craft_scan::after_transfer`] with the Windows clarification: a generic failure (an `E_FAIL`
/// the driver gave no reason for) after some pages is a stack that stopped part-way, and with no
/// pages a page the scanner did not deliver. Other faults keep their own sentence.
pub fn after_transfer(page_count: usize, cancelled: bool, download_error: Option<ScanError>) -> Result<(), ScanError> {
    let download_error = download_error.map(|error| {
        if cancelled && page_count == 0 || !generic_scan_failure(&error) {
            return error;
        }
        if page_count > 0 {
            restated(&error, "The scan stopped before all pages arrived. Check for a paper jam, an open cover, or that the scanner stayed online.")
        } else {
            restated(
                &error,
                "The scanner did not deliver a usable page. Check that it stayed online; for a network scanner, remove it in Windows and add it again while it is on if this keeps happening.",
            )
        }
    });
    craft_scan::after_transfer(page_count, cancelled, download_error)
}

/// True when the transfer stopped because the feeder had no more sheets, not because of a jam.
fn feeder_ended(error: &ScanError) -> bool {
    matches!(error.interrupt_kind(), InterruptKind::FeederEmpty | InterruptKind::BusyOrFeederEmpty)
}

/// [`after_transfer`] plus the page-count check for a fixed-count feeder scan, as
/// [`craft_scan::after_transfer_checked`] does it, but over the Windows wording of
/// [`after_transfer`].
///
/// `pages_requested` is `0` for "the whole feeder" (a short stack is a normal end). Pass `0` for
/// hardware duplex (sides are not sheets) and `feeder = false` for the glass.
pub fn after_transfer_checked(
    pages_requested: usize,
    feeder: bool,
    pages_transferred: usize,
    cancelled: bool,
    download_error: Option<ScanError>,
) -> Result<(), ScanError> {
    let ended_empty = download_error.as_ref().is_none_or(feeder_ended);
    after_transfer(pages_transferred, cancelled, download_error)?;
    if feeder && !cancelled && ended_empty && pages_requested > 0 && pages_transferred > 0 && pages_transferred < pages_requested {
        return Err(ScanError::Native {
            kind: InterruptKind::FeederEmpty,
            native: None,
            message: craft_scan::feeder_shortfall_message(pages_requested, pages_transferred),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Page size, chosen against what the item that will transfer publishes
// ---------------------------------------------------------------------------------------------

/// The nominal sheet of a named page size in thousandths of an inch, `(width, height)`. `Custom`
/// and `Auto` are not a sheet and have none.
pub fn page_size_dims(size: PageSize) -> Option<(u32, u32)> {
    size.extent_mils()
}

/// Why no page size is written at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageSizeOmit {
    /// The item has no page-size property (the glass of the inbox WSD driver). Unsupported: nothing is written.
    NotPublished,
    /// The item publishes sizes, none of which is the one asked for or one that covers it.
    NotOffered,
    /// Every size worth trying was rejected by the driver: the item keeps its own default.
    Rejected,
}

/// What to do with a requested page size on one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageSizePlan {
    /// Write nothing; the item scans its default (full) area.
    Omit(PageSizeOmit),
    /// Write the first of these the driver accepts, in order: the size asked for when the item
    /// lists it, then the listed sizes that cover the asked-for sheet, smallest first.
    Try(Vec<PageSize>),
}

/// `published` is the page-size list of the item that will transfer (`None` when the item has no such
/// property). The inbox WSD driver lists more sizes than it accepts (a size can be listed and still be
/// refused with `E_INVALIDARG`), so the list only narrows the candidates; the backend still tries them
/// in order and falls back to [`PageSizeOmit::Rejected`] when none is taken.
pub fn plan_page_size(requested: PageSize, published: Option<&[PageSize]>) -> PageSizePlan {
    let Some(listed) = published.filter(|sizes| !sizes.is_empty()) else {
        return PageSizePlan::Omit(PageSizeOmit::NotPublished);
    };
    let mut tries = Vec::new();
    if listed.contains(&requested) {
        tries.push(requested);
    }
    if let Some((want_w, want_h)) = page_size_dims(requested) {
        let mut covering: Vec<(u64, PageSize)> = listed
            .iter()
            .copied()
            .filter(|size| *size != requested)
            .filter_map(|size| {
                let (w, h) = page_size_dims(size)?;
                (w >= want_w && h >= want_h).then_some((u64::from(w) * u64::from(h), size))
            })
            .collect();
        covering.sort_by_key(|(area, _)| *area);
        tries.extend(covering.into_iter().map(|(_, size)| size));
    }
    if tries.is_empty() { PageSizePlan::Omit(PageSizeOmit::NotOffered) } else { PageSizePlan::Try(tries) }
}

/// The sentence for the scan log and trace when the size written is not the size asked for.
pub fn page_size_note(requested: PageSize, applied: Option<PageSize>, omit: Option<PageSizeOmit>) -> String {
    match (applied, omit) {
        (Some(size), _) => format!("The scanner does not take {} here; scanned as {} instead.", requested.name(), size.name()),
        (None, Some(PageSizeOmit::NotPublished)) => format!("This source has no page size to set; {} was not applied.", requested.name()),
        (None, Some(PageSizeOmit::NotOffered)) => {
            format!("The scanner does not offer {} for this source; the full area was scanned.", requested.name())
        }
        (None, _) => format!("The scanner did not accept {}; the full area was scanned.", requested.name()),
    }
}

/// A property of an item in words, for the sentence of an error that names it. `None` for a property
/// with no wording here (the id is then all there is to say).
pub fn property_words(id: u32) -> Option<&'static str> {
    match id {
        3096 => Some("page count"),
        3097 => Some("page size"),
        3098 => Some("page width"),
        3099 => Some("page height"),
        4103 => Some("color mode"),
        4104 => Some("bit depth"),
        6146 => Some("scan intent"),
        6147 => Some("horizontal resolution"),
        6148 => Some("vertical resolution"),
        6149 | 6150 => Some("scan area position"),
        6151 | 6152 => Some("scan area size"),
        6154 => Some("brightness"),
        6155 => Some("contrast"),
        6156 => Some("orientation"),
        6157 => Some("rotation"),
        6159 => Some("threshold"),
        _ => None,
    }
}

/// The sentence for a property the driver refused: the property in words, the driver's own text as
/// the detail. A property with no wording keeps its number.
pub fn rejected_property_message(id: u32, driver_text: &str) -> String {
    match property_words(id) {
        Some(words) => format!("The scanner did not accept the {words} setting ({driver_text})"),
        None => format!("The scanner did not accept property {id} ({driver_text})"),
    }
}

/// [`craft_scan::contract_json`] with the HRESULT table beside it, under `hresults` (hex string to
/// symbolic name). The core dump has no backend code table; this is the one the bindings use.
pub fn contract_json() -> String {
    let mut dump = craft_scan::contract_json();
    let table: Vec<String> = CODE_NAMES.iter().map(|(code, name)| format!("\"{code:#010x}\":\"{name}\"")).collect();
    if dump.ends_with('}') {
        dump.pop();
        dump.push_str(&format!(",\"hresults\":{{{}}}}}", table.join(",")));
    }
    dump
}
