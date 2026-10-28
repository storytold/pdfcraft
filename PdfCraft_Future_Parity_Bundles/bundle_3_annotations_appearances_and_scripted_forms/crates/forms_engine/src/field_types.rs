//! Form Field Flags and Types per ISO 32000-1 §12.7.3.

pub const FLAG_READ_ONLY: u32 = 1 << 0;
pub const FLAG_REQUIRED: u32 = 1 << 1;
pub const FLAG_NO_EXPORT: u32 = 1 << 2;
pub const FLAG_MULTILINE: u32 = 1 << 12; // 4096
pub const FLAG_PASSWORD: u32 = 1 << 13;  // 8192
pub const FLAG_COMB: u32 = 1 << 24;      // 16777216

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormFieldType {
    Text,
    PushButton,
    CheckBox,
    RadioButton,
    ChoiceList,
    ChoiceComboBox,
}

#[derive(Debug, Clone)]
pub struct FormField {
    pub name: String,
    pub field_type: FormFieldType,
    pub flags: u32,
    pub value: String,
    pub default_value: Option<String>,
    pub max_len: Option<usize>,
    pub bbox: [f32; 4],
    pub default_appearance: String, // e.g. "/Helv 0 Tf 0 g" (0 means auto-size)
}

impl FormField {
    pub fn is_multiline(&self) -> bool {
        (self.flags & FLAG_MULTILINE) != 0
    }

    pub fn is_password(&self) -> bool {
        (self.flags & FLAG_PASSWORD) != 0
    }

    pub fn is_comb(&self) -> bool {
        (self.flags & FLAG_COMB) != 0
    }
}
