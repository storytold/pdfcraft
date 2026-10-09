//! pdfcraft-a11y — accessibility (L4): the Accessibility Checker's full check.
//!
//! [`check`] runs the 32 rules in Acrobat's seven categories (Document, Page Content, Forms,
//! Alternate Text, Tables, Lists, Headings) and returns, per rule, Passed, Failed, Needs manual
//! check or Skipped, with the elements or pages at fault. [`report_html`] writes the
//! accessibility report. Rules read the document only; the fixes that need edits (language,
//! title, tab order) are applied by the engine.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::BTreeSet;

use pdfcraft_cos::Document;

mod alt;
mod content;
mod report;
mod structure;

pub use alt::{AltError, Figure, figures, mark_decorative, set_alt};
pub use report::{REPORT_WORDS, report_html};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Document,
    PageContent,
    Forms,
    AlternateText,
    Tables,
    Lists,
    Headings,
}

impl Category {
    pub const ALL: [Category; 7] =
        [Category::Document, Category::PageContent, Category::Forms, Category::AlternateText, Category::Tables, Category::Lists, Category::Headings];

    pub fn label(self) -> &'static str {
        match self {
            Category::Document => "Document",
            Category::PageContent => "Page Content",
            Category::Forms => "Forms",
            Category::AlternateText => "Alternate Text",
            Category::Tables => "Tables",
            Category::Lists => "Lists",
            Category::Headings => "Headings",
        }
    }
}

/// The 32 rules of the full check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rule {
    PermissionFlag,
    ImageOnly,
    TaggedPdf,
    LogicalReadingOrder,
    PrimaryLanguage,
    Title,
    Bookmarks,
    ColorContrast,
    TaggedContent,
    TaggedAnnotations,
    TabOrder,
    CharacterEncoding,
    TaggedMultimedia,
    ScreenFlicker,
    Scripts,
    TimedResponses,
    NavigationLinks,
    TaggedFormFields,
    FieldDescriptions,
    FiguresAltText,
    NestedAltText,
    AltTextAssociated,
    AltTextHidesAnnotation,
    OtherElementsAltText,
    TableRows,
    TableCells,
    TableHeaders,
    TableRegularity,
    TableSummary,
    ListItems,
    LblLBody,
    HeadingNesting,
}

impl Rule {
    pub const ALL: [Rule; 32] = [
        Rule::PermissionFlag,
        Rule::ImageOnly,
        Rule::TaggedPdf,
        Rule::LogicalReadingOrder,
        Rule::PrimaryLanguage,
        Rule::Title,
        Rule::Bookmarks,
        Rule::ColorContrast,
        Rule::TaggedContent,
        Rule::TaggedAnnotations,
        Rule::TabOrder,
        Rule::CharacterEncoding,
        Rule::TaggedMultimedia,
        Rule::ScreenFlicker,
        Rule::Scripts,
        Rule::TimedResponses,
        Rule::NavigationLinks,
        Rule::TaggedFormFields,
        Rule::FieldDescriptions,
        Rule::FiguresAltText,
        Rule::NestedAltText,
        Rule::AltTextAssociated,
        Rule::AltTextHidesAnnotation,
        Rule::OtherElementsAltText,
        Rule::TableRows,
        Rule::TableCells,
        Rule::TableHeaders,
        Rule::TableRegularity,
        Rule::TableSummary,
        Rule::ListItems,
        Rule::LblLBody,
        Rule::HeadingNesting,
    ];

