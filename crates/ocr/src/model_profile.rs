//! Fixed OCR model profiles shared by native and web callers. No paths, processes or downloads.

use super::{OcrError, is_turkish_language};

/// A fixed model selection. The extended profile is opt-in and never an automatic fallback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModelProfile {
    #[default]
    Standard,
    TurkishExtended,
}

// Frozen candidate from the controlled model comparison; root verifies final acceptance.
// Argv derives from this exact list: callers cannot supply raw model expressions.
const TURKISH_EXTENDED_PACKS: &[&str] = &["script/Latin"];

/// ISO language codes and their fixed Tesseract pack names (also exported by tesseract).
pub const LANGUAGE_CODES: &[(&str, &str)] = &[
    ("en", "eng"),
    ("de", "deu"),
    ("fr", "fra"),
    ("es", "spa"),
    ("it", "ita"),
    ("pt", "por"),
    ("ru", "rus"),
    ("zh", "chi_sim"),
    ("ja", "jpn"),
    ("ko", "kor"),
    ("ar", "ara"),
    ("nl", "nld"),
    ("tr", "tur"),
];

fn standard_pack(language: &str) -> &'static str {
    if is_turkish_language(language) {
        return "tur";
    }
    let code = language.trim();
    LANGUAGE_CODES.iter().find(|(iso, _)| iso.eq_ignore_ascii_case(code)).map(|(_, pack)| *pack).unwrap_or("eng")
}

impl ModelProfile {
    pub fn id(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::TurkishExtended => "turkish_extended",
        }
    }

    /// Only fixed public ids are accepted, never arbitrary model names or command arguments.
    pub fn parse(value: &str) -> Result<Self, OcrError> {
        match value {
            "standard" => Ok(Self::Standard),
            "turkish_extended" => Ok(Self::TurkishExtended),
            _ => Err(OcrError::Recognize("unknown OCR model profile; choose standard or turkish_extended".into())),
        }
    }

    pub fn validate(self, language: &str) -> Result<(), OcrError> {
        match self {
            Self::TurkishExtended if !is_turkish_language(language) => {
                Err(OcrError::Recognize("turkish_extended supports only Turkish; choose tr or tur and the Tesseract engine".into()))
            }
            _ => Ok(()),
        }
    }

    /// Pack ids are fixed, even for a caller-supplied language. This also works on wasm32.
    pub fn required_packs(self, language: &str) -> Result<Vec<&'static str>, OcrError> {
        self.validate(language)?;
        match self {
            Self::Standard => Ok(vec![standard_pack(language)]),
            Self::TurkishExtended => Ok(TURKISH_EXTENDED_PACKS.to_vec()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Line, OcrImage, RecognizeOptions, Recognizer};

    #[test]
    fn profiles_are_fixed_and_standard_is_the_default() {
        assert_eq!(ModelProfile::default(), ModelProfile::Standard);
        assert_eq!(RecognizeOptions::default().model_profile, ModelProfile::Standard);
        for profile in [ModelProfile::Standard, ModelProfile::TurkishExtended] {
            assert_eq!(ModelProfile::parse(profile.id()).unwrap(), profile);
        }
        for invalid in ["", "Standard", " turkish_extended ", "script/Latin", "tur+eng", "--psm 7", "standard\0"] {
            assert!(ModelProfile::parse(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn extended_accepts_turkish_aliases_but_never_raw_model_expressions() {
        for language in ["tr", " TR ", "tr-TR", "tr_CY.UTF-8", "tur"] {
            assert!(ModelProfile::TurkishExtended.validate(language).is_ok());
            assert_eq!(ModelProfile::TurkishExtended.required_packs(language).unwrap(), ["script/Latin"]);
            assert_eq!(ModelProfile::TurkishExtended.required_packs(language).unwrap().join("+"), "script/Latin");
            assert_eq!(ModelProfile::Standard.required_packs(language).unwrap(), ["tur"]);
        }
        for language in ["", "en", "de", "eng", "tur+eng", "script/Latin", "tr+--psm 7"] {
            assert!(ModelProfile::TurkishExtended.required_packs(language).is_err(), "{language:?}");
        }
        for language in ["", "unknown", "eng+tur", "--list-langs"] {
            assert_eq!(ModelProfile::Standard.required_packs(language).unwrap(), ["eng"], "standard keeps its documented fallback: {language:?}");
        }
        for (iso, pack) in LANGUAGE_CODES {
            assert_eq!(ModelProfile::Standard.required_packs(iso).unwrap(), [*pack]);
        }
    }

    #[test]
    fn an_existing_recognizer_implementation_gets_standard_without_a_new_method() {
        struct Existing;
        impl Recognizer for Existing {
            fn id(&self) -> &'static str {
                "existing"
            }
            fn available(&self) -> bool {
                false
            }
            fn languages(&self) -> Vec<(String, String)> {
                Vec::new()
            }
            fn recognize(&self, _: &OcrImage, _: &RecognizeOptions) -> Result<Vec<Line>, OcrError> {
                Err(OcrError::NoModels)
            }
        }
        let recognizer: std::sync::Arc<dyn Recognizer> = std::sync::Arc::new(Existing);
        assert_eq!(recognizer.model_profiles(), [ModelProfile::Standard]);
    }
}
