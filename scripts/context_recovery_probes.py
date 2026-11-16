#!/usr/bin/env python3
"""Generate deterministic, source-linked probes from a Context Journal JSON export."""
import argparse
import json
import re
from pathlib import Path

ASSIGNMENT = re.compile(r'\b([A-Z][A-Z0-9_-]{1,})=([A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?)')
IDENTIFIER = re.compile(r'\b(?:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[A-Z]{2,}(?:-[A-Z0-9]+){2,})\b')


def text_values(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from text_values(item)
    elif isinstance(value, dict):
        for item in value.values():
            yield from text_values(item)


def generate(entries, side_effect_tools=()):
    """Only extract explicit facts; assignment keys use the last value in journal order.

    Entries are {seq,event} records, or flat events in fixture transcripts. The
    caller declares which tool names have side effects; inference is not safe.
    """
    if not isinstance(entries, list):
        raise ValueError("journal must be a list of entries")
    events = []
    for index, entry in enumerate(entries, 1):
        if not isinstance(entry, dict):
            raise ValueError("journal entry must be an object")
        seq = entry.get("seq", index)
        event = entry.get("event", entry)
        if not isinstance(seq, int) or isinstance(seq, bool) or seq < 1 or not isinstance(event, dict):
            raise ValueError("invalid journal sequence or event")
        events.append((seq, event))
    events.sort(key=lambda item: item[0])
    if len({seq for seq, _ in events}) != len(events):
        raise ValueError("duplicate journal sequence")
    constraints, probes = {}, []
    # Distinct excluded values per (key, event), in text order, so two values
    # one event names for the same key get distinguishable questions.
    exclusions = {}
    for seq, event in events:
        kind = event.get("kind")
        # Assistant text may repeat an obsolete value. Only people and tool
        # observations are treated as source facts.
        if kind == "human":
            source = event.get("response")
            for ordinal, text in enumerate(text_values(source), 1):
                if text.strip():
                    probes.append({"type": "person_instruction", "seq": seq,
                                   "question": f"What was person response text {ordinal} at journal event {seq}?",
                                   "answer": text})
        elif kind == "tool":
            source = event.get("result")
            call = event.get("call") or {}
            input_ids = [match.group() for text in text_values(call.get("arguments"))
                         for match in IDENTIFIER.finditer(text)]
            for ordinal, identifier in enumerate(input_ids, 1):
                probes.append({"type": "identifier", "seq": seq,
                               "question": f"What is tool input identifier {ordinal} at journal event {seq}?",
                               "answer": identifier})
            if call.get("name") in side_effect_tools and "result" in event:
                result = json.dumps(source, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
                probes.append({"type": "side_effect_outcome", "seq": seq,
                               "question": f"What exact JSON result did {call['name']} call {call.get('id', seq)} return?",
                               "answer": result})
        else:
            continue
        identifier_index = 0
        for text in text_values(source):
            for match in ASSIGNMENT.finditer(text):
                key, value = match.groups()
                # Exclusions accumulate; scalar constraints are replaced by later corrections.
                slot = (key, value) if key.startswith("EXCLUDE-") else (key, "")
                if slot[1]:
                    named = exclusions.setdefault((key, seq), [])
                    if value not in named:
                        named.append(value)
                    question = (f"Which excluded value is named by occurrence {named.index(value) + 1} "
                                f"of {key} at event {seq}?")
                else:
                    question = f"What is the current value of {key}?"
                constraints[slot] = {"type": "constraint", "seq": seq, "question": question, "answer": value}
            for match in IDENTIFIER.finditer(text):
                identifier = match.group()
                identifier_index += 1
                probes.append({"type": "identifier", "seq": seq,
                               "question": f"What is identifier {identifier_index} at journal event {seq}?",
                               "answer": identifier})
    probes.extend(constraints.values())
    probes.sort(key=lambda p: (p["seq"], p["type"], p["question"], p["answer"]))
    return [{"id": f"p{index:04d}", **probe} for index, probe in enumerate(probes, 1)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("journal", type=Path, help="JSON array of Context Journal entries")
    parser.add_argument("--side-effect-tool", action="append", default=[])
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    probes = generate(json.loads(args.journal.read_text()), args.side_effect_tool)
    output = json.dumps(probes, indent=2, ensure_ascii=False) + "\n"
    if args.output:
        args.output.write_text(output)
    else:
        print(output, end="")


if __name__ == "__main__":
    main()
