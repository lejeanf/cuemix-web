//! Hardware side of the bridge (MOTU UltraLite-mk5 over USB CDC-Ethernet).

pub mod client;
pub mod discovery;
pub mod fader;
pub mod protocol;

/// Default TCP port of the MOTU binary WebSocket server.
pub const MOTU_WS_PORT: u16 = 1280;
