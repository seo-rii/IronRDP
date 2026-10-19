#!/usr/bin/env python3
"""Build historical implementations or current-SIMD/candidate modules independently."""
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
DIRECT_BASELINE = "289f710ed07790f3691d4edcd188294f9bef9251"
FLAGS = ('-Ctarget-feature=+simd128,+bulk-memory --cfg getrandom_backend="wasm_js" '
         '-Copt-level=s -Ccodegen-units=1 -Cllvm-args=-enable-dfa-jump-thread')
BASELINE_ADAPTER = """

// Benchmark-only adapter for the current SIMD implementation's existing API.
impl Canvas {
    pub(crate) fn draw_image(
        &self,
        image: &ironrdp::session::image::DecodedImage,
        region: ironrdp::pdu::geometry::InclusiveRectangle,
        buffer: &mut ironrdp_core::WriteBuf,
    ) -> anyhow::Result<()> {
        let region = super::image::extract_partial_image(image, region, buffer);
        self.draw(buffer.filled_mut(), region)
    }
}
"""


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=BENCH / "output/simd-presentation-20261019")
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--wasm-bindgen", default=shutil.which("wasm-bindgen"))
    parser.add_argument("--mode", choices=("historical", "direct"), default="historical",
                        help="historical scalar/SIMD/fused, or current SIMD versus working-tree candidate")
    parser.add_argument("--baseline", help="Git baseline; defaults to ea9c379b historically or 289f710e in direct mode")
    parser.add_argument("--toolchain", default="1.94.1", help="explicit rustup toolchain; ignores ambient override")
    args = parser.parse_args()
    if not args.wasm_bindgen:
        parser.error("supply --wasm-bindgen matching the locked wasm-bindgen crate")
    args.baseline = args.baseline or (DIRECT_BASELINE if args.mode == "direct" else BASELINE)
    variants = ["baseline", "candidate"] if args.mode == "direct" else ["scalar", "simd", "fused"]
    output = args.output.resolve()
    target = (args.target_dir or output / "target").resolve()
    output.mkdir(parents=True, exist_ok=True)
    manifest = {
        "baseline": args.baseline,
        "baseline_revision": subprocess.check_output(
            ["git", "rev-parse", args.baseline], cwd=REPO, text=True).strip(),
        "mode": args.mode,
        "comparison_baseline": variants[0],
        "rustc": subprocess.check_output(["rustc", f"+{args.toolchain}", "-Vv"], cwd=BENCH, text=True),
        "wasm_bindgen": subprocess.check_output([args.wasm_bindgen, "--version"], text=True).strip(),
        "rustflags": FLAGS,
        "profile": "release: opt-level=s, lto=true; no wasm-opt",
        "harness_sha256": {name: digest(BENCH / name) for name in [
            "Cargo.toml", "Cargo.lock", "build.rs", "policy.rs", "src/lib.rs",
            "src/presentation.rs", "presentation/build.py"]},
        "variants": {},
    }
    for variant in variants:
        sources = output / "variants" / variant
        sources.mkdir(parents=True, exist_ok=True)
        for filename in ["image.rs", "canvas.rs"]:
            if variant == "candidate":
                content = (REPO / "crates/ironrdp-web/src" / filename).read_bytes()
            else:
                content = subprocess.check_output(
                    ["git", "show", f"{args.baseline}:crates/ironrdp-web/src/{filename}"], cwd=REPO)
            (sources / filename).write_bytes(content)
        production_sources = {filename: digest(sources / filename) for filename in ["image.rs", "canvas.rs"]}
        if args.mode == "historical" and variant != "scalar":
            subprocess.run(["patch", "--batch", "--forward", "-p1", "-d", str(sources),
                            "-i", str(HERE / f"{variant}.patch")], check=True)
        if variant == "baseline":
            canvas_source = sources / "canvas.rs"
            canvas_source.write_text(canvas_source.read_text() + BASELINE_ADAPTER)
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
            "production_sources": production_sources,
            "sources": {f: digest(sources / f) for f in ["image.rs", "canvas.rs"]},
            "artifacts": {p.name: digest(p) for p in sorted(package.iterdir()) if p.is_file()},
            "patch_sha256": digest(HERE / f"{variant}.patch") if variant in ("simd", "fused") else None,
            "adapter_sha256": hashlib.sha256(BASELINE_ADAPTER.encode()).hexdigest()
                              if variant == "baseline" else None,
            "source_origin": "working tree" if variant == "candidate" else args.baseline,
        }
        (output / "build-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Built all variants: {output}")


if __name__ == "__main__":
    main()
