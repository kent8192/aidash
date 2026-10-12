"""Scripted provider for the deterministic fixed-task tier (#122).

One local HTTP server plays three external parties:

- an OpenAI-compatible model at /v1/chat/completions whose replies are the
  scripted fixture Agent behaviors named by corpus tasks;
- the Jev compaction classifier at /jev, which marks every prunable tool
  result as droppable (the most aggressive pruning policy);
- the fixture HTTP tools at /tools/{task}/{alias}.

The scripted Agent is a probe, not a model: it acts only on what the harness
actually delivered in the request. A correction that compaction removed is
invisible to it, so a lost constraint shows up in its answer and the task's
independent oracle catches it. Usage (one token per byte), cost and prompt-cache
counts are synthetic and deterministic; they test accounting, not provider prices.
"""
import hashlib
import json
import math
import re
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# One token per byte. Reported prompt tokens then never fall below Aidash's byte
# estimate, so a calibrated estimator (ADR 0017, capped at bytes) behaves exactly
# like the conservative one and compaction decisions stay identical across
# repeats and arms however many Usage Records accumulate.
CHARS_PER_TOKEN = 1
# Synthetic nanocredits per token, Anthropic-shaped: cache reads are cheaper and
# writes dearer than plain input. These are fixture values, not prices.
PRICE = {"input": 100, "cache_read": 10, "cache_write": 125, "output": 500}
SALT_PREFIX = "aidash-cache-scope:"

PRICING_VISIBLE_ONLY = '''"""Order pricing for the fixed-task evaluation snapshot."""


def discounted_total(prices_cents, coupon_cents=0, percent_off=0):
    """Return the payable total in whole cents.

    Apply percent_off to the subtotal first, rounding half up to a whole cent,
    then subtract coupon_cents. The payable total is never negative.
    """
    subtotal = sum(prices_cents)
    discounted = (subtotal * (100 - percent_off) + 50) // 100
    return discounted - coupon_cents
'''
PRICING_COMPLETE = PRICING_VISIBLE_ONLY.replace(
    "    return discounted - coupon_cents\n", "    return max(0, discounted - coupon_cents)\n")


def tokens(text):
    return math.ceil(len(text.encode()) / CHARS_PER_TOKEN)


def collect_strings(value, output):
    if isinstance(value, str):
        output.append(value)
        nested = decode_json(value)
        if nested is not None and not isinstance(nested, str):
            collect_strings(nested, output)
    elif isinstance(value, dict):
        for item in value.values():
            collect_strings(item, output)
    elif isinstance(value, list):
        for item in value:
            collect_strings(item, output)


def decode_json(text):
    """Decode a JSON document, or the first JSON object embedded in text."""
    text = text.strip()
    try:
        return json.loads(text)
    except ValueError:
        start = text.find("{")
        if start < 0:
            return None
        try:
            return json.JSONDecoder().raw_decode(text[start:])[0]
        except ValueError:
            return None


def request_view(body):
    """Every string the harness delivered, in request order, JSON-decoded."""
    output = []
    for key in ("system",):
        if key in body:
            collect_strings(body[key], output)
    collect_strings(body.get("messages", []), output)
    return "\n".join(output)


def message_text(body):
    """The serialized model-visible text used for synthetic token counts."""
    return json.dumps({"system": body.get("system"), "messages": body.get("messages"),
                       "tools": body.get("tools")}, ensure_ascii=False, sort_keys=True)


def cache_boundaries(body):
    """Serialized prefixes ending at each cache_control breakpoint, in order."""
    boundaries = []
    blocks = []
    for message in body.get("messages", []):
        content = message.get("content")
        parts = content if isinstance(content, list) else [{"type": "text", "text": content}]
        for part in parts:
            blocks.append(part)
            if isinstance(part, dict) and part.get("cache_control"):
                boundaries.append(json.dumps(blocks, ensure_ascii=False, sort_keys=True))
    system = body.get("system")
    if isinstance(system, list):
        prefix = []
        for part in system:
            prefix.append(part)
            if isinstance(part, dict) and part.get("cache_control"):
                boundaries.insert(0, json.dumps(prefix, ensure_ascii=False, sort_keys=True))
    return boundaries


