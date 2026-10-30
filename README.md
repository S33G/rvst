# RVST

**Rust audio plugins. One shared UI. A workbench for humans and agents.**

Build VST3 and CLAP plugins with direct Rust adapters, a skinnable egui editor,
and a Cargo-native CLI/TUI. Run the same editor in your DAW, a standalone app,
or a browser through WebAssembly. Every plugin gets its own repository.

[Get started](docs/getting-started.md) · [Documentation](docs/README.md) ·
[CLI reference](docs/cli.md) · [Contributing](CONTRIBUTING.md)

> **Development preview:** the SDK and CLI are not published on crates.io yet.
> Use the source workflow below. Native validators and Chrome checks pass on
> the tested macOS configuration; [DAW and platform qualification is tracked
> separately](docs/validation-2026-10-30.md).

## Start with RVST Debug

RVST Debug is a hands-on framework tour. Turn a knob, play a note, switch the
skin, recall a preset, then follow the real plugin events into your browser.

Clone the framework, then run one command to create, build, install and open
its separate showcase project:

```sh
git clone https://github.com/S33G/rvst.git
cd rvst
```

```sh
cargo run -p cargo-rvst -- showcase --path ../rvst-debug --sdk-path . --vendor-sdk --install --run --open
```

This installs VST3/CLAP bundles in your **user plugin folders**, starts the
standalone app and opens its local inspector. Start at low gain; raise **Tone**
to hear the oscillator, or try the note buttons on **Events**.

- **Preview the operation:** add `--dry-run --format json`.
- **Build only:** omit `--install --run --open`.
- **Run it again:** `cd ../rvst-debug && cargo run -p plugin-standalone`.
- **Stop:** close the standalone window or press Ctrl-C in its terminal.

The generated repository contains a portable SDK snapshot. It builds without
this checkout or an installed RVST command. See [prerequisites and the complete
tour](docs/getting-started.md) if your machine needs setup.

## Choose your interface

Install the lightweight workbench from the framework checkout:

```sh
cargo install --path crates/cargo-rvst --locked
cargo rvst
```

| Interface | Start here |
| --- | --- |
| Studio TUI | `cargo rvst` in an interactive terminal |
| Agent / script | `cargo rvst --help` and `cargo rvst schema --format json` |
| Generated RVST plugin project | `cargo build --locked`, then `cargo xtask bundle --release` |

The tool does not build native audio or graphical backends during installation.
A [pinned Git install](docs/getting-started.md#4-install-the-workbench) is also
available. RVST uses Cargo's package installation and dependency resolution.

## Make your own plugin

From the **framework checkout**, with the workbench installed:

```sh
cargo rvst new ../my-effect \
  --template gain \
  --vendor Acme \
  --id com.acme.my-effect \
  --sdk-path . --vendor-sdk \
  --non-interactive
cd ../my-effect
cargo build --locked
cargo test --workspace --locked
cargo xtask bundle --release
```

| Starter | Explore |
| --- | --- |
| `debug` | Interactive tour, diagnostics, themes and native/browser workflows |
| `gain` | Parameters, smoothing, bypass, meters and presets |
| `instrument` | Voices, sample-offset notes, pressure and pitch bend |
| `midi` | Event transformation, with pure-MIDI or audio pass-through variants |

Use `cargo rvst templates inspect gain --format json` to discover inputs.
[Template authoring](docs/templates.md) covers local and pinned Git templates.

## Preview the real Rust UI in a browser

From a **generated plugin repository**:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked
cargo xtask web --serve --open
```

Browser v1 uses a deterministic simulated host. It tests the shared Rust editor;
audio runs in the native plugin or standalone app.

The separate [live inspector](docs/diagnostics.md) receives real native plugin
logs, parameters and performance metrics. Chrome DevTools can show those logs;
native DSP breakpoints use LLDB or GDB.

## Explore the framework

| Area | Crates | Guide |
| --- | --- | --- |
| Processing and events | `rvst-core`, `rvst-runtime` | [API contracts](docs/api-contract.md) |
| Native formats | `rvst-adapter`, `rvst-vst3`, `rvst-clap` | [Architecture](docs/architecture.md) |
| Shared UI and runners | `rvst-ui`, `rvst-platform`, `rvst-standalone`, `rvst-web` | [Themes](docs/themes.md) |
| Debugging and tests | `rvst-devtools`, `rvst-test` | [Diagnostics](docs/diagnostics.md), [real-time rules](docs/realtime.md) |
| Tooling | `rvst-build`, `rvst-ops`, `cargo-rvst` | [CLI](docs/cli.md), [templates](docs/templates.md) |

Build API documentation with `cargo doc --workspace --no-deps --open`.
The [documentation index](docs/README.md) links guides, capability reports and
release evidence.

## Open source

RVST and its starters use **MIT OR Apache-2.0**. Proprietary downstream plugins
are allowed; review the licences of the dependencies and assets you distribute.
See [MIT](LICENSE-MIT), [Apache-2.0](LICENSE-APACHE), and
[third-party notices](THIRD_PARTY_NOTICES.md).

[Contributing](CONTRIBUTING.md) explains checks and commit conventions.
[HISTORY.md](HISTORY.md) explains the reconstructed dates in the initial import.
