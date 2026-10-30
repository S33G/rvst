# Native adapter status

[Documentation](README.md) / Native adapter status

Direct CLAP bindings (`clap-sys` 0.5) and VST3 COM bindings (`vst3` 0.3).
No nice-plug backend.

For the final RVST Debug bundles, read the [Debug validation report](debug-validation.md).
The gain results below are an earlier, separately scoped fixture.

## Earlier gain-fixture verification on macOS ARM64

- Generated gain project `/tmp/rvst-cli-gain` builds its real native library and creates both bundles using its generated xtask and `rvst-build`.
- Independent `clap-validator 0.4.1`: **33 passed, 11 skipped, zero failed**. Skips cover unadvertised f64, note input, dynamic port configuration and preset discovery features in the gain fixture. Includes parameter fuzzing, transport fuzzing, sample offsets and state reproducibility.
- Independent `pluginval 1.0.4`, strictness 5, GUI enabled: **SUCCESS**. Includes Editor, Open editor whilst processing, Editor Automation, state, automation and the default sample-rate/block-size combinations. Steinberg's separate validator was not installed or run.
- Both actual exported ABIs dynamically load and process eight blocks in the `clap-host-smoke` and `vst3-host-smoke` examples. This is a native library host, not a DAW.
- Native unit tests cover factories, separate VST3 processor/controller COM creation, in-place buffers, plain/normalized parameter conversion, VST3 implicit sample-minus-one ramps, state streaming in short reads/writes, control/audio exclusivity, bounded event overflow, and zero allocations/deallocations in instrumented callbacks.
- Opt-in `devtools` tests pass. A native instance owns its inspector, emits per-callback timing through a bounded queue, and connects author diagnostic events through `Plugin::connect_diagnostics`.

Raw validator evidence is retained under ignored `.local/native-validation/`.

## Implemented behavior

### Instance association

VST3 component/controller association uses `IConnectionPoint` and standard `IMessage` token exchange. Each processor registers a unique opaque token against a weak reference to its own control bridge. The controller resolves only that exact token; no process-global "latest instance" pointer exists. Matching transfers the plugin-owned `EditorContext`, live parameter mirror, meters and inspector URL. Disconnect clears the controller binding; processor destruction removes the registry entry. Unit tests cover two instances, reversed connect order, Arc identity, disconnect and unknown remote tokens. A separate-process controller retains normal host-driven parameters/state and reports custom context as unavailable.

### Lifecycle, parameters and events

Both formats expose real factories, lifecycle, f32 processing, optional f64 dispatch, parameter automation, note events, opaque host state, latency/tail queries, bypass, and native editor lifecycles. VST3 uses separate component/controller classes and Factory2 metadata; the controller works headlessly. VST3 MIDI input uses stable hidden controller parameters and `IMidiMapping`, including channel pressure and pitch bend. Channel-mode releases and CLAP wildcard/choke events retain note scope.

CLAP parameter values use plain units by default. Explicit `AdapterCompatibility` policies preserve legacy normalized/stepped wire values, length-prefixed state and output-port identities. Plugins with auxiliary inputs expose full-input and main-only CLAP configurations; omitted auxiliary buffers become zero-filled internal channels. VST3 honors bus activation and accepts omitted inactive buses. Neither adapter changes the plugin's declared internal channel offsets.

### Latency and transport

Latency changes are detected after processing. VST3 forwards a latency restart notification on the controller's owning thread. CLAP requests a host restart once while active, then reports the new latency during the next host activation, as required by the CLAP latency extension. Detection itself never reparses state or prepares/resets the plugin. CLAP transport events update the context at their exact sample offsets; tempo ramps use per-sample contexts and stopped song positions do not advance.

### Editor threading and timing

Linux VST3 editor gestures use the host's `IRunLoop` to call host methods on the main thread. Bounded per-parameter mailboxes coalesce values; reentrant editor removal and timer destruction finish active gestures while retaining the host handler.

With native `devtools`, editor CPU duration is recorded into the instance inspector's UI metrics on the UI thread. It measures the plugin editor function, excluding backend rendering, GPU work and presentation. The optional timing adds clock reads and atomic counter updates; when no metrics sink is attached, the editor path performs no timing reads.

### Buffer and event safety

Audio buffers are copied into disjoint, preallocated storage before safe Rust slices are formed. Blocks without events or CLAP tempo ramps use a whole-block path. Eventful blocks use one-sample chunks to preserve exact offsets and VST3 ramps. Input event overflow resets voices and emits AllNotesOff; failed output note releases latch an emergency release for the next callback.

### State and presets

State stream reading/writing runs off audio. Coherent save/load acquires a control ownership gate, with at most 100ms control-thread waiting. Audio never waits: a callback overlapping the actual plugin save/load emits silence. On resumption, voices reset and output note releases are retried, preventing missed note-offs during suspension. Editor Store/Recall uses a separately versioned, fully validated **parameter preset**, not opaque DAW state.

### Packaging

`rvst-build` discovers Cargo JSON artifacts, creates macOS bundles and Windows/Linux artifacts, and offers explicit feature builds and staged user-directory installation. No signing, notarization or publication occurs implicitly.

## Remaining qualification and limits

- Real DAW behavior and Windows/Linux native GUI/runtime qualification remain unverified. VST3's macOS editor passed pluginval; CLAP GUI protocol needs an independent GUI-host run.
- Dense automation uses per-sample plugin calls and queue scans; benchmark/optimization work remains. Event-free blocks use the fast block path.
- State operations can briefly silence audio. A shared immutable snapshot/restore protocol could remove that tradeoff; it is not currently part of the core API.
- `set_parameter`, `reset`, `latency`, and `tail` must obey real-time rules when adapters call them from audio. Adapter guards cannot make allocating plugin code real-time safe.
- Arbitrary dynamic channel layouts, per-note modulation, MIDI2, SysEx and note-expression extensions are not implemented. CLAP supports selection between its declared full-input and main-only layouts while inactive; its optional legacy policy exposes global modulation. VST3 has no direct per-note choke event, so a targeted choke is emitted as a note-off.
- Rejected parameter errors are retained for control-thread reclamation. The fixed retirement buffer holds 64 errors; overflow deliberately leaks the rejected payload rather than destroying heap memory on audio. Plugin setters must still avoid allocating their errors inside audio callbacks.
- CLAP embeds Cocoa/Win32/X11 windows; floating and Wayland GUI support are absent.
- CLAP output note-release recovery and VST3 output emergency CC123 recovery are bounded; a host that continually rejects all output events cannot be forced to release notes.
