# Windows C# compile check (any OS)

`LinkcoPdfPreviewHandler.dll` (`../PreviewHandler.cs`) and the Setup EXE (C# embedded in
`installer.py`) are built on Windows with the .NET Framework's own `csc.exe`, which only
understands **C# 5**. This check compiles both the same way on Linux, macOS or Windows — no
Windows SDK, .NET SDK or Mono required — so changes can be verified before a Windows build:

```sh
node packaging/windows/csharp-check/check.cjs              # compile both + run the logic tests
node packaging/windows/csharp-check/check.cjs --no-setup   # skip the Setup EXE
node packaging/windows/csharp-check/check.cjs --no-tests   # compile only
node packaging/windows/csharp-check/check.cjs --verbose    # also list every test check
```

It prints every error and warning in `csc` format and exits 0 only when both compile with **no
errors and no warnings** and all tests pass (1 otherwise, 2 if the toolchain couldn't be fetched).

## Logic tests (`tests/`)

The registry code that the uninstallers run for every user profile isn't something you want to
find bugs in on a customer's PC, so it also runs here: `check.cjs` cuts `RegRoot` and the
registry helpers of `LinkcoPdfPreviewHandler` out of `PreviewHandler.cs` (the lists are at the top
of `check.cjs`), compiles them with `tests/FakeRegistry.cs` — an in-memory `RegistryKey` that keeps
the real API's rules — and runs `tests/RegistryCleanupTests.cs`: cleanup of another user's
profile from split `NTUSER.DAT`/`UsrClass.dat` hives and from a single hive, restoring per-user and
machine-wide previous handlers, leaving unrelated entries and users who never ran the app
byte-for-byte unchanged, no leaked handles (a mounted hive must unload), and idempotence. If a
method is renamed the check says so instead of silently testing less. Needs Node.js 20+,
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

The compile check covers syntax, types, members, overloads and C# 5 language rules; the tests cover
the registry cleanup logic. Everything that needs Windows itself — COM activation, Explorer,
rendering, mounting real hives — is covered by `../test-windows-integration.ps1` and
`../test-msi.ps1` on Windows.
