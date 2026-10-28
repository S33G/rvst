# Dependency and asset notices

RVST original source and templates are offered under MIT OR Apache-2.0. See
LICENSE-MIT and LICENSE-APACHE. No product artwork has been copied from Hollow
Point. Bundled theme manifests are original RVST assets.

Dependency licence data is generated from Cargo's resolved package metadata by
`scripts/dependency-notices.py`. The inventory is an audit input, not a substitute
for distributing applicable licences/notices with a release. Audit the actual
target/feature graph and all embedded fonts/artwork before publication.

Key dependencies include egui/eframe, Ratatui, Crossterm, clap, Liquid, serde,
rtrb, CPAL, baseview, raw VST3 bindings and clap-sys. The adapters are RVST code;
using ABI bindings does not use a third-party plugin-framework backend.

The default egui font data travels under its bundled font notices; preserve the
upstream crate's font licence files when redistributing. See the generated
`docs/dependency-inventory.json` for resolved versions and licence expressions.

The repository includes `THIRD_PARTY_LICENSES.txt`, assembled from the notice
files actually shipped in the resolved Cargo packages (including font OFL/UFL
texts). The inventory records packages that contain only licence metadata and
no packaged notice file. Those entries require upstream notice verification at
release time. Where a dependency offers GPL/LGPL as an alternative to MIT or
Apache, RVST uses the permissive alternative; fonts retain their separate terms.
