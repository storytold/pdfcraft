//! The paper size the system's region prints on: the size of a new blank document (Create ▸
//! Blank page).

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Paper {
    #[default]
    Letter,
    A4,
}

impl Paper {
    /// Width and height in points, portrait.
    pub fn points(self) -> (f64, f64) {
        match self {
            Paper::Letter => (612.0, 792.0),
            Paper::A4 => (595.28, 841.89),
        }
    }

    /// The system region's paper (craft-i18n reads it once per process).
    pub fn system() -> Paper {
        match craft::i18n::Region::system().paper {
            craft::i18n::Paper::A4 => Paper::A4,
            craft::i18n::Paper::Letter => Paper::Letter,
        }
    }
}
