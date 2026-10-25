#!/usr/bin/env python3
"""Assemble one Reinhardt Pages bundle without Node.js or a second app project."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", type=Path, required=True)
    args = parser.parse_args()
    project = args.project.resolve()
    package = "aidash_" + project.name + "_server"
    bindings = project / "dist-wasm"
    for suffix in (".js", "_bg.wasm"):
        path = bindings / (package + suffix)
        if not path.is_file() or not path.stat().st_size:
            raise SystemExit(f"Build the project's Pages WASM first: missing {path}")
    vendor = project / "static/assets/vendor"
    if vendor.is_dir():
        provenance = json.loads((vendor / "provenance.json").read_text())
        for filename, expected in provenance["files"].items():
            if hashlib.sha256((vendor / filename).read_bytes()).hexdigest() != expected["sha256"]:
                raise SystemExit(f"Vendored browser module differs from its recorded digest: {filename}")
    destination = project / "dist"
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(project / "static", destination)
    shutil.copy2(project / "index.html", destination / "index.html")
    for suffix in (".js", "_bg.wasm"):
        shutil.copy2(bindings / (package + suffix), destination / "assets" / (package + suffix))
    snippets = bindings / "snippets"
    if snippets.is_dir():
        shutil.copytree(snippets, destination / "assets/snippets")
    print(f"Assembled {project.name} Pages in {destination}")


if __name__ == "__main__":
    main()
