use super::*;

#[test]
fn operators_keep_operands_and_spans() {
    let src = b"q 1 0 0 1 72 720 cm BT /F1 12 Tf (Hello \\(world\\)) Tj [(A) -120 (B)] TJ ET Q";
    let p = parse(src);
    assert_eq!(p.skipped, 0);
    let names: Vec<&[u8]> = p.ops.iter().map(|o| o.op.as_slice()).collect();
    assert_eq!(names, [&b"q"[..], b"cm", b"BT", b"Tf", b"Tj", b"TJ", b"ET", b"Q"]);
    let cm = &p.ops[1];
    assert_eq!(cm.nums::<6>(), Some([1.0, 0.0, 0.0, 1.0, 72.0, 720.0]));
    assert_eq!(&src[cm.span.clone()], b"1 0 0 1 72 720 cm");
    assert_eq!(p.ops[4].operands[0].as_string().unwrap().bytes, b"Hello (world)");
    assert_eq!(p.ops[5].operands[0].as_array().unwrap().len(), 3);
    assert_eq!(p.ops[3].name(0), Some(&b"F1"[..]));
}

#[test]
fn round_trips_through_the_serializer() {
    let src = b"0.5 g 10 10 100 50 re f /GS1 gs true false null 3 d0 [3 2] 0 d (x) ' 1 2 (y) \"";
    let a = parse(src);
    let b = parse(&serialize_ops(&a.ops));
    assert_eq!(a.ops.iter().map(|o| (&o.op, &o.operands)).collect::<Vec<_>>(), b.ops.iter().map(|o| (&o.op, &o.operands)).collect::<Vec<_>>());
}

#[test]
fn inline_images_stay_whole() {
    let mut src = b"q 10 0 0 10 0 0 cm BI /W 2 /H 2 /CS /G /BPC 8 ID ".to_vec();
    src.extend_from_slice(&[0, b'E', b'I', 255]);
    src.extend_from_slice(b"\nEI Q 1 g");
    let p = parse(&src);
    let names: Vec<&[u8]> = p.ops.iter().map(|o| o.op.as_slice()).collect();
    assert_eq!(names, [&b"q"[..], b"cm", b"BI", b"Q", b"g"]);
    let (d, data) = p.ops[2].inline.clone().unwrap();
    assert_eq!(d.int(b"W"), Some(2));
    assert_eq!(data, [0, b'E', b'I', 255], "an EI inside the data without whitespace before it is data");
    let again = parse(&serialize_ops(&p.ops));
    assert_eq!(again.ops[2].inline, p.ops[2].inline);
}

#[test]
fn damage_is_skipped_not_fatal() {
    let p = parse(b"1 0 0 RG ) ] 10 20 m 30 40 l S (unterminated");
    let names: Vec<&[u8]> = p.ops.iter().map(|o| o.op.as_slice()).collect();
    assert_eq!(names, [&b"RG"[..], b"m", b"l", b"S"]);
    assert!(p.skipped >= 2);
    assert!(parse(b"").ops.is_empty());
    assert!(parse(b"BI /W 1").ops.len() == 1);
}

#[test]
fn matrices_compose_and_invert() {
    let a = Matrix([2.0, 0.0, 0.0, 3.0, 10.0, 20.0]);
    let b = Matrix([0.0, 1.0, -1.0, 0.0, 5.0, 0.0]);
    let ab = a.then(&b);
    let (x, y) = ab.apply(1.0, 1.0);
    let (x1, y1) = a.apply(1.0, 1.0);
    assert_eq!((x, y), b.apply(x1, y1));
    let inv = ab.invert().unwrap();
    let (u, v) = inv.apply(x, y);
    assert!((u - 1.0).abs() < 1e-9 && (v - 1.0).abs() < 1e-9);
    assert_eq!(Matrix([2.0, 0.0, 0.0, 2.0, 1.0, 1.0]).bbox([0.0, 0.0, 1.0, 1.0]), [1.0, 1.0, 3.0, 3.0]);
    assert!(overlaps([0.0, 0.0, 10.0, 10.0], [5.0, 5.0, 20.0, 20.0], 0.0));
    assert!(!overlaps([0.0, 0.0, 10.0, 10.0], [10.0, 0.0, 20.0, 10.0], 0.0));
}

#[test]
fn pieces_parse_as_one_stream_and_splice_back() {
    // An operator's operands end one piece and its keyword starts the next (§7.8.2).
    let pieces = Pieces::join(&[&b"BT 72 700 Td [(After) -20 (wards)]"[..], b"TJ ET", b"q Q"]);
    assert_eq!(pieces.len(), 3);
    let ops = pieces.parse().ops;
    let names: Vec<&[u8]> = ops.iter().map(|o| o.op.as_slice()).collect();
    assert_eq!(names, [&b"BT"[..], b"Td", b"TJ", b"ET", b"q", b"Q"]);
    assert_eq!(pieces.pieces_of(&ops[2]), (0, 1));
    assert_eq!(pieces.pieces_of(&ops[3]), (1, 1));
    assert_eq!(pieces.piece_of(usize::MAX), 2);
    // Nothing changed: the pieces come back byte for byte.
    assert_eq!(pieces.splice([]), [&b"BT 72 700 Td [(After) -20 (wards)]"[..], b"TJ ET", b"q Q"]);
    // Replacing the split TJ rewrites both pieces it spans; the third is untouched.
    let out = pieces.splice([(ops[2].span.clone(), b"(New) Tj".to_vec())]);
    assert_eq!(out, [&b"BT 72 700 Td (New) Tj"[..], b" ET", b"q Q"]);
    // Removing it leaves no stray operands or keyword.
    let out = pieces.splice([(ops[2].span.clone(), Vec::new())]);
    assert_eq!(out, [&b"BT 72 700 Td "[..], b" ET", b"q Q"]);
    // Out-of-range or reversed edits are clamped, never a panic.
    let out = pieces.splice([(std::ops::Range { start: 10, end: 5 }, b"x".to_vec()), (1000..2000, Vec::new())]);
    assert_eq!(out.len(), 3);
    assert!(Pieces::join::<&[u8]>(&[]).splice([(0..1, b"x".to_vec())]).is_empty());
}
