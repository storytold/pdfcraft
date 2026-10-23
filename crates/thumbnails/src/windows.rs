//! The File Explorer thumbnail handler: an in-process COM server exporting `DllGetClassObject`
//! and `DllCanUnloadNow`, whose one class implements `IInitializeWithStream` (Explorer hands it
//! the file as a stream) and `IThumbnailProvider` (it returns the first page as a bitmap).
//!
//! Explorer runs thumbnail handlers in a separate, isolated process, so even a failure here never
//! takes Explorer down; it still never panics across the COM boundary (every call that renders
//! is behind [`render_thumbnail`]'s `catch_unwind`). Failures return `WTS_E_FAILEDEXTRACTION`
//! and Explorer shows the file's icon instead.
//!
//! `unsafe` is confined to this module: the exported functions dereference the pointers COM
//! passes in, and the bitmap's pixels are written through the pointer `CreateDIBSection` returns.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, ERROR_ALREADY_INITIALIZED, S_FALSE,
};
use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, HBITMAP};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl, IStream, STATFLAG_NONAME, STATSTG};
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithStream, IInitializeWithStream_Impl};
use windows::Win32::UI::Shell::{IThumbnailProvider, IThumbnailProvider_Impl, WTS_ALPHATYPE, WTS_E_FAILEDEXTRACTION, WTSAT_RGB, WTSAT_UNKNOWN};
use windows_core::{BOOL, GUID, HRESULT, IUnknown, Interface, Ref, implement};

use crate::{Thumbnail, render_thumbnail};

/// The thumbnail handler's class id, registered under `PdfCraft.Document\ShellEx`.
pub const CLSID: GUID = GUID::from_u128(0x68e114f4_eb01_42a6_bd92_1695f3711cd1);

/// Files larger than this aren't read for a thumbnail (the whole file is parsed in memory).
pub const MAX_FILE_BYTES: u64 = 64 << 20;

/// The document's bytes, read when Explorer initializes the handler.
#[implement(IInitializeWithStream, IThumbnailProvider)]
#[derive(Default)]
struct Provider {
    bytes: Mutex<Option<Arc<Vec<u8>>>>,
}

fn guarded(f: impl FnOnce() -> windows_core::Result<()>) -> windows_core::Result<()> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(WTS_E_FAILEDEXTRACTION.into()))
}

impl IInitializeWithStream_Impl for Provider_Impl {
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

impl IThumbnailProvider_Impl for Provider_Impl {
    fn GetThumbnail(&self, cx: u32, phbmp: *mut HBITMAP, pdwalpha: *mut WTS_ALPHATYPE) -> windows_core::Result<()> {
        if phbmp.is_null() || pdwalpha.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: COM supplies valid writable out-parameters; both were checked for null.
        unsafe {
            *phbmp = HBITMAP::default();
            *pdwalpha = WTSAT_UNKNOWN;
        }
        guarded(|| {
            let bytes = self.bytes.lock().unwrap_or_else(|p| p.into_inner()).clone().ok_or(windows_core::Error::from(E_INVALIDARG))?;
            let thumbnail = render_thumbnail(bytes, cx).map_err(|_| windows_core::Error::from(WTS_E_FAILEDEXTRACTION))?;
            let bitmap = bitmap(&thumbnail)?;
            // SAFETY: the same checked COM out-parameters remain valid for this call.
            unsafe {
                *phbmp = bitmap;
                *pdwalpha = WTSAT_RGB;
            }
            Ok(())
        })
    }
}

/// Reads the whole stream (up to [`MAX_FILE_BYTES`]).
fn read_all(stream: &IStream) -> windows_core::Result<Vec<u8>> {
    let mut stat = STATSTG::default();
    // SAFETY: `stat` is a valid, writable STATSTG; with STATFLAG_NONAME no name is allocated.
    unsafe { stream.Stat(&mut stat, STATFLAG_NONAME)? };
    if stat.cbSize > MAX_FILE_BYTES {
        return Err(WTS_E_FAILEDEXTRACTION.into());
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
        let got = chunk.get(..read as usize).ok_or(windows_core::Error::from(WTS_E_FAILEDEXTRACTION))?;
        if bytes.len().saturating_add(got.len()) as u64 > MAX_FILE_BYTES {
            return Err(WTS_E_FAILEDEXTRACTION.into());
        }
        bytes.try_reserve(got.len()).map_err(|_| windows_core::Error::from(E_OUTOFMEMORY))?;
        bytes.extend_from_slice(got);
        if read == 0 || hr == S_FALSE {
            return Ok(bytes);
        }
    }
}

/// A top-down 32-bit DIB section holding the thumbnail's pixels; Explorer owns and frees it.
fn bitmap(t: &Thumbnail) -> windows_core::Result<HBITMAP> {
    let (Ok(width), Ok(height)) = (i32::try_from(t.width), i32::try_from(t.height)) else {
        return Err(E_OUTOFMEMORY.into());
    };
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Negative: rows run top to bottom, as the thumbnail's do.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    // SAFETY: `info` describes a valid 32-bit top-down bitmap and `bits` receives its pixel pointer.
    let bitmap = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)? };
    let len = (t.width as usize).saturating_mul(t.height as usize).saturating_mul(4);
    if bits.is_null() || t.bgra.len() != len {
        // SAFETY: `bitmap` was just created here and is not shared.
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::DeleteObject(bitmap.into());
        }
        return Err(E_OUTOFMEMORY.into());
    }
    // SAFETY: the DIB section holds width × height 32-bit pixels, top-down with no row padding
    // (32-bit rows are always DWORD-aligned), which is exactly `len` bytes; the source has `len`.
    unsafe { std::ptr::copy_nonoverlapping(t.bgra.as_ptr(), bits.cast::<u8>(), len) };
    Ok(bitmap)
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
            let provider: IUnknown = Provider::default().into();
            // SAFETY: `riid` and `ppv` are non-null and valid for the call (checked above).
            unsafe { provider.query(riid, ppv) }.ok()
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

