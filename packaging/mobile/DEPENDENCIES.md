# Proposed mobile dependency exception

The contributor explicitly requested a narrowly scoped exception proposal in this PR. Maintainers must review this policy change before merging or an official upstream release. The contributor has authorized this documented proposal and fork preview builds with the required notices. The desktop `deny.toml`, desktop product dependency graph and asset policy are unchanged. MPL-2.0 is not added to any general license allowlist. The root lockfile adds only the permissively licensed ZIP reader used by the Rust packaging task to collect Android notices.

`app/deny.exceptions.toml` permits only the following exact crate versions in the independent mobile workspace. Future versions, new MPL dependencies and other copyleft licenses still fail the audit. The root license, source, advisory and dependency-ban policies remain in force; neither build dependencies nor advisories are excluded. Run from the repository root:

```sh
cargo deny --manifest-path packaging/mobile/app/Cargo.toml --config deny.toml --locked --target aarch64-linux-android check
cargo deny --manifest-path packaging/mobile/app/Cargo.toml --config deny.toml --locked --target aarch64-apple-ios check
```

| Crate | Version | Authors | Use |
| --- | --- | --- | --- |
| cssparser | 0.37.0 | Simon Sapin and Servo contributors | Android Wry HTML processing; both targets' Tauri code generation |
| cssparser-macros | 0.7.1 | Simon Sapin and Servo contributors | Build-time procedural macro used by cssparser |
| dtoa-short | 0.3.5 | Xidorn Quan and contributors | cssparser number formatting; Android runtime and build-time graph |
| selectors | 0.38.0 | The Servo Project Developers | Android Wry HTML processing; both targets' Tauri code generation |
| option-ext | 0.2.0 | Simon Ochsenreither and contributors | Android/iOS runtime directory discovery through dirs/dirs-sys |

Tauri 2.12.3 requires Wry 0.57.0 for the mobile WebView. Wry's Android implementation requires dom_query, which pulls in the CSS crates. Tauri's code-generation graph also requires dom_query. The directory helper is mandatory on both targets. Disabling optional Tauri features does not remove these dependencies while retaining the mobile host.

All five dependencies are unmodified upstream crates under MPL-2.0, as declared by the source packages locked in `app/Cargo.lock`. Their exact source archives, checksums and upstream revisions are listed in [NOTICE.txt](NOTICE.txt); the full license is [LICENSE-MPL-2.0.txt](LICENSE-MPL-2.0.txt). Both documents are bundled as native resources and included alongside CI packages. Source archives remain available from the official registry at the immutable version URLs in the notice. A distributor must retain these notices and provide the corresponding source, including any future modifications, when distributing the executable. This does not relicense PDFCraft's separate original source files.

Primary policy references: [cargo-deny per-project exceptions](https://embarkstudios.github.io/cargo-deny/checks/licenses/cfg.html#additional-exceptions-configuration-file), [Mozilla MPL 2.0](https://www.mozilla.org/en-US/MPL/2.0/). This proposal does not authorize adding MPL visual assets or Adobe material.
