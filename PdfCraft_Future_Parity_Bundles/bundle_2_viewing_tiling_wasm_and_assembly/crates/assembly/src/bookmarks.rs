//! Bookmark Hierarchy and Destination Action Execution (ISO 32000-1 §12.3.2).

#[derive(Debug, Clone, PartialEq)]
pub enum DestinationFit {
    Xyz { left: Option<f32>, top: Option<f32>, zoom: Option<f32> },
    FitPage,
    FitHorizontal { top: f32 },
    FitVertical { left: f32 },
    FitRect { left: f32, bottom: f32, right: f32, top: f32 },
}

#[derive(Debug, Clone)]
pub struct BookmarkDestination {
    pub target_page_index: usize,
    pub fit_mode: DestinationFit,
}

#[derive(Debug, Clone)]
pub struct BookmarkNode {
    pub title: String,
    pub color_rgb: [f32; 3], // Default [0.0, 0.0, 0.0]
    pub is_italic: bool,
    pub is_bold: bool,
    pub destination: Option<BookmarkDestination>,
    pub children: Vec<BookmarkNode>,
}

impl BookmarkNode {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            color_rgb: [0.0, 0.0, 0.0],
            is_italic: false,
            is_bold: false,
            destination: None,
            children: Vec::new(),
        }
    }

    pub fn with_destination(mut self, dest: BookmarkDestination) -> Self {
        self.destination = Some(dest);
        self
    }

    pub fn with_style(mut self, bold: bool, italic: bool) -> Self {
        self.is_bold = bold;
        self.is_italic = italic;
        self
    }

    pub fn add_child(&mut self, child: BookmarkNode) {
        self.children.push(child);
    }
}

pub struct BookmarkTree {
    pub root_items: Vec<BookmarkNode>,
}

impl Default for BookmarkTree {
    fn default() -> Self {
        Self { root_items: Vec::new() }
    }
}

impl BookmarkTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_item(&mut self, item: BookmarkNode) {
        self.root_items.push(item);
    }

    /// Recursively counts total bookmarks in tree.
    pub fn total_count(&self) -> usize {
        fn count_node(node: &BookmarkNode) -> usize {
            1 + node.children.iter().map(count_node).sum::<usize>()
        }
        self.root_items.iter().map(count_node).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bookmark_tree_creation() {
        let mut tree = BookmarkTree::new();
        let mut chapter1 = BookmarkNode::new("Chapter 1: Introduction")
            .with_style(true, false)
            .with_destination(BookmarkDestination {
                target_page_index: 0,
                fit_mode: DestinationFit::FitPage,
            });

        let sec1 = BookmarkNode::new("1.1 Background")
            .with_destination(BookmarkDestination {
                target_page_index: 2,
                fit_mode: DestinationFit::FitHorizontal { top: 500.0 },
            });

        chapter1.add_child(sec1);
        tree.add_item(chapter1);

        assert_eq!(tree.total_count(), 2);
        assert_eq!(tree.root_items[0].is_bold, true);
    }
}
