# Unsafe code audit

The workspace sets `unsafe_code = "forbid"`. There is one exception: the Windows scanner backend,
`crates/scan-wia` (package `pdfcraft-wia`). This page says why, where the unsafe code is, what each
kind of it relies on, and how to review it. Everything not listed here is safe Rust.

## Why unsafe is needed

Windows scanners are reached through WIA, which is COM. The only maintained Rust binding is the
`windows` crate, and its COM method calls are `unsafe fn`s: they take raw pointers (out-parameters,
arrays, interface pointers) and the compiler cannot check them. There is no safe wrapper for WIA, so
the calls cannot be made without `unsafe`. The goal is therefore to keep the unsafe surface small,
in one crate, each block with a written reason.

## Where it lives

| Crate | Unsafe | Role |
|-------|--------|------|
| `pdfcraft-wia` (`crates/scan-wia`) | yes, two modules | The WIA backend. `[lints.rust] unsafe_code = "deny"`; only `src/ffi.rs` and `src/sinks.rs` carry `#![allow(unsafe_code)]`. |
| `craft-scan` (storytold/craft-libs, `crates/scan`) | none | Settings, errors, page handling, the contract and the backend registry. Forbids unsafe, has no Windows dependency, and is not part of this workspace. |
| every other crate | none | Inherits the workspace `forbid`. |

Inside `pdfcraft-wia`:

- `ffi.rs`: small safe functions, each wrapping one COM call or one `PROPVARIANT` read. 30 of the 34 unsafe sites.
- `sinks.rs`: the COM objects WIA calls back into (`#[implement]` types) and four private helpers for the raw pointers COM hands them. 4 sites.
- `backend.rs`: the `Wia` type that implements `craft_scan::Backend` (listing, details, device dump and acquire). No unsafe. It reaches COM only through `ffi` and `sinks`.
- `rules.rs`: the HRESULT tables, the sentences and the status-bit rules. No COM, no unsafe; it builds and is tested on every platform.

Dependency direction: `pdfcraft-wia` depends on `craft-scan`. `pdfcraft-platform` depends on both and
registers the backend once (`pdfcraft_platform::scan::register_backends`, which calls
`pdfcraft_wia::register`); on other platforms nothing is registered and the scan functions answer
`ScanError::Unsupported`. Off Windows `pdfcraft-wia` holds only `rules`.

The compiler and Clippy enforce the policy: `unsafe_code = "deny"` rejects unsafe anywhere except the
two allowed modules, and `clippy::undocumented_unsafe_blocks = "deny"` rejects an `unsafe` block without
a `// SAFETY:` comment.

## Categories and their safety arguments

34 unsafe sites in total (28 blocks and 2 `unsafe impl`s in `ffi.rs`, 4 blocks in `sinks.rs`). Before the
split there were 78 uses of the `unsafe` keyword in one 2,000-line module with a module-wide allow.

**1. COM apartment guard (2).** `CoInitializeEx` and `CoUninitialize` in `ComInit`. The call has no pointer
argument beyond the reserved `None`. `CoUninitialize` runs only if the matching `CoInitializeEx`
succeeded, and `ComInit` is `!Send` (a `PhantomData<*const ()>` field), so it drops on the thread that
initialised COM.

**2. Object creation and device open (3).** `CoCreateInstance`, `IWiaDevMgr2::CreateDevice`,
`IWiaDevMgr::CreateDevice`. Arguments are references to a live GUID and a `BSTR` that outlives the
call. An interface returned by COM owns its reference, so later calls on it cannot dangle.

**3. Enumeration (7).** `EnumDeviceInfo`, `EnumChildItems` and the `Next` calls of the two enumerators
(through one generic `drain` helper), and `IWiaPropertyStorage::Enum` with its `Next`. Out-pointers
point to a one-element local slot and a count local that outlive the call, and the requested count is 1.
Property names from the dump enumerator are read once with `PWSTR::to_string` (the enumerator returns a
NUL-terminated string) and deliberately never freed. The walk is bounded to 512 entries.

**4. Property storage (3).** `ReadMultiple`, `GetPropertyAttributes`, `WriteMultiple`, always with count 1
and a `PROPSPEC`/`PROPVARIANT` that are locals of the wrapper. Values come back as an owned
`PROPVARIANT`, whose `Drop` clears it, so no manual `PropVariantClear` is needed. Writes of integers use
the safe `PROPVARIANT::from(i32)`; a `VT_CLSID` write builds the union (safe) in a `ManuallyDrop`, so the
variant that points at a stack `GUID` is never cleared.

