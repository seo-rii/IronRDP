// Independent WASM modules, shared timing canvas, and balanced paired batches.
// Correctness readbacks use separate canvases and never affect a timing context.
let current;
const frame = () => new Promise(requestAnimationFrame);

function equal(actual, expected, label) {
    if (actual.length !== expected.length) throw new Error(`${label}: length mismatch`);
    for (let index = 0; index < actual.length; index++) {
        if (actual[index] !== expected[index]) {
            throw new Error(`${label}: byte ${index}: ${actual[index]} != ${expected[index]}`);
        }
    }
}

function pixels(canvas) {
    return canvas.getContext("2d").getImageData(0, 0, canvas.width, canvas.height).data;
}

function summary(values) {
    const sorted = [...values].sort((a, b) => a - b);
    const quantile = fraction => {
        const position = (sorted.length - 1) * fraction;
        const lower = Math.floor(position);
        return sorted[lower] + (sorted[Math.ceil(position)] - sorted[lower]) * (position - lower);
    };
    return {
        mean_us: values.reduce((sum, value) => sum + value, 0) / values.length,
        median_us: quantile(0.5), p10_us: quantile(0.1), p90_us: quantile(0.9),
    };
}

window.prepare = async ({pair = ["scalar", "simd", "fused"],
    packageRoot = "../output/simd-presentation-20261019/pkg", baseline = "scalar"} = {}) => {
    if (current) throw new Error("use a fresh page for each preparation");
    if (pair.some(name => !["scalar", "simd", "fused", "baseline", "candidate"].includes(name))) {
        throw new Error("unknown build variant");
    }
    if (!pair.includes(baseline)) throw new Error("pair must include its calibration baseline");
    const modules = [];
    for (let index = 0; index < pair.length; index++) {
        // Separate binding state and WASM memories, including baseline/baseline A/A.
        const module = await import(`${packageRoot}/${pair[index]}/ironrdp_web_dirty_region_bench.js?instance=${index}`);
        await module.default();
        modules.push(module);
    }
    current = {pair, modules, baseline};
    return {pair, baseline, simd128: modules.map(module =>
        typeof module.simd128_enabled === "function" ? module.simd128_enabled() : null),
        user_agent: navigator.userAgent, cross_origin_isolated: crossOriginIsolated,
        hardware_concurrency: navigator.hardwareConcurrency, performance_time_origin: performance.timeOrigin};
};

function checkFixture(module, fixture, label) {
    const {imageWidth, imageHeight, left, top, width, height} = fixture;
    const canvas = document.createElement("canvas");
    const bench = new module.PresentationBenchmark(canvas, imageWidth, imageHeight, left, top, width, height);
    try {
        const before = [bench.source_pixels(0), bench.source_pixels(1)];
        let changed = 0;
        for (let y = 0; y < imageHeight; y++) {
            for (let x = 0; x < imageWidth; x++) {
                const dirty = x >= left && x < left + width && y >= top && y < top + height;
                for (let channel = 0; channel < 4; channel++) {
                    const offset = (y * imageWidth + x) * 4 + channel;
                    if (before[0][offset] !== before[1][offset]) {
                        if (!dirty) throw new Error(`${label}: fixture changed outside dirty region`);
                        changed++;
                    }
                }
            }
        }
        if (!changed) throw new Error(`${label}: fixture frames are identical`);
        bench.draw_batch(0, 0, imageWidth, imageHeight, 2);
        equal(pixels(canvas), bench.expected_pixels(1), `${label} initial full`);
        bench.draw_batch(left, top, width, height, 1);
        equal(pixels(canvas), bench.expected_pixels(0), `${label} partial frame 0`);
        bench.draw_batch(left, top, width, height, 1);
        const result = pixels(canvas);
        equal(result, bench.expected_pixels(1), `${label} partial frame 1`);
        for (const index of [0, 1]) {
            equal(bench.source_pixels(index), before[index], `${label} source ${index} unchanged`);
        }
        return result;
    } finally {
        bench.free();
    }
}

