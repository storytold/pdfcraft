//! Bates Numbering Engine for Legal Document Production.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatesPosition {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

#[derive(Debug, Clone)]
pub struct BatesConfig {
    pub prefix: String,
    pub suffix: String,
    pub start_number: u32,
    pub num_digits: usize, // e.g. 6 digits -> "000123"
    pub position: BatesPosition,
    pub font_size: f32,
}

impl Default for BatesConfig {
    fn default() -> Self {
        Self {
            prefix: "CONFIDENTIAL-".into(),
            suffix: "".into(),
            start_number: 1,
            num_digits: 6,
            position: BatesPosition::BottomRight,
            font_size: 10.0,
        }
    }
}

pub struct BatesNumberer;

impl BatesNumberer {
    /// Formats the Bates stamp string for a given sequential index.
    pub fn format_stamp(config: &BatesConfig, sequence_index: u32) -> String {
        let number = config.start_number + sequence_index;
        format!(
            "{}{:0width$}{}",
            config.prefix,
            number,
            config.suffix,
            width = config.num_digits
        )
    }

    /// Generates content stream overlay bytes for stamping a page.
    pub fn generate_overlay_stream(
        config: &BatesConfig,
        sequence_index: u32,
        page_width: f32,
        page_height: f32,
    ) -> Vec<u8> {
        let text = Self::format_stamp(config, sequence_index);
        let margin = 36.0f32; // 0.5 inch margin

        let (x, y) = match config.position {
            BatesPosition::TopLeft => (margin, page_height - margin),
            BatesPosition::TopCenter => (page_width / 2.0 - 50.0, page_height - margin),
            BatesPosition::TopRight => (page_width - margin - 100.0, page_height - margin),
            BatesPosition::BottomLeft => (margin, margin),
            BatesPosition::BottomCenter => (page_width / 2.0 - 50.0, margin),
            BatesPosition::BottomRight => (page_width - margin - 100.0, margin),
        };

        format!(
            "q 0 0 0 rg BT /Helv {:.2} Tf {:.2} {:.2} Td ({}) Tj ET Q\n",
            config.font_size, x, y, text
        )
        .into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bates_formatting() {
        let config = BatesConfig {
            prefix: "ABC-".into(),
            suffix: "-XYZ".into(),
            start_number: 42,
            num_digits: 6,
            position: BatesPosition::BottomRight,
            font_size: 9.0,
        };

        let stamp = BatesNumberer::format_stamp(&config, 0);
        assert_eq!(stamp, "ABC-000042-XYZ");

        let stamp2 = BatesNumberer::format_stamp(&config, 10);
        assert_eq!(stamp2, "ABC-000052-XYZ");
    }
}
