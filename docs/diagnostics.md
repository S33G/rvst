# Native logs in Chrome, debugging and replay

RVST library crates leave the HTTP inspector disabled by default. Build a native
plugin with `devtools` to enable it. Generated native bundle commands add this
feature for development profiles; `--release`, `--profile release`, and custom
profiles omit it unless `--features devtools` is requested.

Generated `plugin-standalone` wrappers instead enable `devtools` in their Cargo
default features, including release builds. To build a standalone wrapper without
the inspector, use `cargo build -p plugin-standalone --release --no-default-features`.
Profile selection alone does not disable Cargo default features.

```sh
cargo rvst inspect --format json
cargo rvst inspect --open --instance INSTANCE_ID
```

Each native instance starts an ephemeral **127.0.0.1** HTTP endpoint and a
random-token session record. The URL carries its token in a fragment; requests
use the `X-RVST-Token` header. Host/Origin checks prevent arbitrary web pages from
reading the API. On Unix, discovery files are owner-only. No cloud service,
remote listener, microphone access or telemetry upload is used. Treat the URL as
a local capability. Closing the instance shuts down its worker and removes the
registry entry; discovery ignores stale entries.

`DebugSession::log(level, target, message)` is for control/background threads.
`DebugSession::plugin_log()` supplies an independent bounded `DiagnosticSink`.
The native adapter and standalone runner pass it to `Plugin::connect_diagnostics`;
the processor retains it and calls `emit(code, sample_clock, value)` without strings,
locks or allocation. Return static `(code, label)` pairs from
`Plugin::diagnostic_events()` for automatic readable labels, or use
`DebugSession::register_event` during setup. Log histories and producer queues
are bounded. The inspector reports drops and sequence gaps.
Callback timing uses a log2 histogram; percentiles are **upper-bound estimates**,
not precise sampled traces. Instrumentation has a measurable cost: benchmark
with and without `devtools` before claiming production performance.

The inspector's **Live parameters** table refreshes from the adapter's atomic
parameter mirror on every poll. Applied values and pending editor changes are
shown separately. The expanded state view includes live input/output peaks,
reported latency and sample clock, static lifecycle metadata, and timing metrics.
Native editor timing measures the editor function on its UI thread, excluding
backend rendering, GPU work and presentation.

Enable **Echo new logs to DevTools** to send newly received native/plugin events
to Chrome's Console at their matching log level. It is off by default, does not
replay old events, and limits output to 50 events per second plus a suppression
summary. The inspector retains at most 1,000 browser events and displays 250;
pausing the view does not pause the native processor or event collection.

`cargo rvst inspect --format json` returns authenticated live state, metrics and
recent events for discovered instances. Use `--instance ID` to select one.
Transport credentials are omitted from this output and from API snapshots;
`--open` still opens the private capability URL directly in your browser.
The shared `rvst_devtools::snapshot(&session, after_sequence)` client validates
loopback capabilities and instance identity, caps responses at 4 MiB and enforces
a total request deadline. Studio probes newer registry entries first with a
750 ms discovery budget (plus at most one in-flight request). Its initial result
can be partial for a large or unresponsive registry; CLI inspection checks all
eligible records (up to the registry's 256-entry limit).

Studio shows the newest discovered instance's callback metrics, input/output
peaks, latency, parameter count and a sparkline of its cumulative mean callback
time. It polls once per second on a background worker while a job or selected
session remains active. History retains 60 samples and resets when the selected
instance changes. An initial discovery pass runs when Studio opens; press `i` to
rediscover after loading a plugin. When idle, the equivalent-command footer and
Ctrl+Y copy the selected instance's `inspect --format json` command.

Plugin tooling can register additional read-only state during setup:

```rust,no_run
# fn add_state(session: &rvst_devtools::DebugSession) -> std::io::Result<()> {
let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
let source = counter.clone();
session.register_state_provider("engine", move || {
    serde_json::json!({"voices": source.load(std::sync::atomic::Ordering::Relaxed)})
})?;
# Ok(()) }
```

Providers run on the inspector worker only after API authentication. Read atomics
or copied snapshots; never access mutable DSP or wait for the audio callback.
Registration and JSON construction belong off the audio thread. A session accepts
up to 16 uniquely named providers, each with a 64 KiB serialized result limit.
Oversized results and provider panics become visible errors under
`snapshot.live.<name>` while the inspector continues serving other state.
These independent atomic reads are live observations, not a simultaneous DSP
checkpoint; use recording/replay for deterministic comparisons.

## What Chrome can debug

Chrome DevTools can inspect the local developer page, requests, rendering and the
WASM UI preview. It can show live logs/metrics from DSP executing in a DAW through
the local bridge. It does **not** attach JavaScript breakpoints to native Rust DSP.
For native source stepping use the included CodeLLDB launch/attach configurations,
`lldb -p PID`, or Linux `gdb -p PID`. Pausing the audio thread also pauses that
plugin and may interrupt the DAW's audio engine.

The WASM builder retains debug information (`wasm-bindgen --keep-debug`). Rust
source-level WASM debugging depends on the browser and DWARF support; test the
actual browser/tool versions. References: [wasm-bindgen debug information](https://wasm-bindgen.github.io/wasm-bindgen/reference/debug-info.html),
[Chrome's WebAssembly debugging](https://developer.chrome.com/docs/devtools/wasm).

## Reproduction

```sh
cargo xtask record capture.json --blocks 64
cargo xtask replay capture.json
cargo xtask replay capture.json --step
```

The example creates an offline, reproducible fixture containing initial state,
configuration, input/output audio, musical events, transport, precision, statuses
and original block boundaries. `Recording::cursor::<P>()` processes one block per
step and retains DSP state. An incomplete capture cannot claim deterministic
verification. For live capture use `recording_channel` and drain on a worker;
start from a reset or a complete processor-specific DSP checkpoint. Set
`Recording.start` to `CaptureStart::Reset` for reset captures or
`CaptureStart::Checkpoint` when `initial_state` restores the complete DSP history.
Replay resets only reset captures and requires observed processing status and
musical output for every block. Parameter presets alone cannot establish an
arbitrary live DSP checkpoint.
