# Windows C# compile check (any OS)

`LinkcoPdfPreviewHandler.dll` (`../PreviewHandler.cs`) and the Setup EXE (C# embedded in
`installer.py`) are built on Windows with the .NET Framework's own `csc.exe`, which only
understands **C# 5**. This check compiles both the same way on Linux, macOS or Windows — no
Windows SDK, .NET SDK or Mono required — so changes can be verified before a Windows build:

```sh
node packaging/windows/csharp-check/check.cjs              # both
node packaging/windows/csharp-check/check.cjs --no-setup   # PreviewHandler.cs only
```

It prints every error and warning in `csc` format and exits 0 only when both compile with **no
errors and no warnings** (1 otherwise, 2 if the toolchain couldn't be fetched). Needs Node.js 20+,
`npm`, `git`, and Python 3 (to extract the Setup EXE source from `installer.py`).

## How it works

- **Compiler:** Roslyn, running in the .NET 10 WebAssembly runtime from the MIT-licensed npm
  package [`@live-codes/blazor-wasm`](https://github.com/live-codes/browser-blazor) (pinned to
  0.1.1). `check.cjs` boots it in Node (jsdom supplies the DOM Blazor expects) and runs
  `roslyn-driver.cs` through the bundle's `RunCode`. The driver drives Roslyn by reflection with
  `LanguageVersion.CSharp5`, warning level 4, AnyCPU, Release.
- **References:** the .NET Framework 4.8 reference assemblies from the MIT-licensed
  [`mono/reference-assemblies`](https://github.com/mono/reference-assemblies) (pinned commit):
  `mscorlib`, `System`, `System.Core`, `System.Drawing`, `System.Windows.Forms`, plus
  `System.IO.Compression(.FileSystem)` for the Setup EXE — the same set the build passes to `csc`.
- Both are downloaded on first use into `.cache/` here (about 60 MB, git-ignored). Nothing is
  bundled into the product and nothing ships with it.

This checks that the code compiles (syntax, types, members, overloads, C# 5 language rules). It does
not run it: the handler still has to be tried in File Explorer on Windows
(`../test-windows-integration.ps1` covers the rest there).