class Probe:
    """Per-Task scripted Agent state. The evaluator registers each Task UUID."""

    def __init__(self, task, run_key):
        self.task = task
        self.run_key = run_key
        self.asked = 0
        self.pages = 0
        self.searched = False
        self.loaded = False
        self.called = False
        self.lookups = 0


class Fixture:
    def __init__(self, models):
        # models maps a registered model_id to its context window.
        self.models = models
        self.lock = threading.Lock()
        self.probes = {}
        self.requests = {}
        self.tool_calls = {}
        self.jev_requests = {}
        self.cache = set()
        self.errors = []

    # ------------------------------------------------------------ registry
    def register(self, task_uuid, task, run_key):
        with self.lock:
            self.probes[task_uuid] = Probe(task, run_key)
            self.requests[task_uuid] = []
            self.tool_calls[task["id"]] = self.tool_calls.get(task["id"], [])

    def observed(self, task_uuid):
        with self.lock:
            records = list(self.requests.get(task_uuid, []))
        return {"count": len(records), "bytes": [item["bytes"] for item in records],
                "prompt_tokens": [item["prompt_tokens"] for item in records],
                "tool_definitions": [item["tools"] for item in records],
                "tool_definition_bytes": [item["tool_bytes"] for item in records],
                "cache_breakpoints": [item["breakpoints"] for item in records],
                "salted": [item["salted"] for item in records],
                "overflow_rejections": sum(item["overflow"] for item in records),
                "jev_requests": self.jev_requests.get(task_uuid, 0)}

    # ------------------------------------------------------------- model
    def infer(self, body):
        view = request_view(body)
        with self.lock:
            matches = [uuid for uuid in self.probes if uuid in view]
        if len(matches) != 1:
            raise AssertionError(f"request names {len(matches)} registered Tasks")
        task_uuid = matches[0]
        probe = self.probes[task_uuid]
        text = message_text(body)
        prompt = tokens(text)
        system = body.get("system")
        system_text = system if isinstance(system, str) else json.dumps(system) if system else ""
        if isinstance(system, list):
            system_text = "".join(part.get("text", "") for part in system if isinstance(part, dict))
        if not system_text:
            first = (body.get("messages") or [{}])[0]
            if first.get("role") == "system":
                system_text = first["content"] if isinstance(first["content"], str) else json.dumps(first["content"])
        boundaries = cache_boundaries(body)
        tools = body.get("tools") or []
        window = self.models.get(body.get("model"))
        overflow = window is not None and prompt > window
        with self.lock:
            self.requests[task_uuid].append({
                "bytes": len(text.encode()), "prompt_tokens": prompt, "tools": len(tools),
                "tool_bytes": len(json.dumps(tools, ensure_ascii=False).encode()),
                "breakpoints": len(boundaries), "salted": system_text.lstrip().startswith(SALT_PREFIX),
                "overflow": overflow})
        if overflow:
            return 400, {"error": {"code": 400, "message": f"This endpoint's maximum context length is {window} "
                                   f"tokens. However, you requested about {prompt} tokens."}}
        reply = self.behave(probe, body, view)
        read, write = self.simulate_cache(boundaries)
        completion = tokens(json.dumps(reply, ensure_ascii=False))
        cost = ((prompt - read - write) * PRICE["input"] + read * PRICE["cache_read"]
                + write * PRICE["cache_write"] + completion * PRICE["output"])
        calls = reply.get("tool_calls")
        message = {"role": "assistant", "content": reply.get("content")}
        if calls:
            message["tool_calls"] = [{"id": f"call-{probe.task['id']}-{index}-{len(self.requests[task_uuid])}",
                                      "type": "function",
                                      "function": {"name": name, "arguments": json.dumps(arguments)}}
                                     for index, (name, arguments) in enumerate(calls)]
        usage = {"prompt_tokens": prompt, "completion_tokens": completion,
                 "prompt_tokens_details": {"cached_tokens": read, "cache_write_tokens": write},
                 "cost": "__COST__"}
        payload = {"id": "fixture-completion", "model": body.get("model"),
                   "choices": [{"index": 0, "finish_reason": "tool_calls" if calls else "stop", "message": message}],
                   "usage": usage}
        # Cost is a decimal credit amount; render it exactly, without floats.
        credits = f"{cost // 1_000_000_000}.{cost % 1_000_000_000:09d}"
        return 200, json.dumps(payload).replace('"__COST__"', credits)

    def simulate_cache(self, boundaries):
        """A deterministic prefix cache that reads only at earlier marked boundaries."""
        if not boundaries:
            return 0, 0
        with self.lock:
            longest = max((len(item) for item in self.cache if boundaries[-1].startswith(item)), default=0)
            self.cache.update(boundaries)
        last = len(boundaries[-1])
        read = math.ceil(longest / CHARS_PER_TOKEN)
        write = max(0, math.ceil(last / CHARS_PER_TOKEN) - read)
        return read, write

    def behave(self, probe, body, view):
        task = probe.task
        names = [tool["function"]["name"] for tool in body.get("tools") or [] if "function" in tool]
        behavior = task["fixture_agent"]
        if behavior in ("pricing-visible-only-fix", "pricing-complete-fix"):
            if "def discounted_total" not in view:
                return {"content": "The pricing module was not provided, so I cannot fix it."}
            code = PRICING_VISIBLE_ONLY if behavior == "pricing-visible-only-fix" else PRICING_COMPLETE
            return {"content": f"```python\n{code}```\n\nAll tests pass. The task is complete."}
        if behavior == "invoice-overdue-report":
            return {"content": self.overdue_report(view)}
        if behavior == "constraint-tracker":
            answers = task.get("human_answers", [])
            if probe.asked < len(answers):
                probe.asked += 1
                return {"tool_calls": [("human_request", {"kind": "INFORMATION_REQUEST",
                                                          "prompt": "Please confirm the shortlist constraints."})]}
            if probe.pages < task.get("reference_pages", 0):
                probe.pages += 1
                return {"tool_calls": [("reference_lookup", {"page": probe.pages})]}
            # A delivered correction wins regardless of where the projection places it;
            # otherwise the stated constraint applies. Exclusions accumulate.
            corrected = re.findall(r"Correction:[^\n\"]*?BUDGET-USD=(\d+)", view)
            budgets = corrected or re.findall(r"BUDGET-USD=(\d+)", view)
            excluded = sorted(set(re.findall(r"EXCLUDE-VENDOR=([A-Za-z]+)", view)))
            value = {"budget_usd": int(budgets[-1]) if budgets else None, "excluded_vendors": excluded}
            return {"content": f"```json\n{json.dumps(value)}\n```\n\nShortlist complete."}
        if behavior == "tool-selector":
            expected = task["expected_tool"]
            status = re.search(r'"payment_status"\s*:\s*"(\w+)"', view)
            if probe.called and status:
                return {"content": f"Invoice {task['expected_arguments']['invoice_number']} is {status.group(1)}. "
                                   "Lookup complete."}
            if probe.called:
                return {"content": "The invoice status could not be determined."}
            if expected in names:
                probe.called = True
                return {"tool_calls": [(expected, task["expected_arguments"])]}
            if "capability_search" in names and not probe.searched:
                probe.searched = True
                return {"tool_calls": [("capability_search", {"query": task["search_query"]})]}
            if "capability_load" in names and probe.searched and not probe.loaded:
                probe.loaded = True
                return {"tool_calls": [("capability_load", self.load_arguments(body, view, expected))]}
            return {"content": "No suitable tool is available."}
        if behavior == "skill-follower":
            if "DUN-7Q4" in view:
                return {"content": "Reminder: invoice INV-2026-0105 for 155.00 USD is overdue. Please arrange "
                                   "payment at your earliest convenience. Reference DUN-7Q4. Reminder complete."}
            if "capability_search" in names and not probe.searched:
                probe.searched = True
                return {"tool_calls": [("capability_search", {"query": task["search_query"]})]}
            if "capability_load" in names and probe.searched and not probe.loaded:
                probe.loaded = True
                found = self.search_result(view, "skill")
                return {"tool_calls": [("capability_load", {"alias": found.get("alias"),
                                                            "digest": found.get("digest")})]}
            if "skill_load" in names and not probe.loaded:
                if "skill_list" in names and not probe.searched:
                    probe.searched = True
                    return {"tool_calls": [("skill_list", {})]}
                probe.loaded = True
                return {"tool_calls": [("skill_load", self.schema_arguments(body, "skill_load", view))]}
            return {"content": "Reminder: invoice INV-2026-0105 for 155.00 USD is overdue. Reminder complete."}
        if behavior == "approval-escalator":
            if probe.asked == 0:
                probe.asked = 1
                return {"tool_calls": [("human_request", {"kind": "APPROVAL_REQUIRED",
                                                          "prompt": "Approve purging customer records inactive since 2020?"})]}
            return {"content": "Escalated: the purge was not approved, so no records were purged."}
        if behavior == "endless-lookup":
            probe.lookups += 1
            return {"tool_calls": [("reference_lookup", {"page": probe.lookups})]}
        raise AssertionError(f"unknown fixture agent {behavior}")

    @staticmethod
    def search_result(view, kind):
        """The first capability_search result object of a kind, from the delivered text."""
        for match in re.finditer(r'\{[^{}]*"kind"\s*:\s*"' + kind + r'"[^{}]*\}', view):
            try:
                return json.loads(match.group(0))
            except ValueError:
                continue
        return {}

    @staticmethod
    def schema_arguments(body, name, view):
        """Fill a tool's declared properties from the first delivered object that has them."""
        schema = next((tool["function"].get("parameters", {}) for tool in body.get("tools", [])
                       if tool.get("function", {}).get("name") == name), {})
        properties = list(schema.get("properties", {}))
        for match in re.finditer(r"\{[^{}]*\}", view):
            try:
                candidate = json.loads(match.group(0))
            except ValueError:
                continue
            if properties and all(key in candidate for key in schema.get("required", properties[:1])):
                return {key: candidate[key] for key in properties if key in candidate}
        return {}

    @staticmethod
    def load_arguments(body, view, alias):
        """Build capability_load arguments from its declared schema and the search result."""
        schema = next((tool["function"].get("parameters", {}) for tool in body.get("tools", [])
                       if tool.get("function", {}).get("name") == "capability_load"), {})
        properties = schema.get("properties", {})
        found = None
        for match in re.finditer(r'\{[^{}]*"' + re.escape(alias) + r'"[^{}]*\}', view):
            try:
                found = json.loads(match.group(0))
            except ValueError:
                continue
        arguments = {}
        for name in properties:
            if found and name in found:
                arguments[name] = found[name]
        if not arguments:
            arguments = {name: alias for name in properties if name in ("alias", "name", "id", "capability")}
        return arguments

    @staticmethod
    def overdue_report(view):
        block = re.search(r"```csv\n(.*?)```", view, re.S)
        if not block:
            return "invoices.csv was not provided."
        lines = block.group(1).strip().splitlines()
        header = lines[0].split(",")
        overdue = {}
        for line in lines[1:]:
            row = dict(zip(header, line.split(","), strict=False))
            if row["status"] == "unpaid" and row["due_date"] < "2026-10-10":
                customer = overdue.setdefault(row["customer"], {})
                customer[row["currency"]] = customer.get(row["currency"], 0) + int(row["amount_cents"])
        value = {"as_of": "2026-10-10", "overdue": overdue}
        return f"```json\n{json.dumps(value, ensure_ascii=False)}\n```\n\nReport complete."

    # --------------------------------------------------------- embeddings
    def embed(self, body):
        """Deterministic 8-dimension hash vectors; they carry no semantic similarity."""
        inputs = body.get("input")
        inputs = [inputs] if isinstance(inputs, str) else list(inputs or [])
        data = []
        for index, text in enumerate(inputs):
            digest = hashlib.sha256(str(text).encode()).digest()
            vector = [(byte - 127.5) / 127.5 for byte in digest[:8]]
            norm = math.sqrt(sum(value * value for value in vector)) or 1.0
            data.append({"object": "embedding", "index": index, "embedding": [value / norm for value in vector]})
        used = sum(tokens(str(text)) for text in inputs)
        return 200, {"object": "list", "data": data, "model": body.get("model"),
                     "usage": {"prompt_tokens": used, "total_tokens": used}}

    @staticmethod
    def strings(body):
        output = []
        collect_strings(body, output)
        return output

    # --------------------------------------------------------------- Jev
    def jev(self, body):
        view = "\n".join(text for text in self.strings(body))
        with self.lock:
            for task_uuid in self.probes:
                if task_uuid in view:
                    self.jev_requests[task_uuid] = self.jev_requests.get(task_uuid, 0) + 1
        questions = body.get("questions") or {}
        return 200, {"answers": {name: {"type": "noul", "noul": 0.0} for name in questions}}

    # ------------------------------------------------------------- tools
    def tool(self, task, alias, arguments):
        with self.lock:
            self.tool_calls.setdefault(task["id"], []).append({"alias": alias, "arguments": arguments})
        if alias == "reference_lookup":
            page = int(arguments.get("page", 0))
            correction = (task.get("reference_corrections") or {}).get(str(page), "")
            size = task.get("reference_page_bytes", 400)
            filler = (f"Archive page {page}: vendor catalogue entries, delivery terms and historical quotes. "
                      * (size // 80 + 1))[:size]
            return 200, {"page": page, "content": (correction + " " if correction else "") + filler}
        if alias == "invoice_lookup":
            return 200, {"invoice": arguments.get("invoice_number"), "amount_cents": 15500, "currency": "USD",
                         "due_date": "2026-10-01", "payment_status": "unpaid", "payer": "Aster Labs"}
        return 200, {"alias": alias, "result": "fixture"}

    # ------------------------------------------------------------ server
    def handler(self, tasks_by_id):
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def reply(self, status, value):
                payload = (value if isinstance(value, str) else json.dumps(value)).encode()
                try:
                    self.send_response(status)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except (BrokenPipeError, ConnectionResetError):
                    pass

            def do_GET(self):
                self.reply(405, {"error": "method not allowed"})

            def do_POST(self):
                try:
                    body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
                    if self.path == "/v1/chat/completions":
                        return self.reply(*fixture.infer(body))
                    if self.path == "/v1/embeddings":
                        return self.reply(*fixture.embed(body))
                    if self.path == "/jev":
                        return self.reply(*fixture.jev(body))
                    match = re.fullmatch(r"/tools/([a-z0-9-]+)/([a-z_]+)", self.path)
                    if match and match.group(1) in tasks_by_id:
                        return self.reply(*fixture.tool(tasks_by_id[match.group(1)], match.group(2), body))
                    self.reply(404, {"error": "unknown fixture path"})
                except Exception as error:  # Report fixture faults; never hide them as model output.
                    with fixture.lock:
                        fixture.errors.append(f"{self.path}: {type(error).__name__}: {error}")
                    self.reply(500, {"error": "fixture failure"})

        return Handler

    def serve(self, tasks_by_id):
        server = ThreadingHTTPServer(("127.0.0.1", 0), self.handler(tasks_by_id))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        return server
