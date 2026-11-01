//! Reading a large file from disk a part at a time (`core.lazy-loading`). See
//! [`pdfcraft_cos::OnDiskFile`].

use std::fs::File;
use std::path::Path;
use std::time::SystemTime;

use pdfcraft_cos::{ByteSource, OnDiskFile};

use crate::EditError;

/// Files from this size up open from disk, when they can (see [`crate::OpenContext::prepare_on_disk`]).
/// A smaller file costs little to hold, and holding it leaves it free to edit.
pub const ON_DISK_FROM: u64 = 1 << 30;

/// Why a document that stayed on disk can't be edited.
pub const ON_DISK_REASON: &str = "this large file is being read from disk; load it for editing first";

/// A file on disk, read at any offset from any thread. It keeps the handle it was opened with, so
/// a file replaced under its name reads as it was, and it refuses to read a file changed in place:
/// offsets into it mean nothing then. On Windows that handle makes `MoveFileEx` over the file fail
/// while it is open; renaming, deleting and `ReplaceFile` still work.
pub struct FileSource {
    file: File,
    size: u64,
    modified: Option<SystemTime>,
}

/// Check that `file` is still what its own `path` holds, going by size and time. Saving it there
/// writes nothing, so it must not report success for a file that has gone or been replaced.
pub fn check_own_path(path: &Path, file: &OnDiskFile) -> Result<(), EditError> {
    let found = std::fs::metadata(path).map_err(|_| EditError::ChangedOnDisk)?;
    if found.len() == file.size() && found.modified().ok() == file.modified() { Ok(()) } else { Err(EditError::ChangedOnDisk) }
}

impl FileSource {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        Ok(Self { size: metadata.len(), modified: metadata.modified().ok(), file })
    }
}

