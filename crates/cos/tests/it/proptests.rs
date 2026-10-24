//! Property tests for `pdfcraft-cos` save fidelity — proptest coverage of two areas raised in
//! #1187. **This does not close #1187.** That issue asks for five Hegel properties; this adds
//! save/reopen and edit-history coverage using `proptest`, which is already a dev-dependency of this
//! crate (`Cargo.toml:20`), so no new dependency is needed to review it. Left for follow-ups:
//! #1187's reference-following comparison, its snapshot/undo state machine, the password property (the
//! one whose pilot found #1185) and its damaged-file byte splices. The object-level serialise → parse
//! round trip it also lists already exists inline in `src/writer.rs`.
//!
//! Covered:
//!
//! 1. **A full save keeps every value it wrote into the catalog**, in both output styles (objects
//!    packed into object streams, §7.5.7; the classic cross-reference table otherwise), keeps the
//!    catalog's own structure intact (`/Type /Catalog`, and a `/Pages` node whose `/Kids` array and
//!    `/Count` agree — the fixture's tree is empty, so this pins shape and agreement, not rendered
//!    pages), preserves dictionary
//!    key order (which `README.md:75` promises is byte-stable), emits spec-shaped §7.5.4 cross-reference
//!    rows, and reopens with nothing to repair.
//! 2. **An incremental save is a strict append**: the original bytes come out untouched at the front
//!    (§7.5.6), the file keeps two revisions whose first still points at the base cross-reference and
//!    opens standalone, the edit is visible while every existing value stays put in order, and it
//!    reopens with nothing to repair.
//! 3. **A write/delete sequence equals a plain map**, checked after every step and again once saved and
//!    reopened. A deleted key must stay gone — not return its old value, not read as null either.
//!    Deletes are generated against keys that were just written, so the delete path cannot be skipped.
//!
//! ## Scope, stated honestly
//! Generated values are pure data — no streams and no indirect references — so this says nothing about
//! stream payloads or the renumbering a full save legitimately performs. Strings are compared as bytes,
//! so literal ↔ hexadecimal re-emission is not treated as a change (§7.3.6); the *choice* of spelling
//! is therefore not asserted, and `PdfString.hex` is not pinned here. Dictionary keys at the catalog
//! level *are* compared in order, so reversing them fails; nested dictionary order inside a value is
//! sorted away by [`canon`] and is not pinned. Generated reals stay within ±1e6: `write_real`
//! (`src/writer.rs:46`) turns a non-finite real into `0` and an integral real below 1e15 into integer
//! syntax, so [`canon`] applies the writer's documented normalisation to **both** sides rather than
//! forgiving a mismatch afterwards.

use std::sync::Arc;

use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

use pdfcraft_cos::{Dict, Document, Object, PdfString, SaveOptions, write_full, write_incremental};

/// The prefix every generated key carries. `Document::new_empty` puts `/Type` and `/Pages` in the
/// catalog; property 1 asserts those two structurally rather than ignoring them.
const KEY: &str = "px";

/// Generated keys go into the catalog in this deliberately unsorted sequence. Ascending insertion would make
/// "preserves dictionary key order" and "every dictionary sorted by key" the same observable, so a writer
/// that silently sorted would rewrite every saved byte and stay green. This scramble catches sorting of the
/// keys it names; it does NOT on its own pin where structural entries like `/Type` sit, because `saved`
/// filters to the `px` prefix - that gap is closed separately by [`catalog_keys`] asserting the full
/// sequence. Nested dictionary order is deliberately not pinned (see `canon`). Every permutation of the
/// generated keys is NOT drawn here either: one fixed order per case, chosen to make sorting detectable.
const INSERTION_ORDER: [usize; 8] = [5, 0, 7, 2, 6, 1, 4, 3];

