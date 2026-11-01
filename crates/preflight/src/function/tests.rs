use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object};
use pdfcraft_testkit::Pdf;

use super::*;

/// A document whose objects 2, 3… are `objects`.
fn doc(objects: &[Vec<u8>]) -> Document {
    Document::open(Arc::new(Pdf::new().obj("<< /Type /Catalog >>").objs(objects).build())).unwrap()
}

fn read(objects: &[&str]) -> Option<Function> {
    let d = doc(&objects.iter().map(|o| o.as_bytes().to_vec()).collect::<Vec<_>>());
    Function::read(&d, &Object::Ref(ObjRef::new(2, 0)))
}

fn calculator(program: &str, domain: &str, range: &str) -> Option<Function> {
    let d = doc(&[pdfcraft_testkit::stream_obj(&format!("/FunctionType 4 /Domain [{domain}] /Range [{range}]"), program)]);
    Function::read(&d, &Object::Ref(ObjRef::new(2, 0)))
}

fn near(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9)
}

#[test]
fn exponential_interpolates_and_clips_its_input() {
    let f = read(&["<< /FunctionType 2 /Domain [0 1] /C0 [0 0] /C1 [1 0.5] /N 2 >>"]).unwrap();
    assert_eq!((f.inputs(), f.outputs()), (1, Some(2)));
    assert!(near(&f.eval(&[0.5]).unwrap(), &[0.25, 0.125]));
    assert!(near(&f.eval(&[3.0]).unwrap(), &[1.0, 0.5]), "inputs are clipped to the domain");
    // Defaults: C0 [0], C1 [1].
    assert!(near(&read(&["<< /FunctionType 2 /Domain [0 1] /N 1 >>"]).unwrap().eval(&[0.3]).unwrap(), &[0.3]));
    // A negative base to a fractional power has no value.
    assert_eq!(read(&["<< /FunctionType 2 /Domain [-1 1] /N 0.5 >>"]).unwrap().eval(&[-0.5]), None);
    assert!(read(&["<< /FunctionType 2 /Domain [0 1] /C0 [0 0] /C1 [1] /N 1 >>"]).is_none(), "C0 and C1 differ in length");
    assert!(read(&["<< /FunctionType 2 /Domain [1 0] /N 1 >>"]).is_none(), "a domain must be in order");
}

#[test]
fn stitching_picks_its_subdomain_and_reports_where_it_jumps() {
    let f = read(&[
        "<< /FunctionType 3 /Domain [0 1] /Functions [3 0 R 4 0 R] /Bounds [0.5] /Encode [0 1 1 0] >>",
        "<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>",
        "<< /FunctionType 2 /Domain [0 1] /C0 [0.2] /C1 [0.4] /N 1 >>",
    ])
    .unwrap();
    assert!(near(&f.eval(&[0.25]).unwrap(), &[0.5]));
    // The bound belongs to the second function, whose encoding runs backwards.
    assert!(near(&f.eval(&[0.5]).unwrap(), &[0.4]));
    assert!(near(&f.eval(&[0.75]).unwrap(), &[0.3]));
    assert!(near(&f.eval(&[1.0]).unwrap(), &[0.2]));
    let fs = Functions(vec![f]);
    assert_eq!(fs.breaks(0.0, 1.0), vec![0.5]);
    // A nested stitching function's bounds are mapped back to the outer input.
    let nested = read(&[
        "<< /FunctionType 3 /Domain [0 2] /Functions [3 0 R 4 0 R] /Bounds [1] /Encode [0 1 0 1] >>",
        "<< /FunctionType 3 /Domain [0 1] /Functions [4 0 R 4 0 R] /Bounds [0.25] /Encode [0 1 0 1] >>",
        "<< /FunctionType 2 /Domain [0 1] /N 1 >>",
    ])
    .unwrap();
    assert_eq!(Functions(vec![nested]).breaks(0.0, 2.0), vec![0.25, 1.0]);
}