impl ByteSource for FileSource {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, out: &mut [u8]) -> std::io::Result<()> {
        let now = self.file.metadata()?;
        if now.len() != self.size || now.modified().ok() != self.modified {
            return Err(std::io::Error::other("the file changed on disk"));
        }
        read_exact_at(&self.file, offset, out)
    }

    fn modified(&self) -> Option<SystemTime> {
        self.modified
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, offset: u64, out: &mut [u8]) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, out, offset)
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut offset: u64, mut out: &mut [u8]) -> std::io::Result<()> {
    while !out.is_empty() {
        match std::os::windows::fs::FileExt::seek_read(file, out, offset) {
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                offset += n as u64;
                out = out.get_mut(n..).ok_or(std::io::ErrorKind::InvalidInput)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn read_exact_at(_: &File, _: u64, _: &mut [u8]) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edit, Session};
    use pdfcraft_testkit::Pdf;
    use std::sync::Arc;

    /// A file in its own folder, gone with the folder when the test ends.
    struct Temp {
        dir: std::path::PathBuf,
        path: std::path::PathBuf,
    }

    impl Temp {
        fn new(bytes: &[u8]) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("pdfcraft-on-disk-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("large.pdf");
            std::fs::write(&path, bytes).unwrap();
            Self { dir, path }
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A page drawn by one 300 by 300 image, with a bookmark.
    fn scanned() -> Vec<u8> {
        let image: Vec<u8> = (0..90_000u32).map(|i| (i % 241) as u8).collect();
        Pdf::new()
            .obj("<</Type/Catalog/Pages 2 0 R/Outlines 7 0 R>>")
            .obj("<</Type/Pages/Kids[3 0 R]/Count 1>>")
            .obj("<</Type/Page/Parent 2 0 R/MediaBox[0 0 100 100]/Contents 4 0 R/Resources<</XObject<</I 5 0 R>>>>/Annots[6 0 R]>>")
            .stream("", "q 100 0 0 100 0 0 cm /I Do Q")
            .stream("/Type/XObject/Subtype/Image/Width 300/Height 300/BitsPerComponent 8/ColorSpace/DeviceGray", image)
            .obj("<</Type/Annot/Subtype/Text/Rect[0 0 10 10]/Contents(a note)>>")
            .obj("<</Type/Outlines/First 8 0 R/Last 8 0 R/Count 1>>")
            .obj("<</Title(Chapter one)/Parent 7 0 R/Dest[3 0 R/Fit]>>")
            .build()
    }

    /// Open `path` from disk whatever its size.
    fn open(path: &Path) -> Option<(Session, crate::DocId)> {
        let mut session = Session::default().with_on_disk_from(0);
        let prepared = session.open_context().prepare_on_disk("large.pdf", path)?;
        let id = session.commit_open(prepared);
        Some((session, id))
    }

    #[test]
    fn a_large_file_opens_with_its_structure_and_draws_from_disk() {
        let pdf = scanned();
        let file = Temp::new(&pdf);
        let (session, id) = open(&file.path).expect("stays on disk");
        let doc = session.get(id).unwrap();
        assert!(doc.on_disk.is_some() && doc.bytes.is_empty(), "the file's bytes are not in memory");
        assert_eq!(doc.info.pages.len(), 1);
        assert_eq!(doc.info.file_size, pdf.len());
        assert_eq!(doc.info.outline.len(), 1, "bookmarks read");
        assert_eq!(doc.info.annotations.len(), 1, "comments read");
        assert!(doc.read_only_reason.is_some() && !doc.editable());
        let whole = {
            let mut session = Session::default();
            let id = session.open("large.pdf", None, Arc::new(pdf.clone()), None).unwrap();
            let doc = session.get(id).unwrap();
            pdfcraft_render::PageRenderer::new(doc.source(), Default::default()).render(Default::default())
        };
        let page = pdfcraft_render::PageRenderer::new(doc.source(), Default::default()).render(Default::default());
        assert_eq!(page.error, None);
        assert!(page.rgba == whole.rgba, "the same pixels");
    }

    #[test]
    fn a_smaller_file_is_read_whole() {
        let file = Temp::new(&scanned());
        assert!(Session::default().open_context().prepare_on_disk("large.pdf", &file.path).is_none());
    }

    #[test]
    fn what_is_not_a_clean_pdf_is_read_whole() {
        let mut no_xref = scanned();
        let at = no_xref.windows(9).rposition(|w| w == b"startxref").unwrap();
        no_xref.truncate(at);
        for bytes in [no_xref, b"just some text".repeat(4_000), vec![0; 20_000]] {
            let file = Temp::new(&bytes);
            assert!(open(&file.path).is_none());
        }
        assert!(open(Path::new("no such file")).is_none());
    }

    #[test]
    fn an_encrypted_file_is_read_whole() {
        let mut session = Session::default();
        let id = session.open("large.pdf", None, Arc::new(scanned()), None).unwrap();
        let protection = crate::Protection { permissions_password: Some("owner".into()), copy: false, ..crate::Protection::default() };
        session.apply(id, Edit::Protect(protection)).unwrap();
        let file = Temp::new(&session.save_full_bytes(id).unwrap());
        assert!(open(&file.path).is_none(), "permissions come from an editor, which a file on disk has none of");
    }

    #[test]
    fn a_document_on_disk_is_never_saved_from_memory() {
        let file = Temp::new(&scanned());
        let (mut session, id) = open(&file.path).expect("stays on disk");
        assert_eq!(session.save_bytes(id), Err(EditError::OnDisk));
        assert!(session.save_full_bytes(id).is_err());
        assert!(matches!(session.apply(id, Edit::DeletePages { pages: vec![0] }), Err(EditError::ReadOnly(_))));
        assert_eq!(session.get(id).unwrap().whole().err(), Some(EditError::OnDisk));
    }

    #[test]
    fn what_needs_the_whole_file_says_so() {
        let file = Temp::new(&scanned());
        let (session, id) = open(&file.path).expect("stays on disk");
        let doc = session.get(id).unwrap();
        let source = doc.export_source();
        assert!(crate::export::extract_images(&source, &[0], 1).is_err_and(|e| e.contains("load the whole file")));
        assert!(session.export_data(id, crate::DataFormat::Xfdf, true, false).is_err());
        assert!(
            crate::print_raster::rasterize(doc, &[0], &Default::default(), pdfcraft_print::Content::default())
                .is_err_and(|e| e.contains("load the whole file"))
        );
    }

    #[test]
    fn a_file_changed_in_place_stops_reading() {
        let pdf = scanned();
        let file = Temp::new(&pdf);
        let (session, id) = open(&file.path).expect("stays on disk");
        let mut renderer = pdfcraft_render::PageRenderer::new(session.get(id).unwrap().source(), Default::default());
        std::fs::write(&file.path, &pdf[..pdf.len() - 10]).unwrap();
        let page = renderer.render(Default::default());
        assert!(page.error.is_some_and(|e| e.contains("could not be read")));
    }

    #[test]
    fn a_file_replaced_under_its_name_keeps_reading_as_it_was() {
        let pdf = scanned();
        let file = Temp::new(&pdf);
        let (session, id) = open(&file.path).expect("stays on disk");
        let mut renderer = pdfcraft_render::PageRenderer::new(session.get(id).unwrap().source(), Default::default());
        let swapped = file.dir.join("new.pdf");
        std::fs::write(&swapped, b"another file").unwrap();
        std::fs::rename(&swapped, &file.path).unwrap();
        assert_eq!(renderer.render(Default::default()).error, None);
    }

    #[test]
    fn a_save_knows_the_file_by_its_size_and_time() {
        let pdf = scanned();
        let file = Temp::new(&pdf);
        let (session, id) = open(&file.path).expect("stays on disk");
        let held = session.get(id).unwrap().on_disk.clone().unwrap();
        assert_eq!(check_own_path(&file.path, &held), Ok(()));

        // The same size and a later time, as a rebuild leaves a file.
        let other = file.dir.join("other.pdf");
        let mut rebuilt = pdf.clone();
        rebuilt[0] ^= 1;
        std::fs::write(&other, &rebuilt).unwrap();
        let later = held.modified().unwrap() + std::time::Duration::from_secs(5);
        File::options().write(true).open(&other).unwrap().set_modified(later).unwrap();
        std::fs::rename(&other, &file.path).unwrap();
        assert_eq!(check_own_path(&file.path, &held), Err(EditError::ChangedOnDisk));

        std::fs::remove_file(&file.path).unwrap();
        assert_eq!(check_own_path(&file.path, &held), Err(EditError::ChangedOnDisk));
    }

    #[test]
    fn loading_the_whole_file_makes_it_editable() {
        let pdf = scanned();
        let file = Temp::new(&pdf);
        let (mut session, id) = open(&file.path).expect("stays on disk");
        session.reload(id, Arc::new(pdf)).unwrap();
        let doc = session.get(id).unwrap();
        assert!(doc.on_disk.is_none() && doc.editable() && doc.read_only_reason.is_none());
        assert_eq!(session.docs().len(), 1, "the same tab");
    }

    #[test]
    fn view_settings_keep_reading_from_disk() {
        let file = Temp::new(&scanned());
        let (mut session, id) = open(&file.path).expect("stays on disk");
        assert!(session.set_hide_comments(id, true));
        let doc = session.get(id).unwrap();
        doc.renderer.set_queue(vec![Default::default()]);
        let start = std::time::Instant::now();
        let page = loop {
            match doc.renderer.try_recv() {
                Some(page) => break page,
                None if start.elapsed().as_secs() < 30 => std::thread::yield_now(),
                None => panic!("the page never came"),
            }
        };
        assert_eq!(page.error, None, "the new renderer reads the file too");
        assert!(page.rgba.as_chunks::<4>().0.iter().any(|p| p[0] != 255));
    }

    #[test]
    fn the_file_copies_exactly() {
        let pdf = scanned();
        let file = Temp::new(&pdf);
        let held = OnDiskFile::read(Arc::new(FileSource::open(&file.path).unwrap())).unwrap().unwrap();
        let mut out = Vec::new();
        held.copy_to(&mut out).unwrap();
        assert_eq!(out, pdf);
    }
}
