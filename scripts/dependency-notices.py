#!/usr/bin/env python3
"""Inventory the locked Cargo graph and preserve packaged upstream notices."""
import json
import pathlib
import subprocess
root = pathlib.Path(__file__).resolve().parents[1]
data = json.loads(subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--locked"], cwd=root))
packages = []
notices = []
missing = []
for package in sorted(data["packages"], key=lambda p: (p["name"], p["version"])):
    record = {key: package.get(key) for key in ("name", "version", "license", "license_file", "repository", "source")}
    directory = pathlib.Path(package["manifest_path"]).parent
    files = set()
    if package.get("source"):
        for folder in (directory, directory / "fonts"):
            if folder.is_dir():
                for path in folder.iterdir():
                    if path.is_file() and path.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "OFL.", "UFL.")):
                        files.add(path)
        if package.get("license_file"):
            path = directory / package["license_file"]
            if path.is_file() and path.resolve().is_relative_to(directory.resolve()):
                files.add(path)
        if not files:
            missing.append(f'{package["name"]} {package["version"]}')
    record["included_notices"] = []
    for path in sorted(files):
        relative = str(path.relative_to(directory))
        try:
            content = path.read_text()
        except UnicodeDecodeError:
            continue
        record["included_notices"].append(relative)
        notices.append(f'--- {package["name"]} {package["version"]} / {relative} ---\n{content.strip()}\n')
    packages.append(record)
result = {"schema_version": 1, "scope": "complete lockfile graph; target and feature selection narrow distributed dependencies", "packages": packages, "packages_without_packaged_notice_files": missing}
(root / "docs/dependency-inventory.json").write_text(json.dumps(result, indent=2) + "\n")
(root / "THIRD_PARTY_LICENSES.txt").write_text("RVST locked dependency notices\n\n" + "\n".join(notices))
unknown = [p for p in packages if not p["license"] and not p["license_file"]]
print(f"Inventoried {len(packages)} packages; {len(unknown)} lack licence metadata; {len(missing)} lack packaged notice files.")
if unknown:
    print(json.dumps(unknown, indent=2))
    raise SystemExit(1)
