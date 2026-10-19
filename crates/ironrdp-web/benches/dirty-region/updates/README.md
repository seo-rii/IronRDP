# Synthetic update coalescing and frame pacing

This auxiliary feature uses the real `Canvas`, `PendingRegions`, and
`FramePacing` sources. It does not replay RDP packets or measure network, video
decoding, compositor completion, or transported input latency. Historical
dirty-region and presentation benchmarks retain their existing feature modes.

[Recorded results and tradeoffs](RESULTS.md) include all case summaries, A/A
controls, timing observations and the exact measured harness revision.

Build with `updates/build.py` from the parent `dirty-region` directory. It snapshots
the production helpers, selects Rust 1.94.1, and packages the `updates` feature
with release/LTO, the application WASM flags and wasm-bindgen 0.2.126:

```sh
python3 updates/build.py --output output/updates \
    --wasm-bindgen /path/to/wasm-bindgen \
    --target-dir /path/to/this-workspace-cache \
    --build-dir /path/to/this-workspace-intermediates
```

Reuse the target and intermediate directories belonging to this physical
benchmark workspace. Without overrides, the builder uses `output/updates/target`
and its `build` subdirectory. Do not share these with another checkout.

`build.rs` accepts `IRONRDP_BENCH_IMAGE_SOURCE`, `IRONRDP_BENCH_CANVAS_SOURCE`,
`IRONRDP_BENCH_PENDING_SOURCE`, `IRONRDP_BENCH_PACING_SOURCE`, and
`IRONRDP_BENCH_YIELD_SOURCE` for reproducible
source snapshots. The defaults read the corresponding production files.
`PendingRegions` is copied under the name `pending_regions`; the generated module
adds only a queue-length accessor for instrumentation. The pacing module keeps
its `frame_pacing` name. No source policy is rewritten.

The builder records compiler, flags, source/harness SHA-256 values and package
hashes in `build-manifest.json`. The runner requires an `artifacts` object whose keys
are `ironrdp_web_dirty_region_bench.js` and
`ironrdp_web_dirty_region_bench_bg.wasm`, with their SHA-256 strings as values;
it rejects packages that do not match. Keep the package inside this benchmark
directory. For example, after building `output/updates/pkg`:

```sh
python3 updates/browser.py --package output/updates/pkg \
    --manifest output/updates/build-manifest.json --chromium /path/to/chromium \
    --correctness-only --output output/updates/correctness
python3 updates/browser.py --package output/updates/pkg \
    --manifest output/updates/build-manifest.json --chromium /path/to/chromium \
    --suite packet --output output/updates/packet
python3 updates/browser.py --package output/updates/pkg \
    --manifest output/updates/build-manifest.json --chromium /path/to/chromium \
    --suite pacing --output output/updates/pacing
```

Runs require new or empty result directories. `--case` and `--comparison` are
repeatable. Defaults use four rounds and fresh Chromium per comparison/session;
all sessions verify their final Canvas. The runner records the build, package,
browser, harness, GPU and host-load provenance. No builds should run concurrently
with timing. The output contains correctness checks, individual sessions and a
summary using independent browser sessions rather than individual batches for
the exploratory interval.

## Packet notifications

The framebuffer has already received the packet's updates. Two prebuilt patterned
framebuffers alternate; only the declared dirty pixels differ. `baseline` draws
each notification. `coalesced` passes the same notifications through production
`PendingRegions`, flushes at a barrier/end, and on overflow flushes then draws the
returned region directly, as in the isolated PDU-coalescing change at `a14d304e`.
The later pacing change keeps updates across PDUs and requeues the overflow region.

| Case | Notifications | Baseline draws | Coalesced draws |
| --- | ---: | ---: | ---: |
| single | 1 | 1 | 1 |
| repeat | 8 | 8 | 1 |
| contained | 4 | 4 | 1 |
| overlap | 2 | 2 | 2 |
| disjoint | 16 | 16 | 16 |
| overflow | 65 | 65 | 65 |
| barrier | 3 | 3 | 2 |

