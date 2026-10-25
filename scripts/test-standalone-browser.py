#!/usr/bin/env python3
"""Run standalone Pages E2E through Reinhardt's fixtures::wasm::e2e (no Node.js)."""

import argparse
from contextlib import ExitStack
import os
from pathlib import Path
import signal
import socket
import subprocess
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


def stop(process):
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=10)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--product", choices=("auth", "marketplace", "both"), default="both")
    parser.add_argument("--driver", default=os.environ.get("CHROMEDRIVER", "chromedriver"))
    parser.add_argument("--filter", help="run a matching Rust test name during focused repair")
    args = parser.parse_args()
    environment = dict(os.environ)
    for product in (("marketplace",) if args.product == "marketplace" else ("auth", "marketplace")):
        if not (ROOT / "enterprise" / product / "dist/index.html").is_file():
            raise SystemExit(f"Build both products' Pages first: enterprise/{product}/scripts/wasm-build-dev.sh")
    with ExitStack() as cleanup:
        if not environment.get("WEBDRIVER_URL"):
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            (ROOT / "notes").mkdir(exist_ok=True)
            log = cleanup.enter_context((ROOT / "notes/standalone-webdriver.log").open("w"))
            process = subprocess.Popen([args.driver, f"--port={port}", "--allowed-ips=127.0.0.1"],
                                       stdout=log, stderr=log, start_new_session=True)
            cleanup.callback(stop, process)
            environment["WEBDRIVER_URL"] = f"http://127.0.0.1:{port}"
            for _ in range(100):
                if process.poll() is not None:
                    raise SystemExit("WebDriver stopped; see notes/standalone-webdriver.log")
                try:
                    urllib.request.urlopen(environment["WEBDRIVER_URL"] + "/status", timeout=1).close()
                    break
                except OSError:
                    time.sleep(0.1)
            else:
                raise SystemExit("WebDriver did not become ready")
        products = ("auth", "marketplace") if args.product == "both" else (args.product,)
        for product in products:
            print(f"Running real {product} Pages with Reinhardt WASM browser fixtures", flush=True)
            subprocess.run(["cargo", "test", "--locked", "--manifest-path", "enterprise/Cargo.toml",
                            "-p", f"aidash-{product}-server", "--features", "browser-e2e", "--lib", *([args.filter] if args.filter else []),
                            "--", "--test-threads=1"], cwd=ROOT, env=environment, check=True, timeout=1200)


if __name__ == "__main__":
    main()
