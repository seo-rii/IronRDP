#!/usr/bin/env python3
"""Build independent scalar/SIMD/fused modules, without timed policy dispatch."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess


HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
REPO = BENCH.parents[3]
BASELINE = "ea9c379b9e0b5bcf59cc531f4b97316c24040cf7"
FLAGS = ('-Ctarget-feature=+simd128,+bulk-memory --cfg getrandom_backend="wasm_js" '
         '-Copt-level=s -Ccodegen-units=1 -Cllvm-args=-enable-dfa-jump-thread')


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=BENCH / "output/simd-presentation-20261019")
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--wasm-bindgen", default=shutil.which("wasm-bindgen"))
    parser.add_argument("--baseline", default=BASELINE)
    parser.add_argument("--toolchain", default="1.94.1", help="explicit rustup toolchain; ignores ambient override")
    args = parser.parse_args()
    if not args.wasm_bindgen:
        parser.error("supply --wasm-bindgen matching the locked wasm-bindgen crate")
    output = args.output.resolve()
    target = (args.target_dir or output / "target").resolve()
    output.mkdir(parents=True, exist_ok=True)
    manifest = {
        "baseline": args.baseline,
        "rustc": subprocess.check_output(["rustc", f"+{args.toolchain}", "-Vv"], cwd=BENCH, text=True),
        "wasm_bindgen": subprocess.check_output([args.wasm_bindgen, "--version"], text=True).strip(),
        "rustflags": FLAGS,
        "profile": "release: opt-level=s, lto=true; no wasm-opt",
        "harness_sha256": {name: digest(BENCH / name) for name in [
            "Cargo.toml", "Cargo.lock", "build.rs", "policy.rs", "src/lib.rs",
            "src/presentation.rs", "presentation/build.py"]},
        "variants": {},
    }
    for variant in ["scalar", "simd", "fused"]:
        sources = output / "variants" / variant
        sources.mkdir(parents=True, exist_ok=True)
        for filename in ["image.rs", "canvas.rs"]:
            content = subprocess.check_output(
                ["git", "show", f"{args.baseline}:crates/ironrdp-web/src/{filename}"], cwd=REPO)
            (sources / filename).write_bytes(content)
        if variant != "scalar":
            subprocess.run(["patch", "--batch", "--forward", "-p1", "-d", str(sources),
                            "-i", str(HERE / f"{variant}.patch")], check=True)
        env = dict(os.environ, CARGO_TARGET_DIR=str(target), CARGO_BUILD_BUILD_DIR=str(target / "build"),
                   RUSTFLAGS=FLAGS, IRONRDP_BENCH_VARIANT=variant,
                   IRONRDP_BENCH_IMAGE_SOURCE=str(sources / "image.rs"),
                   IRONRDP_BENCH_CANVAS_SOURCE=str(sources / "canvas.rs"))
        subprocess.run(["cargo", f"+{args.toolchain}", "build", "--lib", "--locked", "--release", "--target", "wasm32-unknown-unknown",
                        "--features", "presentation"], cwd=BENCH, env=env, check=True)
        package = output / "pkg" / variant
        subprocess.run([args.wasm_bindgen,
                        str(target / "wasm32-unknown-unknown/release/ironrdp_web_dirty_region_bench.wasm"),
                        "--target", "web", "--out-dir", str(package)], check=True)
        manifest["variants"][variant] = {
            "sources": {f: digest(sources / f) for f in ["image.rs", "canvas.rs"]},
            "artifacts": {p.name: digest(p) for p in sorted(package.iterdir()) if p.is_file()},
            "patch_sha256": None if variant == "scalar" else digest(HERE / f"{variant}.patch"),
        }
        (output / "build-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Built all variants: {output}")


if __name__ == "__main__":
    main()
