#!/usr/bin/env python3
"""Single-node, real-model prompt-cache trial for procurement-001 (stdlib only).

One `aidash serve` node, isolated PostgreSQL/NATS (unique Compose project and
ports), a recording proxy in front of OpenRouter, four deterministic HTTP
facts tools and the procurement-001 oracle. Each variant changes only the
Agent/model fields under test; fields are sent only when non-default, so
binaries that predate a field never receive it.

Raw request/response traces contain full prompts. Write them outside the
repository (`--out`) and publish only the compact `--results` file.
"""
import argparse
import datetime
import hashlib
import http.server
import itertools
import json
import os
from pathlib import Path
import secrets
import shlex
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[3]
OPENROUTER = "https://openrouter.ai/api/v1"
CASE_ID = "procurement-001"
ROLES = ["procurement", "engineering", "transport", "production"]
TERMINAL = ("COMPLETED", "FAILED", "CANCELLED", "BLOCKED")
SALT_PREFIX = "Cache scope: k"
DEFAULT_TOOLS = [
    "workspace_observe", "workspace_wait", "skill_list", "skill_load", "skill_read",
    "file_search", "file_read", "task_create", "task_delegate", "agent_discover",
    "artifact_publish", "workspace_message", "memory_mutate", "memory_recall", "memory_reflect",
]
APPROVAL_PREFIX = "Approve this exact external tool action once? Tool: "

DATA = {
    "procurement": {
        "evidence_id": "PROC-001", "currency": "JPY", "quantity": 100,
        "rules": "Choose exactly one sensor supplier for all 100 kits. No split orders. Each kit needs one sensor and one baseboard. All prices are tax-inclusive and fixed; no unlisted charges or discounts.",
        "suppliers": [
            {"id": "A", "sensor": "S", "stock": 60, "unit_price": 1800, "dispatch_day": 0},
            {"id": "B", "sensor": "S", "stock": 100, "unit_price": 2400, "dispatch_day": 1},
            {"id": "C", "sensor": "T", "stock": 100, "unit_price": 1700, "dispatch_day": 1},
            {"id": "D", "sensor": "T", "stock": 100, "unit_price": 1400, "dispatch_day": 3},
        ],
        "baseboards": {"stock": 100, "unit_price": 1200, "location": "assembly plant", "available_day": 0},
    },
    "engineering": {
        "evidence_id": "ENG-001",
        "compatibility": [
            {"sensor": "S", "compatible": True, "adapter_per_kit": 0, "production_type": "standard"},
            {"sensor": "T", "compatible": True, "adapter_per_kit": 1, "production_type": "adapted"},
        ],
        "adapter": {"stock": 100, "unit_price": 300, "location": "assembly plant", "available_day": 0},
        "rules": "T requires one adapter per kit and an adapted assembly slot. S uses a standard slot. Both meet the target specification when these rules are followed.",
    },
    "transport": {
        "evidence_id": "SHIP-001",
        "inbound": [{"service": "normal", "days": 2, "cost": 10000}, {"service": "express", "days": 1, "cost": 30000}],
        "outbound": [{"service": "normal", "days": 2, "cost": 15000}, {"service": "express", "days": 1, "cost": 40000}],
        "rules": "Inbound fees cover the complete 100-sensor order from any supplier to the plant. Arrival is at the start of dispatch_day + days. Outbound fees cover all 100 completed kits to X; delivery is at the end of assembly_day + days. Baseboards and adapters are already at the plant and need no shipping. No other routes exist.",
    },
    "production": {
        "evidence_id": "BUILD-001",
        "slots": [
            {"id": "STD-3", "production_type": "standard", "day": 3, "capacity": 100, "cost": 60000},
            {"id": "STD-4", "production_type": "standard", "day": 4, "capacity": 100, "cost": 50000},
            {"id": "ALT-4", "production_type": "adapted", "day": 4, "capacity": 100, "cost": 80000},
        ],
        "rules": "All parts must arrive by the START of the slot day; arrival on that day is allowed. All 100 kits finish at the END of that day. Each listed slot handles the whole order; fees are per order. No other slots exist.",
    },
}
OUTPUT_SCHEMA = {
    "supplier": "A|B|C|D", "sensor": "S|T", "quantity": 100,
    "adapter_quantity": "integer", "inbound_service": "normal|express",
    "inbound_arrival_day": "integer", "assembly_slot": "slot id",
    "outbound_service": "normal|express", "delivery_day": "integer",
    "costs": {"sensors": "integer", "baseboards": "integer", "adapters": "integer", "inbound": "integer", "assembly": "integer", "outbound": "integer"},
    "total_cost": "integer", "evidence_ids": ["tool result evidence IDs"],
    "explanation": "short rationale including why cheaper alternatives fail",
}
GOAL = ("Produce 100 specification-compliant sensor kits and deliver all of them to X by the END of day 5, for at most JPY 600000. "
        "Find the MINIMUM TOTAL COST feasible plan using the four facts tools. Time is expressed as integer business-day indices; day 0 is now. "
        "Select ONE sensor supplier for all 100 kits; split orders are forbidden. This is a read-only planning exercise, do not place orders. "
        "Do not invent data or tool evidence. If genuinely infeasible, return that conclusion with evidence. Otherwise your FINAL response must be "
        "ONLY a JSON object matching these fields (replace description strings with actual values): " + json.dumps(OUTPUT_SCHEMA))
