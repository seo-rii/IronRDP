# SIMD alpha normalization comparison

Measured on 2026-10-19 UTC. **Select SIMD alpha normalization in `Canvas::draw`,
keeping the existing extraction and copy paths.** On the repository's Rust 1.94.1,
against the cap-only scalar baseline at `ea9c379b`, the paired full-screen draw
estimate improved by 19.4%; the path with one JavaScript-to-WASM call per update
improved by 20.9%.

The fused copy/alpha candidate also improved full-screen drawing, but its narrow
odd-width case was slower in all four sessions of the initial Rust 1.94.0
comparison. SIMD-only is the smaller change:
it preserves the extraction contract and uses the old scalar loop on targets
without `wasm32 + simd128`, and for the remaining pixels after a SIMD chunk.

## Confirmation with Rust 1.94.1

The initial experiment inherited a toolchain environment override and used
Rust 1.94.0, as recorded in its manifest and WASM producer metadata. The final
build script explicitly selects a toolchain. Rust 1.94.1 generated different WASM,
so the full-screen case was measured again rather than transferring the earlier
percentages to the repository's compiler. The final source also removes an
unfulfilled Clippy expectation; the pixel operations are unchanged.

This confirmation used twelve fresh Chromium sessions, the same four-round
A/B and A/A design described below, and both full-screen draw modes. Host CPU
busy time averaged 54.3%; one-minute load at session boundaries ranged from
4.14 to 7.29. These are separate measurements, not pooled with the earlier run.

| Full-screen mode | SIMD alpha | Fused copy/alpha | A/A control |
| --- | ---: | ---: | ---: |
| WASM draw batch | **−19.41% [−23.85, −14.70]** | −28.45% [−34.59, −21.72] | +0.75% [−1.33, +2.87] |
| JS call per update | **−20.89% [−22.23, −19.52]** | −29.83% [−36.95, −21.92] | +1.28% [−5.96, +9.07] |

SIMD batch ratios were 0.8022, 0.7701, 0.8392 and 0.8136; JS-call ratios were
0.7885, 0.7804, 0.7992 and 0.7964. All four sessions improved in both modes.
See [summary-rust1941.csv](summary-rust1941.csv) and
[build-manifest-rust1941.json](build-manifest-rust1941.json).
The latter's SIMD source hashes match the final production files exactly.

Rust 1.94.1 passes seven native integration tests, native/WASM Clippy with
`-D warnings`, workspace formatting, and `git diff --check`. The auxiliary
benchmark passes release WASM Clippy. Its Chromium byte probes and full/sparse/
resize screen checks were repeated successfully with the final compiler.

## Initial implementation comparison: method and limits

AMD EPYC 7B12, 8 logical CPUs; Rust 1.94.0; wasm-bindgen 0.2.126; headless Chromium
149.0.7827.55. Chromium used software Canvas/compositing with ANGLE SwiftShader.
Each candidate was compiled separately with the application WASM flags, size
optimization and LTO. Source and artifact hashes are in
[build-manifest.json](build-manifest.json); reproduction and the complete timing
protocol are in [README.md](README.md). This historical manifest records the
builder and lint annotation as they existed before the final toolchain fixes.

Four rounds interleaved scalar/SIMD, scalar/fused, and scalar/scalar controls:
12 fresh Chromium sessions, four per comparison. Each case/mode used twelve
balanced ABBA/BAAB blocks after at least 400 ms of measured warmup per variant.
Module order and mode order were independently counterbalanced. Both variants
used a shared timing Canvas and the same even batch count, calibrated on scalar
to at least 16 ms. No compilation ran during measurement.

Other host work increased during the experiment. Host CPU busy time averaged
84.0% over session windows; the observed one-minute load range at session/case
boundaries was 2.08–21.09. A/A controls are therefore material: even identical
binaries produced sizeable differences for some small cases. These results
support the large-frame improvement in this environment, not precise universal
speedup percentages or improvements for every small region.

Each session estimate is the median log(candidate/scalar) ratio of its paired
blocks; the table aggregates those four independent session estimates
geometrically. Intervals are exploratory two-sided t intervals across the four
session log estimates. The batches within a session are not treated as independent
experiments. With only four sessions, correlated host load and multiple cases,
these intervals do not prove equivalence or reliable population coverage.
Absolute medians in [summary.csv](summary.csv) use a different estimator and must
not be used to reconstruct the paired percentage changes.

## Initial Rust 1.94.0 draw results

Percent change in synchronous extraction + alpha + `ImageData` + `putImageData`
time; negative is faster. Brackets show the exploratory 95% interval.

| Dirty region on 1920×1080 | SIMD alpha | Fused copy/alpha | A/A control |
| --- | ---: | ---: | ---: |
| 1920×1080 | **−21.96% [−27.45, −16.05]** | −27.39% [−38.92, −13.69] | +1.73% [−1.52, +5.08] |
| 64×720 | −17.35% [−29.21, −3.52] | −5.91% [−18.94, +9.21] | −4.55% [−12.36, +3.96] |
| 32×32 | −14.06% [−28.72, +3.62] | −12.71% [−22.34, −1.89] | +5.40% [−9.07, +22.17] |
| 17×65 | −10.53% [−22.55, +3.35] | +7.44% [+5.85, +9.06] | +2.87% [−6.62, +13.32] |
| 1920×1080, JS call per update | **−20.75% [−28.39, −12.29]** | −19.88% [−31.34, −6.50] | −2.61% [−12.17, +7.98] |

The SIMD full-screen ratios in session order were 0.7840, 0.7318, 0.8149 and
0.7935; the JS-call ratios were 0.7232, 0.8071, 0.8385 and 0.8061. Both paths
improved in all four sessions. Small-region findings are less reliable, especially
given the A/A spread. The fused candidate's full-screen improvement does not
establish superiority to SIMD-only: their comparisons used different sessions.

The optional `prepare` measurements in `summary.csv` are extraction-only. Fused
extraction includes alpha normalization, unlike the other two builds, so its
longer extraction time is not itself a regression. As a noise warning, even A/A
`prepare` at 64×720 measured +9.70% with an exploratory interval excluding zero;
the binaries were identical.

## Mechanism and correctness

The original alpha loop remained scalar despite enabling `+simd128`: WASM
disassembly showed an `i32.store8` for each pixel. The selected code explicitly
loads 16 bytes, ORs four `0xff000000` lanes, then stores 16 bytes. Disassembly of
the timed draw function confirms `v128.load align=1`, `v128.or`, and
`v128.store align=1`; the remaining complete pixels use scalar stores. This
changes processing four alpha bytes per vector rather than issuing one byte
store per pixel, while preserving RGB and incomplete final pixels.

Before timing, all three builds passed 108 initial/full/partial RGBA comparisons
and 24 sparse-first-update/resize comparisons, with unchanged source framebuffers.
The actual SIMD helper passed 1,088 WASM alignment/length probes (offsets 0–15,
lengths 0–67), including empty buffers, tails and surrounding bytes. Every session
also checked its loaded variants on separate correctness canvases, and each
timing Canvas matched its final expected framebuffer after measurement. Native
integration tests cover the scalar fallback with the same offset/length matrix.

The decoder, framebuffer, selected dirty rectangle, Canvas context settings,
upload area and call frequency remain unchanged by the SIMD addition. This test
does not measure compositor completion, live RDP decoding, full-screen video
playback or input latency. It does not establish a fix for issue #790's accumulating
delay. The earlier cap-only results remain separate in [../RESULTS.md](../RESULTS.md).
