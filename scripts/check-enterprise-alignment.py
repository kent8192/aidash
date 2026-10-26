#!/usr/bin/env python3
"""Keep the Enterprise workspace on the Community toolchain, lints and locked versions.

The two workspaces have separate lockfiles (ADR 0032). Every version that
enterprise/Cargo.lock holds of a package Community also locks must be one that
Cargo.lock holds, so that Community tests describe the code the Enterprise
build runs.

The [workspace.package] tables match except for the component license:
Apache-2.0 for Community and BUSL-1.1 for Enterprise (ADR 0035).
"""
from collections import defaultdict
from pathlib import Path
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
SHARED_FILES = ("rust-toolchain.toml", "rustfmt.toml", "clippy.toml")


def locked(lockfile):
    versions = defaultdict(set)
    for package in tomllib.loads(lockfile.read_text())["package"]:
        versions[package["name"], package.get("source")].add(
            (package["version"], package.get("checksum"))
        )
    return versions


def dependency(spec, root):
    """A workspace dependency with its path made absolute, so both roots compare."""
    if isinstance(spec, dict) and "path" in spec:
        return {**spec, "path": str((root / spec["path"]).resolve())}
    return spec


def misalignments(community, enterprise):
    errors = []
    for name in SHARED_FILES:
        if (community / name).read_bytes() != (enterprise / name).read_bytes():
            errors.append(f"enterprise/{name} differs from {name}")
    ours = tomllib.loads((community / "Cargo.toml").read_text())
    theirs = tomllib.loads((enterprise / "Cargo.toml").read_text())
    ours_package = dict(ours["workspace"].get("package", {}))
    theirs_package = dict(theirs["workspace"].get("package", {}))
    ours_license = ours_package.pop("license", None)
    theirs_license = theirs_package.pop("license", None)
    if ours_package != theirs_package:
        errors.append("[workspace.package] differs between the workspaces")
    # The license is the one package key that must differ, by component.
    for manifest, actual, expected in (
        ("Cargo.toml", ours_license, "Apache-2.0"),
        ("enterprise/Cargo.toml", theirs_license, "BUSL-1.1"),
    ):
        if actual != expected:
            errors.append(
                f'[workspace.package] license in {manifest} must be "{expected}"'
            )
    if ours["workspace"].get("lints") != theirs["workspace"].get("lints"):
        errors.append("[workspace.lints] differs between the workspaces")
    if ours.get("profile") != theirs.get("profile"):
        errors.append("[profile] differs between the workspaces")
    shared = ours["workspace"]["dependencies"].keys() & theirs["workspace"]["dependencies"].keys()
    for name in sorted(shared):
        if dependency(ours["workspace"]["dependencies"][name], community) != dependency(
            theirs["workspace"]["dependencies"][name], enterprise
        ):
            errors.append(f"[workspace.dependencies] {name} differs between the workspaces")
    community_lock = locked(community / "Cargo.lock")
    enterprise_lock = locked(enterprise / "Cargo.lock")
    # Community also locks its other members and dev-dependencies, so it may
    # hold more versions of a package; Enterprise may not hold one it lacks.
    for name, source in sorted(community_lock.keys() & enterprise_lock.keys(), key=str):
        expected = community_lock[name, source]
        extra = enterprise_lock[name, source] - expected
        if extra:
            versions = sorted(version for version, _ in expected)
            errors.append(
                f"{name} ({source or 'path'}) is locked at "
                f"{', '.join(sorted(version for version, _ in extra))} in enterprise/Cargo.lock "
                f"but at {', '.join(versions)} in Cargo.lock; run `cargo update --manifest-path "
                f"enterprise/Cargo.toml -p {name} --precise "
                f"{versions[0] if len(versions) == 1 else '<version>'}`"
            )
    return errors


def main():
    errors = misalignments(ROOT, ROOT / "enterprise")
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print("The Enterprise workspace shares the Community toolchain, lints, profiles and locked versions.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
