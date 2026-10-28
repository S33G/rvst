# Declarative templates and independent projects

`cargo rvst new` creates an independent Cargo workspace and Git repository. The
new root contains `[workspace]`, so parent Cargo/Git workspaces do not adopt it.
Generation stages files in a sibling directory and atomically publishes them;
an existing destination is a conflict (exit 4), never a merge destination.

```sh
cargo rvst templates list
cargo rvst templates inspect debug
cargo rvst templates check --path ./my-template
cargo rvst new ./my-gain --template gain --vendor 'Example Audio' --id com.example.gain
cargo rvst --format json --dry-run new ./planned --template instrument
```

Builtins are `debug`, `gain`, `instrument`, and `midi`. The MIDI template accepts `--set audio_mode=pure-midi` (default) or `--set audio_mode=pass-through`, which adds unchanged stereo audio routing. Builtin contents are
embedded in the installed executable, so generation does not need the framework
source checkout. `debug` is the interactive feature tour used by `cargo rvst
showcase`. Each project separates core DSP, shared egui UI, native VST3/CLAP
wrappers, desktop runner, browser editor, and its typed `xtask`.

## SDK selection before publication

Registry dependencies are the portable default. RVST 0.1.0 is not implied to be
published by generation. Before publication, choose the SDK explicitly:

```sh
cargo rvst new ./my-gain --template gain --sdk-path /path/to/rvst
cargo rvst new ./portable-gain --template gain --sdk-path /path/to/rvst --vendor-sdk
cargo rvst showcase --path ./rvst-debug --sdk-path /path/to/rvst --install --run --open
```

`--sdk-path` records absolute development dependencies in the generated
manifests. `--vendor-sdk` instead snapshots the SDK into `vendor/rvst` and emits
relative dependencies. It includes SDK crates, templates, docs, licenses, and
third-party notices. The generated workspace excludes the vendored workspace.
Neither mode silently discovers or adopts a framework checkout. Builtin `Cargo.lock` files pin this source/vendored SDK mode, so a freshly generated project supports immediate `cargo build --locked --offline` when the pinned dependencies are cached. Publishing registry-backed RVST requires regenerating those lockfiles against the published packages; generation does not pretend the unpublished SDK is available.

`rvst-project.json` records template origin/version, resolved Git SHA when
registered from Git, typed input values, SDK compatibility requirement, SDK
source, and whether the SDK is a development override or vendored snapshot.
Keep it with the generated project. The manifest records a per-project bundle
name and stable reverse-domain ID; installed projects get distinct filenames.

## Author a template

```
my-template/
  rvst-template.toml
  README.md
  files/
    Cargo.toml.liquid
    src/lib.rs.liquid
```

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

Schema 1 supports `string`, `identifier`, `bool`, `integer`, `number`, and
`choice`. Identifiers start with a lowercase ASCII letter and contain lowercase
letters, digits, `_`, or `-`, with a maximum length of 64. Numerical values must
be finite. Unknown input names, missing required inputs, and invalid typed
values fail before publication. Template `version` is semver and `sdk_version`
is a semver requirement checked against the generator SDK version. Older schema
1 manifests without these fields default to `0.1.0` and `^0.1`.

Input defaults can be overridden using repeatable `--set NAME=VALUE`.
`--vendor` and `--id` are conveniences for the builtin `vendor` and `plugin_id`
inputs. Supplying a value twice is an error. `crate_name` defaults to the output
directory name. The builtin `plugin_id` defaults to `org.rvst.<crate_name>`.

Every relative output path is rendered with Liquid. Files ending in `.liquid`
are UTF-8 rendered contents and lose that suffix. Generated `.rs` files, including
plain Rust source files, are then formatted as Rust 2024 through rustfmt stdin.
The built-in formatter step disables child-module traversal and ignores external
rustfmt configuration; it does not process copied vendored SDK files. Other files
are copied as bytes. Install the required formatter with
`rustup component add rustfmt`; missing rustfmt returns exit 3 and rolls back the
staged project. Builtin templates use Rust-escaped string variables to keep metadata
values safe in source. Unknown manifest fields are rejected. Templates cannot
use executable hooks, symlinks, special files, absolute paths, parent traversal,
or write Git internals. `templates check` verifies manifest compatibility,
filesystem safety, and Liquid syntax. Rendering still checks every resolved
output path and required input.

## Register local or pinned Git sources

```sh
cargo rvst templates add my-local ./my-template
cargo rvst templates add my-team --git https://example.com/team/template.git --rev FULL_40_CHARACTER_COMMIT_SHA
cargo rvst new ./my-plugin --template my-team
```

Git registration requires a full commit SHA, verifies checked-out HEAD, validates
the template, and records the source and revision. Mutable branch names are not
pins. HTTP URLs containing credentials are rejected; use Git credential helpers.
Local registrations reference the canonical local path and reflect subsequent
edits. Registrations cannot overwrite an existing name. Registry state lives in
`$RVST_HOME`, or `~/.config/rvst` by default.

## Studio and machine usage

`cargo rvst studio` opens searchable templates, a typed creation wizard, live
build logs, a command palette, and the equivalent CLI command. `s` starts the
RVST Debug wizard; Tab changes fields, F2 toggles a portable SDK, Enter starts
work, Esc cancels a task, and Ctrl+Y copies its command using OSC 52. Work runs
outside the render loop. Quitting cancels and joins task workers.

`--format human|json|jsonl` applies to commands; `--output` is an alias. JSON is a
single schema-versioned result or error record. JSONL adds task-start and log
records. `--non-interactive` rejects the studio and never prompts. `--dry-run`
plans generation, registration, tasks, or showcase without performing them.
`--manifest-path`, `-p/--package`, `--target`, `--profile`, and `--features`
forward Cargo options; packaging/web forward plugin options through `xtask`.
Package format selection uses `--plugin-format all|vst3|clap`.

Exit codes: 0 success, 1 operation/test/I/O failure, 2 usage/invalid input,
3 unavailable dependency or incompatible template, 4 conflicting destination
or registration, and 130 cancellation.

The regression script `python3 scripts/check-generated.py` generates all four templates (both MIDI modes), checks formatting, builds and tests with `--locked --offline`, runs Clippy, and checks that Cargo.lock remains unchanged. Add `--vendor-sdk` to verify portable snapshots.
