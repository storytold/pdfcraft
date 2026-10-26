//! Property tests for what the security handler does to *content*, and to the ciphertext a
//! stranger's file hands back.
//!
//! `src/tests.rs` covers this handler carefully with fixed vectors — including the `/O`, `/U` and
//! file keys qpdf 12.4.2 produced for real files — plus one property over malformed dictionaries.
//! What it does not vary is the *payload*. Upstream's encrypted round trips run three shapes: a
//! 30-byte text, a 1000-byte ramp, and a 10-byte attachment in `identity_crypt_filters_leave_data_alone`.
//! The shapes nobody walks are the ones that break CBC and readers: empty content, one block minus
//! one, a ciphertext cut short by a damaged `/Length`, and keys of a length `create` never builds.
//!
//! - plaintext survives encryption at every length, for each algorithm and each crypt filter;
//! - truncated or mutated ciphertext produces bytes rather than a panic (`AGENTS.md` §4: PdfCraft
//!   opens files from strangers, and encrypted streams in the wild do get truncated);
//! - object keys are scoped to their object, and AES-256's documented exception still holds;
//! - a file opens for exactly the passwords whose effective bytes match, stated as a rule over
//!   generated Unicode passwords instead of the three examples in `src/tests.rs` (the #1185 rule);
//! - the AES-CBC and RC4 primitives invert each other for any key length, since `/Length` and
//!   crypt filters from a damaged file reach them with shapes `create` would never build.

use pdfcraft_crypt::{
    Algorithm, Auth, CryptError, Method, NewEncryption, PADDING, SecurityHandler, StreamKind, aes_cbc_decrypt, aes_cbc_encrypt, create,
    effective_password, pdfdoc_byte, rc4,
};
use proptest::prelude::*;

/// The first element of `/ID`, which every R2–R4 derivation hashes in (Algorithm 2).
const ID: &[u8] = b"\x01\x23\x45\x67\x89\xab\xcd\xef\x0f\xed\xcb\xa9\x87\x65\x43\x21";

fn algorithms() -> impl Strategy<Value = Algorithm> {
    prop::sample::select(vec![Algorithm::Rc4_40, Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256])
}

/// Content at the lengths where CBC padding goes wrong, plus anything in between.
fn payloads() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        Just(Vec::new()),
        Just(vec![0x5cu8; 1]),
        Just(vec![0xa7u8; 15]),
        Just(vec![0x3fu8; 16]),
        Just(vec![0x9du8; 17]),
        Just(vec![0x10u8; 32]),
        prop::collection::vec(any::<u8>(), 1..96),
    ]
}

/// Passwords from an alphabet the two encodings disagree about: SASLprep deletes U+00AD, folds
/// U+FF21 to "A", U+FB01 to "fi" and U+3000 to a space (revisions 5–6), while PDFDocEncoding has
/// no byte for 'ğ' or CJK at all, which sends revisions 2–4 down their UTF-8 fallback.
const PASSWORD_CHARS: [char; 12] = ['a', 'B', '7', ' ', '-', '\u{AD}', '\u{FF21}', '\u{FB01}', 'ı', 'ğ', '日', '\u{A0}'];

fn passwords() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(PASSWORD_CHARS.to_vec()), 0..12).prop_map(|chars| chars.into_iter().collect())
}

/// `bytes` cut to 32 and padded with the standard padding string (Algorithm 2, step a).
fn padded(bytes: &[u8]) -> Vec<u8> {
    let mut out = PADDING;
    let n = bytes.len().min(32);
    out[..n].copy_from_slice(&bytes[..n]);
    out.to_vec()
}

