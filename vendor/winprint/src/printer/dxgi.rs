use crate::printer::PrinterDevice;
use crate::ticket::PrintTicket;
use crate::ticket::ToDevModeError;
use crate::utils::com::ComInitializer;
use crate::utils::print_completion_source::PrintCompletionSource;
use crate::utils::print_completion_source::PrintCompletionWaiter;
use crate::utils::wchar;
use std::cmp::max;
use std::ffi::OsStr;
use std::ptr;
use thiserror::Error;
use windows::core::Interface;
use windows::core::PCWSTR;
use windows::Win32::Foundation::E_UNEXPECTED;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct2D::D2D1CreateFactory;
use windows::Win32::Graphics::Direct2D::ID2D1Device;
use windows::Win32::Graphics::Direct2D::ID2D1DeviceContext;
use windows::Win32::Graphics::Direct2D::ID2D1Factory1;
use windows::Win32::Graphics::Direct2D::ID2D1PrintControl;
use windows::Win32::Graphics::Direct2D::D2D1_DEVICE_CONTEXT_OPTIONS_NONE;
use windows::Win32::Graphics::Direct2D::D2D1_FACTORY_OPTIONS;
use windows::Win32::Graphics::Direct2D::D2D1_FACTORY_TYPE_SINGLE_THREADED;
use windows::Win32::Graphics::Direct2D::D2D1_PRINT_CONTROL_PROPERTIES;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::D3D11CreateDevice;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext;
use windows::Win32::Graphics::Direct3D11::D3D11_CREATE_DEVICE_BGRA_SUPPORT;
use windows::Win32::Graphics::Direct3D11::D3D11_SDK_VERSION;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::DEVMODEW;
use windows::Win32::Graphics::Imaging::CLSID_WICImagingFactory2;
use windows::Win32::Graphics::Imaging::D2D::IWICImagingFactory2;
use windows::Win32::Storage::Xps::Printing::IPrintDocumentPackageStatusEvent;
use windows::Win32::Storage::Xps::Printing::IPrintDocumentPackageTarget;
use windows::Win32::Storage::Xps::Printing::IPrintDocumentPackageTargetFactory;
use windows::Win32::Storage::Xps::Printing::PrintDocumentPackageTargetFactory;
use windows::Win32::System::Com::CoCreateInstance;
use windows::Win32::System::Com::CLSCTX_ALL;
use windows::Win32::System::Com::{IConnectionPoint, IConnectionPointContainer};
use windows::Win32::UI::Shell::SHCreateMemStream;

