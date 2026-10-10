"""Behavioral regressions for lifecycle races, authorization, and idle semantics."""

from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
from policy import (
    Refused,
    attach_release,
    build_needed,
    idle_due,
    preview_command,
    transition,
)


class LifecycleTests(unittest.TestCase):
    def request(self, action="create", sequence=1, identity="test", **values):
        return dict(
            environment=identity,
            action=action,
            sequence=sequence,
            sha="a" * 40,
            source_ref="main",
            source_repo="kent8192/aidash",
            **values,
        )

    def apply(self, state, request):
        return transition(state, request, "a" * 12, 10000)[0]

    def test_push_never_creates_or_revives_and_test_stays_pinned(self):
        for state in [{}, {"environments": {}}]:
            self.assertFalse(transition(state, self.request("update"), "a" * 12, 0)[1])
        state = self.apply({}, self.request())
        state = self.apply(state, self.request("stop", 2))
        update = self.request("update", 3)
        update["sha"] = "b" * 40
        self.assertEqual(self.apply(state, update), state)
        state = self.apply(state, self.request("destroy", 4))
        self.assertEqual(self.apply(state, self.request("update", 5)), state)

    def test_develop_updates_stopped_source_without_waking(self):
        request = self.request(identity="develop")
        request["source_ref"] = "develop/0.1.0"
        state = self.apply({}, request)
        self.assertFalse(state["environments"]["develop"]["spot"])
        state = self.apply(state, dict(request, action="stop", sequence=2))
        state = self.apply(
            state, dict(request, action="update", sequence=3, sha="b" * 40)
        )
        entry = state["environments"]["develop"]
        self.assertEqual((entry["desired"], entry["sha"]), ("stopped", "b" * 40))
        self.assertFalse(build_needed(entry))

    def test_stale_rerun_cannot_undo_destroy(self):
        state = self.apply({}, self.request())
        state = self.apply(state, self.request("destroy", 5))
        for action in ["create", "up", "resume", "update"]:
            self.assertEqual(self.apply(state, self.request(action, 4)), state)
        with self.assertRaises(Refused):
            self.apply(state, self.request("create", 6))

    def test_stop_arriving_before_create_fences_the_old_request(self):
        state = self.apply({}, self.request("stop", 5))
        self.assertEqual(state["environments"]["test"]["desired"], "destroyed")
        self.assertEqual(self.apply(state, self.request("create", 4)), state)
        self.assertEqual(self.apply(state, self.request("update", 6)), state)

    def test_old_build_cannot_attach_after_stop(self):
        state = self.apply({}, self.request())
        state = self.apply(state, self.request("stop", 2))
        new, accepted = attach_release(state, "test", 1, {})
        self.assertFalse(accepted)
        self.assertEqual(state, new)

    def test_release_requires_immutable_auxiliary_images(self):
        state = self.apply({}, self.request())
        images = {
            name: f"us-central1-docker.pkg.dev/fixture/aidash/{name}@sha256:{'a' * 64}"
            for name in ("app", "postgres", "sandbox", "observer", "nats", "control", "edge", "caddy")
        }
        release = {"source_sha": "a" * 40, "images": images}
        self.assertTrue(attach_release(state, "test", 1, release)[1])
        for name in ("nats", "control", "caddy"):
            for invalid in (None, f"{name}:latest"):
                with self.subTest(name=name, image=invalid):
                    candidate = dict(images)
                    if invalid is None:
                        del candidate[name]
                    else:
                        candidate[name] = invalid
                    with self.assertRaises(Refused):
                        attach_release(
                            state, "test", 1, dict(release, images=candidate)
                        )

    def test_preserve_manual_normal_override_on_update_and_resume(self):
        request = self.request(identity="pr-12", mode="normal")
        request["source_ref"] = "pr/12"
        state = self.apply({}, request)
        state = self.apply(
            state, dict(request, action="update", sequence=2, sha="b" * 40, mode="")
        )
        state = self.apply(state, dict(request, action="stop", sequence=3, mode=""))
        state = self.apply(state, dict(request, action="resume", sequence=4, mode=""))
        self.assertFalse(state["environments"]["pr-12"]["spot"])

    def test_fork_approval_cannot_follow_head(self):
        request = self.request(identity="pr-1", fork=True, approved_sha="a" * 40)
        request["source_ref"] = "pr/1"
        state = self.apply({}, request)
        with self.assertRaises(Refused):
            self.apply(state, dict(request, action="resume", sequence=2, sha="b" * 40))

    def test_request_for_second_pr_waits_without_discarding_first(self):
        first = self.request(identity="pr-1")
        first["source_ref"] = "pr/1"
        state = self.apply({}, first)
        state["environments"]["pr-1"]["applied"] = {"running": True}
        second = dict(first, environment="pr-2", source_ref="pr/2", sequence=2)
        result = self.apply(state, second)
        self.assertEqual(result["environments"]["pr-1"], state["environments"]["pr-1"])
        self.assertEqual(
            result["environments"]["pr-2"]["status"], "waiting_for_pr_slot"
        )

    def test_create_never_overwrites_a_different_source(self):
        state = self.apply({}, self.request())
        with self.assertRaises(Refused):
            self.apply(state, dict(self.request(sequence=2), sha="b" * 40))

    def test_no_production_or_arbitrary_environment(self):
        for identity in ["production", "main", "pr-0", "../test", "test;echo bad"]:
            with self.assertRaises(Refused):
                self.apply({}, self.request(identity=identity))

    def test_only_exact_commands(self):
        self.assertEqual(preview_command("/preview stop")["action"], "stop")
        self.assertEqual(
            preview_command("/preview up normal " + "a" * 40)["approved_sha"], "a" * 40
        )
        for body in [
            "please /preview up",
            "/preview up\necho bad",
            "/preview destroy; bad",
            "/preview up $(bad)",
        ]:
            self.assertIsNone(preview_command(body))

    def test_unknown_or_old_observation_defers_shutdown(self):
        value = dict(
            protocol="aidash-infra-activity/1",
            busy=False,
            observed_at=10000,
            last_active=0,
        )
        self.assertTrue(idle_due(value, 10000))
        self.assertFalse(idle_due(value, 10000, keepalive_at=9999))
        self.assertFalse(idle_due(dict(value, busy=True), 10000))
        self.assertFalse(idle_due(dict(value, last_active=9999), 10000))
        with self.assertRaises(Refused):
            idle_due(value, 10121)
        with self.assertRaises(Refused):
            idle_due(dict(value, protocol="unknown"), 10000)


if __name__ == "__main__":
    unittest.main()
