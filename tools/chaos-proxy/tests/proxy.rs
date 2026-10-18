use chaos_proxy::{serve, Chaos};
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// A WebSocket server that echoes binary frames; reports when a connection ends.
async fn echo_upstream() -> (SocketAddr, mpsc::UnboundedReceiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (closed_tx, closed_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            let closed = closed_tx.clone();
            tokio::spawn(async move {
                let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
                while let Some(Ok(m)) = ws.next().await {
                    if let Message::Binary(b) = m {
                        if ws.send(Message::Binary(b)).await.is_err() {
                            break;
                        }
                    }
                }
                let _ = closed.send(());
            });
        }
    });
    (addr, closed_rx)
}

async fn proxy(upstream: SocketAddr, chaos: Chaos) -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    tokio::spawn(serve(addr, upstream, chaos));
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

fn calm(seed: u64) -> Chaos {
    Chaos {
        seed,
        delay_ms: 0,
        dup: 0.0,
        drop: 0.0,
    }
}

#[tokio::test]
async fn order_kept_under_delay() {
    let (up, _) = echo_upstream().await;
    let addr = proxy(
        up,
        Chaos {
            delay_ms: 30,
            ..calm(1)
        },
    )
    .await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/d/x/ws/x"))
        .await
        .unwrap();
    for i in 0u8..20 {
        ws.send(Message::Binary(vec![i].into())).await.unwrap();
    }
    let mut got = Vec::new();
    while got.len() < 20 {
        match tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
        {
            Some(Ok(Message::Binary(b))) => got.push(b[0]),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(got, (0u8..20).collect::<Vec<_>>());
}

#[tokio::test]
async fn duplicates_arrive() {
    let (up, _) = echo_upstream().await;
    let addr = proxy(
        up,
        Chaos {
            dup: 1.0,
            ..calm(2)
        },
    )
    .await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
        .await
        .unwrap();
    ws.send(Message::Binary(vec![7].into())).await.unwrap();
    let mut copies = 0;
    while let Ok(Some(Ok(Message::Binary(b)))) =
        tokio::time::timeout(Duration::from_millis(500), ws.next()).await
    {
        assert_eq!(&b[..], &[7]);
        copies += 1;
    }
    assert!(copies >= 2, "got {copies} copies");
}

#[tokio::test]
async fn drop_closes_both_sides() {
    let (up, mut upstream_closed) = echo_upstream().await;
    let addr = proxy(
        up,
        Chaos {
            drop: 1.0,
            ..calm(3)
        },
    )
    .await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
        .await
        .unwrap();
    ws.send(Message::Binary(vec![1].into())).await.unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Binary(_))) => continue,
                _ => return,
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the client side was closed");
    assert!(
        tokio::time::timeout(Duration::from_secs(3), upstream_closed.recv())
            .await
            .is_ok(),
        "the upstream side was closed"
    );
}

#[tokio::test]
async fn plain_http_passes_through() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let n = sock.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("POST /doc/new"));
        sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
            .await
            .unwrap();
    });
    let addr = proxy(up, calm(4)).await;
    let mut sock = TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"POST /doc/new HTTP/1.1\r\nhost: x\r\ncontent-length: 2\r\n\r\n{}")
        .await
        .unwrap();
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(3), sock.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.ends_with("\r\n\r\nok"), "{response}");
}
