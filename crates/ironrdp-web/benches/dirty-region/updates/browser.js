// Synthetic workloads only: no network, RDP packet parsing, video codec, or input transport.
let wasm;
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const frame = () => new Promise(requestAnimationFrame);
const statNames = ["applied_updates", "draw_calls", "uploaded_bytes", "flush_batches", "forced_flushes", "max_queue", "terminal_flushes"];
const stats = bench => Object.fromEntries(Array.from(bench.stats(), (value, index) => [statNames[index], value]));
const pixels = canvas => canvas.getContext("2d").getImageData(0, 0, canvas.width, canvas.height).data;
const packetCounts = {single: [1, 1], repeat: [8, 1], contained: [4, 1], overlap: [2, 2], disjoint: [16, 16], overflow: [65, 65], barrier: [3, 2]};

function messageTaskYield() {
    const channel = new MessageChannel();
    let pending;
    channel.port1.onmessage = () => {
        const resolve = pending;
        pending = undefined;
        resolve();
    };
    return {
        wait: () => new Promise(resolve => {
            if (pending) throw new Error("overlapping cooperative yields");
            pending = resolve;
            channel.port2.postMessage(null);
        }),
        close: () => {
            channel.port1.close();
            channel.port2.close();
        },
    };
}

function equal(actual, expected, label) {
    if (actual.length !== expected.length) throw new Error(`${label}: lengths differ`);
    for (let index = 0; index < actual.length; index++) {
        if (actual[index] !== expected[index]) throw new Error(`${label}: byte ${index} differs`);
    }
}

function packetStats(bench, name, coalesce, iterations) {
    const result = stats(bench);
    const [notifications, retained] = packetCounts[name];
    if (result.applied_updates !== notifications * iterations || result.draw_calls !== (coalesce ? retained : notifications) * iterations) {
        throw new Error(`${name}: unexpected notification/draw counts ${JSON.stringify(result)}`);
    }
    if (result.forced_flushes !== (coalesce && name === "overflow" ? iterations : 0) || result.max_queue > 64) {
        throw new Error(`${name}: invalid overflow or queue bound`);
    }
    return result;
}

window.prepare = async packageRoot => {
    wasm = await import(`${packageRoot}/ironrdp_web_dirty_region_bench.js`);
    await wasm.default();
    return {user_agent: navigator.userAgent, cross_origin_isolated: crossOriginIsolated,
        hardware_concurrency: navigator.hardwareConcurrency};
};

window.probeYields = async (count = 32) => {
    const channel = messageTaskYield();
    const observations = [];
    try {
        for (const kind of ["timer", "channel", "channel", "timer"]) {
            const start = performance.now();
            for (let index = 0; index < count; index++) {
                if (kind === "timer") await sleep(0);
                else await channel.wait();
            }
            observations.push({kind, count, elapsed_ms: performance.now() - start});
        }
        const rustProbeAvailable = typeof wasm.probe_browser_yield === "function";
        if (rustProbeAvailable) {
            const start = performance.now();
            const completed = await wasm.probe_browser_yield(count);
            if (completed !== count) throw new Error("Rust BrowserYield probe lost a wakeup");
            observations.push({kind: "production_rust_channel", count, elapsed_ms: performance.now() - start,
                includes_cancel_reuse_drop_checks: true});
        }
        return {observations, rust_probe_available: rustProbeAvailable,
            scope: "no-work task-chain overhead; outside all workload timing"};
    } finally {
        channel.close();
    }
};

window.checkPackets = () => {
    const results = [];
    for (const name of Object.keys(packetCounts)) {
        for (const sparse of [false, true]) {
            let reference;
            for (const coalesce of [false, true]) {
                const canvas = document.createElement("canvas");
                const bench = new wasm.PacketBenchmark(canvas, name, 320, 240, sparse);
                try {
                    const source = [bench.source_pixels(0), bench.source_pixels(1)];
                    bench.draw_batch(coalesce, 3);
                    const result = pixels(canvas);
                    equal(result, bench.expected_pixels(), `${name} initialized/sparse`);
                    if (reference) equal(result, reference, `${name} cross-policy`);
                    else reference = result;
                    for (const index of [0, 1]) equal(bench.source_pixels(index), source[index], "source unchanged");
                    const initial = packetStats(bench, name, coalesce, 3);
                    bench.resize(name, 337, 251);
                    bench.draw_batch(coalesce, 1);
                    equal(pixels(canvas), bench.expected_pixels(), `${name} resized sparse`);
                    results.push({name, sparse, coalesce, initial, resized: packetStats(bench, name, coalesce, 1),
                        rgba_equal: true, source_unchanged: true});
                } finally {
                    bench.free();
                }
            }
        }
    }
    return results;
};

