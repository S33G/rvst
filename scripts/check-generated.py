#!/usr/bin/env python3
"""Generate fresh source-mode projects, then verify locked builds without lock edits."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--template", choices=["gain", "instrument", "midi", "debug"])
    parser.add_argument("--vendor-sdk", action="store_true")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    env = os.environ.copy()
    env.setdefault("CARGO_TARGET_DIR", str(root / "target" / "generated-check"))
    env.setdefault("RUSTUP_TOOLCHAIN", "1.98.0")

    def run(command, cwd=root):
        print("+", " ".join(str(v) for v in command), flush=True)
        subprocess.run(command, cwd=cwd, env=env, check=True)

    # Keep CLI itself on its committed dependency graph.
    run(["cargo", "build", "-p", "cargo-rvst", "--locked"])
    suffix = ".exe" if os.name == "nt" else ""
    cli = Path(env["CARGO_TARGET_DIR"]) / "debug" / ("cargo-rvst" + suffix)
    names = [args.template] if args.template else ["gain", "instrument", "midi", "debug"]
    with tempfile.TemporaryDirectory(prefix="rvst-generated-") as temporary:
        for name in names:
            variants = ["pure-midi", "pass-through"] if name == "midi" else [None]
            for variant in variants:
                project = Path(temporary) / (name + ("-" + variant if variant else ""))
                command = [str(cli), "--non-interactive", "new", str(project),
                           "--template", name, "--sdk-path", str(root), "--no-git"]
                if variant:
                    command += ["--set", "audio_mode=" + variant]
                if args.vendor_sdk:
                    command += ["--vendor-sdk"]
                run(command)
                lock = (project / "Cargo.lock").read_bytes()
                run(["cargo", "fmt", "--all", "--", "--check"], project)
                run(["cargo", "build", "--workspace", "--locked", "--offline"], project)
                run(["cargo", "test", "-p", "plugin-core", "--locked", "--offline"], project)
                run(["cargo", "clippy", "--workspace", "--all-targets", "--locked", "--offline", "--", "-D", "warnings"], project)
                if (project / "Cargo.lock").read_bytes() != lock:
                    raise RuntimeError("Build mutated the generated lockfile")
                print("Verified", project.name, "without changing Cargo.lock", flush=True)


if __name__ == "__main__":
    main()
