---
name: rvst-debug
description: Use when an RVST plugin is silent, freezes, clicks or mishandles automation/state, or when inspecting native logs, replaying DSP or checking release readiness.
---

# Diagnose RVST

Resolve this skill directory's real path: `../..` is the SDK checkout.
Check the generated repository's `rvst-project.json` and SDK dependency. Get
host/version, OS, format, artifact, routing, sample rate and block size. Ask for
missing evidence while investigating source. Reproduce reported buses before
changing wrappers.

## Pick the failing layer

| Symptom | First check |
| --- | --- |
| Chrome works, native silent | WASM preview has no audio. Effects need input; instruments need notes. |
| Standalone effect silent | In the plugin repo: `cargo run -p plugin-standalone --locked --features devtools -- --tone 220`. Start at low volume. |
| DAW silent/frozen | Callback progress, input/output peaks, applied parameters; then active buses, precision, lifecycle and state suspension. |
| Clicks/automation/state | Sample offsets, smoothing, parameter units, state rollback; [contracts](../../docs/api-contract.md). |
| Real-time fault | [Allocation and queue tests](../../docs/realtime.md); instrument numeric events, format off audio. |

## Inspect or replay

From the SDK checkout, discover **local** devtools-enabled native instances:

```sh
cargo run --locked -p cargo-rvst -- inspect --format json
```

Select one with `--instance ID`. This does not inspect another computer's DAW.
Read needed JSON fields; retain full logs as artifacts. For missing instances,
enable `devtools` in the native build. [Inspector details](../../docs/diagnostics.md#enable-and-open-the-inspector).

From the plugin repo, using a fresh capture filename:

```sh
cargo xtask record capture.json --blocks 64
cargo xtask replay capture.json --step
```

This creates an **offline fixture**, not a DAW capture. Live replay needs a
complete reset/checkpoint and no dropped blocks/events.
[Capture details](../../docs/diagnostics.md#record-and-replay-a-reproduction).

## Establish evidence

Add a regression matching the failure; for silence, assert expected signal,
not merely finite samples. Test the changed crate and affected wrapper.
`cargo rvst validate` runs Cargo tests only. Packaging, format validators and
real DAW qualification are separate. Record artifact hash and actual OS/host
for release evidence; [qualification checklist](../../docs/releases.md).

Report: cause/hypothesis; change; tests run; missing host evidence and next action.
A Mac pass cannot qualify Windows/Ableton.
