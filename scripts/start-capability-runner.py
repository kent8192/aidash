#!/usr/bin/env python3
"""Start the trusted runner without putting its private token in process arguments."""
import argparse
import json
import os
from pathlib import Path
import sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("directory", type=Path)
args = parser.parse_args()
directory = args.directory.resolve()
profile_path = directory / "profile.json"
profile = json.loads(profile_path.read_text())
os.environ[profile["runner"]["token_env"]] = (directory / "token").read_text().strip()
os.environ["AIDASH_CAPABILITY_PROFILE"] = str(profile_path)
os.execv(sys.executable, [sys.executable, str(Path(__file__).resolve().parent.parent / "runner/control.py")])
