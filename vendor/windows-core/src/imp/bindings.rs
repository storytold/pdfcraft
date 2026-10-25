windows_link::link!("ole32.dll" "system" fn CoTaskMemAlloc(cb : usize) -> *mut core::ffi::c_void);
windows_link::link!("ole32.dll" "system" fn CoTaskMemFree(pv : *const core::ffi::c_void));
windows_link::link!("kernel32.dll" "system" fn EncodePointer(ptr : *const core::ffi::c_void) -> *mut core::ffi::c_void);
windows_link::link!("kernel32.dll" "system" fn FreeLibrary(hlibmodule : HMODULE) -> BOOL);
windows_link::link!("kernel32.dll" "system" fn GetProcAddress(hmodule : HMODULE, lpprocname : PCSTR) -> FARPROC);
windows_link::link!("kernel32.dll" "system" fn LoadLibraryExA(lplibfilename : PCSTR, hfile : HANDLE, dwflags : LOAD_LIBRARY_FLAGS) -> HMODULE);
windows_link::link!("kernel32.dll" "system" fn GetSystemDirectoryA(lpbuffer : *mut u8, usize : u32) -> u32);
windows_link::link!("rpcrt4.dll" "system" fn UuidCreate(uuid : *mut GUID) -> RPC_STATUS);
pub type BOOL = i32;
pub type CO_MTA_USAGE_COOKIE = *mut core::ffi::c_void;
pub type FARPROC = Option<unsafe extern "system" fn() -> isize>;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GUID {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}
impl GUID {
    pub const fn from_u128(uuid: u128) -> Self {
        Self {
            data1: (uuid >> 96) as u32,
            data2: (uuid >> 80 & 0xffff) as u16,
            data3: (uuid >> 64 & 0xffff) as u16,
            data4: (uuid as u64).to_be_bytes(),
        }
    }
}
pub type HANDLE = *mut core::ffi::c_void;
pub type HINSTANCE = *mut core::ffi::c_void;
pub type HMODULE = *mut core::ffi::c_void;
pub type HRESULT = i32;
pub type LOAD_LIBRARY_FLAGS = u32;
pub const LOAD_LIBRARY_SEARCH_DEFAULT_DIRS: LOAD_LIBRARY_FLAGS = 4096u32;
pub type PCSTR = *const u8;
pub type RPC_STATUS = i32;
pub const RPC_S_UUID_LOCAL_ONLY: RPC_STATUS = 1824i32;
pub type HSTRING = *mut core::ffi::c_void;

// PdfCraft patch (Windows 7): combase.dll and api-ms-win-core-winrt-l1-1-0.dll exist only on
// Windows 8 and later, and a static import of either stops the program loading on Windows 7.
// CoIncrementMTAUsage and RoGetActivationFactory are therefore looked up in System32's
// combase.dll at first use (by full path, so no other directory is searched), and return
// E_NOTIMPL where it is missing. CoTaskMem* above come from ole32.dll, which has them on every
// version. See vendor/README.md.
pub const E_NOTIMPL: HRESULT = 0x80004001_u32 as i32;

/// Looks `function` (nul-terminated) up in System32's combase.dll once; 0 means unavailable.
fn combase_proc(cache: &core::sync::atomic::AtomicUsize, function: &[u8]) -> usize {
    use core::sync::atomic::Ordering;
    // 0: not looked up yet, 1: unavailable, anything else: the address.
    let cached = cache.load(Ordering::Acquire);
    if cached != 0 {
        return if cached == 1 { 0 } else { cached };
    }
    let library = b"\\combase.dll\0";
    let mut path = [0u8; 300];
    let address = unsafe {
        let len = GetSystemDirectoryA(path.as_mut_ptr(), 260) as usize;
        match path.get_mut(len..len + library.len()) {
            Some(tail) if len != 0 && len < 260 => {
                tail.copy_from_slice(library);
                let module = LoadLibraryExA(path.as_ptr(), core::ptr::null_mut(), 0);
                if module.is_null() {
                    0
                } else {
                    GetProcAddress(module, function.as_ptr()).map_or(0, |f| f as usize)
                }
            }
            _ => 0,
        }
    };
    cache.store(if address == 0 { 1 } else { address }, Ordering::Release);
    address
}

pub unsafe fn CoIncrementMTAUsage(pcookie: *mut CO_MTA_USAGE_COOKIE) -> HRESULT {
    static CACHE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    match combase_proc(&CACHE, b"CoIncrementMTAUsage\0") {
        0 => E_NOTIMPL,
        address => unsafe {
            let f: unsafe extern "system" fn(*mut CO_MTA_USAGE_COOKIE) -> HRESULT =
                core::mem::transmute(address);
            f(pcookie)
        },
    }
}

pub unsafe fn RoGetActivationFactory(
    activatableclassid: HSTRING,
    iid: *const GUID,
    factory: *mut *mut core::ffi::c_void,
) -> HRESULT {
    static CACHE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    match combase_proc(&CACHE, b"RoGetActivationFactory\0") {
        0 => E_NOTIMPL,
        address => unsafe {
            let f: unsafe extern "system" fn(HSTRING, *const GUID, *mut *mut core::ffi::c_void) -> HRESULT =
                core::mem::transmute(address);
            f(activatableclassid, iid, factory)
        },
    }
}