/// A value reduced to a comparable spelling, with the writer's documented normalisations applied.
fn canon(o: &Object) -> String {
    match o {
        Object::Null => "null".into(),
        Object::Bool(b) => format!("b{b}"),
        Object::Int(i) => format!("i{i}"),
        Object::Real(r) => {
            if !r.is_finite() {
                "i0".into()
            } else if r.fract() == 0.0 && r.abs() < 1e15 {
                format!("i{}", *r as i64)
            } else {
                format!("r{r}")
            }
        }
        Object::Name(n) => format!("n{}", String::from_utf8_lossy(n)),
        // Bytes, because a literal may be re-emitted as hexadecimal or the reverse; both spellings
        // denote the same bytes (§7.3.6). The `hex` flag itself is deliberately not compared — see
        // the scope note at the top.
        Object::String(s) => format!("s{}", s.bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()),
        Object::Array(a) => format!("[{}]", a.iter().map(canon).collect::<Vec<_>>().join(",")),
        Object::Dict(d) => {
            // Sorted inside a value: nested key order is not pinned here. Catalog-level order is, and
            // it is asserted in iteration order by `saved`.
            let mut pairs: Vec<(String, String)> = d.iter().map(|(k, v)| (String::from_utf8_lossy(k).into_owned(), canon(v))).collect();
            pairs.sort();
            format!("{{{}}}", pairs.iter().map(|(k, v)| format!("{k}=>{v}")).collect::<Vec<_>>().join(","))
        }
        // Unreachable under the generators below, which build no streams and no references. Kept total
        // so this compiles if someone widens them — at which point it should compare decoded bytes and
        // resolved targets instead of a constant.
        Object::Stream(_) => "stream".into(),
        Object::Ref(r) => format!("ref{r}"),
    }
}

fn leaf() -> impl Strategy<Value = Object> {
    prop_oneof![
        Just(Object::Null),
        any::<bool>().prop_map(Object::Bool),
        (-1_000_000i64..1_000_000i64).prop_map(Object::Int),
        (-1e6f64..1e6f64).prop_map(Object::Real),
        // Non-finite reals, drawn explicitly. `write_real` (`src/writer.rs:46`) replaces them with
        // `0`, and [`canon`] spells both sides that way, so this is what makes the normalisation
        // described in the module header actually exercised rather than merely asserted in prose.
        prop::sample::select(vec![f64::NAN, f64::INFINITY, f64::NEG_INFINITY]).prop_map(Object::Real),
        // Raw bytes, including ones a literal string must escape and the NUL byte.
        prop::collection::vec(any::<u8>(), 0..12).prop_map(|b| Object::String(PdfString::literal(b))),
        "[A-Za-z][A-Za-z0-9]{0,6}".prop_map(|s| Object::name(&s)),
    ]
}

/// Nested values to a bounded depth, so generation cannot recurse without limit.
fn value(depth: usize) -> BoxedStrategy<Object> {
    if depth == 0 {
        return leaf().boxed();
    }
    prop_oneof![
        leaf().boxed(),
        prop::collection::vec(value(depth - 1), 0..5).prop_map(Object::Array).boxed(),
        prop::collection::btree_map("[a-z]{1,4}", value(depth - 1), 0..5)
            .prop_map(|m| {
                let mut d = Dict::new();
                for (k, v) in m {
                    d.set(k.into_bytes(), v);
                }
                Object::Dict(d)
            })
            .boxed(),
    ]
    .boxed()
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Every catalog key, in the dictionary's own order - structural entries included. `saved` filters to
/// `px*`, which is why sorting of generated keys is visible there but `/Type` moving to the end was not;
/// this covers that. Read from memory before a save and from the parsed file afterwards, so the two sides
/// share only `Dict` iteration, not the serialiser under test.
fn catalog_keys(doc: &Document) -> Vec<String> {
    let root = match doc.root() {
        Some(r) => r,
        None => return Vec::new(),
    };
    match doc.get(root).as_dict() {
        Some(d) => d.iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).collect(),
        None => Vec::new(),
    }
}

/// The catalog's `px*` entries in **iteration order**, so a reordered save fails.
fn saved(doc: &Document) -> Result<Vec<(String, String)>, TestCaseError> {
    let root = doc.root().ok_or_else(|| TestCaseError::fail("no catalog in the saved file"))?;
    let cat = doc.get(root);
    let d = match cat.as_dict() {
        Some(d) => d,
        None => return Err(TestCaseError::fail("catalog is not a dictionary")),
    };
    Ok(d.iter().filter(|(k, _)| k.starts_with(KEY.as_bytes())).map(|(k, v)| (String::from_utf8_lossy(k).into_owned(), canon(v))).collect())
}

