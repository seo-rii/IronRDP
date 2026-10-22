#!/usr/bin/env python3
"""Run the real Rust/WASM benchmark in Chromium, serving only this directory."""

import argparse
import functools
import hashlib
import http.server
import json
import platform
from pathlib import Path
import threading

from playwright.sync_api import sync_playwright


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--chromium", help="optional Chromium executable path")
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--target-ms", type=float, default=8)
    parser.add_argument("--correctness-only", action="store_true")
    parser.add_argument("--case", action="append", help="repeat selected case, e.g. 1920x1080/64x720")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent

    class Handler(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=root))
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        with sync_playwright() as playwright:
            options = {"headless": True}
            if args.chromium:
                options["executable_path"] = args.chromium
            browser = playwright.chromium.launch(**options)
            page = browser.new_page(viewport={"width": 1920, "height": 1080}, device_scale_factor=1)
            page.on("pageerror", lambda error: print(f"browser error: {error}", flush=True))
            page.on("console", lambda message: print(message.text, flush=True) if message.type == "log" else None)
            page.goto(f"http://127.0.0.1:{server.server_port}/")
            page.wait_for_function("typeof window.runBenchmark === 'function'")
            result = page.evaluate("options => window.runBenchmark(options)", {
                "samples": args.samples,
                "targetMs": args.target_ms,
                "correctnessOnly": args.correctness_only,
                "selectedCases": args.case,
            })
            result["chromium_version"] = browser.version
            result["platform"] = platform.platform()
            cdp = browser.new_browser_cdp_session()
            gpu = cdp.send("SystemInfo.getInfo")["gpu"]
            result["gpu_devices"] = gpu["devices"]
            result["gpu_feature_status"] = gpu["featureStatus"]
            result["gl_renderer"] = gpu["auxAttributes"].get("glRenderer")
            cdp.detach()
            result["wasm_sha256"] = hashlib.sha256((root / "pkg/ironrdp_web_dirty_region_bench_bg.wasm").read_bytes()).hexdigest()
            result["image_rs_sha256"] = hashlib.sha256((root / "../../src/image.rs").resolve().read_bytes()).hexdigest()
            result["canvas_rs_sha256"] = hashlib.sha256((root / "../../src/canvas.rs").resolve().read_bytes()).hexdigest()
            args.output.write_text(json.dumps(result, indent=2) + "\n")
            print(f"wrote {args.output}", flush=True)
            browser.close()
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
