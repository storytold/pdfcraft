//! Viewer parser policy and error mapping; independent from a generic importer's defaults.

use std::sync::Arc;

use craft_pdf::syntax::{DecryptionError, LoadPdfError, Pdf};

use crate::OpenError;

fn open_options() -> craft_pdf::OpenOptions {
    // The existing viewer accepts an already-loaded input without a fixed byte cap.
    // File/link entry points keep their own policies; stream decoding remains bounded.
    craft_pdf::OpenOptions { max_input_bytes: usize::MAX, ..Default::default() }
}

pub(crate) fn open(bytes: Arc<Vec<u8>>, password: Option<&str>) -> Result<Pdf, OpenError> {
    craft_pdf::open_with_options(bytes, password, &open_options()).map_err(|error| match error {
        craft_pdf::OpenError::Load(LoadPdfError::Decryption(DecryptionError::PasswordProtected)) => {
            if password.is_some() {
                OpenError::WrongPassword
            } else {
                OpenError::NeedsPassword
            }
        }
        craft_pdf::OpenError::Load(LoadPdfError::Decryption(other)) => OpenError::Unsupported(format!("encryption not supported: {other:?}")),
        craft_pdf::OpenError::Load(LoadPdfError::Invalid) => OpenError::Invalid("no readable page tree or cross-reference data was found".into()),
        craft_pdf::OpenError::BackendPanic => OpenError::Invalid("the parser crashed".into()),
        other => OpenError::Invalid(other.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_policy_does_not_inherit_the_small_importer_input_limit() {
        let size = (1usize << 30) + 1;
        assert!(craft_pdf::check_input_size(size).is_err());
        assert!(craft_pdf::check_input_size_with(size, &open_options()).is_ok());
    }
}
