//! Web and email addresses written as plain page text, with no link annotation over them (#4207).
//!
//! The viewer makes them clickable the way link annotations are: the address opens only through
//! [`crate::links::document_url`] and the confirmation, like any link from a document. No clause
//! of ISO 32000-2 covers this (it defines link annotations, not reading addresses out of page
//! text); the detector is the one Create links from URLs uses ([`pdfcraft_annot::links::find_urls`]).

use pdfcraft_render::{PageInfo, PageText};

/// Pages with more glyphs than this aren't searched for addresses: the work is linear, but it is
/// done for each page as it comes into view.
pub const MAX_TEXT_LINK_GLYPHS: usize = 200_000;

/// At most this many addresses are found on one page.
pub const MAX_TEXT_LINKS_PER_PAGE: usize = 512;

/// An address written in a page's text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLink {
    /// Where it is written: one rectangle per line, in view space (points from the displayed
    /// page's top-left, y down, the space of [`PageText`]).
    pub rects: Vec<[f32; 4]>,
    /// The address it opens: as written for `http://` and `https://`, `https://` added for
    /// `www.`, and `mailto:` for an email address. Not yet checked: the opener must
    /// ([`crate::links::document_url`]).
    pub uri: String,
}

impl TextLink {
    /// Whether the view-space point `(x, y)` is on the address.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.rects.iter().any(|r| x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3])
    }

    /// Whether a link annotation with the user-space rectangle `rect` (normalized) lies over part
    /// of the address on `page`: the annotation is what the document asks for there.
    pub fn covered_by(&self, page: &PageInfo, rect: [f32; 4]) -> bool {
        let (a, b) = (page.user_to_view(rect[0], rect[1]), page.user_to_view(rect[2], rect[3]));
        let v = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
        self.rects.iter().any(|r| r[0] < v[2] && v[0] < r[2] && r[1] < v[3] && v[1] < r[3])
    }
}

/// The web addresses (`http://`, `https://`, `www.`) and email addresses written in `text`, in
/// reading order. Punctuation that ends a sentence (`.`, `,`, `;`, `:`, `!`, `?`) and brackets
/// around an address are not part of it. Nothing is found on a page of more than
/// [`MAX_TEXT_LINK_GLYPHS`] glyphs, and at most [`MAX_TEXT_LINKS_PER_PAGE`] addresses are kept. An
/// address that starts or ends inside a glyph carrying several characters is left out.
pub fn find_text_links(text: &PageText) -> Vec<TextLink> {
    if text.glyphs.len() > MAX_TEXT_LINK_GLYPHS {
        return Vec::new();
    }
    // Each address with the text it was found in.
    let found: std::cell::RefCell<Vec<(Option<String>, String)>> = Default::default();
    let hits = text.find_with(|chars| {
        let mut urls = pdfcraft_annot::links::find_urls(chars);
        urls.truncate(MAX_TEXT_LINKS_PER_PAGE);
        let ranges = urls.iter().map(|u| u.0.clone()).collect();
        *found.borrow_mut() =
            urls.into_iter().map(|(range, uri)| (chars.get(range.clone()).map(|c| c.iter().collect()), web_uri(chars, range, uri))).collect();
        ranges
    });
    let found = found.into_inner();
    // `find_with` drops a range it can't place on glyphs; then the two lists no longer pair up,
    // and nothing is offered rather than an address on the wrong text.
    if hits.len() != found.len() {
        return Vec::new();
    }
    hits.into_iter()
        .zip(found)
        // A glyph that carries more than one character (a ligature, or a font that maps a glyph to
        // `m,`) is taken whole: an address that starts or ends inside one would make the text next
        // to it clickable, so it is left out.
        .filter(|(glyphs, (written, _))| written.as_deref() == Some(text.text_of(glyphs.clone()).as_str()))
        .map(|(glyphs, (_, uri))| TextLink { rects: text.line_rects(glyphs), uri })
        .filter(|l| !l.rects.is_empty())
        .collect()
}

