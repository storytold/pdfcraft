# Proposal: share the OCLP wgpu compatibility fork

PdfCraft uses the wgpu 30.0.1 backport proposed for
[ArtCraft Launcher](https://github.com/storytold/craft-launcher/pull/917),
[PhotoCraft](https://github.com/storytold/photocraft/pull/9711) and
[LightCraft](https://github.com/storytold/lightcraft/pull/3087).
It preserves configured internal indirect-call validation instead of clearing
that flag at startup. The existing OpenGL recovery path stays available.

## Dependency and ownership

- Prototype: <https://github.com/ITJamie/wgpu-fork>
- Branch: `codex/metal-msl-intel-v30.0.1`
- Immutable commit: `21c08e23135ba27ed1c6b68177ecc20c0d076dd9`
- Base: upstream `v30.0.1`, retaining the released API used by egui.
- Backports: <https://github.com/gfx-rs/wgpu/pull/10725> and
  <https://github.com/gfx-rs/wgpu/pull/10731>.

The backport probes supported Metal Shading Language versions, uses pipeline
workgroup limits when Intel's compiler rejects the shader attribute, and emits
runtime-sized storage arrays as pointers on Intel. Shader bounds checks remain
enabled. `WGPU_VALIDATION_INDIRECT_CALL` still explicitly overrides the default.

A maintainer should decide whether to create `storytold/wgpu` and retain the
reviewed backport there. Switch the patch URL in `Cargo.toml`, keep the immutable
commit pin, and regenerate `Cargo.lock`. The apps should share this dependency
and return to an upstream release once both fixes are released and validated.

Related reports: [#626](https://github.com/storytold/pdfcraft/issues/626) records
an Intel indirect-validation compiler failure; [#435](https://github.com/storytold/pdfcraft/issues/435)
records an older Intel Mac failing to launch without a compiler diagnostic.
The merged [#1627](https://github.com/storytold/pdfcraft/pull/1627) already avoids
the former by disabling validation; this proposal replaces that workaround.
The earlier MSL-only [#1610](https://github.com/storytold/pdfcraft/pull/1610) was
closed without merging. These reports do not establish that every startup
failure has the same cause. Native Monterey hardware remains unverified.

## Validation

```sh
cargo fmt --all -- --check
cargo clippy --locked -p pdfcraft -p pdfcraft-forms -p pdfcraft-ui-egui -j 2 --all-targets -- -D warnings
cargo test --locked -p pdfcraft -j 2
cargo test --locked -p pdfcraft-forms -j 2
cargo test --locked -p pdfcraft-ui-egui -j 2 compare
cargo build --locked -p pdfcraft -p pdfcraft-cli -j 2
cargo xtask layers
cargo xtask assets
cargo check --locked -p pdfcraft-web --target wasm32-unknown-unknown -j 2
```

For a native check, copy the built app into a separate scratch folder and put
`portable.txt` beside it to isolate settings. Use an original synthetic PDF with
no proprietary fonts or images. Start the app with an owner-only control file:

```sh
mkdir -m 700 /private/tmp/pdfcraft-check
PDFCRAFT_SYSTEM_FONTS=0 PDFCRAFT_RENDERER=wgpu WGPU_BACKEND=metal \
WGPU_POWER_PREF=low WGPU_VALIDATION_INDIRECT_CALL=1 \
MTL_DEBUG_LAYER=0 MTL_SHADER_VALIDATION=0 \
./pdfcraft --control /private/tmp/pdfcraft-check/control.json synthetic.pdf
```

Use `WGPU_POWER_PREF=high` for AMD and `low` for Intel on a dual-GPU Mac. Check the
successful `renderer:` log entry identifies the intended Metal adapter, open the
PDF, and capture the rendered page with `pdfcraft-cli ui --control FILE screenshot
--out FILE.png`. `PDFCRAFT_RENDERER=wgpu` prevents OpenGL fallback from hiding a
Metal failure. External Metal validation layers are disabled only for these OCLP
processes; wgpu's internal indirect-call validation is enabled.

### Results

- Apple M4 Max, macOS 27.0.1, Rust 1.96.1 (Cargo 1.99.1): native app/CLI
  build, strict all-target Clippy for the three touched crates, formatting,
  layering and asset gates passed. All 83 app-package tests, 187 forms tests and
  ten targeted UI comparison tests passed. The validation-flag regression was
  observed failing with the old bypass and passing after its removal.
- The web check passed with ten unused-code/variable warnings in unchanged
  renderer/UI areas. No full workspace test sweep was run locally; CI covers it.
- Local builds included the optional craft-fonts checkout at
  `aced4137c022f1f587624d48e81ec77e1914a9cc`. The two equivalent boolean guard
  simplifications in forms and comparison shading satisfy strict Clippy without
  changing their conditions or evaluation order.
- MacBookPro11,5, OCLP macOS 15.8.1, Rust 1.99.1: native app/CLI build and
  strict app-package Clippy passed, without the optional craft-fonts input.
  Both AMD and Intel rendered the original PDF correctly with no page errors;
  CLI screenshots were inspected. Both adapters selected MSL 3.1 below the OS's
  MSL 3.2 maximum, exercising automatic fallback without version overrides.
- The complete Intel package run exceeded a 600-second limit in
  `pipeline_creation_error_is_returned_and_device_remains_usable`. Samples show
  repeated adapter enumeration waiting in the workgroup-size attribute compiler
  probe. That regression passed alone in 234 seconds, and the other 82 package
  tests passed in a separate run. The combined-run timeout remains untriaged;
  this is not a clean full-package OCLP pass. The existing GPU recovery test was run
  without source changes.

| Hardware | Metal adapter confirmed in log | Active PDF at startup (debug) | Native page and CLI screenshot |
|---|---|---|---|
| M4 Max | Apple M4 Max | 2 s | Passed |
| MacBookPro11,5, OCLP macOS 15.8.1 | AMD Radeon R9 M370X | 33 s | Passed |
| Same Mac | Intel Iris Pro Graphics | 32 s | Passed |

These are individual debug runs, timed until the control channel reported the
synthetic PDF active, followed by a rendered-page screenshot and a check for
page errors. They are not a performance benchmark or full PDF fidelity coverage.
Internal indirect-call validation was enabled; OpenGL fallback was disabled for
these checks. External Metal validation layers were disabled per process.

### Shared backport limits

The earlier full wgpu suite on an M4 Max ran 1,010 tests: 1,002 passed, eight
failed and six were skipped. Failures were a subgroup expected-failure mismatch,
two passthrough tests requiring DXC, a metallib test requiring offline Metal
tools, and four untriaged capability/binding-array tests:
`binding_array_storage_buffers`, `partial_binding_array_storage_buffers`,
`validate_capabilities`, and the Computepass Encoding benchmark. These remain
limits of the shared backport's qualification. This proposal does not claim
coverage of every OCLP GPU, native older macOS, Windows or Linux hardware.
