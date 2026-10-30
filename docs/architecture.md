# Architecture and contracts

[Documentation index](README.md) · [API reference](api-contract.md) · [Real-time rules](realtime.md)

RVST separates plugin DSP, format adapters, shared editors and development tools.
The core contract is [`rvst_core::Plugin`](../crates/rvst-core/src/lib.rs); audio
processing never requires an editor.

## Component boundaries

| Component | Responsibility |
| --- | --- |
| Plugin / `rvst-core` | DSP lifecycle, stable descriptors, parameters, events and plugin-owned state. |
| Native adapters | Host buffer conversion, automation, musical events, native entry points and editor integration. |
| `rvst-runtime` | Smoothing, timing metrics, bounded telemetry, recording and offline replay. |
| `rvst-ui` / `EditorHost` | Shared editor code and host edit gestures. Native window/file services stay outside `rvst-ui`. |
| `rvst-ops` | Shared operations called by the CLI and terminal workbench. |

## Processor lifecycle

1. **Describe:** the static descriptor fixes parameter IDs, format identities and
   bus definitions for v1. Never change a released identity to rename a plugin.
2. **Prepare:** allocate for the maximum block size before processing starts.
3. **Process:** receive packed planar audio, sample-offset input events,
   transport and a bounded output event sink. Tail, activity, bypass and latency
   are explicit processor data.
4. **Reset:** clear DSP history.
5. **Save or restore:** serialize state on the control thread. A failed restore
   must leave the previous state intact.

Parameter manifests remain fixed in v1. A plugin handles unknown state versions
explicitly; the framework does not silently rewrite plugin-owned state.

## Parameters, events and host buffers

Parameters cross core/editor boundaries in **plain units**. VST3 adapters convert
to normalized host automation and retain ramp endpoints. CLAP uses parameter
metadata and plain values. `ParameterRamp` represents linear interpolation in
**normalized space**; processors must not interpret it as linear plain-value DSP.

Host buffers can alias. Adapters copy input into distinct, preallocated storage
before exposing Rust slices, then copy processed output back. Optional buses are
represented explicitly. Events and parameter changes retain sample offsets;
queue overflow for musical data triggers recovery instead of being treated like
lossy telemetry. See the [event contract](api-contract.md#events-and-musical-output).

## Audio and control-thread ownership

Native entry points contain unwind boundaries. No Rust panic may cross a C/COM
ABI. Panic containment is a last line of defence; plugin code still must obey
the [real-time rules](realtime.md).

Live state access uses an exclusive suspension gate. A callback that cannot
acquire the gate emits silence without blocking; state operations wait only on
the control thread. **Saving or loading a large state may briefly interrupt
audio.** A future snapshot/restore protocol can remove that interruption for
processors capable of complete lock-free checkpoints.

## Editors and project tooling

Editors speak `EditorHost`, not format APIs. The native bridge coordinates
atomic parameter values and host gestures; the WASM bridge uses a deterministic
local processor model. See [shared UI status](ui-status.md) for execution and
platform limits, and [theme packs](themes.md) for editor appearance.

Project creation stages files, validates inputs, initializes a separate Git
repository and atomically claims an empty destination. Shared templates never
execute hooks. Building a generated Rust project uses normal Cargo, including
dependencies' build scripts; review untrusted source before choosing to build it.
See [templates](templates.md) and [CLI status](cli-status.md).

## Qualification targets and deferred scope

| Release qualification target | Architecture / window system |
| --- | --- |
| macOS 14.2+ | arm64 and x64 |
| Windows 11 | x64 |
| Ubuntu 24.04 | x64, X11/XWayland |

These are qualification targets, not automatic claims about a local build. See
[native status](native-status.md) and the [release checklist](releases.md) for
validation boundaries.

Browser audio, AU/AAX, MIDI 2, native Wayland, alternative UIs, external DSP ABIs
and reusable modular graphs remain deferred.