INSTRUCTIONS = ("You are the procurement planner for benchmark procurement-001. Work in exactly five responses. "
                "In each of your first four responses call exactly one tool, with arguments {}, in this fixed order: "
                "procurement_facts, engineering_facts, transport_facts, production_facts. Never call more than one tool in a response "
                "and never call any other tool. After the fourth tool result, enumerate the feasible combinations yourself and reply "
                "with ONLY the final JSON object requested by the task: no tool call, no Markdown and no other text. "
                "Use only the tool data; do not invent facts. This is a read-only planning exercise and needs no human approval.")


def oracle():
    plans = []
    for supplier, inbound, slot, outbound in itertools.product(DATA["procurement"]["suppliers"], DATA["transport"]["inbound"], DATA["production"]["slots"], DATA["transport"]["outbound"]):
        eng = next(x for x in DATA["engineering"]["compatibility"] if x["sensor"] == supplier["sensor"])
        arrival = supplier["dispatch_day"] + inbound["days"]
        delivery = slot["day"] + outbound["days"]
        if supplier["stock"] < 100 or slot["capacity"] < 100 or slot["production_type"] != eng["production_type"] or arrival > slot["day"] or delivery > 5:
            continue
        costs = dict(sensors=100 * supplier["unit_price"], baseboards=120000, adapters=100 * eng["adapter_per_kit"] * 300, inbound=inbound["cost"], assembly=slot["cost"], outbound=outbound["cost"])
        total = sum(costs.values())
        if total <= 600000:
            plans.append(dict(supplier=supplier["id"], sensor=supplier["sensor"], quantity=100, adapter_quantity=100 * eng["adapter_per_kit"], inbound_service=inbound["service"], inbound_arrival_day=arrival, assembly_slot=slot["id"], outbound_service=outbound["service"], delivery_day=delivery, costs=costs, total_cost=total))
    plans.sort(key=lambda x: x["total_cost"])
    assert plans[0]["total_cost"] == 440000 and plans[1]["total_cost"] == 445000
    return plans


def utc():
    return datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")


def write_json(path, value):
    Path(path).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def fetch(url, timeout=60):
    request = urllib.request.Request(url, headers={"User-Agent": "aidash-prompt-cache-evidence/1"})
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.load(response)


def number(value):
    return value if isinstance(value, (int, float)) and not isinstance(value, bool) else None


def total(values):
    """Sum when every value is known; one unknown value makes the sum unknown."""
    values = list(values)
    return sum(values) if values and all(v is not None for v in values) else (0 if not values else None)


class Ledger:
    """Per-route spend from recorded `usage.cost`, persisted across invocations."""

    def __init__(self, path, route, ceiling):
        self.path = Path(path)
        self.lock = threading.Lock()
        self.state = json.loads(self.path.read_text()) if self.path.exists() else dict(route=route, ceiling_usd=ceiling, spent_usd=0.0, priced_calls=0, unpriced_success_calls=0, max_call_cost_usd=0.0, max_run_cost_usd=0.0)
        if self.state["route"] != route:
            raise SystemExit(f"ledger {self.path} belongs to route {self.state['route']}")
        self.state["ceiling_usd"] = ceiling

    def save(self):
        write_json(self.path, self.state)

    def call_refusal(self):
        s = self.state
        if s["unpriced_success_calls"]:
            return "a successful call reported no usage.cost; the ceiling cannot be enforced"
        if s["spent_usd"] + s["max_call_cost_usd"] > s["ceiling_usd"]:
            return "route cost ceiling reached"
        return None

    def run_refusal(self):
        s = self.state
        return self.call_refusal() or ("route cost ceiling would be exceeded by another run" if s["spent_usd"] + s["max_run_cost_usd"] > s["ceiling_usd"] else None)

    def record(self, status, response):
        cost = number((response.get("usage") or {}).get("cost")) if isinstance(response, dict) else None
        with self.lock:
            if cost is not None:
                self.state["spent_usd"] += cost
                self.state["priced_calls"] += 1
                self.state["max_call_cost_usd"] = max(self.state["max_call_cost_usd"], cost)
            elif status == 200:
                self.state["unpriced_success_calls"] += 1
            self.save()

    def record_run(self, cost):
        with self.lock:
            if cost is not None:
                self.state["max_run_cost_usd"] = max(self.state["max_run_cost_usd"], cost)
            self.save()