**5. `PROPVARIANT` reads (9).** Reading a union member of a tagged union. Every reader checks the tag with
the safe `vt()` first and reads only the member that tag names: integers, `BSTR`, `LPWSTR`, `CLSID`,
and the counted arrays of integers and GUIDs. Pointers inside a variant belong to it and live while it is
borrowed; each reader copies the data out before returning and returns owned values. Array reads are
limited to 64 elements and refuse a null pointer or zero count before building a slice.

**6. Transfer operations (4).** `IWiaTransfer::Download`, `IWiaTransfer::Cancel`,
`IWiaItem2::DeviceCommand` (stop feeder) and `IWiaDataTransfer::idtGetBandedData`. Receivers are live
interfaces, the callback is a live COM object used only during the call, and the `WIA_DATA_TRANSFER_INFO`
is a local with its size set.

**7. Cross-thread cancel (2 `unsafe impl`s).** `CrossThreadTransfer` wraps an `IWiaTransfer` so that
`CancelToken::request` on another thread can call `Cancel`. The wrapper owns a COM reference, so the
object cannot be freed while it exists, and the only operation done through it is `Cancel`. See the known
limits below.

**8. Callback glue (4, `sinks.rs`).** WIA calls the `#[implement]` objects with raw pointers. Four private
helpers cover every use: read the transfer parameters (`read_params`), lend an input slice
(`with_input`), lend an output slice (`with_output`) and write an out-parameter (`store`). Each checks for
null (and a zero length) first, then relies on the COM method contract: the pointer is valid for that
call. The slices are lent to a closure and never escape. These helpers are private and are called only
with the arguments of the running COM method.

## What is not unsafe

The listing, details, dump and acquire logic; property selection and range checks; the page, progress and
cancel state; temp-file handling; error mapping; image decoding; and everything in `craft-scan`. All of
it is checked by the compiler. The `#[implement]` macro (from `windows-core`) generates its own COM
plumbing; that generated code sits under the `sinks.rs` allow and is not counted above.

## Threading and apartment model

- Each public entry point (`Backend::scanner_list`, `scanner_details`, `scanners`, `diagnostic_dump`, `acquire`)
  creates a `ComInit` on the calling thread and keeps it for the whole call. COM is initialised as a
  single-threaded apartment (`COINIT_APARTMENTTHREADED`). If the thread was already initialised in the
  other mode (`RPC_E_CHANGED_MODE`), the call proceeds in that mode and the guard undoes nothing.
- No interface pointer leaves its call. Interfaces and callback objects are created on the calling
  thread and dropped before the entry point returns. The callback objects hold `Rc`/`RefCell`, so they are
  `!Send` by construction and WIA invokes them on the thread that runs the transfer.
- The code does not run its own message loop; WIA delivers callbacks while `Download` or
  `idtGetBandedData` blocks on the calling thread.
- The one cross-thread use is `CancelToken::request`, described below.

## How to review

```text
# every unsafe site in the workspace (expect hits only under crates/scan-wia/src/{ffi,sinks}.rs)
rg -n "\bunsafe\b" crates apps xtask --glob '*.rs'

# the two allow attributes (expect exactly two)
rg -n "allow\(unsafe_code\)" crates apps xtask

# every unsafe block must be preceded by a SAFETY comment
rg -n -B3 "\bunsafe\b" crates/scan-wia/src/ffi.rs crates/scan-wia/src/sinks.rs

# the compiler and Clippy enforce both rules
cargo clippy -p pdfcraft-wia --all-targets -- -D warnings
```

For each site, check three things: the pointers come from references to locals (or from the COM method's
own arguments), nothing borrowed from a pointer outlives the call, and the type being read matches the tag
or contract the comment names. `backend.rs`, `rules.rs` and `craft-scan` need no such review.

## Known limits

- **Cross-thread cancel.** `CancelToken::request` calls `IWiaTransfer::Cancel` on an interface that was
  created in the worker's apartment, from the thread that asks. COM may refuse a call made from the wrong
  apartment; the result is ignored, and the cooperative cancel flag, checked in every callback, still stops
  the transfer. This keeps the behaviour of the first implementation. A more rigorous design would hand over
  an `AgileReference` and resolve it on the cancelling thread; that changes timing on real devices, so it
  is not part of this change.
- **Driver-supplied data.** `PROPVARIANT` readers trust the tag, and callback helpers trust the COM
  contract. A driver that violates either can still cause undefined behaviour; the mitigations are the
  length bounds and null checks above, not validation of the driver.
- **Leaked names.** Property names from the debug dump are not freed (some class drivers return memory
  that is not owned by the COM allocator). The leak is small and only occurs in the dump.
- **Not covered by tools.** Miri cannot run the FFI paths. The unit tests cover the pure logic and the
  variant conversions; the COM calls themselves are exercised only on a real or simulated scanner.
