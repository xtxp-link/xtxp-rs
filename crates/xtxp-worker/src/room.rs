//! One document's Durable Object: the host its muppets sync with, and the
//! page its browsers watch.
//!
//! Its WebSockets aren't hibernatable: the document lives in memory while
//! anyone is connected, and goes to storage at most every [`SAVE_MS`], so a
//! muppet editing at 30 Hz costs one row write per save, not per frame.

use futures_channel::mpsc::{unbounded, UnboundedSender};
use futures_util::StreamExt;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;
use wasm_bindgen_futures::spawn_local;
use worker::{
    durable_object, Date, Delay, Env, Headers, Request, Response, Result, SqlStorageValue, State,
    WebSocket, WebSocketPair, WebsocketEvent,
};
use xtxp_serve::render::{
    render_page, script_tag, template, Grain, Page, PageInfo, Snapshot, PICKER,
};
use xtxp_wire::host::{ConnId, Host};

/// How long a change may wait in memory before it's saved.
const SAVE_MS: u64 = 2_000;

/// How often an open stream gets a heartbeat comment.
const HEARTBEAT: Duration = Duration::from_secs(15);

/// A browser receiving patch sets: an open response or a WebSocket.
enum Viewer {
    Stream(UnboundedSender<Vec<u8>>),
    Socket(WebSocket),
}

impl Viewer {
    /// Sends one chunk; false once the viewer is gone.
    fn send(&self, html: &str) -> bool {
        match self {
            Viewer::Stream(tx) => tx.unbounded_send(html.as_bytes().to_vec()).is_ok(),
            Viewer::Socket(ws) => ws.send_with_str(html).is_ok(),
        }
    }
}

struct Doc {
    host: Host,
    page: Page,
    muppets: BTreeMap<ConnId, WebSocket>,
    viewers: BTreeMap<u64, (Viewer, Grain)>,
    next_viewer: u64,
    unsaved: bool,
    /// A save is scheduled. Setting the alarm again would push it back, so a
    /// muppet editing nonstop would never be saved.
    alarm: bool,
    heartbeat: bool,
}

impl Doc {
    fn new(host: Host) -> Doc {
        let mut doc = Doc {
            host,
            page: Page::new(),
            muppets: BTreeMap::new(),
            viewers: BTreeMap::new(),
            next_viewer: 1,
            unsaved: false,
            alarm: false,
            heartbeat: false,
        };
        doc.render();
        doc
    }

    /// Re-renders the page and sends its patch set to every viewer. The page
    /// says "live": this server is the host, so it's never cut off from it.
    fn render(&mut self) {
        let doc = self.host.doc();
        let patch = self.page.show(
            &doc.tree_with_ids(),
            &doc.snapshot_hash(),
            true,
            Date::now().as_millis(),
        );
        if let Some(patch) = patch {
            self.viewers
                .retain(|_, (viewer, grain)| viewer.send(patch.html(*grain)));
        }
    }

    fn add_viewer(&mut self, viewer: Viewer, grain: Grain) -> u64 {
        let id = self.next_viewer;
        self.next_viewer += 1;
        self.viewers.insert(id, (viewer, grain));
        id
    }

    fn streams(&self) -> usize {
        self.viewers
            .values()
            .filter(|(v, _)| matches!(v, Viewer::Stream(_)))
            .count()
    }
}

#[durable_object]
pub struct Room {
    state: State,
    doc: Rc<RefCell<Option<Doc>>>,
}

type Shared = Rc<RefCell<Option<Doc>>>;

impl worker::DurableObject for Room {
    fn new(state: State, _env: Env) -> Self {
        Room {
            state,
            doc: Rc::new(RefCell::new(None)),
        }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        self.load()?;
        let path = req.path();
        let Some((name, rest)) = crate::doc_path(&path) else {
            return Response::error("not found\n", 404);
        };
        let name = name.to_string();
        if rest.strip_prefix("ws/") == Some(name.as_str()) {
            return self.muppet(&req);
        }
        let url = req.url()?;
        let query = |key: &str| {
            url.query_pairs()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.into_owned())
        };
        match rest {
            "" => match query("live").as_deref() {
                None => self.refresh(&name, &req, None, true),
                Some("refresh") => self.refresh(&name, &req, query("every"), false),
                Some("stream") => match grain(query("grain"), Grain::Region) {
                    Ok(g) => self.stream(&name, g, query("polyfill").as_deref() == Some("1")),
                    Err(e) => Response::error(e, 400),
                },
                Some("ws") => match grain(query("grain"), Grain::Node) {
                    Ok(g) => self.ws_page(&name, g),
                    Err(e) => Response::error(e, 400),
                },
                Some(_) => Response::error("live must be refresh, stream or ws\n", 400),
            },
            "live" => match grain(query("grain"), Grain::Node) {
                Ok(g) => self.live_socket(g),
                Err(e) => Response::error(e, 400),
            },
            _ => Response::error("not found\n", 404),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        if let Some(doc) = self.doc.borrow_mut().as_mut() {
            doc.alarm = false;
        }
        self.save()?;
        Response::ok("saved")
    }
}

