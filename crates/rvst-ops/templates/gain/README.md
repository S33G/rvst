# Stereo Gain

Start an audio effect with a small stereo processor and a shared parameter editor.

## What you get

- Sample-accurate gain events, adjustable smoothing and bypass.
- 32-bit and 64-bit sample processing.
- Versioned parameter state and focused processor tests.
- An independent workspace with native VST3/CLAP wrappers, a desktop runner,
  browser editor preview and project-owned build tasks.

## Create a project

RVST 0.1.0 is unpublished. Point at an SDK checkout; `--vendor-sdk` includes a
portable snapshot so the generated project can be cloned independently.

```sh
cargo rvst new ./my-gain --template gain \
  --sdk-path /path/to/rvst --vendor-sdk
```

The generated README covers first run, bundling, installation and customization.
See the [template guide](../../../../docs/templates.md) for inputs and SDK choices.
Template files are declarative; there are no executable hooks.