    /// The stable id used by automation (`permission-flag`, `tagged-content`, …).
    pub fn id(self) -> &'static str {
        self.info().0
    }

    pub fn category(self) -> Category {
        self.info().1
    }

    /// The short name shown in the results.
    pub fn name(self) -> &'static str {
        self.info().2
    }

    /// What the rule checks, as the options dialog words it.
    pub fn description(self) -> &'static str {
        self.info().3
    }

    /// Whether only a person can judge it.
    pub fn manual(self) -> bool {
        matches!(
            self,
            Rule::LogicalReadingOrder | Rule::ColorContrast | Rule::ScreenFlicker | Rule::Scripts | Rule::TimedResponses | Rule::NavigationLinks
        )
    }

    /// Checked unless deselected (Acrobat leaves colour contrast off).
    pub fn on_by_default(self) -> bool {
        self != Rule::ColorContrast
    }

    /// A short explanation of why the rule matters and how to satisfy it ("Explain").
    pub fn explanation(self) -> &'static str {
        self.info().4
    }

    pub fn from_id(id: &str) -> Option<Rule> {
        Rule::ALL.into_iter().find(|r| r.id() == id)
    }

    fn info(self) -> (&'static str, Category, &'static str, &'static str, &'static str) {
        use Category as C;
        match self {
            Rule::PermissionFlag => (
                "permission-flag",
                C::Document,
                "Accessibility permission flag",
                "Accessibility permission flag is set",
                "Security settings must allow assistive technology to read the content (\"Enable text access for screen reader devices\"). Change the document's security to allow it.",
            ),
            Rule::ImageOnly => (
                "image-only",
                C::Document,
                "Image-only PDF",
                "Document is not image-only PDF",
                "Pages that are only pictures of text can't be read aloud or searched. Recognize text (OCR) to add a text layer.",
            ),
            Rule::TaggedPdf => (
                "tagged-pdf",
                C::Document,
                "Tagged PDF",
                "Document is tagged PDF",
                "Tags give the document a logical structure that assistive technology follows. Tag the document (Automatically tag PDF).",
            ),
            Rule::LogicalReadingOrder => (
                "logical-reading-order",
                C::Document,
                "Logical Reading Order",
                "Document structure provides a logical reading order",
                "Check that the tags read in the order a sighted reader would. Use the Reading Order tool or the Tags panel to correct it.",
            ),
            Rule::PrimaryLanguage => (
                "primary-language",
                C::Document,
                "Primary language",
                "Text language is specified",
                "Screen readers choose their voice from the document's language. Set it in Document Properties > Advanced > Language.",
            ),
            Rule::Title => (
                "title",
                C::Document,
                "Title",
                "Document title is showing in title bar",
                "The window should show a meaningful title rather than the file name. Set a title and Initial View > Show: Document Title.",
            ),
            Rule::Bookmarks => (
                "bookmarks",
                C::Document,
                "Bookmarks",
                "Bookmarks are present in large documents",
                "Documents of more than 20 pages need bookmarks for navigation. Add bookmarks for the main sections.",
            ),
            Rule::ColorContrast => (
                "color-contrast",
                C::Document,
                "Color contrast",
                "Document has appropriate color contrast",
                "Text must contrast enough with its background for people with low vision. Check the colours by eye or with a contrast tool.",
            ),
            Rule::TaggedContent => (
                "tagged-content",
                C::PageContent,
                "Tagged content",
                "All page content is tagged",
                "Everything on a page must be tagged or marked as an artifact (decoration, headers, footers). Tag the untagged content.",
            ),
            Rule::TaggedAnnotations => (
                "tagged-annotations",
                C::PageContent,
                "Tagged annotations",
                "All annotations are tagged",
                "Comments, links and fields must be in the tag tree to be announced. Tag the annotations.",
            ),
            Rule::TabOrder => (
                "tab-order",
                C::PageContent,
                "Tab order",
                "Tab order is consistent with structure order",
                "Pages with links, comments or fields should tab in structure order. The fix sets each page's tab order to Use Document Structure.",
            ),
            Rule::CharacterEncoding => (
                "character-encoding",
                C::PageContent,
                "Character encoding",
                "Reliable character encoding is provided",
                "Fonts must map their characters to Unicode so text can be read and copied. Fix the fonts or recreate the PDF.",
            ),
            Rule::TaggedMultimedia => (
                "tagged-multimedia",
                C::PageContent,
                "Tagged multimedia",
                "All multimedia objects are tagged",
                "Video, sound and rich media must be tagged to be announced. Tag the multimedia annotations.",
            ),
            Rule::ScreenFlicker => (
                "screen-flicker",
                C::PageContent,
                "Screen flicker",
                "Page will not cause screen flicker",
                "Animation that flashes can cause seizures. Check scripts and media for flicker.",
            ),
            Rule::Scripts => (
                "scripts",
                C::PageContent,
                "Scripts",
                "No inaccessible scripts",
                "Scripted content must be usable with a keyboard and assistive technology. Check the document's scripts.",
            ),
            Rule::TimedResponses => (
                "timed-responses",
                C::PageContent,
                "Timed responses",
                "Page does not require timed responses",
                "People may need more time to respond. Check that nothing has a time limit.",
            ),
            Rule::NavigationLinks => (
                "navigation-links",
                C::PageContent,
                "Navigation links",
                "Navigation links are not repetitive",
                "Repeated navigation links should be skippable. Check links that appear on every page.",
            ),
            Rule::TaggedFormFields => (
                "tagged-form-fields",
                C::Forms,
                "Tagged form fields",
                "All form fields are tagged",
                "Form fields must be in the tag tree (Form tags) to be announced. Tag the form fields.",
            ),
            Rule::FieldDescriptions => (
                "field-descriptions",
                C::Forms,
                "Field descriptions",
                "All form fields have description",
                "Each field needs a tooltip that says what to enter. Add one in Field Properties > General > Tooltip.",
            ),
            Rule::FiguresAltText => (
                "figures-alt-text",
                C::AlternateText,
                "Figures alternate text",
                "Figures require alternate text",
                "Pictures need alternate text that describes them, or must be marked decorative. Add alternate text to each figure.",
            ),
            Rule::NestedAltText => (
                "nested-alt-text",
                C::AlternateText,
                "Nested alternate text",
                "Alternate text that will never be read",
                "Alternate text inside an element that already has alternate text is never read. Remove the inner alternate text.",
            ),
            Rule::AltTextAssociated => (
                "alt-text-associated",
                C::AlternateText,
                "Associated with content",
                "Alternate text must be associated with some content",
                "Alternate text on an element without page content is never reached. Remove it or attach the element to content.",
            ),
            Rule::AltTextHidesAnnotation => (
                "alt-text-hides-annotation",
                C::AlternateText,
                "Hides annotation",
                "Alternate text should not hide annotation",
                "Alternate text on an element that contains a link or field hides that annotation from assistive technology. Move the alternate text.",
            ),
            Rule::OtherElementsAltText => (
                "other-elements-alt-text",
                C::AlternateText,
                "Other elements alternate text",
                "Other elements that require alternate text",
                "Formulas and similar non-text elements need alternate text. Add alternate text to them.",
            ),
            Rule::TableRows => (
                "table-rows",
                C::Tables,
                "Rows",
                "TR must be a child of Table, THead, TBody, or TFoot",
                "Table rows belong directly in a table or its head, body or foot. Fix the table's tags.",
            ),
            Rule::TableCells => {
                ("table-cells", C::Tables, "TH and TD", "TH and TD must be children of TR", "Table cells belong in a row. Fix the table's tags.")
            }
            Rule::TableHeaders => (
                "table-headers",
                C::Tables,
                "Headers",
                "Tables should have headers",
                "Header cells (TH) tell listeners what each column or row holds. Mark the header cells.",
            ),
            Rule::TableRegularity => (
                "table-regularity",
                C::Tables,
                "Regularity",
                "Tables must contain the same number of columns in each row and rows in each column",
                "Irregular tables confuse cell navigation. Give every row the same number of cells (counting spans).",
            ),
            Rule::TableSummary => (
                "table-summary",
                C::Tables,
                "Summary",
                "Tables must have a summary",
                "A summary describes the table's purpose and layout. Add a Summary attribute to each table.",
            ),
            Rule::ListItems => ("list-items", C::Lists, "List items", "LI must be a child of L", "List items belong in a list. Fix the list's tags."),
            Rule::LblLBody => (
                "lbl-lbody",
                C::Lists,
                "Lbl and LBody",
                "Lbl and LBody must be children of LI",
                "List labels and bodies belong in a list item. Fix the list's tags.",
            ),
            Rule::HeadingNesting => (
                "heading-nesting",
                C::Headings,
                "Appropriate nesting",
                "Appropriate nesting",
                "Heading levels shouldn't skip (an H1 followed by an H3). Retag the headings.",
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    Passed,
    Failed,
    /// Needs manual check.
    Manual,
    /// Not checked (deselected or skipped).
    Skipped,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Passed => "Passed",
            Status::Failed => "Failed",
            Status::Manual => "Needs manual check",
            Status::Skipped => "Skipped",
        }
    }
}

