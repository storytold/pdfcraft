# wayland-tablet

The pen-tablet sidecar: `zwp_tablet_v2` bound on the same `wl_display` the windowing
library (winit) talks on, on its own event queue, forwarding pen events over a channel.
This is the W28 "sidecar" path — stock winit 0.30, no fork, no `[patch]` added by this
crate.

```text
winit window ──(raw-window-handle, wl_display*)──▶ TabletSidecar::spawn(&window)
        │
        └── sidecar queue ──▶ channel<PenEvent> ──▶ app frame loop
```

- `TabletSidecar::spawn_with(&window, PumpMode)` — `PumpMode::Main` pumps the sidecar
  queue from the app's loop (`sidecar.pump()`), `PumpMode::Thread` runs a dedicated
  reader (`prepare_read` → 2 ms poll → `read` → dispatch). The display behind
  `&window` must outlive the sidecar and drop after it (the crate docs' *Display
  lifetime contract* — the safe signature cannot enforce it; the sidecar borrows
  winit's connection and neither owns nor closes it).
- `PenEvent` — one event per protocol frame; pressure 0..1, tilt in degrees, `frame`'s
  own compositor timestamp. Protocol facts are cited in `src/state.rs` from the
  tablet-v2 XML shipped inside the `wayland-protocols` crate in this build.

Status: **Task 0 feasibility spike** (no egui integration yet). Run
`examples/print_pen.rs` on hardware with a pen:

```text
cargo run -p wayland-tablet --example print_pen          # WAYLAND_TABLET_PUMP=main
WAYLAND_TABLET_PUMP=thread cargo run -p wayland-tablet --example print_pen
```

Known sharp edge: the workspace pins winit itself (`[patch.crates-io]` in the root
`Cargo.toml`), so this crate's bare `winit = "0.30"` dev-dependency resolves to that
pin like every other workspace member — the sidecar code touches only winit's stable
`ApplicationHandler` API.
