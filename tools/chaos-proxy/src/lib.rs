//! A WebSocket chaos proxy between muppets and y-sweet (part 2, P3). It delays
//! frames (keeping their order within a connection) and duplicates some, and
//! drops whole connections at random, sometimes losing the frame in flight.
//! It never reorders frames or drops one while keeping the connection,
//! because a TCP connection doesn't. Plain HTTP passes through unchanged.

use futures_util::{SinkExt, Stream, StreamExt};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

/// How much chaos, from one seed so a failing run can be repeated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chaos {
    pub seed: u64,
    /// Each frame is delayed by 0 to `delay_ms`, keeping order.
    pub delay_ms: u64,
    /// Probability that a frame is sent twice.
    pub dup: f64,
    /// Probability, per frame, that the connection is dropped.
    pub drop: f64,
}

/// Accepts connections on `listen` and proxies them to `upstream`.
pub async fn serve(listen: SocketAddr, upstream: SocketAddr, chaos: Chaos) -> io::Result<()> {
    let listener = TcpListener::bind(listen).await?;
    let mut n = 0u64;
    loop {
        let (sock, _) = listener.accept().await?;
        n += 1;
        tokio::spawn(async move {
            if let Err(e) = handle(n, sock, upstream, chaos).await {
                eprintln!("conn {n}: {e}");
            }
        });
    }
}

async fn handle(
    n: u64,
    mut sock: TcpStream,
    upstream: SocketAddr,
    chaos: Chaos,
) -> Result<(), String> {
    let mut head = Vec::new();
    let mut buf = [0u8; 4096];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 65_536 {
        let k = sock.read(&mut buf).await.map_err(|e| e.to_string())?;
        if k == 0 {
            return Ok(());
        }
        head.extend_from_slice(&buf[..k]);
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    if text.to_ascii_lowercase().contains("upgrade: websocket") {
        let path = text
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("/")
            .to_string();
        let client = tokio_tungstenite::accept_async(Prefixed {
            prefix: head,
            pos: 0,
            inner: sock,
        })
        .await
        .map_err(|e| e.to_string())?;
        let (server, _) = tokio_tungstenite::connect_async(format!("ws://{upstream}{path}"))
            .await
            .map_err(|e| e.to_string())?;
        relay(n, client, server, chaos).await;
        Ok(())
    } else {
        let mut up = TcpStream::connect(upstream)
            .await
            .map_err(|e| e.to_string())?;
        up.write_all(&head).await.map_err(|e| e.to_string())?;
        tokio::io::copy_bidirectional(&mut sock, &mut up)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

async fn relay<A, B>(n: u64, client: A, server: B, chaos: Chaos)
where
    A: Stream<Item = Result<Message, WsError>>
        + futures_util::Sink<Message, Error = WsError>
        + Send
        + 'static,
    B: Stream<Item = Result<Message, WsError>>
        + futures_util::Sink<Message, Error = WsError>
        + Send
        + 'static,
{
    let (client_tx, client_rx) = client.split();
    let (server_tx, server_rx) = server.split();
    let (dead_tx, dead_rx) = watch::channel(false);
    let up = tokio::spawn(pipe(
        n,
        "up",
        client_rx,
        server_tx,
        chaos,
        0,
        dead_tx.clone(),
        dead_rx.clone(),
    ));
    let down = tokio::spawn(pipe(
        n, "down", server_rx, client_tx, chaos, 1, dead_tx, dead_rx,
    ));
    let _ = tokio::join!(up, down);
}

enum Item {
    Frame(Message, Instant),
    /// Close after optionally forwarding the frame in flight.
    DropAfter(Option<(Message, Instant)>),
}

#[allow(clippy::too_many_arguments)]
async fn pipe<R, W>(
    n: u64,
    dir: &'static str,
    mut rx: R,
    mut tx: W,
    chaos: Chaos,
    salt: u64,
    dead_tx: watch::Sender<bool>,
    mut dead_rx: watch::Receiver<bool>,
) where
    R: Stream<Item = Result<Message, WsError>> + Unpin + Send + 'static,
    W: futures_util::Sink<Message, Error = WsError> + Unpin + Send + 'static,
{
    let (q_tx, mut q_rx) = mpsc::unbounded_channel::<Item>();
    let mut writer_dead = dead_rx.clone();
    let writer_dead_tx = dead_tx.clone();
    let writer = tokio::spawn(async move {
        loop {
            let item = tokio::select! {
                item = q_rx.recv() => item,
                _ = writer_dead.changed() => None,
            };
            match item {
                Some(Item::Frame(m, at)) => {
                    tokio::time::sleep_until(at).await;
                    if tx.send(m).await.is_err() {
                        break;
                    }
                }
                Some(Item::DropAfter(frame)) => {
                    if let Some((m, at)) = frame {
                        tokio::time::sleep_until(at).await;
                        let _ = tx.send(m).await;
                    }
                    break;
                }
                None => break,
            }
        }
        let _ = tx.close().await;
        let _ = writer_dead_tx.send(true);
    });

    let mut rng = StdRng::seed_from_u64(chaos.seed ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt);
    let (dup_p, drop_p) = (chaos.dup.clamp(0.0, 1.0), chaos.drop.clamp(0.0, 1.0));
    let mut last = Instant::now();
    let mut k = 0u64;
    loop {
        let next = tokio::select! {
            next = rx.next() => next,
            _ = dead_rx.changed() => break,
        };
        let Some(Ok(message)) = next else { break };
        if message.is_close() {
            break;
        }
        k += 1;
        let delay = if chaos.delay_ms == 0 {
            0
        } else {
            rng.gen_range(0..=chaos.delay_ms)
        };
        let at = (Instant::now() + Duration::from_millis(delay)).max(last);
        last = at;
        if rng.gen_bool(drop_p) {
            let forward = rng.gen_bool(0.5);
            eprintln!(
                "conn {n} {dir} drop after frame {k} ({})",
                if forward { "forwarded" } else { "lost" }
            );
            let _ = q_tx.send(Item::DropAfter(forward.then_some((message, at))));
            break;
        }
        if rng.gen_bool(dup_p) {
            eprintln!("conn {n} {dir} dup frame {k}");
            let _ = q_tx.send(Item::Frame(message.clone(), at));
        }
        let _ = q_tx.send(Item::Frame(message, at));
    }
    drop(q_tx);
    let _ = writer.await;
    let _ = dead_tx.send(true);
}

/// A stream that first replays bytes already read (the HTTP head).
struct Prefixed {
    prefix: Vec<u8>,
    pos: usize,
    inner: TcpStream,
}

impl AsyncRead for Prefixed {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.pos < self.prefix.len() {
            let n = (self.prefix.len() - self.pos).min(buf.remaining());
            let start = self.pos;
            buf.put_slice(&self.prefix[start..start + n]);
            self.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Prefixed {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
