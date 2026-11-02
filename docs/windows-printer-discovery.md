# Windows printer discovery waits on driver capabilities

## Cause

The affected backend at `c5a1cbfb` already uses Windows' registry-backed
`EnumPrinters` level 4, but then reads every driver's PrintCapabilities serially
before returning names. One slow queue delays the whole list. Moving that combined
operation off the UI thread does not make names arrive sooner. The cause is shared
by consumers of the Windows backend.

A read-only probe on Windows 11 x64 measured 49.901 seconds for enumeration with
capabilities, versus 32.482 milliseconds for the same four queues and default
without them. An individual capability read took 46.423 seconds. These are local
measurements, not Windows timeout guarantees.

## Solution

Return names/default without driver capabilities, then query only the selected
printer. Keep the printer list and Print button usable while the answer is pending.
`craft-print` owns cancellation, stale-answer rejection, capability caching and
retry. The UI forwards selection and refresh events.

Reuse successful answers within one dialog. Retry failed reads after reselection.
Opening Print discards all answers; closing native Properties discards only that printer's
answer, leaving other cached answers and pending reads intact. The capability
answer is advisory; submission validates the driver's ticket independently.

Windows queries run in disposable processes so a driver hang or crash does not
hold the UI or another selected printer. Discovery allows ten seconds and
capabilities sixty. Cancellation terminates the query; query deadlines do not
terminate or retry submitted jobs. These processes run with ordinary application
permissions and are not security sandboxes.

The defect and process isolation are Windows-specific. CUPS discovery and transport
are unchanged. The shared session has no platform or UI dependency. External
settings changes while Print stays open require reopening it; hosts that handle
printer-change notifications can use the same refresh operation.

## Prior art

- [Microsoft's EnumPrinters contract](https://learn.microsoft.com/en-us/windows/win32/printdocs/enumprinters)
  distinguishes fast level-4 enumeration from level-2 remote calls.
- [Chromium's BSD Windows backend](https://github.com/chromium/chromium/blob/main/printing/backend/print_backend_win.cc)
  separates enumeration and capability operations. Its enumeration also enriches
  queue metadata, so it is not identical to this names-only API. Its
  [service manager](https://github.com/chromium/chromium/blob/main/chrome/browser/printing/print_backend_service_manager.cc)
  separates Windows drivers into processes and recovers disconnected services.
- [MIT WPF's PrintQueue](https://github.com/dotnet/wpf/blob/main/src/Microsoft.DotNet.Wpf/src/System.Printing/CPP/src/PrintQueue.cpp)
  exposes separate capability reads and explicit refresh of requested properties.
  It is a Windows reference, not a cross-platform dependency.
- [LibreOffice's printer settings documentation](https://help.libreoffice.org/latest/en-GB/text/shared/01/01140000.html?System=WIN)
  covers selection and native Properties. Its implementation and cache were not
  audited.

No external implementation was copied. No copyleft implementation was used to
derive a port, and no third-party assets or fonts were added.

## Verification boundaries

Regression tests cover blocked readers, selection and reopening, failure retry,
late answers, malformed worker requests, Properties completion and independent
pending settings windows. Process tests cover cancellation, deadlines, output
limits and cleanup. Native enumeration and cancellation were measured without
submitting jobs. Physical output is not qualified.

Windows Settings itself can stall on the same native Printing Preferences request
on the investigated machine. That utility behavior predates this change. The
direct native driver dialog opened and cancelled successfully. Pending Properties
work remains off the UI thread and does not block another printer's settings.

Related work: [PdfCraft #2192](https://github.com/storytold/pdfcraft/issues/2192),
[PdfCraft PR #3554](https://github.com/storytold/pdfcraft/pull/3554) and
[craft-libs PR #133](https://github.com/storytold/craft-libs/pull/133).
