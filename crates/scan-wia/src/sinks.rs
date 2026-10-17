//! The COM objects WIA calls back into while a transfer runs: the transfer callback, the file-backed
//! `IStream` it hands out, and the banded data callback. COM calls their methods with raw pointers,
//! so the method bodies read and write through them. That is the only unsafe code here, and it goes
//! through the four private helpers below ([`read_params`], [`with_input`], [`with_output`],
//! [`store`]); the rest of each method is safe state handling.
//!
//! All three objects live on the thread that runs the transfer (they hold `Rc`/`RefCell`), and WIA
//! calls them on that thread, in the apartment that [`crate::ffi::ComInit`] entered.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::rc::Rc;

use craft_scan::bw::{self, PageRule};
use craft_scan::trace::AcquireTrace;
use craft_scan::{CancelToken, Page, ProgressToken, ScanError};
use windows::Win32::Devices::ImageAcquisition::{
    IT_MSG_DATA, IT_MSG_NEW_PAGE, IT_MSG_TERMINATION, IWiaDataCallback, IWiaDataCallback_Impl, IWiaItem2, IWiaTransfer, IWiaTransferCallback,
    IWiaTransferCallback_Impl, WIA_ERROR_PAPER_EMPTY, WIA_TRANSFER_MSG_END_OF_STREAM, WIA_TRANSFER_MSG_STATUS, WiaTransferParams,
};
use windows::Win32::Foundation::{E_FAIL, E_NOTIMPL};
use windows::Win32::System::Com::{
    ISequentialStream_Impl, IStream, IStream_Impl, STATFLAG_NONAME, STATSTG, STGTY_STREAM, STREAM_SEEK_CUR, STREAM_SEEK_END, STREAM_SEEK_SET,
};
use windows::core::{BSTR, implement};

use crate::ffi;

// ---------------------------------------------------------------------------------------------
// Raw-pointer helpers. They are private and are called only with the pointer arguments COM passed
// to the method that is running, which COM guarantees are valid for the length of that call.
// ---------------------------------------------------------------------------------------------

/// The message, percent and byte count of the parameters COM passed to `TransferCallback`.
fn read_params(params: *const WiaTransferParams) -> Option<(i32, i32, u64)> {
    if params.is_null() {
        return None;
    }
    // SAFETY: non-null, and WIA passes a pointer to an initialised `WiaTransferParams` that is valid
    // for the duration of the `TransferCallback` call this helper runs in. The fields are copied
    // out and nothing borrowed from the pointer escapes.
    let params = unsafe { &*params };
    Some((params.lMessage, params.lPercentComplete, params.ulTransferredBytes))
}

/// Run `f` on the `len` bytes COM passed at `ptr`, which `Write` or a band callback may read. A
/// null pointer or a zero length gives an empty slice, so no zero-length slice is built on null.
fn with_input<R>(ptr: *const core::ffi::c_void, len: u32, f: impl FnOnce(&[u8]) -> R) -> R {
    let len = usize::try_from(len).unwrap_or(0);
    if ptr.is_null() || len == 0 {
        return f(&[]);
    }
    // SAFETY: non-null, and the caller of the COM method promises `len` readable, initialised bytes
    // at `ptr` for the duration of the call. The slice is lent to `f` only, so it cannot outlive it.
    f(unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) })
}

/// Run `f` on the `len` bytes COM passed at `ptr` for `Read` to fill. Same contract as [`with_input`].
fn with_output<R>(ptr: *mut core::ffi::c_void, len: u32, f: impl FnOnce(&mut [u8]) -> R) -> R {
    let len = usize::try_from(len).unwrap_or(0);
    if ptr.is_null() || len == 0 {
        return f(&mut []);
    }
    // SAFETY: non-null, and the caller of the COM method promises `len` writable bytes at `ptr`,
    // exclusively ours for the duration of the call. The slice is lent to `f` only.
    f(unsafe { std::slice::from_raw_parts_mut(ptr.cast::<u8>(), len) })
}

/// Write `value` through an optional out-pointer. `false` when the caller passed null.
fn store<T>(out: *mut T, value: T) -> bool {
    if out.is_null() {
        return false;
    }
    // SAFETY: non-null, and a COM out-parameter points to writable, aligned storage for one `T` for
    // the duration of the call. Every `T` stored here is plain data with no destructor, so not
    // dropping the previous contents leaks nothing.
    unsafe { out.write(value) };
    true
}

