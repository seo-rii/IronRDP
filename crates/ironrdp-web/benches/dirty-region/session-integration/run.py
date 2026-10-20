#!/usr/bin/env python3
"""Run real Session browser checks against the recorded wasm-pack packages."""

import argparse
from datetime import datetime, timezone
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import threading
from urllib.parse import quote, unquote, urlsplit

from build import BENCH, HERE, checkout_state, digest, source_hashes, write_json


CASES = {"idle-presentation", "timer-and-server-ready", "termination-flush", "termination-draw-error",
         "protocol-error-preserved", "draw-error-not-retried", "region-capacity-overflow",
         "overflow-error-preserves-region", "drop-cancels-timers",
         "activation-wait-cancellation", "reactivation-size-and-generation"}


class Handler(SimpleHTTPRequestHandler):
    def do_GET(self):
        path = (BENCH / unquote(urlsplit(self.path).path).lstrip("/")).resolve()
        if not path.is_relative_to(BENCH):
            self.send_error(403)
            return
        super().do_GET()

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        super().end_headers()

    def log_message(self, format, *args):
        pass


def verify(build_dir, manifest):
    if manifest.get("status") != "built":
        raise RuntimeError("the package build did not complete")
    if source_hashes() != manifest["source_sha256"]:
        raise RuntimeError("workspace sources or harness changed after the package build")
    state = checkout_state(manifest.get("expected_head_sha"))
    if state["checked_commit_sha"] != manifest["checked_commit_sha"]:
        raise RuntimeError("HEAD changed after the package build")
    for name, variant in manifest["variants"].items():
        package = build_dir / "pkg" / name
        actual = {path.relative_to(package).as_posix(): digest(path)
                  for path in sorted(package.rglob("*")) if path.is_file()}
        if actual != variant["artifacts"] or digest(build_dir / variant["build_log"]) != variant["build_log_sha256"]:
            raise RuntimeError(f"{name}: package or build log changed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=Path, default=BENCH / "output/session-correctness")
    parser.add_argument("--chromium", help="optional Chromium executable path")
    args = parser.parse_args()
    build_dir = args.build_dir.resolve()
    if not build_dir.is_relative_to(BENCH / "output"):
        parser.error("--build-dir must be inside the benchmark output directory")
    report_path = build_dir / "report.json"
    if report_path.exists():
        parser.error("report.json already exists; preserve it and make a new build for another run")
    manifest_path = build_dir / "build-manifest.json"
    manifest = json.loads(manifest_path.read_text())
    report = {"status": "running", "started_at_utc": datetime.now(timezone.utc).isoformat(),
              "checked_commit_sha": manifest["checked_commit_sha"],
              "sources_match_checked_commit": manifest["sources_match_checked_commit"],
              "build_manifest_sha256": digest(manifest_path), "correctness_only": True, "variants": {}}
    server = None
    try:
        from playwright.sync_api import sync_playwright

        verify(build_dir, manifest)
        if not {"default", "paced"}.issubset(manifest["variants"]):
            raise RuntimeError("both normal and opt-in Session builds are required")
        server = ThreadingHTTPServer(("127.0.0.1", 0), partial(Handler, directory=str(BENCH)))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        origin = f"http://127.0.0.1:{server.server_port}"
        with sync_playwright() as playwright:
            launch = {"headless": True}
            if args.chromium:
                launch["executable_path"] = args.chromium
            browser = playwright.chromium.launch(**launch)
            try:
                report["chromium"] = browser.version
                for name, variant in manifest["variants"].items():
                    context = browser.new_context()
                    errors = []
                    page = context.new_page()
                    page.set_default_timeout(120_000)
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    try:
                        page.goto(f"{origin}/{HERE.relative_to(BENCH).as_posix()}/index.html", wait_until="load")
                        package_root = "/" + quote((build_dir / "pkg" / name).relative_to(BENCH).as_posix())
                        if variant["fixture"]:
                            result = page.evaluate("args => window.runSessionChecks(args)",
                                                   {"packageRoot": package_root, "expectedPacing": variant["pacing"]})
                            expected = CASES | ({"drop-cancels-message-channel"} if variant["pacing"] else set())
                            observed = [case["case"] for case in result["cases"]]
                            if len(observed) != len(expected) or set(observed) != expected:
                                raise RuntimeError(f"{name}: required Session cases did not execute")
                            if result["simd128"] is not True or result["pacing"] != variant["pacing"]:
                                raise RuntimeError(f"{name}: wrong compiled features")
                        else:
                            result = page.evaluate("args => window.checkDeployment(args)", {"packageRoot": package_root})
                            if any(result.get(field) is not True for field in
                                   ("initialized", "public_builder_exercised", "test_exports_absent")):
                                raise RuntimeError("deployment smoke did not execute all checks")
                        if errors:
                            raise RuntimeError(f"{name}: uncaught browser errors: {errors}")
                        report["variants"][name] = result
                        write_json(report_path, report)
                    finally:
                        context.close()
            finally:
                browser.close()
        verify(build_dir, manifest)
        report["status"] = "passed"
    except Exception as error:
        report.update(status="failed", error=str(error))
        raise
    finally:
        if server:
            server.shutdown()
            server.server_close()
        report["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
        write_json(report_path, report)
    print(f"Session correctness passed: {report_path}")


if __name__ == "__main__":
    main()
