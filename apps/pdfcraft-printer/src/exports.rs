//! Only the two ABI export attributes need the owner's unsafe-code exception.
#![allow(unsafe_code)]

use windows::ApplicationModel::Background::{IBackgroundTask, IBackgroundTask_Impl, IBackgroundTaskInstance};
use windows::Win32::System::WinRT::{IActivationFactory, IActivationFactory_Impl};
use windows_core::{Error, HRESULT, HSTRING, IInspectable, Interface, OutRef, Ref, Result, implement};

const CLASS: &str = "PdfCraft.Printer.BackgroundTask";
const SETTINGS_CLASS: &str = "PdfCraft.Printer.SettingsTask";

fn factory(name: &HSTRING) -> Result<IActivationFactory> {
    let settings = if name == CLASS {
        false
    } else if name == SETTINGS_CLASS {
        true
    } else {
        return Err(Error::from_hresult(HRESULT(0x80040111u32 as i32)));
    };
    Ok(ActivationFactory { settings }.into())
}

// windows-rs generates the COM vtables; no handwritten pointer dereferences are used here.
#[implement(IActivationFactory)]
struct ActivationFactory {
    settings: bool,
}
impl IActivationFactory_Impl for ActivationFactory_Impl {
    fn ActivateInstance(&self) -> Result<IInspectable> {
        crate::workflow::guard(|| {
            let task: IBackgroundTask = BackgroundTask { settings: self.settings }.into();
            task.cast()
        })
    }
}

#[implement(IBackgroundTask)]
struct BackgroundTask {
    settings: bool,
}
impl IBackgroundTask_Impl for BackgroundTask_Impl {
    fn Run(&self, instance: Ref<'_, IBackgroundTaskInstance>) -> Result<()> {
        crate::workflow::guard(|| if self.settings { crate::workflow::settings(instance.ok()?) } else { crate::workflow::run(instance.ok()?) })
    }
}

// SAFETY: Windows calls this documented WinRT ABI; Ref/OutRef delegate pointer handling to windows-rs.
#[unsafe(no_mangle)]
pub extern "system" fn DllGetActivationFactory(name: Ref<'_, HSTRING>, result: OutRef<'_, IActivationFactory>) -> HRESULT {
    match crate::workflow::guard(|| factory(name.ok()?)) {
        Ok(factory) => result.write(Some(factory)).into(),
        Err(error) => match result.write(None) {
            Ok(()) => error.into(),
            Err(pointer_error) => pointer_error.into(),
        },
    }
}

// Keep code resident while Windows can hold background-task callbacks; the host owns process lifetime.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    HRESULT(1)
}
