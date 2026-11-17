//! Where a document's own metadata is written: the one place a save and a Document Properties
//! change ask.
//!
//! A PDF states its title, author, dates and so on in the document information dictionary
//! (`/Info`, ISO 32000-2 §14.3.3), and in the catalog's XMP packet (§14.3.2) where it has one,
//! which [`crate::set_info`] keeps in step. PDF 2.0 deprecates `/Info`; PDF/A-4 makes that
//! normative: the trailer has no `/Info` unless the catalog has `/PieceInfo`, and an `/Info` that
//! is there holds `/ModDate` and nothing else (ISO 19005-4 §6.1.3). A file that declares PDF/A-4
//! ([`MetadataHome::Xmp`]) therefore keeps its metadata in XMP alone:
//!
//! - a save records its time as `xmp:ModifyDate` (and `xmp:MetadataDate`) instead of stamping
//!   `/ModDate` into a new `/Info` ([`record_save`]); with `/PieceInfo`, whose `LastModified`
//!   dates are read against `/ModDate` (ISO 32000-2 §14.5), `/ModDate` is stamped as well;
//! - a property change writes the XMP property alone ([`set_info_in`]), and is refused, with
//!   nothing changed, where XMP can't hold it here: a custom property, the dates, `Trapped`, or a
//!   packet that can't be edited.
//!
//! An `/Info` such a file already has, from an earlier writer, is not stamped, and nothing is added
//! to it. A property change takes the entry it replaces out of it, since the new value lives in
//! XMP and the old one would contradict it; when that leaves it empty, the trailer's `/Info` goes
//! too. Its other entries stay as they are: dropping them would lose data nobody asked to remove.
//!
//! Every other document ([`MetadataHome::Info`]) is written exactly as before.

use pdfcraft_cos::Document;

use crate::{OrganizeError, STANDARD_INFO_KEYS, xmp};

/// Where a document keeps its document-level metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataHome {
    /// The document information dictionary, with the XMP packet (if any) kept in step: every
    /// document that doesn't declare PDF/A-4.
    Info,
    /// The XMP packet alone (PDF/A-4, ISO 19005-4 §6.1.3). `piece_info`: the catalog has
    /// `/PieceInfo`, so `/Info` may exist, holding `/ModDate` only.
    Xmp { piece_info: bool },
}

impl MetadataHome {
    /// Where `doc` keeps its metadata. `declares_pdfa4` is whether it declares PDF/A-4, as
    /// `pdfcraft_preflight::declares_pdfa4` reads its identification.
    pub fn of(doc: &Document, declares_pdfa4: bool) -> Self {
        if !declares_pdfa4 {
            return Self::Info;
        }
        let piece_info = doc.root().is_some_and(|root| doc.get(root).as_dict().is_some_and(|c| c.get(b"PieceInfo").is_some()));
        Self::Xmp { piece_info }
    }
}

/// Before a save of `doc` that would stamp `mod_date` (a PDF date, from `pdfcraft_cos::pdf_date`
/// or a signing time) into `/Info /ModDate`: the `mod_date` to give the writer
/// (`SaveOptions::mod_date`). For [`MetadataHome::Info`] that is `mod_date` itself, and `doc` is
/// not touched. For [`MetadataHome::Xmp`] the time goes into the XMP packet (see the module
/// documentation), and the writer stamps `/Info` only when `/PieceInfo` allows it.
pub fn record_save(doc: &mut Document, home: MetadataHome, mod_date: Option<String>) -> Result<Option<String>, OrganizeError> {
    let MetadataHome::Xmp { piece_info } = home else { return Ok(mod_date) };
    let Some(date) = mod_date else { return Ok(None) };
    if let Some(iso) = xmp_date(&date) {
        xmp::set_modified(doc, &iso)?;
    }
    Ok(piece_info.then_some(date))
}

