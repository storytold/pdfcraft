//! Alternate text that is there but describes nothing (#7730): a file name, a generic word such as
//! "Picture 1", or a character or two. Tools write these when the author gave none, and a person
//! hearing them learns nothing about the picture: WCAG 2 failure F30, "text alternatives that are
//! not alternatives". The checker reports them as figures still needing alternate text.
//!
//! The generic words are English. Alternate text in another language is only caught when it is a
//! file name or very short.

/// What makes a figure's alternate text a placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placeholder {
    /// It is a file name: it ends in an image file extension, such as `IMG_2041.png`.
    FileName,
    /// It is a generic word, perhaps numbered, such as `Picture 1` or `image_003`.
    Generic,
    /// It is one or two characters (letters, digits or marks from the ASCII range).
    TooShort,
}

/// Image file extensions, lowercase, without the dot.
const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "jpe", "gif", "tif", "tiff", "bmp", "webp", "svg", "heic", "heif", "emf", "wmf"];

/// Generic words for a picture that applications write in place of a description.
const GENERIC: &[&str] = &["picture", "image", "graphic", "photo", "figure", "img", "pic", "graphics", "photograph"];

/// Whether `alt` is a placeholder rather than a description, and which kind.
pub(crate) fn placeholder(alt: &str) -> Option<Placeholder> {
    let alt = alt.trim();
    if alt.is_empty() {
        return None;
    }
    // Two characters of another script can be a whole word (地图, "map"), so only ASCII is short.
    if alt.chars().count() <= 2 && alt.is_ascii() {
        return Some(Placeholder::TooShort);
    }
    if is_file_name(alt) {
        return Some(Placeholder::FileName);
    }
    if is_generic(alt) {
        return Some(Placeholder::Generic);
    }
    None
}

/// Ends in `.ext` for an image extension, with a name right before the dot. A space there means
/// prose that mentions an extension (`converting .jpg to .png`), which is a description.
fn is_file_name(alt: &str) -> bool {
    let Some((stem, ext)) = alt.rsplit_once('.') else { return false };
    if stem.ends_with(char::is_whitespace) {
        return false;
    }
    let stem = stem.trim_end_matches(['/', '\\']);
    !stem.trim().is_empty() && EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e))
}

/// A generic word on its own, or followed by separators (any white space, `_`, `-`, `#`, `.`) and
/// a number (`Picture 1`, `image_003`, `Figure-2`). A word followed by anything else
/// (`Picture of the team`) is a description.
fn is_generic(alt: &str) -> bool {
    let end = alt.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(alt.len());
    let (word, rest) = alt.split_at(end);
    let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '_' | '-' | '#' | '.'));
    GENERIC.iter().any(|g| word.eq_ignore_ascii_case(g)) && rest.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_generic_words_and_short_text_are_placeholders() {
        // Extensions match in any case, after a path, with spaces trimmed.
        for alt in ["IMG_2041.png", "C:\\Users\\a\\scan 3.TIFF "] {
            assert_eq!(placeholder(alt), Some(Placeholder::FileName), "{alt:?}");
        }
        // A word alone, or numbered after a separator (a no-break space too) or none, in any case.
        for alt in ["picture", "Picture 1", "Picture\u{a0}1", "IMAGE_003", "img12"] {
            assert_eq!(placeholder(alt), Some(Placeholder::Generic), "{alt:?}");
        }
        for alt in ["  x  ", "ab"] {
            assert_eq!(placeholder(alt), Some(Placeholder::TooShort), "{alt:?}");
        }
    }

    #[test]
    fn descriptions_and_near_misses_are_not_placeholders() {
        // Each sits just past one of the rules above.
        for alt in [
            "   ",
            "abc",
            "Picture of the team at the 2024 offsite",
            "Figure 2: rainfall by month",
            "Photographs on a wall",
            "PNG export settings dialog",
            "Version 2.png of the logo",
            "Flowchart for converting .jpg to .png",
            ".png",
            "地图",
        ] {
            assert_eq!(placeholder(alt), None, "{alt:?}");
        }
    }
}
