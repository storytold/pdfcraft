//! Password protection to apply, and the security a document has.

use crate::EditError;

/// Document Properties ▸ Security.
#[derive(Clone, Debug, PartialEq)]
pub struct SecuritySummary {
    /// "AES, 256-bit" etc.
    pub method: String,
    /// Opened with the owner password (no restrictions apply).
    pub owner: bool,
    /// For pending protection: the restrictions as they will apply to others.
    pub permissions: pdfcraft_cos::Permissions,
    /// Set in this session; written by the next save.
    pub pending: bool,
}

/// What printing a protected document allows (Acrobat: Printing allowed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Printing {
    None,
    /// "Low Resolution (150 dpi)".
    Low,
    High,
}

/// What changes a protected document allows (Acrobat: Changes allowed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Changes {
    None,
    /// "Inserting, deleting, and rotating pages".
    Pages,
    /// "Filling in form fields and signing existing signature fields".
    FillSign,
    /// "Commenting, filling in form fields, and signing existing signature fields".
    CommentFillSign,
    /// "Any except extracting pages".
    AnyExceptExtract,
}

/// Password protection to apply (Protect Using Password and its Advanced options).
#[derive(Clone, PartialEq)]
pub struct Protection {
    /// Required to open the document (the user password).
    pub open_password: Option<String>,
    /// Required to change security and lift the restrictions below (the owner password).
    pub permissions_password: Option<String>,
    pub printing: Printing,
    pub changes: Changes,
    /// "Enable copying of text, images, and other content".
    pub copy: bool,
    /// "Enable text access for screen reader devices".
    pub accessibility: bool,
    /// Compatibility level; AES-256 (Acrobat X and later) by default.
    pub algorithm: pdfcraft_cos::Algorithm,
    /// `false`: "Encrypt all document contents except metadata".
    pub encrypt_metadata: bool,
}

impl std::fmt::Debug for Protection {
    // Never print passwords (edits end up in logs and error messages).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Protection")
            .field("open_password", &self.open_password.as_ref().map(|_| "…"))
            .field("permissions_password", &self.permissions_password.as_ref().map(|_| "…"))
            .field("printing", &self.printing)
            .field("changes", &self.changes)
            .field("copy", &self.copy)
            .field("accessibility", &self.accessibility)
            .field("algorithm", &self.algorithm)
            .field("encrypt_metadata", &self.encrypt_metadata)
            .finish()
    }
}

impl Default for Protection {
    /// Acrobat's defaults: AES-256, printing and accessibility allowed, no changes, no copying.
    fn default() -> Self {
        Self {
            open_password: None,
            permissions_password: None,
            printing: Printing::High,
            changes: Changes::None,
            copy: false,
            accessibility: true,
            algorithm: pdfcraft_cos::Algorithm::Aes256,
            encrypt_metadata: true,
        }
    }
}

impl Protection {
    /// The `/P` permission bits (ISO 32000-2 Table 22), as Acrobat maps its choices.
    pub fn permission_bits(&self) -> i32 {
        let bit = |n: u32| 1i32 << (n - 1);
        if self.permissions_password.is_none() {
            return -1; // nothing restricted without a permissions password
        }
        let mut p = 0;
        match self.printing {
            Printing::None => {}
            Printing::Low => p |= bit(3),
            Printing::High => p |= bit(3) | bit(12),
        }
        p |= match self.changes {
            Changes::None => 0,
            Changes::Pages => bit(11),
            Changes::FillSign => bit(9),
            Changes::CommentFillSign => bit(6) | bit(9),
            Changes::AnyExceptExtract => bit(4) | bit(6) | bit(9),
        };
        if self.copy {
            p |= bit(5) | bit(10);
        }
        if self.accessibility {
            p |= bit(10);
        }
        p
    }

    pub(crate) fn validate(&self) -> Result<(), EditError> {
        let bad = |m: &str| Err(EditError::Protection(m.to_string()));
        match (&self.open_password, &self.permissions_password) {
            (None, None) => return bad("enter a password to open the document, a permissions password, or both"),
            (Some(a), Some(b)) if a == b => return bad("the open password and the permissions password must be different"),
            _ => {}
        }
        if [&self.open_password, &self.permissions_password].into_iter().flatten().any(String::is_empty) {
            return bad("passwords can't be empty");
        }
        if self.algorithm != pdfcraft_cos::Algorithm::Aes256
            && [&self.open_password, &self.permissions_password].into_iter().flatten().any(|p| p.chars().any(|c| !(' '..='~').contains(&c)))
        {
            return bad("this compatibility level supports only plain ASCII passwords; use AES-256 (Acrobat X and later)");
        }
        // Encryption keys on a password as `effective_password` gives it (AES-256: SASLprep,
        // which drops soft hyphens and folds compatibility forms; older levels: the first 32
        // bytes, padded). One that comes out as the empty password opens the file without asking,
        // and an open password that comes out as the permissions password lifts the restrictions.
        let effective = |p: &str| pdfcraft_cos::effective_password(self.algorithm, p);
        let (open, permissions) = (self.open_password.as_deref().map(effective), self.permissions_password.as_deref().map(effective));
        let empty = effective("");
        if open.iter().chain(&permissions).any(|p| *p == empty) {
            return bad("passwords can't be empty or made only of characters that passwords ignore, such as soft hyphens");
        }
        if open.is_some() && open == permissions {
            return bad("the open password and the permissions password must be different (PDF readers treat these two as the same password)");
        }
        Ok(())
    }
}

/// Opened as owner, or nothing is restricted (no permissions password was set): security may
/// be changed, as in Acrobat.
pub(crate) fn unrestricted(p: &pdfcraft_cos::Permissions) -> bool {
    const ALL: i32 = 0b1111_0011_1100; // bits 3–6 and 9–12
    p.owner || p.bits & ALL == ALL
}
