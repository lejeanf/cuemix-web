//! Mock MOTU WebSocket server for integration tests (no hardware needed).
//!
//! PROVISIONAL, TEST-ONLY kind map (the real `prop_id` table is unknown):
//!
//! | prop_id range     | kind    |
//! |-------------------|---------|
//! | `0x0000..=0x0FFF` | Fixed   |
//! | `0x1000..=0x1FFF` | U32     |
//! | `0x2000..=0x2FFF` | U8      |
//! | `0x3000..`        | Str32   |
//!
//! Behaviour: every valid binary frame (write layout with a length field, or
//! the plain device layout) updates the shared store and is
//! re-broadcast (re-encoded) to ALL connected clients, including the sender,
//! simulating hardware feedback. Malformed binary frames are dropped and
//! counted; text frames are ignored. Neither ever closes a connection.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cuemix_web::motu::protocol::{Frame, Value, ValueKind};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

/// PROVISIONAL test-only kind map; see module docs.
pub fn kind_of(prop_id: u16) -> ValueKind {
    match prop_id {
        0x0000..=0x0FFF => ValueKind::Fixed,
        0x1000..=0x1FFF => ValueKind::U32,
        0x2000..=0x2FFF => ValueKind::U8,
        _ => ValueKind::Str32,
    }
}

type Store = Arc<Mutex<HashMap<(u16, u16), Value>>>;

#[derive(Debug)]
pub struct MockMotu {
    addr: std::net::SocketAddr,
    store: Store,
    malformed: Arc<AtomicUsize>,
    accept_task: JoinHandle<()>,
}

impl MockMotu {
    pub async fn start() -> std::io::Result<MockMotu> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let store: Store = Arc::new(Mutex::new(HashMap::new()));
        let malformed = Arc::new(AtomicUsize::new(0));
        let (tx, _) = broadcast::channel::<Vec<u8>>(1024);

        let (s, m) = (store.clone(), malformed.clone());
        let accept_task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let (s, m, tx) = (s.clone(), m.clone(), tx.clone());
                // Subscribe before the handshake so no echo can be missed.
                let mut rx = tx.subscribe();
                tokio::spawn(async move {
                    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                        return;
                    };
                    loop {
                        tokio::select! {
                            incoming = ws.next() => match incoming {
                                Some(Ok(Message::Binary(bytes))) => {
                                    // Client writes carry a length field; tests may also
                                    // push the plain device layout. Accept both.
                                    match Frame::decode_write(&bytes, kind_of)
                                        .or_else(|_| Frame::decode(&bytes, kind_of))
                                    {
                                        Ok(frame) => {
                                            s.lock().await.insert(
                                                (frame.prop_id, frame.index),
                                                frame.value.clone(),
                                            );
                                            if let Ok(enc) = frame.encode() {
                                                let _ = tx.send(enc);
                                            }
                                        }
                                        Err(_) => {
                                            m.fetch_add(1, Ordering::SeqCst);
                                        }
                                    }
                                }
                                Some(Ok(_)) => {} // text / ping / pong: ignored
                                Some(Err(_)) | None => break,
                            },
                            out = rx.recv() => match out {
                                Ok(bytes) => {
                                    if ws.send(Message::Binary(bytes.into())).await.is_err() {
                                        break;
                                    }
                                }
                                Err(broadcast::error::RecvError::Lagged(_)) => {}
                                Err(broadcast::error::RecvError::Closed) => break,
                            },
                        }
                    }
                });
            }
        });

        Ok(MockMotu {
            addr,
            store,
            malformed,
            accept_task,
        })
    }

    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.addr.port())
    }

    pub async fn get(&self, prop_id: u16, index: u16) -> Option<Value> {
        self.store.lock().await.get(&(prop_id, index)).cloned()
    }

    pub fn malformed_count(&self) -> usize {
        self.malformed.load(Ordering::SeqCst)
    }

    pub async fn shutdown(self) {
        self.accept_task.abort();
    }
}

impl Drop for MockMotu {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}
