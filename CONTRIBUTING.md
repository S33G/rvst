# Contributing to RVST

[RVST home](README.md) · [Documentation](docs/README.md)

Keep changes small, testable and clear about their scope. RVST owns framework
contracts, adapters and tooling; plugin algorithms live in independent plugin
repositories.

## Set up

Clone the repository and use the pinned Rust toolchain. Check local prerequisites
with:

```sh
cargo run -p cargo-rvst -- doctor
```

Work from a branch such as `codex/describe-the-change`. Use normal Cargo
commands; no global RVST installation is required to contribute.

## Check a change

From the framework checkout:

```sh
cargo fmt --all --check
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
python3 scripts/check-docs.py
```

For template/generation changes, also run:

```sh
python3 scripts/check-generated.py --vendor-sdk
```

That check builds fresh plugin workspaces with separate target directories.
Its build/test phase uses `--locked --offline`, so the pinned dependencies must
already be cached. Use `--template gain` to narrow an investigation, then check
all affected starters.

For shared UI/web changes:

```sh
rustup target add wasm32-unknown-unknown
cargo check -p rvst-web --target wasm32-unknown-unknown --locked
```

Also exercise the actual WASM editor with browser input. Compilation alone does
not verify rendering, keyboard behavior or native host embedding.

## Preserve the contracts

- Keep allocations, deallocation, blocking, I/O and formatted logs off audio
  callbacks. Add meaningful regression coverage for behavior changes.
- Preserve released plugin/parameter identities, state formats and host
  automation semantics. Document intentional compatibility changes.
- Keep CLI and TUI operations equivalent. Test JSON errors and cancellation
  when changing operation behavior.
- Keep examples executable and update the relevant guide with API changes.
- Distinguish source tests, builds, native validators and real DAW qualification.
  Record artifact hashes and actual test environments for release claims.

See [real-time rules](docs/realtime.md), [API contracts](docs/api-contract.md),
and the [release checklist](docs/releases.md).

## Write and check documentation

Guides live under `docs/`, with [docs/README.md](docs/README.md) as the index.
State where commands run: framework checkout or generated plugin repository.
Use relative links for repository content, fenced code blocks with a language,
and short sections organized around a task.

`check-docs.py` checks local Markdown file links, heading anchors and code-fence
balance in the root and `docs/`. It runs offline; it does not validate remote
URLs or execute example commands. Generated README links are checked after
rendering because their paths and names come from template inputs.

## Commits and licensing

Every commit must include this exact trailer:

```text
Co-Author: Charlie's Agent
```

Use the actual date for new work. The initial import's reconstructed dates are
explained in [HISTORY.md](HISTORY.md).

Contributions to RVST and its starters use `MIT OR Apache-2.0`. Update dependency
and asset notices when distribution inputs change; do not commit credentials,
private inspector URLs or machine-specific build output.
