//! AST Delta Patcher for In-Place PDF Content Stream Editing.
//! Preserves surrounding graphics states, transforms, and Marked Content (/MCID).

pub struct ContentDeltaPatcher;

impl ContentDeltaPatcher {
    /// Replaces targeted text occurrence inside a content stream without breaking marked content.
    pub fn patch_text(
        content_stream: &[u8],
        target_text: &str,
        replacement_text: &str,
    ) -> Vec<u8> {
        let stream_str = String::from_utf8_lossy(content_stream);

        // Pattern 1: (target_text) Tj
        let target_tj = format!("({}) Tj", target_text);
        let replace_tj = format!("({}) Tj", replacement_text);

        if stream_str.contains(&target_tj) {
            return stream_str.replace(&target_tj, &replace_tj).into_bytes();
        }

        // Pattern 2: [ (target_text) ] TJ
        let target_arr_tj = format!("({})", target_text);
        let replace_arr_tj = format!("({})", replacement_text);

        if stream_str.contains(&target_arr_tj) {
            return stream_str.replace(&target_arr_tj, &replace_arr_tj).into_bytes();
        }

        // Direct replacement fallback
        stream_str.replace(target_text, replacement_text).into_bytes()
    }

    /// Preserves `/Span << /MCID ... >> BDC ... EMC` while inserting updated text block.
    pub fn wrap_in_marked_content(text_block_ops: &str, mcid: u32, tag: &str) -> String {
        format!("/{} << /MCID {} >> BDC\n{}\nEMC\n", tag, mcid, text_block_ops)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_delta_patcher() {
        let stream = b"q 1 0 0 1 100 200 cm BT /F1 12 Tf (Old Text) Tj ET Q";
        let patched = ContentDeltaPatcher::patch_text(stream, "Old Text", "New Updated Text");
        let patched_str = String::from_utf8_lossy(&patched);
        assert!(patched_str.contains("(New Updated Text) Tj"));
        assert!(!patched_str.contains("Old Text"));
        assert!(patched_str.starts_with("q 1 0 0 1 100 200 cm BT"));
    }
}
