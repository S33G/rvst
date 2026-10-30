# RVST implementation ledger

[Documentation](README.md) / RVST implementation ledger

RVST 0.1.0 is a development preview using Rust 2024 and the pinned Rust 1.98
toolchain. The implemented scope is a Rust-owned VST3/CLAP framework, shared
native/WASM egui editors, a standalone runner, and one operations layer for the
CLI and terminal workbench. The SDK is not yet published on crates.io.

This ledger separates source capabilities from artifact and host qualification.
Current Debug, browser and install results are recorded in the
[30 October verification report](validation-2026-10-30.md). An implemented API
or a fixture test does not establish real DAW acceptance.

## Implemented capabilities

| Area | Current implementation | Boundary |
| --- | --- | --- |
| Core API | Static plugin/parameter/bus identities; f32 and optional f64 processing; sample-offset events and normalized ramps; transport; latency/tail; plugin-owned state; editor context and numeric diagnostics | Plugin implementations must obey the real-time and state-restore contracts. Parameter/bus declarations are fixed; this is not a modular DSP graph API. |
| Runtime and test support | Sample-count smoothing, atomic timing histograms, bounded telemetry and recording channels, allocation/deallocation guards, offline host, block-by-block replay | Replay needs complete audio/events/status observations and a reset start or a complete processor-specific DSP checkpoint. A live parameter snapshot is insufficient. |
| Native formats | Direct VST3 COM and CLAP factories, lifecycle, parameters, MIDI, state, optional buses, editor embedding, instance association, and latency notifications | No external plugin-framework backend. Format support and cross-compilation do not establish DAW or native GUI acceptance. |
| Native processing safety | Disjoint preallocated audio scratch, explicit musical-event overflow recovery, exclusive control/audio ownership, off-thread error retirement, panic containment | State access can briefly silence callbacks. Dense automation uses small processing chunks. Plugin code and rejected-error creation must still avoid audio-thread allocation. |
| UI and standalone | Shared editor function; themed controls and session presets; per-instance editor context; native metering; CPAL output with silence, test tone or WAV input; finite runs | No microphone or physical MIDI device integration. External MIDI is routed through a DAW. Asynchronous file services are an extension contract, not built-in dialogs. |
| Web preview | The shared Rust editor runs in an eframe/WASM canvas with a deterministic control/state host; Cargo/wasm-bindgen build and loopback serving helpers | No browser DSP/audio. Native source debugging uses a native debugger; browser DevTools do not attach JavaScript breakpoints to native Rust DSP. |
| Diagnostics | Authenticated loopback inspector; named plugin events; live parameter, meter and latency providers; callback/editor timing; bounded Console echo; token-free CLI snapshots | Providers run off audio. Timing adds optional overhead; UI duration excludes backend/GPU presentation. Live atomic observations are not simultaneous DSP checkpoints. |
| CLI and TUI | Shared generation/build/test/diagnostic operations; versioned JSON/JSONL output and schemas; explicit dry runs; process-tree cancellation; creation wizard; command copy; background telemetry and 60-sample sparkline | Studio requires an interactive terminal. Its bounded discovery pass can return a subset of a large/unresponsive registry. |
| Templates and packaging | Embedded Debug, gain, instrument and MIDI templates; independent Cargo/Git roots; typed Liquid inputs; pinned Git/local registration; portable path validation; atomic generation; built-in Rust formatting; explicit SDK override or vendoring; exact Cargo artifact selection | Generation requires rustfmt. Registry-mode builds need published SDK packages; source/vendored builds need their locked dependencies available. Building a project executes normal Cargo build scripts. |
| Installation and compatibility | Explicit user-scope native installation; legacy adapter policies for parameter/state/port identities | Bundling does not install, sign, notarize, publish or push. Hollow Point migration and historical project compatibility require their own artifact/host evidence. |
| Licensing | Original code/templates use MIT OR Apache-2.0; dependency inventory and collected upstream notice texts are included and preserved in SDK snapshots | Metadata and collected notices are audit inputs. Packages without shipped notice files, target/feature choices and asset terms still need release review. |

See [API contract](api-contract.md), [architecture](architecture.md),
[real-time rules](realtime.md), [native status](native-status.md),
[UI status](ui-status.md), [CLI status](cli-status.md),
[templates](templates.md), and [diagnostics](diagnostics.md) for details.

## Evidence and remaining acceptance

Automated tests exercise the implemented API and adapter contracts, aliased host
buffers, event recovery, control/audio exclusion, allocation/deallocation counts,
record/replay completeness, live diagnostics, HTTP authentication/framing,
subprocess cancellation, template path safety, generated formatting, machine
output, and terminal rendering/polling. The generated-project regression script
checks each template, both MIDI modes and optional vendored SDKs with locked
builds, tests and Clippy. CI definitions cover native and WASM builds; a CI
configuration is not evidence that those jobs have run successfully.

The [native status report](native-status.md) records the separately scoped macOS
ARM64 gain-fixture smoke-host and validator results. Keep those results tied to
their tested fixture and artifact. The [Debug validation report](debug-validation.md) records the installed
showcase binaries and their hashes. The [integration report](validation-2026-10-30.md)
records browser interactions, install checks and Hollow Point migration evidence.

Release qualification targets are macOS 14.2+ arm64/x64, Windows 11 x64 and
Ubuntu 24.04 x64 with X11/XWayland. Real DAW acceptance, Windows/Linux native
window/audio behavior, accessibility and host-specific resize/scale/keyboard
behavior remain unqualified unless an exact result is recorded. AU/AAX, native
Wayland, browser audio, MIDI 2/SysEx and arbitrary dynamic layouts are outside
the current implementation. Use the [release checklist](releases.md); do not
promote compile, unit-test or historical compatibility results into DAW passes.

## Repository conventions

Use the pinned toolchain and locked dependencies, for example
`cargo +1.98.0 test --workspace --locked`. The normal Cargo cache is sufficient;
no machine-specific cache directory is required. Ensure the selected rustc owns
the requested cross-compilation target. Every implementation commit includes
the exact trailer
`Co-Author: Charlie's Agent`. Inspector capability URLs remain private; machine
snapshots omit them.
