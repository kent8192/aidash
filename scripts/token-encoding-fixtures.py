#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "tiktoken==0.14.0",
#   "google-genai==2.31.0",
#   "sentencepiece",
#   "protobuf",
#   "transformers",
#   "torch",
#   "torchvision",
#   "pillow",
# ]
# ///
"""Regenerate the Token Encoding fixtures from the reference implementations.

The counts come from tools other than the engine under test (ADR 0017, revised
2026-10-17): OpenAI's `tiktoken` (`o200k_base`, ordinary text) and Google's
`google-genai` `LocalTokenizer` for Gemini 3.5, which Google maps to Gemma 4.
The generated file is committed, so CI needs no Python.

    uv run scripts/token-encoding-fixtures.py

The Gemma 4 tokenizer is downloaded from Hugging Face on first use.
"""
import base64
import json
import random
import warnings
from pathlib import Path

import tiktoken
from google.genai import local_tokenizer

OUTPUT = (
    Path(__file__).resolve().parent.parent
    / "crates/aidash-domain/src/provider/estimate/encoding/fixtures.json"
)


def texts():
    rng = random.Random(276)
    prose = (
        "The agent reads the workspace, plans its next step and calls a tool. "
        "It reports what changed, which checks passed and what is still open.\n"
    )
    multilingual = (
        "東京都の天気は晴れです。明日は雨が降るでしょう。"
        "한국어 텍스트와 中文文本。Ünïcödé façade naïve café. "
        "Привет, мир! مرحبا بالعالم. नमस्ते दुनिया।\n"
    )
    schema = {
        "type": "object",
        "properties": {
            f"field_{index}": {
                "type": "string",
                "description": f"説明 {index} with \"quotes\" and \\ backslashes",
                "enum": [f"value_{index}_{choice}" for choice in range(3)],
            }
            for index in range(200)
        },
        "required": [f"field_{index}" for index in range(0, 200, 7)],
    }
    nested = {
        "nested": "line\nbreak\ttab \u0001 control",
        "quote": "she said \"hi\"",
        "unicode": "日本語 👩‍👩‍👧‍👦",
        "list": [1, 2.5, None, True],
    }
    tool_output = "\n".join(
        f"{index:04d} {rng.getrandbits(128):032x} "
        f"https://example.com/items/{rng.getrandbits(64):016x}?page={index}"
        for index in range(300)
    )
    return [
        ("prose", prose * 3),
        ("multilingual", multilingual * 3),
        ("emoji_and_combining", "👩‍👩‍👧‍👦 🇯🇵 ✨ éäñ " * 20),
        (
            "code",
            "fn main() {\n    let total: u64 = (1..=10).map(|n| n * n).sum();\n"
            '    println!("{total}");\n}\n' * 5,
        ),
        ("escaped_json", json.dumps(nested)),
        ("ascii_escaped_json", json.dumps(nested, ensure_ascii=True)),
        ("double_escaped_json", json.dumps(json.dumps(nested))),
        ("large_tool_schema", json.dumps(schema, ensure_ascii=False)),
        ("digits", " ".join(str(rng.randrange(10**12)) for _ in range(400))),
        ("tool_output", tool_output),
        ("whitespace_runs", "a" + " " * 5000 + "b\n\n\n\t\t" + "\n" * 2000 + "c"),
        ("base64", base64.b64encode(rng.randbytes(15000)).decode()),
        ("repeated_letter", "x" * 30000),
        (
            "large_multilingual",
            "".join(f"{index}. {prose}{multilingual}" for index in range(250)),
        ),
    ]


def main():
    warnings.simplefilter("ignore")
    o200k = tiktoken.get_encoding("o200k_base")
    gemini = local_tokenizer.LocalTokenizer(model_name="gemini-3.5-flash")
    fixtures = [
        {
            "name": name,
            "text": text,
            "o200k_base": len(o200k.encode_ordinary(text)),
            "gemma4": gemini.count_tokens(text).total_tokens,
        }
        for name, text in texts()
    ]
    OUTPUT.write_text(json.dumps(fixtures, ensure_ascii=False, indent=2) + "\n")
    print(f"wrote {len(fixtures)} fixtures to {OUTPUT}")


if __name__ == "__main__":
    main()