class Recorder:
    """Recording OpenRouter proxy plus deterministic facts tools."""

    def __init__(self, out, model, ledger, max_calls_per_run, max_calls):
        self.out = out
        self.model = model
        self.ledger = ledger
        self.max_calls_per_run = max_calls_per_run
        self.max_calls = max_calls
        self.lock = threading.Lock()
        self.calls = []
        self.tools = []
        self.refusals = []
        self.started = 0
        self.run = None
        self.run_started = 0
        self.tool_secret = secrets.token_hex(24)
        self.proxy_secret = secrets.token_hex(24)

    def begin(self, run):
        with self.lock:
            self.run = run
            self.run_started = 0

    def append(self, kind, row):
        with self.lock:
            getattr(self, kind).append(row)
            with (self.out / (kind + ".jsonl")).open("a") as f:
                f.write(json.dumps(row, ensure_ascii=False) + "\n")

    def handler(self):
        rec = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def respond(self, code, body):
                encoded = json.dumps(body, ensure_ascii=False).encode()
                self.send_response(code)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(encoded)))
                self.end_headers()
                try:
                    self.wfile.write(encoded)
                except (BrokenPipeError, ConnectionResetError):
                    pass

            def refuse(self, code, reason, body=None):
                rec.append("refusals", dict(time=time.time(), run=rec.run, reason=reason, model=(body or {}).get("model")))
                return self.respond(code, {"error": {"message": "evidence proxy: " + reason, "code": code}})

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                if self.path.startswith("/tools/"):
                    role = self.path.split("/")[2]
                    if role not in DATA or self.headers.get("Authorization") != "Bearer " + rec.tool_secret:
                        return self.respond(403, {"error": "forbidden"})
                    rec.append("tools", dict(time=time.time(), run=rec.run, role=role, request=body, idempotency_key=self.headers.get("Idempotency-Key")))
                    return self.respond(200, DATA[role])
                if not self.path.endswith("/chat/completions"):
                    return self.respond(404, {"error": "not found"})
                if self.headers.get("Authorization") != "Bearer " + rec.proxy_secret:
                    return self.respond(403, {"error": "forbidden"})
                if body.get("model") != rec.model:
                    return self.refuse(400, "unexpected model", body)
                if (body.get("provider") or {}).get("zdr") is not True:
                    return self.refuse(400, "provider.zdr is not true", body)
                with rec.lock:
                    refusal = rec.ledger.call_refusal()
                    if refusal is None and rec.started >= rec.max_calls:
                        refusal = "invocation inference call cap reached"
                    if refusal is None and rec.run_started >= rec.max_calls_per_run:
                        refusal = "per-run inference call cap reached"
                    if refusal is None:
                        rec.started += 1
                        rec.run_started += 1
                        index, run, run_index = rec.started, rec.run, rec.run_started
                if refusal:
                    return self.refuse(429, refusal, body)
                start = time.time()
                # The upstream credential is attached here only; neither it nor
                # any Authorization header is ever recorded.
                request = urllib.request.Request(OPENROUTER + "/chat/completions", data=json.dumps(body).encode(), headers={"Authorization": "Bearer " + os.environ["OPENROUTER_API_KEY"], "Content-Type": "application/json"})
                try:
                    with urllib.request.urlopen(request, timeout=300) as response:
                        status, result = response.status, json.load(response)
                except urllib.error.HTTPError as error:
                    status = error.code
                    try:
                        result = json.loads(error.read())
                    except Exception:
                        result = {"error": {"message": "unreadable upstream HTTP failure"}}
                except Exception as error:
                    status, result = 502, {"error": {"message": type(error).__name__}}
                if status == 200 and isinstance(result, dict) and result.get("error"):
                    # OpenRouter can report mid-stream provider errors in a 200 body.
                    status = 502
                rec.ledger.record(status, result)
                rec.append("calls", dict(index=index, run=run, run_call=run_index, start=start, seconds=time.time() - start, status=status, request=body, response=result))
                return self.respond(status, result)

        return Handler


def entity(kind, identifier, config, capability, description, schema=None):
    return dict(id=identifier, version="1.0.0", kind=kind, name={"en": identifier}, description={"en": description}, capabilities=[capability], tags=["prompt-cache-evidence"], languages=["en"], skills=[], schema=schema or {}, config=config)


def parse_variant(spec, defaults):
    label, *pairs = shlex.split(spec)
    variant = dict(defaults, label=label)
    for pair in pairs:
        key, _, value = pair.partition("=")
        key = key.replace("-", "_")
        if key not in ("projection", "projection_versions", "model_cache_mode", "prompt_cache"):
            raise SystemExit(f"unknown variant key {key}")
        variant[key] = [v for v in value.replace("+", ",").split(",") if v] if key == "projection_versions" else value
    if variant["projection"] not in ("legacy", "ordered") or variant["model_cache_mode"] not in ("none", "automatic", "explicit") or variant["prompt_cache"] not in ("off", "explicit"):
        raise SystemExit(f"invalid variant {spec}")
    return variant


# Recorded-request checks --------------------------------------------------

def count_cache_control(value):
    if isinstance(value, dict):
        return ("cache_control" in value) + sum(count_cache_control(v) for v in value.values())
    if isinstance(value, list):
        return sum(count_cache_control(v) for v in value)
    return 0


