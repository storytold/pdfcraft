#!/usr/bin/env bash
# Install the WebAssembly toolchain in a Vercel build environment.
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"

if ! command -v rustup >/dev/null 2>&1; then
  command -v curl >/dev/null 2>&1 || { echo "error: curl is required to install Rust" >&2; exit 1; }
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
  # rustup's installer can't update the PATH of this build step.
  export PATH="$HOME/.cargo/bin:$PATH"
fi

command -v rustup >/dev/null 2>&1 || { echo "error: rustup installation failed" >&2; exit 1; }
rustup target add wasm32-unknown-unknown

if ! command -v trunk >/dev/null 2>&1; then
  cargo install trunk --locked
fi

if ! command -v wasm-opt >/dev/null 2>&1; then
  command -v npm >/dev/null 2>&1 || { echo "error: npm is required to install Binaryen's wasm-opt" >&2; exit 1; }
  npm install --global binaryen@132.0.0
  export PATH="$(npm prefix --global)/bin:$PATH"
fi

command -v cargo >/dev/null 2>&1 || { echo "error: cargo is unavailable" >&2; exit 1; }
command -v trunk >/dev/null 2>&1 || { echo "error: trunk installation failed" >&2; exit 1; }
command -v wasm-opt >/dev/null 2>&1 || { echo "error: wasm-opt installation failed" >&2; exit 1; }