/// Set (or clear, with an empty value) the document property `key` where `home` says the
/// document keeps it: [`crate::set_info`] for [`MetadataHome::Info`], the XMP property alone
/// for [`MetadataHome::Xmp`] (see the module documentation). Refused, changing nothing, when
/// the key has no XMP property PdfCraft writes (Title, Author, Subject, Keywords, Creator and
/// Producer do) or the packet can't be edited. Clearing such a key still takes it out of `/Info`
/// when the packet holds no copy of it; one the packet still holds refuses the clear, since the
/// property would stay.
pub fn set_info_in(doc: &mut Document, home: MetadataHome, key: &str, value: &str) -> Result<(), OrganizeError> {
    let MetadataHome::Xmp { piece_info } = home else { return crate::set_info(doc, key, value) };
    let custom = !STANDARD_INFO_KEYS.contains(&key);
    if custom {
        crate::check_custom_key(key)?;
    }
    let value = value.trim();
    match xmp::set_mirror(doc, key, value)? {
        Some(true) => {}
        Some(false) => {
            return Err(OrganizeError::Invalid(format!(
                "a PDF/A-4 file keeps its document properties in its XMP metadata, which can't be edited here, so {key} was not changed"
            )));
        }
        None if value.is_empty() => {
            if custom {
                xmp::remove_custom_properties(doc, &[key])?;
            }
        }
        None => {
            return Err(OrganizeError::Invalid(format!(
                "{key} can't be set in a PDF/A-4 file: it keeps its document properties in XMP metadata only, where PdfCraft writes Title, Author, Subject, Keywords, Creator and Producer"
            )));
        }
    }
    drop_info_entry(doc, key, piece_info)
}

/// Take `key` out of the `/Info` a PDF/A-4 file already has: the value now lives in XMP. An
/// `/Info` left empty goes from the trailer, unless `/PieceInfo` lets it stay.
fn drop_info_entry(doc: &mut Document, key: &str, piece_info: bool) -> Result<(), OrganizeError> {
    if !crate::has_info(doc, key) {
        return Ok(());
    }
    crate::set_info_entry(doc, key.as_bytes(), None)?;
    let empty = doc.trailer().get(b"Info").map(|o| doc.resolve(o)).is_some_and(|info| info.as_dict().is_some_and(|d| d.iter().next().is_none()));
    if empty && !piece_info {
        doc.trailer_mut().remove(b"Info");
    }
    Ok(())
}

