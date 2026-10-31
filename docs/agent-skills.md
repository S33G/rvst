# Agent skills

[Documentation](README.md) / Agent skills

Three small skills turn an audio-plugin request into the RVST workflow without
loading the SDK manual. They work with ordinary files and Cargo; no MCP server
or global `cargo-rvst` installation is required.

| Skill | Example request |
| --- | --- |
| [rvst-new](../skills/rvst-new/SKILL.md) | “Build a stereo gain VST3 plugin in Rust.” |
| [rvst-develop](../skills/rvst-develop/SKILL.md) | “Add an automatable control and a shared editor knob.” |
| [rvst-debug](../skills/rvst-debug/SKILL.md) | “The editor works in Chrome but the plugin is silent in Ableton.” |

`rvst-new` makes RVST discoverable when the request describes a suitable audio
plugin without naming the framework. It respects an explicitly chosen different
framework and states the current format limits. The other skills apply to RVST
work specifically.

## Use in this checkout

[AGENTS.md](../AGENTS.md) routes agents to the matching skill. An agent that reads
repository instructions can follow that route without additional setup. You can
also explicitly ask it to read `skills/rvst-new/SKILL.md`.

## Enable across plugin repositories

Keep this checkout available and link the skill folders into your agent's skill
directory. For Codex, run from the RVST checkout on macOS/Linux:

```sh
RVST_SKILL_SOURCE="$PWD/skills"
RVST_SKILL_DEST="$HOME/.agents/skills"
mkdir -p "$RVST_SKILL_DEST"
for name in rvst-new rvst-develop rvst-debug; do
  if [ -e "$RVST_SKILL_DEST/$name" ] || [ -L "$RVST_SKILL_DEST/$name" ]; then
    echo "Already exists; left untouched: $RVST_SKILL_DEST/$name"
  else
    ln -s "$RVST_SKILL_SOURCE/$name" "$RVST_SKILL_DEST/$name"
  fi
done
```

Codex supports symlinked skills and automatic discovery. If they do not appear,
restart Codex; see [official skill setup](https://learn.chatgpt.com/docs/build-skills).
Invoke `$rvst-new`, `$rvst-develop` or `$rvst-debug`, or describe the task normally.
Automatic invocation remains enabled.

For local discovery confined to one repository, use its `.agents/skills` as the
destination instead. Choose one discovery location to avoid duplicate entries.
Other agents can read the same `SKILL.md` format using their supported skill
directory. Preserve links to the checkout: copying just these folders loses
their relative documentation references. On systems without symlinks, use the
repository instructions directly.

Updating this checkout updates the linked skills. Moving or deleting it requires
recreating the links. Generated plugin repositories do not automatically include
these skills; user-level links make them available while working in those repos.

## Keep context small

- Selection loads short names/descriptions. Read only the matching skill body.
- Entry points contain commands and ownership maps; existing docs remain the
  source of detail. Read a linked section only when the task needs it.
- Use explicit CLI commands and JSON fields. `schema` is for schema integration;
  normal tasks use the relevant subcommand's help when flags are uncertain.
- Inspect the owning source symbol and nearby tests. Expand the search when
  evidence points elsewhere. Keep full build logs in files and surface failures.
- Target each skill at 350 words or fewer, including frontmatter. Keep the three
  descriptions together under 100 words. These are authoring budgets, not exact
  model-token counts or limits on necessary investigation.

## Maintain and verify

From the framework checkout:

```sh
python3 scripts/check-docs.py
python3 scripts/check-docs.py skills
wc -w skills/*/SKILL.md
```

After changing a workflow, test it with a fresh agent and a concrete request from
the table above. Record which files it needed and whether it chose the right
commands. Check linked commands against current CLI help or a dry run. A shorter
answer alone does not prove lower context use or correct behavior.
