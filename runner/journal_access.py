"""Read the current trusted export afresh for each Journal Query."""
import json
from pathlib import Path

JOURNAL_PATH = Path("/journal/journal.jsonl")


def journal():
    with JOURNAL_PATH.open(encoding="utf-8") as source:
        return [json.loads(line) for line in source if line.strip()]
