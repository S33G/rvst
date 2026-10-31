# CLI, templates, and studio status

[Documentation](README.md) / CLI, templates, and studio status

For commands and keyboard shortcuts, use the [CLI and TUI reference](cli.md).

Implemented in `cargo-rvst` and shared `rvst-ops`. Installation is described in
the [setup wizard](../scripts/install.sh). It provides the direct `rvst`
executable and keeps `cargo rvst` as a Cargo subcommand alias.

## Implemented capabilities

- CLI: new, templates list/inspect/add/check, doctor, dev (native/web), test,
  validate, debug, inspect, replay/step, package, schema, studio, showcase.
- Exact contract flags: `--format human|json|jsonl` (`--output` alias),
  `--non-interactive`, `--dry-run`, `--vendor`, `--id`, Cargo manifest/package/
  target/profile/features forwarding; `--plugin-format` selects native format.
- Formal result and recording JSON schemas are embedded into `schema` output.
  JSON errors are one record; JSONL streams task start/log/result events.
- Failures: 1 operation, 2 input, 3 unavailable, 4 conflict, 130 cancellation.
- Four builtin templates embedded in the executable: gain, instrument, MIDI,
  and RVST Debug. Versioned manifest/SDK compatibility, typed inputs, defaults,
  pinned Git/local registration, provenance, no executable hooks or symlinks,
  traversal/portable-path protection, rollback and atomic destination publication.
  Generated Rust passes through the built-in rustfmt step; rustfmt is required.
- Explicit SDK path override or portable vendored snapshot. SDK snapshot
  includes source, templates, docs, license files, and third-party notices.
- New projects own Cargo and Git roots and split DSP, UI, native, desktop,
  browser, and typed xtask crates. Bundle names and plugin IDs are per-project.
- Studio: searchable template list, typed creation wizard with visible SDK
  choice, RVST Debug starter, command palette, task/log panels, equivalent CLI
  and OSC 52 copy, background workers, cancellation with joined workers, and live
  inspector telemetry with a 60-sample history. Network polling stays off rendering.
- Inspector discovery/open respects explicit request and instance selection;
  machine inspection returns authenticated live snapshots without capability URLs.
  Showcase reuses only owned Debug projects, builds/optionally installs,
  avoids duplicate live instances, and reaps a foreground runner on Ctrl-C.

## Coverage and boundaries

Automated coverage: CLI and operations tests cover all parser
forms, real local pinned Git registration, all embedded template rendering,
unknown/invalid inputs, traversal and symlink rejection, atomic failure cleanup,
existing destinations, paths with spaces, dry-run no project/registry writes,
single JSON errors, formal schema field shapes, subprocess/grandchild
cancellation, parseable JSONL cancellation, TUI rendering and actual wizard
creation through shared operations. Tests use real child processes and local
Git repositories. No network template fixture is required.

Generated-project, clone and installation checks are separate from parser and
operation tests. Current Debug artifact and browser checks are recorded in the
[integration verification report](validation-2026-10-30.md). CLI tests do not
imply Windows/Linux GUI acceptance or DAW validation. See [implementation
status](implementation.md) and [native adapter status](native-status.md).
