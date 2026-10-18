#!/usr/bin/env python3
"""Validate the locked production dependency graph and Rust module conventions."""
import argparse
import json
import os
from pathlib import Path
import re
import sys
import tomllib

# A path segment named `enterprise`, not a word inside a package name such as
# `aidash-enterprise-server`.
ENTERPRISE_PATH = re.compile(r"(?<![\w-])enterprise(?![\w-])")
PRUNED = {".git", "enterprise", "node_modules", "target"}


def enterprise_references(root):
    """Community build inputs that name `enterprise/` (ADR 0032)."""
    inputs = [root / "Dockerfile"]
    for directory, children, files in os.walk(root):
        children[:] = [child for child in children if child not in PRUNED]
        here = Path(directory)
        inputs.extend(here / name for name in files if name in ("Cargo.toml", "Cargo.lock"))
        if here.name == ".cargo":
            inputs.extend(here / name for name in files if name.startswith("config"))
    return [
        f"Community build input names enterprise/: {path.relative_to(root)}"
        for path in inputs
        if path.is_file() and ENTERPRISE_PATH.search(path.read_text())
    ]


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
    # ADR 0006: the test-only Provider Credential write port and fake-provider
    # constructors exist only behind `test-support`. Production builds use normal
    # and build dependencies alone, so none of them may enable it, and no other
    # feature may alias it: enabling an alias would enable the cfg as well.
    for name, member in members.items():
        package = packages[member]
        for dependency in package["dependencies"]:
            if dependency["kind"] != "dev" and dependency["name"] in members \
                    and "test-support" in dependency["features"]:
                errors.append(f"production dependency enables test-support: {name} -> {dependency['name']}")
        for feature, enabled in package["features"].items():
            if feature != "test-support" and any(
                    value == "test-support" or value.endswith("/test-support") for value in enabled):
                errors.append(f"feature {name}/{feature} must not enable test-support")
    enterprise = root / "enterprise"
    for package in packages.values():
        if package["source"] is None and Path(package["manifest_path"]).is_relative_to(enterprise):
            errors.append(f"Community resolves an Enterprise package: {package['name']}")
    errors.extend(enterprise_references(root))
    errors.extend(module_violations(root, (root / "crates", root / "server")))
    return errors


def module_violations(root, directories):
    return [
        f"use module.rs with a sibling directory: {path.relative_to(root)}"
        for directory in directories
        for path in directory.rglob("mod.rs")
        if "target" not in path.parts
    ]


def enterprise_violations(metadata, root):
    """The Enterprise workspace composes Community crates and nothing else local."""
    packages = {package["id"]: package for package in metadata["packages"]}
    members = {packages[member]["name"]: member for member in metadata["workspace_members"]}
    errors = []
    expected = {"aidash-enterprise-server", "aidash-gcp", "aidash-broker"}
    if set(members) != expected:
        errors.append(f"Enterprise workspace packages must be {sorted(expected)}, found {sorted(members)}")
    for name, member in members.items():
        if packages[member]["edition"] != "2024":
            errors.append(f"{name} must use Rust 2024")
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    workspace = manifest["workspace"]
    if workspace.get("resolver") != "3":
        errors.append("the Enterprise workspace must use resolver 3")
    if workspace.get("members") != ["crates/*", "server"]:
        errors.append("the Enterprise workspace must include crates/* and server")
    if "package" in manifest:
        errors.append("the Enterprise root manifest must be a virtual workspace")
    community = root.parent
    for package in packages.values():
        path = Path(package["manifest_path"]).parent
        if package["source"] is not None or path.is_relative_to(root):
            continue
        if path != community / "server" and path.parent != community / "crates":
            errors.append(f"Enterprise may depend locally only on Community crates: {package['name']}")
    errors.extend(module_violations(root, (root / "crates", root / "server")))
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", choices=("community", "enterprise"), default="community")
    args = parser.parse_args()
    metadata = json.load(sys.stdin)
    root = Path(metadata["workspace_root"])
    if args.workspace == "enterprise":
        errors = enterprise_violations(metadata, root)
        message = "The Enterprise workspace composes only Community crates and follows the Rust 2024 conventions."
    else:
        errors = violations(metadata, root)
        message = "Rust 2024 workspace, production dependency boundaries, test-only features, module names, single migration ownership and the absence of Enterprise references are valid."
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(message)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