window.measurePacket = async ({name, pair, samples = 12, targetMs = 16, warmupMs = 400,
    iterations: reusedIterations, blockOrder = 0}) => {
    const canvas = document.createElement("canvas");
    document.querySelector("main").replaceChildren(canvas);
    const benches = pair.map(() => new wasm.PacketBenchmark(canvas, name, 1920, 1080, false));
    const policies = pair.map(policy => policy === "coalesced");
    let iterations = reusedIterations || 2;
    try {
        if (!reusedIterations) {
            const baseline = pair.indexOf("baseline");
            if (baseline < 0) throw new Error("missing baseline calibration slot");
            for (;;) {
                await frame();
                const start = performance.now();
                benches[baseline].draw_batch(false, iterations);
                if (performance.now() - start >= targetMs || iterations >= 262144) break;
                iterations *= 2;
            }
        }
        const warmed = [0, 0];
        let cycle = 0;
        while (Math.min(...warmed) < warmupMs) {
            for (const slot of cycle++ % 2 ? [0, 1, 1, 0] : [1, 0, 0, 1]) {
                await frame();
                const start = performance.now();
                benches[slot].draw_batch(policies[slot], iterations);
                warmed[slot] += performance.now() - start;
            }
        }
        benches.forEach(bench => bench.clear_stats());
        const blocks = [];
        for (let sample = 0; sample < samples; sample++) {
            const order = (sample + blockOrder) % 2 ? [0, 1, 1, 0] : [1, 0, 0, 1];
            const observations = [];
            for (const slot of order) {
                await frame();
                const start = performance.now();
                benches[slot].draw_batch(policies[slot], iterations);
                observations.push({slot, per_packet_us: (performance.now() - start) * 1000 / iterations});
            }
            const times = [0, 1].map(slot => observations.filter(row => row.slot === slot)
                .reduce((total, row) => total + row.per_packet_us, 0) / 2);
            blocks.push({order, observations, a_us: times[0], b_us: times[1]});
        }
        // Counts are even, so either policy leaves the same final framebuffer on the shared Canvas.
        equal(pixels(canvas), benches[0].expected_pixels(), "final timed packet Canvas");
        return {name, pair, iterations, blocks, warmed_ms: warmed, rgba_equal: true,
            counts: benches.map((bench, index) => packetStats(bench, name, policies[index], 2 * samples * iterations))};
    } finally {
        benches.forEach(bench => bench.free());
        canvas.remove();
    }
};

function quantile(values, fraction) {
    if (!values.length) return null;
    const sorted = [...values].sort((a, b) => a - b);
    return sorted[Math.min(sorted.length - 1, Math.floor(fraction * sorted.length))];
}

async function readyRun({name, total, width, height, policy, resize = false}) {
    const canvas = document.createElement("canvas");
    document.querySelector("main").replaceChildren(canvas);
    const paced = policy !== "baseline";
    const fps = policy === "paced30" ? 30 : 60;
    const bench = new wasm.ReadyQueueBenchmark(canvas, name, width, height, total, fps);
    let heartbeat;
    let marker;
    let markerDelay;
    const heartbeatGaps = [];
    try {
        if (resize) {
            for (let index = 0; index < Math.min(3, total); index++) bench.step(true);
            bench.finish(performance.now());
            equal(pixels(canvas), bench.source_pixels(), "flush old framebuffer before replacement");
            bench.restart_after_resize();
        }
        await frame();
        const started = performance.now();
        let lastHeartbeat = started;
        heartbeat = setInterval(() => {
            const now = performance.now();
            heartbeatGaps.push(now - lastHeartbeat);
            lastHeartbeat = now;
        }, 4);
        marker = setTimeout(() => { markerDelay = performance.now() - started; }, 0);
        // One WASM call runs the workload. Rust owns its presentation timer, select loop,
        // work budget and production BrowserYield; JS observes browser task responsiveness.
        const [applyMs, finishMs, firstDraw, firstDrawUpdate, yields, timerEvents, activeMs] = await bench.run(paced);
        const finished = performance.now();
        const count = stats(bench);
        // Include the first heartbeat/marker released by a previously blocking baseline loop.
        await sleep(5);
        clearInterval(heartbeat);
        clearTimeout(marker);
        const idleObservationMs = Math.ceil(1000 / fps) + 5;
        await sleep(idleObservationMs);
        if (bench.pending_delay_ms(performance.now()) !== undefined || JSON.stringify(stats(bench)) !== JSON.stringify(count)) {
            throw new Error("presentation state changed after terminal flush");
        }
        if (count.applied_updates !== total || firstDrawUpdate !== 1 || count.max_queue > 64) {
            throw new Error(`ready-queue count/first-paint failure: ${JSON.stringify(count)}`);
        }
        if (!paced && count.draw_calls !== total) throw new Error("baseline lost a draw");
        if (paced && count.forced_flushes === 0 && count.flush_batches > Math.ceil(finishMs * fps / 1000) + 1) {
            throw new Error("unexpected presentation rate without overflow");
        }
        equal(bench.source_pixels(), bench.expected_pixels(), "all synthetic updates applied");
        equal(pixels(canvas), bench.expected_pixels(), "final ready-queue Canvas");
        return {policy, total, workload: name, width, height, resize, fps: paced ? fps : null,
            apply_ms: applyMs, finish_ms: finishMs, active_work_ms: activeMs, wall_finish_ms: finished - started,
            first_draw_completion_ms: firstDraw, first_draw_after_updates: firstDrawUpdate,
            task_marker_delay_ms: markerDelay, heartbeat_samples: heartbeatGaps.length,
            heartbeat_gap_p50_ms: quantile(heartbeatGaps, 0.5), heartbeat_gap_p95_ms: quantile(heartbeatGaps, 0.95),
            heartbeat_gap_max_ms: heartbeatGaps.length ? Math.max(...heartbeatGaps) : null,
            cooperative_yields: yields, yield_budget_ms: paced ? 8 : null,
            yield_mechanism: paced ? "production Rust BrowserYield" : null,
            handled_presentation_timer_events: timerEvents, pending_empty: true,
            idle_observation_ms: idleObservationMs,
            counts: count, rgba_equal: true, source_equal: true};
    } finally {
        clearInterval(heartbeat);
        clearTimeout(marker);
        bench.free();
        canvas.remove();
    }
}

