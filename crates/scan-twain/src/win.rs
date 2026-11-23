//! Windows 64-bit TWAIN 2.x FFI and acquisition state machine.

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::dib::dib_to_bmp;
use crate::{PaperSource, PixelMode, ScanRequest, ScannedPage, TwainDevice, TwainError};

// ── Win32 FFI ────────────────────────────────────────────────────────────────────────────────

type Hmodule = *mut c_void;
type Hwnd = *mut c_void;
type Handle = *mut c_void;
type Hinstance = *mut c_void;
type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

const GMEM_MOVEABLE: u32 = 0x0002;
const COINIT_APARTMENTTHREADED: u32 = 0x2;
const WS_POPUP: u32 = 0x8000_0000;
const PM_REMOVE: u32 = 0x0001;
const QS_ALLINPUT: u32 = 0x04FF;
const WM_QUIT: u32 = 0x0012;

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    pt: Point,
}

#[repr(C)]
struct WndClassExW {
    cb_size: u32,
    style: u32,
    lpfn_wnd_proc: WndProc,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: Hinstance,
    h_icon: Handle,
    h_cursor: Handle,
    hbr_background: Handle,
    lpsz_menu_name: *const u16,
    lpsz_class_name: *const u16,
    h_icon_sm: Handle,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(lp_lib_file_name: *const u16) -> Hmodule;
    fn GetProcAddress(h_module: Hmodule, lp_proc_name: *const u8) -> *mut c_void;
    fn FreeLibrary(h_lib_module: Hmodule) -> i32;
    fn GetModuleHandleW(lp_module_name: *const u16) -> Hinstance;
    fn GetSystemDirectoryW(lp_buffer: *mut u16, u_size: u32) -> u32;
    fn GlobalAlloc(u_flags: u32, dw_bytes: usize) -> Handle;
    fn GlobalLock(h_mem: Handle) -> *mut c_void;
    fn GlobalUnlock(h_mem: Handle) -> i32;
    fn GlobalFree(h_mem: Handle) -> Handle;
    fn GlobalSize(h_mem: Handle) -> usize;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassExW(lpwcx: *const WndClassExW) -> u16;
    fn CreateWindowExW(
        dw_ex_style: u32,
        lp_class_name: *const u16,
        lp_window_name: *const u16,
        dw_style: u32,
        x: i32,
        y: i32,
        n_width: i32,
        n_height: i32,
        h_wnd_parent: Hwnd,
        h_menu: Handle,
        h_instance: Hinstance,
        lp_param: *mut c_void,
    ) -> Hwnd;
    fn DestroyWindow(h_wnd: Hwnd) -> i32;
    fn DefWindowProcW(h_wnd: Hwnd, msg: u32, w_param: usize, l_param: isize) -> isize;
    fn PeekMessageW(lp_msg: *mut Msg, h_wnd: Hwnd, w_msg_filter_min: u32, w_msg_filter_max: u32, w_remove_msg: u32) -> i32;
    fn MsgWaitForMultipleObjects(n_count: u32, p_handles: *const Handle, f_wait_all: i32, dw_milliseconds: u32, dw_wake_mask: u32) -> u32;
    fn TranslateMessage(lp_msg: *const Msg) -> i32;
    fn DispatchMessageW(lp_msg: *const Msg) -> isize;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(pv_reserved: *mut c_void, dw_co_init: u32) -> i32;
    fn CoUninitialize();
}