/// A PDF date as PdfCraft writes one (ISO 32000-2 §7.9.4: `D:YYYYMMDDHHmmSS`, then `Z`, an
/// offset `+HH'mm'` or `-HH'mm'` (the last apostrophe optional), or nothing) as an XMP date
/// (ISO 8601: `YYYY-MM-DDThh:mm:ss`, then `Z`, `±hh:mm` or nothing). `None` for any other
/// shape: the save then records no XMP date rather than a wrong one.
pub(crate) fn xmp_date(pdf: &str) -> Option<String> {
    let rest = pdf.strip_prefix("D:")?;
    let digits = rest.get(..14).filter(|d| d.bytes().all(|b| b.is_ascii_digit()))?;
    let part = |a: usize, b: usize| digits.get(a..b);
    let (year, month, day, hour, minute, second) = (part(0, 4)?, part(4, 6)?, part(6, 8)?, part(8, 10)?, part(10, 12)?, part(12, 14)?);
    let zone = match rest.get(14..)? {
        "" => String::new(),
        "Z" => "Z".to_string(),
        offset => {
            let sign = offset.get(..1).filter(|s| matches!(*s, "+" | "-"))?;
            let (h, m) = offset.get(1..)?.trim_end_matches('\'').split_once('\'')?;
            if h.len() != 2 || m.len() != 2 || !h.bytes().chain(m.bytes()).all(|b| b.is_ascii_digit()) {
                return None;
            }
            format!("{sign}{h}:{m}")
        }
    };
    Some(format!("{year}-{month}-{day}T{hour}:{minute}:{second}{zone}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_dates_become_xmp_dates() {
        assert_eq!(xmp_date("D:20261117093005Z").as_deref(), Some("2026-11-17T09:30:05Z"));
        assert_eq!(xmp_date("D:20261117093005+02'00'").as_deref(), Some("2026-11-17T09:30:05+02:00"));
        assert_eq!(xmp_date("D:20261117093005-05'30").as_deref(), Some("2026-11-17T09:30:05-05:30"));
        assert_eq!(xmp_date("D:20261117093005").as_deref(), Some("2026-11-17T09:30:05"));
        for bad in [
            "",
            "D:",
            "20261117093005Z",
            "D:2026111709300Z",
            "D:2026111709300xZ",
            "D:20261117093005+2'00'",
            "D:20261117093005*02'00'",
            "D:20261117093005+02'00'x",
            "D:20261117093005é",
        ] {
            assert_eq!(xmp_date(bad), None, "{bad}");
        }
    }

    use pdfcraft_cos::{Dict, Object, PdfString, SaveOptions, Stream, write_full};

    const PDFA4: &str = "pdfaid:part=\"4\" pdfaid:rev=\"2020\"";

    /// A document whose catalog has `xmp` (unfiltered) as its metadata, `catalog` added to the
    /// catalog, and the `/Info` entries `info` (none: no `/Info`).
    fn doc(xmp: &str, catalog: &[(&str, Object)], info: &[(&str, &str)]) -> Document {
        let mut doc = Document::new_empty();
        let mut dict = Dict::new();
        dict.set(b"Type".to_vec(), Object::name("Metadata"));
        dict.set(b"Subtype".to_vec(), Object::name("XML"));
        let m = doc.add(Object::Stream(Stream::from_raw(dict, xmp.as_bytes().to_vec())));
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| {
            c.set(b"Metadata".to_vec(), Object::Ref(m));
            for (k, v) in catalog {
                c.set(k.as_bytes().to_vec(), v.clone());
            }
        })
        .unwrap();
        if !info.is_empty() {
            let mut d = Dict::new();
            for (k, v) in info {
                d.set(k.as_bytes().to_vec(), Object::String(PdfString::text(v)));
            }
            let r = doc.add(d);
            doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(r));
        }
        doc
    }

    fn packet(ids: &str, body: &str) -> String {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
             <rdf:Description rdf:about=\"\" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" {ids}/>{body}</rdf:RDF></x:xmpmeta>"
        )
    }

    fn xmp_of(doc: &Document) -> String {
        let r = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Metadata").unwrap();
        let Object::Stream(s) = &*doc.get(r) else { panic!("a stream") };
        String::from_utf8(s.decoded().unwrap()).unwrap()
    }

    fn written(doc: &Document) -> Vec<u8> {
        write_full(doc, &SaveOptions { mod_date: Some("D:20261117093005Z".into()), ..SaveOptions::default() }).unwrap()
    }

    #[test]
    fn only_a_pdfa4_declaration_moves_metadata_to_xmp() {
        let d = doc(&packet(PDFA4, ""), &[], &[]);
        assert_eq!(MetadataHome::of(&d, false), MetadataHome::Info);
        assert_eq!(MetadataHome::of(&d, true), MetadataHome::Xmp { piece_info: false });
        let d = doc(&packet(PDFA4, ""), &[("PieceInfo", Object::Dict(Dict::new()))], &[]);
        assert_eq!(MetadataHome::of(&d, true), MetadataHome::Xmp { piece_info: true });
    }

    /// Every other document is written exactly as before: the save stamps `/Info`, and a
    /// property change is `set_info`'s.
    #[test]
    fn the_info_home_changes_nothing() {
        let xmp = packet("pdfaid:part=\"2\" pdfaid:conformance=\"B\"", "");
        let mut d = doc(&xmp, &[], &[("Title", "Old")]);
        let before = written(&d);
        assert_eq!(record_save(&mut d, MetadataHome::Info, Some("D:1".into())).unwrap().as_deref(), Some("D:1"));
        assert_eq!(written(&d), before, "nothing touched");
        let mut by_set_info = d.clone();
        set_info_in(&mut d, MetadataHome::Info, "Title", "New").unwrap();
        set_info_in(&mut d, MetadataHome::Info, "Project", "Apollo").unwrap();
        crate::set_info(&mut by_set_info, "Title", "New").unwrap();
        crate::set_info(&mut by_set_info, "Project", "Apollo").unwrap();
        assert_eq!(written(&d), written(&by_set_info));
    }

    #[test]
    fn a_save_dates_the_packet_where_its_dates_stand() {
        // Attribute form: rewritten in place; the missing MetadataDate is added.
        let ids = format!("{PDFA4} xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmp:ModifyDate=\"2020-01-01T00:00:00Z\"");
        let mut d = doc(&packet(&ids, ""), &[], &[]);
        let home = MetadataHome::Xmp { piece_info: false };
        assert_eq!(record_save(&mut d, home, Some("D:20261117093005+01'00'".into())).unwrap(), None, "no /Info stamp");
        let x = xmp_of(&d);
        assert!(x.contains("xmp:ModifyDate=\"2026-11-17T09:30:05+01:00\"") && !x.contains("2020-01-01"), "{x}");
        assert!(x.contains("<xmp:MetadataDate>2026-11-17T09:30:05+01:00</xmp:MetadataDate>"), "{x}");
        // With /PieceInfo the writer stamps /ModDate too.
        let home = MetadataHome::Xmp { piece_info: true };
        assert_eq!(record_save(&mut d, home, Some("D:20261117093005Z".into())).unwrap().as_deref(), Some("D:20261117093005Z"));
        // A date of another shape dates nothing, and no clock stamps nothing.
        let before = xmp_of(&d);
        assert_eq!(record_save(&mut d, MetadataHome::Xmp { piece_info: false }, Some("yesterday".into())).unwrap(), None);
        assert_eq!(record_save(&mut d, MetadataHome::Xmp { piece_info: true }, None).unwrap(), None);
        assert_eq!(xmp_of(&d), before);
    }

    /// A packet that dates itself under another prefix for the XMP namespace (`xap:`, as early
    /// writers did), or binds `xmp` elsewhere, is left alone rather than given a second date.
    #[test]
    fn a_date_under_another_prefix_is_not_doubled() {
        let home = MetadataHome::Xmp { piece_info: false };
        for body in [
            "<rdf:Description rdf:about=\"\" xmlns:xap=\"http://ns.adobe.com/xap/1.0/\"><xap:ModifyDate>2020-01-01T00:00:00Z</xap:ModifyDate></rdf:Description>",
            "<rdf:Description rdf:about=\"\" xmlns:xmp=\"urn:other\"><xmp:Other>1</xmp:Other></rdf:Description>",
        ] {
            let mut d = doc(&packet(PDFA4, body), &[], &[]);
            let before = xmp_of(&d);
            assert_eq!(record_save(&mut d, home, Some("D:20261117093005Z".into())).unwrap(), None);
            assert_eq!(xmp_of(&d), before, "{body}");
        }
    }

    #[test]
    fn a_packet_that_cant_be_edited_refuses_the_change() {
        // No rdf:RDF to add a title to.
        let mut d = doc("<x:xmpmeta xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" pdfaid:part=\"4\"/>", &[], &[("Title", "Old")]);
        let before = written(&d);
        let home = MetadataHome::Xmp { piece_info: false };
        let err = set_info_in(&mut d, home, "Title", "New").unwrap_err();
        assert!(err.to_string().contains("PDF/A-4"), "{err}");
        assert_eq!(written(&d), before, "nothing changed");
        // Clearing it still takes the old value out of /Info, and the empty /Info with it.
        set_info_in(&mut d, home, "Title", "").unwrap();
        assert!(d.trailer().get(b"Info").is_none());
    }

    #[test]
    fn with_piece_info_an_emptied_info_stays() {
        let pieces = [("PieceInfo", Object::Dict(Dict::new()))];
        let mut d = doc(&packet(PDFA4, ""), &pieces, &[("Title", "Old")]);
        set_info_in(&mut d, MetadataHome::Xmp { piece_info: true }, "Title", "New").unwrap();
        let info = d.trailer().get(b"Info").map(|o| d.resolve(o)).unwrap();
        assert_eq!(info.as_dict().map(|i| i.iter().count()), Some(0));
        assert!(xmp_of(&d).contains(">New<"));
    }

    /// A property the packet holds in a form this can't rewrite (another prefix for its
    /// namespace, its prefix bound elsewhere, `rdf:RDF` under another prefix) refuses both the
    /// change and the clear, with nothing changed: `/Info` keeps the value the packet would
    /// otherwise contradict or lose.
    #[test]
    fn a_property_in_a_form_that_cant_be_rewritten_is_refused() {
        let home = MetadataHome::Xmp { piece_info: false };
        let ids = "xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" pdfaid:part=\"4\" pdfaid:rev=\"2020\"";
        let other_rdf = format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><r:RDF xmlns:r=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
             <r:Description r:about=\"\" {ids} xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
             <dc:title><r:Alt><r:li xml:lang=\"x-default\">Old</r:li></r:Alt></dc:title></r:Description></r:RDF></x:xmpmeta>"
        );
        let other_dc = packet(
            PDFA4,
            "<rdf:Description rdf:about=\"\" xmlns:d=\"http://purl.org/dc/elements/1.1/\">\
             <d:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Old</rdf:li></rdf:Alt></d:title></rdf:Description>",
        );
        let vendor_xmp =
            packet(PDFA4, "<rdf:Description rdf:about=\"\" xmlns:xmp=\"urn:vendor\"><xmp:CreatorTool>Vendor</xmp:CreatorTool></rdf:Description>");
        for (xmp, key) in [(&other_rdf, "Title"), (&other_dc, "Title"), (&vendor_xmp, "Creator")] {
            for value in ["New", ""] {
                let mut d = doc(xmp, &[], &[(key, "Old")]);
                let before = written(&d);
                assert!(set_info_in(&mut d, home, key, value).is_err(), "{key}={value:?} in {xmp}");
                assert_eq!(written(&d), before, "nothing changed: {key}={value:?} in {xmp}");
            }
        }
    }

    /// A change the packet doesn't take leaves `/Info` as it was. An `r:RDF` packet whose only
    /// `</rdf:RDF>` is in a comment has no end tag to add a title before, and a clear that would
    /// leave a copy of the property behind (an element that never ends) is refused. A clear of a
    /// title whose end tag holds whitespace takes it out of both.
    #[test]
    fn a_change_the_packet_doesnt_take_keeps_info() {
        let home = MetadataHome::Xmp { piece_info: false };
        let ids = "xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" pdfaid:part=\"4\" pdfaid:rev=\"2020\"";
        let commented = format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><r:RDF xmlns:r=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
             <r:Description r:about=\"\" {ids}/></r:RDF><!-- </rdf:RDF> --></x:xmpmeta>"
        );
        let unended = packet(PDFA4, "<rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>Old");
        // A complete title the clear could take out, then one it can't: the whole clear is refused.
        let after_one = packet(
            PDFA4,
            "<rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
             <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Old</rdf:li></rdf:Alt></dc:title><dc:title>Old",
        );
        for (xmp, value) in [(&commented, "New"), (&unended, ""), (&after_one, "")] {
            let mut d = doc(xmp, &[], &[("Title", "Old")]);
            let before = written(&d);
            assert!(set_info_in(&mut d, home, "Title", value).is_err(), "{value:?} in {xmp}");
            assert_eq!(written(&d), before, "nothing changed: {value:?} in {xmp}");
        }
        let spaced = packet(
            PDFA4,
            "<rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
             <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Old</rdf:li></rdf:Alt></dc:title ></rdf:Description>",
        );
        let mut d = doc(&spaced, &[], &[("Title", "Old")]);
        set_info_in(&mut d, home, "Title", "").unwrap();
        let x = xmp_of(&d);
        assert!(!x.contains("dc:title") && d.trailer().get(b"Info").is_none(), "{x}");
    }

    /// A `>` inside a quoted attribute value of a property's start tag doesn't end the tag (XML 1.0
    /// §2.3 `AttValue`): the rewritten property keeps the `xmlns:pdf` declared after it, so its
    /// prefix stays bound when `/Info` gives up its copy.
    #[test]
    fn a_quoted_gt_in_a_start_tag_keeps_its_namespace() {
        let home = MetadataHome::Xmp { piece_info: false };
        let pdf = "http://ns.adobe.com/pdf/1.3/";
        let xmp = packet(
            PDFA4,
            &format!("<rdf:Description rdf:about=\"\"><pdf:Keywords rdf:value=\"a>b\" xmlns:pdf=\"{pdf}\">old</pdf:Keywords></rdf:Description>"),
        );
        let mut d = doc(&xmp, &[], &[("Keywords", "old")]);
        set_info_in(&mut d, home, "Keywords", "new").unwrap();
        let x = xmp_of(&d);
        assert!(x.contains(&format!("<pdf:Keywords xmlns:pdf=\"{pdf}\">new</pdf:Keywords>")), "{x}");
        assert!(d.trailer().get(b"Info").is_none(), "{x}");
    }

    /// The save's date check reads element and attribute names: a title that mentions
    /// `xap:ModifyDate` doesn't stop the dates being recorded, and an `xmp:ModifyDate` that binds
    /// its own prefix keeps that binding when rewritten.
    #[test]
    fn a_save_dates_packets_that_only_look_unusual() {
        let home = MetadataHome::Xmp { piece_info: false };
        let mention = packet(
            &format!("{PDFA4} xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmp:ModifyDate=\"2020-01-01T00:00:00Z\""),
            "<rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
             <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">On xap:ModifyDate</rdf:li></rdf:Alt></dc:title></rdf:Description>",
        );
        let mut d = doc(&mention, &[], &[]);
        record_save(&mut d, home, Some("D:20261117093005Z".into())).unwrap();
        let x = xmp_of(&d);
        assert!(x.contains("xmp:ModifyDate=\"2026-11-17T09:30:05Z\"") && x.contains("On xap:ModifyDate"), "{x}");

        let local = packet(
            PDFA4,
            "<rdf:Description rdf:about=\"\"><xmp:ModifyDate xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">2020-01-01T00:00:00Z</xmp:ModifyDate></rdf:Description>",
        );
        let mut d = doc(&local, &[], &[]);
        record_save(&mut d, home, Some("D:20261117093005Z".into())).unwrap();
        let x = xmp_of(&d);
        assert!(x.contains("<xmp:ModifyDate xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">2026-11-17T09:30:05Z</xmp:ModifyDate>"), "{x}");
    }
}
