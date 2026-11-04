#!/bin/sh
# Build the example plug-in and refresh the checked-in test fixture, the
# same arrangement as PhotoCraft's examples/plugins/invert-rs: reviewers
# and CI can exercise the host from the fixture without a wasm toolchain.
set -eu
cd "$(dirname "$0")"
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
FIXTURE="../../../crates/plugin/tests/fixtures/hello.wasm"
mkdir -p "$(dirname "$FIXTURE")"
cp target/wasm32-unknown-unknown/release/pdfcraft_plugin_hello.wasm "$FIXTURE"
echo "fixture refreshed: $FIXTURE"
echo "install it from PdfCraft's Plugins panel, or drop it into the plug-ins folder"