// ---------------------------------------------------------------------------------------------
// IWiaTransfer callback
// ---------------------------------------------------------------------------------------------

pub(crate) struct TransferState {
    pub(crate) pages: Vec<Page>,
    pub(crate) stream: Option<IStream>,
    /// File the stream is writing. Read after the stream is dropped, so the handle is closed.
    pub(crate) path: Option<PathBuf>,
    /// Streams already handed to the driver. Past `limit` the transfer is cancelled.
    pub(crate) offered: usize,
    pub(crate) limit: usize,
    pub(crate) transfer: Option<IWiaTransfer>,
    pub(crate) device: Option<IWiaItem2>,
    pub(crate) feeder: bool,
    pub(crate) rule: PageRule,
    pub(crate) cancel: Option<CancelToken>,
    pub(crate) progress: Option<ProgressToken>,
    /// Inbox WSD: trust `ulTransferredBytes`, not `lPercentComplete` (peaks ~40 then snaps to 100).
    pub(crate) wire_byte_progress: bool,
}

#[implement(IWiaTransferCallback)]
struct Transfer {
    state: Rc<RefCell<TransferState>>,
}

/// The transfer callback object for `state`, ready to pass to `IWiaTransfer::Download`.
pub(crate) fn transfer_callback(state: &Rc<RefCell<TransferState>>) -> IWiaTransferCallback {
    Transfer { state: Rc::clone(state) }.into()
}

impl IWiaTransferCallback_Impl for Transfer_Impl {
    fn TransferCallback(&self, _flags: i32, params: *const WiaTransferParams) -> windows::core::Result<()> {
        if cancel_requested(&self.this.state) {
            halt_feeder(&self.this.state);
            return Err(windows::core::Error::from(WIA_ERROR_PAPER_EMPTY));
        }
        let Some((message, percent, transferred)) = read_params(params) else {
            return Ok(());
        };
        let status = i32::try_from(WIA_TRANSFER_MSG_STATUS).unwrap_or(1);
        let end = i32::try_from(WIA_TRANSFER_MSG_END_OF_STREAM).unwrap_or(2);
        if message == status {
            // `PDFCRAFT_SCAN_STATUS_LOG=1` prints STATUS (useful when the CLI has no env_logger).
            if std::env::var_os("PDFCRAFT_SCAN_STATUS_LOG").is_some() {
                eprintln!("scan: transfer status percent={percent} transferred={transferred}");
            }
            if let Ok(state) = self.this.state.try_borrow()
                && let Some(progress) = &state.progress
            {
                let pct = if state.wire_byte_progress {
                    // Only the terminal 100 is meaningful; mid values are a compressed-scale quirk.
                    (percent >= 100).then_some(100u8)
                } else {
                    (0..=100).contains(&percent).then_some(percent as u8)
                };
                progress.set_transfer(pct, transferred, None);
            }
            return Ok(());
        }
        if message != end {
            return Ok(());
        }
        let full = {
            let mut state = self.this.state.try_borrow_mut().map_err(|_| windows::core::Error::from(E_FAIL))?;
            // Drop the stream first so its file handle closes before the bytes are read back.
            state.stream.take();
            if let Some(path) = state.path.take() {
                let bytes = std::fs::read(&path).unwrap_or_default();
                let _ = std::fs::remove_file(&path);
                if !bytes.is_empty() {
                    report_page(&mut state, bytes);
                }
            }
            state.pages.len() >= state.limit
        };
        if full {
            // The borrow is released first. Cancel re-enters this callback.
            halt_feeder(&self.this.state);
        }
        Ok(())
    }

    fn GetNextStream(&self, _flags: i32, _name: &BSTR, _full: &BSTR) -> windows::core::Result<IStream> {
        if cancel_requested(&self.this.state) {
            halt_feeder(&self.this.state);
            return Err(windows::core::Error::from(WIA_ERROR_PAPER_EMPTY));
        }
        let mut state = self.this.state.try_borrow_mut().map_err(|_| windows::core::Error::from(E_FAIL))?;
        if state.offered >= state.limit {
            drop(state);
            halt_feeder(&self.this.state);
            return Err(windows::core::Error::from(WIA_ERROR_PAPER_EMPTY));
        }
        state.offered += 1;
        if let Some(progress) = &state.progress {
            // Keep the page-size estimate; clear fill for the sheet about to start.
            progress.set_transfer(Some(0), 0, None);
        }
        let (stream, path) = file_stream().map_err(|_| windows::core::Error::from(E_FAIL))?;
        state.path = Some(path);
        state.stream = Some(stream.clone());
        Ok(stream)
    }
}

