/// What can go wrong when printing.
///
/// The three things a *person* can be wrong about — there is nothing to print, a page number past
/// the end, a range that is not a page — each have their own variant with the numbers apart from
/// the words, so an interface can say them in its own language with [`PrintError::said`]. The rest
/// are faults rather than mistakes and carry their text as it is.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PrintError {
    #[error("there are no pages to print")]
    NoPages,
    /// A page number past the end of the document: the number asked for, and the page count.
    #[error("page {0} is out of range (1–{1})")]
    OutOfRange(usize, usize),
    /// A range token that is neither a page number nor a page label.
    #[error("{0:?} is not a page number or label")]
    NotAPage(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Spool(String),
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
}

/// How an interface says one of our wordings in its own language, holes and all: given the English
/// in [`WORDINGS`], the same wording in that language. [`in_english`] keeps the English.
pub type Words<'a> = &'a dyn Fn(&str) -> String;

/// The [`Words`] that translate nothing: the wording as it is written here.
pub fn in_english(wording: &str) -> String {
    wording.to_owned()
}

/// What a person can get wrong about what to print, in English, with their holes still in them.
///
/// An interface that reports these in another language translates these three, which is the whole
/// reason they are gathered here; `wordings_are_said` keeps them in step with the errors.
pub const WORDINGS: &[&str] = &["there are no pages to print", "page {n} is out of range (1–{count})", "{page} is not a page number or label"];

impl PrintError {
    /// This error as one sentence, said in the language `words` translates into.
    ///
    /// The wording is translated and the numbers go in afterwards, in a single pass, so what the
    /// person typed cannot be read as a hole. With [`in_english`] this is the error's own
    /// `Display`, which is what the command line and scripts report.
    pub fn said(&self, words: Words) -> String {
        match self {
            PrintError::NoPages => words("there are no pages to print"),
            PrintError::OutOfRange(n, count) => {
                say(&words("page {n} is out of range (1–{count})"), &[("{n}", n.to_string()), ("{count}", count.to_string())])
            }
            // The token goes in quoted, the way it is typed, as the error itself prints it.
            PrintError::NotAPage(token) => say(&words("{page} is not a page number or label"), &[("{page}", format!("{token:?}"))]),
            fault => fault.to_string(),
        }
    }
}

/// A wording with its holes filled.
///
/// A hole is a name in braces, such as `{n}`; what is not in `values` is left as it is written.
/// Values go in during a single pass, so a value with braces in it is never read as a hole.
fn say(wording: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(wording.len() + 8);
    let mut rest = wording;
    while let Some(open) = rest.find('{') {
        let (before, from_brace) = rest.split_at(open);
        out.push_str(before);
        // `find` counts from the brace and `}` is one byte, so this is a char boundary.
        let Some(close) = from_brace.find('}') else {
            // A brace with nothing closing it is just text.
            out.push_str(from_brace);
            return out;
        };
        let Some(hole) = from_brace.get(..=close) else { return out + from_brace };
        match values.iter().find(|(name, _)| *name == hole) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(hole),
        }
        rest = from_brace.get(close + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}
