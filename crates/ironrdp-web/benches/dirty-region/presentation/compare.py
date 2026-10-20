#!/usr/bin/env python3
"""Compare independently compiled WASM variants and A/A controls in Chromium."""

import argparse
import asyncio
from datetime import datetime, timezone
import functools
import hashlib
import http.server
import json
import math
import os
from pathlib import Path
import platform
import re
import sys
import threading
from urllib.parse import quote

from playwright.async_api import async_playwright


TIMEOUT_SECONDS = 120
CORRECTNESS_TIMEOUT_SECONDS = 300
DEFAULT_CASES = ("1920x1080/1920x1080", "1920x1080/64x720", "1920x1080/32x32", "1920x1080/17x65")


def host_snapshot():
    # Host-wide counters show contention without collecting process details.
    fields = ("user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal", "guest", "guest_nice")
    ticks = list(map(int, Path("/proc/stat").read_text().splitlines()[0].split()[1:]))
    return {"load_average": os.getloadavg(), "cpu_ticks": dict(zip(fields, ticks)),
            "clock_ticks_per_second": os.sysconf("SC_CLK_TCK")}


def write_json(path, value):
    # Preserve completed runs if a later session fails or a command is repeated.
    with path.open("x", encoding="utf-8") as output:
        json.dump(value, output, indent=2)
        output.write("\n")


