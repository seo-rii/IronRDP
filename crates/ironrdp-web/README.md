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

All RDP updates are applied to the framebuffer. By default, Canvas presentation
coalesces duplicate and contained regions from the same PDU and flushes at its
end. Disjoint and partially overlapping regions retain their original coverage.
Regions are clipped before entering the queue; invisible and reversed bounds do
not create presentation deadlines. The queue holds at most 64 regions and
flushes before overflowing.

A flush attempts every queued region even if an earlier draw failed, clears the
scratch buffer after every attempt, and returns the first error. The overflowing
region is queued even when that capacity flush failed. The remaining graphics
outputs from the same, already decoded PDU continue through the bounded queue,
including any further capacity flushes; final cleanup attempts the last batch.
The first drawing error is retained, with no retries or further non-graphics
side effects after that failure. Termination and errors perform a best-effort
final flush; an existing protocol/input error takes precedence over a final
drawing error.

Reactivation flushes the old framebuffer and cancels its timer before the
activation exchange. After negotiation, both the Canvas and framebuffer use the
server's final dimensions, including server-initiated or adjusted sizes. A new
framebuffer generation clears the Canvas even when its dimensions are unchanged,
resets pacing, and presents its first visible update immediately. `desktopSize()`
reports the latest negotiated dimensions. The optional `canvasResizedCallback`
runs after initial Canvas setup and each negotiated replacement, including an
equal-size reactivation; it sees the new Canvas and desktop dimensions so the UI
can recompute its scaling. A thrown callback error terminates the run.
The activation
exchange still awaits server traffic without selecting input events, so shutdown
requested during an unanswered activation exchange remains deferred. This change
does not add an activation timeout or restructure that protocol state machine.

Cross-PDU pacing and cooperative yielding are **disabled by default**. The Cargo
feature `experimental-presentation-pacing` explicitly selects a 60 FPS target and
an 8 ms processing budget for live-session experiments. No JavaScript API changes
are required. For an experimental WASM build, add
`--features experimental-presentation-pacing` to `wasm-pack build --target web`,
using the SIMD flags described above if desired. `PresentationConfig` keeps the
presentation interval and yield budget as separate internal options.

Only pending visible draws schedule a timer, and later updates do not restart
its deadline. Lifecycle and capacity flushes can occur before the deadline, so
60 FPS is a target rather than a strict Canvas-call limit. Yielding uses a reusable
`MessageChannel` after the configured amount of **post-read processing**: RDP
decode, output handling and Canvas work after a read has completed. Network waits,
CPU work performed while polling/framing reads, and activation work are excluded.
A PDU and a presentation batch remain atomic; the budget does not bound long tasks
or guarantee input latency. Responses and cursor callbacks remain immediate.

Trace events report presentation reason, framebuffer generation, attempted and
failed regions, successfully uploaded bytes, batch duration and oldest pending
age. Optional yield events report the accumulated post-read processing time.
Timing claims must state whether trace logging was enabled; the recorded synthetic
benchmarks predate these events and do not measure the full Session loop.

Synthetic checks and archived measurements live in
[`benches/dirty-region/updates`](benches/dirty-region/updates/README.md). The earlier
paced unique-tile workload took longer to finish, so pacing remains opt-in and
this change does not claim to resolve live-video backlog or input latency.

[IronRDP]: https://github.com/Devolutions/IronRDP