unsafe extern "system" fn hidden_wnd_proc(hwnd: Hwnd, msg: u32, wp: usize, lp: isize) -> isize {
    // SAFETY: Standard Win32 default window procedure forwarding.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ── TWAIN 2.x Types (#pragma pack(push, 2)) ──────────────────────────────────────────────────

const DG_CONTROL: u32 = 0x0001;
const DG_IMAGE: u32 = 0x0002;
const DF_DSM2: u32 = 0x1000_0000;
const DF_APP2: u32 = 0x2000_0000;

const DAT_CAPABILITY: u16 = 0x0001;
const DAT_EVENT: u16 = 0x0002;
const DAT_IDENTITY: u16 = 0x0003;
const DAT_PARENT: u16 = 0x0004;
const DAT_PENDINGXFERS: u16 = 0x0005;
const DAT_USERINTERFACE: u16 = 0x0009;
const DAT_ENTRYPOINT: u16 = 0x0403;
const DAT_IMAGEINFO: u16 = 0x0101;
const DAT_IMAGENATIVEXFER: u16 = 0x0104;

const MSG_GET: u16 = 0x0001;
const MSG_GETDEFAULT: u16 = 0x0003;
const MSG_GETFIRST: u16 = 0x0004;
const MSG_GETNEXT: u16 = 0x0005;
const MSG_SET: u16 = 0x0006;
const MSG_RESET: u16 = 0x0007;
const MSG_XFERREADY: u16 = 0x0101;
const MSG_CLOSEDSREQ: u16 = 0x0102;
const MSG_CLOSEDSOK: u16 = 0x0103;
const MSG_OPENDSM: u16 = 0x0301;
const MSG_CLOSEDSM: u16 = 0x0302;
const MSG_OPENDS: u16 = 0x0401;
const MSG_CLOSEDS: u16 = 0x0402;
const MSG_DISABLEDS: u16 = 0x0501;
const MSG_ENABLEDS: u16 = 0x0502;
const MSG_PROCESSEVENT: u16 = 0x0601;
const MSG_ENDXFER: u16 = 0x0701;

const TWRC_SUCCESS: u16 = 0;
const TWRC_FAILURE: u16 = 1;
const TWRC_CHECKSTATUS: u16 = 2;
const TWRC_CANCEL: u16 = 3;
const TWRC_DSEVENT: u16 = 4;
const TWRC_NOTDSEVENT: u16 = 5;
const TWRC_XFERDONE: u16 = 6;
const TWRC_ENDOFLIST: u16 = 7;

const TWON_ONEVALUE: u16 = 5;
const TWTY_INT16: u16 = 1;
const TWTY_UINT16: u16 = 4;
const TWTY_BOOL: u16 = 6;
const TWTY_FIX32: u16 = 7;

const CAP_XFERCOUNT: u16 = 0x0001;
const ICAP_PIXELTYPE: u16 = 0x0101;
const ICAP_UNITS: u16 = 0x0102;
const ICAP_XFERMECH: u16 = 0x0103;
const CAP_FEEDERENABLED: u16 = 0x1002;
const CAP_AUTOFEED: u16 = 0x1007;
const CAP_DUPLEXENABLED: u16 = 0x1013;
const ICAP_XRESOLUTION: u16 = 0x1118;
const ICAP_YRESOLUTION: u16 = 0x1119;

const TWLG_USERLOCALE: u16 = u16::MAX;
const TWCY_GERMANY: u16 = 49;

type DsmEntryFn =
    unsafe extern "system" fn(p_origin: *mut TwIdentity, p_dest: *mut TwIdentity, dg: u32, dat: u16, msg: u16, p_data: *mut c_void) -> u16;

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
struct TwVersion {
    major_num: u16,
    minor_num: u16,
    language: u16,
    country: u16,
    info: [u8; 34],
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
struct TwIdentity {
    id: u32,
    version: TwVersion,
    protocol_major: u16,
    protocol_minor: u16,
    supported_groups: u32,
    manufacturer: [u8; 34],
    product_family: [u8; 34],
    product_name: [u8; 34],
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
struct TwFix32 {
    whole: i16,
    frac: u16,
}

impl TwFix32 {
    fn from_u16(v: u16) -> u32 {
        (v as i16 as u16) as u32
    }

    fn to_f64(self) -> f64 {
        let w = self.whole;
        let f = self.frac;
        f64::from(w) + f64::from(f) / 65536.0
    }
}

#[repr(C, packed(2))]
struct TwCapability {
    cap: u16,
    con_type: u16,
    h_container: Handle,
}

#[repr(C, packed(2))]
struct TwOneValue {
    item_type: u16,
    item: u32,
}

#[repr(C, packed(2))]
struct TwUserInterface {
    show_ui: u16,
    modal_ui: u16,
    h_parent: Hwnd,
}

#[repr(C, packed(2))]
struct TwEvent {
    p_event: *mut c_void,
    tw_message: u16,
}

#[repr(C, packed(2))]
struct TwPendingXfers {
    count: u16,
    eoj: u32,
}

#[repr(C, packed(2))]
struct TwImageInfo {
    x_resolution: TwFix32,
    y_resolution: TwFix32,
    image_width: i32,
    image_length: i32,
    samples_per_pixel: i16,
    bits_per_sample: [i16; 8],
    bits_per_pixel: i16,
    planar: u16,
    pixel_type: i16,
    compression: u16,
}

#[repr(C, packed(2))]
struct TwEntryPoint {
    size: u32,
    dsm_entry: *mut c_void,
    dsm_mem_allocate: Option<unsafe extern "system" fn(u32) -> Handle>,
    dsm_mem_free: Option<unsafe extern "system" fn(Handle)>,
    dsm_mem_lock: Option<unsafe extern "system" fn(Handle) -> *mut c_void>,
    dsm_mem_unlock: Option<unsafe extern "system" fn(Handle)>,
}

fn fill_str32(dst: &mut [u8; 34], s: &str) {
    let bytes = s.as_bytes();
    let n = bytes.len().min(32);
    dst[..n].copy_from_slice(&bytes[..n]);
    dst[n] = 0;
}

fn read_str32(src: &[u8; 34]) -> String {
    let end = src.iter().position(|&b| b == 0).unwrap_or(src.len());
    String::from_utf8_lossy(&src[..end]).trim().to_string()
}

fn app_identity() -> TwIdentity {
    let mut id = TwIdentity {
        id: 0,
        version: TwVersion { major_num: 0, minor_num: 22, language: TWLG_USERLOCALE, country: TWCY_GERMANY, info: [0; 34] },
        protocol_major: 2,
        protocol_minor: 4,
        supported_groups: DG_CONTROL | DG_IMAGE | DF_APP2,
        manufacturer: [0; 34],
        product_family: [0; 34],
        product_name: [0; 34],
    };
    fill_str32(&mut id.version.info, "0.22");
    fill_str32(&mut id.manufacturer, "PdfCraft");
    fill_str32(&mut id.product_family, "PdfCraft");
    fill_str32(&mut id.product_name, "PdfCraft");
    id
}

fn zero_identity() -> TwIdentity {
    TwIdentity {
        id: 0,
        version: TwVersion { major_num: 0, minor_num: 0, language: 0, country: 0, info: [0; 34] },
        protocol_major: 0,
        protocol_minor: 0,
        supported_groups: 0,
        manufacturer: [0; 34],
        product_family: [0; 34],
        product_name: [0; 34],
    }
}

// ── DSM Loader & Guard ───────────────────────────────────────────────────────────────────────

struct DsmLib {
    module: Hmodule,
    entry: DsmEntryFn,
}

fn system32_dir() -> Option<PathBuf> {
    let mut buf = [0u16; 260];
    // SAFETY: `buf` is valid writable memory of length `buf.len()`.
    let len = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if len == 0 || len >= buf.len() {
        return None;
    }
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

impl DsmLib {
    fn load() -> Result<Self, TwainError> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            candidates.push(dir.join("TWAINDSM.dll"));
        }
        if let Some(sys) = system32_dir() {
            candidates.push(sys.join("TWAINDSM.dll"));
        }

        for path in candidates {
            if !path.is_file() {
                continue;
            }
            let wide = to_wide(&path.to_string_lossy());
            // SAFETY: `wide` is a valid NUL-terminated absolute UTF-16 path.
            let module = unsafe { LoadLibraryW(wide.as_ptr()) };
            if !module.is_null() {
                // SAFETY: `module` is a valid HMODULE; symbol name is NUL-terminated ASCII.
                let sym = unsafe { GetProcAddress(module, c"DSM_Entry".as_ptr().cast::<u8>()) };
                if !sym.is_null() {
                    // SAFETY: `DSM_Entry` matches the TWAIN 2.x `DsmEntryFn` ABI on Win64.
                    let entry: DsmEntryFn = unsafe { std::mem::transmute(sym) };
                    return Ok(Self { module, entry });
                }
                // SAFETY: `module` was loaded above and has no `DSM_Entry`.
                unsafe { FreeLibrary(module) };
            }
        }
        Err(TwainError::DsmNotFound)
    }
}

impl Drop for DsmLib {
    fn drop(&mut self) {
        if !self.module.is_null() {
            // SAFETY: `self.module` was returned by `LoadLibraryW` and is freed once.
            unsafe { FreeLibrary(self.module) };
        }
    }
}

struct HiddenWindow {
    hwnd: Hwnd,
}

impl HiddenWindow {
    fn create() -> Result<Self, TwainError> {
        let class_name = to_wide("PdfCraftTwainHostWindow");
        let title = to_wide("PdfCraft TWAIN");
        // SAFETY: Querying current process HINSTANCE and registering/creating a hidden window.
        unsafe {
            let hinst = GetModuleHandleW(ptr::null());
            let wc = WndClassExW {
                cb_size: std::mem::size_of::<WndClassExW>() as u32,
                style: 0,
                lpfn_wnd_proc: hidden_wnd_proc,
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance: hinst,
                h_icon: ptr::null_mut(),
                h_cursor: ptr::null_mut(),
                hbr_background: ptr::null_mut(),
                lpsz_menu_name: ptr::null(),
                lpsz_class_name: class_name.as_ptr(),
                h_icon_sm: ptr::null_mut(),
            };
            let _ = RegisterClassExW(&wc);
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                hinst,
                ptr::null_mut(),
            );
            if hwnd.is_null() {
                return Err(TwainError::Failed("could not create TWAIN message window".into()));
            }
            Ok(Self { hwnd })
        }
    }
}

impl Drop for HiddenWindow {
    fn drop(&mut self) {
        if !self.hwnd.is_null() {
            // SAFETY: `self.hwnd` was created on this thread and is destroyed on the same thread.
            unsafe { DestroyWindow(self.hwnd) };
        }
    }
}

struct ComGuard(bool);

impl ComGuard {
    fn init() -> Self {
        // SAFETY: Initializes COM apartment on this worker thread.
        let hr = unsafe { CoInitializeEx(ptr::null_mut(), COINIT_APARTMENTTHREADED) };
        Self(hr >= 0)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: Balances successful `CoInitializeEx` on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

struct TwainMem {
    alloc: Option<unsafe extern "system" fn(u32) -> Handle>,
    free: Option<unsafe extern "system" fn(Handle)>,
    lock: Option<unsafe extern "system" fn(Handle) -> *mut c_void>,
    unlock: Option<unsafe extern "system" fn(Handle)>,
}

impl TwainMem {
    fn query(dsm: &DsmLib, app: &mut TwIdentity) -> Self {
        let mut ep = TwEntryPoint {
            size: std::mem::size_of::<TwEntryPoint>() as u32,
            dsm_entry: ptr::null_mut(),
            dsm_mem_allocate: None,
            dsm_mem_free: None,
            dsm_mem_lock: None,
            dsm_mem_unlock: None,
        };
        if (app.supported_groups & DF_DSM2) != 0 {
            // SAFETY: `ep` has valid `size` and lives for the duration of the call.
            let rc = unsafe { (dsm.entry)(app, ptr::null_mut(), DG_CONTROL, DAT_ENTRYPOINT, MSG_GET, (&raw mut ep).cast()) };
            if rc == TWRC_SUCCESS {
                return Self { alloc: ep.dsm_mem_allocate, free: ep.dsm_mem_free, lock: ep.dsm_mem_lock, unlock: ep.dsm_mem_unlock };
            }
        }
        Self { alloc: None, free: None, lock: None, unlock: None }
    }

    unsafe fn alloc(&self, size: u32) -> Handle {
        if let Some(f) = self.alloc { unsafe { f(size) } } else { unsafe { GlobalAlloc(GMEM_MOVEABLE, size as usize) } }
    }

    unsafe fn free(&self, h: Handle) {
        if h.is_null() {
            return;
        }
        if let Some(f) = self.free {
            unsafe { f(h) };
        } else {
            unsafe { GlobalFree(h) };
        }
    }

    unsafe fn lock(&self, h: Handle) -> *mut c_void {
        if let Some(f) = self.lock { unsafe { f(h) } } else { unsafe { GlobalLock(h) } }
    }

    unsafe fn unlock(&self, h: Handle) {
        if let Some(f) = self.unlock {
            unsafe { f(h) };
        } else {
            unsafe { GlobalUnlock(h) };
        }
    }
}

fn set_cap_one_value(dsm: &DsmLib, mem: &TwainMem, app: &mut TwIdentity, ds: &mut TwIdentity, cap: u16, item_type: u16, item: u32) -> bool {
    // SAFETY: Allocates a `TwOneValue` container, populates it, passes it to `DAT_CAPABILITY / MSG_SET`,
    // and always frees the handle afterwards per TWAIN 2.x ownership rules.
    unsafe {
        let h = mem.alloc(std::mem::size_of::<TwOneValue>() as u32);
        if h.is_null() {
            return false;
        }
        let ptr = mem.lock(h).cast::<TwOneValue>();
        if ptr.is_null() {
            mem.free(h);
            return false;
        }
        ptr::write_unaligned(ptr, TwOneValue { item_type, item });
        mem.unlock(h);

        let mut tw_cap = TwCapability { cap, con_type: TWON_ONEVALUE, h_container: h };
        let rc = (dsm.entry)(app, ds, DG_CONTROL, DAT_CAPABILITY, MSG_SET, (&raw mut tw_cap).cast());
        mem.free(h);
        rc == TWRC_SUCCESS || rc == TWRC_CHECKSTATUS
    }
}

// ── Public Entry Points ──────────────────────────────────────────────────────────────────────

pub fn list_devices() -> Result<Vec<TwainDevice>, TwainError> {
    let _com = ComGuard::init();
    let dsm = DsmLib::load()?;
    let win = HiddenWindow::create()?;
    let mut app = app_identity();
    let mut hwnd = win.hwnd;

    // SAFETY: Opens DSM in State 3 with valid `app` identity and `hwnd` parent pointer.
    let rc = unsafe { (dsm.entry)(&raw mut app, ptr::null_mut(), DG_CONTROL, DAT_PARENT, MSG_OPENDSM, (&raw mut hwnd).cast()) };
    if rc != TWRC_SUCCESS {
        return Err(TwainError::Failed(format!("failed to open TWAIN Data Source Manager (rc={rc})")));
    }

    let mut devices = Vec::new();
    let mut ds = zero_identity();
    // SAFETY: Enumerates TWAIN data sources using `MSG_GETFIRST` / `MSG_GETNEXT`.
    let mut rc = unsafe { (dsm.entry)(&raw mut app, ptr::null_mut(), DG_CONTROL, DAT_IDENTITY, MSG_GETFIRST, (&raw mut ds).cast()) };
    while rc == TWRC_SUCCESS {
        let name = read_str32(&ds.product_name);
        if !name.is_empty() {
            devices.push(TwainDevice { name, manufacturer: read_str32(&ds.manufacturer), product_family: read_str32(&ds.product_family) });
        }
        ds = zero_identity();
        rc = unsafe { (dsm.entry)(&raw mut app, ptr::null_mut(), DG_CONTROL, DAT_IDENTITY, MSG_GETNEXT, (&raw mut ds).cast()) };
        if rc == TWRC_ENDOFLIST {
            break;
        }
    }

    // SAFETY: Balances `MSG_OPENDSM`.
    unsafe {
        let _ = (dsm.entry)(&raw mut app, ptr::null_mut(), DG_CONTROL, DAT_PARENT, MSG_CLOSEDSM, (&raw mut hwnd).cast());
    }

    Ok(devices)
}

pub fn acquire(request: &ScanRequest, cancel: &AtomicBool) -> Result<Vec<ScannedPage>, TwainError> {
    let _com = ComGuard::init();
    let dsm = DsmLib::load()?;
    let win = HiddenWindow::create()?;
    let mut app = app_identity();
    let mut hwnd = win.hwnd;

    // State 2 -> 3: Open DSM.
    // SAFETY: Valid `app` and `hwnd`.
    let rc = unsafe { (dsm.entry)(&raw mut app, ptr::null_mut(), DG_CONTROL, DAT_PARENT, MSG_OPENDSM, (&raw mut hwnd).cast()) };
    if rc != TWRC_SUCCESS {
        return Err(TwainError::Failed(format!("failed to open TWAIN Data Source Manager (rc={rc})")));
    }

    let result = acquire_in_dsm(&dsm, &mut app, hwnd, request, cancel);

    // State 3 -> 2: Close DSM.
    // SAFETY: Balances `MSG_OPENDSM`.
    unsafe {
        let _ = (dsm.entry)(&raw mut app, ptr::null_mut(), DG_CONTROL, DAT_PARENT, MSG_CLOSEDSM, (&raw mut hwnd).cast());
    }

    result
}

fn select_source(dsm: &DsmLib, app: &mut TwIdentity, wanted: Option<&str>) -> Result<TwIdentity, TwainError> {
    let mut ds = zero_identity();
    if let Some(target) = wanted {
        // SAFETY: Enumerates installed sources to find the exact requested `ProductName`.
        let mut rc = unsafe { (dsm.entry)(app, ptr::null_mut(), DG_CONTROL, DAT_IDENTITY, MSG_GETFIRST, (&raw mut ds).cast()) };
        while rc == TWRC_SUCCESS {
            if read_str32(&ds.product_name) == target {
                return Ok(ds);
            }
            ds = zero_identity();
            rc = unsafe { (dsm.entry)(app, ptr::null_mut(), DG_CONTROL, DAT_IDENTITY, MSG_GETNEXT, (&raw mut ds).cast()) };
        }
        return Err(TwainError::Failed(format!("scanner '{target}' was not found")));
    }

    // SAFETY: Queries default TWAIN source.
    let rc = unsafe { (dsm.entry)(app, ptr::null_mut(), DG_CONTROL, DAT_IDENTITY, MSG_GETDEFAULT, (&raw mut ds).cast()) };
    if rc == TWRC_SUCCESS {
        return Ok(ds);
    }
    // Fallback to first source.
    let rc = unsafe { (dsm.entry)(app, ptr::null_mut(), DG_CONTROL, DAT_IDENTITY, MSG_GETFIRST, (&raw mut ds).cast()) };
    if rc == TWRC_SUCCESS {
        return Ok(ds);
    }
    Err(TwainError::NoDevices)
}

fn acquire_in_dsm(
    dsm: &DsmLib,
    app: &mut TwIdentity,
    hwnd: Hwnd,
    request: &ScanRequest,
    cancel: &AtomicBool,
) -> Result<Vec<ScannedPage>, TwainError> {
    let mem = TwainMem::query(dsm, app);
    let mut ds = select_source(dsm, app, request.device_name.as_deref())?;

    // State 3 -> 4: Open DS.
    // SAFETY: `ds` is a valid identity returned by DSM.
    let rc = unsafe { (dsm.entry)(app, ptr::null_mut(), DG_CONTROL, DAT_IDENTITY, MSG_OPENDS, (&raw mut ds).cast()) };
    if rc != TWRC_SUCCESS {
        return Err(TwainError::Failed(format!("failed to open scanner '{}' (rc={rc})", read_str32(&ds.product_name))));
    }

    let result = acquire_in_ds(dsm, &mem, app, &mut ds, hwnd, request, cancel);

    // State 4 -> 3: Close DS.
    // SAFETY: Balances `MSG_OPENDS`.
    unsafe {
        let _ = (dsm.entry)(app, ptr::null_mut(), DG_CONTROL, DAT_IDENTITY, MSG_CLOSEDS, (&raw mut ds).cast());
    }

    result
}

fn acquire_in_ds(
    dsm: &DsmLib,
    mem: &TwainMem,
    app: &mut TwIdentity,
    ds: &mut TwIdentity,
    hwnd: Hwnd,
    request: &ScanRequest,
    cancel: &AtomicBool,
) -> Result<Vec<ScannedPage>, TwainError> {
    // Negotiate capabilities in State 4 (failures are logged and non-fatal so driver defaults still work).
    let _ = set_cap_one_value(dsm, mem, app, ds, ICAP_XFERMECH, TWTY_UINT16, 0); // TWSX_NATIVE
    let _ = set_cap_one_value(dsm, mem, app, ds, CAP_XFERCOUNT, TWTY_INT16, u32::from(u16::MAX)); // -1 (all pages)
    let _ = set_cap_one_value(dsm, mem, app, ds, ICAP_UNITS, TWTY_UINT16, 0); // TWUN_INCHES

    let pixel_type = match request.pixel_mode {
        PixelMode::BlackAndWhite => 0,
        PixelMode::Grayscale => 1,
        PixelMode::Color => 2,
    };
    let _ = set_cap_one_value(dsm, mem, app, ds, ICAP_PIXELTYPE, TWTY_UINT16, pixel_type);

    if request.dpi > 0 {
        let fix = TwFix32::from_u16(request.dpi);
        let _ = set_cap_one_value(dsm, mem, app, ds, ICAP_XRESOLUTION, TWTY_FIX32, fix);
        let _ = set_cap_one_value(dsm, mem, app, ds, ICAP_YRESOLUTION, TWTY_FIX32, fix);
    }

    match request.source {
        PaperSource::DriverDefault => {}
        PaperSource::Flatbed => {
            let _ = set_cap_one_value(dsm, mem, app, ds, CAP_FEEDERENABLED, TWTY_BOOL, 0);
        }
        PaperSource::Feeder => {
            let _ = set_cap_one_value(dsm, mem, app, ds, CAP_FEEDERENABLED, TWTY_BOOL, 1);
            let _ = set_cap_one_value(dsm, mem, app, ds, CAP_AUTOFEED, TWTY_BOOL, 1);
            let _ = set_cap_one_value(dsm, mem, app, ds, CAP_DUPLEXENABLED, TWTY_BOOL, 0);
        }
        PaperSource::FeederDuplex => {
            let _ = set_cap_one_value(dsm, mem, app, ds, CAP_FEEDERENABLED, TWTY_BOOL, 1);
            let _ = set_cap_one_value(dsm, mem, app, ds, CAP_AUTOFEED, TWTY_BOOL, 1);
            let _ = set_cap_one_value(dsm, mem, app, ds, CAP_DUPLEXENABLED, TWTY_BOOL, 1);
        }
    }

    // State 4 -> 5: Enable DS.
    let mut ui = TwUserInterface { show_ui: u16::from(request.show_ui), modal_ui: 0, h_parent: hwnd };
    // SAFETY: Valid `ui` struct with `hwnd` created on this thread.
    let rc = unsafe { (dsm.entry)(app, ds, DG_CONTROL, DAT_USERINTERFACE, MSG_ENABLEDS, (&raw mut ui).cast()) };
    if rc != TWRC_SUCCESS && rc != TWRC_CHECKSTATUS {
        return Err(TwainError::Failed(format!("scanner refused to start (rc={rc})")));
    }

    let pages = pump_and_transfer(dsm, app, ds, f64::from(request.dpi), cancel);

    // State 5 -> 4: Disable DS.
    // SAFETY: Balances `MSG_ENABLEDS`.
    unsafe {
        let _ = (dsm.entry)(app, ds, DG_CONTROL, DAT_USERINTERFACE, MSG_DISABLEDS, (&raw mut ui).cast());
    }

    pages
}

fn pump_and_transfer(
    dsm: &DsmLib,
    app: &mut TwIdentity,
    ds: &mut TwIdentity,
    fallback_dpi: f64,
    cancel: &AtomicBool,
) -> Result<Vec<ScannedPage>, TwainError> {
    let mut msg = Msg { hwnd: ptr::null_mut(), message: 0, w_param: 0, l_param: 0, time: 0, pt: Point { x: 0, y: 0 } };

    // SAFETY: Standard Win32 TWAIN event loop with periodic wakeups so cancellation is responsive.
    unsafe {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(TwainError::Cancelled);
            }
            while PeekMessageW(&raw mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    return Err(TwainError::Cancelled);
                }
                let mut tw_event = TwEvent { p_event: (&raw mut msg).cast(), tw_message: 0 };
                let rc = (dsm.entry)(app, ds, DG_CONTROL, DAT_EVENT, MSG_PROCESSEVENT, (&raw mut tw_event).cast());

                if rc == TWRC_FAILURE {
                    return Err(TwainError::Failed("scanner event processing failed".into()));
                }
                if rc == TWRC_DSEVENT {
                    match tw_event.tw_message {
                        MSG_XFERREADY => {
                            return transfer_all_pages(dsm, app, ds, fallback_dpi, cancel);
                        }
                        MSG_CLOSEDSREQ | MSG_CLOSEDSOK => {
                            return Err(TwainError::Cancelled);
                        }
                        _ => {}
                    }
                }
                if rc == TWRC_NOTDSEVENT {
                    let _ = TranslateMessage(&raw const msg);
                    let _ = DispatchMessageW(&raw const msg);
                }
            }
            let _ = MsgWaitForMultipleObjects(0, ptr::null(), 0, 150, QS_ALLINPUT);
        }
    }
}

