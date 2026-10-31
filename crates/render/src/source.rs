//! What a renderer reads: a file in memory, or a file kept on disk.

use std::sync::Arc;

use hayro::hayro_syntax::{DetachedBodies, PdfData};
use pdfcraft_cos::{DiskBody, OnDiskFile};

/// A PDF to read. Anything that takes one takes the bytes of a whole file too.
#[derive(Clone, Debug)]
pub enum PdfSource {
    Whole(Arc<Vec<u8>>),
    OnDisk(Arc<OnDiskFile>),
}

impl From<Arc<Vec<u8>>> for PdfSource {
    fn from(bytes: Arc<Vec<u8>>) -> Self {
        Self::Whole(bytes)
    }
}

impl From<Arc<OnDiskFile>> for PdfSource {
    fn from(file: Arc<OnDiskFile>) -> Self {
        Self::OnDisk(file)
    }
}

impl PdfSource {
    /// The size of the file.
    pub fn size(&self) -> u64 {
        match self {
            Self::Whole(bytes) => bytes.len() as u64,
            Self::OnDisk(file) => file.size(),
        }
    }

    /// How many reads from disk have failed so far: a stream that couldn't be read draws as
    /// nothing, and this is how a renderer finds out.
    pub(crate) fn failed_reads(&self) -> u64 {
        match self {
            Self::Whole(_) => 0,
            Self::OnDisk(file) => file.failed_reads(),
        }
    }

    pub(crate) fn data(&self) -> PdfData {
        match self {
            Self::Whole(bytes) => bytes.clone().into(),
            Self::OnDisk(file) => PdfData::with_detached(file.held().clone(), Arc::new(Bodies(file.clone()))),
        }
    }
}

struct Bodies(Arc<OnDiskFile>);

impl DetachedBodies for Bodies {
    fn held_offset(&self, offset: usize) -> Option<usize> {
        self.0.held_offset(offset as u64)
    }
    fn body_at(&self, held: usize) -> Option<(u64, u64)> {
        self.0.body_at(held).map(|body| (body.start, body.len))
    }
    fn read(&self, start: u64, len: u64) -> Option<Vec<u8>> {
        self.0.read_body(DiskBody { start, len })
    }
}
