//! AccessKit Screen-Reader Bridge for Native OS Accessibility (UIA / NSAccessibility / AT-SPI).
//! Maps PDF Logical Structure (/StructTreeRoot) into accessible node hierarchies.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessibleRole {
    Document,
    Heading(u8), // Level 1..6
    Paragraph,
    Table { rows: usize, cols: usize },
    TableRow,
    TableHeaderCell,
    TableCell,
    Image,
    Link,
    Button,
    TextInput,
    CheckBox,
}

#[derive(Debug, Clone)]
pub struct AccessibleNode {
    pub id: u64,
    pub role: AccessibleRole,
    pub name: Option<String>,
    pub value: Option<String>,
    pub description: Option<String>, // Alternative text (/Alt)
    pub children: Vec<AccessibleNode>,
}

impl AccessibleNode {
    pub fn new(id: u64, role: AccessibleRole) -> Self {
        Self {
            id,
            role,
            name: None,
            value: None,
            description: None,
            children: Vec::new(),
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_alt_text(mut self, alt: impl Into<String>) -> Self {
        self.description = Some(alt.into());
        self
    }

    pub fn add_child(&mut self, child: AccessibleNode) {
        self.children.push(child);
    }
}

pub struct ScreenReaderTreeBuilder {
    next_id: u64,
}

impl Default for ScreenReaderTreeBuilder {
    fn default() -> Self {
        Self { next_id: 1 }
    }
}

impl ScreenReaderTreeBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Builds accessible root node for the PDF document.
    pub fn build_document_root(&mut self, title: Option<&str>) -> AccessibleNode {
        let id = self.alloc_id();
        let mut node = AccessibleNode::new(id, AccessibleRole::Document);
        if let Some(t) = title {
            node.name = Some(t.to_string());
        }
        node
    }

    pub fn build_heading(&mut self, level: u8, text: &str) -> AccessibleNode {
        let id = self.alloc_id();
        AccessibleNode::new(id, AccessibleRole::Heading(level.clamp(1, 6))).with_name(text)
    }

    pub fn build_paragraph(&mut self, text: &str) -> AccessibleNode {
        let id = self.alloc_id();
        AccessibleNode::new(id, AccessibleRole::Paragraph).with_name(text)
    }

    pub fn build_image(&mut self, alt_text: &str) -> AccessibleNode {
        let id = self.alloc_id();
        AccessibleNode::new(id, AccessibleRole::Image).with_alt_text(alt_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accessible_tree_generation() {
        let mut builder = ScreenReaderTreeBuilder::new();
        let mut doc = builder.build_document_root(Some("Sample Annual Report"));

        let h1 = builder.build_heading(1, "Executive Summary");
        let p1 = builder.build_paragraph("Revenue grew by 24% year over year.");
        let img = builder.build_image("Chart showing Q1-Q4 quarterly revenue growth");

        doc.add_child(h1);
        doc.add_child(p1);
        doc.add_child(img);

        assert_eq!(doc.children.len(), 3);
        assert_eq!(doc.children[0].role, AccessibleRole::Heading(1));
        assert_eq!(doc.children[2].description.as_deref(), Some("Chart showing Q1-Q4 quarterly revenue growth"));
    }
}
