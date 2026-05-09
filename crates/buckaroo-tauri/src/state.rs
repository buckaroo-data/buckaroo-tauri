//! Shared mutable state owned by the plugin: the discovered sidecar port and
//! the channel used to push messages onto the internal WebSocket.

use std::sync::Mutex as StdMutex;
use tokio::sync::{mpsc, Mutex as AsyncMutex};

pub(crate) struct SidecarState {
    /// OS-assigned port the sidecar bound to (parsed from `BUCKAROO_PORT=<n>`).
    pub port: StdMutex<Option<u16>>,
    /// Sender half of the WS write channel. None until the internal WS is open.
    pub ws_tx: AsyncMutex<Option<mpsc::UnboundedSender<String>>>,
    /// Active session ID used for the internal WS connection. None until
    /// the first /load mints one (or the caller pre-supplies one).
    pub session_id: StdMutex<Option<String>>,
}

impl SidecarState {
    pub fn new() -> Self {
        Self {
            port: StdMutex::new(None),
            ws_tx: AsyncMutex::new(None),
            session_id: StdMutex::new(None),
        }
    }
}
