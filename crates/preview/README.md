# pdfcraft-preview

First-page preview of PDF files in the Windows File Explorer preview pane (#3207).

- [`render_page_fit`] (every platform) renders a document's first page, fitted into a bounded
  rectangle, onto white, as top-down BGRA pixels. It never panics. Rendering is local to this
  crate so the preview DLL does not link `pdfcraft_thumbnails` (duplicate COM exports).
- On Windows the crate also builds `pdfcraft_preview.dll`, an `IPreviewHandler` that Explorer and
  other preview hosts load for files whose type is `PdfCraft.Document`, so PDFs show their first
  page in the preview pane while PdfCraft is the default PDF app. The MSI registers it under HKMU
  (per-user or per-machine), consistent with the thumbnail handler (`packaging/windows/pdfcraft.wxs`):

  ```
  HKMU\Software\Classes\CLSID\{C31A5F44-7E02-4D9A-A1B3-8F2E6D4C0A91}\InprocServer32
      (default) = …\pdfcraft_preview.dll, ThreadingModel = Apartment
  HKMU\Software\Classes\PdfCraft.Document\ShellEx\{8895b1c6-b41f-4c1c-a562-0d564250836f}
      (default) = {C31A5F44-7E02-4D9A-A1B3-8F2E6D4C0A91}
  HKMU\Software\Microsoft\Windows\CurrentVersion\PreviewHandlers\{C31A5F44-7E02-4D9A-A1B3-8F2E6D4C0A91}
      (default) = PdfCraft PDF preview, AppId = {C31A5F44-7E02-4D9A-A1B3-8F2E6D4C0A91}
  ```

macOS Quick Look and Linux file managers use their own preview mechanisms; there the crate is only
linked for shared tests of the rendering helper.

This is the one crate where `unsafe` is allowed, and only in `src/windows.rs`: a COM server's
exported entry points take raw pointers, and the preview bitmap is painted through GDI.
