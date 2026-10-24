#!/usr/bin/env python3
"""Opt-in acceptance observer; never persist argv, input, stderr or full Pods.

The setup-generated private executable binds these two leading arguments. The
real kubectl inherits stdin and stderr, receives its original arguments, and
its original stdout is released only after allowlisted evidence is durable.
"""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import uuid


def subset(value, keys):
    return {key: value[key] for key in keys if key in value}


def pod(value):
    if value.get("kind") != "Pod":
        return None
    metadata = value.get("metadata", {})
    spec, status = value.get("spec", {}), value.get("status", {})
    result = {"metadata": subset(metadata, ("uid", "name", "resourceVersion")),
              "spec": {}, "status": subset(status, ("startTime", "phase"))}
    for group in ("containers", "initContainers"):
        result["spec"][group] = [subset(item, ("name",)) for item in spec.get(group, [])]
    for group in ("containerStatuses", "initContainerStatuses"):
        result["status"][group] = []
        for item in status.get(group, []):
            saved = subset(item, ("name", "restartCount"))
            saved["state"] = {}
            for state, fields in (("running", ("startedAt",)),
                                  ("terminated", ("startedAt", "finishedAt", "exitCode", "signal", "reason")),
                                  ("waiting", ("reason",))):
                if state in item.get("state", {}):
                    saved["state"][state] = subset(item["state"][state], fields)
            result["status"][group].append(saved)
    return result


def persist(directory, value):
    name = str(uuid.uuid4())
    path = directory / (name + ".pending")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as output:
        json.dump(value, output, sort_keys=True, separators=(",", ":"))
        output.flush()
        os.fsync(output.fileno())
    # Independent readers see only complete snapshots. The controller cannot
    # observe the original provider response until the directory is durable.
    path.rename(directory / (name + ".json"))
    fd = os.open(directory, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def main():
    executable, destination, *arguments = sys.argv[1:]
    result = subprocess.run([executable, *arguments], stdout=subprocess.PIPE, check=False)
    try:
        value = json.loads(result.stdout)
    except (ValueError, UnicodeDecodeError):
        value = None
    try:
        if isinstance(value, dict):
            values = value.get("items", []) if value.get("kind") in ("List", "PodList") else [value]
            for item in values:
                captured = pod(item)
                if captured is not None:
                    persist(Path(destination), captured)
    except Exception:
        # No provider output is released when its independent capture failed.
        # Keep a secret-free failure marker when the storage still permits it.
        try:
            persist(Path(destination), {"observer_failed": True})
        finally:
            raise SystemExit("allocation provider evidence could not be persisted") from None
    sys.stdout.buffer.write(result.stdout)
    sys.stdout.buffer.flush()
    if result.returncode < 0:
        signal.signal(-result.returncode, signal.SIG_DFL)
        os.kill(os.getpid(), -result.returncode)
    raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()
