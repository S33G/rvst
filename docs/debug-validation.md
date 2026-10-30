# RVST Debug native validation — 2026-10-30

[Documentation](README.md) / RVST Debug native validation — 2026-10-30

The permanent `../rvst-debug` project was generated with a vendored SDK and its
installed development bundles were tested on **macOS 26.7.1 (25G241), arm64**.
The final validator rerun covers the rebuilt bundles after the keyboard-gesture
and inspector favicon fixes. They were built with the project’s own
`rvst-debug/target` directory. This is current Debug evidence, separate from the
earlier gain-fixture results.

| Check | Result |
| --- | --- |
| CLAP validator 0.4.1 | 39 passed, 5 skipped, 0 failed; exit 0 |
| pluginval 1.0.4, strictness 5, GUI enabled | SUCCESS; exit 0 |
| Windows x86_64 MSVC adapter check, all features | `cargo check` passed; no Windows execution |
| Native adapters/platform/standalone strict Clippy | All targets and features, `-D warnings`, passed |

Validated installed bundles:

- `~/Library/Audio/Plug-Ins/CLAP/rvst-debug.clap`
- `~/Library/Audio/Plug-Ins/VST3/rvst-debug.vst3`

The installed binaries contain the inspector favicon response and Console hint.
Both `Contents/MacOS/rvst-debug` binaries had the same SHA-256 before and after
validation:

```text
493b93a6b7beb525ec51a66f1f183d865b52f7da40e5a72bce6eef225bd09339
```

CLAP skipped three unimplemented preset-discovery checks, plus the
`audio-ports-activation` and `configurable-audio-ports` extensions. The implemented
full/main-only configuration extension was tested. No warnings or failures were
reported.

pluginval ran Editor, Open editor whilst processing, Audio processing, Plugin
state, Automation and Editor Automation checks. It inspected double-precision
support and stereo main/auxiliary inputs, enabled all buses, disabled non-main
buses and restored the default layout. Its separate Steinberg-validator step was
skipped because no Steinberg validator path was configured. This was a VST3 run,
not AU qualification.

Raw reports remain under ignored `.local/native-validation/`:

- `clap-debug-final.json` and `clap-debug-final.stderr.log`
- `pluginval-debug-final-console.log` and `pluginval-debug-final`
- `debug-final-sha256.txt` and `debug-final-summary.json`

The validation commands used the installed bundle paths:

```sh
clap-validator validate --json "$HOME/Library/Audio/Plug-Ins/CLAP/rvst-debug.clap"
env -u SKIP_GUI_TESTS -u DISABLED_TESTS pluginval --strictness-level 5 \
  --validate "$HOME/Library/Audio/Plug-Ins/VST3/rvst-debug.vst3"
```

These passes do not qualify a real DAW/platform combination, a physical MIDI
controller, CLAP GUI hosting, Windows runtime behavior, signing/notarization or
release performance. They validate these development binaries with these tools.
