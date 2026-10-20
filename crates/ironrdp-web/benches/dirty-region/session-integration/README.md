# Browser Session correctness

This harness executes the production `Session::run`, `LocalFuturesFramed`,
`ActiveStage`, decoding, Canvas uploads and future cleanup in Chromium. The
`__test-session` feature supplies an already established connection and encodes
server PDUs with the repository's normal encoders. JavaScript scripts the
WebSocket events, monotonic clock and browser timers, and can inject Canvas
failures. MessageChannel yields use real browser ports, including delivery after
cancellation. No copy of the Session loop or presentation policy is tested.

`build.py` uses the actual web crate and `wasm-pack build --target web --release`,
including wasm-pack's wasm-opt step, with the release flags from `xtask web build`.
It builds two SIMD packages: default presentation plus `__test-session`, and the
same fixture with `experimental-presentation-pacing`. `--deployment-smoke` also
builds the normal package with neither opt-in feature.

The fixture starts after connection establishment. It does not test authentication,
the RDCleanPath handshake, a live server, Vite bundling, compositor completion,
performance or main-thread latency. The normal deployment package is loaded in a
separate context with the real browser globals. That smoke check initializes the
optimized bindings, exercises the public desktop-size and Canvas builder methods,
and confirms that fixture exports are absent. A normal-deployment first frame
requires a configured RDP endpoint and is explicitly reported as **not run**.

## Run

Use the repository's Rust 1.94.1, wasm-pack 0.15.0, Binaryen 133, Python 3.11 or
newer, Playwright 1.61.0 and Chromium:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-pack --version 0.15.0 --locked
python3 -m pip install playwright==1.61.0
python3 -m playwright install chromium
```

Install [Binaryen 133](https://github.com/WebAssembly/binaryen/releases/tag/version_133)
and place its `bin` directory on `PATH`, or pass its executable with `--wasm-opt`.
The Linux x86-64 archive's SHA-256 is
`2dc9c7813f5375db93d96ead4b78222fcc3e2677bbb832297af4797782a37489`;
CI verifies it before extraction. This harness pins that exact optimizer for
reproducibility. A local Binaryen 108 build exported the wrong externref table
with this toolchain, causing bindings initialization to fail. This observation
does not establish a minimum supported Binaryen version for other builds.

From `crates/ironrdp-web/benches/dirty-region`:

```sh
python3 session-integration/build.py \
    --output output/session-correctness --deployment-smoke
python3 session-integration/run.py --build-dir output/session-correctness
```

The output directory must be new or empty. The builder defaults to an isolated
Cargo target directory inside it. Use `--target-dir` to reuse a cache dedicated
to this physical checkout; do not share that target with another worktree that
contains the same path packages. `--wasm-pack` and the runner's `--chromium`
accept explicit executable paths. The builder puts the selected `--wasm-opt`
directory first on `PATH` and requires wasm-pack's build log to identify that
executable; the optimizer step remains enabled.

For committed source verification, add `--expected-head "$(git rev-parse HEAD)"`
to the build command. It requires a clean tracked checkout and no untracked source
files. Builds record the checked commit, source and harness hashes, compiler and
wasm-pack versions, commands, flags, per-variant features, the optimizer's path,
version and binary hash, optimizer-step logs and package hashes in
`build-manifest.json`. The runner rejects changed sources,
packages, build logs or HEAD before and after the browser checks. Local builds
of uncommitted edits remain supported and are identified in the report.

## Checked behavior

Both fixture packages check final RGBA bytes and these production paths:

- Thin, non-square New Pointer updates survive browser cursor downscaling. The
  callback's PNG dimensions, opaque pixels and hotspot are checked independently,
  followed by another framebuffer update on the same connection.
- A first sparse frame and a final update presented during idle, without another
  server PDU. The default creates no presentation timer; pacing uses one timer
  and leaves none armed after drawing.
- A timer callback and an incoming frame ready before the same Session poll,
  preserving the newest framebuffer contents without duplicate uploads.
- Pending presentation on graceful termination, including propagation of a final
  draw error, and preservation of the original protocol error when final drawing
  also fails. The default has no pending draw between PDUs.
- An upload failure attempted once, without a retry in final cleanup.
- Single PDUs with 65, 66 and 130 disjoint updates, crossing one or two
  64-region capacity flushes while preserving exact coverage. Failure injection
  at the first, middle and last upload in either batch checks that every region
  in the current PDU is attempted once. Repeated failures across both batches
  and final cleanup preserve the first error, including when every upload fails.
  Full RGBA checks independently mark only successful tiles as visible, and a
  distinct next PDU buffered in the same WebSocket event must not be drawn.
- Cancellation of the actual suspended run future, clearing browser timeout and
  clipboard cleanup interval callbacks without a late upload. The paced package
  also cancels a suspended MessageChannel yield and releases its held message
  after the ports have closed.
- Cancellation while the real deactivation/reactivation sequence is waiting for
  the server. The prior pending update is presented, its timer is canceled, and
  dropping the suspended run future releases the remaining browser callbacks.
- Writer shutdown on initialization failure, normal termination, processing
  errors and cancellation, before freeing the retained Session object. Already
  queued outgoing frames must be sent before the socket closes. A rejected
  second `run()` must leave the first run's connection usable.
- An unsolicited negotiated resize followed by another activation with equal
  dimensions. Each new framebuffer generation clears old Canvas pixels, cancels
  the preceding timer, and presents its first sparse update immediately.

Every completed session checks socket closure before freeing the Session,
remaining timers, MessageChannel handlers and uploads after completion.
`report.json` contains each case's outcome, upload
attempts, successful uploads and timer counts. The
[`Web Session correctness`](../../../../../.github/workflows/web-session-correctness.yml)
workflow builds the current PR head, runs all three packages, and retains the
reports, build logs, provenance and optimized packages as artifacts. These are
correctness checks; their scripted clock provides no performance evidence for
enabling pacing by default.

Cancellation here uses `SessionFixture.abort()` to drop the actual run future.
Normal `Session::shutdown()` is an input event; the existing activation read loop
does not select input events while awaiting a server response. The harness does
not claim to resolve or cover prompt normal shutdown during that wait.
