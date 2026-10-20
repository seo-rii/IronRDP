// Only the established transport, browser clock and fault injection are scripted.
// Session::run, Framed, ActiveStage, Canvas, futures and their Drop paths are real.
const real = {
    WebSocket, MessageChannel, setTimeout: setTimeout.bind(window), clearTimeout: clearTimeout.bind(window),
    setInterval: setInterval.bind(window), clearInterval: clearInterval.bind(window), now: performance.now.bind(performance),
    putImageData: CanvasRenderingContext2D.prototype.putImageData,
};
const task = () => new Promise(resolve => real.setTimeout(resolve, 0));
function assert(value, message) {
    if (!value) throw new Error(message);
}
function same(actual, expected, label) {
    assert(actual.length === expected.length, `${label}: length`);
    for (let index = 0; index < actual.length; index++) {
        if (actual[index] !== expected[index]) throw new Error(`${label}: byte ${index}: ${actual[index]} != ${expected[index]}`);
    }
}

const clock = {
    now: 0, step: 0, next: 1, timeouts: new Map(), intervals: new Map(), fired: 0, canceled: 0,
    canceledIntervals: 0,
    advance(value) {
        this.now = value;
        const ready = [...this.timeouts.entries()].filter(([, timer]) => timer.at <= this.now);
        for (const [id, timer] of ready) {
            if (!this.timeouts.delete(id)) continue;
            this.fired++;
            timer.callback(...timer.args);
        }
    },
    reset() {
        assert(this.timeouts.size === 0 && this.intervals.size === 0, "previous session left browser timers alive");
        this.now = 0; this.step = 0; this.fired = 0; this.canceled = 0; this.canceledIntervals = 0;
    },
};
const sockets = [];
class ScriptedWebSocket extends EventTarget {
    static CONNECTING = 0; static OPEN = 1; static CLOSING = 2; static CLOSED = 3;
    constructor(url) {
        super();
        this.url = url; this.readyState = 1; this.binaryType = "arraybuffer";
        this.bufferedAmount = 0; this.extensions = ""; this.protocol = ""; this.sent = []; this.receivedFrames = 0;
        this.events = []; this.closeCalls = 0;
        sockets.push(this);
        queueMicrotask(() => this.dispatchEvent(new Event("open")));
    }
    // gloo-net sends Blob objects. Retain the actual write without asynchronous conversion.
    send(data) {
        assert(this.readyState === ScriptedWebSocket.OPEN, "writer sent after its WebSocket closed");
        this.sent.push(data);
        this.events.push({kind: "send", sent: this.sent.length});
    }
    push(bytes) {
        this.receivedFrames++;
        this.dispatchEvent(new MessageEvent("message", {data: Uint8Array.from(bytes).buffer}));
    }
    close() {
        this.closeCalls++;
        if (this.readyState === 3) return;
        this.events.push({kind: "close", sent: this.sent.length});
        this.readyState = 3;
        queueMicrotask(() => this.dispatchEvent(new CloseEvent("close", {code: 1000, wasClean: true})));
    }
}
const channels = [];
let holdMessages = false;
const heldMessages = [];
const observedCanvases = new WeakMap();
function installEnvironment() {
    window.WebSocket = ScriptedWebSocket;
    Object.defineProperty(performance, "now", {configurable: true, value() {
        const value = clock.now; clock.now += clock.step; return value;
    }});
    window.setTimeout = (callback, delay = 0, ...args) => {
        const id = clock.next++;
        clock.timeouts.set(id, {at: clock.now + Number(delay), callback, args});
        return id;
    };
    window.clearTimeout = id => { if (clock.timeouts.delete(id)) clock.canceled++; };
    window.setInterval = (callback, delay = 0, ...args) => {
        const id = clock.next++;
        clock.intervals.set(id, {callback, delay, args});
        return id;
    };
    window.clearInterval = id => { if (clock.intervals.delete(id)) clock.canceledIntervals++; };
    window.MessageChannel = function () {
        const channel = new real.MessageChannel();
        const state = {channel, closed: new Set(), posts: 0};
        channels.push(state);
        for (const port of [channel.port1, channel.port2]) {
            const close = port.close.bind(port);
            port.close = () => { state.closed.add(port); close(); };
            const post = port.postMessage.bind(port);
            port.postMessage = (...args) => {
                state.posts++;
                if (holdMessages) heldMessages.push(() => post(...args));
                else post(...args);
            };
        }
        return channel;
    };
    CanvasRenderingContext2D.prototype.putImageData = function (image, x, y, ...rest) {
        const state = observedCanvases.get(this.canvas);
        if (state) {
            state.attempts.push({x, y, width: image.width, height: image.height, at: clock.now});
            if (state.failDraw || state.failAttempts.has(state.attempts.length)) {
                throw new Error(`session-fixture-draw-failure-${state.attempts.length}`);
            }
        }
        const result = real.putImageData.call(this, image, x, y, ...rest);
        if (state) state.draws++;
        return result;
    };
}
function restoreEnvironment() {
    window.WebSocket = real.WebSocket; window.MessageChannel = real.MessageChannel;
    window.setTimeout = real.setTimeout; window.clearTimeout = real.clearTimeout;
    window.setInterval = real.setInterval; window.clearInterval = real.clearInterval;
    Object.defineProperty(performance, "now", {configurable: true, value: real.now});
    CanvasRenderingContext2D.prototype.putImageData = real.putImageData;
}

