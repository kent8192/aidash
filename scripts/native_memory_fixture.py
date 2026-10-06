#!/usr/bin/env python3
"""Synthetic native-memory provider for real two-Node cluster acceptance."""
import json
from http.server import ThreadingHTTPServer

from remote_memory_fixture import Handler, LOCK, STATE


class NativeHandler(Handler):
    def do_POST(self):
        if self.path != "/v1/chat/completions":
            return super().do_POST()
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        try:
            assert not self.headers.get("Authorization"), "caller credentials reached an unsigned model"
            context = json.loads(body["messages"][1]["content"])
            if "mode" in context:
                assert context["mode"] in ("retain", "run_candidate"), context
                evidence = context["evidence"]
                with LOCK:
                    STATE["requests"].append({"kind": "memory_extraction", "body": body})
                facts = []
                labels = [
                    ("Aiko prefers rail travel. 愛子は鉄道での旅行を好む。", "world", "preference"),
                    ("Aiko does not travel by car. 愛子は車で旅行しない。", "world", "fact"),
                    ("The rollback check failed on 2024-01-04. ロールバック検証は失敗した。", "experience", "failure"),
                    ("Rain caused the train delay. 大雨で列車が遅延した。", "world", "fact"),
                ]
                if context["mode"] == "run_candidate":
                    journal = json.loads(context["text"])
                    assert "observed_tasks" in journal["journal"], journal
                    assert "rollback check failed" in context["text"] and "検証は失敗" in context["text"], journal
                    labels = [
                        ("The rollback check failed; success was not verified. ロールバック検証は失敗し、成功は確認されていない。", "experience", "failure"),
                        ("Check the rollback result before claiming success. 成功を報告する前にロールバック結果を確認する。", "experience", "procedure"),
                    ]
                for text, kind, learning in labels:
                    facts.append({"text": text, "kind": kind, "learning": learning, "verification": "unverified",
                        "mental_model": None, "occurred": None, "entities": [], "evidence": evidence, "links": []})
                return self.reply(200, {"choices": [{"index": 0, "finish_reason": "stop", "message": {
                    "role": "assistant", "content": json.dumps({"facts": facts, "causal": []}, ensure_ascii=False)}}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1}})
            current = context["current"]
            task = current["task"]
            if task["title"].startswith("Learning "):
                with LOCK:
                    STATE["requests"].append({"kind": "learning_inference", "task": task["id"], "body": body})
                return self.reply(200, {"choices": [{"index": 0, "finish_reason": "stop", "message": {
                    "role": "assistant", "content": "Fixture observation: the rollback check failed. ロールバック検証は失敗した。 Success was not verified."}}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1}})
            parent = task["title"] == "Generated origin"
            with LOCK:
                previous = sum(r["kind"] == "parent" and r["task"] == task["id"] for r in STATE["requests"])
                STATE["requests"].append({"kind": "parent" if parent else "inference", "task": task["id"], "body": body})
                semantic = current["semantic_memory"]
                if not parent:
                    assert semantic["home_node"] == "aidash://tx-00", semantic
                    assert not semantic["result"]["matches"], "native retrieval unioned generic memory"
                banks = semantic["memory"]["banks"]
                assert len(banks) == 2 and banks[1]["bank"]["participant"] is None, banks
                encoded = json.dumps(banks, ensure_ascii=False)
                assert "ochre falcon" in encoded and "DO NOT DISCLOSE" not in encoded, banks
                for bank in banks:
                    for unit in bank["recall"].get("units", []):
                        assert unit["revision"] > 0 and unit["content"]["verification"] == "unverified", unit
                names = [item["function"]["name"] for item in body.get("tools", [])]
                if not parent:
                    assert not {"memory_mutate", "memory_recall", "memory_reflect"}.intersection(names), names
                    LOCK.wait_for(lambda: not STATE["hold_inference"], timeout=300)
            if parent:
                if previous == 0:
                    name, arguments = "task_create", {"title": "Generated remote research", "description": "Find the relevant non-keyword archive marker", "requirements": {}}
                else:
                    name, arguments = "human_request", {"kind": "INFORMATION_REQUEST", "prompt": "Wait while the foreign child completes"}
                message = {"role": "assistant", "content": None, "tool_calls": [{"id": f"parent-{previous}", "type": "function", "function": {"name": name, "arguments": json.dumps(arguments)}}]}
            else:
                message = {"role": "assistant", "content": "Remote result: ochre falcon / 黄土色のハヤブサ from current Home memory."}
            self.reply(200, {"choices": [{"index": 0, "finish_reason": "tool_calls" if parent else "stop", "message": message}], "usage": {"prompt_tokens": 1, "completion_tokens": 1}})
        except Exception as error:
            with LOCK:
                STATE["errors"].append(f"{type(error).__name__}: {error}")
            self.reply(400, {"error": "native acceptance assertion failed"})


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 8080), NativeHandler).serve_forever()
