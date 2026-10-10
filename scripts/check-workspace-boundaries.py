#!/usr/bin/env python3
"""Validate the locked production dependency graph and Rust module conventions."""
import json
from pathlib import Path
import sys
import tomllib


def violations(metadata, root):
    packages = {package["id"]: package for package in metadata["packages"]}
    members = {packages[member]["name"]: member for member in metadata["workspace_members"]}
    errors = []
    expected = {
        "aidash-domain",
        "aidash-application",
        "aidash-harness",
        "aidash-runtime",
        "aidash-integrations",
        "aidash-server",
        "aidash-capability",
        "aidash-broker",
    }
    if set(members) != expected:
        errors.append(f"workspace packages must be {sorted(expected)}, found {sorted(members)}")
    for name, member in members.items():
        if packages[member]["edition"] != "2024":
            errors.append(f"{name} must use Rust 2024")
    server = members.get("aidash-server")
    if metadata.get("workspace_default_members") != [server]:
        errors.append("the server must be the only default workspace member")
    workspace = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]
    if workspace.get("resolver") != "3":
        errors.append("the virtual workspace must use resolver 3")
    if workspace.get("members") != ["crates/*", "server"]:
        errors.append("the virtual workspace must include crates/* and server")
    if "package" in tomllib.loads((root / "Cargo.toml").read_text()):
        errors.append("the root manifest must be a virtual workspace")
    for legacy in (root / "src", root / "migration"):
        if legacy.exists():
            errors.append(f"obsolete monolithic source or migration engine remains: {legacy.name}/")
    graph = {
        node["id"]: [
            dependency["pkg"] for dependency in node["deps"]
            if any(kind["kind"] != "dev" for kind in dependency["dep_kinds"])
        ] for node in metadata["resolve"]["nodes"]
    }
    portable_layers = {
        "aidash-capability": set(),
        "aidash-domain": set(),
        "aidash-application": {"aidash-domain"},
        "aidash-harness": {"aidash-domain", "aidash-application"},
    }
    for layer, allowed in portable_layers.items():
        if layer not in members:
            continue
        visited = set()
        pending = [(members[layer], [layer])]
        while pending:
            package, path = pending.pop()
            if package in visited:
                continue
            visited.add(package)
            name = packages[package]["name"]
            forbidden_layer = package != members[layer] and name in members and name not in allowed
            if forbidden_layer or name == "reqwest" or name.startswith(("reinhardt", "axum", "sqlx", "sea-orm")):
                errors.append("forbidden production dependency: " + " -> ".join(path))
            pending.extend((dependency, [*path, packages[dependency]["name"]]) for dependency in graph.get(package, []))
    for directory in (root / "crates", root / "server"):
        for path in directory.rglob("mod.rs"):
            if "target" not in path.parts:
                errors.append(f"use module.rs with a sibling directory: {path.relative_to(root)}")
    return errors


def main():
    metadata = json.load(sys.stdin)
    errors = violations(metadata, Path(metadata["workspace_root"]))
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print("Rust 2024 workspace, production dependency boundaries, module names, and single migration ownership are valid.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