unsafe fn reset_pending(dsm: &DsmLib, app: &mut TwIdentity, ds: &mut TwIdentity, pending: &mut TwPendingXfers) {
    if pending.count != 0 {
        let _ = unsafe { (dsm.entry)(app, ds, DG_CONTROL, DAT_PENDINGXFERS, MSG_RESET, (pending as *mut TwPendingXfers).cast()) };
        pending.count = 0;
    }
}

unsafe fn transfer_all_pages(
    dsm: &DsmLib,
    app: &mut TwIdentity,
    ds: &mut TwIdentity,
    fallback_dpi: f64,
    cancel: &AtomicBool,
) -> Result<Vec<ScannedPage>, TwainError> {
    let mut pages = Vec::new();
    let mut pending = TwPendingXfers { count: 1, eoj: 0 };

    while pending.count != 0 {
        if cancel.load(Ordering::Relaxed) {
            unsafe { reset_pending(dsm, app, ds, &mut pending) };
            return if pages.is_empty() { Err(TwainError::Cancelled) } else { Ok(pages) };
        }
        let mut info = TwImageInfo {
            x_resolution: TwFix32 { whole: 0, frac: 0 },
            y_resolution: TwFix32 { whole: 0, frac: 0 },
            image_width: 0,
            image_length: 0,
            samples_per_pixel: 0,
            bits_per_sample: [0; 8],
            bits_per_pixel: 0,
            planar: 0,
            pixel_type: 0,
            compression: 0,
        };
        let info_rc = unsafe { (dsm.entry)(app, ds, DG_IMAGE, DAT_IMAGEINFO, MSG_GET, (&raw mut info).cast()) };
        let (dpi_x, dpi_y) = if info_rc == TWRC_SUCCESS {
            let x = info.x_resolution.to_f64();
            let y = info.y_resolution.to_f64();
            (if x.is_finite() && x > 0.0 { x } else { fallback_dpi }, if y.is_finite() && y > 0.0 { y } else { fallback_dpi })
        } else {
            (fallback_dpi, fallback_dpi)
        };

        let mut hdib: Handle = ptr::null_mut();
        let rc = unsafe { (dsm.entry)(app, ds, DG_IMAGE, DAT_IMAGENATIVEXFER, MSG_GET, (&raw mut hdib).cast()) };
        match rc {
            TWRC_XFERDONE => {
                if !hdib.is_null() {
                    let size = unsafe { GlobalSize(hdib) };
                    let ptr = unsafe { GlobalLock(hdib) }.cast::<u8>();
                    let bmp_res = if !ptr.is_null() && size >= 40 {
                        let slice = unsafe { std::slice::from_raw_parts(ptr, size) };
                        dib_to_bmp(slice, dpi_x, dpi_y)
                    } else {
                        Err("scanner returned an empty bitmap handle".into())
                    };
                    unsafe {
                        GlobalUnlock(hdib);
                        GlobalFree(hdib);
                    }
                    match bmp_res {
                        Ok(bmp_bytes) => pages.push(ScannedPage { bmp_bytes, dpi_x, dpi_y }),
                        Err(e) => {
                            let _ = unsafe { (dsm.entry)(app, ds, DG_CONTROL, DAT_PENDINGXFERS, MSG_ENDXFER, (&raw mut pending).cast()) };
                            unsafe { reset_pending(dsm, app, ds, &mut pending) };
                            return Err(TwainError::Failed(e));
                        }
                    }
                }
                let end_rc = unsafe { (dsm.entry)(app, ds, DG_CONTROL, DAT_PENDINGXFERS, MSG_ENDXFER, (&raw mut pending).cast()) };
                if end_rc != TWRC_SUCCESS {
                    unsafe { reset_pending(dsm, app, ds, &mut pending) };
                    break;
                }
            }
            TWRC_CANCEL => {
                let _ = unsafe { (dsm.entry)(app, ds, DG_CONTROL, DAT_PENDINGXFERS, MSG_ENDXFER, (&raw mut pending).cast()) };
                unsafe { reset_pending(dsm, app, ds, &mut pending) };
                if pages.is_empty() {
                    return Err(TwainError::Cancelled);
                }
                break;
            }
            _ => {
                let _ = unsafe { (dsm.entry)(app, ds, DG_CONTROL, DAT_PENDINGXFERS, MSG_ENDXFER, (&raw mut pending).cast()) };
                unsafe { reset_pending(dsm, app, ds, &mut pending) };
                if pages.is_empty() {
                    return Err(TwainError::Failed(format!("image transfer failed (rc={rc})")));
                }
                break;
            }
        }
    }

    if pages.is_empty() { Err(TwainError::Cancelled) } else { Ok(pages) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twain_struct_sizes_match_win64_pack2_abi() {
        assert_eq!(std::mem::size_of::<TwVersion>(), 42);
        assert_eq!(std::mem::size_of::<TwIdentity>(), 156);
        assert_eq!(std::mem::size_of::<TwFix32>(), 4);
        assert_eq!(std::mem::size_of::<TwOneValue>(), 6);
        assert_eq!(std::mem::size_of::<TwCapability>(), 12);
        assert_eq!(std::mem::size_of::<TwUserInterface>(), 12);
        assert_eq!(std::mem::size_of::<TwEvent>(), 10);
        assert_eq!(std::mem::size_of::<TwPendingXfers>(), 6);
    }

    #[test]
    fn list_devices_runs_without_panicking() {
        match list_devices() {
            Ok(devs) => {
                for d in &devs {
                    println!("Found TWAIN device: {} ({})", d.name, d.manufacturer);
                }
            }
            Err(TwainError::DsmNotFound) => {
                println!("TWAINDSM.dll not installed in test environment");
            }
            Err(e) => panic!("unexpected TWAIN error: {e}"),
        }
    }
}
