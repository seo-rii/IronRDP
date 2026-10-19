# Dirty-region extraction and Canvas benchmark

This auxiliary crate measures the real Rust extraction helpers and the production
`Canvas::draw` implementation without exporting benchmark APIs from `ironrdp-web`.
`build.rs` copies `src/image.rs` unchanged into `OUT_DIR` and appends `policy.rs` in
the same module so it can call the private helpers. It also copies `src/canvas.rs`
unchanged into `OUT_DIR`. Source hashes accompany the browser results.

See [the measured results](RESULTS.md) for the cap comparison and regression checks.
The [presentation comparison](presentation/README.md) separately compares scalar
alpha normalization, SIMD alpha normalization, and fused SIMD copying/alpha
normalization using independent WASM builds and interleaved A/A controls.

The policies are the original height/width heuristic, expansion caps of 1.25x,
1.5x and 2x, always exact, always whole rows, and the current production policy.
Synthetic nonuniform framebuffers are populated through the public bitmap API;
two frames alternate only the declared dirty pixels during batches. Fixture generation and decoding
are outside the measured interval. These are synthetic rendering measurements,
not an RDP replay or a measurement of video/input latency.

## Run

Run the following from this directory. Use an isolated Cargo target directory if
another checkout contains the same package names and versions. The standalone
lockfile pins benchmark dependencies without changing the application lockfile.
Python's `playwright` package and a Chromium installation are prerequisites;
`--chromium /path/to/chromium` can select an existing installation.

```sh
export CARGO_TARGET_DIR="$PWD/target"
mkdir -p output
cargo +1.94.1 build --release --locked -j 4
# Run the binary after other compiles/benchmarks have stopped.
"$CARGO_TARGET_DIR/release/ironrdp-web-dirty-region-bench" 20 > output/native.csv

RUSTFLAGS='-Ctarget-feature=+simd128,+bulk-memory --cfg getrandom_backend="wasm_js" -Copt-level=s -Ccodegen-units=1 -Cllvm-args=-enable-dfa-jump-thread' \
    cargo +1.94.1 build --lib --release --locked --target wasm32-unknown-unknown -j 4
wasm-bindgen --target web --out-dir pkg \
    "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/ironrdp_web_dirty_region_bench.wasm"
python3 browser.py --samples 20 --output output/chromium.json
```

Use `wasm-bindgen-cli` 0.2.126, matching the root application lockfile and this
crate's bindings. The release profile uses `opt-level = "s"` and LTO, matching the
application's `production-wasm` profile; the flags above match `xtask web build`.
The native benchmark uses the same size optimization profile. No `wasm-opt` pass
is applied by these commands.

Each policy is calibrated to batches of at least 8 ms, warmed again, then sampled
20 times in a deterministic shuffled order. The native CSV retains all samples;
the browser JSON retains all samples and median/p10/p90 summaries. Browser
`wasm_copy` measures extraction and buffer clearing. `copy_and_canvas` additionally
includes the production alpha normalization, `ImageData` construction and
`putImageData` call. Timings cover synchronous submission, not compositor
presentation or GPU completion. The runner uses headless Chromium, a 1920x1080
viewport and device scale factor 1. Buffer allocation is warmed before timing.
Animation-frame waits between timed batches are outside the clock. Timing
canvases are separate from correctness canvases, with one final whole-frame
readback after timing; repeated readbacks cannot induce Chromium's software
fallback during measurement.

## Screen regression checks

Before timing, every policy must match the full expected RGBA framebuffer after
an initial full update and three partial updates, at 320x240 and after a resize
to 640x360 (56 full byte comparisons). A separate sparse-first-update fixture
uses an untouched transparent canvas, both initially and after resize. It checks
dirty bytes and RGB equality, records alpha differences outside the dirty region,
and requires full byte equality after a subsequent full repaint. It deliberately
does not initialize sparse-test canvas backgrounds to opaque black.

The sparse fixture documents a consequence of removing incidental overdraw:
legacy whole-row uploads force untouched pixels in those rows opaque, whereas an
exact upload leaves those canvas pixels transparent. Consequently, whole-canvas
RGBA equality is claimed only once the framebuffer has received a full repaint.
This benchmark does not change the canvas initialization or resize behavior.

Run only regression checks with:

```sh
python3 browser.py --correctness-only --output output/correctness.json
```

Use `--case 1920x1080/64x720` to select a case (repeat the option for different
cases), and increase `--samples` when a result needs more measurements.
