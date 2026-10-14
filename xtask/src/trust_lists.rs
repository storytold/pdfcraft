//! `cargo xtask trust-lists [--out FILE]`: write `dist/trust/eutl-qualified-ca.der`, the CA
//! certificates of the qualified trust services in the EU Trusted Lists (eIDAS, ETSI TS 119 612).
//!
//! The file is a trust list for the user to load (the `sign_trust` tool's `eu_trusted_list`
//! argument); it is not part of the repository or of any binary. The lists are published under
//! CC BY 4.0 by the European Commission and the Member States' supervisory bodies: keep that
//! attribution when passing the file on.
//!
//! Fetches the List of Trusted Lists from the European Commission, then every national list it
//! points to (XML), and keeps the certificates of services of type `CA/QC` whose status is
//! `granted` or `recognisedatnationallevel`. Withdrawn and deprecated services are left out, and
//! so are the ones that issue only certificates for web site authentication (QWAC CAs): their
//! `AdditionalServiceInformation` names `ForWebSiteAuthentication` and neither `ForeSignatures`
//! nor `ForeSeals`. A service that says nothing about it is kept.
//!
//! Not done, and why it matters:
//! - Only a service's current status is read, not its `ServiceHistory`. Whether a CA vouched is
//!   judged as of today, not when the document was signed (ETSI TS 119 615 does the latter): a
//!   service withdrawn since drops out, so its older signatures show as unknown, and one granted
//!   since vouches for older signatures too.
//! - `TSA/QTST` (qualified time-stamp) services are not collected: a loaded trust list vouches for
//!   signers only, never for a time-stamp authority, so they would be dead weight in it. About a
//!   thousand are granted (1,081 certificates on 2026-10-11) and nine in ten of those are
//!   time-stamp units, not CAs; they would need a list of their own that vouches only for time
//!   stamps.
//!
//! Every certificate of an accepted service must be valid base64 and come to exactly one DER
//! SEQUENCE, the shape the trust list loader requires of each entry: a list with one that does not
//! fails, naming the service, and nothing is written, instead of a bundle that is silently
//! incomplete or that the loader refuses as damaged.
//!
//! The bundle is every certificate's DER, concatenated and sorted by SHA-256, so a rerun with the
//! same lists gives the same bytes. The XML signatures on the lists are *not* checked, and nothing
//! is pinned, so everything is fetched over HTTPS only: a list the List of Trusted Lists points to
//! with `http://` (Slovakia's does) is fetched from the same address with `https://`, and curl
//! refuses any other protocol, redirects included.
//!
//! Uses `curl` (macOS, Windows 10+ and Linux have it), like `cargo xtask models`.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::alphabet::STANDARD;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use sha2::{Digest, Sha256};

const LOTL: &str = "https://ec.europa.eu/tools/lotl/eu-lotl.xml";
const OUT: &str = "dist/trust/eutl-qualified-ca.der";
const TSL_XML: &str = "application/vnd.etsi.tsl+xml";
/// The largest list `fetch` accepts (the biggest national list is a few MB).
const MAX_LIST_BYTES: &str = "64000000";
/// How the lists write a certificate (XML base64Binary): the standard alphabet, padding present or
/// not. Everything else stays strict: a stray character, a one-character tail or non-zero trailing
/// bits is an error, not a certificate.
const BASE64: GeneralPurpose = GeneralPurpose::new(&STANDARD, GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent));

/// `url` with `https://` for `http://`: the lists are not signed-checked, so they are not fetched
/// over a channel anyone on the path could change.
fn https(url: &str) -> String {
    url.strip_prefix("http://").map_or_else(|| url.to_string(), |rest| format!("https://{rest}"))
}