/// The catalog's structure, which a reader needs whether or not it holds our test keys.
fn assert_catalog_structure(doc: &Document, style: &str) -> Result<(), TestCaseError> {
    let root = doc.root().ok_or_else(|| TestCaseError::fail(format!("no catalog after the {style} save")))?;
    let cat = doc.get(root);
    let d = match cat.as_dict() {
        Some(d) => d,
        None => return Err(TestCaseError::fail(format!("catalog is not a dictionary after the {style} save"))),
    };
    prop_assert_eq!(d.name(b"Type"), Some(&b"Catalog"[..]), "catalog /Type changed on the {} save", style);
    let pages = match d.get(b"Pages") {
        Some(p) => p,
        None => return Err(TestCaseError::fail(format!("catalog lost /Pages on the {style} save"))),
    };
    let resolved = doc.resolve(pages);
    let pd = match resolved.as_dict() {
        Some(d) => d,
        None => return Err(TestCaseError::fail(format!("/Pages does not resolve to a dictionary on the {style} save"))),
    };
    prop_assert_eq!(pd.name(b"Type"), Some(&b"Pages"[..]), "/Pages /Type changed on the {} save", style);
    let kids = match pd.get(b"Kids").and_then(Object::as_array) {
        Some(k) => k,
        None => return Err(TestCaseError::fail(format!("/Pages /Kids is missing or not an array on the {style} save"))),
    };
    // /Count has to agree with /Kids. A page tree whose count lies still opens, then misnumbers every
    // page downstream, so checking presence alone would not be enough. This fixture's tree is empty, so
    // what is pinned here is shape and agreement — not the existence of rendered pages.
    prop_assert_eq!(pd.int(b"Count"), Some(kids.len() as i64), "/Pages /Count disagrees with its {} kids on the {} save", kids.len(), style);
    Ok(())
}

/// §7.5.4: every entry of a classic cross-reference table is exactly 20 bytes, closed by a two-byte
/// end of line. Unpadded offsets still open in lenient readers and corrupt strict ones, so the byte
/// shape is asserted rather than inferred from a successful reopen.
fn assert_classic_xref_rows(bytes: &[u8]) -> Result<(), TestCaseError> {
    // Anchored on the file's own startxref (§7.5.3) rather than the first "xref" text in the buffer:
    // a generated document name could otherwise point the scan at prose. A startxref that does not land
    // on the `xref` keyword means a cross-reference stream, which has no table to check here.
    let Some(xoff) = startxref(bytes) else { return Err(TestCaseError::fail("no readable startxref in a classic-table save")) };
    let x = xoff as usize;
    if bytes.get(x..x + 4) != Some(&b"xref"[..]) {
        return Ok(());
    }
    let nl = match bytes[x..].iter().position(|b| *b == b'\n') {
        Some(n) => n,
        None => return Err(TestCaseError::fail("xref keyword not followed by a newline")),
    };
    let rest = &bytes[x + nl + 1..];
    let end = match find(rest, b"trailer") {
        Some(e) => e,
        None => return Err(TestCaseError::fail("cross-reference table with no trailer")),
    };
    let mut rows = 0usize;
    let mut expected_rows = 0usize;
    for line in rest[..end].split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        // A CR before the line feed belongs to the two-byte terminator (§7.5.4), not to the keyword:
        // stripped here so a CR+LF row is still recognised as a legal row rather than reported (wrongly)
        // as "no cross-reference rows found". Producers do emit CR+LF terminators.
        //
        // What catches what: classification keys off the keyword, so a row whose keyword is garbage never
        // reaches the width and length asserts below. Those rows are named by the `else` arm at the end
        // of this loop; the row-count compare after it is only a backstop. The length assert is not the
        // safety net for malformed keywords, and this comment used to imply that it was.
        let fields: Vec<&[u8]> = line
            .split(|b| *b == b' ')
            .filter(|f| !f.is_empty())
            .map(|f| match f.last() {
                Some(b'\r') => &f[..f.len() - 1],
                _ => f,
            })
            .collect();
        if fields.len() == 2 && fields.iter().all(|f| f.iter().all(u8::is_ascii_digit)) {
            // `<first> <count>` subsection header.
            expected_rows += std::str::from_utf8(fields[1]).ok().and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
            continue;
        }
        if fields.len() == 3 && matches!(fields[2], b"n" | b"f") {
            // Classified by keyword, not by width: an unpadded row must still be recognised as a row, so
            // the failure names the real crime (wrong width) instead of the misleading "no rows found".
            // Both keywords are legal — `n` in use, `f` free (the head of the free chain carries
            // generation 65535). §7.5.4 fixes each field's width and the whole entry at 20 bytes.
            prop_assert_eq!(
                fields[0].len(),
                10,
                "a cross-reference offset must be 10 digits (§7.5.4), got {:?} in row {:?}",
                String::from_utf8_lossy(fields[0]),
                String::from_utf8_lossy(line)
            );
            prop_assert_eq!(
                fields[1].len(),
                5,
                "a cross-reference generation must be 5 digits (§7.5.4), got {:?} in row {:?}",
                String::from_utf8_lossy(fields[1]),
                String::from_utf8_lossy(line)
            );
            // Exactly 20 bytes per entry in the file (§7.5.4). Both legal terminators land on 19 bytes
            // for the line as split here: SP+LF, which is what this writer emits today, and CR+LF, whose
            // terminator is the pair itself. A 21-byte entry (SP+CRLF, or a doubled CR) fails here.
            prop_assert_eq!(
                line.len(),
                19,
                "a cross-reference row is {} bytes; §7.5.4 wants exactly 20 in the file (SP+LF or CR+LF): {:?}",
                line.len(),
                String::from_utf8_lossy(line)
            );
            rows += 1;
        } else {
            // Inside the table region, a line is either a subsection header or an entry. Anything else is
            // named here with its bytes quoted, instead of surfacing later as a bare row-count mismatch
            // ("no cross-reference rows found") that blames the table for one bad line.
            prop_assert_eq!(
                fields.len(),
                3,
                "unrecognised line in the cross-reference table, want `<offset> <generation> n|f`: {:?}",
                String::from_utf8_lossy(line)
            );
            prop_assert!(
                matches!(fields[2], b"n" | b"f"),
                "illegal cross-reference keyword {:?} in row {:?}",
                String::from_utf8_lossy(fields[2]),
                String::from_utf8_lossy(line)
            );
        }
    }
    prop_assert!(rows > 0, "no cross-reference rows found in a classic-table save");
    prop_assert_eq!(rows, expected_rows, "the subsection headers promised {} rows, the table holds {}", expected_rows, rows);
    Ok(())
}

