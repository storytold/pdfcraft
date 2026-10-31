//! The File Explorer preview handler: an in-process COM server exporting `DllGetClassObject`
//! and `DllCanUnloadNow`, whose one class implements `IInitializeWithStream` (Explorer hands it
//! the file as a stream), `IObjectWithSite`, and `IPreviewHandler` (it draws the first page into
//! the preview pane).
//!
//! Explorer hosts preview handlers in a separate surrogate process; this still never panics across
//! the COM boundary (`catch_unwind` on every entry). Failures return `E_FAIL` and the pane stays
//! blank.
//!
//! `unsafe` is confined to this module: exported functions dereference COM pointers, and GDI
//! writes the bitmap.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use crate::{PreviewBitmap, render_page_fit};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, COLORREF, E_FAIL, E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, ERROR_ALREADY_INITIALIZED, HWND,
    RECT, S_FALSE,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateSolidBrush, DIB_RGB_COLORS, DeleteObject, FillRect, GetDC, ReleaseDC, SRCCOPY, StretchDIBits,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl, IStream, STATFLAG_NONAME, STATSTG};
use windows::Win32::System::Ole::{IObjectWithSite, IObjectWithSite_Impl};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithStream, IInitializeWithStream_Impl};
use windows::Win32::UI::Shell::{IPreviewHandler, IPreviewHandler_Impl};
use windows::Win32::UI::WindowsAndMessaging::MSG;
use windows_core::{BOOL, GUID, HRESULT, IUnknown, Interface, Ref, implement};

/// The preview handler's class id, registered under `PdfCraft.Document\ShellEx` and PreviewHandlers.
pub const CLSID: GUID = GUID::from_u128(0xc31a5f44_7e02_4d9a_a1b3_8f2e6d4c0a91);

/// Files larger than this aren't read for a preview (the whole file is parsed in memory).
pub const MAX_FILE_BYTES: u64 = 64 << 20;

/// The preview pane's host window, target rectangle and document bytes.
#[implement(IInitializeWithStream, IObjectWithSite, IPreviewHandler)]
#[derive(Default)]
struct Handler {
    bytes: Mutex<Option<Arc<Vec<u8>>>>,
    hwnd: Mutex<Option<HWND>>,
    rect: Mutex<RECT>,
}

fn guarded<T>(f: impl FnOnce() -> windows_core::Result<T>) -> windows_core::Result<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(_) => Err(E_FAIL.into()),
    }
}

impl IInitializeWithStream_Impl for Handler_Impl {
    fn Initialize(&self, stream: Ref<IStream>, _mode: u32) -> windows_core::Result<()> {
        guarded(|| {
            let mut slot = self.bytes.lock().unwrap_or_else(|p| p.into_inner());
            if slot.is_some() {
                return Err(ERROR_ALREADY_INITIALIZED.to_hresult().into());
            }
            *slot = Some(Arc::new(read_all(stream.ok()?)?));
            Ok(())
        })
    }
}

impl IObjectWithSite_Impl for Handler_Impl {
    fn SetSite(&self, _punksite: Ref<IUnknown>) -> windows_core::Result<()> {
        Ok(())
    }

    fn GetSite(&self, _riid: *const GUID, ppvsite: *mut *mut c_void) -> windows_core::Result<()> {
        if ppvsite.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: non-null out-parameter; no site is exposed.
        unsafe {
            *ppvsite = std::ptr::null_mut();
        }
        Err(E_FAIL.into())
    }
}

impl IPreviewHandler_Impl for Handler_Impl {
    fn SetWindow(&self, hwnd: HWND, prc: *const RECT) -> windows_core::Result<()> {
        if prc.is_null() {
            return Err(E_POINTER.into());
        }
        guarded(|| {
            *self.hwnd.lock().unwrap_or_else(|p| p.into_inner()) = Some(hwnd);
            *self.rect.lock().unwrap_or_else(|p| p.into_inner()) = unsafe { *prc };
            Ok(())
        })
    }

    fn SetRect(&self, prc: *const RECT) -> windows_core::Result<()> {
        if prc.is_null() {
            return Err(E_POINTER.into());
        }
        guarded(|| {
            *self.rect.lock().unwrap_or_else(|p| p.into_inner()) = unsafe { *prc };
            Ok(())
        })
    }

    fn DoPreview(&self) -> windows_core::Result<()> {
        guarded(|| {
            let bytes = self.bytes.lock().unwrap_or_else(|p| p.into_inner()).clone().ok_or(windows_core::Error::from(E_INVALIDARG))?;
            let hwnd = self.hwnd.lock().unwrap_or_else(|p| p.into_inner()).ok_or(windows_core::Error::from(E_FAIL))?;
            let rect = *self.rect.lock().unwrap_or_else(|p| p.into_inner());
            let (max_w, max_h) = rect_size(&rect)?;
            let thumbnail = render_page_fit(bytes, max_w, max_h).map_err(|_| windows_core::Error::from(E_FAIL))?;
            paint(hwnd, &rect, &thumbnail)
        })
    }

