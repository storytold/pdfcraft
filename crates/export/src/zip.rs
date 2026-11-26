//! Zip writer for Office files using `craft-zip`.

use craft_zip::{Compression, Writer};

/// Zip archive writer wrapping [`Writer`].
#[derive(Default)]
pub struct Zip {
    writer: Writer,
}

impl Zip {
    /// Add a file. Already-compressed data (images) is stored; the rest is deflated.
    pub fn add(&mut self, name: &str, data: &[u8], compress: bool) {
        let compression = if compress { Compression::Deflated } else { Compression::Stored };
        let _ = self.writer.add(name, data, compression);
    }

    pub fn finish(self) -> Vec<u8> {
        self.writer.finish().unwrap_or_default()
    }
}
