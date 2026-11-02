# Windows renderer startup errors and the OpenGL fallback

## Reproduced failure

On one Windows 11 Pro build 26200 host, the official PdfCraft 0.6.0 x86 installation and
portable EXE both exited with code 101 before displaying a usable window. Their EXE SHA-256
matched: `3a3a9ba75ec9f8ecc25863e9138fac57e17f359a5de5add486efedd83e0fcf8f`.
The host has an NVIDIA GeForce RTX 3050 Laptop GPU, driver `32.0.16.1088`, and an AMD iGPU.

Launch the official x86 EXE with `PDFCRAFT_RENDERER` and `WGPU_BACKEND` unset on that host:

```text
drawing on NVIDIA GeForce RTX 3050 Laptop GPU (DiscreteGpu, Dx12)
Shader translation error for stage ShaderStages(VERTEX | FRAGMENT): 参数错误。 (0x80070057)
Handling wgpu errors as fatal by default
wgpu error: Validation Error
Caused by:
  In Device::create_render_pipeline, label = 'egui_pipeline'
    Internal error in ShaderStages(VERTEX | FRAGMENT) shader: 参数错误。 (0x80070057)
```

The process reaches GPU initialization and then panics. A GUI-subsystem EXE has no console
to display this message during a desktop double-click. The official x64 0.6.0 EXE selected
the same NVIDIA/DX12 adapter and displayed a synthetic two-page PDF. This is a single-host
comparison; it does not establish a defect in all x86 builds or in a particular driver.
Issue #326 describes a similar symptom without enough diagnostics to establish the same cause.

## Error propagation

There are two layers: DX12 shader compilation fails, then an unscoped error in `Renderer::new`
reaches wgpu's default fatal handler. The latter prevents the existing OpenGL startup fallback
from running: it needs an `eframe::Error::Wgpu`, not a panic.

`RenderState::create` reuses the existing `winit::catch_errors` around renderer creation and
returns `WgpuError::CreateRenderer`. Auto startup can then retry with Glow before the app is
created. Explicit renderer choices retain their meaning; the non-winit constructor is unchanged.
This recovers from initialization failure; it does not repair DX12 shader compilation.

## Regression and actual-use validation

`pipeline_creation_error_is_returned_and_device_remains_usable` invokes the real constructor
on a real device with sample counts `[3, 1, 3, 1]`. Three is invalid; each error must return,
and each subsequent valid initialization must succeed. The unguarded constructor panicked
with code 101. This deterministic validation error tests propagation and device reuse,
not the underlying host-specific shader compiler defect.

```powershell
$env:PDFCRAFT_TEST_REQUIRE_GPU = '1'
$env:WGPU_BACKEND = 'vulkan'
cargo test --locked -p pdfcraft pipeline_creation_error_is_returned_and_device_remains_usable -- --nocapture
```

An ordinary run reports an unavailable GPU branch as skipped. `PDFCRAFT_TEST_REQUIRE_GPU`
makes that condition fail. Run actual startup acceptance separately with both renderer
variables unset, so the compatibility setting cannot hide the original failure.

The prior local 0.6.0 integration was exercised as a native x86 GUI EXE on the affected host.
It still recorded `0x80070057`, then `Starting with OpenGL instead` and `renderer: OpenGL (glow)`.
The UI and synthetic PDF contents displayed. A filename containing Chinese, spaces, `&`,
parentheses and `#`, page changes, 150% zoom, repeated opens and a nonexistent-file error
were exercised. The modified x64 app still used DX12 and displayed the PDF. After installing
the tested EXE and removing the forced-GL user setting, the user confirmed the original
desktop shortcut and two ordinary PDF double-clicks worked. That local integration also
included the separately credited Chinese font fix from #426; font changes are outside this PR.

The startup-only contribution was then applied to main at
`bbbe60015c571ebfc39708ee07777b89e267122a` and rebuilt as a native x86 GUI EXE (PE `0x014c`,
subsystem `2`). With both renderer variables unset it reproduced the same DX12 error, returned
it, started OpenGL, and displayed the synthetic PDF. Page 2, 150% zoom, another file open and a
nonexistent-file error were exercised without page errors or losing the open documents.
This second run does not include the Chinese product fix and is not counted as font acceptance.

On this contribution branch, workspace tests reported 2,132 passed and 7 declared ignored.
The required-adapter GPU regression, fmt, all-target workspace Clippy with warnings denied,
assets, layering (34 crates) and WASM compilation (30 crates) passed. Browser runtime was
not tested. Initial shared-cache checks hit an existing JBIG2 timeout and an outdated WASM
shading interface; rebuilding differing vendored dependencies cleared both without decoder
or shading code changes. Keep build directories separate between historical worktrees.

## Independent Windows test-fixture correction

The `hostile_args` fixture now records whether its planted symbolic link was created. This
non-elevated host returned OS error 1314; the old test nevertheless tried the nonexistent
link and reported false confinement leaks. The link path is tested only when the fixture
exists, and creation failure is explicitly reported. All other traversal and absolute-path
attempts remain. A skipped link case is not evidence that symbolic-link confinement passed.

No system fonts, screenshots, private PDFs, control tokens or application binaries are included.
Other GPU drivers and non-Windows runtime behavior remain unverified.