/// Every byte string the handler may authenticate `pw` as. Revisions 2–4 use PDFDocEncoding, and
/// for a password holding characters it lacks they also try the raw UTF-8 bytes (qpdf writes such
/// passwords that way); revisions 5–6 use the SASLprep bytes. `effective_password` publishes the
/// first of these per algorithm, and it is what callers such as Protect check before encrypting.
fn candidates(algorithm: Algorithm, pw: &str) -> Vec<Vec<u8>> {
    let mut out = vec![effective_password(algorithm, pw)];
    if algorithm != Algorithm::Aes256 && pw.chars().any(|c| pdfdoc_byte(c).is_none()) {
        out.push(padded(&pw.as_bytes().iter().copied().take(32).collect::<Vec<u8>>()));
    }
    out
}

/// What `create` keys `/O` from: the owner password, except on revisions 2–4 with no owner
/// password given, where it falls back to the user password's bytes.
fn owner_target(algorithm: Algorithm, user: &str, owner: &str) -> Vec<u8> {
    if algorithm != Algorithm::Aes256 && owner.is_empty() { effective_password(algorithm, user) } else { effective_password(algorithm, owner) }
}

/// Which password `pw` is for this file. Owners are tried before users, so bytes that match both
/// authenticate as owner — the order viewers use. `None` means it opens nothing.
fn expected_auth(algorithm: Algorithm, user_target: &[u8], owner_target: &[u8], pw: &str) -> Option<Auth> {
    let tried = candidates(algorithm, pw);
    if tried.iter().any(|c| c == owner_target) {
        Some(Auth::Owner)
    } else if tried.iter().any(|c| c == user_target) {
        // An empty owner password is no permissions password: the user password opens as owner (#2956).
        Some(if candidates(algorithm, "").iter().any(|c| c == owner_target) { Auth::Owner } else { Auth::User })
    } else {
        None
    }
}

fn make(algorithm: Algorithm, user: &str, owner: &str) -> SecurityHandler {
    create(&NewEncryption { algorithm, user_password: user, owner_password: owner, permissions: -1, encrypt_metadata: true, seed: [0x5e; 32] }, ID)
        .unwrap()
}

