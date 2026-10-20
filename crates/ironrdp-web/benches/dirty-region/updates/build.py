#!/usr/bin/env python3
"""Build synthetic update workloads with snapshots of the production helpers."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
REPO = BENCH.parents[3]
sys.path.insert(0, str(BENCH))
from presentation.build import FLAGS  # noqa: E402


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="new or empty output directory")
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--build-dir", type=Path, help="reuse this workspace's existing intermediate directory")
    parser.add_argument("--wasm-bindgen", default=shutil.which("wasm-bindgen"))
    parser.add_argument("--toolchain", default="1.94.1")
    args = parser.parse_args()
    if not args.wasm_bindgen:
        parser.error("supply --wasm-bindgen matching the locked crate")
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("output must be new or empty")
    target = (args.target_dir or output / "target").resolve()
    build = (args.build_dir or target / "build").resolve()
    sources = output / "sources"
    sources.mkdir(parents=True)
    modules = {
        "image.rs": "IRONRDP_BENCH_IMAGE_SOURCE",
        "canvas.rs": "IRONRDP_BENCH_CANVAS_SOURCE",
        "presentation.rs": "IRONRDP_BENCH_PENDING_SOURCE",
        "frame_pacing.rs": "IRONRDP_BENCH_PACING_SOURCE",
        "browser_yield.rs": "IRONRDP_BENCH_YIELD_SOURCE",
    }
    env = dict(os.environ, CARGO_TARGET_DIR=str(target), CARGO_BUILD_BUILD_DIR=str(build), RUSTFLAGS=FLAGS)
    env.pop("IRONRDP_BENCH_VARIANT", None)
    for name, variable in modules.items():
        shutil.copyfile(REPO / "crates/ironrdp-web/src" / name, sources / name)
        env[variable] = str(sources / name)
    manifest = {
        "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip(),
        "source_origin": "working-tree snapshots; not a replay of the production session loop",
        "rustc": subprocess.check_output(["rustc", f"+{args.toolchain}", "-Vv"], text=True),
        "wasm_bindgen": subprocess.check_output([args.wasm_bindgen, "--version"], text=True).strip(),
        "rustflags": FLAGS,
        "profile": "release: opt-level=s, lto=true; no wasm-opt",
        "sources": {name: digest(sources / name) for name in modules},
        "harness_sha256": {name: digest(BENCH / name) for name in [
            "Cargo.toml", "Cargo.lock", "build.rs", "policy.rs", "src/lib.rs", "src/updates.rs",
            "updates/build.py", "updates/browser.py", "updates/browser.js", "updates/index.html",
        ]},
    }
    subprocess.run([
        "cargo", f"+{args.toolchain}", "build", "--lib", "--locked", "--release",
        "--target", "wasm32-unknown-unknown", "--features", "updates",
    ], cwd=BENCH, env=env, check=True)
    package = output / "pkg"
    subprocess.run([
        args.wasm_bindgen, str(target / "wasm32-unknown-unknown/release/ironrdp_web_dirty_region_bench.wasm"),
        "--target", "web", "--out-dir", str(package),
    ], check=True)
    manifest["artifacts"] = {path.name: digest(path) for path in sorted(package.iterdir()) if path.is_file()}
    (output / "build-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Built update workloads: {output}")


if __name__ == "__main__":
    main()
