//! Multilingual OCR Language Models and Script Classifiers.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OcrLanguage {
    English,
    German,
    French,
    Spanish,
    ChineseSimplified,
    ChineseTraditional,
    Japanese,
    Korean,
    Russian,
    Arabic,
}

impl OcrLanguage {
    pub fn model_code(&self) -> &'static str {
        match self {
            Self::English => "eng",
            Self::German => "deu",
            Self::French => "fra",
            Self::Spanish => "spa",
            Self::ChineseSimplified => "chi_sim",
            Self::ChineseTraditional => "chi_tra",
            Self::Japanese => "jpn",
            Self::Korean => "kor",
            Self::Russian => "rus",
            Self::Arabic => "ara",
        }
    }

    pub fn is_cjk(&self) -> bool {
        matches!(self, Self::ChineseSimplified | Self::ChineseTraditional | Self::Japanese | Self::Korean)
    }
}
