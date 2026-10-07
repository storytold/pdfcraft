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
    assert_eq!(parse(b"BI /W 1").ops.len(), 1);
}

#[test]
fn matrices_compose_and_invert() {
    let first = Matrix([2.0, 0.0, 0.0, 3.0, 10.0, 20.0]);
    let second = Matrix([0.0, 1.0, -1.0, 0.0, 5.0, 0.0]);
    let combined = first.then(&second);
    let (x, y) = combined.apply(1.0, 1.0);
    let (x1, y1) = first.apply(1.0, 1.0);
    assert_eq!((x, y), second.apply(x1, y1));
    let inv = combined.invert().unwrap();
    let (inv_x, inv_y) = inv.apply(x, y);
    assert!((inv_x - 1.0).abs() < 1e-9 && (inv_y - 1.0).abs() < 1e-9);
    assert_eq!(Matrix([2.0, 0.0, 0.0, 2.0, 1.0, 1.0]).bbox([0.0, 0.0, 1.0, 1.0]), [1.0, 1.0, 3.0, 3.0]);
    assert!(overlaps([0.0, 0.0, 10.0, 10.0], [5.0, 5.0, 20.0, 20.0], 0.0));
    assert!(!overlaps([0.0, 0.0, 10.0, 10.0], [10.0, 0.0, 20.0, 10.0], 0.0));
}