#[test]
fn a_stitching_function_that_contains_itself_is_refused() {
    assert!(read(&["<< /FunctionType 3 /Domain [0 1] /Functions [2 0 R] /Bounds [] /Encode [0 1] >>"]).is_none());
}

#[test]
fn sampled_functions_interpolate_between_their_samples() {
    // Two inputs, one output, 8 bits: 0 and 255 on one diagonal, 255 and 0 on the other.
    let d = doc(&[pdfcraft_testkit::stream_obj("/FunctionType 0 /Domain [0 1 0 1] /Range [0 1] /Size [2 2] /BitsPerSample 8", [0u8, 255, 255, 0])]);
    let f = Function::read(&d, &Object::Ref(ObjRef::new(2, 0))).unwrap();
    assert!(near(&f.eval(&[0.0, 0.0]).unwrap(), &[0.0]));
    assert!(near(&f.eval(&[1.0, 0.0]).unwrap(), &[1.0]), "the first input varies fastest");
    assert!(near(&f.eval(&[0.5, 0.5]).unwrap(), &[0.5]));
    assert!(near(&f.eval(&[0.25, 0.0]).unwrap(), &[0.25]));
    // 4-bit samples packed across bytes, with a decode array.
    let d = doc(&[pdfcraft_testkit::stream_obj("/FunctionType 0 /Domain [0 1] /Range [0 2] /Decode [0 2] /Size [3] /BitsPerSample 4", [0x0F, 0x50])]);
    let f = Function::read(&d, &Object::Ref(ObjRef::new(2, 0))).unwrap();
    assert!(near(&f.eval(&[0.5]).unwrap(), &[2.0]));
    assert!(near(&f.eval(&[1.0]).unwrap(), &[10.0 / 15.0]));
    // 12-bit samples.
    let d = doc(&[pdfcraft_testkit::stream_obj("/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 12", [0x00, 0x0F, 0xFF])]);
    let f = Function::read(&d, &Object::Ref(ObjRef::new(2, 0))).unwrap();
    assert!(near(&f.eval(&[1.0]).unwrap(), &[1.0]));
    // Too little data, and a cubic order, aren't read.
    for dict in ["/Size [4] /BitsPerSample 8", "/Size [2] /BitsPerSample 8 /Order 3", "/Size [2] /BitsPerSample 7"] {
        let d = doc(&[pdfcraft_testkit::stream_obj(&format!("/FunctionType 0 /Domain [0 1] /Range [0 1] {dict}"), [0u8, 255])]);
        assert!(Function::read(&d, &Object::Ref(ObjRef::new(2, 0))).is_none(), "{dict}");
    }
}

#[test]
fn sampled_functions_written_here_read_back() {
    let values = [0.0, 1.0, 0.25, 0.5, 1.0, 0.0, 0.75, 0.5];
    let s = sampled_stream(&[(0.0, 2.0)], &[2], &values);
    let d = doc(&[]);
    let f = Function::read(&d, &Object::Stream(s)).unwrap();
    assert_eq!((f.inputs(), f.outputs()), (1, Some(4)));
    let at = |x: f64| f.eval(&[x]).unwrap();
    assert!(at(0.0).iter().zip(&values[..4]).all(|(a, b)| (a - b).abs() < 1e-4));
    assert!(at(2.0).iter().zip(&values[4..]).all(|(a, b)| (a - b).abs() < 1e-4));
    assert!(at(1.0).iter().zip([0.5, 0.5, 0.5, 0.5]).all(|(a, b)| (a - b).abs() < 1e-4), "{:?}", at(1.0));
}