#[derive(Error, Debug)]
/// Represents an error occurred while printing via DXGI.
pub enum DxgiPrintContextError {
    /// Print ticket error.
    #[error("Print Ticker Error")]
    PrintTicketError(#[source] ToDevModeError),
    /// Failed to create event.
    #[error("Failed to create event")]
    FailedToCreateEvent(#[source] windows::core::Error),
    /// Failed to initialize DirectX.
    #[error("Failed to initialize DirectX")]
    InitializeDirectX(#[source] windows::core::Error),
    /// Failed to start job.
    #[error("Failed to start job")]
    FailedToStartJob(#[source] windows::core::Error),
    /// Stream not allocated.
    #[error("Stream not allocated")]
    StreamNotAllocated,
    /// The spooler reported failure/cancellation, or did not finish within two minutes.
    #[error("Print job failed, was canceled, or timed out")]
    CompletionFailed(#[source] windows::core::Error),
    /// Failed to close print control.
    #[error("Failed to close print control")]
    FailedToClosePrintControl(#[source] windows::core::Error),
}

const DEFAULT_DPI: i16 = 300;

/// Represents a print context during printing via DXGI.
#[allow(missing_docs)]
pub struct DxgiPrintContext {
    pub d3d_device: ID3D11Device,
    pub d3d_context: ID3D11DeviceContext,
    pub dxgi_device: IDXGIDevice,
    pub d2d_factory: ID2D1Factory1,
    pub d2d_device: ID2D1Device,
    pub d2d_context: ID2D1DeviceContext,
    pub wic_factory: IWICImagingFactory2,
    pub document_target_factory: IPrintDocumentPackageTargetFactory,
    pub document_target: IPrintDocumentPackageTarget,
    pub print_control: ID2D1PrintControl,
    pub completion_waiter: PrintCompletionWaiter,
    status_connection: IConnectionPoint,
    status_cookie: u32,
    completed: bool,
    // The last field will be dropped last, according to the drop order.
    _com_initializer: ComInitializer,
}

impl DxgiPrintContext {
    /// Create a new print context, used for printing via DXGI.
    pub fn new(
        device: &PrinterDevice,
        options: &PrintTicket,
        job_name: &OsStr,
    ) -> Result<Self, DxgiPrintContextError> {
        // Initialize COM before calling any COM functions.
        let _com_initializer = ComInitializer::new();
        unsafe {
            let dev_mode = options
                .to_dev_mode(device)
                .map_err(DxgiPrintContextError::PrintTicketError)?;
            let raster_dpi = {
                let dev_mode = &*(dev_mode.as_ptr() as *const DEVMODEW);
                let dpi_x = if (dev_mode.dmFields & windows::Win32::Graphics::Gdi::DM_PRINTQUALITY)
                    .0
                    != 0
                {
                    Some(dev_mode.Anonymous1.Anonymous1.dmPrintQuality)
                } else {
                    None
                };
                let dpi_y =
                    if (dev_mode.dmFields & windows::Win32::Graphics::Gdi::DM_YRESOLUTION).0 != 0 {
                        Some(dev_mode.dmYResolution)
                    } else {
                        None
                    };
                match (dpi_x, dpi_y) {
                    (Some(x), Some(y)) => max(x, y),
                    (Some(x), None) => x,
                    (None, Some(y)) => y,
                    (None, None) => DEFAULT_DPI,
                }
            };
            let mut d3d_device: Option<ID3D11Device> = None;
            let mut d3d_context: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(ptr::addr_of_mut!(d3d_device)),
                None,
                Some(ptr::addr_of_mut!(d3d_context)),
            )
            .map_err(DxgiPrintContextError::InitializeDirectX)?;
            let d3d_device = d3d_device.ok_or(DxgiPrintContextError::InitializeDirectX(
                windows::core::Error::from_hresult(E_UNEXPECTED),
            ))?;
            let d3d_context = d3d_context.ok_or(DxgiPrintContextError::InitializeDirectX(
                windows::core::Error::from_hresult(E_UNEXPECTED),
            ))?;
            let dxgi_device = d3d_device
                .cast::<IDXGIDevice>()
                .map_err(DxgiPrintContextError::InitializeDirectX)?;

            let d2d_factory_options = D2D1_FACTORY_OPTIONS::default();
            let d2d_factory: ID2D1Factory1 = D2D1CreateFactory(
                D2D1_FACTORY_TYPE_SINGLE_THREADED,
                Some(&d2d_factory_options),
            )
            .map_err(DxgiPrintContextError::InitializeDirectX)?;

            let d2d_device = d2d_factory
                .CreateDevice(&dxgi_device)
                .map_err(DxgiPrintContextError::InitializeDirectX)?;
            let d2d_context = d2d_device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
                .map_err(DxgiPrintContextError::InitializeDirectX)?;

            let wic_factory: IWICImagingFactory2 =
                CoCreateInstance(&CLSID_WICImagingFactory2, None, CLSCTX_ALL)
                    .map_err(DxgiPrintContextError::InitializeDirectX)?;

            let document_target_factory: IPrintDocumentPackageTargetFactory =
                CoCreateInstance(&PrintDocumentPackageTargetFactory, None, CLSCTX_ALL)
                    .map_err(DxgiPrintContextError::FailedToStartJob)?;
            let print_ticket_stream = SHCreateMemStream(Some(options.get_xml()))
                .ok_or(DxgiPrintContextError::StreamNotAllocated)?;
            let document_target = document_target_factory
                .CreateDocumentPackageTargetForPrintJob(
                    PCWSTR(wchar::to_wide_chars(device.os_name()).as_ptr()),
                    PCWSTR(wchar::to_wide_chars(job_name).as_ptr()),
                    None,
                    &print_ticket_stream,
                )
                .map_err(DxgiPrintContextError::FailedToStartJob)?;
            let print_control_options = D2D1_PRINT_CONTROL_PROPERTIES {
                rasterDPI: raster_dpi as f32,
                ..Default::default()
            };
            let print_control = d2d_device
                .CreatePrintControl(
                    &wic_factory,
                    &document_target,
                    Some(ptr::addr_of!(print_control_options)),
                )
                .map_err(DxgiPrintContextError::FailedToStartJob)?;

            let completion_source =
                PrintCompletionSource::new().map_err(DxgiPrintContextError::FailedToCreateEvent)?;
            let completion_waiter = completion_source.waiter();
            let completion_source: IPrintDocumentPackageStatusEvent = completion_source.into();
            let cpc: IConnectionPointContainer = document_target
                .cast()
                .map_err(DxgiPrintContextError::FailedToCreateEvent)?;
            let status_connection = cpc
                .FindConnectionPoint(&IPrintDocumentPackageStatusEvent::IID)
                .map_err(DxgiPrintContextError::FailedToCreateEvent)?;
            let status_cookie = status_connection
                .Advise(&completion_source)
                .map_err(DxgiPrintContextError::FailedToCreateEvent)?;

            Ok(Self {
                d3d_device,
                d3d_context,
                dxgi_device,
                d2d_factory,
                d2d_device,
                d2d_context,
                wic_factory,
                document_target_factory,
                document_target,
                print_control,
                completion_waiter,
                status_connection,
                status_cookie,
                completed: false,
                _com_initializer,
            })
        }
    }

    /// Close the print context and wait for the completion of the print job.
    pub fn close_and_wait(mut self) -> Result<(), DxgiPrintContextError> {
        unsafe {
            self.print_control
                .Close()
                .map(|_| ())
                .map_err(DxgiPrintContextError::FailedToClosePrintControl)?;
        }
        self.completion_waiter
            .wait()
            .map_err(DxgiPrintContextError::CompletionFailed)?;
        self.completed = true;
        Ok(())
    }
}

impl Drop for DxgiPrintContext {
    fn drop(&mut self) {
        unsafe {
            // A render/Close/terminal error must not leave an incomplete job behind. Cancel
            // is best effort: report the original failure so callers do not assume success.
            if !self.completed {
                let _ = self.document_target.Cancel();
            }
            // Release the event sink while the COM apartment is still initialized.
            let _ = self.status_connection.Unadvise(self.status_cookie);
        }
    }
}
