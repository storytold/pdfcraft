#![forbid(unsafe_code)]
#![cfg_attr(not(feature = "winrt-server"), allow(dead_code))]

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use windows::ApplicationModel::Background::{BackgroundTaskCanceledEventHandler, BackgroundTaskDeferral, IBackgroundTaskInstance};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Printing::PrintSupport::{
    PrintSupportExtensionSession, PrintSupportExtensionTriggerDetails, PrintSupportPrintTicketValidationRequestedEventArgs,
    WorkflowPrintTicketValidationStatus,
};
use windows::Graphics::Printing::Workflow::{
    PrintWorkflowSubmittedStatus, PrintWorkflowVirtualPrinterDataAvailableEventArgs, PrintWorkflowVirtualPrinterSession,
    PrintWorkflowVirtualPrinterTriggerDetails,
};
use windows::Storage::Streams::{Buffer, DataReader, DataWriter, InputStreamOptions};
use windows_core::{AgileReference, Error, Interface, Result};

pub fn guard<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .map_err(|_| Error::new(windows_core::HRESULT(0x80004005u32 as i32), "PdfCraft printer operation failed"))?
}

fn failure(message: impl AsRef<str>) -> Error {
    Error::new(windows_core::HRESULT(0x80004005u32 as i32), message.as_ref())
}

pub fn run(instance: &IBackgroundTaskInstance) -> Result<()> {
    let details = instance.TriggerDetails()?.cast::<PrintWorkflowVirtualPrinterTriggerDetails>()?;
    let session = details.VirtualPrinterSession()?;
    let deferral = instance.GetDeferral()?;
    let result = start(instance, &session, &deferral);
    if result.is_err() {
        let _ = deferral.Complete();
    }
    result
}

fn start(instance: &IBackgroundTaskInstance, session: &PrintWorkflowVirtualPrinterSession, deferral: &BackgroundTaskDeferral) -> Result<()> {
    let canceled = Arc::new(AtomicBool::new(false));
    let flag = canceled.clone();
    let token = Arc::new(Mutex::new(None));
    let cancel_event = token.clone();
    let cancel_session = session.clone();
    let cancel_completion = deferral.clone();
    let cancel_token = instance.Canceled(&BackgroundTaskCanceledEventHandler::new(move |_, _| {
        flag.store(true, Ordering::Release);
        if let Ok(mut token) = cancel_event.lock()
            && let Some(token) = token.take()
        {
            let _ = cancel_session.RemoveVirtualPrinterDataAvailable(token);
        }
        cancel_completion.Complete()
    }))?;
    let event_token = token.clone();
    let task = AgileReference::new(instance)?;
    let completion = deferral.clone();
    let registered = session.VirtualPrinterDataAvailable(&TypedEventHandler::<
        PrintWorkflowVirtualPrinterSession,
        PrintWorkflowVirtualPrinterDataAvailableEventArgs,
    >::new(move |sender, args| {
        let result = guard(|| print_job(args.ok()?, &canceled));
        if let Err(error) = &result {
            record_error(error);
        }
        let status = if canceled.load(Ordering::Acquire) {
            PrintWorkflowSubmittedStatus::Canceled
        } else if result.is_ok() {
            PrintWorkflowSubmittedStatus::Succeeded
        } else {
            PrintWorkflowSubmittedStatus::Failed
        };
        if let Some(args) = args.as_ref() {
            let _ = args.CompleteJob(status);
        }
        if let (Some(sender), Ok(mut token)) = (sender.as_ref(), event_token.lock())
            && let Some(token) = token.take()
        {
            let _ = sender.RemoveVirtualPrinterDataAvailable(token);
        }
        if let Ok(task) = task.resolve() {
            let _ = task.RemoveCanceled(cancel_token);
        }
        let _ = completion.Complete();
        Ok(())
    }));
    let start = registered.and_then(|registered| {
        *token.lock().map_err(|_| failure("Print event state could not be locked"))? = Some(registered);
        session.Start()
    });
    if start.is_err() {
        if let Ok(mut token) = token.lock()
            && let Some(token) = token.take()
        {
            let _ = session.RemoveVirtualPrinterDataAvailable(token);
        }
        let _ = instance.RemoveCanceled(cancel_token);
    }
    start
}

