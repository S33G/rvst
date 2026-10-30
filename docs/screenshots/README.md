# Screenshot sources

[RVST home](../../README.md) · [Getting started](../getting-started.md)

These are captures of running RVST software, taken on 30 October 2026 on macOS.
The editor and inspector images were captured with Google Chrome. They are not design mockups. The showcase was the separate
RVST Debug repository at revision `40c7517`; the framework documentation was at
`a73ffab` before adding these images.

| Image | What it shows | Capture size |
| --- | --- | --- |
| [Studio editor](debug-studio.jpg) | Actual shared egui Signal page compiled to WASM, with the Studio theme | 1000 × 560 |
| [Light theme](themes-light.jpg) | Actual shared egui Themes page with scaling and accessibility controls | 1000 × 500 |
| [Native inspector](live-inspector.jpg) | Live native standalone instance, callback metrics, parameter values and filtered plugin logs | 1000 × 1060 |
| [Studio TUI](studio-tui.jpg) | Actual terminal output with template browser, native telemetry, logs and equivalent CLI | 1140 × 676 |
| [Command palette](studio-palette.jpg) | The same running TUI after pressing `:` | 1140 × 676 |

The browser editor uses a deterministic simulated host; it does not process
audio. The inspector receives real telemetry from the native standalone runner.
Its log view is paused on `Gain changed` events, with Console echo enabled.
Numbers are an illustrative session snapshot, not benchmark results or evidence
of DAW qualification. The inspector capture excludes browser chrome and the
private connection URL. No session token is stored in this directory.

## Refresh the captures

1. Follow [Getting started](../getting-started.md) to generate RVST Debug.
2. In its repository, run `cargo xtask web --serve --open`.
3. Choose **Themes → Studio**, then **Signal**, for the editor capture.
4. Choose **Themes → Light** for the theme capture.
5. Run `cargo run -p plugin-standalone` and open its local inspector using the
   URL printed by the runner. Change Gain in the native editor.
6. Filter inspector logs to `Gain changed`, enable **Echo new logs to DevTools**,
   and pause the view. Include the header, timing cards, parameters and logs.
7. Capture page content only. Keep the connection URL and any unrelated desktop
   content out of screenshots. Exclude personal usernames, shell prompts and
   absolute home-directory paths; use repository-relative paths in examples.
   Check image metadata before publishing. Preserve the actual UI and telemetry values.

Images are JPEG page captures cropped to the documented rectangles. No controls,
text or metrics have been composited or retouched. These project screenshots
are distributed under the repository's MIT OR Apache-2.0 license.

## Terminal workbench captures

The Studio and palette images show the unmodified ANSI output of the actual
`cargo-rvst --project . studio` process, captured in a 130-column × 36-row PTY
from the RVST Debug repository. The CLI was built from the framework before the
README-only changes at `670a70c`; native telemetry came from RVST Debug `40c7517`.
The terminal output was replayed through the bundled xterm.js renderer and
screenshotted in the Codex in-app browser. These are terminal renders, not
screenshots of a native terminal application or hand-built HTML reproductions.

To reproduce the content:

1. Start `cargo run -p plugin-standalone` in RVST Debug.
2. In a second terminal, select that project and run `cargo rvst studio`.
3. Use 130 columns and 36 rows, `TERM=xterm-256color`, and a color-enabled shell
   without `NO_COLOR`. Wait for native inspector discovery and telemetry.
4. Capture the main view, then press `:` and capture the command palette.
5. For the same presentation, use Menlo 14 px, line height 1.15, a dark terminal
   background, and 20 px outside padding. The image is cropped to 1140 × 676.
6. Quit Studio with Ctrl-C and close the temporary standalone instance.

The displayed instance IDs are transient, non-secret identifiers. The captures
contain no inspector capability token. Metrics and logs are real session values;
they are not performance qualification. No generated repository, successful
build or test run is implied by the idle dashboard.
