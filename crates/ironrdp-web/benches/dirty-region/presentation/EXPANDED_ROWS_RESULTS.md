# Copy and alpha fusion after selecting upload coverage

Measured on 2026-10-19 UTC directly against **a5682c16**, which already includes
the 1.25× dirty-region cap, SIMD alpha normalization, and copy/alpha fusion for
large input rectangles spanning the framebuffer width. These results isolate
the additional benefit of applying fusion to the final selected upload rectangle.

For example, 1536×720, 1800×720 and 1919×1080 updates on a 1920px-wide framebuffer
already upload whole rows in the baseline. Selecting that coverage before
checking the fusion threshold lets those updates use the same fused copy as an
originally full-width update. The threshold remains 256 KiB of selected RGBA
pixels, and the existing 1.25× cap and uploaded byte counts are unchanged.
Raw extraction still preserves source alpha. Canvas initialization and resize
remain transparent; this change does not introduce a background fill.

## Method

Rust 1.94.1, wasm-bindgen 0.2.126, Chromium 149.0.7827.55, AMD EPYC 7B12 with eight
logical CPUs. Chromium reported software Canvas/compositing and ANGLE SwiftShader.
Both independently compiled SIMD modules call their actual `Canvas::draw_image`;
neither baseline nor candidate needs an adapter. Release flags, source hashes and
artifact hashes are retained in
[build-manifest-expanded-rows.json](build-manifest-expanded-rows.json).

Four rounds interleaved baseline/candidate and baseline/baseline controls in eight
fresh Chromium sessions. Each case/mode used twelve balanced ABBA/BAAB blocks,
at least 400 ms of measured warmup per variant, and common even batch counts
calibrated to at least 16 ms. Both instances shared one timing Canvas. Builds
finished before timing. See [README.md](README.md) for the full protocol.

Results are geometric means of four independent session estimates, each the
median paired log ratio. Intervals are exploratory 95% t intervals over sessions;
individual timing blocks are not independent experiments. Host CPU busy time
averaged 35.7%; one-minute load at sampled boundaries ranged from 3.04 to 5.29.

## Results

Percent change in synchronous extraction, alpha normalization, ImageData creation
and putImageData time; negative is faster. All unspecified framebuffer dimensions
are 1920×1080. A/A compares separate instances of the identical baseline module.

| Dirty region / mode | Candidate time change | Exploratory 95% interval | A/A control |
| --- | ---: | ---: | ---: |
| 1535×720, remains compact | −1.00% | −2.92% to +0.95% | +0.72% |
| 1536×720, expands to whole rows | **−10.98%** | −12.71% to −9.21% | +2.33% |
| 1537×720, expands to whole rows | **−11.01%** | −16.30% to −5.39% | −0.29% |
| 1800×720, expands to whole rows | **−12.50%** | −16.33% to −8.50% | +0.18% |
| 1919×1080, expands to full frame | **−10.05%** | −12.94% to −7.06% | +1.54% |
| 1920×1080, existing fusion, WASM batch | −0.58% | −3.03% to +1.94% | −1.30% |
| 1920×1080, existing fusion, JS call per update | −0.29% | −5.72% to +5.45% | +0.09% |
| 1536×34, selected rows below 256 KiB | −1.74% | −3.22% to −0.24% | −0.82% |
| 1536×35, selected rows above 256 KiB | **−17.55%** | −20.03% to −14.99% | +0.42% |
| 64×720, remains compact | +0.08% | −3.62% to +3.91% | +1.23% |
| 32×32, remains compact | +0.90% | −0.38% to +2.20% | +3.05% |
| 1537×720 on 1921×1080, expands to odd-width rows | **−12.07%** | −17.97% to −5.75% | −0.16% |
| 3072×1440 on 3840×2160, expands to whole rows | **−24.80%** | −25.62% to −23.97% | +1.76% |

Every newly fused case improved in all four sessions. Previously fused full
frames and compact regions showed no clear regression in this experiment; four
sessions do not establish equivalence. The small improvement below 256 KiB is
not attributed to fusion, since that case still uses separate copy and alpha
passes. A/A variability, including +3.05% for the smallest update, also cautions
against interpreting small differences as algorithmic improvements.

[summary-expanded-rows.csv](summary-expanded-rows.csv) retains the aggregate
results and four session ratios for each comparison. Absolute times are medians
of independent session medians, not the estimator used for paired ratios.

## Correctness and scope

- Ten Rust tests pass: six extraction tests and four Canvas-helper tests. They
  check the shared selection policy, raw alpha, source immutability, append/reuse,
  offset rectangles, odd strides and selected uploads around the fusion threshold.
- The direct performance comparison passed 250 upfront full RGBA comparisons
  across baseline and candidate, including sparse first updates and resize.
  Each variant passed 1,088 alpha probes and 17,408 copy probes. All timing-session
  final canvases matched their expected frames; no browser errors were recorded.
- The separate current-source CI command passed 404 RGBA comparisons across
  scalar and SIMD builds, with 1,088 alpha and 17,408 actual copy-helper probes per
  module. It covered full 4K updates, sparse initialization, resize, the 80% width
  boundary and 256 KiB threshold. Both build guards and loaded WASM modules
  confirmed SIMD states `[false, true]`. The initial local run checked clean commit
  `e84b97dfb1b9ba800bd53c9c086c15b6e4ac3133`; its production source hashes match
  the measured candidate. The workflow records the exact head checked by each run.
- Formatting, native strict Clippy, production WASM strict Clippy with SIMD, and
  the auxiliary presentation crate's WASM strict Clippy passed.

The production SIMD requirement and compile-time scalar fallback are unchanged.
These are synthetic synchronous Canvas-stage measurements, excluding decoding,
network backlog, compositor completion and input latency. Windows hardware-
accelerated browsers and live RDP video sessions were not measured. This does
not establish an optimal fusion threshold or resolve video-delay issue #790.

The earlier narrow-region improvements remain documented in [../RESULTS.md](../RESULTS.md).
SIMD and input-full-width fusion results use different baselines and remain in
[RESULTS.md](RESULTS.md) and [LARGE_COPY_RESULTS.md](LARGE_COPY_RESULTS.md); their
percentages must not be added to the improvements above.

## Reproduce

Use the direct commands in [README.md](README.md) with
`--baseline a5682c165cd81824fae397e976cb2e846d3ef0f4` and these repeated `--case`
arguments to `compare.py`:

```text
1920x1080/1535x720  1920x1080/1536x720  1920x1080/1537x720
1920x1080/1800x720  1920x1080/1919x1080 1920x1080/1920x1080
1920x1080/1536x34   1920x1080/1536x35   1920x1080/64x720
1920x1080/32x32    1921x1080/1537x720  3840x2160/3072x1440
```

Keep both `--comparison candidate` and `--comparison aa`. The default four rounds
provide eight fresh timing sessions. Run `presentation/ci.py` separately to check
the current production source with SIMD disabled and enabled, without timing.
