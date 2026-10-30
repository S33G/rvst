# Native logs, debugging and replay

[Documentation index](README.md) · [API contract](api-contract.md) · [Real-time rules](realtime.md)

The local inspector exposes native plugin logs, live state and timing metrics in
a browser or through the CLI. Native source stepping and deterministic replay
are separate workflows, described below.

## Enable and open the inspector

RVST library crates leave the HTTP inspector disabled by default. Build a native
plugin with `devtools` to enable it.

| Build path | Inspector behavior |
| --- | --- |
| Generated native bundle, development profiles | Adds `devtools`. |
| Generated native bundle, `--release`, `--profile release`, or a custom profile | Omits it unless `--features devtools` is requested. |
| Generated `plugin-standalone` wrapper | Enables `devtools` in default Cargo features, including release builds. |

To build a generated standalone wrapper without the inspector:

```sh
cargo build -p plugin-standalone --release --no-default-features
```

Profile selection alone does not disable Cargo default features. Once an
inspector-enabled instance is running, discover it and open its inspector:

```sh
cargo rvst inspect --format json
cargo rvst inspect --open --instance INSTANCE_ID
```

Replace `INSTANCE_ID` with the instance ID returned by discovery. Use
`--instance ID` to select a single instance for CLI snapshots too.

### Local access and lifetime

Each native instance starts an ephemeral **127.0.0.1** HTTP endpoint and a
random-token session record. The URL carries its token in a fragment; requests
use the `X-RVST-Token` header. Host/Origin checks prevent arbitrary web pages from
reading the API. On Unix, discovery files are owner-only.

No cloud service, remote listener, microphone access or telemetry upload is
used. Treat the URL as a local capability. Closing the instance shuts down its
worker and removes the registry entry; discovery ignores stale entries.

## Emit logs from the right thread

| Source | API and thread contract |
| --- | --- |
| Control/background logs | `DebugSession::log(level, target, message)` accepts formatted messages. |
| Processor diagnostics | `DebugSession::plugin_log()` supplies an independent bounded `DiagnosticSink`. Emit numeric `code`, `sample_clock` and `value` without strings, locks or allocation. |
| Event labels | Return static `(code, label)` pairs from `Plugin::diagnostic_events()`, or call `DebugSession::register_event` during setup. |

The native adapter and standalone runner pass the numeric sink to
`Plugin::connect_diagnostics`; the processor retains it. Log histories and
producer queues are bounded. The inspector reports drops and sequence gaps.

## Read live state and timing

The inspector's **Live parameters** table refreshes from the adapter's atomic
parameter mirror on every poll. Applied values and pending editor changes are
shown separately. The expanded state view includes live input/output peaks,
reported latency and sample clock, static lifecycle metadata, and timing metrics.

Callback timing uses a log2 histogram; percentiles are **upper-bound estimates**,
not precise sampled traces. Native editor timing measures the editor function on
its UI thread, excluding backend rendering, GPU work and presentation.
Instrumentation has a measurable cost: benchmark with and without `devtools`
before claiming production performance.

### Chrome Console echo

Enable **Echo new logs to DevTools** to send newly received native/plugin events
to Chrome's Console at their matching log level. It is off by default and does
not replay old events.

- Console output is limited to 50 events per second plus a suppression summary.
- The inspector retains at most 1,000 browser events and displays 250.
- Pausing the view does not pause the native processor or event collection.

### CLI snapshots and Studio

`cargo rvst inspect --format json` returns authenticated live state, metrics and
recent events for discovered instances. Transport credentials are omitted from
this output and from API snapshots; `--open` still opens the private capability
URL directly in your browser.

The shared `rvst_devtools::snapshot(&session, after_sequence)` client validates
loopback capabilities and instance identity, caps responses at 4 MiB and enforces
a total request deadline.

