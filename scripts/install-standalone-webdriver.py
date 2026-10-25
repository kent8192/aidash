#!/usr/bin/env python3
"""Install a matching official ChromeDriver into task-local private storage."""
import argparse
import json
from pathlib import Path
import platform
import re
import shutil
import subprocess
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=ROOT / "private/standalone-webdriver")
    args = parser.parse_args()
    candidates = [shutil.which(name) for name in ("google-chrome", "google-chrome-stable", "chromium", "chromium-browser")]
    candidates.append("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
    browser = next((path for path in candidates if path and Path(path).is_file()), None)
    if browser is None:
        raise SystemExit("Install Chrome/Chromium or provide an existing WEBDRIVER_URL")
    version = re.search(r"\d+\.\d+\.\d+\.\d+", subprocess.check_output([browser, "--version"], text=True)).group()
    metadata = json.load(urllib.request.urlopen("https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json", timeout=30))
    matching = [entry for entry in metadata["versions"] if entry["version"].rsplit(".", 1)[0] == version.rsplit(".", 1)[0]]
    if not matching:
        raise SystemExit(f"No official driver matches browser {version}")
    system = platform.system()
    target = "mac-arm64" if system == "Darwin" and platform.machine() == "arm64" else "mac-x64" if system == "Darwin" else "linux64"
    entry = matching[-1]
    url = next(item["url"] for item in entry["downloads"]["chromedriver"] if item["platform"] == target)
    destination = args.directory.resolve()
    if not destination.is_relative_to(ROOT / "private"):
        raise SystemExit("Use a task-local directory under private/")
    destination.mkdir(parents=True, exist_ok=True)
    archive = destination / "chromedriver.zip"
    archive.write_bytes(urllib.request.urlopen(url, timeout=60).read())
    with zipfile.ZipFile(archive) as source:
        source.extractall(destination)
    binary = destination / f"chromedriver-{target}/chromedriver"
    binary.chmod(0o755)
    print(binary)


if __name__ == "__main__":
    main()