    fn Unload(&self) -> windows_core::Result<()> {
        guarded(|| {
            *self.bytes.lock().unwrap_or_else(|p| p.into_inner()) = None;
            *self.hwnd.lock().unwrap_or_else(|p| p.into_inner()) = None;
            *self.rect.lock().unwrap_or_else(|p| p.into_inner()) = RECT::default();
            Ok(())
        })
    }

    fn SetFocus(&self) -> windows_core::Result<()> {
        guarded(|| {
            if let Some(hwnd) = *self.hwnd.lock().unwrap_or_else(|p| p.into_inner()) {
                // SAFETY: Explorer gave us this window handle in SetWindow.
                unsafe {
                    let _ = SetFocus(Some(hwnd));
                }
            }
            Ok(())
        })
    }

    fn QueryFocus(&self) -> windows_core::Result<HWND> {
        guarded(|| self.hwnd.lock().unwrap_or_else(|p| p.into_inner()).ok_or(E_FAIL.into()))
    }

    fn TranslateAccelerator(&self, _pmsg: *const MSG) -> windows_core::Result<()> {
        Err(S_FALSE.into())
    }
}

fn rect_size(rect: &RECT) -> windows_core::Result<(u32, u32)> {
    let w = i64::from(rect.right) - i64::from(rect.left);
    let h = i64::from(rect.bottom) - i64::from(rect.top);
    if w <= 0 || h <= 0 {
        return Err(E_INVALIDARG.into());
    }
    let (Ok(w), Ok(h)) = (u32::try_from(w), u32::try_from(h)) else {
        return Err(E_OUTOFMEMORY.into());
    };
    Ok((w, h))
}

/// Reads the whole stream (up to [`MAX_FILE_BYTES`]).
fn read_all(stream: &IStream) -> windows_core::Result<Vec<u8>> {
    let mut stat = STATSTG::default();
    // SAFETY: `stat` is a valid, writable STATSTG; with STATFLAG_NONAME no name is allocated.
    unsafe { stream.Stat(&mut stat, STATFLAG_NONAME)? };
    if stat.cbSize > MAX_FILE_BYTES {
        return Err(E_FAIL.into());
    }
    let size = usize::try_from(stat.cbSize).map_err(|_| windows_core::Error::from(E_OUTOFMEMORY))?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size).map_err(|_| windows_core::Error::from(E_OUTOFMEMORY))?;
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        let mut read = 0u32;
        // SAFETY: `chunk` is a writable buffer of exactly the length passed, and `read` a valid u32.
        let hr = unsafe { stream.Read(chunk.as_mut_ptr().cast(), chunk.len() as u32, Some(&mut read)) };
        hr.ok()?;
        let got = chunk.get(..read as usize).ok_or(windows_core::Error::from(E_FAIL))?;
        if bytes.len().saturating_add(got.len()) as u64 > MAX_FILE_BYTES {
            return Err(E_FAIL.into());
        }
        bytes.try_reserve(got.len()).map_err(|_| windows_core::Error::from(E_OUTOFMEMORY))?;
        bytes.extend_from_slice(got);
        if read == 0 || hr == S_FALSE {
            return Ok(bytes);
        }
    }
}

