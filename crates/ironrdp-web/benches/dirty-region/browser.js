import init, { Benchmark } from "./pkg/ironrdp_web_dirty_region_bench.js";

const policies = ["baseline", "cap1.25", "cap1.5", "cap2", "exact", "whole", "production"];
const cases = [
    [1920, 1080, 32, 32],
    [1920, 1080, 64, 720],
    [1920, 1080, 512, 720],
    [1920, 1080, 960, 720],
    [1920, 1080, 1280, 720],
    [1920, 1080, 1536, 720],
    [1920, 1080, 1800, 720],
    [1920, 1080, 1920, 1080],
    [1920, 1080, 640, 16],
    [1920, 1080, 1536, 16],
    [3840, 2160, 64, 1440],
    [3840, 2160, 3840, 2160],
];

const canvas = document.querySelector("canvas");
let random = 0x12345678;
function shuffledPolicies() {
    const order = policies.map((_, index) => index);
    for (let index = order.length - 1; index > 0; index--) {
        random ^= random << 13;
        random ^= random >>> 17;
        random ^= random << 5;
        const selected = (random >>> 0) % (index + 1);
        [order[index], order[selected]] = [order[selected], order[index]];
    }
    return order;
}

function readPixels() {
    return canvas.getContext("2d").getImageData(0, 0, canvas.width, canvas.height).data;
}

function assertPixels(actual, expected, label) {
    if (actual.length !== expected.length) throw new Error(`${label}: length mismatch`);
    for (let index = 0; index < actual.length; index++) {
        if (actual[index] !== expected[index]) {
            throw new Error(`${label}: byte ${index}: ${actual[index]} != ${expected[index]}`);
        }
    }
}

function correctness() {
    const checks = [];
    for (let policy = 0; policy < policies.length; policy++) {
        const bench = new Benchmark(canvas, 320, 240);
        for (const [width, height] of [[320, 240], [640, 360]]) {
            if (width === 640) bench.resize(width, height);
            bench.paint(0, 0, width, height, 1);
            bench.draw_batch(policy, 0, 0, width, height, 1);
            assertPixels(readPixels(), bench.expected_pixels(0), `${policies[policy]} initial ${width}`);
            const patches = [[13, 10, 16, 180], [0, 0, 32, 32], [230, 50, 80, 120]];
            for (let index = 0; index < patches.length; index++) {
                const patch = patches[index];
                bench.paint(...patch, 2 + index);
                bench.draw_batch(policy, ...patch, 1);
                assertPixels(readPixels(), bench.expected_pixels(0), `${policies[policy]} patch ${index} ${width}`);
            }
            checks.push({ policy: policies[policy], width, height, byte_exact_checks: 4 });
        }
        bench.free();
    }

    // No opaque initialization here: preserve the existing transparent-canvas behavior.
    const sparse = [];
    for (const resized of [false, true]) {
        const snapshots = [];
        const repaints = [];
        for (let policy = 0; policy < policies.length; policy++) {
            const bench = new Benchmark(canvas, resized ? 640 : 320, resized ? 360 : 240);
            if (resized) {
                bench.paint(0, 0, 640, 360, 1);
                bench.draw_batch(policy, 0, 0, 640, 360, 1);
                bench.resize(320, 240);
            }
            bench.paint(13, 10, 16, 180, 5);
            bench.draw_batch(policy, 13, 10, 16, 180, 1);
            snapshots.push(readPixels());
            bench.draw_batch(policy, 0, 0, 320, 240, 1);
            repaints.push(readPixels());
            bench.free();
        }
        for (let policy = 1; policy < policies.length; policy++) {
            let dirtyMismatches = 0;
            let rgbMismatches = 0;
            let alphaMismatches = 0;
            let legacyOpaqueCandidateTransparent = 0;
            for (let pixel = 0; pixel < 320 * 240; pixel++) {
                const x = pixel % 320;
                const y = Math.floor(pixel / 320);
                const dirty = x >= 13 && x < 29 && y >= 10 && y < 190;
                for (let channel = 0; channel < 4; channel++) {
                    const mismatch = snapshots[0][pixel * 4 + channel] !== snapshots[policy][pixel * 4 + channel];
                    if (mismatch && dirty) dirtyMismatches++;
                    if (mismatch && channel < 3) rgbMismatches++;
                    if (mismatch && channel === 3) alphaMismatches++;
                }
                if (!dirty && snapshots[0][pixel * 4 + 3] === 255 && snapshots[policy][pixel * 4 + 3] === 0) {
                    legacyOpaqueCandidateTransparent++;
                }
            }
            if (dirtyMismatches || rgbMismatches) throw new Error("sparse update changed dirty/RGB data");
            const expectedAlphaMismatches = policy === 5 ? 0 : (320 - 16) * 180;
            if (alphaMismatches !== expectedAlphaMismatches || alphaMismatches !== legacyOpaqueCandidateTransparent) {
                throw new Error(`${policies[policy]} sparse update changed unexpected alpha pixels`);
            }
            assertPixels(repaints[0], repaints[policy], "full repaint convergence");
            sparse.push({
                after_resize: resized,
                policy: policies[policy],
                dirty_byte_mismatches: dirtyMismatches,
                rgb_byte_mismatches: rgbMismatches,
                alpha_byte_mismatches_outside_dirty: alphaMismatches,
                legacy_opaque_candidate_transparent_pixels: legacyOpaqueCandidateTransparent,
                full_repaint_byte_mismatches: 0,
            });
        }
    }
    return { initialized_checks: checks, sparse_transparent_canvas: sparse };
}

