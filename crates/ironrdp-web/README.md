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

Dirty regions are intersected with the framebuffer before selecting a copy path
or calculating pixel offsets. Partly overlapping regions draw their visible
portion; regions outside the framebuffer or with reversed bounds do not upload
pixels.

After clipping, the upload coverage is selected once. Whole rows are selected
only within the existing 25% overdraw limit. The final coverage determines whether
to combine copying and alpha normalization for uploads of at least 256 KiB;
expanded whole-row uploads use the same rule as originally full-width updates.
The copy step preserves that selected coverage and does not apply the expansion
policy again. Raw image extraction continues to preserve source alpha.

## Browser pixel correctness

The [Web pixel correctness workflow](../../.github/workflows/web-pixel-correctness.yml)
checks the PR head commit in Chromium. It builds the current production
`image.rs` and `canvas.rs` into an auxiliary WASM package twice, with `simd128`
explicitly disabled and enabled. Build guards and an exported WASM feature probe
verify which implementation each package compiled. No historical Git revision
or benchmark patch is needed.

The checks cover full and partial updates, whole-row expansion and fusion
thresholds for both full-width and expanded uploads, unaligned pixel-copy buffers,
incomplete trailing pixels, sparse first updates, resizes, out-of-bounds update
clipping, source immutability, and equality of the resulting Canvas pixels. Bounds checks cross clipped widths
of 1535/1536/1537 pixels with 34/35-row uploads and retain fully outside updates as
no-ops before expansion. CI retains commit IDs, source/package hashes, browser
information and correctness results as artifacts.
It runs no performance measurements and does not emulate a browser lacking SIMD
support.

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

## Canvas presentation

All RDP updates are applied to the framebuffer. Canvas presentation combines
pending regions and targets 60 FPS, with an immediate first update. Duplicate
regions and regions fully contained in another pending region need only one draw;
disjoint and partially overlapping regions retain their original coverage.
The queue holds at most 64 regions and flushes before overflowing.

Only pending draws schedule a presentation timer. Additional updates do not
restart its deadline. Reactivation flushes the old framebuffer before replacing
it and resets the first-update deadline. Termination flushes the final pending
regions; errors attempt the same flush while preserving the original error.
These lifecycle and capacity flushes can occur before the normal frame deadline,
so 60 FPS is a presentation target rather than a strict Canvas-call limit.

The session yields to browser tasks after about 8 ms of accumulated processing
work, using a reusable `MessageChannel`. Network waits do not consume that budget.
Each PDU and presentation batch remains atomic, so this does not bound individual
long tasks or guarantee a maximum input latency. Protocol responses and cursor
callbacks do not wait for the presentation timer.

Synthetic correctness and performance checks live in
[`benches/dirty-region/updates`](benches/dirty-region/updates/README.md).


[IronRDP]: https://github.com/Devolutions/IronRDP
