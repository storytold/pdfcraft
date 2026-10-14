//! `cargo xtask trust-lists [--out FILE]`: write `dist/trust/eutl-qualified-ca.der`, the CA
//! certificates of the qualified trust services in the EU Trusted Lists (eIDAS, ETSI TS 119 612).
//!
//! The file is a trust list for the user to load (`sign_trust` with `eu_trusted_list`); it is not
//! part of the repository or of any binary. The lists are published under CC BY 4.0 by the
//! European Commission and the Member States' supervisory bodies: keep that attribution when
//! passing the file on.
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
use sha2::{Digest, Sha256};

const LOTL: &str = "https://ec.europa.eu/tools/lotl/eu-lotl.xml";
const OUT: &str = "dist/trust/eutl-qualified-ca.der";
const TSL_XML: &str = "application/vnd.etsi.tsl+xml";
/// The largest list `fetch` accepts (the biggest national list is a few MB).
const MAX_LIST_BYTES: &str = "64000000";

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

/// DER certificates of the granted qualified CA services of one list.
fn qualified_cas(list: &str) -> Result<Vec<Vec<u8>>> {
    let doc = roxmltree::Document::parse(list).context("parsing a trusted list")?;
    let mut out = Vec::new();
    for svc in doc.descendants().filter(|n| n.has_tag_name("ServiceInformation")) {
        let kind = text(svc, "ServiceTypeIdentifier").unwrap_or_default();
        let status = text(svc, "ServiceStatus").unwrap_or_default();
        let granted = status.ends_with("/granted") || status.ends_with("/recognisedatnationallevel");
        if kind.ends_with("/CA/QC") && granted && issues_for_documents(svc) {
            for c in svc.descendants().filter(|n| n.has_tag_name("X509Certificate")) {
                if let Some(der) = c.text().and_then(base64) {
                    out.push(der);
                }
            }
        }
    }
    Ok(out)
}

pub(crate) fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
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
        "{} certificates, {} bytes -> {}\nLoad it with sign_trust {{\"eu_trusted_list\": \"<path>\"}}. Source: the EU Trusted Lists (CC BY 4.0, European Commission and Member States' supervisory bodies).",
        certs.len(),
        bundle.len(),
        out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(kind: &str, status: &str, info: &[&str], cert: &str) -> String {
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
             <ServiceDigitalIdentity><DigitalId><X509Certificate>{cert}</X509Certificate></DigitalId></ServiceDigitalIdentity>\
             <ServiceStatus>http://uri.etsi.org/TrstSvc/TrustedList/Svcstatus/{status}</ServiceStatus>{extension}</ServiceInformation></TSPService>"
        )
    }

    #[test]
    fn only_granted_qualified_cas_that_issue_for_documents_are_collected() {
        let list = format!(
            "<TrustServiceStatusList>{}</TrustServiceStatusList>",
            [
                service("CA/QC", "granted", &["ForeSignatures"], "AQID"),
                // A QWAC CA does not vouch for people who sign documents.
                service("CA/QC", "granted", &["ForWebSiteAuthentication"], "BAUG"),
                service("CA/QC", "granted", &["RootCA-QC", "ForWebSiteAuthentication"], "EBES"),
                // One that issues for web sites and for seals, or says nothing, still does.
                service("CA/QC", "granted", &["ForWebSiteAuthentication", "ForeSeals"], "BwgJ"),
                service("CA/QC", "recognisedatnationallevel", &[], "CgsM"),
                service("CA/QC", "withdrawn", &["ForeSignatures"], "DQ4P"),
                service("TSA/QTST", "granted", &[], "FBUW"),
            ]
            .concat()
        );
        assert_eq!(qualified_cas(&list).unwrap(), vec![vec![1, 2, 3], vec![7, 8, 9], vec![10, 11, 12]]);
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
