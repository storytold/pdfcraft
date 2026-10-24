//! A recipient is an address, never a `mailto:` link: `xdg-email` reads a `mailto:` argument as the
//! whole message, so its `bcc` and `attach` fields would be taken from the recipient.

use std::path::PathBuf;

use pdfcraft_platform::mail::{Compose, MailError, check};

/// A file that exists, so the check gets as far as the recipients.
fn compose_to(to: &[&str]) -> Compose {
    let path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    Compose { path, subject: String::new(), body: String::new(), to: to.iter().map(|a| a.to_string()).collect() }
}

#[test]
fn a_mailto_uri_recipient_is_refused() {
    for bad in ["mailto:x@y?bcc=evil@z&attach=/home/u/.ssh/id_rsa", "MAILTO:x@y", "MailTo:x@y", "  mailto:x@y?bcc=evil@z", "\tmailto:x@y"] {
        assert_eq!(check(&compose_to(&[bad])), Err(MailError::BadRecipient(bad.into())), "{bad:?}");
    }
    // One bad recipient among good ones still refuses the message.
    assert!(matches!(check(&compose_to(&["a@example.org", "mailto:b@example.org"])), Err(MailError::BadRecipient(_))));
}

#[test]
fn ordinary_recipients_are_still_accepted() {
    // An address that merely contains the word, and ones too short to hold the prefix.
    assert_eq!(check(&compose_to(&["a@example.org", "mailto@example.org", "ñ", "m", "é@example.org"])), Ok(()));
    assert_eq!(check(&compose_to(&[])), Ok(()));
}
