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
        sockets.push(this);
        queueMicrotask(() => this.dispatchEvent(new Event("open")));
    }
    // gloo-net sends Blob objects. Retain the actual write without asynchronous conversion.
    send(data) { this.sent.push(data); }
    push(bytes) {
        this.receivedFrames++;
        this.dispatchEvent(new MessageEvent("message", {data: Uint8Array.from(bytes).buffer}));
    }
    close() {
        if (this.readyState === 3) return;
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
            if (state.failDraw) throw new Error(`session-fixture-draw-failure-${state.attempts.length}`);
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
function start(module, width = 64, height = 48) {
    clock.reset();
    const canvas = document.createElement("canvas");
    document.querySelector("main").replaceChildren(canvas);
    const fixture = new module.SessionFixture(canvas, "ws://session-fixture.invalid/established", width, height);
    const state = {canvas, fixture, socket: sockets.at(-1), width, height, attempts: [], draws: 0, failDraw: false,
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
    return {outcome: state.outcome, draw_attempts: draws, successful_uploads: state.draws,
        canceled_timers: clock.canceled, fired_timers: clock.fired,
        canceled_cleanup_intervals: clock.canceledIntervals, received_frames: state.socket.receivedFrames};
}

window.runSessionChecks = async ({packageRoot, expectedPacing}) => {
    const module = await import(`${packageRoot}/ironrdp_web.js`);
    await module.default();
    assert(module.session_fixture_simd_enabled() === true, "fixture did not compile SIMD");
    assert(module.session_fixture_pacing_enabled() === expectedPacing, "wrong pacing feature");
    const results = [];
    installEnvironment();
    try {
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
        {
            const state = start(module, 64, 32); await task();
            state.socket.push(module.session_fixture_tiles());
            await task(); clock.advance(20); await task();
            for (let index = 0; index < 65; index++) paint(state, (index % 16) * 4, Math.floor(index / 16) * 4, 1, 1, 0xCC5533);
            check(state, "65 disjoint updates in one PDU");
            assert(state.draws === 65, "overflow changed disjoint update coverage or repeated uploads");
            state.socket.push(module.session_fixture_disconnect());
            results.push({case: "region-capacity-overflow", ...await finished(state)});
        }
        {
            const state = start(module, 64, 32); await task();
            state.failDraw = true;
            state.socket.push(module.session_fixture_tiles());
            const result = await finished(state, "session-fixture-draw-failure-1");
            assert(state.attempts.length === 65 && state.draws === 0,
                "overflow failure lost the unqueued region or retried an already attempted region");
            assert(!state.outcome.error.includes("session-fixture-draw-failure-65"), "overflow cleanup replaced the original error");
            check(state, "overflow failure");
            results.push({case: "overflow-error-preserves-region", ...result});
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