fn fetch(url: &str) -> Result<String> {
    let url = https(url);
    let out = Command::new("curl")
        .args([
            "-sSfL",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "-m",
            "90",
            "--max-filesize",
            MAX_LIST_BYTES,
            "--retry",
            "3",
            "--retry-all-errors",
            &url,
        ])
        .output()
        .context("running curl")?;
    if !out.status.success() {
        bail!("{url}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    String::from_utf8(out.stdout).with_context(|| format!("{url}: not UTF-8"))
}

fn text<'a>(n: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.descendants().find(|d| d.has_tag_name(name)).and_then(|d| d.text()).map(str::trim)
}

/// The XML list locations in the List of Trusted Lists (not itself, not the post-Brexit UK list,
/// which the Commission still points to: a frozen copy, no longer an EU list).
fn national_lists(lotl: &str) -> Result<Vec<String>> {
    let doc = roxmltree::Document::parse(lotl).context("parsing the LOTL")?;
    Ok(doc
        .descendants()
        .filter(|n| n.has_tag_name("OtherTSLPointer"))
        .filter(|n| n.descendants().any(|d| d.has_tag_name("MimeType") && d.text().map(str::trim) == Some(TSL_XML)))
        .filter(|n| text(*n, "SchemeTerritory") != Some("UK"))
        .filter_map(|n| text(n, "TSLLocation"))
        .filter(|u| !u.ends_with("eu-lotl.xml"))
        .map(str::to_string)
        .collect())
}

/// Whether a `CA/QC` service issues certificates that sign documents. Its additional service
/// information (ETSI TS 119 612 §5.5.9.4) says what the qualified certificates are for; a CA that
/// names only `ForWebSiteAuthentication` issues QWACs, which are not signing certificates.
fn issues_for_documents(svc: roxmltree::Node) -> bool {
    let uses: Vec<&str> = svc
        .descendants()
        .filter(|n| n.has_tag_name("AdditionalServiceInformation"))
        .flat_map(|n| n.descendants().filter(|d| d.has_tag_name("URI")).filter_map(|d| d.text()))
        .filter_map(|u| u.trim().rsplit('/').next())
        .collect();
    uses.iter().any(|u| matches!(*u, "ForeSignatures" | "ForeSeals")) || !uses.contains(&"ForWebSiteAuthentication")
}

/// Whether `der` is exactly one DER SEQUENCE, the shape of an X.509 certificate: the tag, a
/// definite length written the shortest way DER allows, and as many bytes after it as it says.
/// The trust list loader refuses a file whose entries are not of this shape, so none is written.
fn is_der_sequence(der: &[u8]) -> bool {
    let Some((&0x30, rest)) = der.split_first() else { return false };
    let Some((&first, rest)) = rest.split_first() else { return false };
    if first < 0x80 {
        return rest.len() == usize::from(first);
    }
    let count = usize::from(first & 0x7f);
    if count == 0 || count > 4 || rest.len() < count {
        return false;
    }
    let (length, body) = rest.split_at(count);
    let value = length.iter().fold(0usize, |acc, b| (acc << 8) | usize::from(*b));
    // Minimal: no leading zero byte, and not a length the short form could have held.
    length.first() != Some(&0) && value >= 0x80 && body.len() == value
}

/// One certificate as a list writes it: base64, with line breaks, that comes to one DER SEQUENCE.
fn decode_certificate(text: &str) -> Result<Vec<u8>> {
    let compact: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let der = BASE64.decode(compact.as_bytes()).context("not valid base64")?;
    if !is_der_sequence(&der) {
        bail!("valid base64, but not one DER SEQUENCE ({} bytes)", der.len());
    }
    Ok(der)
}

/// DER certificates of the granted qualified CA services of one list. A certificate of such a
/// service that can't be read fails the list, naming the service.
fn qualified_cas(list: &str) -> Result<Vec<Vec<u8>>> {
    let doc = roxmltree::Document::parse(list).context("parsing a trusted list")?;
    let mut out = Vec::new();
    for svc in doc.descendants().filter(|n| n.has_tag_name("ServiceInformation")) {
        let kind = text(svc, "ServiceTypeIdentifier").unwrap_or_default();
        let status = text(svc, "ServiceStatus").unwrap_or_default();
        let granted = status.ends_with("/granted") || status.ends_with("/recognisedatnationallevel");
        if kind.ends_with("/CA/QC") && granted && issues_for_documents(svc) {
            for c in svc.descendants().filter(|n| n.has_tag_name("X509Certificate")) {
                let der = decode_certificate(c.text().unwrap_or_default())
                    .with_context(|| format!("a certificate of the service \"{}\"", text(svc, "Name").unwrap_or("(unnamed)")))?;
                out.push(der);
            }
        }
    }
    Ok(out)
}

pub fn run(args: &[String]) -> Result<()> {
    let out = match args {
        [] => Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(OUT),
        [flag, file] if flag == "--out" => Path::new(file).to_path_buf(),
        _ => bail!("usage: cargo xtask trust-lists [--out FILE]"),
    };
    let lists = national_lists(&fetch(LOTL)?)?;
    println!("{} national trusted lists", lists.len());
    let mut certs: BTreeMap<[u8; 32], Vec<u8>> = BTreeMap::new();
    let mut failed = Vec::new();
    for url in &lists {
        match fetch(url).and_then(|l| qualified_cas(&l)) {
            Ok(found) => {
                println!("  {url}: {} certificates", found.len());
                for der in found {
                    certs.insert(Sha256::digest(&der).into(), der);
                }
            }
            Err(e) => {
                eprintln!("  {url}: {e:#}");
                failed.push(url.as_str());
            }
        }
    }
    if !failed.is_empty() {
        bail!("{} list(s) could not be read (rerun later; the bundle was not changed): {}", failed.len(), failed.join(", "));
    }
    let bundle: Vec<u8> = certs.values().flatten().copied().collect();
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out, &bundle)?;
    println!(
        "{} certificates, {} bytes -> {}\nLoad it with the sign_trust tool: {{\"tool\": \"sign_trust\", \"args\": {{\"eu_trusted_list\": \"<path>\"}}}} (the path must be inside the --root of the session). Source: the EU Trusted Lists (CC BY 4.0, European Commission and Member States' supervisory bodies).",
        certs.len(),
        bundle.len(),
        out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use base64::engine::general_purpose::STANDARD as ENCODER;

    use super::*;

    /// A stand-in for a certificate that is well-formed where it matters here: SEQUENCE { INTEGER n }.
    fn cert(n: u8) -> (String, Vec<u8>) {
        let der = vec![0x30, 3, 2, 1, n];
        (ENCODER.encode(&der), der)
    }

    fn service(name: &str, kind: &str, status: &str, info: &[&str], cert: &str) -> String {
        let uris: String = info.iter().map(|i| format!("<URI>http://uri.etsi.org/TrstSvc/TrustedList/SvcInfoExt/{i}</URI>")).collect();
        let extension = if info.is_empty() {
            String::new()
        } else {
            format!(
                "<ServiceInformationExtensions><Extension><AdditionalServiceInformation>{uris}</AdditionalServiceInformation></Extension></ServiceInformationExtensions>"
            )
        };
        format!(
            "<TSPService><ServiceInformation><ServiceTypeIdentifier>http://uri.etsi.org/TrstSvc/Svctype/{kind}</ServiceTypeIdentifier>\
             <ServiceName><Name xml:lang=\"en\">{name}</Name></ServiceName>\
             <ServiceDigitalIdentity><DigitalId><X509Certificate>{cert}</X509Certificate></DigitalId></ServiceDigitalIdentity>\
             <ServiceStatus>http://uri.etsi.org/TrstSvc/TrustedList/Svcstatus/{status}</ServiceStatus>{extension}</ServiceInformation></TSPService>"
        )
    }

    fn list_of(services: &[String]) -> String {
        format!("<TrustServiceStatusList>{}</TrustServiceStatusList>", services.concat())
    }

    #[test]
    fn only_granted_qualified_cas_that_issue_for_documents_are_collected() {
        let list = list_of(&[
            service("Signatures CA", "CA/QC", "granted", &["ForeSignatures"], &cert(1).0),
            // A QWAC CA does not vouch for people who sign documents.
            service("Web CA", "CA/QC", "granted", &["ForWebSiteAuthentication"], &cert(2).0),
            service("Root web CA", "CA/QC", "granted", &["RootCA-QC", "ForWebSiteAuthentication"], &cert(3).0),
            // One that issues for web sites and for seals, or says nothing, still does.
            service("Seals CA", "CA/QC", "granted", &["ForWebSiteAuthentication", "ForeSeals"], &cert(4).0),
            service("National CA", "CA/QC", "recognisedatnationallevel", &[], &cert(5).0),
            service("Withdrawn CA", "CA/QC", "withdrawn", &["ForeSignatures"], &cert(6).0),
            service("Time stamps", "TSA/QTST", "granted", &[], &cert(7).0),
        ]);
        assert_eq!(qualified_cas(&list).unwrap(), vec![cert(1).1, cert(4).1, cert(5).1]);
    }

    #[test]
    fn a_certificate_that_cannot_be_read_fails_the_list_naming_the_service() {
        let variants: Vec<(&str, String)> = vec![
            ("one character", "A".into()),
            ("a stray character", "MAMC*QE=".into()),
            ("padding in the middle", "MA=MCAQE".into()),
            ("non-zero trailing bits", "MAMCAQF=".into()),
            ("nothing", String::new()),
            ("not a SEQUENCE", ENCODER.encode([1, 2, 3, 4])),
            ("a byte after the SEQUENCE", ENCODER.encode([0x30, 3, 2, 1, 5, 0])),
            ("a SEQUENCE cut short", ENCODER.encode([0x30, 3, 2, 1])),
            ("a long length that fits the short form", ENCODER.encode([0x30, 0x81, 3, 2, 1, 5])),
            ("a length with a leading zero", ENCODER.encode([0x30, 0x82, 0, 3, 2, 1, 5])),
        ];
        for (what, text) in &variants {
            let list = list_of(&[service("Fine CA", "CA/QC", "granted", &[], &cert(1).0), service("Broken CA", "CA/QC", "granted", &[], text)]);
            let err = format!("{:#}", qualified_cas(&list).expect_err(what));
            assert!(err.contains("\"Broken CA\""), "{what}: {err}");
        }
        // An element with no text at all is the same as nothing.
        let empty_element =
            list_of(&[service("Broken CA", "CA/QC", "granted", &[], "")]).replace("<X509Certificate></X509Certificate>", "<X509Certificate/>");
        assert!(empty_element.contains("<X509Certificate/>"));
        assert!(format!("{:#}", qualified_cas(&empty_element).unwrap_err()).contains("\"Broken CA\""));
        // A service nobody collects may carry what it likes.
        let list = list_of(&[
            service("Web CA", "CA/QC", "granted", &["ForWebSiteAuthentication"], "A"),
            service("Withdrawn", "CA/QC", "withdrawn", &[], "A"),
        ]);
        assert_eq!(qualified_cas(&list).unwrap(), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn line_breaks_and_missing_padding_are_fine() {
        let want = cert(5).1;
        for text in ["MAMCAQU=", "MAMCAQU", "  MAMC\n  AQU=\n", "MAMC\r\nAQU"] {
            assert_eq!(decode_certificate(text).unwrap(), want, "{text:?}");
        }
        // A long length in its shortest long form.
        let mut long = vec![0x30, 0x81, 200];
        long.extend([0u8; 200]);
        assert_eq!(decode_certificate(&ENCODER.encode(&long)).unwrap(), long);
    }

    #[test]
    fn what_is_one_der_sequence() {
        let sequence = |len_bytes: &[u8], body: usize| [&[0x30][..], len_bytes, &vec![0; body]].concat();
        assert!(is_der_sequence(&sequence(&[0], 0)));
        assert!(is_der_sequence(&sequence(&[0x7f], 0x7f)));
        assert!(is_der_sequence(&sequence(&[0x81, 0x80], 0x80)));
        assert!(is_der_sequence(&sequence(&[0x82, 0x01, 0x00], 0x100)));
        for (what, der) in [
            ("empty", vec![]),
            ("a tag only", vec![0x30]),
            ("another tag", vec![0x31, 0]),
            ("short of its length", sequence(&[5], 4)),
            ("past its length", sequence(&[5], 6)),
            ("indefinite length", vec![0x30, 0x80, 0, 0]),
            ("a length of lengths too long", vec![0x30, 0x85, 0, 0, 0, 0, 1, 0]),
            ("a length cut off", vec![0x30, 0x82, 1]),
            ("non-minimal: fits the short form", sequence(&[0x81, 0x7f], 0x7f)),
            ("non-minimal: leading zero", sequence(&[0x82, 0, 0x80], 0x80)),
        ] {
            assert!(!is_der_sequence(&der), "{what}");
        }
    }

    #[test]
    fn the_uk_list_is_left_out_by_its_territory_not_its_address() {
        let pointer = |territory: &str, url: &str, mime: &str| {
            format!(
                "<OtherTSLPointer><TSLLocation>{url}</TSLLocation><AdditionalInformation>\
                 <OtherInformation><SchemeTerritory>{territory}</SchemeTerritory></OtherInformation>\
                 <OtherInformation><MimeType>{mime}</MimeType></OtherInformation></AdditionalInformation></OtherTSLPointer>"
            )
        };
        let lotl = format!(
            "<TrustServiceStatusList>{}</TrustServiceStatusList>",
            [
                pointer("DE", "https://tl.example/de.xml", TSL_XML),
                pointer("UK", "https://tl.example/a-new-address-for-the-uk.xml", TSL_XML),
                pointer("EU", "https://ec.example/eu-lotl.xml", TSL_XML),
                pointer("FR", "https://tl.example/fr.pdf", "application/pdf"),
            ]
            .concat()
        );
        assert_eq!(national_lists(&lotl).unwrap(), vec!["https://tl.example/de.xml".to_string()]);
    }

    #[test]
    fn lists_are_only_fetched_over_https() {
        assert_eq!(https("http://tl.nbu.gov.sk/kca/tsl/tsl.xml"), "https://tl.nbu.gov.sk/kca/tsl/tsl.xml");
        assert_eq!(https("https://tsl.belgium.be/tsl-be.xml"), "https://tsl.belgium.be/tsl-be.xml");
        // Only the scheme is touched.
        assert_eq!(https("http://a.example/x?u=http://b"), "https://a.example/x?u=http://b");
    }
}
