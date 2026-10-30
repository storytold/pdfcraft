//! A PIN buffer whose editor-facing text contains only bullets. No plaintext undo snapshots.
use egui::{TextBuffer, text::CharIndex};
use zeroize::Zeroizing;

#[derive(Clone, Default, PartialEq)]
pub struct SecretPin {
    secret: Zeroizing<String>,
    mask: String,
}
impl std::fmt::Debug for SecretPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretPin([redacted])")
    }
}
impl SecretPin {
    pub fn value(&self) -> Option<&str> {
        (!self.secret.is_empty()).then_some(self.secret.as_str())
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub(crate) fn field(&mut self, ui: &mut egui::Ui, salt: &str) -> egui::Response {
        let id = ui.make_persistent_id(salt);
        if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), id) {
            state.set_undoer(Default::default());
            state.store(ui.ctx(), id);
        }
        let mut output = crate::widgets::line(self).password(true).id(id).desired_width(220.0).show(ui);
        output.state.set_undoer(Default::default());
        output.state.store(ui.ctx(), id);
        output.response.response
    }
    fn offset(&self, at: usize) -> usize {
        self.secret.char_indices().nth(at).map_or(self.secret.len(), |(i, _)| i)
    }
    fn update(&mut self, value: Zeroizing<String>) {
        self.mask = "•".repeat(value.chars().count());
        self.secret = value; // Zeroizing clears the previous allocation, including deleted input.
    }
}
impl TextBuffer for SecretPin {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }
    fn is_mutable(&self) -> bool {
        true
    }
    fn as_str(&self) -> &str {
        &self.mask
    }
    fn insert_text(&mut self, text: &str, at: CharIndex) -> usize {
        if self.secret.len().saturating_add(text.len()) > 256 {
            return 0;
        }
        let offset = self.offset(at.0);
        let Some((before, after)) = self.secret.get(..offset).zip(self.secret.get(offset..)) else { return 0 };
        let mut value = Zeroizing::new(String::with_capacity(self.secret.len().saturating_add(text.len())));
        value.push_str(before);
        value.push_str(text);
        value.push_str(after);
        self.update(value);
        text.chars().count()
    }
    fn delete_char_range(&mut self, range: std::ops::Range<CharIndex>) {
        if range.start > range.end {
            return;
        }
        let (start, end) = (self.offset(range.start.0), self.offset(range.end.0));
        let Some((before, after)) = self.secret.get(..start).zip(self.secret.get(end..)) else { return };
        let mut value = Zeroizing::new(String::with_capacity(before.len().saturating_add(after.len())));
        value.push_str(before);
        value.push_str(after);
        self.update(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn certificate_pin_editor_never_exposes_plaintext_and_preserves_utf8_and_spaces() {
        let mut pin = SecretPin::default();
        assert_eq!(pin.insert_text(" é12 ", CharIndex(0)), 5);
        assert_eq!(pin.as_str(), "•••••");
        assert_eq!(pin.value(), Some(" é12 "));
        pin.delete_char_range(CharIndex(1)..CharIndex(2));
        assert_eq!(pin.value(), Some(" 12 "));
        assert!(!format!("{pin:?}").contains("12"));
        assert_eq!(pin.insert_text(&"x".repeat(257), CharIndex(0)), 0);
        pin.clear();
        assert_eq!(pin.value(), None);
        assert_eq!(pin.as_str(), "");
    }
}
