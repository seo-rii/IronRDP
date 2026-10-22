# Dirty-region expansion measurements

Measured on 2026-10-18 UTC (2026-10-19 KST), based on `62246b36` plus this change.

These are the original cap-only measurements, retained with their measured source
hashes. The later independent-build comparison of pixel preparation is recorded
in [presentation/RESULTS.md](presentation/RESULTS.md).

The selected limit is **1.25x**: retain the existing height/width trigger, but widen only when the dirty region covers at least 80% of the image width. At 1920 pixels, the boundary is 1536 pixels. This bounds extra copying, alpha normalization, and Canvas upload to 25% while preserving contiguous copies for near-full and full-width updates.

## Environment and method

- AMD EPYC 7B12, 8 logical CPUs; Linux 6.1.0-54-cloud-amd64, glibc 2.36.
- Rust 1.94.1; `wasm-bindgen` 0.2.126; `web-sys` 0.3.103.
- Headless Chromium 149.0.7827.55, 1920x1080 viewport, device scale factor 1. Chromium reported software 2D canvas/compositing/rasterization and ANGLE SwiftShader; these results do not establish hardware-accelerated browser performance.
- Size-optimized release builds with LTO, the WASM flags in [README.md](README.md), and no `wasm-opt` pass.
- Native: 20 samples per policy/case after calibration to at least 8 ms and warmup. Final Chromium: 30 samples per policy/case, batches calibrated to at least 16 ms, with shuffled policy order and animation-frame waits outside the timer. Builds were finished before timing.
- Two patterned synthetic framebuffers alternate only dirty pixels. Timings exclude fixture generation, RDP decoding, compositor completion, and input latency. The browser times real extraction, alpha normalization, `ImageData`, and `putImageData` synchronously. Correctness and timing use separate canvases; timing canvases are read back only after their final sample.

An initial 20-sample, 12-case run compared all caps. The final run repeated six 1080p cases with longer batches and the selected production code. Absolute timings changed between runs, so all comparisons below use the final run only. [measurements.csv](measurements.csv) contains every final native/WASM policy summary, including p10/p90. The runners retain raw samples in their output files.

## Final browser results

Times are microseconds per update. Brackets contain the empirical p10–p90 range. Ratios describe this synthetic synchronous draw stage.

| Dirty region | Uploaded KiB, before → after | Baseline µs [p10–p90] | Production µs [p10–p90] | Baseline / production |
| --- | ---: | ---: | ---: | ---: |
| 64×720 | 5400 → 180 | 1256.25 [1175.00–1475.00] | 50.59 [48.44–52.93] | 24.83× |
| 512×720 | 5400 → 1440 | 1268.75 [1150.00–1618.75] | 307.81 [285.94–368.75] | 4.12× |
| 960×720 | 5400 → 2700 | 1175.00 [1100.00–1318.75] | 587.50 [546.88–703.12] | 2.00× |
| 1280×720 | 5400 → 3600 | 1193.75 [1131.25–1262.50] | 825.00 [753.13–896.87] | 1.45× |
| 1536×720 | 5400 → 5400 | 1306.25 [1137.50–1562.50] | 1293.75 [1193.75–1562.50] | 1.01× |
| 1920×1080 | 8100 → 8100 | 2187.50 [1900.00–2650.00] | 2325.00 [1912.50–2875.00] | 0.94× |

The 64×720 update uploads 180 KiB instead of 5400 KiB (30x fewer bytes) and measured 24.8x faster. Near-full-width and full-screen cases retain the same whole-row extraction path and byte count. Their timing distributions overlap; no full-screen improvement is claimed.

## Choosing the cap

Final Canvas-stage medians for the cases that distinguish the proposed limits:

| Dirty region | 1.25x cap µs | 1.5x cap µs | 2x cap µs |
| --- | ---: | ---: | ---: |
| 960×720 | 575.00 | 581.25 | 1200.00 |
| 1280×720 | 837.50 | 1212.50 | 1175.00 |

The 2x cap still expands a 960-pixel region to whole rows. The 1.5x cap additionally expands a 1280-pixel region. The 1.25x cap avoids both costs, with a clear 1280-pixel draw-stage benefit in the longer final run. It is a conservative bound among the three tested caps, not a claim that 1.25 is universally optimal.

Native helper-only medians illustrate the copying tradeoff (no Canvas or alpha pass):

| Dirty region on 1920×1080 | Exact rectangle µs | Whole rows µs |
| --- | ---: | ---: |
| 32×32 | 0.17 | 6.92 |
| 64×720 | 7.63 | 284.14 |
| 512×720 | 52.89 | 264.98 |
| 960×720 | 117.49 | 306.73 |
| 1280×720 | 175.35 | 278.86 |
| 1536×720 | 215.26 | 262.13 |
| 1920×1080 | 729.52 | 587.28 |

Contiguous extraction remains useful for full-width updates. Exact extraction can also win for near-full-width rectangles; the selected cap deliberately leaves some room for contiguous-copy tradeoffs on other browsers and machines.

## Correctness and limits

- Six Rust tests passed: pixel values/selected dimensions, both extraction paths, offsets/edges, 1535/1536/1537 threshold boundaries, original size thresholds, append/clear/reuse, transparent bytes, and maximum `u16` width arithmetic.
- Chromium passed 56 full RGBA comparisons across all seven policies: initial full repaint, three partial updates, resize, full repaint, and partial updates again. Every measured final canvas also matched its full expected framebuffer byte-for-byte.
- A genuinely transparent first-partial-update test, repeated after resize, found zero dirty-pixel or RGB mismatches. For a 16×180 update on a 320×240 canvas, legacy whole-row uploads make 54,720 untouched pixels opaque black; capped uploads leave them transparent. This intentional removal of incidental overdraw is observable before a complete repaint. The subsequent full repaint converged byte-for-byte. No universal equality claim is made for partially initialized canvases.
- Native and WASM Clippy checks with `-D warnings`, workspace formatting, and `git diff --check` passed.

This builds on the direct Canvas path from [upstream PR #1374](https://github.com/Devolutions/IronRDP/pull/1374). It leaves Canvas call frequency, RDP decoding/protocol handling, and the public API unchanged. There is no live-server/video replay in these measurements, and no claim to resolve the accumulating input latency in [upstream issue #790](https://github.com/Devolutions/IronRDP/issues/790).

## Reproduce the final browser run

After following the build instructions in [README.md](README.md):

```sh
python3 browser.py --samples 30 --target-ms 16 --output output/chromium-final.json \
    --case 1920x1080/64x720 --case 1920x1080/512x720 \
    --case 1920x1080/960x720 --case 1920x1080/1280x720 \
    --case 1920x1080/1536x720 --case 1920x1080/1920x1080
```

Final source/artifact SHA-256 values:

- `image.rs`: `37e38799170227e3fe4719e494efd138d23cd5789c2c9c7955f1126b72d6aa69`
- `canvas.rs`: `63aeb6af765b7081d47b0591749df583040e36be0a9a7633f95d12f127146735`
- benchmark WASM: `216d84a3934660540c4293527663306394703fe3deeb72b0aad76ecdfb5cadb2`