proptest! {
    /// Encrypting then decrypting returns the input at every length, as a string and as each kind
    /// of stream — and is byte-for-byte reproducible, because re-running a save must not silently
    /// produce different bytes. Payloads of at least one block must come back changed: RC4 would
    /// need an all-zero keystream to leave them alone, and CBC cannot shrink a padded block.
    #[test]
    fn content_survives_the_trip_at_every_length(alg in algorithms(), data in payloads()) {
        let h = make(alg, "user", "owner");

        // The key width each algorithm promises: 5 bytes for export-grade 40-bit RC4, 16 for
        // AES-128 and RC4-128, 32 for AES-256. `key_len` is the only place that decides this and
        // nothing in either suite reads it back, so `if d.r == 2 { 5 }` becoming `{ 4 }` today
        // produces a file key of the wrong size that every test still calls fine.
        let want_len = match alg { Algorithm::Rc4_40 => 5, Algorithm::Aes128 | Algorithm::Rc4_128 => 16, Algorithm::Aes256 => 32 };
        prop_assert_eq!(h.file_key().len(), want_len, "{:?} must use a {} byte file key", alg, want_len);

        let h = make(alg, "user", "owner");
        for num in [0u32, 1, 4096, u32::MAX] {
            let generation = (num % 65_535) as u16;

            let enc = h.encrypt_string(num, generation, &data);
            prop_assert_eq!(h.decrypt_string(num, generation, &enc), &data[..], "string round trip at {} bytes ({:?})", data.len(), alg);
            prop_assert_eq!(h.encrypt_string(num, generation, &data), enc, "string encryption must be reproducible ({:?})", alg);

            for kind in [StreamKind::Normal, StreamKind::EmbeddedFile, StreamKind::Named(b"StdCF")] {
                let enc = h.encrypt_stream(num, generation, &data, kind);
                prop_assert_eq!(h.decrypt_stream(num, generation, &enc, kind), &data[..], "stream round trip at {} bytes ({:?}, {:?})", data.len(), alg, kind);
                prop_assert_eq!(h.encrypt_stream(num, generation, &data, kind), enc, "{:?} {:?} encryption must be reproducible", alg, kind);
            }

            if data.len() >= 16 {
                prop_assert_ne!(h.encrypt_string(num, generation, &data), &data[..], "one block left in plain sight ({:?})", alg);
            }
        }
    }

    /// Ciphertext of any length — truncated by a damaged `/Length`, padded out with garbage, or
    /// byte-flipped inside — must decrypt to bytes and never read past the end. AES discards a
    /// trailing partial block and leaves invalid padding in place rather than failing, so its
    /// output can only be a whole number of blocks minus at most one; RC4 keeps its length
    /// whatever the bytes are. An input shorter than the IV has nothing left to decode.
    #[test]
    fn damaged_ciphertext_never_panics(alg in algorithms(), enc in prop::collection::vec(any::<u8>(), 0..64)) {
        let h = make(alg, "user", "owner");
        for cut in 0..=enc.len() {
            let broken = &enc[..cut];
            let out = h.decrypt_string(7, 3, broken);
            if alg == Algorithm::Rc4_40 || alg == Algorithm::Rc4_128 {
                prop_assert_eq!(out.len(), cut, "RC4 must be length-preserving ({:?})", alg);
            } else {
                let blocks = cut.saturating_sub(16) / 16;
                prop_assert!(out.len() <= blocks * 16, "{:?}: {} bytes of ciphertext yielded {} decrypted bytes", alg, cut, out.len());
                prop_assert!(blocks * 16 - out.len() <= 16, "{:?}: padding removed more than one block", alg);
            }
        }

        // Flip every byte of a real ciphertext in turn: no position may crash the reader.
        let full = h.encrypt_string(7, 3, b"twenty-four bytes of PDF content");
        for i in 0..full.len() {
            let mut mutated = full.clone();
            mutated[i] ^= 0xFF;
            let _ = h.decrypt_string(7, 3, &mutated);
        }
    }

    /// A crypt filter name decides whether content is encrypted at all: PDF 2.0 permits
    /// `/StmF /Identity` with attachments still encrypted, and a name that appears in neither `/CF`
    /// nor as `Identity` resolves to Identity rather than failing. The reported answer and the
    /// bytes must agree, at every length including empty (where AES still adds a whole block).
    /// `/EFF` is set here so embedded files and normal streams must take *different* paths: the
    /// mix-up that leaves an attachment in plain sight, or encrypts a stream its own filter says
    /// should not be. And sometimes `/CF` carries an entry *named* `Identity` that claims
    /// `/CFM /AESV2`: a producer writing nonsense. The name wins and the bytes stay plain, since
    /// encrypting what both sides agreed is plaintext would write a file other readers cannot open.
    #[test]
    fn crypt_filters_decide_what_gets_encrypted(
        data in payloads(),
        name in prop::sample::select(vec!["Identity", "StdCF", "Nope"]),
        bogus_identity_in_cf in any::<bool>(),
    ) {
        let mut d = make(Algorithm::Aes256, "", "owner").dict().clone();
        d.str_f = name.as_bytes().to_vec();
        d.stm_f = b"Identity".to_vec();
        d.ef_f = b"StdCF".to_vec();
        if bogus_identity_in_cf {
            d.crypt_filters.push((b"Identity".to_vec(), Method::Aes256));
        }
        let h = SecurityHandler::open(d, ID, None).unwrap();

        // Only `StdCF` really encrypts; `Identity` does not even when `/CF` defines it, and a name
        // in neither place resolves to Identity rather than failing.
        let strings_encrypted = name == "StdCF";
        prop_assert_eq!(h.encrypts_strings(), strings_encrypted, "/StrF /{} reported {}", name, strings_encrypted);

        let enc = h.encrypt_string(3, 0, &data);
        if strings_encrypted {
            prop_assert_ne!(&enc, &data, "/StrF /{} left strings in plain sight", name);
            prop_assert!(enc.len() > data.len(), "AES always pads: {} bytes of ciphertext for {} plaintext", enc.len(), data.len());
            prop_assert_eq!(h.decrypt_string(3, 0, &enc), &data[..], "/StrF /{} round trip", name);
        } else {
            prop_assert_eq!(&enc, &data, "/StrF /{} must pass strings through", name);
        }

        // A normal stream follows `/StmF /Identity` here, whatever the string filter says.
        prop_assert_eq!(&h.encrypt_stream(3, 0, &data, StreamKind::Normal), &data[..], "normal streams follow /StmF Identity");
        // An embedded file follows `/EFF /StdCF`, so it is encrypted at every length — empty
        // included, where AES still emits a padded block — and comes back intact.
        let ef = h.encrypt_stream(3, 0, &data, StreamKind::EmbeddedFile);
        prop_assert_ne!(&ef, &data[..], "/EFF /StdCF left an embedded file in plain sight at {} bytes", data.len());
        prop_assert_eq!(h.decrypt_stream(3, 0, &ef, StreamKind::EmbeddedFile), &data[..], "embedded file round trip");
        // An unknown named filter is Identity, not an error; a known one uses its real method.
        prop_assert_eq!(&h.encrypt_stream(3, 0, &data, StreamKind::Named(b"Missing")), &data[..], "an unknown crypt filter must not encrypt");
        prop_assert_ne!(&h.encrypt_stream(3, 0, &data, StreamKind::Named(b"StdCF")), &data[..], "a filter named /StdCF must encrypt");
    }

    /// Object keys are scoped to the object (Algorithm 1, §7.6.3.3): RC4 and AES-128 fold the
    /// object number and generation into the key, so another object must not read these bytes back.
    /// AES-256 is the documented exception — revisions 5 and 6 use the file key for every object.
    /// The comparisons that decrypt under a foreign number carry the weight: wrong keystream bytes
    /// match the plaintext only by a chance of one in 256 per byte. Checking that *encryption*
    /// differs would prove less, because our initial vector already covers `/Num`.
    #[test]
    fn object_keys_are_scoped_to_their_object(alg in prop::sample::select(vec![Algorithm::Rc4_40, Algorithm::Rc4_128, Algorithm::Aes128]), data in prop::collection::vec(any::<u8>(), 8..40)) {
        let h = make(alg, "user", "owner");

        let enc = h.encrypt_string(100, 0, &data);
        prop_assert_eq!(h.decrypt_string(100, 0, &enc), &data[..], "{:?} reads its own object back", alg);
        prop_assert_ne!(h.decrypt_string(101, 0, &enc), &data[..], "{:?}: another object number must not decrypt these bytes", alg);
        prop_assert_ne!(h.decrypt_string(100, 1, &enc), &data[..], "{:?}: another generation must not decrypt these bytes", alg);

        // Algorithm 1 folds the LOW THREE bytes of `/Num` into the key. Two directions are pinned:
        // numbers that differ inside those three bytes must separate (a fold to `[..2]` would let
        // object 0 and object 65_536 share a key, silently handing one object's plaintext to another),
        // and numbers that differ only ABOVE them must share it — the fold is three bytes by design,
        // so a widening to four would break this and nothing else in the crate notices.
        let low = h.encrypt_string(0, 0, &data);
        prop_assert_ne!(&h.decrypt_string(65_536, 0, &low)[..], &data[..], "{:?}: /Num is folded three bytes wide; 0 and 65_536 must not share a key", alg);
        prop_assert_ne!(&h.decrypt_string(1 << 23, 0, &low)[..], &data[..], "{:?}: the third byte of /Num must still reach the key", alg);
        prop_assert_eq!(&h.decrypt_string(1 << 24, 0, &low)[..], &data[..], "{:?}: /Num is folded three bytes wide, so a difference above bit 23 shares the key by design", alg);

        let r6 = make(Algorithm::Aes256, "user", "owner");
        let enc6 = r6.encrypt_string(100, 0, &data);
        prop_assert_eq!(&r6.decrypt_string(101, 3, &enc6)[..], &data[..], "AES-256 uses the file key for every object (§7.6.3.3)");
        // ...which is exactly why the initial vector has to vary: revisions 5 and 6 reuse one key
        // for the whole file, so two objects holding the same bytes must still produce different
        // ciphertext. Identical bytes for a different `/Num` would be CBC initial-vector reuse —
        // the flaw that leaks structure in encrypted data.
        prop_assert_ne!(&r6.encrypt_string(101, 0, &data)[..], &enc6[..], "AES-256 reused the same initial vector for two objects");
        prop_assert_ne!(&r6.encrypt_string(100, 1, &data)[..], &enc6[..], "AES-256 reused the same initial vector across generations");
    }

    /// A file created for `user` and `owner` opens for exactly the passwords whose effective bytes
    /// equal one of theirs, and reports the role that match earned — owner first, since viewers try
    /// `/O` before `/U`. Everything else is `WrongPassword`: never an opening, and no panic on odd
    /// Unicode. This is the rule #1185 became (bytes decide, so a soft hyphen alone is no password
    /// and U+FF21 is "A"), stated for all inputs rather than the three examples in `src/tests.rs`.
    /// It also pins #1762's upgrade path: a user session gains owner rights on the owner password
    /// and on nothing else.
    #[test]
    fn authentication_accepts_exactly_the_matching_passwords(
        alg in algorithms(),
        mut user in passwords(),
        owner in prop::sample::select(vec!["owner".to_string(), String::new(), "Owner\u{AD}".into(), "\u{FF21}".into()]),
        extra in passwords(),
    ) {
        // `user` may have been grown past the 32-byte cut above; create with that spelling.
        // Grow the legacy password PAST the 32-byte cut BEFORE the file is built from it, so a
        // partner differing only beyond byte 32 is the same password to this file.
        if alg != Algorithm::Aes256 {
            user = format!("{user}{}", "abcdefghijklmnopqrstuvwxyz0123456789".repeat(2));
        }

        let h = make(alg, &user, &owner);
        let d = h.dict().clone();
        let user_bytes = effective_password(alg, &user);
        let owner_bytes = owner_target(alg, &user, &owner);

        // A collision engineered by construction, not by luck. Revisions 2-4 cut the password at
        // 32 bytes (Algorithm 2), so any spelling that agrees on the first 32 is ONE password;
        // revisions 5-6 run SASLprep, which deletes U+00AD. Growing past byte 32 guarantees the
        // partner is a different string with identical bytes at every draw — earlier drafts here
        // claimed `(` alone did it, which is false: appending "(" to a short password DISPLACES the
        // padding (pad("a") is [61,28,BF..], pad("a(") is [61,28,28,BF..]), and this alphabet draws
        // at most 11 characters, so chance produced zero legacy collisions in 1,566 measured cases.

        let mut suspects: Vec<String> = vec![user.clone(), owner.clone(), String::new()];
        // Near misses are PREPENDED: appending to a legacy password already past the cut would be
        // another collision rather than a refusal, which would silently thin the negative testing.
        let mut near_misses: Vec<String> = vec![format!("q{user}"), extra.clone()];
        if alg == Algorithm::Aes256 && !user.is_empty() {
            near_misses.push(format!("{user}q"));
        }

        let collision_partner = if alg == Algorithm::Aes256 { format!("{user}\u{AD}") } else { format!("{user}Z") };
        prop_assert_ne!(&collision_partner[..], &user[..], "the collision partner must be a different string");
        prop_assert_eq!(effective_password(alg, &collision_partner), effective_password(alg, &user), "{:?}: {:?} must become the same bytes as {:?}", alg, collision_partner, user);

        // The mistake itself, pinned so it cannot come back: a short legacy password plus "(" is NOT the
        // same password, because it shifts the padding by one byte instead of filling a slot.
        if alg != Algorithm::Aes256 {
            prop_assert_ne!(effective_password(alg, "a("), effective_password(alg, "a"), "appending ( to a short password displaces the padding; that is not a collision");
            prop_assert_eq!(effective_password(alg, &"a".repeat(31)), effective_password(alg, &format!("{}(", "a".repeat(31))), "at exactly 31 characters ( does land in the padding slot, and THAT is a collision");
        }

        suspects.extend(near_misses);
        suspects.push(collision_partner.clone());
        suspects.dedup();

        for suspect in &suspects {
            let want = expected_auth(alg, &user_bytes, &owner_bytes, suspect);
            match (want, SecurityHandler::open(d.clone(), ID, Some(suspect))) {
                (Some(role), Ok(got)) => {
                    prop_assert_eq!(got.auth(), role, "{:?}: {:?} should open as {:?}", alg, suspect, role);
                    prop_assert_eq!(got.file_key(), h.file_key(), "{:?}: whichever password opens the file, the key is the same", alg);
                }
                (Some(role), Err(e)) => prop_assert!(false, "{:?}: {:?} must open as {:?} but gave {:?}", alg, suspect, role, e),
                (None, Ok(got)) => prop_assert!(false, "{:?}: {:?} is not a password for this file, yet it opened as {:?}", alg, suspect, got.auth()),
                (None, Err(CryptError::WrongPassword)) => {}
                (None, Err(e)) => prop_assert!(false, "{:?}: {:?} gave {}, expected WrongPassword", alg, suspect, e),
            }

            // The owner password upgrades a user session; nothing else does. When the two share
            // bytes, `create` gave /O the user's own bytes (revisions 2–4) or left it empty
            // (AES-256, #1185's sharp edge), so the open password *is* an owner password here.
            if let Ok(session) = SecurityHandler::open(d.clone(), ID, Some(&user)) {
                let is_owner_password = expected_auth(alg, &user_bytes, &owner_bytes, suspect) == Some(Auth::Owner);
                let upgrade = session.authenticate_owner(ID, suspect).map(|u| u.auth());
                if is_owner_password {
                    prop_assert_eq!(upgrade, Ok(Auth::Owner), "{:?}: {:?} must unlock owner rights", alg, suspect);
                } else {
                    prop_assert_eq!(upgrade, Err(CryptError::WrongPassword), "{:?}: {:?} must not grant owner rights", alg, suspect);
                }
            }
        }

        // `None` means no password was offered: the handler tries the empty string, and nothing
        // else (a file with a real open password never opens silently).
        let want_none = expected_auth(alg, &user_bytes, &owner_bytes, "");
        match (want_none, SecurityHandler::open(d.clone(), ID, None)) {
            (Some(role), Ok(got)) => prop_assert_eq!(got.auth(), role, "{:?}: no password opened as {:?}", alg, got.auth()),
            (None, Ok(got)) => prop_assert!(false, "{:?}: the empty password is not valid, yet no password opened as {:?}", alg, got.auth()),
            (Some(role), Err(e)) => prop_assert!(false, "{:?}: the empty password should open as {:?} but gave {:?}", alg, role, e),
            (None, Err(CryptError::WrongPassword)) => {}
            (None, Err(e)) => prop_assert!(false, "{:?}: no password gave {}, expected WrongPassword", alg, e),
        }
    }

    /// The AES-CBC and RC4 primitives taken directly: a damaged file's `/Length` and crypt filters
    /// hand them keys and data of shapes `create` would never build, so they must invert each other
    /// for any key length (`Cipher::new` takes AES-256 at 32 bytes and zero-padded AES-128 below
    /// it, identically on both sides of the trip) and every payload length.
    #[test]
    fn primitives_are_invertible_for_any_key_and_length(
        key in prop::collection::vec(any::<u8>(), 1..40),
        data in payloads(),
        iv in prop::collection::vec(any::<u8>(), 16..24),
    ) {
        let iv: [u8; 16] = *iv.as_slice().first_chunk::<16>().expect("the iv strategy draws at least 16 bytes; a silent zero fallback would hide a generator change");

        let enc = aes_cbc_encrypt(&key, &iv, &data, true);
        prop_assert_eq!(aes_cbc_decrypt(&key, &iv, &enc, true), &data[..], "padded AES round trip at {} bytes", data.len());
        prop_assert!(enc.len() >= 16 && enc.len().is_multiple_of(16), "PKCS#5 makes whole blocks: {} bytes for {} plaintext", enc.len(), data.len());

        // Without padding, a partial block is zero-padded rather than refused, and extra trailing
        // bytes decrypt to more output instead of being dropped.
        let raw = aes_cbc_encrypt(&key, &iv, &data, false);
        prop_assert_eq!(raw.len(), data.len().div_ceil(16) * 16, "unpadded AES output is whole blocks");
        let mut longer = raw.clone();
        longer.extend_from_slice(b"0123456789abcdef");
        prop_assert_eq!(aes_cbc_decrypt(&key, &iv, &longer, false).len(), raw.len() + 16, "a whole extra block decrypts to a whole extra block");

        // RC4 undoes itself at every length; an empty key encrypts nothing, which is why the
        // handler never derives an empty object key.
        prop_assert_eq!(rc4(&key, &rc4(&key, &data)), &data[..], "RC4 round trip at {} bytes", data.len());
        prop_assert_eq!(rc4(&[], &data), &data[..], "an empty RC4 key passes bytes through");
    }

    /// `/Perms` certifies the permission bits and `/EncryptMetadata`: decrypted with the file key it
    /// must hold `/P` little-endian, four set bytes, 'T' or 'F', and the `adb` sentinel (Algorithm 10).
    /// Nothing else in this crate reads it — `cos` writes it out and no code takes it back apart — so
    /// without this property a flipped metadata byte would tell every other viewer the wrong thing about
    /// our own file while all our tests stayed green. The older revisions we create emit no `/Perms`
    /// at all, which is recorded here as well so a change in what goes out shows up rather than
    /// shipping quietly.
    #[test]
    fn perms_value_agrees_with_the_permissions_it_certifies(permissions in any::<i32>(), encrypt_metadata in any::<bool>()) {
        let h = create(
            &NewEncryption { algorithm: Algorithm::Aes256, user_password: "user", owner_password: "owner", permissions, encrypt_metadata, seed: [0x5e; 32] },
            ID,
        )
        .unwrap();

        prop_assert_eq!(h.dict().perms.len(), 16, "/Perms is one encrypted block");
        let open = aes_cbc_decrypt(h.file_key(), &[0u8; 16], &h.dict().perms, false);

        // Bits 1-2 clear and the reserved bits set (§7.6.4.2 Table 22) is what `/P` must say, and the
        // encrypted certificate has to repeat it byte for byte.
        let normalised = (permissions | !0xFFF | 0xC0) & !0x3;
        prop_assert_eq!(&open[..4], &normalised.to_le_bytes(), "/Perms must carry /P");
        prop_assert_eq!(h.dict().p, normalised, "the /P written must match the permission bits drawn");
        prop_assert_eq!(&open[4..8], &[0xFF; 4], "the four bytes following /P are set");
        prop_assert_eq!(open[8], if encrypt_metadata { b'T' } else { b'F' }, "/Perms must report /EncryptMetadata");
        prop_assert_eq!(&open[9..12], b"adb", "the adb sentinel must be present");

        // A user session's answers come from those same bits, not from the owner shortcut.
        let as_user = SecurityHandler::open(h.dict().clone(), ID, Some("user")).unwrap();
        prop_assert_eq!(as_user.permissions().print(), normalised & 0b100 != 0, "/P bit 3 decides printing");
        prop_assert_eq!(as_user.permissions().copy(), normalised & 0b1_0000 != 0, "/P bit 5 decides copying");
        prop_assert!(!as_user.permissions().owner, "the user password must not report owner rights");

        // Bytes 13-16 of /Perms are the non-repeating filler. Nothing reads them, so pinning that
        // they move with the seed is the only way to notice a producer that freezes them at zero.
        let other = create(
            &NewEncryption { algorithm: Algorithm::Aes256, user_password: "user", owner_password: "owner", permissions, encrypt_metadata, seed: [0x11; 32] },
            ID,
        )
        .unwrap();
        let open_other = aes_cbc_decrypt(other.file_key(), &[0u8; 16], &other.dict().perms, false);
        prop_assert_ne!(&open[12..16], &open_other[12..16], "the four filler bytes of /Perms must follow the seed, not sit constant");

        // Revisions below 5 carry no /Perms; pinned so the boundary of what we emit is explicit.
        for older in [Algorithm::Rc4_40, Algorithm::Rc4_128, Algorithm::Aes128] {
            let legacy = make(older, "user", "owner");
            prop_assert!(legacy.dict().perms.is_empty(), "{:?} must emit no /Perms", older);
        }
    }
    /// Revisions 4 mix four set bytes into every file key when `/EncryptMetadata` is false (Algorithm
    /// 2, step f), so the same password over the same `/ID` must produce a *different* key than when
    /// metadata is encrypted. Every other test in this crate builds `encrypt_metadata: true` only,
    /// which is exactly why that step is reachable but never walked: switching it off entirely changed
    /// nothing any test could see. This also records the rule that revisions below 4 always encrypt
    /// metadata whatever was asked, so the boundary between the two is named rather than assumed.
    #[test]
    fn metadata_flag_changes_the_key_it_claims_to_change(data in payloads()) {
        let build = |encrypt_metadata: bool| {
            create(&NewEncryption { algorithm: Algorithm::Aes128, user_password: "user", owner_password: "owner", permissions: -1, encrypt_metadata, seed: [0x5e; 32] }, ID).unwrap()
        };
        let (yes, no) = (build(true), build(false));

        // Algorithm 2, step f: revisions 4 hash four set bytes into every key when metadata stays
        // plain, so two files with identical passwords and the same /ID cannot share a key.
        prop_assert_ne!(yes.file_key(), no.file_key(), "R4 must fold four set bytes into the key when /EncryptMetadata is false (Algorithm 2, step f)");
        prop_assert_eq!(no.dict().encrypt_metadata, false, "the flag must reach the dictionary");
        prop_assert!(!no.encrypts_metadata(), "/EncryptMetadata false must report itself honestly");
        prop_assert!(yes.encrypts_metadata(), "and true must report true");

        // Both still open on the same password and round trip at every length: a derivation that
        // drifted would show up as an unreadable file rather than passing quietly.
        for h in [&yes, &no] {
            let open = SecurityHandler::open(h.dict().clone(), ID, Some("user")).unwrap();
            prop_assert_eq!(open.file_key(), h.file_key(), "the user password must still derive this file's key");
            let enc = h.encrypt_string(7, 0, &data);
            prop_assert_eq!(&open.decrypt_string(7, 0, &enc)[..], &data[..], "round trip at {} bytes", data.len());
        }

        // And the two files must not read each other's content: different keys, different plaintext.
        if data.len() >= 8 {
            prop_assert_ne!(&yes.decrypt_string(7, 0, &no.encrypt_string(7, 0, &data))[..], &data[..], "a key built for plain metadata must not decrypt a file whose keys encrypt it");
        }

        // Revisions below 4 always encrypt metadata, whatever the caller asked for.
        let legacy = create(&NewEncryption { algorithm: Algorithm::Rc4_128, user_password: "user", owner_password: "owner", permissions: -1, encrypt_metadata: false, seed: [0x5e; 32] }, ID).unwrap();
        prop_assert!(legacy.dict().encrypt_metadata && legacy.encrypts_metadata(), "V<4 always encrypts metadata, as create() documents");
    }

}