def request_shape(body, variant):
    messages = body.get("messages", [])
    system = messages[0]["content"] if messages and messages[0].get("role") == "system" else None
    system_text = system if isinstance(system, str) else (system[0].get("text", "") if isinstance(system, list) and system else "")
    system_marked = isinstance(system, list) and len(system) == 1 and "cache_control" in system[0]
    user = messages[1]["content"] if len(messages) > 1 and messages[1].get("role") == "user" else None
    parts = user if isinstance(user, list) else []
    marked = [i for i, part in enumerate(parts) if isinstance(part, dict) and "cache_control" in part]
    markers = count_cache_control(body)
    shape = dict(messages=len(messages), user_parts=len(parts) if isinstance(user, list) else ("string" if isinstance(user, str) else None), cache_control_markers=markers, system_cache_control=system_marked, user_cache_control_parts=marked, salt_line=system_text.startswith(SALT_PREFIX), tools=len(body.get("tools") or []))
    if variant["prompt_cache"] == "explicit":
        breakpoints_ok = system_marked and len(parts) >= 2 and marked == [len(parts) - 2] and markers == 2
    else:
        breakpoints_ok = markers == 0
    shape["breakpoints_as_expected"] = breakpoints_ok
    shape["salt_as_expected"] = shape["salt_line"] == (variant["projection"] == "ordered")
    return shape


def call_metrics(call, variant):
    response = call["response"] if isinstance(call["response"], dict) else {}
    usage = response.get("usage") or {}
    details = usage.get("prompt_tokens_details") or {}
    completion = usage.get("completion_tokens_details") or {}
    choice = (response.get("choices") or [{}])[0]
    message = choice.get("message") or {}
    tool_calls = message.get("tool_calls") or []
    return dict(
        run_call=call["run_call"], status=call["status"], seconds=round(call["seconds"], 3),
        provider=response.get("provider"), response_model=response.get("model"),
        prompt_tokens=number(usage.get("prompt_tokens")), completion_tokens=number(usage.get("completion_tokens")),
        reasoning_tokens=number(completion.get("reasoning_tokens")),
        cached_tokens=number(details.get("cached_tokens")), cache_write_tokens=number(details.get("cache_write_tokens")),
        cost_usd=number(usage.get("cost")), finish_reason=choice.get("finish_reason"),
        tool_calls=[(c.get("function") or {}).get("name") for c in tool_calls],
        request=request_shape(call["request"], variant),
    )


def summarize_calls(metrics):
    later = metrics[1:]
    prompt_later = total(m["prompt_tokens"] for m in later)
    cached_later = total(m["cached_tokens"] for m in later)
    return dict(
        calls=len(metrics),
        prompt_tokens=total(m["prompt_tokens"] for m in metrics),
        completion_tokens=total(m["completion_tokens"] for m in metrics),
        reasoning_tokens=total(m["reasoning_tokens"] for m in metrics),
        cached_tokens=total(m["cached_tokens"] for m in metrics),
        cache_write_tokens=total(m["cache_write_tokens"] for m in metrics),
        cost_usd=total(m["cost_usd"] for m in metrics),
        prompt_tokens_calls_2_n=prompt_later,
        cached_tokens_calls_2_n=cached_later,
        cached_share_calls_2_n=(cached_later / prompt_later) if prompt_later and cached_later is not None else None,
        breakpoints_as_expected=all(m["request"]["breakpoints_as_expected"] for m in metrics) if metrics else None,
        salt_as_expected=all(m["request"]["salt_as_expected"] for m in metrics) if metrics else None,
    )


def evaluate(snapshot, task_id, tools, expected):
    finals = [a for a in snapshot.get("artifacts", []) if a.get("task_id") == task_id]
    final = next((a for a in reversed(finals) if a.get("name") == "Final response"), finals[-1] if finals else None)
    result = None
    if final:
        content = final.get("content")
        if isinstance(content, dict):
            result = content
        elif isinstance(content, str):
            try:
                result = json.loads(content.strip().removeprefix("```json").removeprefix("```").removesuffix("```").strip())
            except ValueError:
                pass
    checks = {"parseable_final_json": isinstance(result, dict)}
    if isinstance(result, dict):
        for key, value in expected.items():
            checks[key] = result.get(key) == value
        checks["all_domain_evidence"] = set(result.get("evidence_ids") or []) == {x["evidence_id"] for x in DATA.values()}
    checks["all_facts_tools_called"] = {x["role"] for x in tools} == set(ROLES)
    return dict(passed=all(checks.values()), checks=checks, result=result)


