//! The editor: a peer on y-sweet that changes the logo's cross at a fixed
//! rate, numbering each edit in `data-bench`.

use futures_util::{SinkExt, StreamExt};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;
use xmesh::Op;
use xtxp_wire::{Input, Output, Peer};

/// The cross in the logo, by path from the root element.
const CROSS: [u32; 2] = [2, 0];

fn ops(k: u64) -> Vec<Op> {
    let set = |name: &str, value: String| Op::SetAttr {
        path: CROSS.to_vec(),
        name: name.into(),
        value,
    };
    let even = k.is_multiple_of(2);
    vec![
        set("data-bench", k.to_string()),
        set("stroke", if even { "#ff5436" } else { "#00ff00" }.into()),
        set("stroke-width", if even { "9.9" } else { "14" }.into()),
    ]
}

/// Edits `base + 1`, `base + 2`, … at `rate` per second for `duration`, and
/// returns when each was sent.
pub async fn run(
    ysweet: &str,
    doc: &str,
    base: u64,
    rate: f64,
    duration: Duration,
) -> Result<Vec<(u64, Instant)>, String> {
    let url = format!("ws://{ysweet}/d/{doc}/ws/{doc}");
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| format!("editor: {e}"))?;
    let (mut tx, mut rx) = ws.split();
    let mut peer = Peer::new(rand::random::<u32>().max(1) as u64).map_err(|e| e.to_string())?;
    let mut outbox = peer.handle(Input::Connected);
    let deadline = Instant::now() + Duration::from_secs(10);
    // Sync first, so the edits apply to the whole logo.
    loop {
        for o in outbox.drain(..) {
            if let Output::Send(f) = o {
                tx.send(Message::Binary(f.into()))
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        if peer.is_synced() {
            break;
        }
        if Instant::now() > deadline {
            return Err("editor: no sync within 10 s".into());
        }
        match tokio::time::timeout(Duration::from_secs(1), rx.next()).await {
            Ok(Some(Ok(Message::Binary(b)))) => outbox = peer.handle(Input::Frame(b.to_vec())),
            Ok(Some(Ok(_))) | Err(_) => {}
            Ok(Some(Err(e))) => return Err(e.to_string()),
            Ok(None) => return Err("editor: y-sweet closed".into()),
        }
    }
    let period = Duration::from_secs_f64(1.0 / rate);
    let mut tick = tokio::time::interval(period);
    let end = Instant::now() + duration;
    let mut sent = Vec::new();
    let mut k = base;
    while Instant::now() < end {
        tokio::select! {
            _ = tick.tick() => {
                k += 1;
                let out = peer.handle(Input::Edit(ops(k)));
                for o in out {
                    if let Output::Send(f) = o {
                        tx.send(Message::Binary(f.into())).await.map_err(|e| e.to_string())?;
                    }
                }
                sent.push((k, Instant::now()));
            }
            frame = rx.next() => match frame {
                Some(Ok(Message::Binary(b))) => {
                    for o in peer.handle(Input::Frame(b.to_vec())) {
                        if let Output::Send(f) = o {
                            tx.send(Message::Binary(f.into())).await.map_err(|e| e.to_string())?;
                        }
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.to_string()),
                None => return Err("editor: y-sweet closed".into()),
            },
        }
    }
    Ok(sent)
}
