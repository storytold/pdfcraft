//! A tiny contributor-original model used only with the `test-model` feature.

// Minimal protobuf encoding of the public ONNX schema, not a committed binary fixture:
// https://github.com/onnx/onnx/blob/main/onnx/onnx.proto
fn varint(mut n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while n >= 128 {
        out.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    out.push(n as u8);
    out
}
fn integer(field: u64, n: u64) -> Vec<u8> {
    [varint(field << 3), varint(n)].concat()
}
fn message(field: u64, bytes: &[u8]) -> Vec<u8> {
    [varint((field << 3) | 2), varint(bytes.len() as u64), bytes.to_vec()].concat()
}
fn value(name: &str, dims: &[u64]) -> Vec<u8> {
    let shape: Vec<u8> = dims.iter().flat_map(|n| message(1, &integer(1, *n))).collect();
    let tensor_type = [integer(1, 1), message(2, &shape)].concat();
    [message(1, name.as_bytes()), message(2, &message(1, &tensor_type))].concat()
}
fn tensor(name: &str, dims: &[u64], values: &[f32]) -> Vec<u8> {
    let dims: Vec<u8> = dims.iter().flat_map(|n| integer(1, *n)).collect();
    let raw: Vec<u8> = values.iter().flat_map(|n| n.to_le_bytes()).collect();
    [dims, integer(2, 1), message(8, name.as_bytes()), message(9, &raw)].concat()
}
pub fn bytes() -> Vec<u8> {
    let boxes = [0.5, 0.25, 0.4, 0.05, 0.7, 0.45, 0.03, 0.03, 0.4, 0.75, 0.4, 0.05];
    let scores = [8.0, -8.0, -8.0, -8.0, 8.0, -8.0, -8.0, -8.0, 8.0];
    let graph = [
        message(2, b"synthetic form field proposals"),
        message(5, &tensor("dets", &[1, 3, 4], &boxes)),
        message(5, &tensor("labels", &[1, 3, 3], &scores)),
        message(11, &value("input", &[1, 3, 576, 576])),
        message(12, &value("dets", &[1, 3, 4])),
        message(12, &value("labels", &[1, 3, 3])),
    ]
    .concat();
    [integer(1, 8), message(7, &graph), message(8, &integer(2, 17))].concat()
}
