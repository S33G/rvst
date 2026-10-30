# Screenshot sources

[RVST home](../../README.md) · [Getting started](../getting-started.md)

These are captures of running RVST software, taken on 30 October 2026 on macOS
with Google Chrome. They are not design mockups. The showcase was the separate
RVST Debug repository at revision `40c7517`; the framework documentation was at
`a73ffab` before adding these images.

| Image | What it shows | Capture size |
| --- | --- | --- |
| [Studio editor](debug-studio.jpg) | Actual shared egui Signal page compiled to WASM, with the Studio theme | 1000 × 560 |
| [Light theme](themes-light.jpg) | Actual shared egui Themes page with scaling and accessibility controls | 1000 × 500 |
| [Native inspector](live-inspector.jpg) | Live native standalone instance, callback metrics, parameter values and filtered plugin logs | 1000 × 1060 |

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
   content out of screenshots. Preserve the actual UI and telemetry values.

Images are JPEG page captures cropped to the documented rectangles. No controls,
text or metrics have been composited or retouched. These project screenshots
are distributed under the repository's MIT OR Apache-2.0 license.
