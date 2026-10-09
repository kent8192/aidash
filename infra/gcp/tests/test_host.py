"""Activity/admission races using private temporary host state."""

from contextlib import ExitStack
import bz2
import hashlib
import io
import json
from pathlib import Path
import sys
import tarfile
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "runtime"))
import host


class HostTests(unittest.TestCase):
    def setUp(self):
        self.context = ExitStack()
        self.addCleanup(self.context.close)
        self.directory = Path(self.context.enter_context(TemporaryDirectory()))
        self.context.enter_context(patch.object(host, "ROOT", self.directory))
        self.context.enter_context(patch.object(host, "RUN", self.directory / "run"))
        self.context.enter_context(patch.object(host.time, "time", return_value=10000))
        self.context.enter_context(
            patch.object(
                host, "request", return_value=b'{"inflight":0,"last_active":0}'
            )
        )
        self.snapshot = dict(
            protocol="aidash-infra-activity/1",
            busy=False,
            counts={"runs": 0, "database_work": 0},
            completed_transfers=[],
        )

    def test_memory_recovery_directory_follows_the_retained_home_identity(self):
        secret = {"payload": {"data": host.base64.b64encode(json.dumps({"AIDASH_OIDC_CLIENT_ID": "fixture-client", "AIDASH_OIDC_CLIENT_SECRET": "fixture-secret"}).encode()).decode()}}
        with patch.object(host, "request", return_value=json.dumps(secret).encode()), patch.object(host, "cloud_token", return_value="fixture"):
            host.configuration({"project": "fixture", "secret": "home-a", "hostname": "example.invalid"})
        values = dict(line.split("=", 1) for line in (host.RUN / "app.env").read_text().splitlines())
        self.assertEqual(values["AIDASH_MEMORY_RECOVERY_DIR"], str(host.ROOT / "memory-recovery"))
        self.assertEqual(values["AIDASH_NODE_ID"], "aidash://home-a")

    def test_runtime_key_allowlist_accepts_fingerprint_and_rejects_master_key(self):
        base = {"AIDASH_OIDC_CLIENT_ID": "fixture-client", "AIDASH_OIDC_CLIENT_SECRET": "fixture-secret"}
        for key, allowed in [("AIDASH_PROVIDER_FINGERPRINT_KEY", True), ("AIDASH_SECRET_TOOL", True), ("AIDASH_PROVIDER_STORE_MASTER_KEY", False), ("UNRELATED_SENTINEL", False)]:
            external = dict(base, **{key: "fixture-value"})
            secret = {"payload": {"data": host.base64.b64encode(json.dumps(external).encode()).decode()}}
            with patch.object(host, "request", return_value=json.dumps(secret).encode()), patch.object(host, "cloud_token", return_value="fixture"):
                if allowed:
                    host.configuration({"project": "fixture", "secret": "home-a", "hostname": "example.invalid"})
                    self.assertIn(key + "=fixture-value", (host.RUN / "app.env").read_text())
                else:
                    with self.assertRaises(ValueError):
                        host.configuration({"project": "fixture", "secret": "home-a", "hostname": "example.invalid"})

    def record_previous(self, observed_at, busy):
        (self.directory / "last-active").write_text("100")
        (self.directory / "activity.json").write_text(
            json.dumps(dict(observed_at=observed_at, busy=busy))
        )

    def test_work_completion_starts_a_full_new_idle_hour(self):
        self.record_previous(9990, True)
        with patch.object(host, "snapshot", return_value=self.snapshot):
            result = host.observe()
        self.assertFalse(result["busy"])
        self.assertEqual(result["last_active"], 10000)

    def test_observation_gap_is_not_silently_counted_as_idle(self):
        self.record_previous(9800, False)
        with patch.object(host, "snapshot", return_value=self.snapshot):
            self.assertEqual(host.observe()["last_active"], 10000)

    def test_transfer_completed_between_samples_renews_idle_once(self):
        self.record_previous(9990, False)
        self.snapshot["completed_transfers"] = [
            "delivered-outbound",
            "committed-inbound",
        ]
        with patch.object(host, "snapshot", side_effect=lambda: dict(self.snapshot)):
            observed = host.observe()
            self.assertEqual(observed["last_active"], 10000)
            self.assertEqual(observed["last_work_completed"], 10000)
            with patch.object(host.time, "time", return_value=10030):
                self.assertEqual(host.observe()["last_active"], 10000)
            self.snapshot["completed_transfers"] = []
            with patch.object(host.time, "time", return_value=10060):
                self.assertEqual(host.observe()["last_active"], 10000)

    def test_idle_database_maintenance_does_not_keep_a_host_alive(self):
        self.record_previous(9990, False)
        self.snapshot["counts"]["database_work"] = 1
        self.snapshot["busy"] = True
        with patch.object(host, "snapshot", return_value=self.snapshot):
            result = host.observe()
        self.assertFalse(result["busy"])
        self.assertEqual(result["last_active"], 100)

    def test_unconfirmed_runner_writer_is_busy_even_for_terminal_operation(self):
        (self.directory / "release.json").write_text(
            json.dumps({"images": {"observer": "fixture"}})
        )
        journal = self.directory / "journal"
        journal.mkdir()
        record = journal / "operation.json"
        record.write_text(
            json.dumps({"status": "uncertain", "termination_confirmed": False})
        )
        with patch.object(
            host, "command", return_value=json.dumps(self.snapshot).encode()
        ):
            self.assertTrue(host.snapshot()["busy"])
            record.write_text(
                json.dumps({"status": "completed", "writer_frozen": True})
            )
            self.assertFalse(host.snapshot()["busy"])

    def test_pause_failure_restores_admission_and_never_claims_sealed(self):
        calls = []

        def command(*args, **kwargs):
            calls.append(args)
            if args[:2] == ("docker", "inspect"):
                return b'{"Running": true, "Paused": false}'
            if args[:2] == ("docker", "pause"):
                raise RuntimeError("pause failed")
            return b""

        with patch.object(host, "command", command):
            with self.assertRaises(RuntimeError):
                host.seal()
        self.assertIn(("docker", "unpause", "aidash-app"), calls)
        self.assertFalse((self.directory / "run/sealed").exists())
        self.assertFalse((self.directory / "run/draining").exists())

    def test_seal_checks_uncommitted_database_work_after_pausing_admission(self):
        self.snapshot["busy"] = True
        self.snapshot["counts"]["database_work"] = 1
        with (
            patch.object(
                host, "command", return_value=b'{"Running": true, "Paused": false}'
            ),
            patch.object(host, "snapshot", return_value=self.snapshot),
        ):
            self.assertFalse(host.seal()["sealed"])
        self.assertFalse((self.directory / "run/sealed").exists())

    def test_health_reclaims_only_superseded_images_and_keeps_live_pods(self):
        prefix = "us-central1-docker.pkg.dev/fixture/aidash/"
        old_app, old_sandbox, current_app, current_sandbox, observer = (
            prefix + kind + "@sha256:" + digest * 64
            for kind, digest in (
                ("app", "a"),
                ("sandbox", "b"),
                ("app", "c"),
                ("sandbox", "d"),
                ("observer", "e"),
            )
        )
        unrelated = "example/other@sha256:" + "f" * 64
        docker = {
            old_app,
            old_sandbox,
            current_app,
            current_sandbox,
            observer,
            unrelated,
        }
        containerd = {old_sandbox, current_sandbox, unrelated}
        live = [old_sandbox]
        for name, value in {
            "release": {
                "source_sha": "a" * 40,
                "images": {
                    "app": current_app,
                    "sandbox": current_sandbox,
                    "observer": observer,
                },
            },
            "profile": {"runner": {"endpoint": "http://fixture"}},
            "identity": {"runner": "fixture-token"},
        }.items():
            (self.directory / (name + ".json")).write_text(json.dumps(value))

        def command(*args, **kwargs):
            if args[:3] == ("docker", "image", "ls"):
                return "\n".join(docker).encode()
            if args[:3] == ("docker", "image", "rm"):
                self.assertNotIn("--force", args)
                docker.remove(args[-1])
                return b""
            if args[:5] == ("ctr", "-n", "k8s.io", "images", "list"):
                return "\n".join(containerd).encode()
            if args[:5] == ("ctr", "-n", "k8s.io", "images", "rm"):
                containerd.remove(args[-1])
                return b""
            if args[0] == "kubectl":
                return json.dumps(
                    {
                        "items": [
                            {"spec": {"containers": [{"image": image}]}}
                            for image in live
                        ]
                    }
                ).encode()
            self.fail(f"unexpected host command: {args}")

        with (
            patch.object(host, "command", command),
            patch.object(
                host,
                "request",
                return_value=b'{"verified":true,"python_verified":true}',
            ),
            patch.object(host, "observe", return_value=self.snapshot),
        ):
            self.assertTrue(host.health()["ready"])
            self.assertEqual(
                docker, {current_app, current_sandbox, observer, unrelated}
            )
            self.assertEqual(containerd, {old_sandbox, current_sandbox, unrelated})
            live.clear()
            self.assertTrue(host.health()["ready"])
            self.assertEqual(containerd, {current_sandbox, unrelated})


