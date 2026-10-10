"""Activity/admission races using private temporary host state."""

from contextlib import ExitStack, redirect_stdout
from concurrent.futures import ThreadPoolExecutor
import bz2
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import tarfile
from threading import Event
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "runtime"))
import host
import gcip_quiesce
import controller


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

    def configure(self, descriptor=None, fingerprint=None, extra=None):
        external = {"AIDASH_OIDC_CLIENT_ID": "fixture-client", "AIDASH_OIDC_CLIENT_SECRET": "fixture-secret"}
        if fingerprint is not None:
            external["AIDASH_PROVIDER_FINGERPRINT_KEY"] = fingerprint
        external.update(extra or {})
        secret = {"payload": {"data": host.base64.b64encode(json.dumps(external).encode()).decode()}}

        def request(url, *args, **kwargs):
            if url.startswith("https://secretmanager.googleapis.com/"):
                return json.dumps(secret).encode()
            self.assertTrue(url.endswith("/instance/attributes/aidash-provider-credentials"))
            if isinstance(descriptor, BaseException):
                raise descriptor
            if descriptor is None:
                raise HTTPError(url, 404, "legacy host", {}, None)
            return json.dumps(descriptor).encode()

        with patch.object(host, "request", side_effect=request), patch.object(host, "cloud_token", return_value="fixture"):
            host.configuration({"project": "fixture", "secret": "home-a", "hostname": "example.invalid"})
        return dict(line.split("=", 1) for line in (host.RUN / "app.env").read_text().splitlines())

    def test_memory_recovery_directory_follows_the_retained_home_identity(self):
        values = self.configure()
        self.assertEqual(values["AIDASH_MEMORY_RECOVERY_DIR"], str(host.ROOT / "memory-recovery"))
        self.assertEqual(values["AIDASH_NODE_ID"], "aidash://home-a")

    def test_prompt_cache_key_is_added_to_an_existing_identity_without_rotation(self):
        (self.directory / "identity.json").write_text(json.dumps({"database": "d", "api": "a", "runner": "r"}))
        keys = []
        for _ in range(2):
            values = self.configure()
            self.assertEqual(values["AIDASH_API_TOKEN"], "a")
            keys.append(values["AIDASH_PROMPT_CACHE_KEY"])
        self.assertEqual(keys[0], keys[1])
        self.assertEqual(len(keys[0]), 64)

    def test_runtime_key_allowlist_accepts_fingerprint_and_rejects_master_key(self):
        for key, allowed in [("AIDASH_PROVIDER_FINGERPRINT_KEY", True), ("AIDASH_SECRET_TOOL", True), ("AIDASH_PROVIDER_STORE_MASTER_KEY", False), ("UNRELATED_SENTINEL", False)]:
            with self.subTest(key=key):
                if allowed:
                    values = self.configure(extra={key: "fixture-value"})
                    self.assertEqual(values[key], "fixture-value")
                else:
                    with self.assertRaises(ValueError):
                        self.configure(extra={key: "fixture-value"})

    def test_managed_provider_descriptor_reaches_worker_without_key_material(self):
        provider = {
            "fingerprint_key": {"env": "AIDASH_PROVIDER_FINGERPRINT_KEY"},
            "store": {"kind": "secret_manager", "byok_project_id": "aidash-byok-fixture", "environment_id": "test"},
            "broker": {"endpoint": "https://broker.run.app/api/v1", "issuer": "aidash", "audience": "test", "kid": "kms-version"},
        }
        previous_umask = os.umask(0o077)
        try:
            self.configure(provider, "independent-fingerprint-canary-0123456789")
        finally:
            os.umask(previous_umask)
        target = host.RUN / "provider-settings/settings.json"
        self.assertEqual(json.loads(target.read_text()), {"provider_credentials": provider})
        self.assertEqual(target.stat().st_mode & 0o777, 0o644)
        self.assertEqual(target.parent.stat().st_mode & 0o777, 0o755)
        self.assertNotIn("canary", target.read_text())
        self.assertIn(f"AIDASH_PROVIDER_CREDENTIAL_SETTINGS={target}\n", (host.RUN / "app.env").read_text())
        self.configure(None)
        self.assertEqual(json.loads(target.read_text()), {"provider_credentials": {"fingerprint_key": None, "store": None, "broker": None}})

    def test_legacy_metadata_absence_and_external_source_allowlist(self):
        external = {"AIDASH_OIDC_CLIENT_ID": "fixture", "AIDASH_OIDC_CLIENT_SECRET": "fixture"}
        def request(url, *args):
            if "instance/attributes/aidash-provider-credentials" in url:
                raise HTTPError(url, 404, "absent", {}, None)
            return json.dumps({"payload": {"data": host.base64.b64encode(json.dumps(external).encode()).decode()}}).encode()
        with patch.object(host, "request", side_effect=request), patch.object(host, "cloud_token", return_value="fixture"):
            host.configuration({"project": "fixture", "secret": "test", "hostname": "example.invalid"})
            self.assertEqual(json.loads((host.RUN / "provider-settings/settings.json").read_text())["provider_credentials"], {"fingerprint_key": None, "store": None, "broker": None})
            external["AIDASH_PROVIDER_CREDENTIAL_SETTINGS"] = "/untrusted/settings.json"
            with self.assertRaisesRegex(ValueError, "runtime secret may contain only"):
                host.configuration({"project": "fixture", "secret": "test", "hostname": "example.invalid"})

    def store_descriptor(self):
        return {"fingerprint_key": {"env": "AIDASH_PROVIDER_FINGERPRINT_KEY"}, "store": {"kind": "secret_manager", "byok_project_id": "aidash-byok-fixture", "environment_id": "pr-42"}, "broker": None}

    def test_byok_metadata_enables_store_without_putting_fingerprint_in_settings(self):
        fingerprint = "independent-fingerprint-test-key-0123456789"
        previous = host.os.umask(0o077)
        try:
            values = self.configure(self.store_descriptor(), fingerprint)
        finally:
            host.os.umask(previous)
        path = Path(values["AIDASH_PROVIDER_CREDENTIAL_SETTINGS"])
        self.assertEqual(path, host.RUN / "provider-settings/settings.json")
        self.assertEqual(json.loads(path.read_text()), {"provider_credentials": self.store_descriptor()})
        self.assertNotIn(fingerprint, path.read_text())
        self.assertEqual(path.stat().st_mode & 0o777, 0o644)
        self.assertEqual(path.parent.stat().st_mode & 0o777, 0o755)
        self.assertEqual((host.RUN / "app.env").stat().st_mode & 0o777, 0o600)
        self.assertEqual(values["AIDASH_PROVIDER_FINGERPRINT_KEY"], fingerprint)
        again = self.configure(self.store_descriptor(), fingerprint)
        self.assertEqual(again["AIDASH_PROVIDER_FINGERPRINT_KEY"], fingerprint)
        self.assertEqual(json.loads(path.read_text()), {"provider_credentials": self.store_descriptor()})

    def test_byok_disabled_renders_no_store_and_requires_no_fingerprint(self):
        for descriptor in [None, {"fingerprint_key": None, "store": None, "broker": None}]:
            with self.subTest(descriptor=descriptor):
                values = self.configure(descriptor)
                self.assertNotIn("AIDASH_PROVIDER_FINGERPRINT_KEY", values)
                self.assertEqual(json.loads(Path(values["AIDASH_PROVIDER_CREDENTIAL_SETTINGS"]).read_text()), {"provider_credentials": {"fingerprint_key": None, "store": None, "broker": None}})

    def test_legacy_fingerprint_name_rolls_over_without_reaching_the_app(self):
        legacy = "legacy-independent-fingerprint-0123456789"
        values = self.configure(self.store_descriptor(), extra={"AIDASH_SECRET_PROVIDER_FINGERPRINT": legacy})
        self.assertEqual(values["AIDASH_PROVIDER_FINGERPRINT_KEY"], legacy)
        self.assertNotIn("AIDASH_SECRET_PROVIDER_FINGERPRINT", values)
        current = "current-independent-fingerprint-0123456789"
        values = self.configure(self.store_descriptor(), current, extra={"AIDASH_SECRET_PROVIDER_FINGERPRINT": legacy})
        self.assertEqual(values["AIDASH_PROVIDER_FINGERPRINT_KEY"], current)
        self.assertNotIn("AIDASH_SECRET_PROVIDER_FINGERPRINT", values)
        values = self.configure(extra={"AIDASH_SECRET_PROVIDER_FINGERPRINT": legacy})
        self.assertNotIn("AIDASH_SECRET_PROVIDER_FINGERPRINT", values)

    def test_enabled_store_requires_fingerprint_and_rejects_invalid_metadata(self):
        for fingerprint in [None, "too-short", " " * 32, "  too-short  "]:
            with self.subTest(fingerprint=fingerprint), self.assertRaisesRegex(ValueError, "fingerprint key"):
                self.configure(self.store_descriptor(), fingerprint)
        for descriptor in [{"store": {}, "core": {}}, {**self.store_descriptor(), "store": {**self.store_descriptor()["store"], "environment_id": "pr-42/other"}}, {**self.store_descriptor(), "fingerprint_key": {"env": "AIDASH_SECRET_OTHER"}}, {**self.store_descriptor(), "store": {**self.store_descriptor()["store"], "kind": "postgres"}}]:
            with self.subTest(descriptor=descriptor), self.assertRaisesRegex(ValueError, "invalid managed"):
                self.configure(descriptor, "independent-fingerprint-test-key-0123456789")
        with self.assertRaises(HTTPError):
            self.configure(HTTPError("", 403, "denied", {}, None))
        self.assertFalse((host.RUN / "app.env").exists())
        self.assertFalse((host.RUN / "provider-settings/settings.json").exists())

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

    def test_gcip_quiescence_freezes_application_before_stopping_runner(self):
        calls = []

        def command(*args, **kwargs):
            calls.append(args)
            if args[:2] == ("docker", "ps"):
                return b"owned-app\n"
            if args[:2] == ("docker", "inspect"):
                return b'{"Running": true, "Paused": false}'
            return b""

        with patch.object(host, "command", command):
            self.assertEqual(gcip_quiesce.quiesce(host), {"quiesced": True})
        self.assertLess(
            calls.index(("docker", "pause", "aidash-app")),
            calls.index(("systemctl", "stop", "aidash-runner")),
        )
        self.assertTrue((self.directory / "run/draining").exists())
        self.assertFalse((self.directory / "run/serving").exists())
        self.assertNotIn(("docker", "unpause", "aidash-app"), calls)

    def test_gcip_quiescence_failure_never_reopens_old_authority(self):
        calls = []

        def command(*args, **kwargs):
            calls.append(args)
            if args[:2] == ("docker", "ps"):
                return b"owned-app\n"
            if args[:2] == ("docker", "inspect"):
                return b'{"Running": true, "Paused": false}'
            if args[:2] == ("systemctl", "stop"):
                raise RuntimeError("runner stop failed")
            return b""

        with patch.object(host, "command", command):
            with self.assertRaisesRegex(RuntimeError, "runner stop failed"):
                gcip_quiesce.quiesce(host)
        self.assertIn(("docker", "pause", "aidash-app"), calls)
        self.assertNotIn(("docker", "unpause", "aidash-app"), calls)
        self.assertTrue((self.directory / "run/draining").exists())

    def test_gcip_ssh_fence_executes_on_the_retained_host_api(self):
        calls = []

        def command(*args, **kwargs):
            calls.append(args)
            if args[:2] == ("docker", "ps"):
                return b"owned-app\n"
            if args[:2] == ("docker", "inspect"):
                return b'{"Running": true, "Paused": false}'
            return b""

        with patch.object(controller, "run", return_value=b'{"quiesced":true}') as ssh:
            controller.host(
                {"project_id": "fixture"},
                {"instance": "retained", "zone": "fixture-zone"},
                "quiesce",
            )
        args = ssh.call_args.args
        cmd = args[args.index("--command") + 1]
        source = cmd.split("<<'AIDASH_GCIP_QUIESCE'\n", 1)[1].rsplit("\nAIDASH_GCIP_QUIESCE", 1)[0]
        output = io.StringIO()
        with patch.object(host, "command", command), patch.object(sys, "path", list(sys.path)), redirect_stdout(output):
            # The retained lifecycle state and command API need no new CLI action or
            # updated bundle is required to freeze its old policy first.
            exec(source, {"__name__": "quiesce_fixture"})
        self.assertEqual(json.loads(output.getvalue()), {"quiesced": True})
        self.assertIn(("docker", "pause", "aidash-app"), calls)
        self.assertIn(("systemctl", "stop", "aidash-runner"), calls)

    def test_gcip_quiescence_accepts_an_already_paused_or_removed_application(self):
        for container in (b"owned-app\n", b""):
            with self.subTest(container=bool(container)):
                calls = []

                def command(*args, calls=calls, container=container, **kwargs):
                    calls.append(args)
                    if args[:2] == ("docker", "ps"):
                        return container
                    if args[:2] == ("docker", "inspect"):
                        return b'{"Running": true, "Paused": true}'
                    return b""

                with patch.object(host, "command", command), patch.object(host, "request", side_effect=AssertionError("a paused or absent application cannot answer HTTP")):
                    self.assertEqual(gcip_quiesce.quiesce(host), {"quiesced": True})
                self.assertNotIn(("docker", "pause", "aidash-app"), calls)
                self.assertIn(("systemctl", "stop", "aidash-runner"), calls)

    def test_gcip_quiescence_waits_for_the_existing_host_lifecycle_lock(self):
        host.RUN.mkdir()
        lock_path = host.RUN / "lifecycle.lock"
        acquiring = Event()
        calls = []
        flock = host.fcntl.flock

        with lock_path.open("a") as owner:
            flock(owner, host.fcntl.LOCK_EX)

            def acquire(lock, operation):
                self.assertEqual(os.fstat(lock.fileno()).st_ino, os.fstat(owner.fileno()).st_ino)
                acquiring.set()
                flock(lock, operation)

            def command(*args, **kwargs):
                calls.append(args)
                return b""

            with patch.object(host.fcntl, "flock", acquire), patch.object(host, "command", command), ThreadPoolExecutor(max_workers=1) as executor:
                future = executor.submit(gcip_quiesce.quiesce, host)
                try:
                    self.assertTrue(acquiring.wait(1), "quiescence bypassed the lifecycle lock")
                    self.assertFalse(future.done())
                    self.assertFalse(calls)
                finally:
                    flock(owner, host.fcntl.LOCK_UN)
                self.assertEqual(future.result(timeout=5), {"quiesced": True})

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