/// A `www.` address opens over `https://`: `find_urls`, which writes links into the document,
/// keeps `http://` for them, but a reader following an address it was never given should use the
/// secure scheme.
fn web_uri(chars: &[char], range: std::ops::Range<usize>, uri: String) -> String {
    let www = "www.".chars().enumerate().all(|(k, w)| chars.get(range.start.saturating_add(k)).is_some_and(|c| c.eq_ignore_ascii_case(&w)));
    match uri.strip_prefix("http://") {
        Some(rest) if www => format!("https://{rest}"),
        _ => uri,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_render::TextGlyph;

    /// One line of text, a glyph per character, 6 points apart; `\n` starts the next line.
    fn page_text(s: &str) -> PageText {
        let mut glyphs = Vec::new();
        let (mut line, mut x) = (0u32, 0.0f32);
        let mut line_of = Vec::new();
        let mut space_before = Vec::new();
        let mut gap = false;
        for c in s.chars() {
            if c == '\n' {
                (line, x, gap) = (line + 1, 0.0, false);
                continue;
            }
            if c == ' ' {
                (x, gap) = (x + 6.0, true);
                continue;
            }
            let y = 10.0 + line as f32 * 20.0;
            let rect = [x, y, x + 6.0, y + 12.0];
            glyphs.push(TextGlyph {
                text: c.to_string(),
                rect,
                quad: [[x, y + 12.0], [x + 6.0, y + 12.0], [x + 6.0, y], [x, y]],
                direction: [1.0, 0.0],
            });
            line_of.push(line);
            space_before.push(gap);
            (x, gap) = (x + 6.0, false);
        }
        PageText { glyphs, line_of, space_before }
    }

    fn uris(s: &str) -> Vec<String> {
        find_text_links(&page_text(s)).into_iter().map(|l| l.uri).collect()
    }

    #[test]
    fn web_and_email_addresses_in_text_are_found() {
        assert_eq!(uris("Visit https://example.com/plain-text-url today"), ["https://example.com/plain-text-url"]);
        assert_eq!(uris("or http://example.org/a?b=c#d"), ["http://example.org/a?b=c#d"]);
        assert_eq!(uris("Go to www.example.com/no-scheme now"), ["https://www.example.com/no-scheme"]);
        assert_eq!(uris("Go to WWW.Example.com"), ["https://WWW.Example.com"]);
        assert_eq!(uris("Write to someone@example.com please"), ["mailto:someone@example.com"]);
        assert_eq!(
            uris("a https://a.example\nb www.b.example\nc c@example.org"),
            ["https://a.example", "https://www.b.example", "mailto:c@example.org"]
        );
    }

    #[test]
    fn sentence_punctuation_and_brackets_are_not_part_of_the_address() {
        assert_eq!(uris("See https://example.com."), ["https://example.com"]);
        assert_eq!(uris("See https://example.com/x, then"), ["https://example.com/x"]);
        assert_eq!(uris("(see https://example.com/x)"), ["https://example.com/x"]);
        assert_eq!(uris("See www.example.com!"), ["https://www.example.com"]);
        assert_eq!(uris("Mail someone@example.com."), ["mailto:someone@example.com"]);
        assert_eq!(uris("Ask (someone@example.com)."), ["mailto:someone@example.com"]);
    }

    #[test]
    fn text_that_only_looks_like_an_address_is_not_found() {
        for s in ["example.com", "http://", "https://x", "www.", "user@localhost", "a@b.c", "notwww.example.com", "Version 1.2.3", "@example.com"] {
            assert_eq!(uris(s), Vec::<String>::new(), "{s}");
        }
    }

    #[test]
    fn an_address_is_found_where_it_is_written() {
        let links = find_text_links(&page_text("ab https://example.com cd"));
        // "ab " is 3 characters of 6 points; the address is 19 characters on the first line.
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].rects, vec![[18.0, 10.0, 132.0, 22.0]]);
        assert!(links[0].contains(20.0, 15.0) && links[0].contains(131.0, 21.0));
        assert!(!links[0].contains(10.0, 15.0), "the text before it");
        assert!(!links[0].contains(140.0, 15.0), "the text after it");
        assert!(!links[0].contains(20.0, 30.0), "the line below");
    }

    #[test]
    fn a_link_annotation_over_the_address_covers_it() {
        let page = PageInfo { width: 300.0, height: 300.0, label: String::new(), crop: [0.0, 0.0, 300.0, 300.0], rotation: 0 };
        let link = &find_text_links(&page_text("ab https://example.com cd"))[0];
        // View y 10–22 is user y 278–290.
        assert!(link.covered_by(&page, [100.0, 280.0, 120.0, 285.0]));
        assert!(!link.covered_by(&page, [100.0, 200.0, 120.0, 250.0]), "a link elsewhere on the page");
        assert!(!link.covered_by(&page, [140.0, 280.0, 160.0, 285.0]), "a link after it on the line");
    }

    /// `text` with glyph `i` and the one after it drawn as one glyph carrying both characters.
    fn merged(mut text: PageText, i: usize) -> PageText {
        let next = text.glyphs.remove(i + 1);
        text.line_of.remove(i + 1);
        text.space_before.remove(i + 1);
        let g = &mut text.glyphs[i];
        g.text.push_str(&next.text);
        g.rect[2] = next.rect[2];
        text
    }

    #[test]
    fn an_address_ending_inside_a_glyph_is_not_offered() {
        // "m," is one glyph: the address ends on its first character, and its rectangle would
        // take the comma too.
        let s = "See https://example.com, then";
        // Glyph indexes skip the one space before the address.
        let m = s.find(',').expect("a comma") - 2;
        assert!(find_text_links(&merged(page_text(s), m)).is_empty());
        // A glyph of several characters inside the address is part of it.
        let links = find_text_links(&merged(page_text("See https://fi.example.com"), 11));
        assert_eq!(links.iter().map(|l| l.uri.as_str()).collect::<Vec<_>>(), ["https://fi.example.com"]);
    }

    #[test]
    fn work_per_page_is_capped() {
        let many = "a@example.org ".repeat(MAX_TEXT_LINKS_PER_PAGE + 5);
        assert_eq!(find_text_links(&page_text(&many)).len(), MAX_TEXT_LINKS_PER_PAGE);
        let mut big = page_text("https://example.com");
        let filler = big.glyphs[0].clone();
        big.glyphs.resize(MAX_TEXT_LINK_GLYPHS + 1, filler);
        big.line_of.resize(MAX_TEXT_LINK_GLYPHS + 1, 0);
        big.space_before.resize(MAX_TEXT_LINK_GLYPHS + 1, false);
        assert!(find_text_links(&big).is_empty());
        assert!(find_text_links(&PageText::default()).is_empty());
    }
}
