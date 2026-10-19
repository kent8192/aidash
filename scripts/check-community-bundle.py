#!/usr/bin/env python3
"""Reject a Community web bundle that holds Enterprise code (#253).

Build the dashboard with `vite build --sourcemap`, then pass the output
directory. The bundle is rejected when

- a source map lists a source inside the repository's `enterprise/`, or
- any file contains the name of a GCIP client library or endpoint.

The second rule also covers a bundle that was built without source maps. A
directory without source maps is refused, because the first rule would then
check nothing.
"""
import json
from pathlib import Path
import re
import sys

ENTERPRISE = Path(__file__).resolve().parents[1] / "enterprise"

# Community has no GCIP code, so these names mean Enterprise code got in.
MARKERS = re.compile(rb"firebase|gcip|identitytoolkit", re.IGNORECASE)


def violations(bundle: Path, enterprise: Path = ENTERPRISE) -> list[str]:
    if not bundle.is_dir():
        return [f"{bundle}: not a directory"]
    files = sorted(path for path in bundle.rglob("*") if path.is_file())
    maps = [path for path in files if path.suffix == ".map"]
    problems = []
    if not maps:
        problems.append(f"{bundle}: no source maps; build with `vite build --sourcemap`")
    for path in maps:
        try:
            sources = json.loads(path.read_text()).get("sources", [])
        except (OSError, ValueError) as error:
            problems.append(f"{path}: unreadable source map ({error})")
            continue
        for source in sources:
            # Sources are relative to the map, which sits inside the bundle.
            resolved = (path.parent / source.replace("\\", "/")).resolve()
            if resolved.is_relative_to(enterprise.resolve()):
                problems.append(f"{path}: source {source} is under enterprise/")
    for path in files:
        found = MARKERS.search(path.read_bytes())
        if found:
            name = found.group().decode()
            problems.append(f"{path}: contains {name!r}, a GCIP name")
    return problems


def main(arguments: list[str]) -> int:
    if len(arguments) != 1:
        print("usage: check-community-bundle.py DIST", file=sys.stderr)
        return 2
    problems = violations(Path(arguments[0]))
    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