function checkBounds() {
    // Expected upload bounds include the existing whole-row expansion policy.
    // Explicit outcomes keep this oracle independent of the clipping implementation.
    const fixtures = [
        {name: "right-overflow", region: [90, 7, 120, 73], upload: [90, 7, 95, 73]},
        {name: "bottom-overflow", region: [13, 80, 44, 100], upload: [13, 80, 44, 87]},
        {name: "both-overflow", region: [91, 84, 120, 100], upload: [91, 84, 95, 87]},
        {name: "outside-right", region: [96, 7, 120, 73], upload: null},
        {name: "outside-bottom", region: [13, 88, 44, 110], upload: null},
        {name: "outside-both", region: [96, 88, 120, 100], upload: null},
        {name: "inverted-horizontal", region: [19, 7, 18, 73], upload: null},
        {name: "inverted-vertical", region: [13, 8, 44, 7], upload: null},
        {name: "maximum-coordinates", region: [0, 0, 65535, 65535], upload: [0, 0, 95, 87]},
        {name: "maximum-outside", region: [65535, 65535, 65535, 65535], upload: null},
        {name: "last-pixel", region: [95, 87, 95, 87], upload: [95, 87, 95, 87]},
        {name: "maximum-right", region: [95, 5, 65535, 75], upload: [95, 5, 95, 75]},
        {name: "clipped-whole-rows", region: [19, 3, 120, 73], upload: [0, 3, 95, 73]},
        {name: "clipped-below-cap", region: [20, 3, 120, 73], upload: [20, 3, 95, 73]},
        {name: "clipped-full-width-fusion", imageWidth: 1920, imageHeight: 40,
            region: [0, 5, 65535, 65535], upload: [0, 5, 1919, 39]},
        {name: "outside-before-expansion", imageWidth: 64, imageHeight: 128,
            region: [64, 0, 127, 127], upload: null},
    ];
    // Clipping leaves each side of the 80% cap, crossed with the 256 KiB fusion
    // threshold. A selection performed before clipping would expand all six.
    for (const [width, left, expanded] of [[1535, 385, false], [1536, 384, true], [1537, 383, true]]) {
        for (const height of [34, 35]) {
            fixtures.push({name: `clipped-cap-${width}-height-${height}`, imageWidth: 1920, imageHeight: 96,
                region: [left, 5, 65535, 5 + height - 1],
                upload: [expanded ? 0 : left, 5, 1919, 5 + height - 1]});
        }
    }
    const results = [];
    current.modules.forEach((module, index) => {
        // Historical packages retain their original behavior. ci.py separately
        // requires both current-source builds to execute every bounds check.
        if (typeof module.PresentationBenchmark.prototype.draw_region !== "function") return;
        for (const fixture of fixtures) {
            const {name, imageWidth = 96, imageHeight = 88, region, upload} = fixture;
            for (const initialized of [false, true]) {
                const canvas = document.createElement("canvas");
                const bench = new module.PresentationBenchmark(canvas,
                    imageWidth, imageHeight, 0, 0, imageWidth, imageHeight);
                try {
                    if (!initialized) {
                        // Exercise untouched framebuffer alpha as well as a transparent canvas.
                        bench.clear_source();
                        bench.paint(1, 1, imageWidth - 2, imageHeight - 2, 31);
                    }
                    const before = [bench.source_pixels(0), bench.source_pixels(1)];
                    if (initialized) bench.draw_batch(0, 0, imageWidth, imageHeight, 2);
                    const expected = initialized ? new Uint8ClampedArray(bench.expected_pixels(1)) :
                        new Uint8ClampedArray(imageWidth * imageHeight * 4);
                    let expectedBytes = 0;
                    if (upload) {
                        const [left, top, right, bottom] = upload;
                        expectedBytes = (right - left + 1) * (bottom - top + 1) * 4;
                        for (let y = top; y <= bottom; y++) {
                            for (let x = left; x <= right; x++) {
                                const offset = (y * imageWidth + x) * 4;
                                expected.set(before[0].subarray(offset, offset + 3), offset);
                                expected[offset + 3] = 255;
                            }
                        }
                    }
                    const bytes = bench.draw_region(...region);
                    const label = `${current.pair[index]} ${name} initialized=${initialized}`;
                    if (bytes !== expectedBytes) {
                        throw new Error(`${label}: buffer bytes ${bytes} != ${expectedBytes}`);
                    }
                    equal(pixels(canvas), expected, `${label} RGBA`);
                    for (const frameIndex of [0, 1]) {
                        equal(bench.source_pixels(frameIndex), before[frameIndex], `${label} source unchanged`);
                    }
                    results.push({variant: current.pair[index], case: name, initialized, imageWidth, imageHeight,
                        region, upload, buffer_bytes: bytes, no_op: upload === null,
                        rgba_checks: 1, source_unchanged: true, buffer_bytes_checked: true});
                } finally {
                    bench.free();
                }
            }
        }
    });
    return results;
}