async function until(predicate, label) {
    for (let iteration = 0; iteration < 80; iteration++) {
        if (predicate()) return;
        await task();
    }
    throw new Error(`session did not reach ${label}`);
}
function start(module, width = 64, height = 48, options = {}) {
    clock.reset();
    const canvas = document.createElement("canvas");
    if (options.failCanvasContext) {
        canvas.getContext = () => { throw new Error("session-fixture-canvas-context-failure"); };
    }
    document.querySelector("main").replaceChildren(canvas);
    const cursorUpdates = [];
    const fixture = new module.SessionFixture(canvas, "ws://session-fixture.invalid/established", width, height,
        (kind, data, hotspotX, hotspotY) => cursorUpdates.push({kind, data, hotspotX, hotspotY}));
    const state = {canvas, fixture, socket: sockets.at(-1), width, height, attempts: [], draws: 0, failDraw: false,
        failAttempts: new Set(), cursorUpdates,
        expected: new Uint8ClampedArray(width * height * 4), channelStart: channels.length, outcome: null};
    observedCanvases.set(canvas, state);
    state.done = fixture.run().then(
        value => { state.outcome = {ok: true, value}; },
        error => { state.outcome = {ok: false, error: String(error)}; },
    );
    return state;
}
function paint(state, left, top, width, height, color) {
    const rgba = [(color >> 16) & 255, (color >> 8) & 255, color & 255, 255];
    for (let y = top; y < top + height; y++) {
        for (let x = left; x < left + width; x++) state.expected.set(rgba, (y * state.width + x) * 4);
    }
}
function check(state, label) {
    assert(state.canvas.width === state.width && state.canvas.height === state.height, `${label}: canvas size`);
    same(state.canvas.getContext("2d").getImageData(0, 0, state.width, state.height).data, state.expected, label);
}
async function first(module, state, full = false) {
    await task();
    const region = full ? [0, 0, state.width, state.height] : [1, 1, 3, 3];
    state.socket.push(module.session_fixture_bitmap(...region, 0xDD2211));
    await until(() => state.draws === 1 || state.outcome, "first frame");
    assert(!state.outcome, `first frame failed: ${JSON.stringify(state.outcome)}`);
    assert(clock.timeouts.size === 0 && clock.intervals.size === 1, "first frame left an unexpected timer");
    paint(state, ...region, 0xDD2211);
    check(state, "first frame");
}
async function second(module, state, paced) {
    clock.now = 1;
    state.socket.push(module.session_fixture_bitmap(8, 3, 3, 3, 0x22CC44));
    await task();
    if (paced) {
        assert(state.draws === 1 && clock.timeouts.size === 1, "second frame should wait for one presentation timer");
    } else {
        assert(state.draws === 2 && clock.timeouts.size === 0, "default presentation should not create a timer");
        paint(state, 8, 3, 3, 3, 0x22CC44);
    }
}
async function finished(state, expectedError = null) {
    await until(() => state.outcome, "session completion");
    if (expectedError) {
        assert(!state.outcome.ok && state.outcome.error.includes(expectedError), `wrong error: ${JSON.stringify(state.outcome)}`);
    } else {
        assert(state.outcome.ok, `unexpected session failure: ${JSON.stringify(state.outcome)}`);
    }
    // Keep the Session object alive: freeing it here would hide a writer that only
    // releases its WebSocket when the UI eventually discards the Session.
    await until(() => state.socket.readyState === ScriptedWebSocket.CLOSED, "WebSocket close before Session free");
    assert(state.socket.closeCalls === 1, "run completion did not close its WebSocket exactly once");
    assert(clock.timeouts.size === 0 && clock.intervals.size === 0, "completed session left timers alive");
    const draws = state.attempts.length;
    clock.advance(1000);
    await task();
    assert(state.attempts.length === draws, "completed session performed a late upload");
    for (const item of channels.slice(state.channelStart)) {
        assert(item.closed.size === 2 && item.channel.port1.onmessage === null, "completed session left yield callbacks alive");
    }
    state.fixture.free();
    await task();
    assert(state.socket.closeCalls === 1, "freeing a completed Session closed its WebSocket again");
    return {outcome: state.outcome, draw_attempts: draws, successful_uploads: state.draws,
        canceled_timers: clock.canceled, fired_timers: clock.fired,
        canceled_cleanup_intervals: clock.canceledIntervals, received_frames: state.socket.receivedFrames,
        socket_closed_before_free: true, socket_close_calls: state.socket.closeCalls, outbound_frames: state.socket.sent.length};
}

