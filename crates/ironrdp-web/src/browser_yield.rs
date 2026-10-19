use anyhow::Context as _;
use futures_channel::mpsc;
use futures_util::StreamExt as _;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use web_sys::{MessageChannel, MessagePort};

/// Yields to a browser task without chaining timers and their minimum-delay clamping.
pub(crate) struct BrowserYield {
    posting_port: MessagePort,
    receiving_port: MessagePort,
    notifications: mpsc::Receiver<()>,
    // Keep the callback alive until Drop disconnects the ports and removes the event handler.
    _on_message: Closure<dyn FnMut()>,
}

impl BrowserYield {
    pub(crate) fn new() -> anyhow::Result<Self> {
        let channel =
            MessageChannel::new().map_err(|error| anyhow::anyhow!("create browser yield channel failed: {error:?}"))?;
        let posting_port = channel.port2();
        let receiving_port = channel.port1();

        // One sender has one reserved slot. Extra callbacks after cancellation can be coalesced.
        let (mut notify, notifications) = mpsc::channel(0);
        let on_message = Closure::<dyn FnMut()>::new(move || {
            let _ = notify.try_send(());
        });
        receiving_port.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

        Ok(Self {
            posting_port,
            receiving_port,
            notifications,
            _on_message: on_message,
        })
    }

    pub(crate) async fn yield_now(&mut self) -> anyhow::Result<()> {
        // A canceled call may have left a notification. Remove it before posting so this call
        // always waits for a browser task, even when an earlier message is still in flight.
        let _ = self.notifications.try_recv();
        self.posting_port
            .post_message(&JsValue::NULL)
            .map_err(|error| anyhow::anyhow!("post browser yield message failed: {error:?}"))?;
        self.notifications.next().await.context("browser yield channel closed")
    }
}

impl Drop for BrowserYield {
    fn drop(&mut self) {
        self.receiving_port.set_onmessage(None);
        self.receiving_port.close();
        self.posting_port.close();
    }
}