fn print_job(args: &PrintWorkflowVirtualPrinterDataAvailableEventArgs, canceled: &AtomicBool) -> Result<()> {
    let content = args.SourceContent()?;
    if content.ContentType()? != "application/oxps" {
        return Err(failure("PdfCraft requires OpenXPS print content"));
    }
    let input = content.GetInputStream()?;
    let buffer = Buffer::Create(64 * 1024)?;
    let mut bytes = Vec::new();
    loop {
        check_cancel(canceled)?;
        let chunk = input.ReadAsync(&buffer, 64 * 1024, InputStreamOptions::Partial)?.join()?;
        let count = usize::try_from(chunk.Length()?).map_err(|_| failure("Invalid print stream size"))?;
        if count == 0 {
            break;
        }
        let length = bytes.len().checked_add(count).filter(|n| *n <= 256 * 1024 * 1024).ok_or_else(|| failure("Print job exceeds 256 MiB"))?;
        bytes.try_reserve(count).map_err(|_| failure("Not enough memory for the print job"))?;
        let offset = bytes.len();
        bytes.resize(length, 0);
        DataReader::FromBuffer(&chunk)?.ReadBytes(bytes.get_mut(offset..).ok_or_else(|| failure("Invalid print read size"))?)?;
    }
    input.Close()?;
    check_cancel(canceled)?;
    let title = args.Configuration()?.JobTitle()?.to_string_lossy();
    let pdf = crate::convert(&title, &bytes).map_err(failure)?;
    check_cancel(canceled)?;
    let target = args.GetTargetFileAsync()?.join()?;
    write_pdf(&target, &pdf, canceled)
}

// Convert completely before opening the user-approved destination, then commit transactionally.
fn write_pdf(target: &windows::Storage::StorageFile, pdf: &[u8], canceled: &AtomicBool) -> Result<()> {
    let transaction = target.OpenTransactedWriteAsync()?.join()?;
    let result = (|| {
        let stream = transaction.Stream()?;
        stream.SetSize(0)?;
        let writer = DataWriter::CreateDataWriter(&stream.GetOutputStreamAt(0)?)?;
        for chunk in pdf.chunks(64 * 1024) {
            check_cancel(canceled)?;
            writer.WriteBytes(chunk)?;
            if writer.StoreAsync()?.join()? != u32::try_from(chunk.len()).map_err(|_| failure("Invalid PDF write size"))? {
                return Err(failure("Incomplete PDF write"));
            }
        }
        if !writer.FlushAsync()?.join()? {
            return Err(failure("The PDF could not be flushed"));
        }
        let _ = writer.DetachStream()?;
        check_cancel(canceled)?;
        transaction.CommitAsync()?.join()
    })();
    let closed = transaction.Close();
    result.and(closed)
}

fn check_cancel(canceled: &AtomicBool) -> Result<()> {
    if canceled.load(Ordering::Acquire) { Err(failure("Print job canceled")) } else { Ok(()) }
}

fn record_error(error: &Error) {
    let _ = guard(|| {
        let folder = windows::Storage::ApplicationData::Current()?.LocalFolder()?.Path()?;
        std::fs::write(std::path::PathBuf::from(folder.to_string_lossy()).join("last-print-error.txt"), error.to_string())
            .map_err(|_| failure("Could not record print failure"))
    });
}

pub fn settings(instance: &IBackgroundTaskInstance) -> Result<()> {
    let details = instance.TriggerDetails()?.cast::<PrintSupportExtensionTriggerDetails>()?;
    let session = details.Session()?;
    let deferral = instance.GetDeferral()?;
    let result = start_settings(instance, &session, &deferral);
    if result.is_err() {
        let _ = deferral.Complete();
    }
    result
}

fn start_settings(instance: &IBackgroundTaskInstance, session: &PrintSupportExtensionSession, deferral: &BackgroundTaskDeferral) -> Result<()> {
    let token = session.PrintTicketValidationRequested(&TypedEventHandler::<
        PrintSupportExtensionSession,
        PrintSupportPrintTicketValidationRequestedEventArgs,
    >::new(|_, args| {
        guard(|| {
            let args = args.ok()?;
            let deferral = args.GetDeferral()?;
            // Windows validates the standard choices exposed by our PDC; there are no custom settings.
            let result = args.SetPrintTicketValidationStatus(WorkflowPrintTicketValidationStatus::Resolved);
            let completed = deferral.Complete();
            result.and(completed)
        })
    }))?;
    let cleanup_session = session.clone();
    let cleanup = deferral.clone();
    instance.Canceled(&BackgroundTaskCanceledEventHandler::new(move |_, _| {
        let _ = cleanup_session.RemovePrintTicketValidationRequested(token);
        cleanup.Complete()
    }))?;
    let result = session.Start();
    if result.is_err() {
        let _ = session.RemovePrintTicketValidationRequested(token);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_is_committed_or_preserved_on_cancellation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("transaction.pdf");
        std::fs::write(&path, b"previous document").unwrap();
        let file = windows::Storage::StorageFile::GetFileFromPathAsync(&path.to_string_lossy().as_ref().into()).unwrap().join().unwrap();
        assert!(write_pdf(&file, b"replacement", &AtomicBool::new(true)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous document");
        let pdf = b"%PDF-1.4\nsynthetic print output\n%%EOF";
        write_pdf(&file, pdf, &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), pdf);
    }
}
