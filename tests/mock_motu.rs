//! Integration tests against the in-process mock MOTU server.

mod support;

use std::time::Duration;

use cuemix_web::motu::fader::pos_to_gain;
use cuemix_web::motu::protocol::{from_fixed_8_24, to_fixed_8_24, Frame, Value};
use futures_util::{SinkExt, StreamExt};
use support::{kind_of, MockMotu};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

const T: Duration = Duration::from_secs(2);
type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn connect(mock: &MockMotu) -> Ws {
    let (ws, _) = timeout(T, connect_async(mock.url()))
        .await
        .expect("connect timeout")
        .expect("connect");
    ws
}

async fn send_frame(ws: &mut Ws, f: &Frame) {
    ws.send(Message::Binary(f.encode().expect("encode")))
        .await
        .expect("send");
}

async fn recv_frame(ws: &mut Ws) -> Frame {
    loop {
        let msg = timeout(T, ws.next())
            .await
            .expect("recv timeout")
            .expect("stream ended")
            .expect("ws error");
        if let Message::Binary(b) = msg {
            return Frame::decode(&b, kind_of).expect("decode");
        }
    }
}

/// Value as it looks after one trip through the 8.24 wire format.
fn quantized(v: f64) -> f64 {
    from_fixed_8_24(to_fixed_8_24(v).expect("in range"))
}

fn fixed(prop_id: u16, index: u16, v: f64) -> Frame {
    Frame {
        prop_id,
        index,
        value: Value::Fixed(v),
    }
}

#[tokio::test]
async fn echo_and_store() {
    let mock = MockMotu::start().await.expect("start");
    let mut ws = connect(&mock).await;
    let gain = pos_to_gain(0.75);
    send_frame(&mut ws, &fixed(0x0010, 3, gain)).await;
    let echo = recv_frame(&mut ws).await;
    assert_eq!((echo.prop_id, echo.index), (0x0010, 3));
    assert_eq!(echo.value, Value::Fixed(quantized(gain)));
    assert_eq!(
        mock.get(0x0010, 3).await,
        Some(Value::Fixed(quantized(gain)))
    );
    assert_eq!(mock.get(0x0010, 4).await, None);
    mock.shutdown().await;
}

#[tokio::test]
async fn broadcast_to_other_client() {
    let mock = MockMotu::start().await.expect("start");
    let mut a = connect(&mock).await;
    let mut b = connect(&mock).await;
    let f = Frame {
        prop_id: 0x1001,
        index: 9,
        value: Value::U32(0xDEAD_BEEF),
    };
    send_frame(&mut a, &f).await;
    assert_eq!(recv_frame(&mut b).await, f);
    assert_eq!(recv_frame(&mut a).await, f);
    mock.shutdown().await;
}

#[tokio::test]
async fn malformed_frames_counted_and_connection_survives() {
    let mock = MockMotu::start().await.expect("start");
    let mut ws = connect(&mock).await;
    let mut bad_utf8 = vec![0x30, 0x00, 0x00, 0x00, 0xFF, 0xFE];
    bad_utf8.resize(4 + 32, 0);
    let bad: Vec<Vec<u8>> = vec![
        vec![],
        vec![0, 1, 2],
        vec![0x00, 0x01, 0x00, 0x00, 0xAA], // Fixed needs 4 payload bytes
        bad_utf8,
    ];
    for b in &bad {
        ws.send(Message::Binary(b.clone())).await.expect("send");
    }
    ws.send(Message::Text("ignored".into()))
        .await
        .expect("send");
    // A valid frame afterwards proves the connection and server are alive
    // (frames are handled in order, so all bad ones were processed first).
    let f = Frame {
        prop_id: 0x2000,
        index: 1,
        value: Value::U8(1),
    };
    send_frame(&mut ws, &f).await;
    assert_eq!(recv_frame(&mut ws).await, f);
    assert_eq!(mock.malformed_count(), bad.len());
    assert_eq!(mock.get(0x2000, 1).await, Some(Value::U8(1)));
    mock.shutdown().await;
}

#[tokio::test]
async fn burst_of_100_frames_round_trips() {
    let mock = MockMotu::start().await.expect("start");
    let mut ws = connect(&mock).await;
    for i in 0..100u16 {
        send_frame(&mut ws, &fixed(0x0001, i, f64::from(i) / 10.0)).await;
    }
    for i in 0..100u16 {
        let f = recv_frame(&mut ws).await;
        assert_eq!(f.index, i);
        assert_eq!(f.value, Value::Fixed(quantized(f64::from(i) / 10.0)));
    }
    mock.shutdown().await;
}
