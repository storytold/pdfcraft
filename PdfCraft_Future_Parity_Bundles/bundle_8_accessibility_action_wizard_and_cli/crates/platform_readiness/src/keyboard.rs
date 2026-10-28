//! Keyboard Focus Manager and Trap Prevention per WCAG 2.1 Guideline 2.1.2.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusableControl {
    pub id: usize,
    pub name: String,
    pub tab_index: i32,
    pub is_enabled: bool,
}

pub struct KeyboardFocusManager {
    controls: Vec<FocusableControl>,
    current_focus_index: Option<usize>,
    modal_active: bool,
}

impl Default for KeyboardFocusManager {
    fn default() -> Self {
        Self {
            controls: Vec::new(),
            current_focus_index: None,
            modal_active: false,
        }
    }
}

impl KeyboardFocusManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_control(&mut self, id: usize, name: impl Into<String>, tab_index: i32) {
        self.controls.push(FocusableControl {
            id,
            name: name.into(),
            tab_index,
            is_enabled: true,
        });
        self.controls.sort_by_key(|c| c.tab_index);
    }

    /// Advances focus on Tab key press, wrapping at end to prevent keyboard traps.
    pub fn on_tab_pressed(&mut self) -> Option<usize> {
        if self.controls.is_empty() {
            return None;
        }

        let next_idx = match self.current_focus_index {
            None => 0,
            Some(curr) => (curr + 1) % self.controls.len(),
        };

        self.current_focus_index = Some(next_idx);
        Some(self.controls[next_idx].id)
    }

    /// Moves focus backward on Shift+Tab key press.
    pub fn on_shift_tab_pressed(&mut self) -> Option<usize> {
        if self.controls.is_empty() {
            return None;
        }

        let prev_idx = match self.current_focus_index {
            None => self.controls.len() - 1,
            Some(0) => self.controls.len() - 1,
            Some(curr) => curr - 1,
        };

        self.current_focus_index = Some(prev_idx);
        Some(self.controls[prev_idx].id)
    }

    /// Dismisses active modal or resets focus on Escape.
    pub fn on_escape_pressed(&mut self) -> bool {
        if self.modal_active {
            self.modal_active = false;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyboard_tab_cycling_no_trap() {
        let mut mgr = KeyboardFocusManager::new();
        mgr.register_control(1, "OpenButton", 1);
        mgr.register_control(2, "SaveButton", 2);
        mgr.register_control(3, "CloseButton", 3);

        assert_eq!(mgr.on_tab_pressed(), Some(1));
        assert_eq!(mgr.on_tab_pressed(), Some(2));
        assert_eq!(mgr.on_tab_pressed(), Some(3));
        // Must wrap around to 1 (no focus trap!)
        assert_eq!(mgr.on_tab_pressed(), Some(1));
    }
}
