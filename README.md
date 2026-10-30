# RVST

**Rust plugins. One shared UI. A workbench built for humans and agents.**

RVST is an open-source VST3/CLAP SDK, egui UI toolkit and Cargo workbench. Each
plugin lives in its own repository. The framework owns its native adapters;
there is no nice-plug backend. Licensed **MIT OR Apache-2.0**, including the
starters. Downstream plugins may use their own licence.

## Start with RVST Debug

RVST Debug is an interactive feature tour: real audio, parameters, themes,
MIDI, presets and a browser inspector showing logs from the native instance.
Start quietly, turn a control, follow the event, inspect the callback timing.
The browser preview runs the **same Rust editor** against a simulated host.

This repository is a development preview, **not yet published on crates.io**.
From an RVST checkout, build/install/start the separate showcase repository:

```sh
cargo run -p cargo-rvst -- showcase --path ../rvst-debug --sdk-path . --vendor-sdk --install --run --open
```

This explicitly installs VST3/CLAP bundles in your user's plugin folders,
starts the standalone app, and opens its local inspector. Close the window or
press Ctrl-C to stop. To build without installing or opening anything, omit
`--install --run --open`. Use `--dry-run --format json` to inspect the plan.
`--vendor-sdk` makes the generated repository independent of this checkout.

For the interactive studio console:

```sh
cargo install --path crates/cargo-rvst --locked
cargo rvst
```

After a release is published, the intended first-run commands are:

```sh
cargo install cargo-rvst --locked
cargo rvst showcase --install --run --open
```

A pinned Git installation uses ordinary Cargo:
`cargo install --git https://github.com/S33G/rvst --rev "$RVST_REV" cargo-rvst --locked`.
Set `RVST_REV` to the commit you want to install. Until SDK crates are published,
pass an explicit checkout through `--sdk-path` when generating a project so
it can build; add `--vendor-sdk` to make it independent of that checkout.
The CLI does not depend on a native audio device or graphical backend.

## Make your own plugin

```sh
cargo rvst templates list
cargo rvst new my-effect --template gain --vendor Acme --id com.acme.my-effect --sdk-path . --vendor-sdk --non-interactive --format json
cd my-effect
cargo build --locked
cargo test --workspace --locked
cargo xtask bundle --release
cargo xtask web --serve --open
```

Browser builds need `rustup target add wasm32-unknown-unknown` and a
`wasm-bindgen-cli` version matching the lockfile; `cargo rvst doctor` reports
prerequisites. The currently pinned version is **0.2.130**:
`cargo install wasm-bindgen-cli --version 0.2.130 --locked`.

Generated projects have separate processor, UI, native, standalone, web and
xtask crates. They remain normal Cargo workspaces. RVST need not be installed
to build, test, package, record, replay or preview them.

| Crate | Responsibility |
| --- | --- |
| `rvst-core` | Plugin lifecycle, permanent identities, parameters, buses, transport and events |
| `rvst-runtime` | Smoothing, bounded telemetry, timing histograms, capture and block replay |
| `rvst-adapter` | Shared native buffer, parameter and state coordination |
| `rvst-vst3` / `rvst-clap` | Direct format adapters and ABI exports |
| `rvst-ui` | Shared egui controls and versioned theme packs |
| `rvst-platform` | Native editor embedding and thread-safe control bridge |
| `rvst-standalone` / `rvst-web` | Native audio app and WASM simulated-host preview |
| `rvst-devtools` | Local authenticated browser log and performance inspector |
| `rvst-build` / `rvst-test` | Cargo packaging/web helpers and offline/real-time test harness |
| `rvst-ops` / `cargo-rvst` | Shared operations, template engine, CLI and Ratatui workbench |

## Documentation

- [Getting started and the Debug tour](docs/getting-started.md)
- [Architecture and adapter contracts](docs/architecture.md)
- [Audio-thread rules and testing](docs/realtime.md)
- [Chrome inspection, logging and replay](docs/diagnostics.md)
- [Shared UI and themes](docs/themes.md)
- [Template authoring](docs/templates.md)
- [Release and qualification](docs/releases.md)
- [Implementation ledger and limitations](docs/implementation.md)
- [Verified builds, browser checks and remaining qualification](docs/validation-2026-10-30.md)
- [Dependency and asset notices](THIRD_PARTY_NOTICES.md)

Run `cargo doc --workspace --no-deps --open` for Rust API documentation.
Run `cargo rvst schema --format json` for the machine contract.

## Contributing

Use the pinned Rust toolchain. Run formatting, workspace tests and Clippy before
submitting a change. Include a regression test for a meaningful behavior change.
Keep audio callbacks free of allocation, destruction, locks and I/O. Keep effect
algorithms and modular graphs in plugin repositories. Every implementation
commit includes the exact trailer `Co-Author: Charlie's Agent`.

Passing unit tests does not qualify a DAW/platform combination. The release
matrix records compiled, validator-tested and DAW-tested results separately.
