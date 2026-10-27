//! PDF open parameters on a path (#3425): help systems and other programs start a PDF reader with
//! `manual.pdf#nameddest=Intro` or `manual.pdf#page=12` to show one place in a document. The
//! parameters follow Adobe's public "Parameters for Opening PDF Files": `name=value` pairs after a
//! `#`, separated by `&` or `#`. PdfCraft reads `page` (1-based) and `nameddest`, and, as browsers'
//! PDF viewers do, a fragment without `=` as a destination name. Other parameters (`zoom`, `view`,
//! `pagemode`, `search`, …) are ignored, and the file still opens.

use pdfcraft_render::{DestView, RemoteDestination};

/// How many `#` of one path are tried as the end of its file name.
const MAX_CANDIDATES: usize = 32;

/// The file `path` names and the destination its open parameters ask for, when `path` is not a
/// file itself but what comes before one of its `#` is. The first such `#` counts, so a folder or
/// file name may contain `#` too. `None`: open `path` as it is.
pub(crate) fn split(path: &str, is_file: impl Fn(&str) -> bool) -> Option<(String, RemoteDestination)> {
    if is_file(path) {
        return None;
    }
    let (file, fragment) = path.match_indices('#').take(MAX_CANDIDATES).find_map(|(i, _)| {
        // `#` is one byte, so `i` and `i + 1` are char boundaries.
        let file = path.get(..i)?;
        is_file(file).then(|| (file, path.get(i + 1..).unwrap_or_default()))
    })?;
    Some((file.to_string(), destination(fragment)))
}

/// The destination of a fragment's parameters; the last `page` or `nameddest` wins, as each
/// parameter is applied in turn. A page before the first, or one that isn't a number, is ignored.
fn destination(fragment: &str) -> RemoteDestination {
    if !fragment.contains('=') {
        return if fragment.is_empty() { RemoteDestination::Default } else { RemoteDestination::Named(fragment.to_string()) };
    }
    let mut destination = RemoteDestination::Default;
    for (key, value) in fragment.split(['&', '#']).filter_map(|p| p.split_once('=')) {
        if key.eq_ignore_ascii_case("page") {
            if let Some(page) = value.trim().parse::<usize>().ok().and_then(|n| n.checked_sub(1)) {
                destination = RemoteDestination::Page { page, view: DestView::Top };
            }
        } else if key.eq_ignore_ascii_case("nameddest") && !value.is_empty() {
            destination = RemoteDestination::Named(value.to_string());
        }
    }
    destination
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(names: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |p| names.contains(&p)
    }

    fn page(page: usize) -> RemoteDestination {
        RemoteDestination::Page { page, view: DestView::Top }
    }

    fn named(name: &str) -> RemoteDestination {
        RemoteDestination::Named(name.to_string())
    }

    #[test]
    fn a_named_destination_or_page_after_the_file_name_is_split_off() {
        let manual = files(&[r"C:\Help\manual.pdf"]);
        // #3425: DIgSILENT PowerFactory's F1 help.
        assert_eq!(split(r"C:\Help\manual.pdf#nameddest=Load Flow", &manual), Some((r"C:\Help\manual.pdf".into(), named("Load Flow"))));
        assert_eq!(split(r"C:\Help\manual.pdf#page=12", &manual), Some((r"C:\Help\manual.pdf".into(), page(11))));
        // A bare fragment names a destination, as in a browser.
        assert_eq!(split(r"C:\Help\manual.pdf#intro", &manual), Some((r"C:\Help\manual.pdf".into(), named("intro"))));
    }

    #[test]
    fn several_parameters_apply_in_turn_and_unknown_ones_are_ignored() {
        let pdf = files(&["/docs/a.pdf"]);
        assert_eq!(split("/docs/a.pdf#page=3&zoom=200,0,0", &pdf), Some(("/docs/a.pdf".into(), page(2))));
        assert_eq!(split("/docs/a.pdf#zoom=50#nameddest=x", &pdf), Some(("/docs/a.pdf".into(), named("x"))));
        assert_eq!(split("/docs/a.pdf#nameddest=x&page=4", &pdf), Some(("/docs/a.pdf".into(), page(3))));
        assert_eq!(split("/docs/a.pdf#PAGE=2", &pdf), Some(("/docs/a.pdf".into(), page(1))));
        // The file opens at its default view when nothing can be applied.
        for odd in
            ["/docs/a.pdf#", "/docs/a.pdf#page=0", "/docs/a.pdf#page=two", "/docs/a.pdf#page=-1", "/docs/a.pdf#nameddest=", "/docs/a.pdf#view=Fit"]
        {
            assert_eq!(split(odd, &pdf), Some(("/docs/a.pdf".into(), RemoteDestination::Default)), "{odd}");
        }
        assert_eq!(split("/docs/a.pdf#page=99999999999999999999999", &pdf), Some(("/docs/a.pdf".into(), RemoteDestination::Default)));
    }

    #[test]
    fn a_path_that_names_a_file_is_left_alone() {
        // A file whose name has a `#` in it, alone and with parameters of its own.
        let hashed = files(&["/docs/C#.pdf", "/docs/#1/a.pdf"]);
        assert_eq!(split("/docs/C#.pdf", &hashed), None);
        assert_eq!(split("/docs/C#.pdf#page=2", &hashed), Some(("/docs/C#.pdf".into(), page(1))));
        assert_eq!(split("/docs/#1/a.pdf#page=2", &hashed), Some(("/docs/#1/a.pdf".into(), page(1))));
        // Nothing before a `#` is a file: the open fails on the path as given.
        assert_eq!(split("/docs/missing.pdf#page=2", &hashed), None);
        assert_eq!(split("/docs/plain.pdf", &hashed), None);
        assert_eq!(split("#", &hashed), None);
    }
}