window.checkCorrectness = ({extraFixtures = []} = {}) => {
    const checks = [];
    const fixtures = [1, 2, 3, 4, 5, 7, 15, 16, 17].map(width => ({
        imageWidth: 96, imageHeight: 88, left: 13, top: 7, width, height: 65,
    }));
    fixtures.push(
        {imageWidth: 1920, imageHeight: 1080, left: 13, top: 17, width: 64, height: 720},
        {imageWidth: 1920, imageHeight: 1080, left: 0, top: 0, width: 1920, height: 1080},
        {imageWidth: 96, imageHeight: 88, left: 13, top: 9, width: 32, height: 32},
    );
    const dimensions = ["imageWidth", "imageHeight", "left", "top", "width", "height"];
    for (const extra of extraFixtures) {
        if (!fixtures.some(existing => dimensions.every(key => existing[key] === extra[key]))) {
            fixtures.push(extra);
        }
    }
    for (const fixture of fixtures) {
        let reference;
        current.modules.forEach((module, index) => {
            const result = checkFixture(module, fixture, current.pair[index]);
            if (reference) equal(result, reference, "cross-variant initialized RGBA");
            else reference = result;
            checks.push({variant: current.pair[index], ...fixture, rgba_checks: 3, source_unchanged: true});
        });
    }

    const sparse = [];
    const sparseFixtures = [17, 80].map(width => ({
        imageWidth: 96, imageHeight: 88, left: 9, top: 5, width, height: 65,
    }));
    for (const extra of extraFixtures) {
        if (!sparseFixtures.some(existing => dimensions.every(key => existing[key] === extra[key]))) {
            sparseFixtures.push(extra);
        }
    }
    for (const resized of [false, true]) {
        for (const fixture of sparseFixtures) {
            const {left, top, width, height} = fixture;
            let reference;
            current.modules.forEach((module, index) => {
                const canvas = document.createElement("canvas");
                const bench = new module.PresentationBenchmark(canvas,
                    fixture.imageWidth, fixture.imageHeight, 0, 0, fixture.imageWidth, fixture.imageHeight);
                try {
                    const imageWidth = fixture.imageWidth + (resized && fixture.imageWidth < 65535 ? 1 : 0);
                    const imageHeight = fixture.imageHeight + (resized && fixture.imageHeight < 65533 ? 3 : 0);
                    if (resized) {
                        bench.draw_batch(0, 0, fixture.imageWidth, fixture.imageHeight, 2);
                        bench.resize(imageWidth, imageHeight);
                    } else {
                        bench.clear_source();
                    }
                    bench.paint(left, top, width, height, 31);
                    const before = [bench.source_pixels(0), bench.source_pixels(1)];
                    const expected = new Uint8ClampedArray(imageWidth * imageHeight * 4);
                    const wholeRows = (height > 64 || width > 512) && imageWidth * 4 <= width * 5;
                    for (let y = top; y < top + height; y++) {
                        for (let x = wholeRows ? 0 : left; x < (wholeRows ? imageWidth : left + width); x++) {
                            const offset = (y * imageWidth + x) * 4;
                            expected.set(before[0].subarray(offset, offset + 3), offset);
                            expected[offset + 3] = 255;
                        }
                    }
                    bench.draw_batch(left, top, width, height, 1);
                    const result = pixels(canvas);
                    equal(result, expected, `${current.pair[index]} sparse RGBA`);
                    if (reference) equal(result, reference, "cross-variant sparse RGBA");
                    else reference = result;
                    bench.draw_batch(0, 0, imageWidth, imageHeight, 1);
                    equal(pixels(canvas), bench.expected_pixels(1), "sparse full repaint");
                    for (const frameIndex of [0, 1]) {
                        equal(bench.source_pixels(frameIndex), before[frameIndex], "sparse source unchanged");
                    }
                    sparse.push({variant: current.pair[index], resized, imageWidth, imageHeight,
                        left, top, width, height, whole_rows: wholeRows,
                        rgba_checks: 2, source_unchanged: true});
                } finally {
                    bench.free();
                }
            });
        }
    }

    const probes = [];
    current.modules.forEach((module, index) => {
        let alphaChecks = 0, copyChecks = 0;
        for (let offset = 0; offset < 16; offset++) {
            for (let length = 0; length < 68; length++) {
                const input = Uint8Array.from({length: offset + length + 19}, (_, byte) => (byte * 37 + 19) & 255);
                const expected = input.slice();
                for (let byte = 3; byte < length; byte += 4) expected[offset + byte] = 255;
                equal(module.probe_alpha(input, offset, length), expected,
                    `${current.pair[index]} alpha offset ${offset} length ${length}`);
                alphaChecks++;
                for (let destinationOffset = 0; destinationOffset < 16; destinationOffset++) {
                    const destination = Uint8Array.from({length: destinationOffset + length + 19},
                        (_, byte) => (byte * 13 + 3) & 255);
                    const copied = destination.slice();
                    copied.set(input.subarray(offset, offset + length), destinationOffset);
                    for (let byte = 3; byte < length; byte += 4) copied[destinationOffset + byte] = 255;
                    equal(module.probe_copy(input, destination, offset, destinationOffset, length), copied,
                        `${current.pair[index]} copy offsets ${offset}/${destinationOffset} length ${length}`);
                    copyChecks++;
                }
            }
        }
        probes.push({variant: current.pair[index], alpha_checks: alphaChecks, copy_checks: copyChecks,
            offsets: [0, 15], lengths: [0, 67], prefix_suffix_and_incomplete_pixel_checked: true});
    });
    return {initialized: checks, sparse, probes, bounds: checkBounds()};
};

