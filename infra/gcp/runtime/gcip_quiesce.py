"""Fence old application authority before shared GCIP configuration changes."""

import fcntl
import json


def quiesce(host):
    host.RUN.mkdir(parents=True, exist_ok=True, mode=0o755)
    # Match host.main's lock so an in-flight install cannot restart old authority
    # after the fence is confirmed. This also works with retained host bundles.
    with (host.RUN / "lifecycle.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        host.private(host.RUN / "draining", "GCIP policy refresh\n", 0o644)
        (host.RUN / "serving").unlink(missing_ok=True)
        # Paused or absent applications cannot answer gate's HTTP request. The
        # proxy marker blocks new admission; pausing freezes existing requests.
        # A successful query distinguishes absence from a daemon/control failure.
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
        # Thaw an idle-sealed runner only after application authorization is
        # frozen, so systemd can stop it without SIGKILL escalation.
        host.command(
            "systemctl", "kill", "--signal=SIGCONT", "aidash-runner",
            check=False, timeout=5,
        )
        host.command("systemctl", "stop", "aidash-runner", timeout=40)
        # No rollback reopens admission. Bootstrap installs the new policy and
        # restarts the runner before the controller can unseal.
        return {"quiesced": True}
