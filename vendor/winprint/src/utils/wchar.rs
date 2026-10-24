use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};

extern "C" {
    fn wcslen(string: *const u16) -> usize;
}

pub fn from_wide_ptr(ptr: *const u16) -> OsString {
    unsafe {
        if ptr.is_null() {
            OsString::default()
        } else {
            let len = wcslen(ptr);
            let slice = std::slice::from_raw_parts(ptr, len);
            OsString::from_wide(slice)
        }
    }
}

pub fn to_wide_chars<P: AsRef<OsStr>>(s: P) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}