def parse_args():
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="new result directory")
    parser.add_argument("--build-dir", type=Path, default=root / "output/simd-presentation-20261019")
    parser.add_argument("--chromium", help="optional Chromium executable path")
    parser.add_argument("--rounds", type=int, default=4, help="each round runs every selected comparison in a fresh browser")
    parser.add_argument("--comparison", action="append",
                        help="repeat candidate variant names or aa; default: all candidates and baseline/baseline")
    parser.add_argument("--samples", type=int, default=12, help="ABBA or BAAB blocks per case and mode")
    parser.add_argument("--target-ms", type=float, default=16, help="baseline batch duration used for initial common calibration")
    parser.add_argument("--warmup-ms", type=float, default=400, help="minimum measured warmup time per variant and mode")
    parser.add_argument("--seed", type=lambda value: int(value, 0), default=0x12345678)
    parser.add_argument("--case", action="append", help="repeat for selected image/dirty dimensions, e.g. 1920x1080/64x720")
    parser.add_argument("--include-prepare", action="store_true", help="also measure extraction (includes alpha only in fused build)")
    parser.add_argument("--correctness-only", action="store_true")
    args = parser.parse_args()
    if (min(args.rounds, args.samples, args.target_ms, args.warmup_ms) <= 0
            or not all(math.isfinite(value) for value in (args.target_ms, args.warmup_ms))):
        parser.error("rounds, samples, target duration, and warmup duration must be positive")
    args.build_dir = args.build_dir.resolve()
    if not args.build_dir.is_relative_to(root):
        parser.error("--build-dir must be inside the dirty-region benchmark directory")
    manifest_path = args.build_dir / "build-manifest.json"
    if not manifest_path.is_file():
        parser.error("missing build-manifest.json")
    manifest = json.loads(manifest_path.read_text())
    args.baseline = manifest.get("comparison_baseline", "scalar")
    mode = manifest.get("mode", "historical")
    args.variants = ("baseline", "candidate") if mode == "direct" else ("scalar", "simd", "fused")
    if (mode not in ("historical", "direct") or set(manifest.get("variants", {})) != set(args.variants)
            or args.baseline != args.variants[0]):
        parser.error("build manifest is incomplete or has unsupported variants/baseline")
    available = [variant for variant in args.variants if variant != args.baseline] + ["aa"]
    args.comparisons = args.comparison or available
    if not set(args.comparisons) <= set(available) or len(set(args.comparisons)) != len(args.comparisons):
        parser.error(f"comparisons must be unique choices from: {', '.join(available)}")
    for variant in args.variants:
        for suffix in (".js", "_bg.wasm"):
            if not (args.build_dir / "pkg" / variant / f"ironrdp_web_dirty_region_bench{suffix}").is_file():
                parser.error(f"missing {variant} WASM package; finish building the variants first")
    args.fixtures = []
    for name in args.case or DEFAULT_CASES:
        match = re.fullmatch(r"([0-9]+)x([0-9]+)/([0-9]+)x([0-9]+)", name)
        if not match:
            parser.error("--case must look like 1920x1080/64x720")
        image_width, image_height, width, height = map(int, match.groups())
        if not (0 < width <= image_width <= 65535 and 0 < height <= image_height <= 65535):
            parser.error("case dimensions must be positive u16 values and the dirty region must fit")
        if name in [fixture["id"] for fixture in args.fixtures]:
            parser.error(f"duplicate case: {name}")
        args.fixtures.append({"id": name, "imageWidth": image_width, "imageHeight": image_height,
                              "width": width, "height": height, "left": (image_width - width) // 2,
                              "top": (image_height - height) // 2})
    args.output = args.output.resolve()
    if args.output.exists() and any(args.output.iterdir()):
        parser.error("--output must be new or empty; completed measurements are never overwritten")
    args.output.mkdir(parents=True, exist_ok=True)
    return args


async def evaluate(page, expression, options=None, *, timeout=TIMEOUT_SECONDS):
    return await asyncio.wait_for(page.evaluate(expression, options), timeout)


async def session(playwright, args, url, package_root, plan, counts):
    launch = {"headless": True, "timeout": TIMEOUT_SECONDS * 1000}
    if args.chromium:
        launch["executable_path"] = args.chromium
    browser = await playwright.chromium.launch(**launch)
    try:
        page = await browser.new_page(viewport={"width": 1920, "height": 1080}, device_scale_factor=1)
        page.set_default_timeout(TIMEOUT_SECONDS * 1000)
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)) if len(errors) < 10 else None)
        page.on("console", lambda message: errors.append(message.text)
                if message.type == "error" and len(errors) < 10 else None)
        await page.goto(url, wait_until="load")
        await page.wait_for_function("typeof window.prepare === 'function'")
        prepared = await evaluate(page, "options => window.prepare(options)",
                                  {"pair": plan["pair"], "packageRoot": package_root, "baseline": args.baseline})
        if not prepared["cross_origin_isolated"]:
            raise RuntimeError("benchmark page is not cross-origin isolated")
        cdp = await browser.new_browser_cdp_session()
        gpu = (await cdp.send("SystemInfo.getInfo"))["gpu"]
        result = {"plan": plan, "started_at_utc": datetime.now(timezone.utc).isoformat(),
                  "chromium_version": browser.version, "platform": platform.platform(),
                  "gpu_devices": gpu["devices"], "gpu_feature_status": gpu.get("featureStatus", {}),
                  "gl_renderer": gpu.get("auxAttributes", {}).get("glRenderer"), "prepared": prepared,
                  "host_before": host_snapshot()}
        if plan["kind"] == "correctness":
            # Expanded 4K and bounds checks include complete RGBA readbacks in
            # software Canvas. Keep their budget separate from timing sessions.
            result["correctness"] = await evaluate(page, "options => window.checkCorrectness(options)",
                                                   {"extraFixtures": args.fixtures},
                                                   timeout=CORRECTNESS_TIMEOUT_SECONDS)
        else:
            measurements = []
            for fixture in plan["cases"]:
                modes = ["draw"]
                if fixture["width"] == fixture["imageWidth"] and fixture["height"] == fixture["imageHeight"]:
                    modes.append("draw_js")
                if args.include_prepare:
                    modes.append("prepare")
                if plan["reverse_modes"]:
                    modes.reverse()
                options = {"fixture": fixture, "samples": args.samples, "targetMs": args.target_ms,
                           "warmupMs": args.warmup_ms, "blockOrder": plan["block_order"],
                           "modeOrder": modes, "iterationsByMode": counts.get(fixture["id"], {})}
                before = host_snapshot()
                measurement = await evaluate(page, "options => window.measureCase(options)", options)
                measurement["host_before"] = before
                measurement["host_after"] = host_snapshot()
                measurements.append(measurement)
                counts[fixture["id"]] = {row["mode"]: row["iterations"] for row in measurement["results"]}
            result["measurements"] = measurements
        result["host_after"] = host_snapshot()
        result["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
        result["browser_errors"] = errors
        await cdp.detach()
        if errors:
            raise RuntimeError(f"browser errors: {errors}")
        return result
    finally:
        await asyncio.wait_for(browser.close(), TIMEOUT_SECONDS)


async def run(args):
    script_root = Path(__file__).resolve().parent
    root = script_root.parent
    manifest_bytes = (args.build_dir / "build-manifest.json").read_bytes()
    provenance = {"build_manifest": json.loads(manifest_bytes),
                  "build_manifest_sha256": hashlib.sha256(manifest_bytes).hexdigest(),
                  "harness_sha256": {name: hashlib.sha256((script_root / name).read_bytes()).hexdigest()
                                     for name in ("compare.py", "compare.js", "index.html")},
                  "package_sha256": {variant: {suffix: hashlib.sha256(
                      (args.build_dir / "pkg" / variant / f"ironrdp_web_dirty_region_bench{suffix}").read_bytes()
                  ).hexdigest() for suffix in (".js", "_bg.wasm")} for variant in args.variants}}
    for variant in args.variants:
        artifacts = provenance["build_manifest"].get("variants", {}).get(variant, {}).get("artifacts", {})
        for suffix, actual in provenance["package_sha256"][variant].items():
            filename = f"ironrdp_web_dirty_region_bench{suffix}"
            if artifacts.get(filename) != actual:
                raise RuntimeError(f"{variant}/{filename} does not match the completed build manifest")
    schedule = []
    for round_index in range(args.rounds):
        comparisons = args.comparisons[:]
        rotation = (args.seed + round_index) % len(comparisons)
        comparisons = comparisons[rotation:] + comparisons[:rotation]
        if (round_index // 3) % 2:
            comparisons.reverse()
        for comparison in comparisons:
            comparison_index = args.comparisons.index(comparison)
            pair = [args.baseline, args.baseline if comparison == "aa" else comparison]
            # Across four rounds, each pair order occurs with each mode order.
            if round_index % 2:
                pair.reverse()
            rotation = (args.seed + round_index + comparison_index) % len(args.fixtures)
            fixtures = args.fixtures[rotation:] + args.fixtures[:rotation]
            schedule.append({"kind": "timing", "session": len(schedule) + 1, "round": round_index + 1,
                             "comparison": comparison, "baseline": args.baseline, "pair": pair, "cases": fixtures,
                             "reverse_modes": bool((round_index // 2 + comparison_index) % 2),
                             "block_order": (args.seed + round_index + comparison_index) % 2})
    experiment = {**provenance, "seed": args.seed, "samples": args.samples, "rounds": args.rounds,
                  "target_ms": args.target_ms, "warmup_ms": args.warmup_ms,
                  "include_prepare": args.include_prepare, "correctness_only": args.correctness_only,
                  "comparison_baseline": args.baseline, "comparisons": args.comparisons,
                  "schedule": [] if args.correctness_only else schedule,
                  "notes": ["timings exclude compositor completion and RDP decoding",
                            "historical fused prepare includes alpha; select implementations using complete draw work",
                            f"common even batch sizes are calibrated on {args.baseline} and reused across sessions"]}
    write_json(args.output / "experiment.json", experiment)

    class Handler(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def end_headers(self):
            self.send_header("Cross-Origin-Opener-Policy", "same-origin")
            self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
            self.send_header("Cache-Control", "no-store")
            super().end_headers()

        def send_head(self):
            # Reject symlinks escaping the benchmark's parent directory.
            if not Path(self.translate_path(self.path)).resolve().is_relative_to(root):
                self.send_error(403, "outside benchmark directory")
                return None
            return super().send_head()

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=root))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        url = f"http://127.0.0.1:{server.server_port}/presentation/"
        package_root = "/" + quote((args.build_dir / "pkg").relative_to(root).as_posix(), safe="/")
        counts = {}
        async with async_playwright() as playwright:
            correctness = await session(playwright, args, url, package_root,
                                        {"kind": "correctness", "pair": list(args.variants)}, counts)
            write_json(args.output / "correctness.json", correctness)
            print(f"wrote {args.output / 'correctness.json'}", flush=True)
            if not args.correctness_only:
                for plan in schedule:
                    result = await session(playwright, args, url, package_root, plan, counts)
                    path = args.output / f"session-{plan['session']:03d}.json"
                    write_json(path, result)
                    print(f"wrote {path}", flush=True)
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    try:
        asyncio.run(run(parse_args()))
    except Exception as error:
        print(f"error: {type(error).__name__}: {error}", file=sys.stderr, flush=True)
        raise SystemExit(1) from error