window.runSessionChecks = async ({packageRoot, expectedPacing}) => {
    const module = await import(`${packageRoot}/ironrdp_web.js`);
    await module.default();
    assert(module.session_fixture_simd_enabled() === true, "fixture did not compile SIMD");
    assert(module.session_fixture_pacing_enabled() === expectedPacing, "wrong pacing feature");
    const results = [];
    installEnvironment();
    try {
        for (const [width, height, hotspotX, hotspotY, pngWidth, pngHeight, pngHotspotX, pngHotspotY] of [
            [1, 64, 0, 63, 1, 32, 0, 31],
            [64, 1, 63, 0, 32, 1, 31, 0],
            [1, 32, 0, 31, 1, 32, 0, 31],
            [2, 64, 1, 63, 1, 32, 0, 31],
            [3, 64, 2, 63, 1, 32, 0, 31],
        ]) {
            const state = start(module); await task();
            state.socket.push(module.session_fixture_pointer(width, height, hotspotX, hotspotY));
            await until(() => state.cursorUpdates.length || state.outcome, "cursor callback");
            assert(!state.outcome, `cursor ${width}x${height} terminated the session: ${JSON.stringify(state.outcome)}`);
            assert(state.cursorUpdates.length === 1, "pointer update did not produce exactly one cursor callback");
            const cursor = state.cursorUpdates[0];
            assert(cursor.kind === "url" && cursor.data.startsWith("data:image/png;base64,"), "cursor is not a PNG URL");
            const png = new Image();
            png.src = cursor.data;
            await png.decode();
            assert(png.naturalWidth === pngWidth && png.naturalHeight === pngHeight,
                `cursor ${width}x${height}: incorrect PNG dimensions`);
            assert(cursor.hotspotX === pngHotspotX && cursor.hotspotY === pngHotspotY,
                `cursor ${width}x${height}: incorrect scaled hotspot`);
            assert(cursor.hotspotX < png.naturalWidth && cursor.hotspotY < png.naturalHeight,
                "cursor hotspot lies outside its PNG");
            const decoded = document.createElement("canvas");
            decoded.width = pngWidth; decoded.height = pngHeight;
            const context = decoded.getContext("2d");
            context.drawImage(png, 0, 0);
            const pixels = context.getImageData(0, 0, pngWidth, pngHeight).data;
            const expectedPixels = new Uint8ClampedArray(pixels.length);
            for (let offset = 0; offset < expectedPixels.length; offset += 4) {
                expectedPixels.set([0xCC, 0x66, 0x33, 0xFF], offset);
            }
            same(pixels, expectedPixels, `cursor ${width}x${height} retains opaque RGB`);
            assert(state.draws === 0 && clock.timeouts.size === 0, "pointer update created a framebuffer presentation");
            check(state, "pointer leaves framebuffer untouched");
            await first(module, state);
            state.socket.push(module.session_fixture_disconnect());
            results.push({case: `cursor-${width}x${height}`, png_width: png.naturalWidth, png_height: png.naturalHeight,
                hotspot_x: cursor.hotspotX, hotspot_y: cursor.hotspotY, ...await finished(state)});
        }
        {
            const state = start(module, 64, 48, {failCanvasContext: true});
            const result = await finished(state, "session-fixture-canvas-context-failure");
            assert(state.attempts.length === 0 && state.socket.sent.length === 0,
                "Canvas initialization failure performed session work");
            results.push({case: "canvas-init-error-closes-writer", ...result});
        }
        {
            const state = start(module); await first(module, state);
            let secondRunError = null;
            try { await state.fixture.run_again(); } catch (error) { secondRunError = String(error); }
            assert(secondRunError?.includes("RDP session can be started only once"), "second run was not rejected");
            assert(!state.outcome && state.socket.readyState === ScriptedWebSocket.OPEN,
                "rejected second run closed the running session");
            await second(module, state, expectedPacing);
            clock.advance(20); await task();
            if (expectedPacing) paint(state, 8, 3, 3, 3, 0x22CC44);
            check(state, "first run continues after second run rejected");
            state.socket.push(module.session_fixture_disconnect());
            results.push({case: "second-run-preserves-active-writer", ...await finished(state)});
        }
        {
            const state = start(module); await first(module, state);
            const writes = state.socket.sent.length;
            // Wake the session first, then queue a write without letting the writer run.
            // Closing its channel must preserve this frame for the writer to drain.
            state.socket.push(module.session_fixture_disconnect());
            state.fixture.synchronize_lock_keys();
            assert(state.socket.sent.length === writes, "writer drain fixture yielded before queuing its frame");
            const result = await finished(state);
            assert(state.socket.sent.length === writes + 1, "run completion discarded a queued outgoing frame");
            const lastEvents = state.socket.events.slice(-2);
            assert(lastEvents[0]?.kind === "send" && lastEvents[1]?.kind === "close"
                && lastEvents[1].sent === writes + 1, "WebSocket closed before its queued frame was sent");
            results.push({case: "writer-drains-queued-frame", ...result});
        }
        {
            const state = start(module); await first(module, state); await second(module, state, expectedPacing);
            const received = state.socket.receivedFrames;
            clock.advance(20); await task();
            if (expectedPacing) paint(state, 8, 3, 3, 3, 0x22CC44);
            check(state, "idle timer flush");
            assert(state.draws === 2, "idle session did not present its last update");
            assert(state.socket.receivedFrames === received && received === 2, "idle presentation needed another server frame");
            assert(clock.fired === Number(expectedPacing) && clock.timeouts.size === 0, "idle presentation left a timer armed");
            state.socket.push(module.session_fixture_disconnect());
            results.push({case: "idle-presentation", ...await finished(state)});
        }
        {
            const state = start(module); await first(module, state); await second(module, state, expectedPacing);
            // Make the real timeout future and framed WebSocket read ready in the same JS task.
            clock.advance(20);
            state.socket.push(module.session_fixture_bitmap(8, 3, 3, 3, 0x2244EE));
            await task(); clock.advance(40); await task();
            paint(state, 8, 3, 3, 3, 0x2244EE); check(state, "timer and server ready");
            assert(state.draws >= 2 && state.draws <= 3, "ready timer caused duplicate or missing presentation");
            assert(clock.timeouts.size === 0, "simultaneously ready timer and frame left a timer armed");
            state.socket.push(module.session_fixture_disconnect());
            results.push({case: "timer-and-server-ready", ...await finished(state)});
        }
        {
            const state = start(module); await first(module, state); await second(module, state, expectedPacing);
            state.socket.push(module.session_fixture_disconnect());
            const result = await finished(state);
            if (expectedPacing) paint(state, 8, 3, 3, 3, 0x22CC44);
            check(state, "termination flush");
            results.push({case: "termination-flush", ...result});
        }
        {
            const state = start(module); await first(module, state); await second(module, state, expectedPacing);
            state.failDraw = true;
            state.socket.push(module.session_fixture_disconnect());
            const result = await finished(state, expectedPacing ? "session-fixture-draw-failure-2" : null);
            assert(state.attempts.length === 2, "termination retried an upload or skipped its pending region");
            check(state, "failed termination presentation");
            results.push({case: "termination-draw-error", ...result});
        }
        {
            const state = start(module); await first(module, state); await second(module, state, expectedPacing);
            state.failDraw = true;
            state.socket.push(module.session_fixture_invalid_frame());
            await until(() => state.outcome, "protocol failure");
            assert(!state.outcome.ok && !state.outcome.error.includes("session-fixture-draw-failure"),
                `final draw error replaced protocol error: ${JSON.stringify(state.outcome)}`);
            const result = await finished(state, "Invalid update code");
            assert(state.attempts.length === 2, "protocol error skipped or retried final presentation");
            assert(state.draws === (expectedPacing ? 1 : 2), "protocol-error fixture did not exercise the final draw failure");
            results.push({case: "protocol-error-preserved", ...result});
        }
        {
            const state = start(module); await first(module, state);
            state.failDraw = true;
            clock.now = 20;
            state.socket.push(module.session_fixture_bitmap(8, 3, 3, 3, 0x22CC44));
            const result = await finished(state, "session-fixture-draw-failure");
            assert(state.attempts.length === 2, "failed upload was retried by final cleanup");
            results.push({case: "draw-error-not-retried", ...result});
        }
        for (const {name, count, failures} of [
            {name: "region-capacity-overflow", count: 65, failures: []},
            {name: "region-capacity-66", count: 66, failures: []},
            {name: "region-capacity-130", count: 130, failures: []},
            {name: "overflow-error-preserves-region", count: 65, failures: Array.from({length: 65}, (_, index) => index + 1)},
            ...[1, 32, 64].map(attempt => ({name: `overflow-66-error-${attempt}`, count: 66, failures: [attempt]})),
            ...[65, 96, 128].map(attempt => ({name: `overflow-130-error-${attempt}`, count: 130, failures: [attempt]})),
            {name: "overflow-130-repeated-errors", count: 130, failures: [1, 32, 64, 65, 96, 128, 129, 130]},
            {name: "overflow-130-all-errors", count: 130, failures: Array.from({length: 130}, (_, index) => index + 1)},
        ]) {
            const state = start(module, 64, Math.ceil(count / 16) * 4); await task();
            state.failAttempts = new Set(failures);
            const tiles = module.session_fixture_tiles(count);
            if (failures.length) {
                // Framed already has another complete PDU buffered. Error cleanup must finish
                // the current PDU's graphics without progressing to this distinct update.
                const next = module.session_fixture_bitmap(1, 1, 1, 1, 0x11EECC);
                const frames = new Uint8Array(tiles.length + next.length);
                frames.set(tiles); frames.set(next, tiles.length);
                state.socket.push(frames);
            } else {
                state.socket.push(tiles);
                await task(); clock.advance(20); await task();
                assert(!state.outcome, `${name}: successful overflow terminated the session`);
                state.socket.push(module.session_fixture_disconnect());
            }
            const result = await finished(state, failures.length ? `session-fixture-draw-failure-${failures[0]}` : null);
            assert(state.attempts.length === count && state.draws === count - failures.length,
                `${name}: skipped a dirty region, retried an upload, or consumed the next PDU`);
            for (let index = 0; index < count; index++) {
                const x = (index % 16) * 4, y = Math.floor(index / 16) * 4;
                const attempt = state.attempts[index];
                assert(attempt.x === x && attempt.y === y && attempt.width === 1 && attempt.height === 1,
                    `${name}: upload ${index + 1} changed order or coverage`);
                if (!state.failAttempts.has(index + 1)) paint(state, x, y, 1, 1, 0xCC5533);
            }
            if (failures.length) {
                const errors = [...state.outcome.error.matchAll(/session-fixture-draw-failure-(\d+)/g)];
                assert(errors.length > 0 && errors.every(match => Number(match[1]) === failures[0]),
                    `${name}: later drawing replaced the first error`);
            }
            check(state, name);
            results.push({case: name, injected_failures: failures, ...result});
        }
        {
            const state = start(module); await first(module, state); await second(module, state, expectedPacing);
            state.fixture.abort();
            const result = await finished(state);
            assert(state.outcome.value === "fixture aborted", "run future was not canceled");
            assert(clock.canceled >= Number(expectedPacing) && clock.canceledIntervals === 1,
                "dropping the run future did not cancel its browser timers");
            check(state, "cancel suspended run");
            results.push({case: "drop-cancels-timers", ...result});
        }
        {
            const state = start(module); await first(module, state); await second(module, state, expectedPacing);
            state.socket.push(module.session_fixture_deactivate());
            await task();
            assert(!state.outcome && state.draws === 2 && clock.timeouts.size === 0,
                "activation wait did not flush pending drawing and cancel its presentation timer");
            if (expectedPacing) paint(state, 8, 3, 3, 3, 0x22CC44);
            // Supply no DemandActive response: the real activation sequence is waiting on Framed.
            // Abort drops the actual run future; this is not a normal Session::shutdown request.
            state.fixture.abort();
            const result = await finished(state);
            assert(state.outcome.value === "fixture aborted", "activation read wait was not canceled");
            assert(clock.canceled >= Number(expectedPacing) && clock.canceledIntervals === 1,
                "activation cancellation retained browser timers");
            check(state, "cancel activation read wait");
            results.push({case: "activation-wait-cancellation", ...result});
        }
        {
            const state = start(module); await first(module, state, true); await second(module, state, expectedPacing);
            for (const [width, height] of [[80, 40], [80, 40]]) {
                state.socket.push(module.session_fixture_deactivate());
                await task();
                assert(clock.timeouts.size === 0, "deactivation retained a presentation timer");
                const writes = state.socket.sent.length;
                state.socket.push(module.session_fixture_activate(width, height));
                await until(() => state.socket.sent.length >= writes + 5 || state.outcome, "reactivation writes");
                await task();
                assert(!state.outcome, `reactivation failed: ${JSON.stringify(state.outcome)}`);
                state.width = width; state.height = height;
                state.expected = new Uint8ClampedArray(width * height * 4);
                check(state, "new framebuffer generation");
                state.socket.push(module.session_fixture_bitmap(2, 2, 2, 2, 0xEECC11));
                await task();
                paint(state, 2, 2, 2, 2, 0xEECC11);
                check(state, "first sparse frame after reactivation");
            }
            state.socket.push(module.session_fixture_disconnect());
            results.push({case: "reactivation-size-and-generation", ...await finished(state)});
        }
        if (expectedPacing) {
            const state = start(module); await task();
            clock.step = 9; holdMessages = true;
            state.socket.push(module.session_fixture_bitmap(1, 1, 3, 3, 0xDD2211));
            await until(() => heldMessages.length > 0 || state.outcome, "pending browser yield");
            assert(!state.outcome, `yield fixture failed: ${JSON.stringify(state.outcome)}`);
            state.fixture.abort();
            const result = await finished(state);
            holdMessages = false; clock.step = 0;
            while (heldMessages.length) heldMessages.shift()();
            await task();
            assert(state.attempts.length === result.draw_attempts, "canceled yield caused a late upload");
            assert(clock.timeouts.size === 0 && clock.intervals.size === 0, "canceled yield recreated browser timers");
            results.push({case: "drop-cancels-message-channel", ...result});
        }
        return {pacing: expectedPacing, simd128: true, cases: results,
            boundary: "actual Session::run with an established connection and scripted browser transport/clock"};
    } finally {
        holdMessages = false;
        restoreEnvironment();
    }
};

window.checkDeployment = async ({packageRoot}) => {
    const module = await import(`${packageRoot}/ironrdp_web.js`);
    await module.default();
    assert(Object.keys(module).every(name => name !== "SessionFixture" && !name.startsWith("session_fixture_")),
        "test fixture leaked into deployment module");
    module.setup("error");
    const size = new module.DesktopSize(64, 48);
    const builder = new module.SessionBuilder();
    const canvas = document.createElement("canvas");
    // desktopSize consumes the exported DesktopSize wrapper and returns another builder.
    const sized = builder.desktopSize(size);
    const configured = sized.renderCanvas(canvas);
    configured.free(); sized.free(); builder.free();
    return {initialized: true, public_builder_exercised: true, test_exports_absent: true,
        boundary: "optimized normal wasm-pack bindings initialization and public builder methods",
        first_frame: {status: "not_run", reason: "no configured RDP endpoint"}};
};
