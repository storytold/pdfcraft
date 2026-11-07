# Windows print jobs rejected before submission

A report on PdfCraft 0.19.0 described the preview closing without output while Edge printed the same PDF. On the investigated Windows machine, the HUAWEI CV81Z driver accepted the PrintTicket (paper, grayscale, duplex and collation), and its capabilities XML parsed successfully.

The native submission test failed before queueing: `StorageFile::GetFileFromPathAsync` returned `0x80070005` for the temporary PDF. Ordinary desktop file access could read it. This isolates one reproducible failure; it does not establish the cause of every report of an empty print queue.

The temporary winprint patch reads the job using desktop file access, transfers it in 64 KiB chunks to an `InMemoryRandomAccessStream`, and calls `PdfDocument::LoadFromStreamAsync`. It does not fall back to another printer or retry an already submitted job. Tickets, layout, permissions and completion handling remain in place. The stream still holds the PDF in memory; large-job memory use needs qualification.

Upstream fix: https://github.com/storytold/winprint.rs/pull/15. Remove the temporary patch once craft-print adopts a revision containing it.

## Evidence

- Existing Windows loader: synthetic A4 job failed at PDF loading with `0x80070005`.
- Stream loader: same HUAWEI job returned `spooled 1 sheet(s)`.
- No-printer regression: existing test PDF in a hidden temporary file fails under the old loader and passes under the new one.
- 14 Print Schema reader/writer tests passed. Wider native ticket tests require installing a null printer; 13 failed because Add-Printer was denied (20 passed).
- Physical paper output and a packaged 0.19.0 application build have not been independently verified. A successful queue submission is not proof of physical printing.

When reporting a different failure, include the PdfCraft version, printer/driver name, displayed error and whether a job appears in the Windows queue. Saving the print-ready PDF and printing it in another viewer remains a workaround until the fix ships.