/// One problem: where it is and what is wrong.
///
/// The sentence comes in two forms. [`message`](Finding::message) is it, in English, ready to read.
/// [`pattern`](Finding::pattern) is the same sentence with a named hole where each value went, and
/// [`values`](Finding::values) is what filled them, so an interface can say it in another language
/// instead of showing the English one: it translates the pattern and puts the same values back.
/// `message` is always `pattern` with `values` in it, because [`Finding::worded`] builds it.
///
/// Every hole holds the document's own data — an element type, a font name, a count — and goes in
/// as it stands, with one exception: a hole named `{what}` holds another wording of ours, still with
/// its own holes in it. An interface translates that wording too and puts it in the frame first, and
/// then fills every hole in what it has; [`Finding::nested`] builds such a finding.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    /// 0-based page, when the problem is on a page.
    pub page: Option<usize>,
    pub message: String,
    /// The English sentence with a hole, such as `{p}` or `{what}`, where each value went.
    pub pattern: &'static str,
    /// What goes in each of `pattern`'s holes, by the hole's name (`"{p}"`, braces included).
    pub values: Vec<(&'static str, String)>,
}

impl Finding {
    /// A finding whose sentence says the same thing however the document is put together.
    pub fn plain(page: Option<usize>, sentence: &'static str) -> Self {
        Finding { page, message: sentence.to_owned(), pattern: sentence, values: Vec::new() }
    }

