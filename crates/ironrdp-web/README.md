# WASM bindings for web

This crate is part of the [IronRDP] project. See the [web client](../../web-client)
for the browser components and demonstration application.

## Build and SIMD

From the repository root, prepare and build the complete web client with:

```sh
cargo xtask web install
cargo xtask web build
```

`cargo xtask web build` explicitly enables WebAssembly `simd128` and `bulk-memory`
in `RUSTFLAGS`, along with the application's size optimization flags. This selects
the SIMD pixel-copy and alpha-normalization paths. The implementation is selected
at compile time: a build without `simd128` uses the scalar fallback, and remaining
pixels after each SIMD chunk are also handled by scalar code.
Unset `CARGO_ENCODED_RUSTFLAGS` before these commands if it is present in your
environment: Cargo gives it precedence over the `RUSTFLAGS` set by `xtask`.

`cargo xtask web check` invokes `wasm-pack build --dev --target web` and does not add
those SIMD flags. A direct `wasm-pack build --target web` from this crate also does
not enable SIMD by itself. Both use the Cargo configuration and any inherited
`RUSTFLAGS`; the repository's WASM configuration sets the `getrandom` backend but
does not enable `simd128`. Release versus development mode alone does not select
the SIMD implementation.

The optimized module requires a browser that supports the WebAssembly features
used by that module. The scalar fallback is a separate build configuration; it
does not let an already compiled SIMD module load on a browser without SIMD.

## Canvas pixel contract

New and resized canvases start transparent. A draw makes its uploaded pixels
opaque without changing the source framebuffer. Pixels outside the uploaded
region retain their previous Canvas values, including transparency before their
first upload. Extraction can select whole rows for sufficiently wide dirty
regions, so the uploaded region can be wider than the dirty region. The client
does not fill the entire Canvas with opaque black on initialization or resize.

## Browser pixel correctness

The [Web pixel correctness workflow](../../.github/workflows/web-pixel-correctness.yml)
checks the PR head commit in Chromium. It builds the current production
`image.rs` and `canvas.rs` into an auxiliary WASM package twice, with `simd128`
explicitly disabled and enabled. Build guards and an exported WASM feature probe
verify which implementation each package compiled. No historical Git revision
or benchmark patch is needed.

The checks cover full and partial updates, whole-row expansion and full-width
fusion thresholds, unaligned pixel-copy buffers, incomplete trailing pixels, sparse first updates,
resizes, source immutability, and equality of the resulting Canvas pixels. CI
retains commit IDs, source/package hashes, browser information and correctness
results as artifacts. It runs no performance measurements and does not emulate
a browser lacking SIMD support.

To run the same check locally on Linux, install the repository Rust toolchain's
`wasm32-unknown-unknown` target, `wasm-bindgen-cli` 0.2.126, Python 3.11+, Playwright and
Chromium, then run from the repository root:

```sh
python3 crates/ironrdp-web/benches/dirty-region/presentation/ci.py \
    --output crates/ironrdp-web/benches/dirty-region/output/pixel-correctness-local
```

The output directory must be new or empty. `--target-dir` can reuse a build cache
belonging to this physical benchmark workspace. `--expected-head` additionally
requires an unchanged checkout of the given full Git SHA, as in CI; local working
tree checks otherwise record their source hashes and whether they match `HEAD`.
The browser runner currently requires Linux for its `/proc/stat` host counters.

[IronRDP]: https://github.com/Devolutions/IronRDP
