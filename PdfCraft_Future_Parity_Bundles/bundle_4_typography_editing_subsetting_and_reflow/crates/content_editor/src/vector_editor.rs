//! Vector Path Manipulation (Translate, Scale, Recolor, Delete).

#[derive(Debug, Clone)]
pub struct VectorElement {
    pub id: usize,
    pub path_ops: String,
    pub stroke_color: Option<[f32; 3]>,
    pub fill_color: Option<[f32; 3]>,
    pub line_width: f32,
}

pub struct VectorPathEditor {
    pub elements: Vec<VectorElement>,
}

impl Default for VectorPathEditor {
    fn default() -> Self {
        Self { elements: Vec::new() }
    }
}

impl VectorPathEditor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_element(&mut self, element: VectorElement) {
        self.elements.push(element);
    }

    /// Recolor vector stroke.
    pub fn recolor_stroke(&mut self, id: usize, rgb: [f32; 3]) -> bool {
        if let Some(el) = self.elements.iter_mut().find(|e| e.id == id) {
            el.stroke_color = Some(rgb);
            true
        } else {
            false
        }
    }

    /// Recolor vector fill.
    pub fn recolor_fill(&mut self, id: usize, rgb: [f32; 3]) -> bool {
        if let Some(el) = self.elements.iter_mut().find(|e| e.id == id) {
            el.fill_color = Some(rgb);
            true
        } else {
            false
        }
    }

    /// Delete vector element.
    pub fn delete_element(&mut self, id: usize) -> bool {
        let initial_len = self.elements.len();
        self.elements.retain(|e| e.id != id);
        self.elements.len() < initial_len
    }

    /// Serialize updated vector elements back into content stream.
    pub fn serialize_to_stream(&self) -> Vec<u8> {
        let mut stream = String::new();
        for el in &self.elements {
            stream.push_str("q\n");
            if let Some(s) = el.stroke_color {
                stream.push_str(&format!("{:.3} {:.3} {:.3} RG\n", s[0], s[1], s[2]));
            }
            if let Some(f) = el.fill_color {
                stream.push_str(&format!("{:.3} {:.3} {:.3} rg\n", f[0], f[1], f[2]));
            }
            stream.push_str(&format!("{:.2} w\n", el.line_width));
            stream.push_str(&el.path_ops);
            stream.push_str("\nQ\n");
        }
        stream.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_path_manipulation() {
        let mut editor = VectorPathEditor::new();
        editor.add_element(VectorElement {
            id: 1,
            path_ops: "0 0 100 100 re B".into(),
            stroke_color: Some([0.0, 0.0, 0.0]),
            fill_color: Some([1.0, 0.0, 0.0]),
            line_width: 1.0,
        });

        // Recolor fill to blue
        assert!(editor.recolor_fill(1, [0.0, 0.0, 1.0]));
        let out = editor.serialize_to_stream();
        let out_str = String::from_utf8_lossy(&out);
        assert!(out_str.contains("0.000 0.000 1.000 rg"));
    }
}
