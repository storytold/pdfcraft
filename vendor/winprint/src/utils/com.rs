use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

#[non_exhaustive]
pub struct ComInitializer {
    // Do not use empty struct.
    // Empty struct is easy to misuse.
    _dummy: (),
}
impl ComInitializer {
    pub fn new() -> ComInitializer {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        ComInitializer { _dummy: () }
    }
}
impl Drop for ComInitializer {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
