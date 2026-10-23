//! CMS read as BER: the bounds on what a crafted /Contents can make the reader do.
//!
//! The BER reader itself is `der::Tlv::parse_ber`; these tests go through `SignedData::parse`,
//! the way a PDF's signature reaches it.

use pdfcraft_sign::cms::SignedData;
use pdfcraft_sign::der::{self, tag};

/// `levels` constructed OCTET STRINGs inside one another, each with a definite length, around one
/// empty primitive OCTET STRING. Built from the inside out in linear time.
fn nested_octet_strings(levels: usize) -> Vec<u8> {
    let header = |len: usize| -> Vec<u8> {
        let mut h = vec![tag::OCTET_STRING_SEGMENTS];
        if len < 0x80 {
            h.push(len as u8);
        } else {
            let be: Vec<u8> = len.to_be_bytes().into_iter().skip_while(|b| *b == 0).collect();
            h.push(0x80 | be.len() as u8);
            h.extend(be);
        }
        h
    };
    // The content length of each level, the innermost (the empty `04 00`, 2 bytes) first.
    let mut inner = vec![2usize];
    for _ in 0..levels {
        let last = inner.last().copied().unwrap_or(2);
        inner.push(last + header(last).len());
    }
    let mut out = Vec::new();
    for len in inner.iter().rev().skip(1) {
        out.extend(header(*len));
    }
    out.extend([tag::OCTET_STRING, 0x00]);
    out
}

/// A ContentInfo holding SignedData whose encapsulated content is `econtent`. It has no
/// SignerInfo, so parsing always ends in an error: what matters is where.
fn cms_with_econtent(econtent: &[u8]) -> Vec<u8> {
    let encap = der::seq(&[&der::oid("1.2.840.113549.1.7.1"), &der::explicit(0, econtent)]);
    let signed_data = der::seq(&[&der::int(1), &der::tlv(tag::SET, &[]), &encap]);
    der::seq(&[&der::oid("1.2.840.113549.1.7.2"), &der::explicit(0, &signed_data)])
}

fn error_of(cms: &[u8]) -> String {
    SignedData::parse(cms).err().map(|e| e.to_string()).unwrap_or_default()
}

#[test]
fn deeply_nested_segmented_content_is_an_error_not_a_stack_overflow() {
    // Definite lengths nest without any end-of-contents scan to bound them, so a /Contents of a few
    // hundred KB can hold 100 000 levels. It used to be able to overflow the stack and abort.
    let deep = cms_with_econtent(&nested_octet_strings(100_000));
    assert!(deep.len() < 1 << 20, "a plausible /Contents");
    let why = error_of(&deep);
    assert!(why.contains("nested too deeply"), "{why}");
    // A few segments, as a BER signer writes them, are read: the error is the missing SignerInfo.
    let shallow = error_of(&cms_with_econtent(&nested_octet_strings(2)));
    assert!(shallow.contains("signerInfos"), "{shallow}");
}

#[test]
fn indefinite_length_nesting_bombs_are_refused() {
    // 10 000 indefinite-length SEQUENCEs: every level would have to be walked to find its end.
    let bomb: Vec<u8> = std::iter::repeat_n([tag::SEQUENCE, 0x80], 10_000).flatten().collect();
    assert!(SignedData::parse(&bomb).is_err());
    let in_content = cms_with_econtent(&bomb);
    assert!(SignedData::parse(&in_content).is_err());
}

#[test]
fn a_truncated_indefinite_length_cms_is_an_error() {
    // The end-of-contents octets are missing, as in a /Contents cut short.
    let cut = [0x30, 0x80, 0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x02];
    assert!(SignedData::parse(&cut).is_err());
}
