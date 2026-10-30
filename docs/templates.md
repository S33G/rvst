# Templates and independent projects

[Documentation](README.md) · [Getting started](getting-started.md)

`cargo rvst new` creates a separate Cargo workspace and, by default, a new Git
repository. Choose a starter, supply an SDK, then work in the generated project
with normal Cargo commands. The generated README is its day-to-day guide.

## Choose a starter

| Template | Best starting point | Included behavior |
| --- | --- | --- |
| `debug` | Explore RVST interactively | Gain, tone, events, themes and diagnostics tour |
| `gain` | Build an audio effect | Stereo gain, bypass and sample-accurate parameter events |
| `instrument` | Build a synthesizer | Polyphonic MIDI-controlled sine instrument |
| `midi` | Build a MIDI effect | Transpose with preserved note-off mapping |

Each starter separates the processor, shared egui editor, VST3/CLAP wrappers,
standalone runner, browser editor and project-owned `xtask`. Browser previews
simulate the host UI; native runners process audio.

```sh
cargo rvst templates list
cargo rvst templates inspect gain
```

Builtin templates are embedded in the installed executable. The generator does
not need a framework checkout to list or render them; compiling a generated
project still needs its SDK dependencies.

## Choose the SDK explicitly

**RVST 0.1.0 is currently unpublished.** The generator defaults to registry
references, but those references are not usable until the packages are published.
Use one of the source modes below today.

| Generation options | Generated dependencies | Use when |
| --- | --- | --- |
| `--sdk-path /path/to/rvst --vendor-sdk` | Relative paths into `vendor/rvst` | Share or clone an independent project |
| `--sdk-path /path/to/rvst` | Absolute paths to that checkout | Develop the SDK and plugin together |
| Neither option | Registry versions | A compatible SDK release has been published |

Create a portable starter with a deliberate name, vendor and plugin identity:

```sh
cargo rvst new ./my-gain --template gain \
  --sdk-path /path/to/rvst --vendor-sdk \
  --set plugin_name='My Gain' --vendor 'Example Audio' --id com.example.gain
cd my-gain
cargo run -p plugin-standalone --locked -- --tone 220
```

The output directory sets `crate_name`, while `plugin_name` has the template's
display-name default unless overridden. For example, a gain starter is named
**Stereo Gain** by default, regardless of the directory name.

`--vendor-sdk` requires `--sdk-path`. It snapshots SDK crates, embedded templates,
docs, licenses and third-party notices. Commit `vendor/rvst` with the generated
project: its relative dependencies work in another clone without the original
checkout. Third-party Cargo dependencies are not vendored by this option.

The generated workspace excludes the vendored SDK workspace. An absolute SDK
path remains tied to its local checkout. Neither mode silently discovers or
adopts an SDK from a parent directory.

### Lockfiles and offline builds

Builtin lockfiles cover the source and vendored SDK modes. A new project can run
`cargo build --locked --offline` when its toolchain and pinned dependencies are
already installed or cached. Offline does not mean dependency-free.

Before shipping registry-backed templates, regenerate their lockfiles against
the published packages. Generation does not make an unpublished SDK available.

## Customize project inputs

Pass repeatable `--set NAME=VALUE` options. Common builtin inputs are:

| Input | Default or purpose |
| --- | --- |
| `crate_name` | Output directory name; Rust project identifier |
| `plugin_name` | Template display name shown to hosts |
| `vendor` | Vendor label; `--vendor` is a shorthand |
| `plugin_id` | `org.rvst.<crate_name>`; `--id` is a shorthand |
| `url`, `email` | Vendor website and support address |
| `audio_mode` | MIDI template only: `pure-midi` or `pass-through` |

`pure-midi` declares no audio ports. `pass-through` adds unchanged stereo audio:

```sh
cargo rvst new ./my-midi --template midi \
  --sdk-path /path/to/rvst --vendor-sdk \
  --set audio_mode=pass-through
```

Unknown names, invalid values, missing required inputs and duplicate assignments
are errors. Use `templates inspect` to see a template's typed inputs before
creating a project.

### Identity and provenance

Keep `rvst-project.json` with the project. It records template origin/version,
the resolved Git SHA for Git registrations, input values, SDK compatibility and
whether the SDK is an external checkout or a vendored snapshot.

The generated `Cargo.toml` records the bundle name and plugin ID. The descriptor
in `crates/plugin-core/src/lib.rs` contains the plugin ID and VST3 class identity.
Preserve shipped plugin, class and parameter IDs when changing display names;
existing host sessions and automation depend on those identities.

### Destination safety

Generation stages files in a sibling directory, then atomically publishes the
new project. An existing destination is a conflict (exit 4), never a merge target.
The new `[workspace]` and default Git repository keep it separate from parent
workspaces. Use `--no-git` when you deliberately want to manage Git yourself.

Preview the operation without creating it:

