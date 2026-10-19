# Presentation implementation comparison

See [RESULTS.md](RESULTS.md) for the implementation comparison and the final
full-screen confirmation using the repository's Rust 1.94.1.

This experiment compares three independently compiled implementations:

- `scalar`: capped extraction and scalar alpha normalization at `ea9c379b`.
- `simd`: the same extraction, followed by a SIMD alpha pass in `Canvas::draw`.
- `fused`: copy pixels and normalize alpha in one SIMD pass during extraction;
  `Canvas::draw` then constructs `ImageData` and calls `putImageData`.

`build.py` reads the pinned baseline with `git show` and applies `simd.patch` or
`fused.patch` to ignored source snapshots. It uses the application WASM flags,
release size optimization and LTO. Each build calls its own `extract_partial_image`
and `Canvas::draw` directly, without a policy switch in the timed path. The extra
Rust and JavaScript exports live only in this auxiliary benchmark crate. Source,
harness, compiler and package hashes are recorded in `build-manifest.json`;
the browser runner verifies the package hashes before launching Chromium.

## Run

From the parent `dirty-region` directory, with Rust 1.94.1, wasm-bindgen-cli
0.2.126, Python Playwright, Chromium and `patch` installed:

```sh
python3 presentation/build.py --wasm-bindgen /path/to/wasm-bindgen \
    --output output/presentation-current
python3 presentation/compare.py --chromium /path/to/chromium \
    --build-dir output/presentation-current \
    --include-prepare --output output/presentation-current/measurements
python3 presentation/summarize.py output/presentation-current/measurements
```

`build.py` defaults to an isolated target directory under its output directory.
It explicitly selects Rust 1.94.1; `--toolchain 1.94.0` reproduces the compiler
used for the recorded measurements. The initial run inherited an environment
override, which is recorded in its manifest and confirmed in WASM producer metadata.
Use `--target-dir` to reuse the cache for this physical benchmark workspace;
do not share it with a different checkout containing the same path packages.
Finish builds and other CPU-heavy jobs before timing. Results must go into a
new or empty directory. `--correctness-only` runs only the browser checks.
To reproduce the final full-screen confirmation, omit `--include-prepare` and
add `--case 1920x1080/1920x1080`. The earlier all-case comparison used Rust 1.94.0;
its captured manifest retains the pre-fix builder/source hashes. The final source
only removes a lint expectation from the measured SIMD pixel implementation.
Manifests are retained as captured. The final patch files use zero-context diffs
to pass whitespace checks; applying them reproduces the recorded Rust 1.94.1
candidate source hashes exactly.

## Measurements and controls

The default run has four rounds, each containing scalar/SIMD, scalar/fused, and
scalar/scalar A/A sessions. Each session starts a fresh Chromium process.
Module load order and mode order are independently counterbalanced across the
four rounds; comparison and case order also rotate. A/A uses separate WASM
instances of the same binary, just as A/B uses separate candidate/baseline
instances. It measures order and instance variability rather than a code change.

Both variants share the same timing Canvas context. Each mode has at least
400 ms of measured warmup per variant, then twelve alternating ABBA/BAAB blocks.
Batch counts are even, calibrated on scalar to at least 16 ms, and reused across
variants and subsequent sessions. Animation-frame waits occur outside the timer.
The variants alternate between two nonuniform framebuffers that differ only in
the declared dirty region. Allocation and fixture creation occur before timing.

- `draw`: extraction, alpha normalization, `ImageData`, and `putImageData`, batched
  within WASM.
- `draw_js`: the same work with one JavaScript-to-WASM call per update, for the
  full-screen case.
- `prepare` (optional): extraction only. **This is unequal work:** fused extraction
  also makes pixels opaque, while scalar/SIMD extraction preserves source alpha.
  Use `draw`, not `prepare`, to choose the implementation.

These are synchronous submission measurements on synthetic frames. They exclude
decoding, compositor completion, network behavior, input latency and frame pacing.
Host-wide CPU counters and load averages are recorded around sessions and cases;
they include the benchmark itself and cannot attribute load to particular jobs.
Use independent session estimates for inference, rather than treating repeated
batches in one browser as independent experiments.

## Correctness

Before timing, the runner compares all RGBA bytes for initial full repaint,
successive partial updates, narrow odd-width regions, sparse first updates and
resizes. Both exact rectangles and widened whole rows are covered. The tests
check that the original framebuffer bytes remain unchanged. The comparison
baseline already contains the dirty-region cap, so sparse Canvas behavior is
expected to match exactly among these three variants.

Actual WASM helper probes test offsets 0–15 and lengths 0–67, including prefix and
suffix preservation, RGB values and incomplete final pixels. Fused copy probes
independently vary source and destination offsets and check source immutability.
Correctness canvases are separate from timing canvases. Each timing canvas is
read back only after its final sample, and must match the expected final frame.
