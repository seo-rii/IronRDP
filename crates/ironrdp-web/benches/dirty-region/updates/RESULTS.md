# Coalescing and pacing measurements

Measured on 2026-10-19. Removing duplicate/contained notifications reduces
Canvas submission time. Pacing improves browser task responsiveness in a ready
backlog, but increases completion time, especially when no rectangles can be
removed. The pacing policy remains an experiment for fork review. Current production now
uses only within-PDU coalescing by default; the slower paced policy requires the
`experimental-presentation-pacing` Cargo feature. These archived numbers describe
the revisions below, before the later clipping, best-effort draw-error handling,
negotiated framebuffer-generation reset and Session trace events. They are not a
new performance measurement of the revised Session loop.

These results build on the [dirty-region cap](../RESULTS.md) and
[conditional full-width copy fusion](../presentation/LARGE_COPY_RESULTS.md).
Both policies in this comparison already use those optimizations. Their earlier
results remain separate; the ratios must not be multiplied together.

## Sources and environment

- Production helpers: `4f9a8323ed63f75c21b88adb9c4f419aa8319863`.
- Measured harness: `8f3b416236111294f025bb4274d6f846d3602a90`. Its files match
  the working-tree hashes captured before that harness was committed. Use this
  revision to reproduce the timing experiment.
- Rust 1.94.1, release/LTO, application SIMD/bulk-memory flags, wasm-bindgen
  0.2.126, no wasm-opt. [Build manifest](build-manifest.json) records exact flags,
  source hashes and generated JS/WASM hashes.
- Chromium 149.0.7827.55, headless Linux, AMD EPYC 7B12, 8 logical CPUs,
  1920×1080 viewport. Canvas 2D and compositing reported software rendering;
  ANGLE reported SwiftShader. This does not establish hardware GPU performance.
- No concurrent benchmark builds. Session-boundary one-minute system load ranged
  from 0.69 to 4.51. [Environment and session records](environment.json) retain
  browser/GPU information, scheduling, host CPU counters, correctness and counts.

Each comparison has four independent Chromium sessions. Packet timing uses
twelve balanced ABBA/BAAB blocks per case/session after at least 400 ms warmup per
policy. Each block averages its two observations per policy, then takes the
candidate/baseline log ratio; the session estimate is the median block log ratio.
Pacing uses one balanced block after warming each policy, and takes the log ratio
of the two within-session policy means. The reported change is the geometric
mean of session ratios, with an exploratory Student-t interval on the four log
estimates. These are small-n intervals, not equivalence tests.

A/A compares two baseline instances. Its variability is reported separately,
without subtracting it from A/B. The 672 packet blocks and 144 pacing observations
are retained in [packet-blocks.csv](packet-blocks.csv) and
[pacing-observations.csv](pacing-observations.csv). Machine-readable paired
estimates are in [summary-packet.json](summary-packet.json) and
[summary-pacing.json](summary-pacing.json).

## Coalescing within a PDU

Both policies receive the same already-updated framebuffer. The baseline draws
each notification; the candidate uses production `PendingRegions`, flushes at
barriers/end and flushes before overflow. This isolates the PDU-only policy in
`a14d304e`; the subsequent paced session instead retains regions across PDUs and
requeues the overflow region. Timings include extraction, alpha normalization,
ImageData construction and putImageData, excluding decoding.

| Synthetic notification group | Draws before → after | Time change | Exploratory 95% interval | A/A change |
| --- | ---: | ---: | ---: | ---: |
| One 64×128 region | 1 → 1 | +2.87% | −5.73% to +12.26% | +4.47% |
| Same full frame eight times | 8 → 1 | **−83.85%** | −85.51% to −81.98% | +0.92% |
| Four contained regions | 4 → 1 | **−46.09%** | −49.55% to −42.40% | +1.64% |
| Two partially overlapping regions | 2 → 2 | +0.45% | −1.91% to +2.86% | −0.31% |
| Sixteen disjoint tiles | 16 → 16 | +2.59% | −3.85% to +9.47% | +1.47% |
| Sixty-five disjoint tiles, queue overflow | 65 → 65 | +2.40% | +0.50% to +4.34% | −0.91% |
| Duplicate full frame, barrier, full frame | 3 → 2 | **−30.85%** | −34.43% to −27.07% | +0.76% |

Containment removes redundant submissions without expanding coverage. Distinct
regions still incur queue inspection, and the overflow case shows a small cost.
The measurements do not establish that queuing is free for other unchanged-draw
cases. No arbitrary bounding-box union or decoder change is involved.

## Pacing a ready backlog

The baseline applies and presents every input. The candidate applies every input
but targets 60 FPS, combines pending regions and yields through production
`BrowserYield` after about 8 ms of accumulated synchronous work. First presentation
is immediate. Capacity and lifecycle flushes can precede a normal deadline.

All inputs are ready from the start. Payloads are prebuilt uncompressed bitmap
updates, applied through the existing framebuffer API inside one Rust/WASM call.
JavaScript observes a queued task marker and a 4 ms heartbeat. This measures a
synthetic backlog, not network traffic, RDP codec processing, real input events,
display/compositor completion or a normally paced 60 Hz stream.

