#!/usr/bin/env python3
"""Check the current production pixels in Chromium with and without WASM SIMD."""

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tomllib

from build import BENCH, FLAGS, HERE, REPO, digest


CASES = (
    "1920x1080/1920x1080", "1920x1080/64x720", "1920x1080/32x32", "1920x1080/17x65",
    "1920x1080/1535x720", "1920x1080/1536x720", "1920x1080/1537x720",
    "1920x1080/1800x720", "1920x1080/1919x720", "1920x1080/1920x720",
    "1920x1080/1536x34", "1920x1080/1536x35", "1920x1080/1919x1080",
    "1920x1080/1537x34", "1920x1080/1537x35",
    "1920x1080/1920x34", "1920x1080/1920x35", "1919x1080/1919x35",
    "1919x1080/1919x1080", "1921x1080/1537x35",
    "3840x2160/3840x2160", "3840x2160/64x1440", "3840x2160/3072x1440",
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=BENCH / "output/pixel-correctness")
    parser.add_argument("--target-dir", type=Path, help="cache belonging to this physical benchmark workspace")
    parser.add_argument("--wasm-bindgen", default=shutil.which("wasm-bindgen"))
    parser.add_argument("--chromium", help="optional Chromium executable path")
    parser.add_argument("--toolchain", default=tomllib.loads((REPO / "rust-toolchain.toml").read_text())["toolchain"]["channel"])
    parser.add_argument("--expected-head", help="require this full Git SHA and an unchanged checkout (used by CI)")
    args = parser.parse_args()
    if not args.wasm_bindgen:
        parser.error("supply --wasm-bindgen matching the benchmark Cargo.lock")
    if args.expected_head and not re.fullmatch(r"[0-9a-f]{40}", args.expected_head):
        parser.error("--expected-head must be a full lowercase Git SHA")
    output = args.output.resolve()
    if not output.is_relative_to(BENCH):
        parser.error("--output must be inside the dirty-region benchmark directory")
    if output.exists() and any(output.iterdir()):
        parser.error("--output must be new or empty; completed results are never overwritten")
    output.mkdir(parents=True, exist_ok=True)
    target = (args.target_dir or output / "target").resolve()
    report = {"status": "running", "started_at_utc": datetime.now(timezone.utc).isoformat(),
              "expected_head_sha": args.expected_head, "correctness_only": True}
    try:
        head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip()
        report["checked_commit_sha"] = head
        if args.expected_head and args.expected_head != head:
            raise RuntimeError(f"checked out {head}, expected {args.expected_head}")
        clean = subprocess.run(["git", "diff", "--quiet", "HEAD", "--"], cwd=REPO, check=False)
        if clean.returncode not in (0, 1):
            raise RuntimeError("could not check the tracked checkout")
        report["tracked_checkout_clean"] = clean.returncode == 0
        if args.expected_head and not report["tracked_checkout_clean"]:
            raise RuntimeError("tracked checkout differs from the checked commit")

        source_paths = [REPO / "crates/ironrdp-web/src" / name for name in ("image.rs", "canvas.rs")]
        harness_paths = [BENCH / name for name in (
            "Cargo.toml", "Cargo.lock", "build.rs", "policy.rs", "src/lib.rs", "src/presentation.rs",
            "presentation/build.py", "presentation/ci.py", "presentation/compare.py",
            "presentation/compare.js", "presentation/index.html",
        )]
        original_hashes = {path: digest(path) for path in source_paths + harness_paths}
        matches_head = True
        for path in original_hashes:
            committed = subprocess.run(["git", "show", f"{head}:{path.relative_to(REPO).as_posix()}"],
                                       cwd=REPO, capture_output=True, check=False)
            matches_head &= committed.returncode == 0 and committed.stdout == path.read_bytes()
        if args.expected_head and not matches_head:
            raise RuntimeError("production or harness files differ from the checked commit")
        report["sources_match_checked_commit"] = matches_head
        report["production_source_sha256"] = {path.name: original_hashes[path] for path in source_paths}

        version = subprocess.check_output([args.wasm_bindgen, "--version"], text=True).strip()
        packages = tomllib.loads((BENCH / "Cargo.lock").read_text())["package"]
        locked_version = next(package["version"] for package in packages if package["name"] == "wasm-bindgen")
        if version != f"wasm-bindgen {locked_version}":
            raise RuntimeError(f"{version} does not match locked wasm-bindgen {locked_version}")
        manifest = {
            "mode": "direct", "purpose": "current-source scalar/SIMD correctness only",
            "comparison_baseline": "baseline", "checked_commit_sha": head,
            "expected_head_sha": args.expected_head, "sources_match_checked_commit": matches_head,
            "tracked_checkout_clean": report["tracked_checkout_clean"],
            "rustc": subprocess.check_output(["rustc", f"+{args.toolchain}", "-Vv"], cwd=BENCH, text=True),
            "wasm_bindgen": version, "profile": "release: opt-level=s, lto=true; no wasm-opt",
            "harness_sha256": {path.relative_to(BENCH).as_posix(): original_hashes[path] for path in harness_paths},
            "variants": {},
        }
        sources = output / "sources"
        sources.mkdir()
        for path in source_paths:
            shutil.copyfile(path, sources / path.name)
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")

        # Names satisfy the existing comparison runner. Both use the current, unadapted
        # implementation, including its actual copy helper; only target features differ.
        for variant, simd in (("baseline", False), ("candidate", True)):
            flags = FLAGS if simd else FLAGS.replace("+simd128", "-simd128")
            env = dict(os.environ, CARGO_TARGET_DIR=str(target), CARGO_BUILD_BUILD_DIR=str(target / "build"),
                       RUSTFLAGS=flags, IRONRDP_BENCH_VARIANT="candidate",
                       IRONRDP_BENCH_EXPECT_SIMD128="1" if simd else "0",
                       IRONRDP_BENCH_IMAGE_SOURCE=str(sources / "image.rs"),
                       IRONRDP_BENCH_CANVAS_SOURCE=str(sources / "canvas.rs"))
            # CARGO_ENCODED_RUSTFLAGS takes precedence over RUSTFLAGS if inherited.
            env.pop("CARGO_ENCODED_RUSTFLAGS", None)
            subprocess.run(["cargo", f"+{args.toolchain}", "build", "--lib", "--locked", "--release",
                            "--target", "wasm32-unknown-unknown", "--features", "presentation"],
                           cwd=BENCH, env=env, check=True)
            package = output / "pkg" / variant
            subprocess.run([args.wasm_bindgen,
                            str(target / "wasm32-unknown-unknown/release/ironrdp_web_dirty_region_bench.wasm"),
                            "--target", "web", "--out-dir", str(package)], check=True)
            manifest["variants"][variant] = {
                "simd128": simd, "rustflags": flags, "compiled_variant": "candidate",
                "production_sources": report["production_source_sha256"],
                "sources": {name: digest(sources / name) for name in ("image.rs", "canvas.rs")},
                "source_origin": "checked commit" if matches_head else "working-tree snapshot",
                "artifacts": {path.name: digest(path) for path in sorted(package.iterdir()) if path.is_file()},
            }
            (output / "build-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")

        command = [sys.executable, str(HERE / "compare.py"), "--build-dir", str(output),
                   "--correctness-only", "--output", str(output / "results")]
        if args.chromium:
            command.extend(["--chromium", args.chromium])
        for case in CASES:
            command.extend(["--case", case])
        subprocess.run(command, cwd=BENCH, check=True)
        correctness = json.loads((output / "results/correctness.json").read_text())
        observed = correctness["prepared"].get("simd128")
        if observed != [False, True]:
            raise RuntimeError(f"WASM target features were not scalar/SIMD as required: {observed!r}")
        if any(digest(path) != expected for path, expected in original_hashes.items()):
            raise RuntimeError("production or harness files changed during the check")
        report.update(status="passed", observed_simd128=observed,
                      chromium_version=correctness["chromium_version"],
                      correctness_sha256=digest(output / "results/correctness.json"),
                      build_manifest_sha256=digest(output / "build-manifest.json"))
        print(f"Current production scalar/SIMD correctness passed: {output}", flush=True)
    except Exception as error:
        report.update(status="failed", error=f"{type(error).__name__}: {error}")
        raise
    finally:
        report["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"error: {type(error).__name__}: {error}", file=sys.stderr, flush=True)
        raise SystemExit(1) from error
