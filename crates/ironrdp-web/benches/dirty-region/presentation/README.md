# Presentation implementation comparison

See [RESULTS.md](RESULTS.md) for the implementation comparison and the final
full-screen confirmation using the repository's Rust 1.94.1.
See [LARGE_COPY_RESULTS.md](LARGE_COPY_RESULTS.md) for the subsequent direct
comparison of conditional full-width copy/alpha fusion against that SIMD baseline.
See [EXPANDED_ROWS_RESULTS.md](EXPANDED_ROWS_RESULTS.md) for fusion applied after
selecting the upload rectangle, compared directly with `a5682c16`.

This experiment compares three independently compiled implementations:

- `scalar`: capped extraction and scalar alpha normalization at `ea9c379b`.
- `simd`: the same extraction, followed by a SIMD alpha pass in `Canvas::draw`.
- `fused`: copy pixels and normalize alpha in one SIMD pass during extraction;
  `Canvas::draw` then constructs `ImageData` and calls `putImageData`.

These remain the default historical comparison. The `direct` build mode below
instead compares the current SIMD baseline with the working-tree implementation.

`build.py` reads the frozen `ea9c379b` sources in [`baseline/`](baseline/) and applies
`simd.patch` or `fused.patch` to ignored source snapshots. The defaults do not read
Git history: shallow clones, squash merges and source archives can reproduce both
historical and direct comparisons without the original fork's intermediate commits.
[`baseline/manifest.json`](baseline/manifest.json) records the source repository,
original revisions, SHA-256 hashes of the frozen sources, patches and generated
variants. The builder verifies the hashes before compiling each variant. It uses the application WASM flags,
release size optimization and LTO. Each build calls its own `extract_partial_image`
and `Canvas::draw` directly, without a policy switch in the timed path. The extra
Rust and JavaScript exports live only in this auxiliary benchmark crate. Source,
harness, compiler and package hashes are recorded in `build-manifest.json`;
the browser runner verifies the package hashes before launching Chromium.

## Run

Run the browser measurements on **Linux**: `compare.py` reads `/proc/stat` and
host load averages for its CPU-load controls. Windows and macOS are not supported
by this runner. These Linux measurements do not substitute for profiling a
deployed Windows browser with hardware acceleration.

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
It removes inherited `CARGO_ENCODED_RUSTFLAGS`, which otherwise takes precedence
over the recorded `RUSTFLAGS`, and requires the WASM build to report `simd128` as
enabled. This includes the historical `scalar` variant: its alpha loop remains
scalar source code, compiled with the same application flags as the other variants.
The separate [`ci.py`](ci.py) correctness check builds current production sources
with SIMD both enabled and disabled and verifies the loaded modules' feature probes;
see the [crate README](../../../README.md) for the CI and local command.
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

## Direct comparison against current SIMD

Use a new output directory to compare the SIMD implementation at `289f710e` with
the current working-tree `image.rs` and `canvas.rs`. The bundled direct baseline
is reconstructed by applying `simd.patch` to the frozen scalar sources. Its
resulting bytes are checked against the hashes of the original `289f710e` files:

```sh
python3 presentation/build.py --mode direct --wasm-bindgen /path/to/wasm-bindgen \
    --output output/presentation-direct
python3 presentation/compare.py --chromium /path/to/chromium \
    --build-dir output/presentation-direct \
    --comparison candidate --comparison aa \
    --case 1920x1080/1920x1080 --case 1920x1080/64x720 \
    --case 1920x1080/32x32 --case 1920x1080/17x65 \
    --output output/presentation-direct/measurements
python3 presentation/summarize.py output/presentation-direct/measurements
```

The packages and reports call these implementations `baseline` and `candidate`;
the baseline already includes SIMD alpha normalization. Each of four rounds runs
candidate/baseline and baseline/baseline A/A in separate Chromium processes,
with the same counterbalancing and timing controls as the historical experiment.
`--comparison` is optional and repeatable: by default the runner compares every
built candidate and A/A. `--case` remains repeatable, so small full-width strips,
odd framebuffer widths, and sizes on either side of an implementation threshold
can be added without changing the harness. Every selected case is also checked
for RGBA equality and source immutability before measurement.

