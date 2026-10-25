use crate::utils::wchar;
use std::alloc::{alloc, dealloc, Layout};
use std::ffi::{OsStr, OsString};
use std::mem;
use std::ptr;
use thiserror::Error;
use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows::Win32::Graphics::Printing::*;
#[derive(Clone, Debug)]
/// Represents a printer device.
pub struct PrinterDevice {
    name: String,
    os_server: OsString,
    os_name: OsString,
    os_attributes: u32,
}

#[derive(Error, Debug)]
/// Represents an error occurred while enumerating printer devices.
pub enum EnumDeviceError {
    /// Failed to enum printer device.
    #[error("Failed to enum printer device")]
    FailedToEnumPrinterDevice(#[source] windows::core::Error),
}

impl PrinterDevice {
    /// Fetch all printer devices.
    pub fn all() -> Result<Vec<Self>, EnumDeviceError> {
        let mut bytes_needed: u32 = 0;
        let mut count_returned: u32 = 0;
        let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
        let name = PCWSTR::null();
        let level = 4;
        unsafe {
            let mut buffer: *mut u8 = ptr::null_mut();
            let mut buffer_size: usize = 0;

            loop {
                let slice = if buffer_size > 0 {
                    Some(std::slice::from_raw_parts_mut(buffer, buffer_size))
                } else {
                    None
                };
                let result = EnumPrintersW(
                    flags,
                    name,
                    level,
                    slice,
                    &mut bytes_needed,
                    &mut count_returned,
                );
                match result {
                    Ok(()) => break,
                    Err(e) if e.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() => {
                        let new_size = bytes_needed as usize;
                        let align = mem::align_of::<PRINTER_INFO_4W>();
                        if buffer_size > 0 {
                            dealloc(
                                buffer,
                                Layout::from_size_align_unchecked(buffer_size, align),
                            );
                        }
                        buffer = alloc(Layout::from_size_align_unchecked(new_size, align));
                        buffer_size = new_size;
                    }
                    Err(e) => {
                        if buffer_size > 0 {
                            dealloc(
                                buffer,
                                Layout::from_size_align_unchecked(
                                    buffer_size,
                                    mem::align_of::<PRINTER_INFO_4W>(),
                                ),
                            );
                        }
                        return Err(EnumDeviceError::FailedToEnumPrinterDevice(e));
                    }
                }
            }

            let mut result = Vec::<PrinterDevice>::with_capacity(count_returned as usize);
            for i in 0..count_returned {
                let info = &*(buffer as *const PRINTER_INFO_4W).offset(i as isize);
                let os_name = wchar::from_wide_ptr(info.pPrinterName.0);
                result.push(Self {
                    name: os_name.to_string_lossy().into_owned(),
                    os_server: wchar::from_wide_ptr(info.pServerName.0),
                    os_attributes: info.Attributes,
                    os_name,
                });
            }

            if buffer_size > 0 {
                dealloc(
                    buffer,
                    Layout::from_size_align_unchecked(
                        buffer_size,
                        mem::align_of::<PRINTER_INFO_4W>(),
                    ),
                );
            }

            Ok(result)
        }
    }

    /// Get a reference to the printer device's name.
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// Get the printer device's os attributes.
    pub fn os_attributes(&self) -> u32 {
        self.os_attributes
    }

    /// Get a reference to the printer device's os server.
    pub fn os_server(&self) -> &OsStr {
        self.os_server.as_ref()
    }

    /// Get a reference to the printer device's os name.
    pub fn os_name(&self) -> &OsStr {
        self.os_name.as_ref()
    }

    /// Check if the printer device is local.
    pub fn is_local(&self) -> bool {
        self.os_attributes & PRINTER_ATTRIBUTE_LOCAL != 0
    }

    /// Check if the printer device is remote.
    pub fn is_remote(&self) -> bool {
        self.os_attributes & PRINTER_ATTRIBUTE_NETWORK != 0
    }
}

#[cfg(test)]
mod tests {
    use super::PrinterDevice;
    use crate::test_utils::null_device;

    #[test]
    fn fetch_printer_device() {
        let _devices = PrinterDevice::all().unwrap();
    }

    #[test]
    fn test_printer_should_be_local() {
        let device = null_device::thread_local();
        assert!(device.is_local());
        assert_eq!(device.is_remote(), false);
        assert_eq!(device.os_server(), "");
        // PRINTER_ATTRIBUTE_LOCAL == 0x00000040
        assert!(device.os_attributes() & 0x00000040 != 0);
    }
}