/// Paints the thumbnail letterboxed on white inside `dest`.
fn paint(hwnd: HWND, dest: &RECT, thumb: &PreviewBitmap) -> windows_core::Result<()> {
    let (dest_w, dest_h) = rect_size(dest)?;
    let (Ok(tw), Ok(th)) = (i32::try_from(thumb.width), i32::try_from(thumb.height)) else {
        return Err(E_OUTOFMEMORY.into());
    };
    if tw <= 0 || th <= 0 {
        return Err(E_FAIL.into());
    }
    let scale = (dest_w as f32 / tw as f32).min(dest_h as f32 / th as f32);
    let draw_w = (tw as f32 * scale).round().clamp(1.0, dest_w as f32) as i32;
    let draw_h = (th as f32 * scale).round().clamp(1.0, dest_h as f32) as i32;
    let x = dest.left + (i32::try_from(dest_w).unwrap_or(0) - draw_w) / 2;
    let y = dest.top + (i32::try_from(dest_h).unwrap_or(0) - draw_h) / 2;

    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: tw,
            biHeight: -th,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let len = thumb.bgra.len();
    // SAFETY: Explorer gave us a valid window handle in SetWindow.
    unsafe {
        let hdc = GetDC(Some(hwnd));
        if hdc.is_invalid() {
            return Err(E_FAIL.into());
        }
        let brush = CreateSolidBrush(COLORREF(0x00FFFFFF));
        if !brush.is_invalid() {
            let _ = FillRect(hdc, dest, brush);
            let _ = DeleteObject(brush.into());
        }
        let lines = StretchDIBits(hdc, x, y, draw_w, draw_h, 0, 0, tw, th, Some(thumb.bgra.as_ptr().cast()), &info, DIB_RGB_COLORS, SRCCOPY);
        let _ = ReleaseDC(Some(hwnd), hdc);
        if lines == 0 || len != (tw as usize).saturating_mul(th as usize).saturating_mul(4) {
            return Err(E_FAIL.into());
        }
        Ok(())
    }
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, riid: *const GUID, ppv: *mut *mut c_void) -> windows_core::Result<()> {
        if ppv.is_null() || riid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: `ppv` is non-null and points to the caller's out-parameter.
        unsafe { *ppv = std::ptr::null_mut() };
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        guarded(|| {
            let handler: IUnknown = Handler::default().into();
            // SAFETY: `riid` and `ppv` are non-null and valid for the call (checked above).
            unsafe { handler.query(riid, ppv) }.ok()
        })
    }

    fn LockServer(&self, _lock: BOOL) -> windows_core::Result<()> {
        Ok(())
    }
}

/// COM entry point: the class factory for [`CLSID`].
///
/// # Safety
/// Called by COM with valid pointers (each is still checked for null).
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(rclsid: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
    if rclsid.is_null() || riid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    match guarded(|| {
        // SAFETY: all three pointers are non-null and, per the COM contract, valid for this call.
        unsafe {
            *ppv = std::ptr::null_mut();
            if *rclsid != CLSID {
                return Err(CLASS_E_CLASSNOTAVAILABLE.into());
            }
            let factory: IClassFactory = Factory.into();
            factory.query(riid, ppv).ok()
        }
    }) {
        Ok(()) => HRESULT(0),
        Err(error) => error.code(),
    }
}

/// COM entry point. The DLL stays loaded for the life of the preview host process.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::Shell::SHCreateMemStream;
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WS_CHILD};

    fn pdf(w: u32, h: u32, content: &str) -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents 4 0 R >>"),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len() + 1),
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        out
    }

    #[test]
    fn explorer_gets_the_first_page_in_the_preview_pane() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok();
            let mut factory: *mut c_void = std::ptr::null_mut();
            DllGetClassObject(&CLSID, &IClassFactory::IID, &mut factory).ok().unwrap();
            let factory = IClassFactory::from_raw(factory);
            let init: IInitializeWithStream = factory.CreateInstance(None).unwrap();
            let bytes = pdf(200, 100, "1 0 0 rg 0 0 100 100 re f");
            let stream = SHCreateMemStream(Some(&bytes)).unwrap();
            init.Initialize(&stream, 0).unwrap();
            let preview: IPreviewHandler = init.cast().unwrap();
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                windows::core::w!("STATIC"),
                windows::core::w!(""),
                WINDOW_STYLE(WS_CHILD.0),
                0,
                0,
                320,
                240,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let rect = RECT { left: 0, top: 0, right: 320, bottom: 240 };
            preview.SetWindow(hwnd, &rect).unwrap();
            preview.DoPreview().unwrap();
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn unreadable_files_fail_without_panicking() {
        unsafe {
            let init: IInitializeWithStream = Handler::default().into();
            let preview: IPreviewHandler = init.cast().unwrap();
            let stream = SHCreateMemStream(Some(b"not a PDF")).unwrap();
            init.Initialize(&stream, 0).unwrap();
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                windows::core::w!("STATIC"),
                windows::core::w!(""),
                WINDOW_STYLE(WS_CHILD.0),
                0,
                0,
                64,
                64,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let rect = RECT { left: 0, top: 0, right: 64, bottom: 64 };
            preview.SetWindow(hwnd, &rect).unwrap();
            assert!(preview.DoPreview().is_err());
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    fn other_class_ids_are_refused() {
        unsafe {
            let mut out: *mut c_void = std::ptr::null_mut();
            let other = GUID::from_u128(1);
            assert_eq!(DllGetClassObject(&other, &IClassFactory::IID, &mut out), CLASS_E_CLASSNOTAVAILABLE);
            assert!(out.is_null());
        }
        assert_eq!(DllCanUnloadNow(), S_FALSE);
    }

    #[test]
    fn com_guard_turns_an_escaped_panic_into_a_failure() {
        assert!(guarded(|| -> windows_core::Result<()> { panic!("synthetic COM failure") }).is_err());
    }
}
