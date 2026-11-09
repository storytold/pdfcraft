#![cfg(feature = "test-utils")]
/// Provides an auto-managed "print-to-file" virtual printer for testing.
///
/// See [`file_device::FilePrinterDevice`] for the high-level entry point.
pub mod file_device;
/// Provides a auto-managed null printer device for testing.
pub mod null_device;
