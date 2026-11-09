use std::mem::ManuallyDrop;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use windows::core::w;
use windows::core::BSTR;
use windows::core::HRESULT;
use windows::Win32::Foundation::{
    DISP_E_BADPARAMCOUNT, DISP_E_EXCEPTION, DISP_E_MEMBERNOTFOUND, DISP_E_TYPEMISMATCH,
    DISP_E_UNKNOWNNAME,
};
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_UNEXPECTED};
use windows::Win32::Storage::Xps::Printing::PrintDocumentPackageCompletion_Canceled;
use windows::Win32::Storage::Xps::Printing::PrintDocumentPackageCompletion_Completed;
use windows::Win32::Storage::Xps::Printing::PrintDocumentPackageCompletion_Failed;
use windows::Win32::System::Com::DISPATCH_METHOD;
use windows::Win32::System::Ole::DISPID_UNKNOWN;
use windows::Win32::System::Variant::{VARIANT, VT_BYREF};
use windows::Win32::{
    Foundation::E_NOTIMPL,
    Storage::Xps::Printing::{
        IPrintDocumentPackageStatusEvent, IPrintDocumentPackageStatusEvent_Impl,
        PrintDocumentPackageStatus,
    },
    System::Com::{IDispatch, IDispatch_Impl, ITypeInfo, DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO},
};
use windows_implement::implement;

// The waiter owns the shared state independently of the COM event sink. A late callback
// cannot use a closed HANDLE, and completion cannot be mistaken for success.
type CompletionState = Arc<(Mutex<Option<Result<(), HRESULT>>>, Condvar)>;
const CANCELED: HRESULT = HRESULT(0x800704C7_u32 as i32);
const TIMED_OUT: HRESULT = HRESULT(0x800705B4_u32 as i32);

#[implement(IPrintDocumentPackageStatusEvent, IDispatch)]
pub struct PrintCompletionSource {
    state: CompletionState,
}

impl PrintCompletionSource {
    pub fn new() -> windows::core::Result<Self> {
        Ok(Self {
            state: Arc::new((Mutex::new(None), Condvar::new())),
        })
    }
    pub fn waiter(&self) -> PrintCompletionWaiter {
        PrintCompletionWaiter {
            state: self.state.clone(),
        }
    }
    fn record(&self, status: &PrintDocumentPackageStatus) -> windows::core::Result<()> {
        let result = match status.Completion {
            #[allow(non_upper_case_globals)]
            PrintDocumentPackageCompletion_Completed => status.PackageStatus.ok(),
            #[allow(non_upper_case_globals)]
            PrintDocumentPackageCompletion_Canceled => Err(windows::core::Error::from_hresult(
                if status.PackageStatus.is_err() {
                    status.PackageStatus
                } else {
                    CANCELED
                },
            )),
            #[allow(non_upper_case_globals)]
            PrintDocumentPackageCompletion_Failed => Err(windows::core::Error::from_hresult(
                if status.PackageStatus.is_err() {
                    status.PackageStatus
                } else {
                    E_FAIL
                },
            )),
            _ => return Ok(()),
        }
        .map_err(|e| e.code());
        let (state, ready) = &*self.state;
        let mut terminal = state
            .lock()
            .map_err(|_| windows::core::Error::from_hresult(E_UNEXPECTED))?;
        // Terminal state is immutable even if a driver sends repeated notifications.
        if terminal.is_none() {
            *terminal = Some(result);
            ready.notify_all();
        }
        Ok(())
    }
}

pub struct PrintCompletionWaiter {
    state: CompletionState,
}

impl PrintCompletionWaiter {
    pub fn wait(&self) -> windows::core::Result<()> {
        self.wait_for(Duration::from_secs(120))
    }
    fn wait_for(&self, timeout: Duration) -> windows::core::Result<()> {
        let (state, ready) = &*self.state;
        let terminal = state
            .lock()
            .map_err(|_| windows::core::Error::from_hresult(E_UNEXPECTED))?;
        let (terminal, _) = ready
            .wait_timeout_while(terminal, timeout, |s| s.is_none())
            .map_err(|_| windows::core::Error::from_hresult(E_UNEXPECTED))?;
        match *terminal {
            Some(Ok(())) => Ok(()),
            Some(Err(code)) => Err(windows::core::Error::from_hresult(code)),
            None => Err(windows::core::Error::from_hresult(TIMED_OUT)),
        }
    }
}

impl IPrintDocumentPackageStatusEvent_Impl for PrintCompletionSource_Impl {
    fn PackageStatusUpdated(
        &self,
        packagestatus: *const PrintDocumentPackageStatus,
    ) -> windows::core::Result<()> {
        // COM guarantees this pointer remains valid for the duration of the callback.
        let status = unsafe { packagestatus.as_ref() }
            .ok_or_else(|| windows::core::Error::from_hresult(E_INVALIDARG))?;
        self.record(status)
    }
}