```sh
cargo rvst --format json --dry-run new ./planned --template instrument \
  --sdk-path /path/to/rvst --vendor-sdk
```

## Author a template

A template is a manifest plus declarative files. It has no executable hooks.

```text
my-template/
  rvst-template.toml
  README.md
  files/
    Cargo.toml.liquid
    src/lib.rs.liquid
```

The top-level README describes the template. Put `README.md.liquid` inside
`files/` if you also want a README in each generated project.

### Manifest and typed inputs

```toml
schema_version = 1
version = "1.0.0"
sdk_version = "^0.1"
id = "my-template"
name = "My template"
description = "A declarative project template"

[inputs.crate_name]
type = "identifier"
description = "Project identifier"
required = true

[inputs.voice_count]
type = "integer"
description = "Number of voices"
default = 8

[inputs.precision]
type = "choice"
description = "Preferred precision"
choices = ["f32", "f64"]
default = "f32"
```

| Schema 1 type | Constraint |
| --- | --- |
| `string` | Text value |
| `identifier` | Lowercase ASCII letter first; then lowercase letters, digits, `_` or `-`; maximum 64 characters |
| `bool` | Boolean value |
| `integer` | Integer value |
| `number` | Finite numeric value |
| `choice` | One of the declared choices |

`version` must be semver. `sdk_version` is a semver requirement checked against
the generator SDK version. Older schema 1 manifests without those fields default
to `0.1.0` and `^0.1`. Unknown manifest fields are rejected.

### Rendering and formatting

Every relative output path is rendered with Liquid. Files ending in `.liquid`
also render their UTF-8 contents and lose that suffix. Other file contents are
copied as bytes, except generated Rust source is formatted after rendering.

Use the builtin Rust-escaped string variables, such as `plugin_name_rust` and
`plugin_id_rust`, when inserting metadata into Rust string expressions:

```liquid
const NAME: &str = {{ plugin_name_rust }};
```

All generated `.rs` files pass through rustfmt as Rust 2024, including plain
source files. The formatter disables child-module traversal and ignores external
rustfmt configuration. It does not reformat the copied SDK snapshot.

```sh
rustup component add rustfmt
cargo rvst templates check --path ./my-template
```

Missing rustfmt returns exit 3 and rolls back the staged project. Templates
cannot contain symlinks, special files, absolute paths, parent traversal or writes
to Git internals. `templates check` validates manifest compatibility, filesystem
safety and Liquid syntax. Generation additionally validates resolved output
paths and required inputs.

## Register local or pinned Git templates

```sh
cargo rvst templates add my-local ./my-template
cargo rvst templates add my-team \
  --git https://example.com/team/template.git \
  --rev FULL_40_CHARACTER_COMMIT_SHA
cargo rvst new ./my-plugin --template my-team \
  --sdk-path /path/to/rvst --vendor-sdk
```

| Source | Update behavior |
| --- | --- |
| Local directory | Canonical path is recorded; later edits are reflected |
| Git repository | Full commit SHA is required and checked against HEAD |

Mutable branch names are not pins. Use Git credential helpers; HTTP URLs with
embedded credentials are rejected. Registrations cannot overwrite an existing
name. Registry state lives in `$RVST_HOME`, or `~/.config/rvst` by default.

## Studio and automation

`cargo rvst studio` provides searchable templates, a typed creation wizard,
live build logs and equivalent CLI commands. Work runs outside the render loop;
quitting cancels and joins task workers.

| Key | Action |
| --- | --- |
| `s` | Start the RVST Debug wizard |
| Tab | Change field |
| F2 | Toggle portable SDK mode |
| Enter | Start work |
| Esc | Cancel a task |
| Ctrl+Y | Copy the equivalent command using OSC 52 |

For scripted use:

| Option | Behavior |
| --- | --- |
| `--format human\|json\|jsonl` | Human text, one schema-versioned record, or task/log records; `--output` is an alias |
| `--non-interactive` | Rejects Studio and never prompts |
| `--dry-run` | Plans generation, registration, tasks or showcase without performing them |
| `--manifest-path`, `-p/--package`, `--target`, `--profile`, `--features` | Forward Cargo selection options where supported |
| `--plugin-format all\|vst3\|clap` | Selects native formats for packaging |

| Exit code | Meaning |
| --- | --- |
| `0` | Success |
| `1` | Operation, test or I/O failure |
| `2` | Usage or invalid input |
| `3` | Missing dependency or incompatible template |
| `4` | Destination or registration conflict |
| `130` | Cancellation |

## Verify builtin template changes

From the RVST source checkout:

```sh
python3 scripts/check-generated.py
python3 scripts/check-generated.py --vendor-sdk
```

The script generates all four templates, including both MIDI modes, then checks
formatting, builds, processor tests and Clippy with locked offline dependencies.
It also checks that each generated lockfile remains unchanged. Use
`--template gain` to narrow a development run. These checks validate generation
and builds; native validator and DAW acceptance remain separate work.
