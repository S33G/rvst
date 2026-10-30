# RVST Debug

Explore the framework with an interactive audio, event, theme and diagnostics tour.

## What you get

- Stereo gain, bypass, smoothing and a built-in tone.
- On-screen MIDI event controls, parameter presets and native meters.
- A shared showcase editor for native hosts, standalone and browser preview.
- An independent workspace with native VST3/CLAP wrappers, a desktop runner,
  browser editor preview and project-owned build tasks.

## Create a project

RVST 0.1.0 is unpublished. Point at an SDK checkout; `--vendor-sdk` includes a
portable snapshot so the generated project can be cloned independently.

```sh
cargo rvst new ./my-debug --template debug \
  --sdk-path /path/to/rvst --vendor-sdk
```

The generated README covers first run, bundling, installation and customization.
See the [template guide](../../../../docs/templates.md) for inputs and SDK choices.
Template files are declarative; there are no executable hooks.
