#!/usr/bin/env python3
"""Run synthetic packet coalescing or ready-queue pacing workloads in Chromium."""
import argparse
import asyncio
from datetime import datetime, timezone
import functools
import hashlib
import http.server
import json
import math
from pathlib import Path
import statistics
import sys
import threading
from urllib.parse import quote

from playwright.async_api import async_playwright

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
sys.path.insert(0, str(BENCH))
from presentation.compare import host_snapshot, write_json  # noqa: E402
from presentation.summarize import T975  # noqa: E402


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, required=True, help="WASM package directory inside the benchmark")
    parser.add_argument("--manifest", type=Path, required=True, help="build manifest with top-level artifacts SHA-256 map")
    parser.add_argument("--output", type=Path, required=True, help="new or empty result directory")
    parser.add_argument("--chromium")
    parser.add_argument("--suite", choices=("packet", "pacing"), default="packet")
    parser.add_argument("--case", action="append")
    parser.add_argument("--comparison", action="append")
    parser.add_argument("--rounds", type=int, default=4)
    parser.add_argument("--samples", type=int, default=12, help="paired ABBA/BAAB blocks for packet timing")
    parser.add_argument("--full-updates", type=int, default=96)
    parser.add_argument("--narrow-updates", type=int, default=1024)
    parser.add_argument("--partial-updates", type=int, default=16384)
    parser.add_argument("--correctness-only", action="store_true")
    args = parser.parse_args()
    if min(args.rounds, args.samples, args.full_updates, args.narrow_updates, args.partial_updates) <= 0:
        parser.error("counts must be positive")
    cases = ["single", "repeat", "contained", "overlap", "disjoint", "overflow", "barrier"] if args.suite == "packet" else ["full", "narrow", "partial"]
    comparisons = ["coalesced", "aa"] if args.suite == "packet" else ["paced60", "paced30", "aa"]
    args.cases = args.case or cases
    args.comparisons = args.comparison or comparisons
    for values, allowed, label in [(args.cases, cases, "cases"), (args.comparisons, comparisons, "comparisons")]:
        if not set(values) <= set(allowed) or len(set(values)) != len(values):
            parser.error(f"{label} must be unique choices from {', '.join(allowed)}")
    args.package = args.package.resolve()
    if not args.package.is_relative_to(BENCH):
        parser.error("package must be inside the benchmark directory")
    args.manifest = args.manifest.resolve()
    manifest = json.loads(args.manifest.read_text())
    args.package_hashes = {}
    for suffix in (".js", "_bg.wasm"):
        name = f"ironrdp_web_dirty_region_bench{suffix}"
        actual = digest(args.package / name)
        if manifest.get("artifacts", {}).get(name) != actual:
            parser.error(f"{name} does not match the supplied build manifest")
        args.package_hashes[name] = actual
    args.output = args.output.resolve()
    if args.output.exists() and any(args.output.iterdir()):
        parser.error("output must be new or empty")
    args.output.mkdir(parents=True, exist_ok=True)
    return args


async def session(playwright, args, url, package_url, plan, counts):
    launch = {"headless": True}
    if args.chromium:
        launch["executable_path"] = args.chromium
    browser = await playwright.chromium.launch(**launch)
    try:
        page = await browser.new_page(viewport={"width": 1920, "height": 1080}, device_scale_factor=1)
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        await page.goto(url, wait_until="load")
        prepared = await page.evaluate("root => window.prepare(root)", package_url)
        if not prepared["cross_origin_isolated"]:
            raise ValueError("benchmark requires cross-origin isolation")
        cdp = await browser.new_browser_cdp_session()
        gpu = (await cdp.send("SystemInfo.getInfo"))["gpu"]
        result = {"plan": plan, "prepared": prepared, "chromium_version": browser.version,
                  "gpu": gpu, "started_at_utc": datetime.now(timezone.utc).isoformat(), "host_before": host_snapshot()}
        if plan["kind"] == "correctness":
            result["yield_probe"] = await page.evaluate("() => window.probeYields(32)")
            result["packet_checks"] = await page.evaluate("() => window.checkPackets()")
            result["pacing_checks"] = await page.evaluate("() => window.checkPacing()")
        else:
            rows = []
            for name in plan["cases"]:
                options = {"name": name, "pair": plan["pair"], "blockOrder": plan["block_order"],
                           "samples": args.samples, "iterations": counts.get(name),
                           "fullUpdates": args.full_updates, "narrowUpdates": args.narrow_updates, "partialUpdates": args.partial_updates}
                before = host_snapshot()
                function = "measurePacket" if args.suite == "packet" else "measurePacing"
                row = await asyncio.wait_for(page.evaluate(f"options => window.{function}(options)", options), 300)
                row["host_before"] = before
                row["host_after"] = host_snapshot()
                rows.append(row)
                if args.suite == "packet":
                    counts[name] = row["iterations"]
            result["measurements"] = rows
        result.update(host_after=host_snapshot(), browser_errors=errors,
                      finished_at_utc=datetime.now(timezone.utc).isoformat())
        if errors:
            raise ValueError(f"browser errors: {errors[:5]}")
        return result
    finally:
        await browser.close()


