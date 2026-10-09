"""Fence old application authority before shared GCIP configuration changes."""

import json


def quiesce(host):
    host.gate()
    # A failed bootstrap may already have removed the application. A successful
    # Docker query distinguishes that safe state from a daemon/control failure.
    container = host.command(
        "docker", "ps", "--all", "--quiet", "--filter", "name=^/aidash-app$",
        timeout=5,
    ).strip()
    if container:
        state = json.loads(host.command(
            "docker", "inspect", "--format={{json .State}}", "aidash-app",
            timeout=5,
        ))
        if state["Running"] and not state["Paused"]:
            host.command("docker", "pause", "aidash-app", timeout=15)
    # Thaw a previously idle-sealed runner only after application authorization
    # is frozen, so systemd can stop it without waiting for SIGKILL escalation.
    host.command(
        "systemctl", "kill", "--signal=SIGCONT", "aidash-runner",
        check=False, timeout=5,
    )
    host.command("systemctl", "stop", "aidash-runner", timeout=40)
    # No rollback reopens admission on failure. Bootstrap installs the new
    # policy and restarts the runner before the controller can unseal.
    return {"quiesced": True}
