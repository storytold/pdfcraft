//! pdfcraft-ml — machine-learning inference on the GPU (L3).
//!
//! With the `gpu` feature (native targets only), this runs the OCR models on wgpu compute
//! shaders (Vulkan, Metal, DX12): the graph and weights are read straight from the `.onnx`
//! files, the same files the CPU path (rten) loads, so both read the same model.
//!
//! - [`onnx`]: the model graph and weights, read with `rten-onnx` (rten's own parser).
//! - [`gpu`]: the device, the kernels, an executor for convolutional graphs, and the two ocrs
//!   models: [`gpu::Detector`] (text pixels) and [`gpu::Recognizer`] (convolutional trunk,
//!   bidirectional GRU, output layer).
//!
//! Every input — the model file, the image, the device — is untrusted: sizes are checked
//! against the device's limits, wgpu errors are caught in error scopes and returned, and
//! nothing here panics. The caller keeps a CPU path and falls back to it on any error.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
pub mod gpu;
#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
pub mod onnx;
