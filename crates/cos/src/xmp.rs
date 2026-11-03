//! Reading simple properties out of an XMP packet (ISO 32000-2 §14.3.2).
//!
//! PDF 2.0 deprecates the document information dictionary for everything but `CreationDate` and
//! `ModDate` (§14.3.3), so a PDF 2.0 file states its title in its metadata stream as `dc:title`
//! and may carry no `/Info` at all. A reader that only looks at `/Info` sees such a document as
//! having no title; every PDF/UA-2 file is of this shape.
//!
//! Pure text helpers, shared by inspection and the accessibility checker; no object graph and no
//! XML library. The packet is scanned, never rewritten: `pdfcraft-organize` holds the editing
//! side, which must preserve the rest of the packet byte for byte.
//!
//! Each property is written in one of three shapes, and a file may use either the element or the
//! attribute (RDF shorthand) form of any of them:
//!
//! ```text
//! <dc:title><rdf:Alt><rdf:li xml:lang="x-default">Report</rdf:li></rdf:Alt></dc:title>
//! <dc:creator><rdf:Seq><rdf:li>Ada</rdf:li><rdf:li>Grace</rdf:li></rdf:Seq></dc:creator>
//! <pdf:Keywords>quarterly</pdf:Keywords>                     dc:title="Report"
//! ```

/// A packet is a few kilobytes; the padding XMP recommends rarely takes one past 100 kB. This
/// limit is far above that and bounds what a crafted `/Metadata` stream can ask a reader to
/// inflate. The same limit `pdfcraft-organize` decodes a packet for editing within.
pub const MAX_PACKET_BYTES: usize = 16 << 20;

/// The qualified name of the XMP property that mirrors a document information key, with the
/// mapping PDF/A pairs them by (ISO 19005-1 §6.7.3) — the same mapping `pdfcraft-organize`
/// writes and `pdfcraft-preflight` emits.
pub fn mirrors_info(key: &[u8]) -> Option<&'static str> {
    Some(match key {
        b"Title" => "dc:title",
        b"Author" => "dc:creator",
        b"Subject" => "dc:description",
        b"Keywords" => "pdf:Keywords",
        b"Creator" => "xmp:CreatorTool",
        b"Producer" => "pdf:Producer",
        _ => return None,
    })
}

/// The value of a simple XMP property, whichever of the three shapes holds it. A language
/// alternative answers with its `x-default` item (the one a viewer shows), or with its first
/// item when the packet names no default; an ordered array answers with its items joined by
/// `", "`, as Document Properties shows several authors. `None` when the packet doesn't state
/// the property, states it empty, or holds a structure this can't read as one value.
pub fn property(packet: &str, name: &str) -> Option<String> {
    // The element form first: it is the only form a language alternative can take, so a packet
    // using both states the simple value as the attribute and the real one as the element.
    let value = element_value(packet, name).or_else(|| attribute_value(packet, name))?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// The text of the element `name`, resolving `rdf:Alt`, `rdf:Seq` and `rdf:Bag`.
fn element_value(packet: &str, name: &str) -> Option<String> {
    let inner = element_inner(packet, name)?;
    if let Some(alt) = element_inner(inner, "rdf:Alt") {
        let items = items(alt);
        let default = items.iter().find(|(tag, _)| tag.contains("x-default"));
        return default.or_else(|| items.first()).map(|(_, text)| text.clone());
    }
    for ordered in ["rdf:Seq", "rdf:Bag"] {
        if let Some(array) = element_inner(inner, ordered) {
            let items = items(array);
            return (!items.is_empty()).then(|| items.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(", "));
        }
    }
    // Anything else with markup in it is a structure (`rdf:parseType="Resource"`, a qualified
    // value), not one value to show.
    (!inner.contains('<')).then(|| unescape(inner.trim()))
}

/// The items of an `rdf:Alt`, `rdf:Seq` or `rdf:Bag`, as (the rest of the `rdf:li` start tag,
/// which carries any `xml:lang`, text). Items with no text are left out: they are nothing to
/// show, and `x-default` empty must not hide a later language.
fn items(container: &str) -> Vec<(&str, String)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(open) = container.get(from..).and_then(|s| s.find("<rdf:li")).map(|i| i + from) {
        let after = open + "<rdf:li".len();
        from = after;
        match container.get(after..).and_then(|s| s.chars().next()) {
            Some('>' | '/') => {}
            Some(c) if c.is_whitespace() => {}
            // `<rdf:link`, not an item.
            _ => continue,
        }
        let Some(tag_end) = container.get(after..).and_then(|s| s.find('>')).map(|i| i + after) else { break };
        from = tag_end + 1;
        if container.get(..tag_end).is_some_and(|s| s.ends_with('/')) {
            continue;
        }
        let Some(end) = container.get(from..).and_then(|s| s.find("</rdf:li>")).map(|i| i + from) else { break };
        let (Some(tag), Some(text)) = (container.get(after..tag_end), container.get(from..end)) else { break };
        from = end;
        let text = unescape(text.trim());
        if !text.is_empty() {
            out.push((tag, text));
        }
    }
    out
}