function distribution(values) {
    const sorted = [...values].sort((a, b) => a - b);
    const quantile = (fraction) => sorted[Math.floor((sorted.length - 1) * fraction)];
    return { median_us: quantile(0.5), p10_us: quantile(0.1), p90_us: quantile(0.9) };
}

await init();
window.runBenchmark = async ({ samples = 20, targetMs = 8, correctnessOnly = false, selectedCases = null } = {}) => {
    const validation = correctness();
    if (correctnessOnly) return { validation };
    const results = [];
    const chosenCases = cases.filter(([iw, ih, w, h]) => !selectedCases || selectedCases.includes(`${iw}x${ih}/${w}x${h}`));
    if (chosenCases.length === 0) throw new Error("no benchmark cases selected");
    for (const [imageWidth, imageHeight, width, height] of chosenCases) {
        // Use a new canvas after correctness readbacks to avoid Chromium's adaptive
        // frequent-readback context behavior affecting the timing canvas.
        const timingCanvas = document.createElement("canvas");
        canvas.replaceWith(timingCanvas);
        const bench = new Benchmark(timingCanvas, imageWidth, imageHeight);
        const left = Math.floor((imageWidth - width) / 2);
        const top = Math.floor((imageHeight - height) / 2);
        bench.paint_common(imageWidth, imageHeight, 11);
        bench.paint(left, top, width, height, 12);
        // Establish an opaque initial screen, exactly as the initialized regression fixture.
        bench.draw_batch(4, 0, 0, imageWidth, imageHeight, 1);
        let finalFrame = 0;
        for (const mode of ["wasm_copy", "copy_and_canvas"]) {
            const draw = mode === "copy_and_canvas";
            const operation = draw ? bench.draw_batch.bind(bench) : bench.copy_batch.bind(bench);
            const counts = policies.map(() => 1);
            const raw = policies.map(() => []);
            const bytes = policies.map(() => 0);
            for (const policy of shuffledPolicies()) {
                for (;;) {
                    await new Promise(requestAnimationFrame);
                    const start = performance.now();
                    bytes[policy] = operation(policy, left, top, width, height, counts[policy]);
                    const elapsed = performance.now() - start;
                    if (elapsed >= targetMs || counts[policy] >= 16384) break;
                    counts[policy] *= 2;
                }
                operation(policy, left, top, width, height, counts[policy]);
            }
            for (let sample = 0; sample < samples; sample++) {
                for (const policy of shuffledPolicies()) {
                    await new Promise(requestAnimationFrame);
                    const start = performance.now();
                    bytes[policy] = operation(policy, left, top, width, height, counts[policy]);
                    raw[policy].push((performance.now() - start) * 1000 / counts[policy]);
                    if (draw) finalFrame = (counts[policy] - 1) % 2;
                }
            }
            for (let policy = 0; policy < policies.length; policy++) {
                results.push({
                    image_width: imageWidth, image_height: imageHeight, width, height, mode,
                    policy: policies[policy], iterations: counts[policy], bytes: bytes[policy],
                    ...distribution(raw[policy]), samples_us: raw[policy],
                });
            }
        }
        // Read back only after timing this canvas, avoiding frequent-readback
        // fallback during measurements. Compositor work is outside the claim.
        const finalPixels = timingCanvas.getContext("2d").getImageData(0, 0, imageWidth, imageHeight).data;
        const expected = bench.expected_pixels(finalFrame);
        assertPixels(finalPixels, expected, "timing canvas full final framebuffer");
        bench.free();
        timingCanvas.replaceWith(canvas);
        console.log(`completed ${imageWidth}x${imageHeight} / ${width}x${height}`);
    }
    return { validation, results, user_agent: navigator.userAgent, samples, target_ms: targetMs, random_seed: "0x12345678" };
};
