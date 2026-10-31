# RVST documentation

[RVST home](../README.md)

Build a plugin, understand its contracts, then qualify it for the hosts you
intend to support. RVST is a development preview; the guides below distinguish
implemented behavior from tested platform and DAW combinations.

## Start building

| Goal | Read |
| --- | --- |
| Try RVST Debug and preview its Rust UI | [Getting started](getting-started.md) |
| Use the terminal workbench or automate a task | [CLI and TUI reference](cli.md) |
| Build with a coding agent and minimal context | [Agent skills](agent-skills.md) |
| Create or share a starter | [Templates and project generation](templates.md) |
| Change colors, typography and controls | [Shared UI and theme packs](themes.md) |
| Undo control edits, resets and preset recalls | [Undo and redo](undo-redo.md) |
| See native logs, inspect performance or replay a capture | [Diagnostics](diagnostics.md) |
| Install RVST in WSL or route native audio to Windows | [WSL setup and audio](wsl-audio.md) |

## Understand the framework

| Topic | Read |
| --- | --- |
| Crate boundaries, adapters and execution paths | [Architecture](architecture.md) |
| Lifecycle, parameters, buses, events and state | [API contracts](api-contract.md) |
| Audio-thread rules, queues and tests | [Real-time development](realtime.md) |
| Source contribution and local checks | [Contributing](../CONTRIBUTING.md) |

Generate the Rust API reference from the framework checkout:

```sh
cargo doc --workspace --no-deps --open
```

## Check support and evidence

| Question | Read |
| --- | --- |
| What is implemented, and what remains? | [Implementation ledger](implementation.md) |
| Which CLI and studio features exist? | [Workbench status](cli-status.md) |
| What do the native adapters support? | [Native status](native-status.md) |
| What works in the shared UI and runners? | [UI status](ui-status.md) |
| What has actually been verified? | [Integration verification](validation-2026-10-30.md), [Debug native validation](debug-validation.md) |
| How should a release be qualified? | [Release checklist](releases.md) |
| Which third-party obligations need review? | [Dependency notices](../THIRD_PARTY_NOTICES.md) |

A compiled target, a validator pass and a real DAW session are separate pieces
of evidence. Use the reports' recorded artifact, operating system and tool
versions when judging whether a result applies to your plugin.