/// What the element `name` encloses: between the end of its start tag and its end tag. An empty
/// element (`<dc:title/>`) encloses nothing.
fn element_inner<'a>(packet: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}");
    let mut from = 0;
    while let Some(i) = packet.get(from..).and_then(|s| s.find(&open)).map(|i| i + from) {
        let after = i + open.len();
        from = after;
        // `<dc:title` must end there: not `<dc:titles`.
        match packet.get(after..).and_then(|s| s.chars().next()) {
            Some('>' | '/') => {}
            Some(c) if c.is_whitespace() => {}
            _ => continue,
        }
        let tag_end = after + packet.get(after..)?.find('>')?;
        if packet.get(..tag_end)?.ends_with('/') {
            return Some("");
        }
        let body = packet.get(tag_end + 1..)?;
        return body.get(..body.find(&format!("</{name}>"))?);
    }
    None
}

/// The value of the attribute `name`, which counts only inside a start tag: a packet whose
/// `<dc:description>` text says `dc:title="no"` doesn't state a title.
///
/// The start tags are walked once and the name looked for inside each. Finding the name first and
/// then scanning back for the `<` that encloses it is quadratic: a packet that repeats the name
/// with no angle bracket anywhere makes every occurrence scan the whole prefix, so 16 MiB of it
/// -- a few kilobytes once deflated -- is on the order of 10^13 byte comparisons, six times over
/// for the six entries `doc_info` reads from a packet.
fn attribute_value(packet: &str, name: &str) -> Option<String> {
    // Everything before the first `<` is text; each piece after one begins a tag.
    for piece in packet.split('<').skip(1) {
        // XML allows an unescaped `>` in an attribute value, which XMP writers escape anyway;
        // one here ends the tag for this scan, so the property is missed rather than misread.
        let Some(tag) = piece.find('>').and_then(|e| piece.get(..e)) else { continue };
        let mut from = 0;
        while let Some(at) = tag.get(from..).and_then(|s| s.find(name)).map(|i| i + from) {
            from = at + name.len();
            // Preceded by whitespace, so the tag's own name is not one of its attributes and
            // `xmp:dc:title` is not `dc:title`.
            if !tag.get(..at).is_some_and(|before| before.ends_with(char::is_whitespace)) {
                continue;
            }
            let Some(after_eq) = tag.get(from..).map(str::trim_start).and_then(|s| s.strip_prefix('=')) else { continue };
            let after_eq = after_eq.trim_start();
            let Some(quote) = after_eq.chars().next().filter(|c| matches!(c, '"' | '\'')) else { continue };
            let value = after_eq.get(1..)?;
            return Some(unescape(value.get(..value.find(quote)?)?));
        }
    }
    None
}

/// The five predefined XML entities. `&amp;` resolves last: `&amp;lt;` is the text `&lt;`.
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape a PDF 2.0 / PDF-UA-2 file uses: the title in a language alternative, element form.
    const PACKET: &str = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
<rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\">\
<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Quarterly report</rdf:li></rdf:Alt></dc:title>\
<dc:creator><rdf:Seq><rdf:li>Ada</rdf:li><rdf:li>Grace</rdf:li></rdf:Seq></dc:creator>\
<pdf:Keywords>quarterly, report</pdf:Keywords></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>";

    #[test]
    fn reads_the_three_property_shapes() {
        assert_eq!(property(PACKET, "dc:title").as_deref(), Some("Quarterly report"));
        assert_eq!(property(PACKET, "dc:creator").as_deref(), Some("Ada, Grace"));
        assert_eq!(property(PACKET, "pdf:Keywords").as_deref(), Some("quarterly, report"));
        assert_eq!(property(PACKET, "dc:description"), None);
    }

    #[test]
    fn reads_the_attribute_form_and_both_quotes() {
        let packet = "<rdf:Description rdf:about=\"\" dc:title=\"Odd &amp; old\" pdf:Keywords='one, two'/>";
        assert_eq!(property(packet, "dc:title").as_deref(), Some("Odd & old"));
        assert_eq!(property(packet, "pdf:Keywords").as_deref(), Some("one, two"));
    }

    /// A language alternative shows its `x-default`, whichever item that is, and falls back to
    /// the first item when the packet names no default (as Exiftool writes a single language).
    /// An `x-default` with no text doesn't hide the languages that follow it.
    #[test]
    fn language_alternative_prefers_x_default() {
        let alt = "<dc:title><rdf:Alt><rdf:li xml:lang=\"fr-FR\">Rapport</rdf:li>\
                   <rdf:li xml:lang=\"x-default\">Report</rdf:li></rdf:Alt></dc:title>";
        assert_eq!(property(alt, "dc:title").as_deref(), Some("Report"));
        let no_default = "<dc:title><rdf:Alt><rdf:li xml:lang=\"fr-FR\">Rapport</rdf:li></rdf:Alt></dc:title>";
        assert_eq!(property(no_default, "dc:title").as_deref(), Some("Rapport"));
        let empty_default = "<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\"></rdf:li>\
                             <rdf:li xml:lang=\"fr-FR\">Rapport</rdf:li></rdf:Alt></dc:title>";
        assert_eq!(property(empty_default, "dc:title").as_deref(), Some("Rapport"));
    }

    /// Names are matched whole, and an attribute counts only inside a start tag: neither a
    /// `dc:titles` element nor the words of a description state a title.
    #[test]
    fn near_misses_do_not_count_as_the_property() {
        let packet = "<rdf:Description rdf:about=\"\"><dc:titles>list</dc:titles>\
                      <dc:description><rdf:Alt><rdf:li>say dc:title=\"no\"</rdf:li></rdf:Alt></dc:description></rdf:Description>";
        assert_eq!(property(packet, "dc:title"), None);
        assert_eq!(property(packet, "dc:description").as_deref(), Some("say dc:title=\"no\""));
    }

    /// Stated but empty is no value: a viewer would show nothing, and the checker's Title rule
    /// must still report the document as having no title.
    #[test]
    fn an_empty_or_unreadable_value_is_no_value() {
        assert_eq!(property("<rdf:Description><dc:title/></rdf:Description>", "dc:title"), None);
        assert_eq!(property("<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\"></rdf:li></rdf:Alt></dc:title>", "dc:title"), None);
        assert_eq!(property("<dc:title> \n </dc:title>", "dc:title"), None);
        assert_eq!(property("<rdf:Description dc:title=\"\"/>", "dc:title"), None);
        // A qualified value is a structure, not one value to show.
        assert_eq!(property("<dc:title rdf:parseType=\"Resource\"><rdf:value>V</rdf:value></dc:title>", "dc:title"), None);
    }

    #[test]
    fn a_truncated_packet_answers_nothing_rather_than_guessing() {
        assert_eq!(property("<dc:title><rdf:Alt><rdf:li>Report", "dc:title"), None);
        assert_eq!(property("<dc:title", "dc:title"), None);
        assert_eq!(property("<rdf:Description dc:title=", "dc:title"), None);
        assert_eq!(property("", "dc:title"), None);
    }

    /// A packet that repeats the property's name outside any tag states nothing, and finding that
    /// out costs one pass over it. Searching back from each occurrence for the `<` that encloses
    /// it was quadratic: this packet, 9 MB and a few kilobytes once deflated, took on the order of
    /// 10^13 byte comparisons -- hours -- where the pass takes milliseconds, so the bound below
    /// has a margin of about a thousand and is not a measurement of the machine.
    #[test]
    fn a_name_repeated_outside_any_tag_is_read_in_one_pass() {
        let packet = " dc:title".repeat(1 << 20);
        let started = std::time::Instant::now();
        assert_eq!(property(&packet, "dc:title"), None);
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
        // The same text inside a tag does state it, however many times it is repeated beside it.
        let stated = format!("{packet}<rdf:Description dc:title=\"Quarterly report\"/>");
        assert_eq!(property(&stated, "dc:title").as_deref(), Some("Quarterly report"));
    }

    #[test]
    fn information_keys_map_to_the_properties_pdfa_pairs_them_with() {
        assert_eq!(mirrors_info(b"Title"), Some("dc:title"));
        assert_eq!(mirrors_info(b"Author"), Some("dc:creator"));
        assert_eq!(mirrors_info(b"Subject"), Some("dc:description"));
        assert_eq!(mirrors_info(b"Keywords"), Some("pdf:Keywords"));
        assert_eq!(mirrors_info(b"Trapped"), None);
    }
}
