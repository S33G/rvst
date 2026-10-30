# MIDI Transpose

Start a MIDI effect that keeps note releases paired with their transposed notes.

## What you get

- MIDI note transposition with preserved note-off mapping.
- `audio_mode=pure-midi` declares no audio ports (default).
- `audio_mode=pass-through` adds unchanged stereo audio routing.
- An independent workspace with native VST3/CLAP wrappers, a desktop runner,
  browser editor preview and project-owned build tasks.

## Create a project

RVST 0.1.0 is unpublished. Point at an SDK checkout; `--vendor-sdk` includes a
portable snapshot so the generated project can be cloned independently.

```sh
cargo rvst new ./my-midi --template midi \
  --sdk-path /path/to/rvst --vendor-sdk
```

Add `--set audio_mode=pass-through` for stereo pass-through.

The generated README covers first run, bundling, installation and customization.
See the [template guide](../../../../docs/templates.md) for inputs and SDK choices.
Template files are declarative; there are no executable hooks.