/// The `startxref` value of a file, which the next incremental update must point back at via `/Prev`.
/// The keyword is followed by whitespace, then the offset (§7.5.3), so leading whitespace is skipped.
fn startxref(bytes: &[u8]) -> Option<u64> {
    let at = find(bytes, b"startxref")?;
    let tail = &bytes[at + 9..];
    let digits: String = tail.iter().skip_while(|b| b.is_ascii_whitespace()).take_while(|b| b.is_ascii_digit()).map(|b| char::from(*b)).collect();
    digits.parse().ok()
}

proptest! {
    // 256 cases per property; the suite runs in well under a second, so coverage is free here.
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// 1. A full save keeps the values it wrote, their order, the catalog's structure and the shape of
    /// its cross-reference table — in both output styles — and reopens with nothing to repair.
    #[test]
    fn full_save_keeps_the_catalog_intact(map in prop::collection::btree_map(0usize..8, value(3), 1..6)) {
        let mut doc = Document::new_empty();
        let root = doc.root().expect("an empty document has a catalog");
        doc.update_dict(root, |d| {
            for i in INSERTION_ORDER.into_iter().filter(|&i| map.contains_key(&i)) {
                d.set(format!("{KEY}{i}").into_bytes(), map[&i].clone());
            }
        })
        .map_err(|e| TestCaseError::fail(format!("cannot write the generated catalog: {e}")))?;
        // Insertion order is this scrambled sequence filtered to the drawn keys: what must come back.
        let expected: Vec<(String, String)> = INSERTION_ORDER.into_iter().filter(|&i| map.contains_key(&i)).map(|i| (format!("{KEY}{i}"), canon(&map[&i]))).collect();

        // Full key sequence taken from memory BEFORE any byte is written - /Type and /Pages included.
        let order_before = catalog_keys(&doc);

        // Right before any save, or the later assertion would prove nothing about saving.
        prop_assert_eq!(saved(&doc)?, expected.clone(), "the in-memory catalog is wrong before saving");

        for object_streams in [true, false] {
            let style: &str = if object_streams { "object-stream" } else { "classic-table" };
            let opts = SaveOptions { object_streams, ..SaveOptions::default() };
            let bytes = write_full(&doc, &opts).map_err(|e| TestCaseError::fail(format!("full save failed: {e}")))?;

            if !object_streams {
                assert_classic_xref_rows(&bytes).map_err(|e| TestCaseError::fail(format!("{style}: {e}")))?;
            }

            let again = Document::open(Arc::new(bytes)).map_err(|e| TestCaseError::fail(format!("our own full save must reopen: {e}")))?;

            prop_assert!(again.repair_log().is_empty(), "a file we wrote must not need repair ({}); log: {:?}", style, again.repair_log());
            // Values *and* order: README.md promises dictionaries keep their key order.
            prop_assert_eq!(saved(&again)?, expected.clone(), "the catalog changed on a {} save (values or order)", style);
            // Structural position too: /Type and /Pages must not drift to the end or swap places.
            prop_assert_eq!(catalog_keys(&again), order_before.clone(), "the catalog's full key sequence changed on a {} save", style);
            assert_catalog_structure(&again, style)?;
        }
    }

    /// 2. An incremental save leaves the original bytes untouched, chains back through `/Prev`, and
    /// reopens carrying every intended value plus the edit. The base is built **both** ways -
    /// cross-reference stream and classic table - because the two are serialised by different code, and a
    /// path that never runs cannot fail here. Expected contents are computed from the values we put in,
    /// not read back out of the writer: comparing against our own earlier output would pass a writer that
    /// is consistently wrong, which is the one thing a fidelity test must not do.
    #[test]
    fn incremental_save_appends_without_touching_the_original(map in prop::collection::btree_map(0usize..8, value(2), 1..5), marker in -1_000_000i64..1_000_000i64) {
        let mut doc = Document::new_empty();
        let root = doc.root().expect("an empty document has a catalog");
        doc.update_dict(root, |d| {
            for i in INSERTION_ORDER.into_iter().filter(|&i| map.contains_key(&i)) {
                d.set(format!("{KEY}{i}").into_bytes(), map[&i].clone());
            }
        })
        .map_err(|e| TestCaseError::fail(format!("cannot write the generated catalog: {e}")))?;

        // Our own record of what the catalog should hold, derived from the inputs before any byte is
        // written. The edit key joins it below; order follows INSERTION_ORDER, not sorted order (§7.5.4).
        let intended: Vec<(String, String)> = INSERTION_ORDER.into_iter().filter(|&i| map.contains_key(&i)).map(|i| (format!("{KEY}{i}"), canon(&map[&i]))).collect();

        for object_streams in [true, false] {
            let style: &str = if object_streams { "object-stream" } else { "classic-table" };
            let opts = SaveOptions { object_streams, ..SaveOptions::default() };

            let base = write_full(&doc, &opts).map_err(|e| TestCaseError::fail(format!("{}: full save failed: {e}", style)))?;
            let base_xref = startxref(&base).ok_or_else(|| TestCaseError::fail(format!("{}: the base file has no readable startxref", style)))?;

            let mut again = Document::open(Arc::new(base.clone())).map_err(|e| TestCaseError::fail(format!("{}: the saved file must reopen: {e}", style)))?;
            // Independent of the writer's own readings: the reopen must show what we intended to write.
            prop_assert_eq!(saved(&again)?, intended.clone(), "{}: the base catalog differs from what we put in", style);

            let root2 = again.root().expect("the reopened document has a catalog");
            again.update_dict(root2, |d| d.set(b"pxMark".to_vec(), Object::Int(marker))).map_err(|e| TestCaseError::fail(format!("{}: cannot apply the incremental edit: {e}", style)))?;

            let bytes = write_incremental(&again, &opts).map_err(|e| TestCaseError::fail(format!("{}: incremental save failed: {e}", style)))?;
            prop_assert!(bytes.len() > base.len(), "{}: an incremental save must add bytes ({} over a base of {})", style, bytes.len(), base.len());
            prop_assert_eq!(bytes.get(..base.len()), Some(&base[..]), "{}: an incremental save must not rewrite the original bytes", style);

            let out = Document::open(Arc::new(bytes.clone())).map_err(|e| TestCaseError::fail(format!("{}: the incremental file must reopen: {e}", style)))?;
            prop_assert!(out.repair_log().is_empty(), "{}: an increment we wrote must not need repair; log: {:?}", style, out.repair_log());

            // The revision chain in the crate's own terms (Revision { xref_offset, is_stream }): two
            // revisions, the first still pointing at the base cross-reference. Without that link no other
            // reader can reach the earlier revision's objects, which is the classic incremental corruption.
            // /Prev sits in the trailer for a table base and inside the cross-reference stream for a
            // stream base, so it is not read here; `revisions` covers both (§7.5.3-7.5.6).
            let revs = out.revisions();
            prop_assert_eq!(revs.len(), 2, "{}: an incremental save must leave two revisions: {:?}", style, revs);
            prop_assert_eq!(revs[0].xref_offset, base_xref, "{}: the first revision must still point at the base xref {}", style, base_xref);
            prop_assert!(revs[1].xref_offset > base_xref, "{}: the second revision must sit after the first: {:?}", style, revs);
            let ends = out.revision_ends();
            prop_assert_eq!(ends.last().copied(), Some(bytes.len()), "{}: the last revision must run to the end of the file: {:?}", style, ends);

            // The base revision opens on its own. "Nothing was rewritten" buys the user this: cut the file
            // at the append point and the earlier document still reads, with our intended values.
            if let Some(end0) = ends.first().copied() {
                let before_doc = Document::open(Arc::new(bytes[..end0].to_vec())).map_err(|e| TestCaseError::fail(format!("{}: the base revision must open alone: {e}", style)))?;
                prop_assert_eq!(saved(&before_doc)?, intended.clone(), "{}: the base revision lost or changed values when opened without the increment", style);
            }

            assert_catalog_structure(&out, style)?;

            let mut expected = intended.clone();
            expected.push(("pxMark".into(), canon(&Object::Int(marker))));
            prop_assert_eq!(saved(&out)?, expected.clone(), "{}: the increment lost a value, reordered the catalog, or dropped the edit", style);
        }
    }

    /// 3. Writes then deletes against a plain ordered model, checked after every step and again after
    /// save and reopen. Deletes always target a key that exists, so the delete path is never empty.
    #[test]
    fn edit_sequence_equals_a_plain_map(plan in prop::collection::btree_map(0usize..8, value(2), 1..6).prop_flat_map(|sets| {
        let keys: Vec<usize> = sets.keys().copied().collect();
        (Just(sets), prop::collection::vec(0usize..keys.len(), 1..=keys.len())).prop_map(move |(s, picks)| {
            let mut dels: Vec<usize> = picks.into_iter().map(|i| keys[i]).collect();
            // Deduplicated: the same key drawn twice would make the second delete target a key that is
            // already gone, which is legal PDF but not what this property means to assert. `picks` draws
            // 1..=keys.len() over a non-empty key set, so `dels` cannot come back empty — the guarantee
            // lives in the generator, not in an assume that could never fire.
            dels.sort_unstable();
            dels.dedup();
            (s, dels)
        })
    })) {
        let (sets, deletes) = plan;
        let mut doc = Document::new_empty();
        let root = doc.root().expect("an empty document has a catalog");

        let mut model: Vec<(String, String)> = Vec::new();
        for (i, o) in &sets {
            let key = format!("{KEY}{i}");
            doc.update_dict(root, |d| d.set(key.clone().into_bytes(), o.clone())).map_err(|e| TestCaseError::fail(format!("cannot write {key}: {e}")))?;
            model.push((key.clone(), canon(o)));
            prop_assert_eq!(saved(&doc)?, model.clone(), "after writing {} the catalog is wrong before saving", key);
        }

        for i in &deletes {
            let key = format!("{KEY}{i}");
            prop_assert!(model.iter().any(|(k, _)| k == &key), "the generator tried to delete a key that was never written: {}", key);
            doc.update_dict(root, |d| {
                d.remove(key.as_bytes());
            })
            .map_err(|e| TestCaseError::fail(format!("cannot delete {key}: {e}")))?;
            model.retain(|(k, _)| k != &key);
            prop_assert_eq!(saved(&doc)?, model.clone(), "after deleting {} it is still reachable", key);
        }

        let bytes = write_full(&doc, &SaveOptions::default()).map_err(|e| TestCaseError::fail(format!("full save failed: {e}")))?;
        let again = Document::open(Arc::new(bytes)).map_err(|e| TestCaseError::fail(format!("our own save must reopen: {e}")))?;
        let got = saved(&again)?;
        prop_assert_eq!(got.clone(), model.clone(), "the reopened catalog differs from the edit history (values or order)");
        for i in 0..8usize {
            let key = format!("{KEY}{i}");
            if !model.iter().any(|(k, _)| k == &key) {
                prop_assert!(!got.iter().any(|(g, _)| *g == key), "a deleted key came back after save+reopen: {}", key);
            }
        }
    }
}
