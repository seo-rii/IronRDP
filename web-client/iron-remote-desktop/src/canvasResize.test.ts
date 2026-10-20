import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import IronRemoteDesktop from './iron-remote-desktop.svelte';
import { ScreenScale } from './enums/ScreenScale';
import type { RemoteDesktopModule } from './interfaces/RemoteDesktopModule';
import type { UserInteraction } from './interfaces/UserInteraction';

// The backend changes the canvas dimensions before invoking the callback registered by the component.
// Keep the real component and service in this test so the callback must reach their scaling logic.
function createResizingBackend() {
    let canvas: HTMLCanvasElement;
    let onCanvasResized: () => void;
    const resize = vi.fn();
    const registerCanvasResized = vi.fn((callback: () => void) => {
        onCanvasResized = callback;
    });
    const SessionBuilder = class {
        constructor() {
            return new Proxy(this, {
                get: (_target, name: string) => {
                    switch (name) {
                        case 'renderCanvas':
                            return (element: HTMLCanvasElement) => {
                                canvas = element;
                            };
                        case 'canvasResizedCallback':
                            return registerCanvasResized;
                        case 'connect':
                            return async () => ({
                                desktopSize: () => ({ width: 64, height: 48 }),
                                resize,
                            });
                        default:
                            return () => undefined;
                    }
                },
            });
        }
    };

    return {
        module: {
            SessionBuilder,
            DesktopSize: class {},
            InputTransaction: class {},
            ClipboardData: class {},
            DeviceEvent: {},
        } as unknown as RemoteDesktopModule,
        resize,
        registerCanvasResized,
        negotiatedSize(width: number, height: number) {
            canvas.width = width;
            canvas.height = height;
            onCanvasResized();
        },
    };
}

describe('canvas resize callback', () => {
    let component: ReturnType<typeof mount> | null = null;

    afterEach(async () => {
        if (component) {
            await unmount(component);
            component = null;
        }
        vi.clearAllTimers();
        vi.useRealTimers();
        vi.unstubAllGlobals();
        document.body.innerHTML = '';
    });

    it.each([
        { name: 'fit', scale: ScreenScale.Fit, wide: ['800px', '400px'], tall: ['300px', '600px'] },
        { name: 'real', scale: ScreenScale.Real, wide: ['80px', '40px'], tall: ['40px', '80px'] },
    ])('uses negotiated dimensions and preserves $name scaling', async ({ scale, wide, tall }) => {
        vi.stubGlobal('isSecureContext', false);
        vi.stubGlobal('innerWidth', 800);
        vi.stubGlobal('innerHeight', 600);

        const backend = createResizingBackend();
        const target = document.createElement('div');
        document.body.appendChild(target);
        const ready = vi.fn();
        target.addEventListener('ready', ready);
        component = mount(IronRemoteDesktop, {
            target,
            props: { scale: 'fit', verbose: 'false', flexcenter: 'true', module: backend.module },
        });
        await vi.waitFor(() => expect(ready).toHaveBeenCalledTimes(1));

        const ui = (ready.mock.calls[0][0] as CustomEvent<{ irgUserInteraction: UserInteraction }>).detail
            .irgUserInteraction;
        const config = ui
            .configBuilder()
            .withDestination('host:3389')
            .withProxyAddress('ws://proxy')
            .withAuthToken('token')
            .build();
        await ui.connect(config);
        expect(backend.registerCanvasResized).toHaveBeenCalledTimes(1);

        vi.useFakeTimers();
        ui.setVisibility(true);
        await vi.advanceTimersByTimeAsync(150);
        flushSync();
        const viewer = target.querySelector<HTMLDivElement>('.screen-viewer')!;
        expect([viewer.style.width, viewer.style.height]).toEqual(['800px', '600px']);

        // Change the scale through the public API; a backend resize must retain this selection.
        ui.setScale(scale);
        flushSync();

        // The server can negotiate a different size from the client's request.
        ui.resize(100, 100);
        expect(backend.resize).toHaveBeenCalledWith(100, 100, undefined);
        backend.negotiatedSize(80, 40);
        flushSync();
        expect([viewer.style.width, viewer.style.height]).toEqual(wide);

        // A server-driven resize has no preceding request or browser resize event.
        backend.negotiatedSize(40, 80);
        flushSync();
        expect([viewer.style.width, viewer.style.height]).toEqual(tall);
        expect([viewer.style.minWidth, viewer.style.maxWidth]).toEqual([tall[0], tall[0]]);
        expect([viewer.style.minHeight, viewer.style.maxHeight]).toEqual([tall[1], tall[1]]);
    });
});
