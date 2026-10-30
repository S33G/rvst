# Undo and redo

[Documentation](README.md) / Undo and redo

RVST's editor history belongs to each plugin instance. Native and browser
runners wrap the shared editor host automatically, including custom editors.
Plugin DSP does not implement an undo stack.

## Behavior

- A knob drag is one step. Keyboard adjustments and discrete control edits are
  individual steps. Changes that return to their starting value add no step.
- Resetting all parameters is one grouped step. Successful editor preset recalls
  are reversible, including opaque state exposed through `EditorHost`.
- Default and showcase editors have Undo/Redo buttons. Custom editors receive
  the same keyboard handling and can call `rvst_ui::history_controls(ui, host)`
  to place buttons in their own layout.
- Cmd/Ctrl+Z undoes; Shift+Cmd/Ctrl+Z redoes. Ctrl+Y also redoes. Focused text
  editors keep their own text undo rather than changing plugin parameters.
  Shortcuts pressed during a drag run after release, preserving one action.
- A new effective edit clears redo. History survives closing and reopening a
  native editor. Native project saves include both undo and redo history.
- Playback automation does not create history entries. Parameter undo changes
  only the parameters belonging to that edit, through the ordinary host gesture
  path. A DAW may subsequently apply automation again.
- Loading a native project restores its saved history. Older project states
  still load, starting with empty history. A failed load leaves history intact.
  Plugin history is separate from the DAW's global undo stack.

## Custom controls

Existing controls that use `begin_edit`, `set_parameter`, and `end_edit` gain
history through the runner. To combine several edits into one action:

```rust
fn reset_parameters(host: &mut dyn rvst_core::EditorHost) {
    host.begin_edit_group("Reset parameters");
    for parameter in host.descriptor().parameters {
        host.begin_edit(parameter.id);
        host.set_parameter(parameter.id, parameter.default);
        host.end_edit(parameter.id);
    }
    host.end_edit_group();
}
```

Keep groups balanced. A control that bypasses `EditorHost`, such as an internal
MIDI command or arbitrary plugin-owned UI state, is not automatically reversible.
UI appearance preferences are outside parameter/preset history.

External runners can wrap their raw host in `rvst_core::HistoryHost` and render
through `rvst_ui::editor_frame`. Keep the wrapper alive across frames. The
optional `EditorHost::edit_history` handle lets a runner retain history across
editor recreation. Native RVST adapters already provide this handle.

## Saved projects

VST3 and CLAP save a versioned envelope containing the plugin's own state and
both history stacks. The envelope identifies the plugin and checks corruption
before restoring state. Editor presets remain raw snapshots, so history never
recursively embeds itself. CLAP's optional legacy length prefix stays outermost.

A paused drag is captured as one step without ending the live gesture. Saves
check that parameter values and history describe the same revision. A save that
cannot obtain a stable revision after bounded retries returns an error to the host.

The updated adapters accept older raw project states. Older plugin binaries do
not understand the new envelope; reopening a newly saved project requires a
plugin built with this SDK version or newer.

The browser preview has no DAW project store. Its history lasts for the current
preview session.

## Memory and real-time boundary

History is bounded to 128 actions and 8 MiB of retained change data. Old actions
are evicted as needed. If a valid preset cannot be snapshotted within the history
budget, loading still succeeds and clears the previous history rather than
allowing undo to cross an unrecorded state change.

Editing history happens on the editor thread; project serialization and
restoration happen through the format's control-thread state callbacks.
Audio callbacks neither record history nor acquire its lock. Undo/redo preserves
the existing host notification path and does not call DSP directly from the UI.

Existing plugins need to update their SDK dependency or vendored snapshot and
rebuild to gain this support. Previously built binaries are unchanged.
