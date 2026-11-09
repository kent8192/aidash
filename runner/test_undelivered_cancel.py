"""A cancel can reach the Runner before the command it cancels, or instead of it."""
import hashlib
import http.client
import json
import tempfile
import threading
import unittest
import uuid
from http.server import ThreadingHTTPServer
from pathlib import Path
from unittest.mock import Mock, patch

from control import ActivityAggregate, Handler, Rejected, Runner

INSTANCE = "original-runner"
DIGEST = "immutable-intent"


def request(operation):
    return {"operation_id": operation, "area_id": str(uuid.uuid4()), "epoch": 1, "usage_tenant": "acme",
            "digest": DIGEST, "kind": "shell", "code": "git push", "seconds": 60, "files": []}


def refusal(instance=INSTANCE):
    return {"digest": DIGEST, "instance": instance}


class UndeliveredCancelTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = directory.name
        self.runner = self.restart()
        self.operation = str(uuid.uuid4())

    def restart(self):
        runner = object.__new__(Runner)
        runner.root = Path(self.root)
        runner.lock = threading.RLock()
        runner.instance = INSTANCE
        runner.proven = True
        runner.active = set()
        runner.aggregate = ActivityAggregate()
        runner.config = {"image": "runner-image", "maximum_seconds": 600, "working_bytes": 1 << 20, "limits": {}}
        runner.kube_json = Mock(return_value={"items": []})
        runner.start = Mock()
        return runner

    def assert_refused(self, runner, operation):
        """The send starts nothing and is answered with the refusal on record."""
        reply = runner.accept(request(operation))
        self.assert_refusal_reply(reply, operation)
        runner.kube_json.assert_not_called()
        runner.start.assert_not_called()
        self.assertEqual(list(Path(self.root).glob("area-*.json")), [])

    def assert_refusal_reply(self, reply, operation):
        self.assertEqual(reply["operation_id"], operation)
        self.assertEqual(reply["digest"], DIGEST)
        self.assertEqual(reply["status"], "cancelled")
        self.assertIs(reply["termination_confirmed"], True)
        self.assertIs(reply["executed"], False)

    def test_a_cancel_ahead_of_the_send_ends_the_operation_unexecuted(self):
        reply = self.runner.cancel(self.operation, refusal())
        self.assert_refusal_reply(reply, self.operation)
        self.runner.start.assert_not_called()

    def test_a_send_after_the_cancel_is_refused_and_starts_nothing(self):
        self.runner.cancel(self.operation, refusal())
        self.assert_refused(self.runner, self.operation)

    def test_a_resend_after_a_lost_refusal_reply_gets_the_refusal_back(self):
        # The refusal is recorded but its reply never reached the server, which
        # still holds the operation as unsent and sends it again. The Runner must
        # answer with the record so the server can settle, not conflict forever.
        lost = self.runner.cancel(self.operation, refusal())
        self.assertEqual(self.runner.accept(request(self.operation)), lost)
        self.assertEqual(self.runner.accept(request(self.operation)), lost)
        self.runner.start.assert_not_called()

    def test_a_send_with_another_intent_than_the_refusal_is_a_conflict(self):
        self.runner.cancel(self.operation, refusal())
        with self.assertRaises(Rejected) as rejected:
            self.runner.accept(dict(request(self.operation), digest="another-intent"))
        self.assertEqual(rejected.exception.code, 409)
        self.runner.start.assert_not_called()

    def test_a_repeated_cancel_returns_the_same_outcome(self):
        first = self.runner.cancel(self.operation, refusal())
        self.assertEqual(self.runner.cancel(self.operation, refusal()), first)

    def test_a_delivered_operation_is_cancelled_as_usual(self):
        self.runner.accept(request(self.operation))
        reply = self.runner.cancel(self.operation, refusal())
        self.assertEqual(reply["status"], "accepted")
        self.assertIs(reply["cancel_requested"], True)
        self.assertIs(reply["termination_confirmed"], False)
        self.runner.start.assert_called_once_with(self.operation)

    def test_a_cancel_waits_for_a_send_already_holding_the_lock(self):
        listing, finish = threading.Event(), threading.Event()

        def stalled(_):
            listing.set()
            self.assertTrue(finish.wait(10))
            return {"items": []}

        self.runner.kube_json = Mock(side_effect=stalled)
        send = threading.Thread(target=self.runner.accept, args=(request(self.operation),))
        send.start()
        self.assertTrue(listing.wait(10))
        replies = []
        cancel = threading.Thread(target=lambda: replies.append(self.runner.cancel(self.operation, refusal())))
        cancel.start()
        cancel.join(0.2)
        self.assertTrue(cancel.is_alive())
        finish.set()
        send.join(10)
        cancel.join(10)
        self.assertEqual(replies[0]["status"], "accepted")
        self.assertIs(replies[0]["cancel_requested"], True)

    def test_a_cancel_is_not_overtaken_by_the_start_of_the_command_it_read(self):
        # `commit_inputs` starts the command under the lock. If the cancel read
        # `awaiting_files` and released the lock before it wrote `cancelled`, the
        # command would start in between while the reply, which the server trusts,
        # says it never ran.
        data = b"input"
        file_id = str(uuid.uuid4())
        staged = dict(request(self.operation), files=[{
            "file_id": file_id, "path": "a", "scope": "working", "size": len(data),
            "digest": hashlib.sha256(data).hexdigest()}])
        self.runner.accept(staged)
        self.runner.input_chunk(self.operation, file_id, 0, data)
        read, canceller, committers = Runner.get, threading.Thread(), []

        def get(runner, operation):
            record = read(runner, operation)
            if threading.current_thread() is canceller and not committers:
                committers.append(threading.Thread(target=runner.commit_inputs, args=(operation,)))
                committers[0].start()
                committers[0].join(0.5)  # a correct cancel keeps the lock, so this times out
            return record

        replies = []
        canceller = threading.Thread(target=lambda: replies.append(self.runner.cancel(self.operation, refusal())))
        with patch.object(Runner, "get", get):
            canceller.start()
            canceller.join(10)
            committers[0].join(10)
        self.assertFalse(canceller.is_alive() or committers[0].is_alive())
        never_ran = replies[0]["status"] == "cancelled" and replies[0]["executed"] is False
        self.assertFalse(never_ran and self.runner.start.called,
                         "the reply says the command never ran, but it was started")

    def test_a_send_waiting_for_the_lock_is_refused_after_the_cancel(self):
        outcome = []

        def send():
            outcome.append(self.runner.accept(request(self.operation)))

        with self.runner.lock:
            waiting = threading.Thread(target=send)
            waiting.start()
            waiting.join(0.2)
            self.assertTrue(waiting.is_alive())
            self.runner.cancel(self.operation, refusal())
        waiting.join(10)
        self.assertEqual(len(outcome), 1)
        self.assert_refusal_reply(outcome[0], self.operation)
        self.runner.kube_json.assert_not_called()
        self.runner.start.assert_not_called()

    def test_another_runner_instance_records_nothing_and_answers_absent(self):
        with self.assertRaises(Rejected) as rejected:
            self.runner.cancel(self.operation, refusal("replacement-runner"))
        self.assertEqual(rejected.exception.code, 404)
        self.assertFalse(self.runner.path(self.operation).exists())

    def test_a_plain_cancel_of_an_unknown_operation_stays_absent(self):
        with self.assertRaises(Rejected) as rejected:
            self.runner.cancel(self.operation)
        self.assertEqual(rejected.exception.code, 404)
        self.assertFalse(self.runner.path(self.operation).exists())

    def test_a_malformed_refusal_records_nothing(self):
        for malformed in ({}, {"digest": DIGEST}, {"digest": "", "instance": INSTANCE},
                          {"digest": 1, "instance": INSTANCE}, dict(refusal(), extra=True)):
            with self.assertRaises(Rejected) as rejected:
                self.runner.cancel(self.operation, malformed)
            self.assertEqual(rejected.exception.code, 400)
            self.assertFalse(self.runner.path(self.operation).exists())

    def test_the_refusal_is_settled_and_survives_a_restart(self):
        self.runner.cancel(self.operation, refusal())
        self.assertEqual(self.runner.aggregate.unsettled, set())
        restarted = self.restart()
        restarted.recover()
        restarted.start.assert_not_called()
        self.assertEqual(restarted.aggregate.unsettled, set())
        self.assert_refused(restarted, self.operation)

    def test_the_refusal_stays_in_force_after_its_acknowledgement(self):
        self.runner.cancel(self.operation, refusal())
        reply = self.runner.acknowledge(self.operation, DIGEST)
        self.assertIs(reply["acknowledged"], True)
        self.assertIs(reply["executed"], False)
        self.assert_refused(self.runner, self.operation)


class UndeliveredCancelRouteTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        runner = object.__new__(Runner)
        runner.root = Path(directory.name)
        runner.lock = threading.RLock()
        runner.instance = INSTANCE
        runner.proven = True
        runner.aggregate = ActivityAggregate()
        runner.token = "secret"
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server.runner = runner
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        self.port = server.server_address[1]
        self.operation = str(uuid.uuid4())

    def post(self, body=None):
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        self.addCleanup(connection.close)
        connection.request("POST", f"/v1/operations/{self.operation}/cancel", body,
                           {"Authorization": "Bearer secret"})
        response = connection.getresponse()
        return response.status, json.loads(response.read())

    def test_the_route_passes_the_refusal_to_the_runner(self):
        status, reply = self.post(json.dumps(refusal()))
        self.assertEqual(status, 200)
        self.assertEqual(reply["status"], "cancelled")

    def test_the_route_keeps_a_cancel_without_a_body_plain(self):
        self.assertEqual(self.post()[0], 404)

    def test_the_route_limits_the_refusal_size(self):
        self.assertEqual(self.post(json.dumps({"digest": "x" * 2048, "instance": INSTANCE}))[0], 413)


if __name__ == "__main__":
    unittest.main()
