//! Bi-directional Text Classification (LTR, RTL, CJK).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDirection {
    LeftToRight,
    RightToLeft,
}

#[derive(Debug, Clone)]
pub struct TextRun {
    pub text: String,
    pub direction: TextDirection,
    pub is_cjk: bool,
}

pub struct BidiClassifier;

impl BidiClassifier {
    /// Classifies character into LTR, RTL, or Neutral.
    pub fn is_rtl_char(ch: char) -> bool {
        let code = ch as u32;
        // Hebrew (0590-05FF), Arabic (0600-06FF), Arabic Supplement (0750-077F)
        (0x0590..=0x05FF).contains(&code) || (0x0600..=0x077F).contains(&code)
    }

    pub fn is_cjk_char(ch: char) -> bool {
        let code = ch as u32;
        // CJK Unified Ideographs, Hiragana, Katakana, Hangul
        (0x4E00..=0x9FFF).contains(&code)
            || (0x3040..=0x309F).contains(&code)
            || (0x30A0..=0x30FF).contains(&code)
            || (0xAC00..=0xD7AF).contains(&code)
    }

    /// Segments text into directional runs.
    pub fn segment_runs(text: &str) -> Vec<TextRun> {
        let mut runs = Vec::new();
        if text.is_empty() {
            return runs;
        }

        let mut current_text = String::new();
        let mut current_dir: Option<TextDirection> = None;
        let mut current_is_cjk = false;

        for ch in text.chars() {
            let is_rtl = Self::is_rtl_char(ch);
            let is_cjk = Self::is_cjk_char(ch);
            let dir = if is_rtl { TextDirection::RightToLeft } else { TextDirection::LeftToRight };

            match current_dir {
                None => {
                    current_dir = Some(dir);
                    current_is_cjk = is_cjk;
                    current_text.push(ch);
                }
                Some(d) if d == dir && current_is_cjk == is_cjk => {
                    current_text.push(ch);
                }
                Some(_) => {
                    runs.push(TextRun {
                        text: std::mem::take(&mut current_text),
                        direction: current_dir.unwrap(),
                        is_cjk: current_is_cjk,
                    });
                    current_dir = Some(dir);
                    current_is_cjk = is_cjk;
                    current_text.push(ch);
                }
            }
        }

        if !current_text.is_empty() && current_dir.is_some() {
            runs.push(TextRun {
                text: current_text,
                direction: current_dir.unwrap(),
                is_cjk: current_is_cjk,
            });
        }

        runs
    }
}
