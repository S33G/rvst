# Sine Instrument

Start a synthesizer with a polyphonic sine processor and MIDI note handling.

## What you get

- Stereo audio output and MIDI note input.
- Gain control, parameter state and note-release tests.
- A shared parameter editor; play notes through a DAW MIDI track.
- An independent workspace with native VST3/CLAP wrappers, a desktop runner,
  browser editor preview and project-owned build tasks.

## Create a project

RVST 0.1.0 is unpublished. Point at an SDK checkout; `--vendor-sdk` includes a
portable snapshot so the generated project can be cloned independently.

```sh
cargo rvst new ./my-instrument --template instrument \
  --sdk-path /path/to/rvst --vendor-sdk
```

The generated README covers first run, bundling, installation and customization.
See the [template guide](../../../../docs/templates.md) for inputs and SDK choices.
Template files are declarative; there are no executable hooks.
