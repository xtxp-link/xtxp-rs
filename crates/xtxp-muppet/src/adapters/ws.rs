//! The WebSocket transport, the system clock and random client IDs, and the
//! event loop that drives a [`Muppet`] with them.

use crate::adapters::ysweet::ws_url;
use crate::core::{Command, Mode, Muppet};
use crate::ports::{Clock, IdSource, Outcome, Screen, Transport, View};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const TICK: Duration = Duration::from_millis(50);

pub struct SystemClock(Instant);

impl SystemClock {
    pub fn new() -> SystemClock {
        SystemClock(Instant::now())
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
}

pub struct RandomIds;

impl IdSource for RandomIds {
    fn next_client_id(&mut self) -> u64 {
        rand::thread_rng().gen_range(1..=u32::MAX as u64)
    }
}

#[derive(Default)]
struct Requests {
    connect: bool,
    close: bool,
    frames: Vec<Vec<u8>>,
}

/// Queues the core's requests for the event loop.
struct WsTransport(Rc<RefCell<Requests>>);

impl Transport for WsTransport {
    fn connect(&mut self) {
        self.0.borrow_mut().connect = true;
    }
    fn send(&mut self, frame: Vec<u8>) {
        self.0.borrow_mut().frames.push(frame);
    }
    fn close(&mut self) {
        self.0.borrow_mut().close = true;
    }
}

/// Passes everything to the caller's view and keeps the outcome.
struct Capture<V> {
    inner: V,
    outcome: Rc<RefCell<Option<Outcome>>>,
}

impl<V: View> View for Capture<V> {
    fn show(&mut self, screen: &Screen) {
        self.inner.show(screen)
    }
    fn status(&mut self, line: &str) {
        self.inner.status(line)
    }
    fn finished(&mut self, outcome: &Outcome) {
        self.inner.finished(outcome);
        *self.outcome.borrow_mut() = Some(outcome.clone());
    }
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
type Connecting = Pin<Box<dyn Future<Output = Result<Ws, String>>>>;

/// Runs a muppet against `server` until its mode finishes or, for
/// interactive use, until the `commands` channel closes.
pub fn run<V: View>(
    mode: Mode,
    server: &str,
    doc: &str,
    view: V,
    commands: Option<UnboundedReceiver<Command>>,
) -> Outcome {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(event_loop(mode, ws_url(server, doc), view, commands))
}

async fn event_loop<V: View>(
    mode: Mode,
    url: String,
    view: V,
    mut commands: Option<UnboundedReceiver<Command>>,
) -> Outcome {
    let requests = Rc::new(RefCell::new(Requests::default()));
    let outcome = Rc::new(RefCell::new(None));
    let view = Capture {
        inner: view,
        outcome: outcome.clone(),
    };
    let mut muppet = Muppet::new(
        mode,
        WsTransport(requests.clone()),
        SystemClock::new(),
        RandomIds,
        view,
    );
    let mut ws: Option<Ws> = None;
    let mut connecting: Option<Connecting> = None;
    let mut tick = tokio::time::interval(TICK);

    loop {
        let (connect, close, frames) = {
            let mut r = requests.borrow_mut();
            (
                std::mem::take(&mut r.connect),
                std::mem::take(&mut r.close),
                std::mem::take(&mut r.frames),
            )
        };
        if close {
            connecting = None;
            if let Some(mut socket) = ws.take() {
                let _ = socket.close(None).await;
            }
            if outcome.borrow().is_none() {
                muppet.on_disconnected();
            }
        }
        if let Some(found) = outcome.borrow().clone() {
            return found;
        }
        for frame in frames {
            if let Some(socket) = ws.as_mut() {
                if socket.send(Message::Binary(frame.into())).await.is_err() {
                    ws = None;
                    muppet.on_disconnected();
                    break;
                }
            }
        }
        if connect && ws.is_none() && connecting.is_none() {
            let url = url.clone();
            connecting = Some(Box::pin(async move {
                match tokio::time::timeout(CONNECT_TIMEOUT, connect_async(url)).await {
                    Ok(Ok((socket, _))) => Ok(socket),
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(_) => Err("connect timed out".into()),
                }
            }));
        }

        tokio::select! {
            result = async { connecting.as_mut().expect("guarded").await }, if connecting.is_some() => {
                connecting = None;
                match result {
                    Ok(socket) => {
                        ws = Some(socket);
                        muppet.on_connected();
                    }
                    Err(_) => muppet.on_disconnected(),
                }
            }
            message = async { ws.as_mut().expect("guarded").next().await }, if ws.is_some() => match message {
                Some(Ok(Message::Binary(bytes))) => muppet.on_frame(bytes.to_vec()),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                    ws = None;
                    muppet.on_disconnected();
                }
                Some(Ok(_)) => {}
            },
            _ = tick.tick() => muppet.on_tick(),
            command = async { commands.as_mut().expect("guarded").recv().await }, if commands.is_some() => match command {
                Some(command) => muppet.command(command),
                None => {
                    if let Some(mut socket) = ws.take() {
                        let _ = socket.close(None).await;
                    }
                    let doc = muppet.doc();
                    return Outcome {
                        ok: true,
                        doc_hash: doc.hash(),
                        snapshot_hash: doc.snapshot_hash(),
                        updates_before_synced: 0,
                        canonical: doc.canonical(),
                        message: None,
                    };
                }
            },
        }
    }
}