Single, overlap and disjoint cases guard against queue overhead when there is
nothing to remove. Overflow checks the 64-entry storage bound without losing or
widening any rectangle. Barriers keep otherwise identical draws in distinct
groups. Correctness also covers sparse initialization, changed dimensions and
unchanged source pixels. Timed batches share one Canvas, use common even iteration
counts calibrated on baseline, warm each policy for at least 400 ms, and run
twelve alternating ABBA/BAAB blocks. A/A uses separate benchmark objects with the
same baseline policy in the same WASM binary.

## Ready queue and browser responsiveness

Every input record starts ready. Its bitmap pixels are prebuilt outside timing;
each step applies one synthetic uncompressed bitmap through the existing public
bitmap-update API. A separate reference framebuffer receives the full sequence
before timing. Final source and Canvas bytes must match it, and applied-update
counts must match the requested total. This is a synthetic framebuffer workload,
not a measurement of a live RDP decoder or session.

`baseline` presents every update synchronously. `paced60` and `paced30` use actual
`FramePacing` deadlines, begin with an immediate presentation, retain all pending
updates, and yield through production `BrowserYield` after 8 ms of accumulated
work. A single WASM call runs the ready-input loop in Rust with `select!` and
`Fuse<TimeoutFuture>`. JavaScript only observes heartbeat and task-marker timing.
Waits do not consume the work budget; timer-driven drawing does. The ports close
when each run ends. Presentation cancels any existing timer, including a ready
timer superseded by the selected input update. Capacity overflow flushes first and
queues the returned rectangle. A finite workload performs a terminal flush and
cancels its pending timer; observation after termination checks that no pending
regions remain and draw counts stay unchanged. Framebuffer replacement clears pending state and
resets the first-frame deadline.

The full-frame case defaults to 96 updates. The narrow case repeats 64×720 at
(17,19) for 1,024 updates. The partial case defaults to 16,384 updates cycling
through 65 disjoint 16×16 rectangles, deliberately exercising
overflow. Queue overflow and termination can force presentation before a normal
deadline; they are counted separately, so these runs do not claim an unconditional
60/30 FPS limit. Each case warms the policies (16 full-frame or 256 other updates),
then runs one balanced ABBA or BAAB
block per fresh session. Policy order and block order vary independently.

Results include applied updates, Canvas calls/bytes, flushes, forced and terminal
flushes, maximum queue length, time to first completed draw, workload completion,
a queued task marker, and 4 ms timer heartbeat gaps. Active-work time sums the
synchronous input/timer branches and final flush; it excludes browser-task waits
and some loop/selection overhead, and is not a hardware CPU-time measurement.
Baseline active-work time spans the entire synchronous run, so its ratio against
the differently scoped paced field must not be interpreted as CPU savings.
The timer count describes handled Rust timer branches, not every JS timer callback.
First presentation must follow
exactly one applied update. Heartbeat sample counts accompany quantiles because
short workloads yield few samples. Marker delay and heartbeat gaps describe
browser task responsiveness; they are not real input-event latency.

A separate no-work probe chains 32 timer yields and 32 message yields in
counterbalanced order outside workload timing. It records the cost of nested
timer clamping independently of rendering. When the package exports
`probe_browser_yield(count)`, the probe also awaits the production Rust
`BrowserYield` helper and verifies that all requested wakeups completed. It polls
and cancels a pending yield, awaits reuse, and drops a second channel with a
message in flight; subsequent browser tasks must produce no callback errors.

`--correctness-only` additionally runs two idle presentation probes. They use an
actual Rust `TimeoutFuture` with a pending input future: after the initial draw,
further updates retain the same deadline, and the timer alone flushes the latest
pixels. The second probe cancels the timer and performs a terminal flush. Both
verify all four updates, final source/Canvas bytes, and no further draws during
an observation period beyond the original deadline. A 200 ms interval leaves
room for cold Canvas startup; these are correctness checks, not 60 FPS timing
samples or tests of the complete production Session loop.
