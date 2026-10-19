# Full-width copy and alpha fusion

Measured on 2026-10-19 UTC against **289f710e**, which already contains both the
dirty-region cap and SIMD alpha normalization. These are direct comparisons with
the current SIMD implementation, not differences between earlier scalar results.

The candidate copies and makes pixels opaque in one SIMD pass only when the
dirty region spans the full framebuffer width and contains at least 256 KiB of
RGBA pixels. Smaller and partial-width regions keep the existing extraction and
SIMD alpha path, including the 1.25× overdraw cap. Raw extraction still preserves
alpha and source framebuffer bytes. Other targets use copying plus scalar alpha.

## Method

Rust 1.94.1, wasm-bindgen 0.2.126, Chromium 149.0.7827.55, AMD EPYC 7B12 with eight
logical CPUs. Chromium reported software Canvas/compositing and ANGLE SwiftShader.
The release flags, LTO, source hashes and artifact hashes are retained in
[build-manifest-large-copy.json](build-manifest-large-copy.json).

Four rounds interleaved baseline/candidate and baseline/baseline controls in eight
fresh Chromium sessions. Each case/mode used twelve balanced ABBA/BAAB blocks,
at least 400 ms of measured warmup per variant, and common even batch counts
calibrated to at least 16 ms. Both instances shared one timing Canvas. Builds
finished before timing. The protocol and direct-build command are described in
[README.md](README.md).

Results are geometric means of four independent session estimates, each the
median paired log ratio. Intervals are exploratory 95% t intervals over sessions;
the repeated timing blocks are not independent experiments. Host CPU busy time
averaged 29.3%; one-minute load at sampled boundaries ranged from 0.87 to 3.98.

## Results

Percent change in synchronous extraction, alpha normalization, ImageData creation
and putImageData time; negative is faster. The byte counts uploaded are unchanged
by this optimization. All regions except the last row use a 1920×1080 framebuffer.

| Dirty region / mode | Candidate time change | Exploratory 95% interval | A/A control |
| --- | ---: | ---: | ---: |
| 1920×1080, WASM batch | **−8.30%** | −14.95% to −1.13% | −1.99% |
| 1920×1080, JS call per update | **−11.39%** | −13.39% to −9.35% | −1.83% |
| 64×720 | +0.48% | −6.15% to +7.58% | −1.02% |
| 512×720 | +1.05% | −3.66% to +5.98% | −0.48% |
| 1280×720 | −2.48% | −11.48% to +7.44% | −0.15% |
| 1536×720 | +1.28% | −3.75% to +6.58% | +1.12% |
| 17×65 | −1.95% | −6.99% to +3.37% | −2.42% |
| 32×32 | −3.92% | −10.55% to +3.21% | +0.10% |
| 1920×34, below threshold | +0.71% | −1.31% to +2.78% | −0.54% |
| 1920×35, above threshold | **−18.28%** | −21.23% to −15.22% | +1.09% |
| 1920×137, approximately 1 MiB | **−21.54%** | −23.94% to −19.07% | −1.30% |
| 1920×547, approximately 4 MiB | **−15.00%** | −16.61% to −13.37% | −2.66% |
| 1919×35 on 1919×1080 | **−16.74%** | −17.75% to −15.71% | −1.72% |

Both full-screen modes improved in all four sessions. The selected lower bound
is supported by improvements immediately above 256 KiB, around 1 MiB and 4 MiB,
and for an odd-width strip. This does not establish an optimal threshold below
256 KiB or on other machines. Partial-width updates were not switched to fusion;
their measurements did not show a clear regression, but these small experiments
do not prove equivalence. A/A itself measured −2.66% at 1920×547, illustrating
that even an interval excluding zero can reflect execution conditions.

[summary-large-copy.csv](summary-large-copy.csv) retains session summaries. Do
not reconstruct paired ratios from the medians of independent timing samples.

## Correctness and scope

- Nine Rust extraction/Canvas tests pass, including the existing raw-alpha,
  dimension, threshold, append and reuse contracts. Copy probes independently
  vary source and destination offsets 0–15 and lengths 0–67.
- Chromium passed 154 upfront full RGBA comparisons across the two variants,
  including sparse initialization, resize, the new threshold cases and odd-width
  rows. Each variant passed 1,088 alpha probes and 17,408 copy probes. The source
  framebuffers stayed unchanged; every final timing canvas matched its expected
  frame. Each timing session also checked its loaded variants.
- Native and WASM strict Clippy and formatting checks pass. WASM builds use the existing
  SIMD requirement; the change adds no runtime feature requirement.

These are synthetic synchronous Canvas-stage measurements. They exclude RDP
decoding, network behavior, compositor completion and input latency, and are not
evidence that video-delay issue #790 is resolved. The earlier dirty-region gains
remain documented separately in [../RESULTS.md](../RESULTS.md).

To reproduce the measured case matrix, use the direct commands in README.md and
pass these repeated `--case` arguments to `compare.py`:

```text
1920x1080/1920x1080  1920x1080/64x720   1920x1080/512x720
1920x1080/1280x720   1920x1080/1536x720 1920x1080/17x65
1920x1080/32x32     1920x1080/1920x34  1920x1080/1920x35
1920x1080/1920x137   1920x1080/1920x547 1919x1080/1919x35
```
