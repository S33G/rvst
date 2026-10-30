# Integration verification — 30 October 2026

[Documentation](README.md) / Integration verification — 30 October 2026

These checks ran on macOS 26.7.1, Apple Silicon, with Rust 1.98.0. They describe
a development preview, not a qualified cross-platform release. The initial
October commit dates are reconstructed as explained in [HISTORY.md](../HISTORY.md);
this report uses the actual verification date.

## Framework

At source revision `374e9b1b1ee81cbff3dd338017b99c60c48f41cf`:

- `cargo test --workspace --all-features --locked --offline`: **111 passed**, zero failures.
- `cargo clippy --workspace --all-targets --all-features --locked --offline -- -D warnings`: passed.
- `cargo fmt --all --check`: passed.
- Two Node tests cover the browser inspector Console echo and bounded history.
- The Windows x86_64 MSVC check passed for both native adapters with all features;
  this was cross-compilation, not Windows execution.

Tests include real-time allocation/deallocation guards, queue saturation and
musical-event recovery, aliased native buffers, sample-offset processing, state
failures, authentication and request framing, template traversal rejection,
JSON output, process cancellation and keyboard gestures. See the source tests
for the scope of each claim.

## Git install and generation

The actual public Git source was installed, using a separate installation root:

```sh
cargo install --git https://github.com/S33G/rvst.git \
  --rev 374e9b1b1ee81cbff3dd338017b99c60c48f41cf \
  cargo-rvst --locked --debug --root /tmp/rvst-github-install-374e9b1
```

This tested the **debug installation profile**, with cached dependencies. From
that installed binary, offline template listing/checking and generation passed
for Debug, gain, instrument and MIDI. Every generated workspace contained its
lockfile, Cargo alias, CI example, debugger configuration, Git ignore file and
vendored dependency notices. Locked offline Cargo metadata passed in all four.
The CLI installation does not compile the native audio or graphical backends.

Each maintained example lives in its own Git repository: `rvst-debug`,
`rvst-gain`, `rvst-instrument` and `rvst-midi`. SDK vendoring makes those projects
independent of the framework checkout. Registry-mode projects require the SDK
packages to be published; that publication has not happened.

The Debug repository at `40c7517808b10213cf109f7ae1232d06890c047c` was cloned
into a path containing spaces. With `cargo-rvst` absent from `PATH`, cached
offline dependencies and isolated target directories, normal Cargo build,
workspace tests (**8 passed**), formatting, strict Clippy, `cargo xtask bundle`
and `cargo xtask web` all passed. Tracked source and `Cargo.lock` were unchanged.
WASM used the rustup compiler that owned its target. Clippy used its matching
compiler; an initial Homebrew/rustup mismatch was corrected without code changes.

Reference repositories passed isolated formatting, locked offline builds,
strict Clippy and native bundling. Loaded CLAP descriptors and VST3 component
identities matched each project's source:

| Repository | Revision | Core tests | CLAP identity |
| --- | --- | ---: | --- |
| rvst-gain | `47908bc` | 2 | `org.rvst.gain` |
| rvst-instrument | `cac86f1` | 3 | `org.rvst.instrument` |
| rvst-midi | `360408c` | 4 | `org.rvst.midi` |

Their code also passed fresh-clone builds. Each reference repository keeps its
own native identity/hash receipt under `docs/native-identities.json`.

## Native showcase

The one-line showcase operation generated a separate Debug repository, built
and installed VST3/CLAP bundles, and launched the native app and local inspector.
The final installed development binaries passed **39 CLAP checks, with 5 skipped
and none failed**, and **pluginval strictness 5 with GUI tests enabled**. The
[artifact-specific report](debug-validation.md) records hashes, commands and
skipped checks.

The Debug processor has seven core tests. A separate standalone regression
verifies that a note requested through the editor context actually produces
samples and releases, without an external MIDI output device.

## Real browser and terminal interactions

In Chrome, the actual shared Rust/WASM Debug editor was exercised with native
mouse and keyboard input:

- Three Right-arrow presses changed Gain from 0.50 to 0.56 and left Tone at 0.
  This exposed and then verified a fix for unwanted spatial focus navigation.
- Store at 0.56, change to 0.60, recall: Gain returned to 0.56.
- The event view recorded the parameter changes and preset recall.
- Dark, Light and Studio themes rendered; SVG theme artwork loaded.
- The browser event view disabled note playback and explained its native-only
  audio boundary.

Chrome's inspector showed the running native process, parameters, callback
timing and real `[RVST plugin]` Console events including Output peak records
with advancing sample clocks. Console Verbose must be enabled for debug-level
records. A PTY run of the Ratatui workbench discovered the native instance,
showed its telemetry/logs and restored the terminal when quit.

These were Chrome checks. Firefox, Safari, screen-reader behavior, minimum-OS
rendering and real DAW embedding are not qualified by them. The WASM canvas does
not currently expose a complete browser accessibility tree.

## Hollow Point migration

Hollow Point remains in its own repository. Legacy fixtures were captured before
migration; its preserved identities, parameter/state compatibility and adapter
policies have dedicated regression coverage. Migration validation passed 163
default tests, 63 guard-feature tests, formatting, strict Clippy and native ABI /
guard-page checks. Its own migration report records the exact baseline and
contracts. The later shared-control keyboard fix was built and checked there.
No real DAW session was used to qualify the migration.

## Build isolation

During verification, a shared `CARGO_TARGET_DIR` exposed collisions between the
identically named `xtask` and `plugin-native` outputs in separate example
workspaces. Earlier shared-directory bundle results were rejected. Final
bundle checks use separate target directories and inspect actual plugin
identities. The generated-project verification script now enforces separate
output directories for every project and variant. Its process-level regression
failed before this fix and passed after it for all five variants. A real offline
gain run then passed generation, formatting, build, tests and strict Clippy
without modifying the generated lockfile.

## Remaining release gates

- REAPER, Bitwig, Ableton Live and Cubase sessions, recorded by OS, architecture,
  plugin format and category; no DAW passes are claimed here.
- Windows and Linux runtime/audio/editor checks; Intel macOS and minimum macOS
  version checks; Firefox and Safari browser checks.
- Full accessibility qualification, long-running performance/overhead baselines,
  and host-specific resize/focus/scale behavior.
- Review of dependency packages without shipped notice texts and final
  distribution assets; dependency metadata alone is not a completed legal audit.
- SDK/crates.io publication, release CI runs, signing and notarization. Bundles
  are development artifacts; signing/notarization is not claimed.

Browser audio, AU/AAX, MIDI 2, native Wayland, alternate UI toolkits, external DSP
ABI and a reusable modular graph remain outside this version's scope.