    /// A finding built from a wording and what goes in its holes. The English `message` is filled
    /// in here, so it and `pattern` can never drift apart.
    pub fn worded(page: Option<usize>, pattern: &'static str, values: Vec<(&'static str, String)>) -> Self {
        Finding { page, message: say(pattern, &values), pattern, values }
    }

    /// A finding that says where the problem is and, in `{what}`, what is wrong, each worded
    /// separately so the two read in whichever order the language reads them.
    ///
    /// `what` is one of our own wordings and may have holes of its own; they are filled from
    /// `values` along with the frame's. It goes in before the values do — it is ours, not the
    /// document's — which is what lets its holes be filled at all.
    pub fn nested(page: Option<usize>, pattern: &'static str, what: &'static str, mut values: Vec<(&'static str, String)>) -> Self {
        let message = say(&pattern.replace("{what}", what), &values);
        values.push(("{what}", what.to_owned()));
        Finding { page, message, pattern, values }
    }

    /// This finding as one sentence, said in the language `words` translates into.
    ///
    /// The frame is translated, the wording in `{what}` — if the frame has one — is translated and
    /// goes in, and only then does the document's own data, in a single pass, so nothing the
    /// document itself says can be read as a hole. With `&|s: &str| s.to_owned()` this is
    /// [`message`](Finding::message).
    pub fn said(&self, words: Words) -> String {
        let mut frame = words(self.pattern);
        if let Some((_, what)) = self.values.iter().find(|(name, _)| *name == "{what}") {
            frame = frame.replace("{what}", &words(what));
        }
        say(&frame, &self.values)
    }
}

/// How an interface says one of our wordings in its own language, holes and all: given the English
/// in [`WORDINGS`], the same wording in that language. [`in_english`] keeps the English.
pub type Words<'a> = &'a dyn Fn(&str) -> String;

/// The [`Words`] that translate nothing: the wording as it is written here.
pub fn in_english(wording: &str) -> String {
    wording.to_owned()
}