| Workload | Draws before → after | Completion time change | Exploratory 95% interval | A/A completion change |
| --- | ---: | ---: | ---: | ---: |
| 96 full 1920×1080 updates | 96 → 96 | **+6.50%** | +0.32% to +13.06% | +0.13% |
| 1,024 repeated 64×720 updates | 1,024 → 36–41 | +4.91% | −1.13% to +11.31% | −4.88% |
| 16,384 updates cycling through 65 disjoint 16×16 tiles | 16,384 → 16,384 | **+43.78%** | +26.89% to +62.92% | −5.20% |

The full-frame update/presentation pipeline exceeds the target frame interval in
this environment, and the 60 FPS policy still draws every update. The narrow workload
reduces submitted RGBA bytes from 180 MiB to 6.33–7.21 MiB, while still applying all
1,024 bitmaps. The disjoint workload reaches the 64-region bound and performs 255
forced flushes plus a terminal flush: it removes no draws. Its additional queue,
clock/scheduling work and browser-task waits are a substantial throughput cost;
these measurements do not isolate each cost individually.

| Workload | Queued task marker, baseline → 60 FPS | Maximum heartbeat gap, baseline → 60 FPS |
| --- | ---: | ---: |
| Full frame | 2,369 → 42.4 ms | 2,369 → 67.8 ms |
| Repeated narrow region | 632 → 21.2 ms | 632 → 29.9 ms |
| Disjoint tiles / overflow | 111 → 20.7 ms | 111 → 29.2 ms |

These absolute values are medians of eight observations per policy in the four
60 FPS comparison sessions. They are descriptive, while the completion changes
above use paired session estimates. Baseline runs record one heartbeat observation
spanning the blocked workload, delivered in the observation tail after the
synchronous run ends; candidate sample counts are retained in the
CSV. A PDU and a presentation batch remain atomic, so an 8 ms work budget is not
an 8 ms callback or input-latency guarantee. Full-frame task markers ranged from
38 to 47 ms despite yielding after each update.

The 30 FPS comparison is exploratory. The measured candidate used 60 FPS;
current production pacing is opt-in:

| Workload | 30 FPS completion change | Exploratory 95% interval | Candidate draws |
| --- | ---: | ---: | ---: |
| Full frame | +3.82% | −15.18% to +27.07% | 49–94 |
| Repeated narrow region | +1.92% | −2.84% to +6.91% | 18–29 |
| Disjoint tiles / overflow | +47.44% | +14.20% to +90.36% | 16,384 |

Separate session variability prevents choosing 30 FPS over 60 FPS from these
completion estimates. The responsiveness/throughput tradeoff is the supported
conclusion. The raw `active_work_ms` field is diagnostic: baseline uses whole-run
synchronous duration, whereas pacing sums branches and final flush, omitting
browser waits and some loop/selection overhead. Its ratio is not a CPU saving.

Every timed run handled zero presentation-timer branches. In this all-ready
workload, an input branch reaches the deadline first, or capacity/termination
forces a flush. These timing results alone do not test pending presentation after
incoming input becomes idle.

## Correctness and remaining review

The measured package passed 28 packet and 18 pacing correctness scenarios,
including sparse initialization and changed dimensions. Each packet case/session
verified its final shared Canvas after all timed blocks. Each pacing observation
verified its final Canvas and additionally checked the reference
framebuffer, all applied updates, immediate first presentation, the 64-region
bound, empty pending state and stable draw counts after termination. Browser
errors were empty. Actual Rust MessageChannel probes exercised 32 wakeups and
cancellation, reuse and drop with messages in flight.

A subsequent correctness-only package at `e6ae2c42` adds two idle timer probes;
it is not the package used for the performance tables. Production helper hashes
are unchanged. With a 200 ms fixture interval, both probes applied four updates
and produced two draws. Follow-up input retained the original deadline. The idle
case handled one actual Rust timeout branch with no ready input and flushed the
latest pixels; the termination case canceled its timeout and flushed immediately.
Both matched the reference source/Canvas, left no pending regions, and produced
no further draws during a 219 ms observation. The original 28 packet and 18
pacing scenarios also passed again. [Probe results](idle-checks.json) and the
[separate build manifest](idle-check-manifest.json) preserve this evidence. These
probes use production helpers with a synthetic Rust select loop, not the complete
production Session.

Production validation passed 20 Rust tests (3 canvas, 6 extraction, 6 pending
regions and 5 clock-driven pacing tests), native and WASM Clippy with `-D warnings`,
and formatting. Pending-region coverage includes 46,656 ordered triples on a small
grid and 648 pairs around extraction thresholds. The auxiliary release WASM build
also passed strict Clippy, package hash verification and JS/Python syntax checks.

The production implementation flushes before framebuffer replacement and at
termination, preserves the original error when a final draw also fails, and
cancels superseded timers. These are not full `Session::run` integration tests.
Live server reactivation/resizes, transported input and ACK behavior, background
tab recovery, normal receive pacing, and the best default presentation rate still
need evaluation before promoting the pacing change. Cursor callbacks may observe
a Canvas that temporarily trails the latest framebuffer, and deferred Canvas
errors may be discovered after additional PDUs have been decoded. The tests do
not establish identical intermediate screens or identical error-time screens.

An earlier JS-driven pacing experiment was stopped after review found timer
lifetime and work-budget differences from the Rust implementation. Its pacing
numbers are excluded. Packet timing was also rerun using the final measured
Rust-driver package so all results above share the recorded package and harness.
No claim is made that this resolves the video/input delay in upstream #790.

See [README.md](README.md) for build and run commands.