class RuntimeTests(unittest.TestCase):
    def test_retained_runtime_upgrades_repairs_and_restarts_only_when_changed(self):
        with TemporaryDirectory() as temporary, ExitStack() as context:
            root = Path(temporary)

            def path(value, *parts):
                value = Path(value, *parts)
                return (
                    root / str(value).lstrip("/")
                    if value.is_absolute() and not value.is_relative_to(root)
                    else value
                )

            original_symlink = Path.symlink_to
            context.enter_context(
                patch.object(
                    Path,
                    "symlink_to",
                    lambda link, target, **kwargs: original_symlink(
                        link, path(target), **kwargs
                    ),
                )
            )
            context.enter_context(patch.object(host, "Path", path))
            context.enter_context(patch.object(host, "ROOT", root / "data"))
            context.enter_context(patch.object(host, "RUN", root / "run"))
            calls = context.enter_context(
                patch.object(host, "command", return_value=b"")
            )
            files = {
                "runsc": b"updated runsc",
                "containerd-shim-runsc-v1": b"updated shim",
                "gvisor-bin/gvisor_sentry": b"updated sidecar",
            }
            archive = io.BytesIO()
            with tarfile.open(fileobj=archive, mode="w") as tar:
                for name, data in files.items():
                    member = tarfile.TarInfo(name)
                    member.size = len(data)
                    tar.addfile(member, io.BytesIO(data))
            compressed = bz2.compress(archive.getvalue())
            k3s = b"updated k3s"
            context.enter_context(
                patch.object(host, "K3S_SHA", hashlib.sha256(k3s).hexdigest())
            )
            context.enter_context(
                patch.object(host, "GVISOR_SHA", hashlib.sha256(compressed).hexdigest())
            )
            download = context.enter_context(
                patch.object(
                    host,
                    "request",
                    side_effect=lambda url: k3s if url.endswith("/k3s") else compressed,
                )
            )
            for name in ("k3s", *files):
                destination = path("/usr/local/bin") / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(b"old runtime")
            host.configure_runtime()
            self.assertEqual(path("/usr/local/bin/k3s").read_bytes(), k3s)
            for name, data in files.items():
                self.assertEqual((path("/usr/local/bin") / name).read_bytes(), data)
            calls.assert_any_call("systemctl", "restart", "k3s", timeout=240)
            calls.reset_mock()
            download.reset_mock()
            host.configure_runtime()
            download.assert_not_called()
            self.assertFalse(
                any(
                    call.args[:3] == ("systemctl", "restart", "k3s")
                    for call in calls.call_args_list
                )
            )
            # Missing sidecars and drifted containerd configuration must also
            # be repaired on a retained VM, even with an intact runsc binary.
            path("/usr/local/bin/gvisor-bin/gvisor_sentry").unlink()
            path("/etc/containerd/runsc.toml").write_text("stale configuration")
            host.configure_runtime()
            self.assertEqual(
                path("/usr/local/bin/gvisor-bin/gvisor_sentry").read_bytes(),
                files["gvisor-bin/gvisor_sentry"],
            )
            self.assertNotEqual(
                path("/etc/containerd/runsc.toml").read_text(), "stale configuration"
            )
            calls.assert_any_call("systemctl", "restart", "k3s", timeout=240)


if __name__ == "__main__":
    unittest.main()
