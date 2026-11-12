//! PDF portfolios (`/Collection`, ISO 32000-2 §7.11.6): the attachment a portfolio presents
//! first, and how its file list first shows.

use super::{Arc, Portfolio, PortfolioView, inspect};

/// A one-page portfolio of `a.txt` and `b.pdf`, keyed in UTF-16 as portfolio writers key them,
/// with `collection` as its `/Collection` entries (`None`: not a portfolio).
fn portfolio(collection: Option<&str>) -> Vec<u8> {
    let collection = collection.map(|c| format!("/Collection << /Type /Collection {c} >>")).unwrap_or_default();
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R {collection} /Names << /EmbeddedFiles << /Names [<FEFF0061002E007400780074> 5 0 R <FEFF0062002E007000640066> 7 0 R] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
4 0 obj << /Type /EmbeddedFile /Length 5 >> stream
hello
endstream endobj
5 0 obj << /Type /Filespec /F (a.txt) /UF (a.txt) /EF << /F 4 0 R >> >> endobj
6 0 obj << /Type /EmbeddedFile /Length 8 >> stream
%PDF-1.7
endstream endobj
7 0 obj << /Type /Filespec /F (b.pdf) /UF (b.pdf) /EF << /F 6 0 R >> >> endobj
trailer << /Root 1 0 R >>
%%EOF"
    )
    .into_bytes()
}

/// The portfolio [`portfolio`] with `collection` reads as.
fn read(collection: Option<&str>) -> Option<Portfolio> {
    inspect(Arc::new(portfolio(collection)), None).expect("opens").portfolio
}

/// `/D` picks the attachment presented first, matched as the keys decode, and `/View` how the
/// list first shows.
#[test]
fn a_portfolio_presents_the_attachment_its_d_names() {
    // `/D` names `b.pdf` in the keys' own UTF-16, then as a literal string: both decode as the
    // keys do.
    let tile = Some(Portfolio { initial: Some(1), view: PortfolioView::Tile });
    assert_eq!(read(Some("/D <FEFF0062002E007000640066> /View /T")), tile);
    assert_eq!(read(Some("/D (b.pdf) /View /T")), tile);
    // Without `/D` the portfolio's own pages come first, and a `/D` naming no file presents none.
    assert_eq!(read(Some("")), Some(Portfolio { initial: None, view: PortfolioView::Details }));
    assert_eq!(read(Some("/D (c.pdf) /View /D")), Some(Portfolio { initial: None, view: PortfolioView::Details }));
    // `/View /H` keeps the file list hidden; `/C` is a custom navigator; an unknown view is
    // the default, details.
    assert_eq!(read(Some("/D (a.txt) /View /H")), Some(Portfolio { initial: Some(0), view: PortfolioView::Hidden }));
    assert_eq!(read(Some("/View /C")).map(|p| p.view), Some(PortfolioView::Custom));
    assert_eq!(read(Some("/View /Bogus /D 7")), Some(Portfolio { initial: None, view: PortfolioView::Details }));
    // The same attachments without a `/Collection` are no portfolio.
    assert_eq!(read(None), None);
}