def aggregate(variant, runs):
    attempted = [r for r in runs if r["variant"] == variant["label"]]
    completed = [r for r in attempted if r["result"] in ("passed", "failed")]
    passed = [r for r in attempted if r["result"] == "passed"]
    metrics = [m for r in completed for m in r["calls"]]
    later = [m for r in completed for m in r["calls"][1:]]
    summary = summarize_calls(metrics)
    prompt_later = total(m["prompt_tokens"] for m in later)
    cached_later = total(m["cached_tokens"] for m in later)
    summary.update(prompt_tokens_calls_2_n=prompt_later, cached_tokens_calls_2_n=cached_later, cached_share_calls_2_n=(cached_later / prompt_later) if prompt_later and cached_later is not None else None)
    costs = [r["totals"]["cost_usd"] for r in completed]
    return dict(
        variant=variant, runs_attempted=len(attempted), runs_completed=len(completed), runs_passed=len(passed),
        pass_rate_attempted=(len(passed) / len(attempted)) if attempted else None,
        outcomes=[r["result"] for r in attempted],
        totals=summary,
        mean_per_run=dict(
            prompt_tokens=summary["prompt_tokens"] / len(completed) if completed and summary["prompt_tokens"] is not None else None,
            cached_tokens=summary["cached_tokens"] / len(completed) if completed and summary["cached_tokens"] is not None else None,
            cost_usd=total(costs) / len(completed) if completed and total(costs) is not None else None,
            inference_calls=len(metrics) / len(completed) if completed else None,
            elapsed_seconds=sum(r["elapsed_seconds"] for r in completed) / len(completed) if completed else None,
        ),
        spend_including_failed_runs_usd=total(r["totals"]["cost_usd"] for r in attempted if r.get("totals")),
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--prepare-only", action="store_true", help="write case.json/oracle.json next to this script and exit")
    parser.add_argument("--binary", help="prebuilt aidash executable (this script builds nothing)")
    parser.add_argument("--source-root", default=str(REPO), help="worktree the binary was built from (migrations, compose.yaml)")
    parser.add_argument("--out", help="raw evidence directory outside the repository")
    parser.add_argument("--route", default="route", help="route label; the cost ledger is per route")
    parser.add_argument("--model", help="OpenRouter model slug")
    parser.add_argument("--reasoning-effort", help="omitted means model default")
    parser.add_argument("--projection", default="legacy", choices=["legacy", "ordered"])
    parser.add_argument("--projection-versions", default="", help="comma list; omitted means the binary default (legacy only)")
    parser.add_argument("--model-cache-mode", default="none", choices=["none", "automatic", "explicit"])
    parser.add_argument("--prompt-cache", default="off", choices=["off", "explicit"])
    parser.add_argument("--variant", action="append", default=[], help="'LABEL key=value ...' with keys projection, projection_versions (a+b), model_cache_mode, prompt_cache; overrides the flags above. Repeat for interleaved variants")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--cost-ceiling-usd", type=float, default=5.0)
    parser.add_argument("--ledger", help="spend ledger (default OUT/ledger-ROUTE.json)")
    parser.add_argument("--max-calls-per-run", type=int, default=12)
    parser.add_argument("--max-calls", type=int, default=200)
    parser.add_argument("--run-timeout", type=int, default=600)
    parser.add_argument("--max-steps", type=int, default=12)
    parser.add_argument("--prompt-cache-key-version", help="AIDASH_PROMPT_CACHE_KEY_VERSION; omitted means the binary default")
    parser.add_argument("--project-name", help="Compose project (default unique)")
    parser.add_argument("--results", help="write the compact, prompt-free results JSON here")
    parser.add_argument("--keep-infra", action="store_true", help="leave containers running")
    args = parser.parse_args()

    plans = oracle()
    write_json(HERE / "case.json", dict(id=CASE_ID, goal=GOAL, instructions=INSTRUCTIONS, facts=DATA, output_schema=OUTPUT_SCHEMA))
    write_json(HERE / "oracle.json", dict(feasible_plans=plans, optimum=plans[0]))
    if args.prepare_only:
        print(json.dumps(dict(feasible_plans=len(plans), optimum=plans[0])))
        return
    for name in ("binary", "out", "model"):
        if not getattr(args, name):
            parser.error(f"--{name.replace('_', '-')} is required")
    if not os.environ.get("OPENROUTER_API_KEY"):
        raise SystemExit("OPENROUTER_API_KEY is required")
    defaults = dict(projection=args.projection, projection_versions=[v for v in args.projection_versions.split(",") if v], model_cache_mode=args.model_cache_mode, prompt_cache=args.prompt_cache)
    variants = [parse_variant(spec, defaults) for spec in args.variant] or [dict(defaults, label=args.projection)]
    if len({v["label"] for v in variants}) != len(variants):
        raise SystemExit("variant labels must be unique")
    source_root = Path(args.source_root).resolve()
    binary = Path(args.binary).resolve()
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    experiment = f"{args.route}-{stamp}-{uuid.uuid4().hex[:6]}"
    out = Path(args.out).resolve() / experiment
    (out / "runs").mkdir(parents=True)
    ledger = Ledger(args.ledger or Path(args.out).resolve() / f"ledger-{args.route}.json", args.route, args.cost_ceiling_usd)
    ledger.save()

    catalog = next((m for m in fetch(OPENROUTER + "/models")["data"] if m["id"] == args.model), None)
    zdr = [e for e in fetch(OPENROUTER + "/endpoints/zdr")["data"] if e.get("model_id") == args.model]
    if catalog is None or not zdr:
        raise SystemExit(f"{args.model} is not in the catalog or has no ZDR endpoint")
    pricing = catalog["pricing"]

    def git(*command, raw=False):
        try:
            output = subprocess.check_output(["git", "-C", str(source_root), *command])
            return output if raw else output.decode().strip()
        except Exception:
            return None

    project = args.project_name or f"aidash-promptcache-{args.route}-{uuid.uuid4().hex[:6]}".lower()
    pg_port, nats_port, api_port = free_port(), free_port(), free_port()
    node_id = f"aidash://prompt-cache-{uuid.uuid4().hex[:8]}"
    base = f"http://127.0.0.1:{api_port}"
    database = "evidence_" + uuid.uuid4().hex[:10]
    token = secrets.token_hex(24)
    cache_key = secrets.token_hex(32)
    recorder = Recorder(out, args.model, ledger, args.max_calls_per_run, args.max_calls)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), recorder.handler())
    threading.Thread(target=server.serve_forever, daemon=True).start()
    proxy = f"http://127.0.0.1:{server.server_port}"
    compose = ["docker", "compose", "-p", project, "-f", str(source_root / "compose.yaml")]
    compose_env = {**{k: v for k, v in os.environ.items() if k != "OPENROUTER_API_KEY"}, "COMPOSE_PROJECT_NAME": project, "AIDASH_POSTGRES_PORT": str(pg_port), "AIDASH_NATS_PORT": str(nats_port)}
    setup = dict(
        case=CASE_ID, experiment=experiment, route=args.route, started_at=utc(),
        binary=dict(path=str(binary), sha256=hashlib.sha256(binary.read_bytes()).hexdigest()),
        # Same value as `git diff HEAD | shasum -a 256`; untracked files are listed in status_short.
        source=dict(root=str(source_root), commit=git("rev-parse", "HEAD"), status_short=git("status", "--short"), diff_sha256=hashlib.sha256(git("diff", "HEAD", raw=True) or b"").hexdigest()),
        model=dict(slug=args.model, canonical_slug=catalog.get("canonical_slug"), reasoning_effort=args.reasoning_effort, context_window=catalog.get("context_length"), max_output_tokens=(catalog.get("top_provider") or {}).get("max_completion_tokens"), catalog_pricing=pricing, zdr_endpoints=[dict(tag=e.get("tag"), provider=e.get("provider_name"), pricing=e.get("pricing"), supports_implicit_caching=e.get("supports_implicit_caching")) for e in zdr]),
        variants=variants, repeats=args.repeats, order="interleaved: repeat-major, variants in the given order",
        limits=dict(cost_ceiling_usd=args.cost_ceiling_usd, max_calls_per_run=args.max_calls_per_run, max_calls=args.max_calls, run_timeout_seconds=args.run_timeout, max_steps=args.max_steps),
        infrastructure=dict(compose_project=project, postgres_port=pg_port, nats_port=nats_port, api=base, node_id=node_id, database=database, prompt_cache_key="random 64 hex characters per invocation; not recorded", prompt_cache_key_version=args.prompt_cache_key_version or "binary default"),
        proxy="records request and response bodies; forwards unchanged to OpenRouter; refuses provider.zdr != true, other models, calls beyond caps or the route ceiling",
    )
    write_json(out / "setup.json", setup)
    children, files, runs = [], [], []
    approvals = dict(count=0, errors=[])
    stop = threading.Event()

    def api(path, data=None, timeout=30):
        request = urllib.request.Request(base + path, data=None if data is None else json.dumps(data).encode(), headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            raise RuntimeError(f"{path}: HTTP {error.code}: {error.read().decode()[:1500]}") from None

    def approve_facts_calls():
        allowed = tuple(f"{APPROVAL_PREFIX}{role}-facts@1.0.0;" for role in ROLES)
        while not stop.wait(0.3):
            try:
                for request in api("/api/state").get("human_requests", []):
                    if request.get("response") is None and request.get("kind") == "APPROVAL_REQUIRED" and request.get("prompt", "").startswith(allowed):
                        api(f"/api/human-requests/{request['id']}/answer", {"approved": True})
                        approvals["count"] += 1
            except Exception as error:
                approvals["errors"].append(str(error)[:300])

    def log(message):
        print(f"[{utc()}] {message}", flush=True)

    status = "infrastructure_error"
    try:
        log(f"starting infrastructure {project}")
        subprocess.run([*compose, "up", "-d", "--wait", "postgres", "nats"], env=compose_env, check=True, stdout=subprocess.DEVNULL)
        subprocess.run([*compose, "exec", "-T", "postgres", "psql", "-U", "aidash", "-d", "aidash_a", "-v", "ON_ERROR_STOP=1", "-c", f"CREATE DATABASE {database} TEMPLATE template0"], env=compose_env, check=True, stdout=subprocess.DEVNULL)
        env = {k: v for k, v in os.environ.items() if not k.startswith("AIDASH_") and k not in ("OPENROUTER_API_KEY", "DATABASE_URL", "NATS_URL")}
        env.update(DATABASE_URL=f"postgres://aidash:aidash-local@127.0.0.1:{pg_port}/{database}", NATS_URL=f"nats://127.0.0.1:{nats_port}", AIDASH_NODE_ID=node_id, AIDASH_ENDPOINT=base, AIDASH_LISTEN=f"127.0.0.1:{api_port}", AIDASH_API_TOKEN=token, AIDASH_API_RATE_BURST="1000", AIDASH_ACTIVATION_BOOTSTRAP="true", AIDASH_SECRET_BENCH_MODEL=recorder.proxy_secret, AIDASH_SECRET_BENCH_TOOL=recorder.tool_secret, AIDASH_PROMPT_CACHE_KEY=cache_key, RUST_LOG="info")
        if args.prompt_cache_key_version:
            env["AIDASH_PROMPT_CACHE_KEY_VERSION"] = args.prompt_cache_key_version
        migrate_log = (out / "migrate.log").open("w")
        files.append(migrate_log)
        subprocess.run([str(binary), "migrate", "--database", env["DATABASE_URL"], "--migrations-dir", str(source_root / "server/migrations")], cwd=out, env=env, check=True, stdout=migrate_log, stderr=migrate_log)
        node_log = (out / "node.log").open("w")
        files.append(node_log)
        children.append(subprocess.Popen([str(binary), "serve"], cwd=out, env=env, stdout=node_log, stderr=node_log))
        deadline = time.monotonic() + 120
        while True:
            try:
                api("/health", timeout=5)
                break
            except Exception as error:
                if time.monotonic() > deadline or children[0].poll() is not None:
                    raise RuntimeError("node startup failed; see node.log") from error
                time.sleep(0.5)
        log("node healthy; registering")
        for role in ROLES:
            api("/api/registry", entity("tool", f"{role}-facts", dict(registry_node=node_id, provider="integration.http@1", operation="invoke", default_alias=f"{role}_facts", tier="integration", transport=dict(transport="http", endpoint=f"{proxy}/tools/{role}", credential_env="AIDASH_SECRET_BENCH_TOOL", replay="read_only")), "benchmark.facts", f"Read the complete authoritative {role} data for procurement-001, including its evidence ID. Call with {{}}. Read-only.", schema={"type": "object", "properties": {}, "additionalProperties": False}))
        cost = dict(currency="USD", input_per_million=float(pricing["prompt"]) * 1e6, output_per_million=float(pricing["completion"]) * 1e6)
        registered = {}
        for variant in variants:
            label = variant["label"].lower()
            model = dict(provider="openrouter", model_id=args.model, endpoint=proxy + "/v1", credential_env="AIDASH_SECRET_BENCH_MODEL", context_window=catalog["context_length"], max_output_tokens=catalog["top_provider"]["max_completion_tokens"], modalities=["text"], cost=cost)
            if args.reasoning_effort:
                model["reasoning_effort"] = args.reasoning_effort
            if variant["projection_versions"]:
                model["projection_versions"] = variant["projection_versions"]
            if variant["model_cache_mode"] != "none":
                model["cache_mode"] = variant["model_cache_mode"]
            agent = dict(schema_version=1, model=dict(id=f"model-{label}", version="1.0.0"), instructions=INSTRUCTIONS, bindings=[dict(kind="tool", target=dict(registry_node=node_id, id=f"{role}-facts", version="1.0.0"), alias=f"{role}_facts", narrow={}) for role in ROLES], remove_default=DEFAULT_TOOLS, cluster=None, max_steps=args.max_steps)
            if variant["projection"] != "legacy":
                agent["projection_version"] = variant["projection"]
            if variant["prompt_cache"] != "off":
                agent["prompt_cache"] = variant["prompt_cache"]
            api("/api/registry", entity("model", f"model-{label}", model, "inference", f"{args.model} for variant {variant['label']}"))
            api("/api/registry", entity("agent", f"planner-{label}", agent, "benchmark.procurement", f"procurement-001 planner, variant {variant['label']}"))
            registered[variant["label"]] = dict(model_config={k: v for k, v in model.items() if k not in ("endpoint", "credential_env")}, agent_config={k: v for k, v in agent.items() if k != "instructions"})
        write_json(out / "registered.json", registered)
        threading.Thread(target=approve_facts_calls, daemon=True).start()
        schedule = [(repeat, variant) for repeat in range(1, args.repeats + 1) for variant in variants]
        for repeat, variant in schedule:
            name = f"{variant['label']}-r{repeat}"
            refusal = ledger.run_refusal()
            if refusal:
                runs.append(dict(name=name, variant=variant["label"], repeat=repeat, result="not_attempted", reason=refusal))
                log(f"{name}: not attempted ({refusal})")
                continue
            run_dir = out / "runs" / name
            run_dir.mkdir()
            recorder.begin(name)
            tools_before, calls_before = len(recorder.tools), len(recorder.calls)
            report = dict(name=name, variant=variant["label"], repeat=repeat, started_at=utc())
            started = time.monotonic()
            try:
                conversation = api("/api/conversations", dict(title=f"{CASE_ID} {name}", goal=GOAL, target=dict(id=f"planner-{variant['label'].lower()}", version="1.0.0"), target_kind="agent"))
                workspace, task_id = conversation["workspace"]["id"], conversation["task"]["id"]
                report.update(workspace_id=workspace, root_task_id=task_id)
                root, snapshot = None, None
                while time.monotonic() - started < args.run_timeout:
                    snapshot = api(f"/api/workspaces/{workspace}")
                    root = next(t for t in snapshot["tasks"] if t["id"] == task_id)
                    if root["status"] in TERMINAL:
                        break
                    time.sleep(1)
                report.update(elapsed_seconds=round(time.monotonic() - started, 1), root_status=root["status"] if root else None, timed_out=root is None or root["status"] not in TERMINAL)
                write_json(run_dir / "workspace.json", snapshot)
                state = api("/api/state")
                task_runs = [r for r in state.get("runs", []) if r.get("task_id") == task_id]
                write_json(run_dir / "runs.json", task_runs)
                # A timed-out Run must not keep calling the model during later runs.
                for stale in task_runs:
                    if stale.get("phase") not in ("COMPLETED", "FAILED", "CANCELLED") and stale.get("control") != "CANCELLED":
                        api(f"/api/runs/{stale['id']}/control", {"action": "cancel"})
                        report.setdefault("cancelled_runs", []).append(stale["id"])
                run_tools = recorder.tools[tools_before:]
                report["evaluation"] = evaluate(snapshot, task_id, run_tools, plans[0])
                report["result"] = "passed" if report["evaluation"]["passed"] and report["root_status"] == "COMPLETED" else "failed"
            except Exception as error:
                report.update(result="infrastructure_error", error=str(error)[:1500], elapsed_seconds=round(time.monotonic() - started, 1))
            time.sleep(0.5)
            # Attribute calls by the root task ID carried in every request context.
            task_id = report.get("root_task_id")
            run_calls = [c for c in recorder.calls[calls_before:] if task_id and task_id in json.dumps(c["request"])]
            report["call_attribution"] = "root task id in request"
            if not run_calls:
                run_calls = [c for c in recorder.calls[calls_before:] if c["run"] == name]
                report["call_attribution"] = "proxy run label"
            report["calls"] = [call_metrics(c, variant) for c in run_calls]
            report["totals"] = summarize_calls(report["calls"])
            report["tool_order"] = [t["role"] for t in recorder.tools[tools_before:]]
            report["protocol"] = dict(fixed_tool_order=report["tool_order"] == ROLES, one_tool_per_response=all(len(m["tool_calls"]) <= 1 for m in report["calls"]), inference_calls=len(run_calls), upstream_errors=sum(1 for c in run_calls if c["status"] != 200))
            report["refusals"] = [r for r in recorder.refusals if r["run"] == name]
            report["finished_at"] = utc()
            ledger.record_run(report["totals"]["cost_usd"])
            write_json(run_dir / "report.json", report)
            runs.append(report)
            log(f"{name}: {report['result']} status={report.get('root_status')} calls={len(run_calls)} cost={report['totals']['cost_usd']} cached_2n={report['totals']['cached_share_calls_2_n']} spent={ledger.state['spent_usd']:.4f}")
        status = "finished"
    except Exception as error:
        setup["error"] = str(error)[:2000]
        log(f"ERROR: {error}")
    finally:
        stop.set()
        for child in children:
            if child.poll() is None:
                child.terminate()
        for child in children:
            try:
                child.wait(timeout=20)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        server.shutdown()
        for f in files:
            f.close()
        if not args.keep_infra:
            subprocess.run([*compose, "down"], env=compose_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        setup.update(status=status, finished_at=utc(), approvals=approvals["count"], approval_errors=approvals["errors"][-5:], infrastructure_stopped=not args.keep_infra, refusals=len(recorder.refusals))
        write_json(out / "setup.json", setup)
        result = dict(
            case=CASE_ID, route=args.route, experiment=experiment, raw_evidence=str(out), started_at=setup["started_at"], finished_at=setup["finished_at"], status=status, error=setup.get("error"),
            binary=setup["binary"], source=setup["source"], model=setup["model"], limits=setup["limits"],
            route_spend=ledger.state,
            variants=[aggregate(v, runs) for v in variants],
            runs=[{k: v for k, v in r.items() if k not in ("evaluation",)} | ({"oracle": {"passed": r["evaluation"]["passed"], "failed_checks": [k for k, ok in r["evaluation"]["checks"].items() if not ok]}} if "evaluation" in r else {}) for r in runs],
        )
        write_json(out / "aggregate.json", result)
        if args.results:
            write_json(args.results, result)
        print(json.dumps(dict(experiment=str(out), status=status, variants=[dict(label=a["variant"]["label"], attempted=a["runs_attempted"], completed=a["runs_completed"], passed=a["runs_passed"], cost=a["totals"]["cost_usd"], cached_share_2n=a["totals"]["cached_share_calls_2_n"]) for a in result["variants"]], spent=ledger.state["spent_usd"])), flush=True)
        if status != "finished":
            sys.exit(1)


if __name__ == "__main__":
    main()