fn cancel_requested(state: &RefCell<TransferState>) -> bool {
    state.try_borrow().ok().and_then(|guard| guard.cancel.as_ref().map(CancelToken::is_requested)).unwrap_or(false)
}

fn report_page(state: &mut TransferState, bytes: Vec<u8>) {
    let page = page_from_bytes(&bytes, state.rule, state.pages.len());
    if let Some(progress) = &state.progress {
        progress.push_page(page.clone());
    }
    state.pages.push(page);
}

/// The page as delivered, or a real 1-bit BMP when a black-and-white request got deeper data.
fn page_from_bytes(bytes: &[u8], rule: PageRule, index: usize) -> Page {
    let (page, from_bits) = bw::page_for(bytes, rule);
    if let Some(from_bits) = from_bits {
        log::info!("scan: page {} came back {from_bits}-bit for a black-and-white request; thresholded to 1-bit", index + 1);
        let trace = AcquireTrace::from_env();
        if trace.enabled() {
            let (index_s, from_s, threshold_s) =
                (index.to_string(), from_bits.to_string(), rule.bw_threshold.unwrap_or(bw::DEFAULT_THRESHOLD).to_string());
            trace.phase("bw_software_threshold", &[("index", &index_s), ("from_bits", &from_s), ("to_bits", "1"), ("threshold", &threshold_s)]);
        }
    }
    page
}

/// End the transfer once the requested number of images has arrived. On this driver the Pages
/// property can read back as that count while the feeder still runs the stack, so the transfer
/// is cancelled and the feeder is told to stop.
pub(crate) fn halt_feeder(state: &RefCell<TransferState>) {
    let (transfer, device, feeder) = {
        let guard = match state.try_borrow() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        (guard.transfer.clone(), guard.device.clone(), guard.feeder)
    };
    if let Some(transfer) = transfer {
        let _ = ffi::cancel_transfer(&transfer);
    }
    if feeder && let Some(device) = device {
        let _ = ffi::stop_feeder(&device);
    }
}

/// Bytes the driver may send in one band: 32 scan lines, and at least 64 KB.
pub(crate) fn band_bytes(width: u32, depth: u32) -> u32 {
    let stride = (u64::from(width) * u64::from(depth)).div_ceil(32) * 4;
    u32::try_from(stride.saturating_mul(32)).unwrap_or(1024 * 1024).max(64 * 1024)
}

fn temp_page() -> Result<(std::fs::File, PathBuf), ScanError> {
    let path = std::env::temp_dir().join(format!(
        "pdfcraft-scan-{}-{}.bin",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
    ));
    let file =
        std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).map_err(|e| ScanError::Failed(e.to_string()))?;
    Ok((file, path))
}

fn file_stream() -> Result<(IStream, PathBuf), ScanError> {
    let (file, path) = temp_page()?;
    let stream: IStream = FileStream { file: RefCell::new(file), pos: RefCell::new(0), len: RefCell::new(0) }.into();
    Ok((stream, path))
}

// ---------------------------------------------------------------------------------------------
// File-backed IStream
// ---------------------------------------------------------------------------------------------

/// An `IStream` that writes each chunk to a file. WIA's download calls `Write` as bytes arrive,
/// so a page never has to sit in one memory block.
#[implement(IStream)]
struct FileStream {
    file: RefCell<std::fs::File>,
    pos: RefCell<u64>,
    len: RefCell<u64>,
}

