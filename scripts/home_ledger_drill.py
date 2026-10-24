#!/usr/bin/env python3
"""Supervised lost-ledger drill, run inside the Home's maintenance environment.

Record evidence before the fault. Stop all writers and replace the lost disk
outside this program, then recover. This program never deletes or moves a disk.
"""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import subprocess
import tempfile


def stamp():
    return datetime.now(timezone.utc).isoformat()


def save(path, value, *, first=False):
    data = json.dumps(value, indent=2) + "\n"
    if first:
        with path.open("x") as output:
            os.chmod(path, 0o600)
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
    else:
        with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, delete=False) as output:
            temporary = Path(output.name)
            try:
                output.write(data)
                output.flush()
                os.fsync(output.fileno())
                temporary.replace(path)
            finally:
                temporary.unlink(missing_ok=True)
    directory = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def command(binary, directory, action, *options):
    result = subprocess.run([binary, "memory-recovery", action, "--directory", str(directory), *options],
                            capture_output=True, text=True, check=False, timeout=7200)
    if result.returncode:
        # Command output may contain private database diagnostics; do not copy it
        # into the portable drill receipt or the Deployment Administrator's terminal.
        raise RuntimeError(f"memory-recovery {action} failed; inspect the private maintenance logs")
    return json.loads(result.stdout) if result.stdout.lstrip().startswith("{") else None


def resumed_reconstruction(status, receipt):
    """Recognize only the Resume immediately following this saved reconciliation."""
    report = receipt["reconciliation"]
    before = receipt["before"]
    head = (status or {}).get("head") or {}
    return bool(status and status.get("protected") is True and status.get("serving") is True
                and status.get("home") == report.get("home") == before.get("home")
                and status.get("epoch") == report.get("epoch") == before.get("epoch")
                and status.get("restoring") is False and head.get("recovering") is False
                and report["head"].get("recovering") is True
                and head.get("sequence") == report["head"]["sequence"] + 1)


def run(args):
    directory = args.directory.resolve()
    evidence = args.evidence.resolve()
    if evidence.is_relative_to(directory):
        raise ValueError("drill evidence must live outside the disk being tested")
    if args.phase == "record":
        status = command(args.binary, directory, "status", "--json")
        if not status or not status.get("protected") or not status.get("serving") or not status.get("head"):
            raise ValueError("record requires an independently protected, serving Home")
        save(evidence, {"format": 1, "recorded_at": stamp(), "before": status}, first=True)
        return
    if not args.writers_fenced:
        raise ValueError("recovery requires --writers-fenced after stopping every floor writer")
    receipt = json.loads(evidence.read_text())
    if receipt.get("format") != 1:
        raise ValueError("invalid drill evidence")
    if receipt.get("passed"):
        raise ValueError("completed drill evidence cannot be reused for a new fault")
    if "fault_at" not in receipt:
        if not args.fault_at:
            raise ValueError("recovery requires --fault-at with the actual fault time")
        fault = datetime.fromisoformat(args.fault_at.replace("Z", "+00:00"))
        if fault.utcoffset() is None or fault > datetime.now(timezone.utc):
            raise ValueError("fault time must include a zone and cannot be in the future")
        if (directory / "ledger.cbor").exists() or (directory / "epoch.cbor").exists():
            raise ValueError("mount an empty replacement disk before starting the lost-ledger drill")
        receipt.update(fault_at=fault.isoformat(), replacement=str(directory), recovery_started_at=stamp())
        save(evidence, receipt)
    elif receipt["replacement"] != str(directory) or (args.fault_at and datetime.fromisoformat(args.fault_at.replace("Z", "+00:00")).isoformat() != receipt["fault_at"]):
        raise ValueError("an interrupted drill must keep the same replacement disk and original fault clock")
    report = directory / "drill-reconciliation.cbor"
    if "reconciliation" not in receipt:
        rebuilt = command(args.binary, directory, "reconstruct", "--writers-fenced")
        if not rebuilt or not rebuilt.get("recovering") or rebuilt["sequence"] <= receipt["before"]["head"]["sequence"]:
            raise ValueError("reconstruction did not advance the recorded history behind a recovery barrier")
        receipt["reconciliation"] = command(args.binary, directory, "reconcile-all", "--writers-fenced", "--report", str(report))
        if receipt["reconciliation"]["epoch"] != receipt["before"]["epoch"]:
            raise ValueError("reconstructed epoch differs from the recorded Home; do not resume")
        save(evidence, receipt)
    after = command(args.binary, directory, "status", "--json")
    if not resumed_reconstruction(after, receipt):
        command(args.binary, directory, "resume", "--writers-fenced", "--report", str(report))
        after = command(args.binary, directory, "status", "--json")
    complete = datetime.now(timezone.utc)
    seconds = (complete - datetime.fromisoformat(receipt["fault_at"])).total_seconds()
    passed = resumed_reconstruction(after, receipt) and seconds <= 7200
    receipt.update(completed_at=complete.isoformat(), elapsed_from_fault_seconds=seconds, after=after, passed=bool(passed))
    save(evidence, receipt)
    if not passed:
        raise RuntimeError("drill did not meet its epoch, serving, or two-hour-from-fault acceptance; retain the receipt")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=("record", "recover"))
    parser.add_argument("--binary", default="aidash")
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--fault-at")
    parser.add_argument("--writers-fenced", action="store_true")
    args = parser.parse_args()
    try:
        run(args)
    except (ValueError, RuntimeError, OSError, subprocess.TimeoutExpired) as error:
        parser.exit(1, f"{error}\n")
    print("drill evidence saved")


if __name__ == "__main__":
    main()
