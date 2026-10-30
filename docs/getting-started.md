# Getting started

[Documentation](README.md) / Getting started

The shortest path is **RVST Debug → your own repository → shared UI preview →
native plugin**. Start with the showcase to explore the framework before
writing DSP.

## 1. Prepare the toolchain

Install Rust through rustup and Git. The repository pins **Rust 1.98.0** and
uses Cargo for building, testing and tool installation.

Clone the framework, enter its directory, then check the toolchain:

```sh
git clone https://github.com/S33G/rvst.git
cd rvst
rustup toolchain install 1.98.0 --component rustfmt --component clippy
cargo run -p cargo-rvst -- doctor
```

`doctor` reports prerequisites and setup commands. Native builds additionally
need the target platform's compiler and audio/window libraries. The tool itself
can be installed without compiling those native backends.

> If you have both Homebrew Rust and rustup, check `which cargo` and
> `which rustc`. Install targets for the compiler Cargo actually uses.
> `RUSTUP_TOOLCHAIN` does not redirect a standalone Homebrew compiler.

## 2. Run the RVST Debug tour

From the framework checkout, create the separate showcase repository, install its native
bundles, and open its standalone UI and browser inspector:

```sh
cargo run -p cargo-rvst -- showcase --path ../rvst-debug --sdk-path . --vendor-sdk --install --run --open
```

| Option | What it does |
| --- | --- |
| `--path ../rvst-debug` | Creates the showcase beside the framework checkout |
| `--sdk-path . --vendor-sdk` | Copies the local SDK into a portable plugin repository |
| `--install` | Installs VST3/CLAP bundles in the user's plugin folders |
| `--run` | Runs the native standalone app |
| `--open` | Opens the local native inspector in your default browser |
| `--dry-run --format json` | Shows the plan without creating, building or launching |

Omit `--install --run --open` to build only. Close the window or press Ctrl-C to
stop the native runner. The showcase command can reuse a project it recognises
as an RVST Debug project; `new` always requires a fresh destination.

### Explore the four pages

| Page | Try it |
| --- | --- |
| **Signal** | Start at low Gain, raise Tone above 0 Hz, change Waveform, then test Bypass and the meters. Store a preset, move a knob, then recall it. Turn Log marker to add a numeric breadcrumb. |
| **Events** | Tap a note button. Each note releases automatically. Watch the control-change trail. |
| **Themes** | Switch Dark, Light and Studio. Try keyboard navigation, scaling, high contrast and reduced motion. |
| **Inspect** | Open the native inspector; match the process/instance ID, watch callback timings and enable Console echo. |

For Chrome logs, open the inspector URL in Chrome, open DevTools, select
**Console**, enable **Verbose**, then select **Echo new logs to DevTools**.
The records come from the running native plugin. See [diagnostics](diagnostics.md)
for filtering, timing limits and native debugger workflows.

Standalone has no physical MIDI device integration. The Debug note buttons use
an internal editor command path. To use an external controller, host the native
plugin in a DAW and route MIDI to it.

### Return to the showcase later

From the generated **rvst-debug repository**:

```sh
cargo run -p plugin-standalone
```

No globally installed RVST command is required.

## 3. Preview the shared UI in a browser

From the generated **plugin repository**, install the browser build tools:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked
cargo xtask web --serve --open
```

The `wasm-bindgen-cli` version must match the project's lockfile. The current
starters pin **0.2.130**. If the default port is occupied:

```sh
cargo xtask web --serve --open --port 8788
```

The WASM runner calls the same Rust editor function as the native formats. Its
host simulates parameter and state behavior; it does not run browser audio.
Debug note playback is therefore disabled in this preview. Chrome's native
inspector and this WASM preview are separate tools.

## 4. Install the workbench

From the **framework checkout**:

```sh
cargo install --path crates/cargo-rvst --locked
cargo rvst
```

Bare `cargo rvst` opens Studio only on an interactive terminal. For scripts and
agents, use explicit subcommands; see the [CLI reference](cli.md).

A pinned Git install uses Cargo's normal mechanism:

```sh
RVST_REV=6967784f9bb84527b1a3b0f4963d71c991136051
cargo install --git https://github.com/S33G/rvst.git --rev "$RVST_REV" cargo-rvst --locked
```

That revision is the initial verified preview; use another full commit SHA to
choose a later version. The [verification report](validation-2026-10-30.md)
records the exact earlier revision and profile used in the Git-install test.
The `cargo install cargo-rvst --locked` registry command becomes available only
after publication. Until then, generate with an explicit SDK checkout.

## 5. Create your own plugin

From the **framework checkout**, after installing the workbench:

```sh
cargo rvst new ../my-effect \
  --template gain \
  --vendor 'Example Audio' \
  --id com.example.my-effect \
  --sdk-path . --vendor-sdk \
  --non-interactive
cd ../my-effect
```

Edit `crates/plugin-core/src/lib.rs` for DSP and
`crates/plugin-ui/src/lib.rs` for the shared editor. The generated README maps
the remaining crates and their commands.

Choose the identity before shipping. Preserve plugin IDs, parameter IDs and
state compatibility when renaming a released plugin.

## 6. Use ordinary Cargo workflows

Run these from the **generated plugin repository**:

| Task | Command |
| --- | --- |
| Build | `cargo build --locked` |
| Test | `cargo test --workspace --locked` |
| Check formatting | `cargo fmt --all --check` |
| Lint | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| Create release bundles | `cargo xtask bundle --release` |
| Install bundles for local testing | `cargo xtask install` |
| Create a deterministic offline capture | `cargo xtask record capture.json --blocks 64` |
| Replay one block at a time | `cargo xtask replay capture.json --step` |

A plain bundle does not install, sign or publish anything. Release qualification
needs separate [validator and DAW checks](releases.md).

The recording example is an offline host fixture. Replaying arbitrary live DSP
history requires a complete DSP checkpoint or a controlled reset; a parameter
preset is not sufficient. See [recording and replay](diagnostics.md).

## Troubleshooting

| Symptom | Next step |
| --- | --- |
| SDK package cannot be found on crates.io | Regenerate into a fresh destination with `--sdk-path /path/to/rvst --vendor-sdk`. The SDK is not published yet. |
| Destination already exists | Use a new directory for `new`, or keep editing the generated project with Cargo. Generation never overwrites authored code. |
| Browser build cannot find its target | Run `rustup target add wasm32-unknown-unknown` and check which rustc Cargo uses. |
| `wasm-bindgen` reports a version mismatch | Install the exact CLI version pinned by the plugin's `Cargo.lock`. |
| Debug Console shows no plugin records | Use the native inspector, enable Console echo and Verbose, then generate a new event. Old events are not echoed. |
| Plugin is silent | In Debug, raise Tone or tap a native note button. Gain needs input; an instrument needs notes. Browser preview has no audio. |

Continue with [themes](themes.md), [templates](templates.md), or the
[API contracts](api-contract.md).