window.measureCase = async ({fixture, samples = 12, targetMs = 16, warmupMs = 400,
    blockOrder = 0, modeOrder = ["draw", "draw_js"], iterationsByMode = {}}) => {
    if (current.pair.length !== 2) throw new Error("timing requires exactly two variants");
    const {imageWidth, imageHeight, left, top, width, height} = fixture;
    // Each session checks its loaded modules before creating the timing context.
    current.modules.forEach((module, index) => checkFixture(module, fixture, current.pair[index]));
    const canvas = document.createElement("canvas");
    document.querySelector("main").replaceChildren(canvas);
    const benches = current.modules.map(module => new module.PresentationBenchmark(
        canvas, imageWidth, imageHeight, left, top, width, height));
    const region = [left, top, width, height];
    const operation = (variant, mode, iterations) => {
        const bench = benches[variant];
        if (mode === "prepare") return bench.copy_batch(...region, iterations);
        if (mode === "draw") return bench.draw_batch(...region, iterations);
        if (mode === "draw_js") {
            let bytes;
            for (let index = 0; index < iterations; index++) bytes = bench.draw_batch(...region, 1);
            return bytes;
        }
        throw new Error(`unknown mode ${mode}`);
    };
    const results = [];
    try {
        for (const bench of benches) bench.draw_batch(0, 0, imageWidth, imageHeight, 2);
        for (const mode of modeOrder) {
            let iterations = iterationsByMode[mode] || 2;
            if (!Number.isSafeInteger(iterations) || iterations < 2 || iterations % 2) {
                throw new Error("common batch size must be a positive even integer");
            }
            const calibration = [];
            if (!iterationsByMode[mode]) {
                // Calibrate only baseline builds. Use one common even count for A/B;
                // Python also reuses this count across subsequent fresh sessions.
                const baselineSlots = current.pair.map((name, index) => name === current.baseline ? index : -1)
                    .filter(index => index !== -1);
                if (!baselineSlots.length) throw new Error("calibration requires the baseline");
                for (;;) {
                    await frame();
                    const elapsed = baselineSlots.map(variant => {
                        const start = performance.now();
                        operation(variant, mode, iterations);
                        return performance.now() - start;
                    });
                    calibration.push({iterations, baseline_slots: baselineSlots, elapsed_ms: elapsed});
                    if (Math.max(...elapsed) >= targetMs || iterations >= 262144) break;
                    iterations *= 2;
                }
            }
            const warmedMs = [0, 0];
            let cycle = 0;
            while (Math.min(...warmedMs) < warmupMs) {
                await frame();
                const order = cycle++ % 2 ? [0, 1, 1, 0] : [1, 0, 0, 1];
                for (const variant of order) {
                    const start = performance.now();
                    operation(variant, mode, iterations);
                    warmedMs[variant] += performance.now() - start;
                }
            }
            const raw = [[], []];
            const blocks = [];
            for (let sample = 0; sample < samples; sample++) {
                const order = (sample + blockOrder) % 2 ? [0, 1, 1, 0] : [1, 0, 0, 1];
                const observations = [];
                for (const variant of order) {
                    await frame();
                    const start = performance.now();
                    const bytes = operation(variant, mode, iterations);
                    const elapsedMs = performance.now() - start;
                    const us = elapsedMs * 1000 / iterations;
                    raw[variant].push(us);
                    observations.push({slot: variant, variant: current.pair[variant], start_ms: start,
                        elapsed_ms: elapsedMs, per_update_us: us, returned_bytes: bytes});
                }
                const paired = [0, 1].map(slot => observations.filter(row => row.slot === slot)
                    .reduce((sum, row) => sum + row.per_update_us, 0) / 2);
                blocks.push({sample, order, observations, a_us: paired[0], b_us: paired[1],
                    ratio_b_over_a: paired[1] / paired[0]});
            }
            results.push({mode, iterations, calibration, reused_iterations: Boolean(iterationsByMode[mode]),
                warmed_ms: warmedMs, blocks, a: {...summary(raw[0]), samples_us: raw[0]},
                b: {...summary(raw[1]), samples_us: raw[1]}});
        }
        // Every call count is even, including JS-to-WASM calls, so the persistent
        // cursor ends on frame 1. This is the timing canvas's first readback.
        equal(pixels(canvas), benches[0].expected_pixels(1), "final timing canvas");
        return {fixture, pair: current.pair, samples, target_ms: targetMs, warmup_ms: warmupMs,
            mode_order: modeOrder, initial_block_order: blockOrder, shared_timing_canvas: true,
            final_rgba_equal: true, results};
    } finally {
        benches.forEach(bench => bench.free());
        canvas.remove();
    }
};