window.checkPacing = async () => {
    const results = [];
    for (const name of ["full", "narrow", "partial"]) {
        for (const policy of ["baseline", "paced60", "paced30"]) {
            for (const resize of [false, true]) {
                results.push(await readyRun({name, policy, resize, total: name === "partial" ? 67 : 7,
                    width: 320, height: name === "narrow" ? 768 : 240}));
            }
        }
    }
    return results;
};

window.checkIdleTimers = async () => {
    const results = [];
    for (const terminate of [false, true]) {
        const canvas = document.createElement("canvas");
        // A longer interval isolates delivery/cancellation from cold Canvas startup cost.
        const bench = new wasm.ReadyQueueBenchmark(canvas, "full", 320, 240, 4, 5);
        try {
            const [timerEvents, originalDelay, unchangedDelay, idleObservationMs, beforeIdle, afterIdle] =
                await bench.probe_idle_timer(terminate);
            const count = stats(bench);
            if (timerEvents !== (terminate ? 0 : 1) || originalDelay !== unchangedDelay ||
                beforeIdle !== 2 || afterIdle !== 2 || count.applied_updates !== 4 || count.draw_calls !== 2 ||
                count.flush_batches !== 2 || count.forced_flushes !== 0 || count.max_queue !== 1 ||
                count.terminal_flushes !== (terminate ? 1 : 0) ||
                bench.pending_delay_ms(performance.now()) !== undefined) {
                throw new Error(`idle timer probe failed: ${JSON.stringify(count)}`);
            }
            equal(bench.source_pixels(), bench.expected_pixels(), "idle timer applied all updates");
            equal(pixels(canvas), bench.expected_pixels(), "idle timer final Canvas");
            results.push({kind: terminate ? "termination_cancels_timer" : "idle_timer_flush",
                interval_ms: 200, requested_followup_delay_ms: 10, handled_presentation_timer_events: timerEvents,
                original_delay_ms: originalDelay, delay_at_same_timestamp_after_updates_ms: unchangedDelay,
                idle_observation_ms: idleObservationMs, draws_before_idle: beforeIdle, draws_after_idle: afterIdle,
                counts: count, rgba_equal: true, source_equal: true, pending_empty: true,
                scope: "production presentation helpers with a synthetic Rust timeout/select driver; not the Session loop"});
        } finally {
            bench.free();
        }
    }
    return results;
};

window.measurePacing = async ({name, pair, blockOrder = 0, fullUpdates = 96, narrowUpdates = 1024, partialUpdates = 16384}) => {
    const order = blockOrder % 2 ? [0, 1, 1, 0] : [1, 0, 0, 1];
    const observations = [];
    const warmupUpdates = name === "full" ? 16 : 256;
    for (const policy of pair) {
        await readyRun({name, policy, total: warmupUpdates, width: 1920, height: 1080});
    }
    for (const slot of order) {
        observations.push({slot, result: await readyRun({name, policy: pair[slot],
            total: name === "full" ? fullUpdates : name === "narrow" ? narrowUpdates : partialUpdates, width: 1920, height: 1080})});
    }
    return {name, pair, order, warmup_updates: warmupUpdates, observations};
};
