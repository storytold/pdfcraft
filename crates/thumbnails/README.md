# pdfcraft-thumbnails

First-page thumbnails of PDF files for the operating system's file manager.

- `render_thumbnail` (every platform) renders a document's first page, fitted into a square of the
  requested size, onto white, as top-down BGRA pixels. It never panics: a file that can't be read
  or drawn is an error, and the file manager then shows the file's icon.
- On Windows the crate also builds `pdfcraft_thumbnails.dll`, an `IThumbnailProvider` that File
  Explorer loads (in its isolated thumbnail process) for files whose type is `PdfCraft.Document`,
  so PDFs show their first page while PdfCraft is the default PDF app. The MSI registers it
  (`packaging/windows/pdfcraft.wxs`):

  ```
  HKLM\Software\Classes\CLSID\{68E114F4-EB01-42A6-BD92-1695F3711CD1}\InprocServer32
      (default) = C:\Program Files\PdfCraft\pdfcraft_thumbnails.dll, ThreadingModel = Apartment
  HKLM\Software\Classes\PdfCraft.Document\ShellEx\{e357fccd-a995-4576-b01f-234630154e96}
      (default) = {68E114F4-EB01-42A6-BD92-1695F3711CD1}
  ```

macOS draws PDF thumbnails itself (Quick Look), and Linux file managers use their own
thumbnailers, so there the crate is only the shared rendering function.

This is the one crate where `unsafe` is allowed, and only in `src/windows.rs`: a COM server's
exported entry points take raw pointers, and the bitmap is filled through one.
