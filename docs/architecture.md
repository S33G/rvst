# Architecture and contracts

A processor implements `rvst_core::Plugin`. Its static descriptor fixes parameter
IDs, format identities and bus definitions for v1. Prepare allocates for the
maximum block size. Processing receives packed planar storage, sample-offset
input events, transport and a bounded output event sink. Reset clears DSP
history. State serialisation is control-thread work and a failed restore must
leave the previous state intact. Never change a released identity to rename a
plugin.

Parameters cross core/editor boundaries in **plain units**. VST3 adapters convert
to normalised host automation and retain ramp endpoints. CLAP uses parameter
metadata and plain values. `ParameterRamp` represents linear interpolation in
normalised space; processors must not interpret it as linear plain-value DSP.
Parameter manifests remain fixed in v1. A plugin handles unknown state versions
explicitly; the framework does not silently rewrite plugin-owned state.

Host buffers can alias. Adapters copy input into distinct, preallocated storage
before exposing Rust slices, then copy processed output back. Optional buses are
represented explicitly. Events and parameter changes retain sample offsets;
queue overflow for musical data triggers recovery instead of being treated like
lossy telemetry. Tail, activity, bypass and latency are explicit processor data.

Native entry points contain unwind boundaries. Panic containment is a last line
of defence, not real-time correctness. No Rust panic may cross a C/COM ABI.

Live state access uses an exclusive suspension gate. A callback that cannot
acquire the gate emits silence without blocking; state operations wait only on
the control thread. Thus saving/loading a large state may briefly interrupt
audio. A future snapshot/restore protocol can remove that interruption for
processors capable of complete lock-free checkpoints.

Editors speak `EditorHost`, not format APIs. The native bridge coordinates
atomic parameter values and host gestures; the WASM bridge uses a deterministic
local processor model. Native window/file services stay outside `rvst-ui`.

The CLI and TUI call `rvst-ops`. Project creation stages files, validates inputs,
initialises a separate Git repository and atomically claims an empty destination.
Shared templates never execute hooks. Building a generated Rust project uses
normal Cargo, including dependencies' build scripts; review untrusted source
before choosing to build it.

The supported release targets are macOS 14.2+ arm64/x64, Windows 11 x64 and
Ubuntu 24.04 x64 X11/XWayland. These are qualification targets, not automatic
claims about a local build. Browser audio, AU/AAX, MIDI 2, native Wayland,
alternative UIs, external DSP ABIs and reusable modular graphs remain deferred.
