#!/usr/bin/env python3
"""Partition the actual Cargo test targets without omitting new targets."""

import argparse
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
PARTITIONS = (
    "foundation", "server-unit", "identity", "execution", "persistence",
    "collaboration", "federation", "knowledge-marketplace",
)
PERSISTENCE = {
    "migrations", "postgres", "reinhardt_persistence", "record_constraints",
    "composite_keys", "startup", "providers", "provider_timeout_persistence",
    "orchestration", "observation",
}


def inventory(metadata):
    partitions = {name: [] for name in PARTITIONS}
    members = set(metadata["workspace_members"])
    for package in metadata["packages"]:
        if package["id"] not in members:
            continue
        for target in package["targets"]:
            if not target["test"]:
                continue
            if package["name"] != "aidash-server":
                partition = "foundation"
            elif target["kind"] in (["lib"], ["bin"]):
                partition = "server-unit"
            elif target["kind"] == ["test"]:
                source = Path(target["src_path"]).relative_to(ROOT / "server/src/apps")
                app = source.parts[0]
                partition = {
                    "identity": "identity", "registry": "collaboration",
                    "workspaces": "collaboration", "federation": "federation",
                    "knowledge": "knowledge-marketplace",
                    "marketplace": "knowledge-marketplace",
                    "operations": "execution", "execution": "execution",
                }[app]
                if app == "execution" and target["name"] in PERSISTENCE:
                    partition = "persistence"
            else:
                raise ValueError(f"Unpartitioned target kind: {target['name']} {target['kind']}")
            partitions[partition].append({
                "package": package["name"], "target": target["name"],
                "kind": target["kind"][0],
            })
    identities = [(item["package"], item["kind"], item["target"])
                  for items in partitions.values() for item in items]
    if len(identities) != len(set(identities)) or any(not items for items in partitions.values()):
        raise ValueError("Test partitions must be nonempty and contain every target once")
    return partitions


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--partition", choices=PARTITIONS)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], cwd=ROOT,
    ))
    partitions = inventory(metadata)
    if args.check or args.partition is None:
        print(json.dumps(partitions, indent=2))
    elif args.partition == "foundation":
        print("\n".join(["--workspace", "--exclude", "aidash-server", "--all-targets"]))
    elif args.partition == "server-unit":
        print("\n".join(["-p", "aidash-server", "--lib", "--bins"]))
    else:
        arguments = ["-p", "aidash-server"]
        for item in sorted(partitions[args.partition], key=lambda item: item["target"]):
            arguments += ["--test", item["target"]]
        print("\n".join(arguments))


if __name__ == "__main__":
    main()
