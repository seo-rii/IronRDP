#!/usr/bin/env python3
"""Build the real web crate for Session integration checks and deployment initialization."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tomllib


HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
CRATE = BENCH.parent.parent
REPO = CRATE.parent.parent
FLAGS = ('-Ctarget-feature=+simd128,+bulk-memory --cfg getrandom_backend="wasm_js" '
         '-Copt-level=s -Ccodegen-units=1 -Cllvm-args=-enable-dfa-jump-thread')
WASM_OPT_VERSION = 133


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_hashes():
    # Include workspace dependency sources, not only the thin exported fixture.
    names = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
                                    cwd=REPO).decode().split("\0")
    paths = set()
    for name in filter(None, names):
        path = REPO / name
        if not path.is_file():
            continue
        if (path.suffix == ".rs" or path.name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml")
                or path.is_relative_to(HERE) and path.suffix in (".py", ".js", ".html", ".md")
                or name == ".github/workflows/web-session-correctness.yml"
                or name.startswith(".cargo/") and path.suffix == ".toml"):
            paths.add(path)
    return {path.relative_to(REPO).as_posix(): digest(path) for path in sorted(paths)}


def checkout_state(expected_head=None):
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip()
    clean = subprocess.run(["git", "diff", "--quiet", "HEAD", "--"], cwd=REPO, check=False)
    if clean.returncode not in (0, 1):
        raise RuntimeError("could not inspect the tracked checkout")
    other = subprocess.check_output(["git", "ls-files", "--others", "--exclude-standard", "-z"], cwd=REPO)
    untracked = set(filter(None, other.decode().split("\0"))) & set(source_hashes())
    matches = clean.returncode == 0 and not untracked
    if expected_head and (head != expected_head or not matches):
        raise RuntimeError("source checkout does not match --expected-head")
    return {"checked_commit_sha": head, "sources_match_checked_commit": matches,
            "tracked_checkout_clean": clean.returncode == 0}


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=BENCH / "output/session-correctness")
    parser.add_argument("--target-dir", type=Path, help="Cargo cache unique to this physical workspace")
    parser.add_argument("--wasm-pack", default=shutil.which("wasm-pack"))
    parser.add_argument("--wasm-opt", default=shutil.which("wasm-opt"),
                        help=f"Binaryen {WASM_OPT_VERSION} optimizer executable (defaults to PATH)")
    parser.add_argument("--toolchain", default=tomllib.loads((REPO / "rust-toolchain.toml").read_text())["toolchain"]["channel"])
    parser.add_argument("--expected-head", help="require this full commit SHA and unchanged sources")
    parser.add_argument("--deployment-smoke", action="store_true", help="also build the normal package without fixture exports")
    args = parser.parse_args()
    if not args.wasm_pack:
        parser.error("install wasm-pack or supply --wasm-pack")
    optimizer = shutil.which(args.wasm_opt) if args.wasm_opt else None
    if not optimizer:
        parser.error(f"install Binaryen {WASM_OPT_VERSION} or supply --wasm-opt")
    optimizer = Path(optimizer).resolve()
    optimizer_version = subprocess.check_output([str(optimizer), "--version"], text=True).strip()
    version_match = re.fullmatch(r"wasm-opt version (\d+)(?:\s.*)?", optimizer_version)
    if not version_match or int(version_match[1]) != WASM_OPT_VERSION:
        parser.error(f"this correctness harness pins Binaryen {WASM_OPT_VERSION}; found {optimizer_version!r}")
    if args.expected_head and not re.fullmatch(r"[0-9a-f]{40}", args.expected_head):
        parser.error("--expected-head must be a full lowercase Git SHA")
    output = args.output.resolve()
    if not output.is_relative_to(BENCH / "output"):
        parser.error("--output must be inside the benchmark output directory")
    if output.exists() and any(output.iterdir()):
        parser.error("--output must be new or empty")
    output.mkdir(parents=True, exist_ok=True)
    target = (args.target_dir or output / "target").resolve()
    manifest = {"status": "building", "started_at_utc": datetime.now(timezone.utc).isoformat(),
                "expected_head_sha": args.expected_head, "correctness_only": True, "variants": {}}
    try:
        manifest.update(checkout_state(args.expected_head))
        original = source_hashes()
        manifest.update(source_sha256=original,
                        rustc=subprocess.check_output(["rustc", f"+{args.toolchain}", "-Vv"], cwd=REPO, text=True),
                        wasm_pack=subprocess.check_output([args.wasm_pack, "--version"], text=True).strip(),
                        wasm_opt={"path": str(optimizer), "version": optimizer_version,
                                  "sha256": digest(optimizer), "pinned_version": WASM_OPT_VERSION},
                        profile="workspace release via wasm-pack, followed by its default wasm-opt step",
                        rustflags=FLAGS,
                        package_boundary="raw wasm-pack web package before xtask's Vite URL rewrite and npm build")
        metadata = tomllib.loads((CRATE / "Cargo.toml").read_text()).get("package", {}).get("metadata", {})
        optimization = metadata.get("wasm-pack", {}).get("profile", {}).get("release", {}).get("wasm-opt")
        if optimization is False:
            raise RuntimeError("release wasm-opt is disabled in the crate metadata")
        env = dict(os.environ, RUSTUP_TOOLCHAIN=args.toolchain, RUSTFLAGS=FLAGS,
                   CARGO_TARGET_DIR=str(target), CARGO_BUILD_BUILD_DIR=str(target / "build"),
                   PATH=os.pathsep.join((str(optimizer.parent), os.environ.get("PATH", ""))))
        env.pop("CARGO_ENCODED_RUSTFLAGS", None)
        variants = [("default", ["__test-session"]),
                    ("paced", ["__test-session", "experimental-presentation-pacing"])]
        if args.deployment_smoke:
            variants.append(("deployment", []))
        write_json(output / "build-manifest.json", manifest)
        for name, features in variants:
            package = output / "pkg" / name
            command = [args.wasm_pack, "build", str(CRATE), "--target", "web", "--release",
                       "--out-dir", str(package), "--out-name", "ironrdp_web", "--", "--locked"]
            if features:
                command.extend(["--features", ",".join(features)])
            log_path = output / f"build-{name}.log"
            # wasm-pack logs include whether its actual optimizer step ran. Keep full
            # output on disk instead of treating an unrelated system wasm-opt as proof.
            with log_path.open("x") as log:
                subprocess.run(command, cwd=CRATE, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
            log_text = log_path.read_text()
            if "wasm-opt" not in log_text or "Optimizing wasm binaries" not in log_text:
                raise RuntimeError(f"{name}: wasm-pack did not report its wasm-opt step; inspect {log_path}")
            optimizer_paths = re.findall(r'found wasm-opt at "([^"]+)"', log_text)
            if not optimizer_paths or any(Path(path).resolve() != optimizer for path in optimizer_paths):
                raise RuntimeError(f"{name}: wasm-pack did not select {optimizer}; inspect {log_path}")
            manifest["variants"][name] = {
                "features": features, "default_features": True, "simd128": True,
                "pacing": "experimental-presentation-pacing" in features,
                "fixture": "__test-session" in features, "command": command,
                "wasm_opt_step_reported": True, "build_log": log_path.name,
                "wasm_opt_selected_path": str(optimizer),
                "build_log_sha256": digest(log_path),
                "artifacts": {path.relative_to(package).as_posix(): digest(path)
                              for path in sorted(package.rglob("*")) if path.is_file()},
            }
            write_json(output / "build-manifest.json", manifest)
        if source_hashes() != original:
            raise RuntimeError("workspace sources or harness changed while building")
        if digest(optimizer) != manifest["wasm_opt"]["sha256"]:
            raise RuntimeError("wasm-opt executable changed while building")
        current = checkout_state(args.expected_head)
        if current["checked_commit_sha"] != manifest["checked_commit_sha"]:
            raise RuntimeError("HEAD changed while building")
        manifest["status"] = "built"
    except Exception as error:
        manifest.update(status="failed", error=str(error))
        raise
    finally:
        manifest["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
        write_json(output / "build-manifest.json", manifest)
    print(f"Built {', '.join(manifest['variants'])}: {output / 'build-manifest.json'}")


if __name__ == "__main__":
    main()