impl IDispatch_Impl for PrintCompletionSource_Impl {
    fn GetTypeInfoCount(&self) -> windows::core::Result<u32> {
        Ok(0)
    }
    fn GetTypeInfo(&self, _itinfo: u32, _lcid: u32) -> windows::core::Result<ITypeInfo> {
        Err(windows::core::Error::from_hresult(E_NOTIMPL))
    }
    fn GetIDsOfNames(
        &self,
        _riid: *const windows::core::GUID,
        rgsznames: *const windows::core::PCWSTR,
        cnames: u32,
        _lcid: u32,
        rgdispid: *mut i32,
    ) -> windows::core::Result<()> {
        let names = unsafe { std::slice::from_raw_parts(rgsznames, cnames as usize) };
        let dispid = unsafe { std::slice::from_raw_parts_mut(rgdispid, cnames as usize) };
        let mut unknown_found = false;
        for (name, dispid) in names.iter().zip(dispid.iter_mut()) {
            if *name == w!("PackageStatusUpdated") {
                *dispid = 1;
            } else {
                *dispid = DISPID_UNKNOWN;
                unknown_found = true;
            }
        }
        if unknown_found {
            Err(windows::core::Error::from_hresult(DISP_E_UNKNOWNNAME))
        } else {
            Ok(())
        }
    }
    fn Invoke(
        &self,
        dispidmember: i32,
        _riid: *const windows::core::GUID,
        _lcid: u32,
        wflags: DISPATCH_FLAGS,
        pdispparams: *const DISPPARAMS,
        pvarresult: *mut VARIANT,
        pexcepinfo: *mut EXCEPINFO,
        puargerr: *mut u32,
    ) -> windows::core::Result<()> {
        if dispidmember == 1 && wflags == DISPATCH_METHOD {
            if !pvarresult.is_null() {
                unsafe {
                    (*pvarresult) = Default::default();
                }
            }
            let params = unsafe { &*pdispparams };
            if params.cArgs != 1 {
                return Err(windows::core::Error::from_hresult(DISP_E_BADPARAMCOUNT));
            }
            let arg = unsafe { &*params.rgvarg };
            let vt = unsafe { arg.Anonymous.Anonymous.vt };
            if vt.0 & VT_BYREF.0 == 0 {
                if !puargerr.is_null() {
                    unsafe {
                        *puargerr = 0;
                    }
                }
                return Err(windows::core::Error::from_hresult(DISP_E_TYPEMISMATCH));
            }
            let status = unsafe { arg.Anonymous.Anonymous.Anonymous.byref }
                as *const PrintDocumentPackageStatus;
            if let Err(e) = self.PackageStatusUpdated(status) {
                if !pexcepinfo.is_null() {
                    unsafe {
                        (*pexcepinfo).wCode = 0;
                        (*pexcepinfo).bstrSource = ManuallyDrop::new(BSTR::default());
                        (*pexcepinfo).bstrDescription = ManuallyDrop::new(BSTR::default());
                        (*pexcepinfo).bstrHelpFile = ManuallyDrop::new(BSTR::default());
                        (*pexcepinfo).dwHelpContext = 0;
                        (*pexcepinfo).scode = e.code().0;
                    }
                }
                Err(windows::core::Error::from_hresult(DISP_E_EXCEPTION))
            } else {
                Ok(())
            }
        } else {
            Err(windows::core::Error::from_hresult(DISP_E_MEMBERNOTFOUND))
        }
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    use windows::Win32::Storage::Xps::Printing::PrintDocumentPackageCompletion_InProgress;

    fn status(
        completion: windows::Win32::Storage::Xps::Printing::PrintDocumentPackageCompletion,
        code: HRESULT,
    ) -> PrintDocumentPackageStatus {
        PrintDocumentPackageStatus {
            Completion: completion,
            PackageStatus: code,
            ..Default::default()
        }
    }

    #[test]
    fn late_failure_hresult_is_preserved_after_pages_finish() {
        let source = PrintCompletionSource::new().unwrap();
        let waiter = source.waiter();
        let code = HRESULT(0x80070005_u32 as i32);
        source
            .record(&status(
                PrintDocumentPackageCompletion_InProgress,
                HRESULT(0),
            ))
            .unwrap();
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(10));
                source
                    .record(&status(PrintDocumentPackageCompletion_Failed, code))
                    .unwrap();
            });
            assert_eq!(waiter.wait().unwrap_err().code(), code);
        });
    }

    #[test]
    fn canceled_and_failed_without_hresult_are_errors() {
        for (completion, expected) in [
            (PrintDocumentPackageCompletion_Canceled, CANCELED),
            (PrintDocumentPackageCompletion_Failed, E_FAIL),
        ] {
            let source = PrintCompletionSource::new().unwrap();
            source.record(&status(completion, HRESULT(0))).unwrap();
            source
                .record(&status(
                    PrintDocumentPackageCompletion_Completed,
                    HRESULT(0),
                ))
                .unwrap();
            assert_eq!(source.waiter().wait().unwrap_err().code(), expected);
        }
    }

    #[test]
    fn missing_terminal_notification_times_out_without_hanging() {
        let source = PrintCompletionSource::new().unwrap();
        let waiter = source.waiter();
        drop(source);
        assert_eq!(
            waiter
                .wait_for(Duration::from_millis(1))
                .unwrap_err()
                .code(),
            TIMED_OUT
        );
    }

    #[test]
    fn completed_checks_hresult_instead_of_blind_success() {
        for code in [HRESULT(0), E_FAIL] {
            let source = PrintCompletionSource::new().unwrap();
            source
                .record(&status(PrintDocumentPackageCompletion_Completed, code))
                .unwrap();
            assert_eq!(
                source.waiter().wait().map_err(|e| e.code()),
                code.ok().map_err(|e| e.code())
            );
        }
    }
}