/// COM entry point. The DLL stays loaded for the life of the (isolated) thumbnail process, which
/// spares counting live objects.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::{BITMAP, DeleteObject, GetObjectW};
    use windows::Win32::UI::Shell::SHCreateMemStream;

    /// Goes through COM exactly as File Explorer does: the exported class factory, the stream
    /// initializer and `GetThumbnail`.
    fn thumbnail_through_com(bytes: &[u8], cx: u32) -> windows_core::Result<(i32, i32)> {
        unsafe {
            let mut factory: *mut c_void = std::ptr::null_mut();
            DllGetClassObject(&CLSID, &IClassFactory::IID, &mut factory).ok()?;
            let factory = IClassFactory::from_raw(factory);
            let init: IInitializeWithStream = factory.CreateInstance(None)?;
            let stream = SHCreateMemStream(Some(bytes)).ok_or(windows_core::Error::from(E_OUTOFMEMORY))?;
            init.Initialize(&stream, 0)?;
            let provider: IThumbnailProvider = init.cast()?;
            let (mut hbmp, mut alpha) = (HBITMAP::default(), WTS_ALPHATYPE::default());
            provider.GetThumbnail(cx, &mut hbmp, &mut alpha)?;
            assert_eq!(alpha, WTSAT_RGB);
            let mut info = BITMAP::default();
            let n = GetObjectW(hbmp.into(), size_of::<BITMAP>() as i32, Some((&mut info as *mut BITMAP).cast()));
            assert!(DeleteObject(hbmp.into()).as_bool());
            assert_eq!(n, size_of::<BITMAP>() as i32);
            assert_eq!(info.bmBitsPixel, 32);
            Ok((info.bmWidth, info.bmHeight))
        }
    }

    #[test]
    fn explorer_gets_the_first_page_as_a_bitmap() {
        let pdf = crate::tests::pdf(200, 100, "1 0 0 rg 0 0 100 100 re f");
        assert_eq!(thumbnail_through_com(&pdf, 256).unwrap(), (256, 128));
    }

    #[test]
    fn unreadable_files_fail_with_the_extraction_error_so_explorer_shows_the_icon() {
        let e = thumbnail_through_com(b"not a PDF", 256).unwrap_err();
        assert_eq!(e.code(), WTS_E_FAILEDEXTRACTION);
    }

    #[test]
    fn other_class_ids_are_refused() {
        unsafe {
            let mut out: *mut c_void = std::ptr::null_mut();
            let other = GUID::from_u128(1);
            assert_eq!(DllGetClassObject(&other, &IClassFactory::IID, &mut out), CLASS_E_CLASSNOTAVAILABLE);
            assert!(out.is_null());
            assert_eq!(DllGetClassObject(&CLSID, &IClassFactory::IID, std::ptr::null_mut()), E_POINTER);
        }
        assert_eq!(DllCanUnloadNow(), S_FALSE);
    }
    #[test]
    fn failed_calls_clear_outputs_and_reinitialization_is_refused() {
        unsafe {
            let init: IInitializeWithStream = Provider::default().into();
            let provider: IThumbnailProvider = init.cast().unwrap();
            let mut bitmap = HBITMAP(std::ptr::dangling_mut::<c_void>());
            let mut alpha = WTSAT_RGB;
            assert!(provider.GetThumbnail(256, &mut bitmap, &mut alpha).is_err());
            assert!(bitmap.is_invalid());
            assert_eq!(alpha, WTSAT_UNKNOWN);
            assert_eq!(provider.GetThumbnail(256, std::ptr::null_mut(), &mut alpha).unwrap_err().code(), E_POINTER);
            let bytes = crate::tests::pdf(200, 100, "");
            let stream = SHCreateMemStream(Some(&bytes)).unwrap();
            init.Initialize(&stream, 0).unwrap();
            assert_eq!(init.Initialize(&stream, 0).unwrap_err().code(), ERROR_ALREADY_INITIALIZED.to_hresult());
        }
    }

    #[test]
    fn com_guard_turns_an_escaped_panic_into_an_extraction_error() {
        assert_eq!(guarded(|| panic!("synthetic COM failure")).unwrap_err().code(), WTS_E_FAILEDEXTRACTION);
    }
}