/// The `grain` parameter, or `default`.
fn grain(value: Option<String>, default: Grain) -> std::result::Result<Grain, &'static str> {
    match value.as_deref() {
        None => Ok(default),
        Some("node") => Ok(Grain::Node),
        Some("region") => Ok(Grain::Region),
        Some(_) => Err("grain must be node or region\n"),
    }
}

fn html(body: String) -> Result<Response> {
    let headers = Headers::new();
    headers.set("content-type", "text/html; charset=utf-8")?;
    headers.set("cache-control", "no-cache")?;
    Ok(Response::ok(body)?.with_headers(headers))
}

fn page_html(name: &str, now: &Snapshot, head: &str, body_end: &str) -> String {
    render_page(
        &now.region_html,
        &PageInfo {
            doc: name,
            snapshot: &now.snapshot,
            connected: now.connected,
            seq: now.seq,
            sent_ms: now.sent_ms,
            head,
            body_end,
        },
    )
}

impl Room {
    /// Reads the document from storage the first time it's needed.
    fn load(&self) -> Result<()> {
        if self.doc.borrow().is_some() {
            return Ok(());
        }
        let sql = self.state.storage().sql();
        sql.exec(
            "CREATE TABLE IF NOT EXISTS doc (id INTEGER PRIMARY KEY CHECK (id = 0), state BLOB NOT NULL)",
            None,
        )?;
        let mut saved = None;
        for row in sql.exec("SELECT state FROM doc WHERE id = 0", None)?.raw() {
            if let Some(SqlStorageValue::Blob(bytes)) = row?.into_iter().next() {
                saved = Some(bytes);
            }
        }
        let host = match saved {
            Some(bytes) => Host::load(&bytes).map_err(worker::Error::RustError)?,
            None => Host::new(),
        };
        *self.doc.borrow_mut() = Some(Doc::new(host));
        Ok(())
    }

    /// Writes the document if it changed since the last write.
    fn save(&self) -> Result<()> {
        let state = {
            let mut doc = self.doc.borrow_mut();
            let Some(doc) = doc.as_mut().filter(|d| d.unsaved) else {
                return Ok(());
            };
            doc.unsaved = false;
            doc.host.state()
        };
        self.state.storage().sql().exec(
            "INSERT OR REPLACE INTO doc (id, state) VALUES (0, ?)",
            vec![SqlStorageValue::Blob(state)],
        )?;
        Ok(())
    }

    /// A muppet's connection to the host.
    fn muppet(&self, req: &Request) -> Result<Response> {
        if req.headers().get("upgrade")?.as_deref() != Some("websocket") {
            return Response::error("expected a WebSocket\n", 426);
        }
        let WebSocketPair { client, server } = WebSocketPair::new()?;
        // Binary messages arrive as Blobs otherwise, which can only be read
        // asynchronously, out of order.
        js_sys::Reflect::set(server.as_ref(), &"binaryType".into(), &"arraybuffer".into())?;
        server.accept()?;
        let conn = {
            let mut doc = self.doc.borrow_mut();
            let doc = doc.as_mut().expect("loaded");
            let (conn, frames) = doc.host.connect();
            for frame in frames {
                server.send_with_bytes(frame)?;
            }
            doc.muppets.insert(conn, server.clone());
            conn
        };
        let (shared, storage) = (self.doc.clone(), self.state.storage());
        spawn_local(async move {
            if let Ok(mut events) = server.events() {
                while let Some(Ok(WebsocketEvent::Message(msg))) = events.next().await {
                    if let Some(frame) = msg.bytes() {
                        if receive(&shared, conn, &frame) {
                            let at = Duration::from_millis(SAVE_MS);
                            if storage.set_alarm(at).await.is_err() {
                                if let Some(doc) = shared.borrow_mut().as_mut() {
                                    doc.alarm = false;
                                }
                            }
                        }
                    }
                }
            }
            if let Some(doc) = shared.borrow_mut().as_mut() {
                doc.host.disconnect(conn);
                doc.muppets.remove(&conn);
            }
        });
        Response::from_websocket(client)
    }