impl ISequentialStream_Impl for FileStream_Impl {
    fn Read(&self, pv: *mut core::ffi::c_void, cb: u32, pcbread: *mut u32) -> windows::core::HRESULT {
        if pv.is_null() {
            return E_FAIL;
        }
        let Ok(mut file) = self.this.file.try_borrow_mut() else { return E_FAIL };
        let Ok(mut pos) = self.this.pos.try_borrow_mut() else { return E_FAIL };
        if file.seek(SeekFrom::Start(*pos)).is_err() {
            return E_FAIL;
        }
        match with_output(pv, cb, |dest| file.read(dest)) {
            Ok(n) => {
                *pos += u64::try_from(n).unwrap_or(0);
                store(pcbread, u32::try_from(n).unwrap_or(u32::MAX));
                windows::core::HRESULT(0)
            }
            Err(_) => E_FAIL,
        }
    }

    fn Write(&self, pv: *const core::ffi::c_void, cb: u32, pcbwritten: *mut u32) -> windows::core::HRESULT {
        if pv.is_null() && cb != 0 {
            return E_FAIL;
        }
        let Ok(mut file) = self.this.file.try_borrow_mut() else { return E_FAIL };
        let Ok(mut pos) = self.this.pos.try_borrow_mut() else { return E_FAIL };
        let Ok(mut len) = self.this.len.try_borrow_mut() else { return E_FAIL };
        if file.seek(SeekFrom::Start(*pos)).is_err() {
            return E_FAIL;
        }
        match with_input(pv, cb, |src| file.write_all(src)) {
            Ok(()) => {
                *pos += u64::from(cb);
                if *pos > *len {
                    *len = *pos;
                }
                store(pcbwritten, cb);
                windows::core::HRESULT(0)
            }
            Err(_) => E_FAIL,
        }
    }
}

impl IStream_Impl for FileStream_Impl {
    fn Seek(&self, dlibmove: i64, dworigin: windows::Win32::System::Com::STREAM_SEEK, plibnewposition: *mut u64) -> windows::core::Result<()> {
        let Ok(mut pos) = self.this.pos.try_borrow_mut() else {
            return Err(windows::core::Error::from(E_FAIL));
        };
        let len = *self.this.len.try_borrow().map_err(|_| windows::core::Error::from(E_FAIL))?;
        let next = if dworigin == STREAM_SEEK_SET {
            u64::try_from(dlibmove).unwrap_or(0)
        } else if dworigin == STREAM_SEEK_CUR {
            pos.saturating_add_signed(dlibmove)
        } else if dworigin == STREAM_SEEK_END {
            len.saturating_add_signed(dlibmove)
        } else {
            return Err(windows::core::Error::from(E_FAIL));
        };
        *pos = next;
        store(plibnewposition, next);
        Ok(())
    }

    fn SetSize(&self, libnewsize: u64) -> windows::core::Result<()> {
        let file = self.this.file.try_borrow_mut().map_err(|_| windows::core::Error::from(E_FAIL))?;
        file.set_len(libnewsize).map_err(|_| windows::core::Error::from(E_FAIL))?;
        *self.this.len.try_borrow_mut().map_err(|_| windows::core::Error::from(E_FAIL))? = libnewsize;
        Ok(())
    }

    fn CopyTo(&self, _pstm: windows::core::Ref<'_, IStream>, _cb: u64, _pcbread: *mut u64, _pcbwritten: *mut u64) -> windows::core::Result<()> {
        Err(windows::core::Error::from(E_NOTIMPL))
    }

    fn Commit(&self, _grfcommitflags: &windows::Win32::System::Com::STGC) -> windows::core::Result<()> {
        self.this.file.try_borrow_mut().map_err(|_| windows::core::Error::from(E_FAIL))?.flush().map_err(|_| windows::core::Error::from(E_FAIL))
    }