#[test]
fn calculator_programs_run() {
    let max = calculator("{ 2 copy lt { exch } if pop }", "0 1 0 1", "0 1").unwrap();
    assert!(near(&max.eval(&[0.2, 0.7]).unwrap(), &[0.7]));
    assert!(near(&max.eval(&[0.9, 0.7]).unwrap(), &[0.9]));
    let step = calculator("{ 0.5 gt { 1 } { 0 } ifelse }", "0 1", "0 1").unwrap();
    assert!(near(&step.eval(&[0.6]).unwrap(), &[1.0]));
    assert!(near(&step.eval(&[0.4]).unwrap(), &[0.0]));
    let trig = calculator("{ 360 mul sin 1 add 2 div }", "0 1", "0 1").unwrap();
    assert!(near(&trig.eval(&[0.25]).unwrap(), &[1.0]));
    let stack = calculator("{ pop 1 2 3 3 1 roll }", "0 1", "0 3 0 3 0 3").unwrap();
    assert!(near(&stack.eval(&[0.0]).unwrap(), &[3.0, 1.0, 2.0]));
    let index = calculator("{ pop 7 8 1 index }", "0 1", "0 9 0 9 0 9").unwrap();
    assert!(near(&index.eval(&[0.0]).unwrap(), &[7.0, 8.0, 7.0]));
    let ints =
        calculator("{ pop 7 2 idiv 7 2 mod 1 3 bitshift 8 -2 bitshift -7 abs 2.5 round cvi }", "0 1", "-9 9 -9 9 -9 9 -9 9 -9 9 -9 9").unwrap();
    assert!(near(&ints.eval(&[0.0]).unwrap(), &[3.0, 1.0, 8.0, 2.0, 7.0, 3.0]));
    let range = calculator("{ 4 mul }", "0 1", "0 1").unwrap();
    assert!(near(&range.eval(&[0.5]).unwrap(), &[1.0]), "outputs are clipped to the range");
}

#[test]
fn calculator_errors_have_no_value() {
    for (program, why) in [
        ("{ 0 div }", "division by zero"),
        ("{ pop pop }", "an empty stack"),
        ("{ 1 0 idiv }", "integer division by zero"),
        ("{ 0.5 1 idiv }", "integer division of a real"),
        ("{ neg sqrt }", "a negative square root"),
        ("{ true add }", "arithmetic on a boolean"),
        ("{ 1 { 2 } if }", "if without a boolean"),
        ("{ 0 ln }", "the logarithm of zero"),
    ] {
        let f = calculator(program, "0.5 1", "0 1").unwrap_or_else(|| panic!("{why}: reads"));
        assert_eq!(f.eval(&[0.5]), None, "{why}");
    }
    // A stack past 100 entries.
    let deep = format!("{{ {} }}", "1 ".repeat(120));
    assert_eq!(calculator(&deep, "0 1", "0 1").unwrap().eval(&[0.5]), None);
}

#[test]
fn malformed_calculator_programs_are_not_read() {
    for program in ["1 add", "{ 1 add", "{ 1 add } }", "{ { 1 } }", "{ 1 frobnicate }", "{ { 1 } { 2 } { 3 } ifelse }", "{ 1e }", "{ inf }"] {
        assert!(calculator(program, "0 1", "0 1").is_none(), "{program}");
    }
    let nested = format!("{{ {} }}", "true { ".repeat(80) + &"} if ".repeat(80));
    assert!(calculator(&nested, "0 1", "0 1").is_none(), "nested too deeply");
    assert!(read(&["<< /FunctionType 4 /Domain [0 1] >>"]).is_none(), "a calculator is a stream");
}

#[test]
fn arrays_of_functions_give_one_output_each() {
    let d = doc(&[
        b"<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>".to_vec(),
        b"<< /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >>".to_vec(),
        b"[2 0 R 3 0 R]".to_vec(),
        b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0] /C1 [1 1] /N 1 >>".to_vec(),
        b"[2 0 R 5 0 R]".to_vec(),
    ]);
    let fs = Functions::read(&d, &Object::Ref(ObjRef::new(4, 0))).unwrap();
    assert_eq!((fs.inputs(), fs.outputs()), (1, Some(2)));
    assert!(near(&fs.eval(&[0.25]).unwrap(), &[0.25, 0.75]));
    assert!(Functions::read(&d, &Object::Ref(ObjRef::new(6, 0))).is_none(), "each function in an array gives one output");
}