    /// The baseline: a full page that refreshes itself, and `304` while
    /// nothing changed. With `auto`, the page with no `live` parameter: the
    /// refresh only inside `<noscript>`, and a script that picks a mode.
    fn refresh(
        &self,
        name: &str,
        req: &Request,
        every: Option<String>,
        auto: bool,
    ) -> Result<Response> {
        let now = self.snapshot();
        let etag = format!("\"{}-live\"", now.snapshot);
        if !now.snapshot.is_empty()
            && req.headers().get("if-none-match")?.as_deref() == Some(etag.as_str())
        {
            return Ok(Response::empty()?.with_status(304));
        }
        let every = every.and_then(|e| e.parse::<u32>().ok()).unwrap_or(1);
        let meta = format!("<meta http-equiv=\"refresh\" content=\"{every}\">");
        let (head, body_end) = if auto {
            (format!("<noscript>{meta}</noscript>"), PICKER)
        } else {
            (meta, "")
        };
        let resp = html(page_html(name, &now, &head, body_end))?;
        if !now.snapshot.is_empty() {
            resp.headers().set("etag", &etag)?;
        }
        Ok(resp)
    }

    /// The patch stream: the page, then the response stays open and each
    /// change arrives as `<template for>` children of `<body>`.
    fn stream(&self, name: &str, grain: Grain, polyfill: bool) -> Result<Response> {
        let (tx, rx) = unbounded::<Vec<u8>>();
        let now = {
            let mut doc = self.doc.borrow_mut();
            let doc = doc.as_mut().expect("loaded");
            doc.add_viewer(Viewer::Stream(tx.clone()), grain);
            doc.page.snapshot()
        };
        let script = if polyfill {
            script_tag("stream", grain)
        } else {
            String::new()
        };
        let page = page_html(name, &now, "", &script);
        let first = page
            .trim_end()
            .strip_suffix("</body></html>")
            .unwrap_or(&page)
            .to_string();
        let _ = tx.unbounded_send(first.into_bytes());
        self.start_heartbeat();
        let headers = Headers::new();
        headers.set("content-type", "text/html; charset=utf-8")?;
        // Compressing would hold patches back until a block fills.
        headers.set("cache-control", "no-cache, no-transform")?;
        headers.set("content-encoding", "identity")?;
        Ok(Response::from_stream(rx.map(Ok::<_, worker::Error>))?.with_headers(headers))
    }

    /// While any stream is open, a comment every [`HEARTBEAT`] keeps it from
    /// timing out and finds the ones that closed.
    fn start_heartbeat(&self) {
        {
            let mut doc = self.doc.borrow_mut();
            let doc = doc.as_mut().expect("loaded");
            if doc.heartbeat {
                return;
            }
            doc.heartbeat = true;
        }
        let shared = self.doc.clone();
        spawn_local(async move {
            loop {
                Delay::from(HEARTBEAT).await;
                let mut doc = shared.borrow_mut();
                let Some(doc) = doc.as_mut() else { return };
                doc.viewers.retain(|_, (viewer, _)| match viewer {
                    Viewer::Stream(_) => viewer.send("<!--hb-->"),
                    Viewer::Socket(_) => true,
                });
                if doc.streams() == 0 {
                    doc.heartbeat = false;
                    return;
                }
            }
        });
    }

    /// The WebSocket mode's page: complete, with the polyfill.
    fn ws_page(&self, name: &str, grain: Grain) -> Result<Response> {
        let now = self.snapshot();
        html(page_html(name, &now, "", &script_tag("ws", grain)))
    }

    /// One viewer's WebSocket: the whole document first, then each patch set.
    fn live_socket(&self, grain: Grain) -> Result<Response> {
        let WebSocketPair { client, server } = WebSocketPair::new()?;
        server.accept()?;
        let id = {
            let mut doc = self.doc.borrow_mut();
            let doc = doc.as_mut().expect("loaded");
            let now = doc.page.snapshot();
            server.send_with_str(template("doc", now.seq, now.sent_ms, &now.region_html))?;
            doc.add_viewer(Viewer::Socket(server.clone()), grain)
        };
        let shared = self.doc.clone();
        spawn_local(async move {
            if let Ok(mut events) = server.events() {
                while let Some(Ok(WebsocketEvent::Message(_))) = events.next().await {}
            }
            if let Some(doc) = shared.borrow_mut().as_mut() {
                doc.viewers.remove(&id);
            }
        });
        Response::from_websocket(client)
    }

    fn snapshot(&self) -> Snapshot {
        self.doc.borrow().as_ref().expect("loaded").page.snapshot()
    }
}

/// Handles one frame from a muppet: relays it, and re-renders if the
/// document changed. True if a save needs scheduling.
fn receive(shared: &Shared, conn: ConnId, frame: &[u8]) -> bool {
    let mut doc = shared.borrow_mut();
    let Some(doc) = doc.as_mut() else {
        return false;
    };
    for (to, out) in doc.host.receive(conn, frame) {
        if let Some(ws) = doc.muppets.get(&to) {
            let _ = ws.send_with_bytes(out);
        }
    }
    if !doc.host.take_changed() {
        return false;
    }
    doc.render();
    doc.unsaved = true;
    !std::mem::replace(&mut doc.alarm, true)
}