| Consumer | Discovery and polling |
| --- | --- |
| CLI inspection | Checks all eligible records, up to the registry's 256-entry limit. |
| Studio discovery | Probes newer entries first with a 750 ms budget, plus at most one in-flight request. Large or unresponsive registries can produce a partial initial result. |
| Studio live view | Polls once per second on a background worker while a job or selected session remains active. Retains 60 history samples; switching instances resets history. |

Studio shows the newest discovered instance's callback metrics, input/output
peaks, latency, parameter count and a sparkline of its cumulative mean callback
time. An initial discovery pass runs when Studio opens; press `i` to rediscover
after loading a plugin. When idle, the equivalent-command footer and `Ctrl+Y`
copy the selected instance's `inspect --format json` command. See
[CLI status](cli-status.md) for the rest of the workbench controls.

## Register additional read-only state

Register providers during setup. This complete helper returns an atomic counter
that a producer can update; the inspector reads its current value:

```rust
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

fn register_voice_counter(
    session: &rvst_devtools::DebugSession,
) -> std::io::Result<Arc<AtomicU64>> {
    let counter = Arc::new(AtomicU64::new(0));
    let source = Arc::clone(&counter);
    session.register_state_provider(
        "engine",
        move || serde_json::json!({"voices": source.load(Ordering::Relaxed)}),
    )?;
    Ok(counter)
}
```

Providers run on the inspector worker only after API authentication. Read
atomics or copied snapshots; never access mutable DSP or wait for the audio
callback. Registration and JSON construction belong off the audio thread.

A session accepts up to **16 uniquely named providers**, each with a **64 KiB
serialized result limit**. Oversized results and provider panics become visible
errors under `snapshot.live.<name>` while other state remains available.

These independent atomic reads are live observations, not a simultaneous DSP
checkpoint. Use recording/replay for deterministic comparisons.

## Debug source code

| Debug target | Tool and limit |
| --- | --- |
| Local inspector page | Chrome DevTools can inspect requests, rendering and the JavaScript page. |
| WASM UI preview | Chrome DevTools can inspect the preview. Rust source-level WASM debugging depends on browser and DWARF support. |
| Native Rust DSP in a DAW | Use the included CodeLLDB launch/attach configurations, `lldb -p PID`, or Linux `gdb -p PID`. Browser logs do not attach JavaScript breakpoints to native DSP. |

Pausing the audio thread also pauses that plugin and may interrupt the DAW's
audio engine.

The WASM builder retains debug information (`wasm-bindgen --keep-debug`). Test
source stepping with the actual browser/tool versions. References:
[wasm-bindgen debug information](https://wasm-bindgen.github.io/wasm-bindgen/reference/debug-info.html)
and [Chrome's WebAssembly debugging](https://developer.chrome.com/docs/devtools/wasm).

## Record and replay a reproduction

Run these commands inside a generated plugin project:

```sh
cargo xtask record capture.json --blocks 64
cargo xtask replay capture.json
cargo xtask replay capture.json --step
```

The example creates an offline, reproducible fixture containing initial state,
configuration, input/output audio, musical events, transport, precision, statuses
and original block boundaries. `Recording::cursor::<P>(1e-6)` processes one block
per step and retains DSP state; its argument is the replay comparison tolerance.

For live capture, use `recording_channel` and drain on a worker. Choose a
reproducible start:

| Start | Set `Recording.start` to | Requirement |
| --- | --- | --- |
| Reset | `CaptureStart::Reset` | Start from a reset. Replay resets the processor. |
| DSP checkpoint | `CaptureStart::Checkpoint` | `initial_state` must restore the complete processor-specific DSP history. Replay does not reset it. |

Replay requires observed processing status and musical output for every block.
**An incomplete capture cannot claim deterministic verification.** Parameter
presets alone cannot establish an arbitrary live DSP checkpoint. See the
[recording implementation](../crates/rvst-runtime/src/recording.rs) and
[real-time test recipes](realtime.md).
