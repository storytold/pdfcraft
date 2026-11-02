//! Explicit document-font choice, never a process-global interface preference.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SyriacFamily {
    #[default]
    Estrangela,
    Eastern,
    Western,
}

impl SyriacFamily {
    pub const ALL: [Self; 3] = [Self::Estrangela, Self::Eastern, Self::Western];

    pub fn name(self) -> &'static str {
        match self {
            Self::Estrangela => "estrangela",
            Self::Eastern => "eastern",
            Self::Western => "western",
        }
    }

    pub fn font_family(self) -> &'static str {
        match self {
            Self::Estrangela => "Noto Sans Syriac",
            Self::Eastern => "Noto Sans Syriac Eastern",
            Self::Western => "Noto Sans Syriac Western",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|family| family.name().eq_ignore_ascii_case(name) || family.font_family().eq_ignore_ascii_case(name))
            .or_else(|| name.eq_ignore_ascii_case("serto").then_some(Self::Western))
    }

    /// Older PDFs have no preference; malformed private metadata falls back leniently.
    pub fn from_dict(dict: &pdfcraft_cos::Dict) -> Self {
        dict.name(Self::PDF_KEY).and_then(|name| std::str::from_utf8(name).ok()).and_then(Self::from_name).unwrap_or_default()
    }

    pub const PDF_KEY: &'static [u8] = b"PCSyriacFamily";
}
