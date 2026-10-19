#!/usr/bin/env python3
"""Build historical implementations or current-SIMD/candidate modules independently."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess


HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
REPO = BENCH.parents[3]
BUNDLED_BASELINE = HERE / "baseline"
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


def source_hashes(directory):
    return {name: digest(directory / name) for name in ("image.rs", "canvas.rs")}


def verify_hashes(actual, expected, description):
    if actual != expected:
        raise RuntimeError(f"{description} hashes do not match baseline/manifest.json: {actual}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=BENCH / "output/simd-presentation-20261019")
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--wasm-bindgen", default=shutil.which("wasm-bindgen"))
    parser.add_argument("--mode", choices=("historical", "direct"), default="historical",
                        help="historical scalar/SIMD/fused, or current SIMD versus working-tree candidate")
    parser.add_argument("--baseline", help="override the bundled baseline with a compatible Git revision")
    parser.add_argument("--toolchain", default="1.94.1", help="explicit rustup toolchain; ignores ambient override")
    args = parser.parse_args()
    if not args.wasm_bindgen:
        parser.error("supply --wasm-bindgen matching the locked wasm-bindgen crate")
    bundled = None
    if args.baseline is None:
        bundled = json.loads((BUNDLED_BASELINE / "manifest.json").read_text())
        if bundled["schema_version"] != 1:
            raise RuntimeError("unsupported bundled baseline manifest version")
        verify_hashes(source_hashes(BUNDLED_BASELINE), bundled["variants"]["scalar"]["sources"],
                      "bundled scalar source")
        baseline_variant = "simd" if args.mode == "direct" else "scalar"
        baseline_revision = bundled["variants"][baseline_variant]["revision"]
    else:
        baseline_revision = subprocess.check_output(
            ["git", "rev-parse", args.baseline], cwd=REPO, text=True).strip()
    variants = ["baseline", "candidate"] if args.mode == "direct" else ["scalar", "simd", "fused"]
    output = args.output.resolve()
    target = (args.target_dir or output / "target").resolve()
    output.mkdir(parents=True, exist_ok=True)
    manifest = {
        "baseline": args.baseline or baseline_revision,
        "baseline_revision": baseline_revision,
        "baseline_origin": "bundled" if bundled else "git",
        "bundled_baseline": {
            "manifest_sha256": digest(BUNDLED_BASELINE / "manifest.json"),
            **bundled,
        } if bundled else None,
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
            elif bundled:
                content = (BUNDLED_BASELINE / filename).read_bytes()
            else:
                content = subprocess.check_output(
                    ["git", "show", f"{baseline_revision}:crates/ironrdp-web/src/{filename}"], cwd=REPO)
            (sources / filename).write_bytes(content)
        production_sources = source_hashes(sources)
        patch_variant = None
        if args.mode == "historical" and variant != "scalar":
            patch_variant = variant
        elif bundled and variant == "baseline":
            patch_variant = "simd"
        patch_sha256 = None
        if patch_variant:
            patch = HERE / f"{patch_variant}.patch"
            patch_sha256 = digest(patch)
            if bundled:
                verify_hashes(patch_sha256, bundled["variants"][patch_variant]["patch_sha256"],
                              f"{patch_variant} patch")
            subprocess.run(["patch", "--batch", "--forward", "--fuzz=0", "-p1", "-d", str(sources),
                            "-i", str(patch)], check=True)
        if bundled and variant != "candidate":
            source_variant = "simd" if variant == "baseline" else variant
            verify_hashes(source_hashes(sources), bundled["variants"][source_variant]["sources"],
                          f"bundled {source_variant} source")
        adapter_sha256 = None
        if variant == "baseline":
            # The bundled direct baseline is generated from scalar sources; record its
            # actual production bytes after the SIMD patch and before any adapter.
            production_sources = source_hashes(sources)
            canvas_source = sources / "canvas.rs"
            if not re.search(r"\bfn\s+draw_image\s*\(", canvas_source.read_text()):
                canvas_source.write_text(canvas_source.read_text() + BASELINE_ADAPTER)
                adapter_sha256 = hashlib.sha256(BASELINE_ADAPTER.encode()).hexdigest()
        env = dict(os.environ, CARGO_TARGET_DIR=str(target), CARGO_BUILD_BUILD_DIR=str(target / "build"),
                   RUSTFLAGS=FLAGS, IRONRDP_BENCH_VARIANT=variant,
                   IRONRDP_BENCH_EXPECT_SIMD128="1",
                   IRONRDP_BENCH_IMAGE_SOURCE=str(sources / "image.rs"),
                   IRONRDP_BENCH_CANVAS_SOURCE=str(sources / "canvas.rs"))
        # Cargo gives this variable precedence over RUSTFLAGS. The recorded flags
        # must be the flags used by every variant, regardless of the parent shell.
        env.pop("CARGO_ENCODED_RUSTFLAGS", None)
        subprocess.run(["cargo", f"+{args.toolchain}", "build", "--lib", "--locked", "--release", "--target", "wasm32-unknown-unknown",
                        "--features", "presentation"], cwd=BENCH, env=env, check=True)
        package = output / "pkg" / variant
        subprocess.run([args.wasm_bindgen,
                        str(target / "wasm32-unknown-unknown/release/ironrdp_web_dirty_region_bench.wasm"),
                        "--target", "web", "--out-dir", str(package)], check=True)
        manifest["variants"][variant] = {
            "production_sources": production_sources,
            "sources": source_hashes(sources),
            "artifacts": {p.name: digest(p) for p in sorted(package.iterdir()) if p.is_file()},
            "patch_sha256": patch_sha256,
            "adapter_sha256": adapter_sha256,
            "source_origin": "working tree" if variant == "candidate" else baseline_revision,
        }
        (output / "build-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Built all variants: {output}")


if __name__ == "__main__":
    main()
