import importlib.util
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    "evaluate_capability_exposure", Path(__file__).with_name("evaluate-capability-exposure.py")
)
exposure = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(exposure)


class EvaluationReadTests(unittest.TestCase):
    def test_answers_requests_on_later_pages_of_the_selected_run(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory) / "fixture.toml"
            fixture.write_text('''version = 1
instructions = "Test"
[[tools]]
alias = "lookup"
description = "Lookup"
schema = '{}'
[[cases]]
id = "one"
language = "en"
task = "Find it"
expected_tool = "lookup"
''')
            reads = 0
            answered = []

            def call(path, body):
                nonlocal reads
                if path == "/api/registry" or path.endswith("/delegate"):
                    return 200, {}
                if path == "/api/workspaces":
                    return 200, {"id": "ws"}
                if path == "/api/workspaces/ws/tasks":
                    return 200, {"id": "task"}
                if path == "/api/tasks/task/runs":
                    return 200, {"items": [{"id": "run"}], "next_cursor": None}
                if path == "/api/runs/run":
                    reads += 1
                    return 200, {"run": {"id": "run", "phase": "WAITING" if reads % 2 else "COMPLETED"}}
                if path == "/api/human-requests?run=run":
                    return 200, {"items": [], "next_cursor": "hidden+/="}
                if path == "/api/human-requests?run=run&cursor=hidden%2B%2F%3D":
                    return 200, {"items": [{"id": "question", "kind": "QUESTION", "prompt": "Where?"}], "next_cursor": None}
                if path == "/api/human-requests/question/answer":
                    answered.append(body)
                    return 200, {}
                if path.startswith("/api/capabilities/approvals?"):
                    return 200, {"items": [], "next_cursor": None}
                if path == "/api/workspaces/ws":
                    return 200, {"events": []}
                raise AssertionError(path)

            report = exposure.evaluate(call, "node", {"id": "model", "version": "1"}, "test",
                                       fixture=fixture, interval=0)
            self.assertEqual(len(answered), 2)
            self.assertEqual(answered[0], {"answer": exposure.NO_HUMAN})
            self.assertEqual(report["cases"][0]["variants"]["legacy"]["phase"], "COMPLETED")


if __name__ == "__main__":
    unittest.main()