Both direct builds call `Canvas::draw_image`. If the baseline lacks that method,
the builder appends a benchmark adapter that calls its existing
`extract_partial_image` and `Canvas::draw`. Baselines already providing the method
and the candidate call their real implementations. No runtime
policy dispatch is added to either timed path. The manifest records production
source hashes before adaptation, compiled source hashes, and the adapter hash.
It still pins Rust 1.94.1. The manifest also records whether the baseline came
from the bundle or Git, along with the bundled provenance manifest and its hash.
An explicit `--baseline <revision>` selects a compatible revision from the local
Git history instead; only this override requires fetching that revision first.
Historical overrides must accept the existing patches, while direct overrides
must provide the pixel-processing APIs used by the harness. Finish editing the candidate before building it; builds
snapshot the source into the ignored output directory.

The direct candidate's copy probe calls `canvas::copy_opaque_rgba` in actual WASM,
independently varying source and destination offsets. Alpha probes call each
build's real `make_opaque`. The baseline copy probe uses copy plus its existing
SIMD alpha pass. These test RGB values, complete and incomplete pixels, surrounding
bytes and unchanged source data. `prepare` in direct mode still measures raw
extraction; it excludes the optimized presentation path and is not a selection
metric. Select using complete `draw` and, for full frames, `draw_js` measurements.

To compare the current expanded-row implementation with its PR #1 base, pass
`--baseline 890620691d36b22dd54ac77e7b255018cc3f683d` to the direct build command.
The retained historical measurements used `a5682c16` and the source hashes in
their manifest; they do not describe the latest head after clipping and copy-path
changes. New measurements require fresh builds of both selected revisions.
The case matrix in [EXPANDED_ROWS_RESULTS.md](EXPANDED_ROWS_RESULTS.md) includes
the 80% width boundary, selected uploads immediately below and above 256 KiB,
odd framebuffer widths, and 4K. Previously optimized full frames and compact
regions serve as controls. Do not combine improvement percentages measured
against different baselines.

## Current-source correctness in CI

From the parent `dirty-region` directory, run:

```sh
python3 presentation/ci.py --expected-head "$(git rev-parse HEAD)" \
    --wasm-bindgen /path/to/wasm-bindgen \
    --output output/pixel-correctness
```

This requires a clean tracked checkout and a new output directory. It builds the
current production files twice, with `simd128` explicitly disabled and enabled,
and calls the real `Canvas::draw_image` and copy/alpha helpers in both modules.
The build guard and the loaded modules must agree on `[false, true]` SIMD support.
No historical revision or source adapter is used. It checks sparse initialization,
resize, boundary cases and all RGBA bytes, without running performance timings.
The current bounds matrix includes updates completely outside the framebuffer
before expansion and clipped widths of 1535/1536/1537 pixels crossed with
34/35-row uploads. Each runs with sparse and initialized canvases in both builds;
sparse source frames include transparent borders. The CI guard requires every
case to report unchanged source pixels and the exact upload buffer length.
The expanded correctness evaluation has a 300-second limit for full RGBA
readbacks in software Canvas. Preparation and performance evaluations retain
their separate 120-second limits; this does not change measurement durations.
The report records the checked commit, source and artifact hashes, compiler,
browser and observed feature flags. Use `--target-dir` for an isolated reusable
cache, as for `build.py`. The Web pixel correctness workflow runs this command
for the exact requested head commit and preserves the reports as CI artifacts.

## Measurements and controls

The historical default run has four rounds, each containing scalar/SIMD, scalar/fused, and
scalar/scalar A/A sessions. Each session starts a fresh Chromium process.
Module load order and mode order are independently counterbalanced across the
four rounds; comparison and case order also rotate. A/A uses separate WASM
instances of the same binary, just as A/B uses separate candidate/baseline
instances. It measures order and instance variability rather than a code change.

Both variants share the same timing Canvas context. Each mode has at least
400 ms of measured warmup per variant, then twelve alternating ABBA/BAAB blocks.
Batch counts are even, calibrated on the comparison baseline to at least 16 ms, and reused across
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