/// A wording with its holes filled.
///
/// A hole is a name in braces, such as `{p}`; what is not in `values` is left as it is written.
/// Values go in during a single pass over the wording, so a value that itself contains braces is
/// never read as a hole: the document's own words reach the screen as they stand.
///
/// An interface says a [`Finding`] in its own language with this, after translating the pattern
/// and, if there is one, the wording in `{what}`.
pub fn say(pattern: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        let (before, from_brace) = rest.split_at(open);
        out.push_str(before);
        // `find` counts from the brace, and `}` is one byte, so this is a char boundary; `get`
        // rather than slicing in case it ever isn't.
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

/// Every sentence the checker can say: each [`Finding::pattern`] and each wording that goes in a
/// `{what}` hole, in English, with its holes still in it.
///
/// An interface that shows the report in another language translates these, which is the whole
/// reason they are gathered here; `wordings_are_complete` fails if a rule says something that is
/// not on the list.
pub const WORDINGS: &[&str] = &[
    // Document rules.
    "The security settings don't allow screen readers to read the text",
    "No page has text: the document appears to be images of pages",
    "The document has no tags (structure tree)",
    "The document isn't marked as tagged (MarkInfo Marked)",
    "No document language is set",
    "The document has no title",
    "The window shows the file name, not the title (Initial View > Show)",
    "{n} pages and no bookmarks",
    // Page content and forms.
    "The document isn't tagged, so none of its content is",
    "Page {p}: {n} untagged content item",
    "Page {p}: {n} untagged content items",
    "Form field on page {p} isn't tagged",
    "{} annotation on page {p} isn't tagged",
    "Page {p} doesn't tab in structure order",
    "Font {} on page {p} doesn't map its characters to Unicode",
    "Font (unnamed) on page {p} doesn't map its characters to Unicode",
    "Field \"{}\" on page {p} has no description (tooltip)",
    // The structure rules: the two frames, then what goes in their `{what}`.
    "{ty} element: {what}",
    "{ty} element on page {p}: {what}",
    "no alternate text",
    "its alternate text is inside another element's and will never be read",
    "alternate text on an element with no page content",
    "alternate text hides an annotation inside it",
    "not in a Table, THead, TBody or TFoot",
    "not in a TR",
    "no header cells (TH)",
    "rows have between {min} and {max} columns",
    "no summary",
    "not in an L (list)",
    "not in an LI (list item)",
    "the first heading is an H{n}; headings should not skip levels",
    "H{n} follows an H{d}; headings should not skip levels",
    // The summary at the end of a long list.
    AND_MORE,
];

#[derive(Clone, Debug, PartialEq)]
pub struct RuleResult {
    pub rule: Rule,
    pub status: Status,
    pub findings: Vec<Finding>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// The rules to run; the others are reported as Skipped.
    pub rules: BTreeSet<Rule>,
    /// 0-based pages for the page rules (`None`: all pages).
    pub pages: Option<Vec<usize>>,
}

impl Default for Options {
    fn default() -> Self {
        Self { rules: Rule::ALL.into_iter().filter(|r| r.on_by_default()).collect(), pages: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub results: Vec<RuleResult>,
}

impl Report {
    pub fn result(&self, rule: Rule) -> Option<&RuleResult> {
        self.results.iter().find(|r| r.rule == rule)
    }

    pub fn count(&self, status: Status) -> usize {
        self.results.iter().filter(|r| r.status == status).count()
    }

    /// Results of one category, in rule order.
    pub fn in_category(&self, c: Category) -> impl Iterator<Item = &RuleResult> {
        self.results.iter().filter(move |r| r.rule.category() == c)
    }
}

/// Findings beyond this many per rule are summarised ("… and n more").
const MAX_FINDINGS: usize = 200;

/// What the summary of the findings beyond [`MAX_FINDINGS`] says.
const AND_MORE: &str = "… and {n} more";

/// Run the full check.
pub fn check(doc: &Document, options: &Options) -> Report {
    let pages = pdfcraft_model::pages(doc);
    let page_list: Vec<usize> = match &options.pages {
        Some(p) => p.iter().copied().filter(|p| *p < pages.len()).collect(),
        None => (0..pages.len()).collect(),
    };
    let tree = structure::Tree::read(doc, &pages);
    let scan = content::Scan::run(doc, &pages, &page_list);
    let mut results = Vec::with_capacity(32);
    for rule in Rule::ALL {
        if !options.rules.contains(&rule) {
            results.push(RuleResult { rule, status: Status::Skipped, findings: Vec::new() });
            continue;
        }
        if rule.manual() {
            results.push(RuleResult { rule, status: Status::Manual, findings: Vec::new() });
            continue;
        }
        let mut findings = match rule.category() {
            Category::Document => content::document_rule(doc, rule, &pages, &scan),
            Category::PageContent | Category::Forms => content::page_rule(doc, rule, &pages, &page_list, &scan, &tree),
            _ => tree.rule(rule),
        };
        if findings.len() > MAX_FINDINGS {
            let more = findings.len() - MAX_FINDINGS;
            findings.truncate(MAX_FINDINGS);
            findings.push(Finding::worded(None, AND_MORE, vec![("{n}", more.to_string())]));
        }
        let status = if findings.is_empty() { Status::Passed } else { Status::Failed };
        results.push(RuleResult { rule, status, findings });
    }
    Report { results }
}

#[cfg(test)]
mod tests;