    fn Revert(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn LockRegion(&self, _liboffset: u64, _cb: u64, _dwlocktype: &windows::Win32::System::Com::LOCKTYPE) -> windows::core::Result<()> {
        Ok(())
    }

    fn UnlockRegion(&self, _liboffset: u64, _cb: u64, _dwlocktype: u32) -> windows::core::Result<()> {
        Ok(())
    }

    fn Stat(&self, pstatstg: *mut STATSTG, _grfstatflag: &windows::Win32::System::Com::STATFLAG) -> windows::core::Result<()> {
        if pstatstg.is_null() {
            return Err(windows::core::Error::from(E_FAIL));
        }
        let len = *self.this.len.try_borrow().map_err(|_| windows::core::Error::from(E_FAIL))?;
        store(pstatstg, STATSTG { r#type: STGTY_STREAM.0 as u32, cbSize: len, ..STATSTG::default() });
        let _ = STATFLAG_NONAME;
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IStream> {
        Err(windows::core::Error::from(E_NOTIMPL))
    }
}

// ---------------------------------------------------------------------------------------------
// Banded data callback
// ---------------------------------------------------------------------------------------------

pub(crate) struct Bands {
    pub(crate) file: Option<std::fs::File>,
    pub(crate) path: Option<PathBuf>,
    pub(crate) pages: Vec<Page>,
    pub(crate) limit: usize,
    pub(crate) rule: PageRule,
    pub(crate) error: Option<String>,
    pub(crate) progress: Option<ProgressToken>,
    /// Uncompressed BMP estimate for the current page (header + padded rows).
    pub(crate) expected_bytes: Option<u64>,
}

#[implement(IWiaDataCallback)]
struct BandSink {
    state: Rc<RefCell<Bands>>,
}

/// The banded data callback object for `state`, ready to pass to `idtGetBandedData`.
pub(crate) fn band_callback(state: &Rc<RefCell<Bands>>) -> IWiaDataCallback {
    BandSink { state: Rc::clone(state) }.into()
}

impl IWiaDataCallback_Impl for BandSink_Impl {
    fn BandedDataCallback(
        &self,
        lmessage: i32,
        _lstatus: i32,
        lpercentcomplete: i32,
        loffset: i32,
        llength: i32,
        _lreserved: i32,
        _lreslength: i32,
        pbbuffer: *mut u8,
    ) -> windows::core::Result<()> {
        let message = u32::try_from(lmessage).unwrap_or(0);
        let mut state = self.this.state.try_borrow_mut().map_err(|_| windows::core::Error::from(E_FAIL))?;
        if message == IT_MSG_NEW_PAGE || message == IT_MSG_TERMINATION {
            if let Err(error) = finish_band(&mut state) {
                state.error = Some(error);
                return Err(windows::core::Error::from(E_FAIL));
            }
            if state.pages.len() >= state.limit {
                return Err(windows::core::Error::from(WIA_ERROR_PAPER_EMPTY));
            }
            return Ok(());
        }
        if message != IT_MSG_DATA || pbbuffer.is_null() || llength <= 0 {
            return Ok(());
        }
        if state.pages.len() >= state.limit {
            return Err(windows::core::Error::from(WIA_ERROR_PAPER_EMPTY));
        }
        let len = u32::try_from(llength).unwrap_or(0);
        let offset = u64::try_from(loffset).unwrap_or(0);
        if state.file.is_none() {
            match temp_page() {
                Ok((file, path)) => {
                    state.path = Some(path);
                    state.file = Some(file);
                }
                Err(error) => {
                    state.error = Some(error.to_string());
                    return Err(windows::core::Error::from(E_FAIL));
                }
            }
        }
        let Some(file) = state.file.as_mut() else { return Err(windows::core::Error::from(E_FAIL)) };
        let written = with_input(pbbuffer.cast_const().cast(), len, |src| file.seek(SeekFrom::Start(offset)).and_then(|_| file.write_all(src)));
        if written.is_err() {
            state.error = Some("a scan band could not be written".into());
            return Err(windows::core::Error::from(E_FAIL));
        }
        // Driver percent when it fills it in; otherwise how far the band buffer has walked the page.
        let received = offset.saturating_add(u64::from(len));
        let pct = (0..=100).contains(&lpercentcomplete).then_some(lpercentcomplete as u8);
        if let Some(progress) = &state.progress {
            progress.set_transfer(pct, received, state.expected_bytes);
        }
        Ok(())
    }
}

/// Close the band file, read the finished page back and keep it.
pub(crate) fn finish_band(state: &mut Bands) -> Result<(), String> {
    let Some(mut file) = state.file.take() else { return Ok(()) };
    file.flush().map_err(|e| e.to_string())?;
    let path = state.path.take();
    drop(file);
    let Some(path) = path else { return Ok(()) };
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&path);
    if !bytes.is_empty() {
        let page = page_from_bytes(&bytes, state.rule, state.pages.len());
        if let Some(progress) = &state.progress {
            progress.push_page(page.clone());
        }
        state.pages.push(page);
    }
    Ok(())
}
