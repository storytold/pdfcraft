//! Recipient Permissions Mask per ISO 32000-1 Table 22.

pub const PERM_PRINT_LOW_RES: u32 = 1 << 2;
pub const PERM_MODIFY_CONTENTS: u32 = 1 << 3;
pub const PERM_COPY_TEXT: u32 = 1 << 4;
pub const PERM_ADD_ANNOTATIONS: u32 = 1 << 5;
pub const PERM_FILL_FORMS: u32 = 1 << 8;
pub const PERM_ACCESSIBILITY_COPY: u32 = 1 << 9;
pub const PERM_ASSEMBLE_DOCUMENT: u32 = 1 << 10;
pub const PERM_PRINT_HIGH_RES: u32 = 1 << 11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecipientPermissions {
    pub raw_mask: u32,
}

impl Default for RecipientPermissions {
    fn default() -> Self {
        Self { raw_mask: 0xFFFFFFFC } // All allowed by default
    }
}

impl RecipientPermissions {
    pub fn can_print_high_res(&self) -> bool {
        (self.raw_mask & PERM_PRINT_HIGH_RES) != 0
    }

    pub fn can_copy_text(&self) -> bool {
        (self.raw_mask & PERM_COPY_TEXT) != 0
    }

    pub fn can_modify_contents(&self) -> bool {
        (self.raw_mask & PERM_MODIFY_CONTENTS) != 0
    }

    pub fn can_fill_forms(&self) -> bool {
        (self.raw_mask & PERM_FILL_FORMS) != 0
    }
}