def summarize(sessions, suite):
    groups = {}
    for session_data in sessions:
        plan = session_data["plan"]
        baseline_slot = 0 if plan["comparison"] == "aa" else plan["pair"].index("baseline")
        candidate_slot = 1 - baseline_slot
        for row in session_data["measurements"]:
            if suite == "packet":
                logs = [math.log([block["a_us"], block["b_us"]][candidate_slot])
                        - math.log([block["a_us"], block["b_us"]][baseline_slot]) for block in row["blocks"]]
                metrics = {"per_packet_us": statistics.median(logs)}
            else:
                metrics = {}
                for metric in ("apply_ms", "finish_ms", "active_work_ms", "task_marker_delay_ms", "heartbeat_gap_max_ms"):
                    values = [statistics.mean(observation["result"][metric] for observation in row["observations"]
                                              if observation["slot"] == slot) for slot in (0, 1)]
                    if min(values) > 0:
                        metrics[metric] = math.log(values[candidate_slot]) - math.log(values[baseline_slot])
            for metric, estimate in metrics.items():
                groups.setdefault((plan["comparison"], row["name"], metric), []).append(estimate)
    results = []
    for (comparison, name, metric), values in sorted(groups.items()):
        mean = statistics.mean(values)
        interval = None
        if len(values) > 1:
            margin = T975[min(len(values) - 2, len(T975) - 1)] * statistics.stdev(values) / math.sqrt(len(values))
            interval = [100 * math.expm1(mean - margin), 100 * math.expm1(mean + margin)]
        results.append({"comparison": comparison, "case": name, "metric": metric, "n_sessions": len(values),
                        "ratio": math.exp(mean), "change_percent": 100 * math.expm1(mean),
                        "exploratory_ci95_percent": interval, "session_ratios": [math.exp(value) for value in values]})
    return {"method": "paired log ratios summarized per fresh browser session, then geometric mean across sessions",
            "limits": "small-n exploratory intervals; synthetic submission/task responsiveness, not live RDP latency",
            "results": results}


async def run(args):
    schedule = []
    for round_index in range(args.rounds):
        rotation = round_index % len(args.comparisons)
        for comparison in args.comparisons[rotation:] + args.comparisons[:rotation]:
            pair = ["baseline", "baseline" if comparison == "aa" else comparison]
            if round_index % 2:
                pair.reverse()
            case_rotation = round_index % len(args.cases)
            schedule.append({"kind": "timing", "session": len(schedule) + 1, "round": round_index + 1,
                             "comparison": comparison, "pair": pair, "block_order": (round_index // 2) % 2,
                             "cases": args.cases[case_rotation:] + args.cases[:case_rotation]})
    write_json(args.output / "experiment.json", {
        "suite": args.suite, "schedule": [] if args.correctness_only else schedule,
        "build_manifest": json.loads(args.manifest.read_text()), "build_manifest_sha256": digest(args.manifest),
        "package_sha256": args.package_hashes,
        "harness_sha256": {name: digest(HERE / name) for name in ("browser.js", "browser.py", "index.html")},
        "samples": args.samples, "full_updates": args.full_updates, "narrow_updates": args.narrow_updates, "partial_updates": args.partial_updates,
        "implementation": "single WASM binary, separate benchmark state per policy; actual Canvas/PendingRegions/FramePacing/BrowserYield; Rust ready-task driver",
    })

    class Handler(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def end_headers(self):
            self.send_header("Cross-Origin-Opener-Policy", "same-origin")
            self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
            self.send_header("Cache-Control", "no-store")
            super().end_headers()

        def send_head(self):
            if not Path(self.translate_path(self.path)).resolve().is_relative_to(BENCH):
                self.send_error(403)
                return None
            return super().send_head()

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=BENCH))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        url = f"http://127.0.0.1:{server.server_port}/updates/"
        package_url = "/" + quote(args.package.relative_to(BENCH).as_posix(), safe="/")
        counts = {}
        results = []
        async with async_playwright() as playwright:
            correctness = await session(playwright, args, url, package_url, {"kind": "correctness"}, counts)
            write_json(args.output / "correctness.json", correctness)
            if not args.correctness_only:
                for plan in schedule:
                    result = await session(playwright, args, url, package_url, plan, counts)
                    write_json(args.output / f"session-{plan['session']:03d}.json", result)
                    results.append(result)
                write_json(args.output / "summary.json", summarize(results, args.suite))
        print(f"Completed {len(results)} timing sessions: {args.output}")
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    asyncio.run(run(parse_args()))
