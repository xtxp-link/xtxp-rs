//! The server in process: screens go straight into the hub, no y-sweet.

use std::net::SocketAddr;
use std::time::Duration;
use xmesh::{Op, Xmesh};
use xtxp_muppet::adapters::serve::{router_with, Hub, Metrics, Options};
use xtxp_muppet::ports::Screen;

const LOGO: &str = include_str!("../../xmesh/tests/fixtures/xtxp-logo.svg");

struct Server {
    hub: Hub,
    metrics: Metrics,
    base: String,
}

async fn start() -> Server {
    let hub = Hub::new("logo");
    let metrics = Metrics::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let app = router_with(
        hub.clone(),
        metrics.clone(),
        Options {
            heartbeat: Duration::from_millis(50),
        },
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server {
        hub,
        metrics,
        base: format!("http://{addr}"),
    }
}

fn logo() -> Xmesh {
    let d = Xmesh::new(7).unwrap();
    d.apply(&Xmesh::seed(LOGO).unwrap()).unwrap();
    d
}

fn screen(d: &Xmesh) -> Screen {
    Screen {
        tree: d.tree_with_ids(),
        snapshot_hash: d.snapshot_hash(),
        doc_hash: d.hash(),
        connected: true,
        changed: vec![],
    }
}

fn widen(d: &Xmesh, width: &str) {
    d.edit(&[Op::SetAttr {
        path: vec![2, 0],
        name: "stroke-width".into(),
        value: width.into(),
    }])
    .unwrap();
}

async fn get(url: &str, etag: Option<&str>) -> (u16, Option<String>, String) {
    let mut req = reqwest::Client::new().get(url);
    if let Some(e) = etag {
        req = req.header("if-none-match", e);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    let tag = resp
        .headers()
        .get("etag")
        .map(|v| v.to_str().unwrap().to_string());
    (status, tag, resp.text().await.unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn page_before_first_sync_says_connecting() {
    let s = start().await;
    let (status, _, body) = get(&format!("{}/?live=refresh", s.base), None).await;
    assert_eq!(status, 200);
    assert!(body.contains("○ connecting"), "{body}");
    assert!(body.contains("<?start name=\"doc\"><?end>"), "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_page_has_meta_and_etag() {
    let s = start().await;
    let d = logo();
    s.hub.show(&screen(&d));
    let (status, tag, body) = get(&format!("{}/?live=refresh", s.base), None).await;
    assert_eq!(status, 200);
    assert!(
        body.contains("<meta http-equiv=\"refresh\" content=\"1\">"),
        "{body}"
    );
    assert_eq!(tag, Some(format!("\"{}\"", d.snapshot_hash())));
    assert!(body.contains("stroke-width=\"9.9\""));
    assert!(body.contains("● live"));
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_every_sets_the_interval() {
    let s = start().await;
    s.hub.show(&screen(&logo()));
    let (_, _, body) = get(&format!("{}/?live=refresh&every=5", s.base), None).await;
    assert!(
        body.contains("<meta http-equiv=\"refresh\" content=\"5\">"),
        "{body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unchanged_poll_is_304_and_changed_poll_is_200() {
    let s = start().await;
    let d = logo();
    s.hub.show(&screen(&d));
    let url = format!("{}/?live=refresh", s.base);
    let (_, tag, _) = get(&url, None).await;
    let tag = tag.unwrap();
    let (status, _, body) = get(&url, Some(&tag)).await;
    assert_eq!(status, 304);
    assert!(body.is_empty());
    widen(&d, "14");
    s.hub.show(&screen(&d));
    let (status, _, body) = get(&url, Some(&tag)).await;
    assert_eq!(status, 200);
    assert!(body.contains("stroke-width=\"14\""));
    let m = s.metrics.snapshot();
    assert_eq!((m.refresh_ok, m.refresh_not_modified), (2, 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_has_cross_document_transitions() {
    let s = start().await;
    let (_, _, body) = get(&format!("{}/?live=refresh", s.base), None).await;
    assert!(body.contains("@view-transition { navigation: auto; }"));
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_live_value_is_400() {
    let s = start().await;
    let (status, _, body) = get(&format!("{}/?live=carrier-pigeon", s.base), None).await;
    assert_eq!(status, 400);
    assert!(body.contains("refresh, stream or ws"), "{body}");
}

/// Reads the open response until `want` appears in what has arrived, or 3 s pass.
async fn read_until(resp: &mut reqwest::Response, got: &mut String, want: &str) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !got.contains(want) {
        match tokio::time::timeout_at(deadline, resp.chunk()).await {
            Ok(Ok(Some(c))) => got.push_str(&String::from_utf8_lossy(&c)),
            _ => return false,
        }
    }
    true
}

async fn open(url: &str) -> reqwest::Response {
    let resp = reqwest::Client::new().get(url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    resp
}

#[tokio::test(flavor = "multi_thread")]
async fn stream_sends_page_then_patches() {
    let s = start().await;
    let d = logo();
    s.hub.show(&screen(&d));
    let mut resp = open(&format!("{}/?live=stream", s.base)).await;
    let mut got = String::new();
    assert!(
        read_until(&mut resp, &mut got, "stroke-width=\"9.9\"").await,
        "{got}"
    );
    assert!(!got.contains("</body>"), "the body stays open: {got}");
    assert!(!got.contains("http-equiv=\"refresh\""), "{got}");
    widen(&d, "14");
    s.hub.show(&screen(&d));
    assert!(
        read_until(&mut resp, &mut got, "<template for=\"doc\" data-seq=\"2\"").await,
        "{got}"
    );
    assert!(
        read_until(&mut resp, &mut got, "stroke-width=\"14\"").await,
        "{got}"
    );
    assert_eq!(s.metrics.snapshot().stream.open, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn stream_node_grain_sends_node_templates() {
    let s = start().await;
    let d = logo();
    s.hub.show(&screen(&d));
    let mut resp = open(&format!("{}/?live=stream&grain=node", s.base)).await;
    let mut got = String::new();
    assert!(read_until(&mut resp, &mut got, "</main>").await);
    widen(&d, "14");
    s.hub.show(&screen(&d));
    assert!(
        read_until(&mut resp, &mut got, "</template>").await,
        "{got}"
    );
    let patch = &got[got.find("<template").unwrap()..];
    assert!(patch.starts_with("<template for=\"n0-"), "{patch}");
}

#[tokio::test(flavor = "multi_thread")]
async fn stream_skips_patches_older_than_the_page() {
    let s = start().await;
    let d = logo();
    s.hub.show(&screen(&d));
    widen(&d, "11");
    s.hub.show(&screen(&d));
    let mut resp = open(&format!("{}/?live=stream", s.base)).await;
    let mut got = String::new();
    assert!(read_until(&mut resp, &mut got, "</main>").await);
    widen(&d, "12");
    s.hub.show(&screen(&d));
    assert!(read_until(&mut resp, &mut got, "</template>").await);
    assert_eq!(got.matches("<template").count(), 1, "{got}");
    assert!(got.contains("data-seq=\"3\""), "{got}");
}

#[tokio::test(flavor = "multi_thread")]
async fn closed_stream_viewer_is_dropped_and_counted() {
    let s = start().await;
    s.hub.show(&screen(&logo()));
    let resp = open(&format!("{}/?live=stream", s.base)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(s.metrics.snapshot().stream.open, 1);
    drop(resp);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while s.metrics.snapshot().stream.open != 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let m = s.metrics.snapshot();
    assert_eq!((m.stream.open, m.stream.accepted), (0, 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_grain_is_400() {
    let s = start().await;
    let (status, _, _) = get(&format!("{}/?live=stream&grain=atom", s.base), None).await;
    assert_eq!(status, 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_page_loads_live_js() {
    let s = start().await;
    s.hub.show(&screen(&logo()));
    let (status, _, body) = get(&format!("{}/?live=ws", s.base), None).await;
    assert_eq!(status, 200);
    assert!(
        body.contains("<script src=\"/live.js\" data-mode=\"ws\" data-grain=\"node\""),
        "{body}"
    );
    assert!(
        body.contains("</body>"),
        "a ws page is a complete page: {body}"
    );
    let js = reqwest::get(format!("{}/live.js", s.base)).await.unwrap();
    assert_eq!(
        js.headers()["content-type"],
        "text/javascript; charset=utf-8"
    );
    let js = js.text().await.unwrap();
    assert!(js.contains("startViewTransition") && js.contains("createContextualFragment"));
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_first_message_is_region_then_patches() {
    use futures_util::StreamExt;
    let s = start().await;
    let d = logo();
    s.hub.show(&screen(&d));
    let url = format!("{}/live?grain=node", s.base.replace("http", "ws"));
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let next = |m: Option<Result<tokio_tungstenite::tungstenite::Message, _>>| {
        m.unwrap().unwrap().into_text().unwrap().to_string()
    };
    let first = next(
        tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .unwrap(),
    );
    assert!(
        first.starts_with("<template for=\"doc\" data-seq=\"1\""),
        "{first}"
    );
    widen(&d, "14");
    s.hub.show(&screen(&d));
    let second = next(
        tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .unwrap(),
    );
    assert!(second.starts_with("<template for=\"n0-"), "{second}");
    assert!(second.contains("stroke-width=\"14\""));
    let m = s.metrics.snapshot();
    assert_eq!((m.ws.open, m.ws.patch_sets), (1, 2));
}

#[tokio::test(flavor = "multi_thread")]
async fn stream_polyfill_flag_adds_the_script() {
    let s = start().await;
    let mut resp = open(&format!("{}/?live=stream&polyfill=1", s.base)).await;
    let mut got = String::new();
    assert!(read_until(&mut resp, &mut got, "</main>").await);
    assert!(
        read_until(
            &mut resp,
            &mut got,
            "<script src=\"/live.js\" data-mode=\"stream\" data-grain=\"region\"></script>"
        )
        .await,
        "{got}"
    );
    let mut plain = open(&format!("{}/?live=stream", s.base)).await;
    let mut got = String::new();
    assert!(read_until(&mut plain, &mut got, "</main>").await);
    assert!(!got.contains("live.js"), "{got}");
}

#[tokio::test(flavor = "multi_thread")]
async fn closed_ws_viewer_is_counted_down() {
    let s = start().await;
    s.hub.show(&screen(&logo()));
    let url = format!("{}/live", s.base.replace("http", "ws"));
    let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(s.metrics.snapshot().ws.open, 1);
    drop(ws);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while s.metrics.snapshot().ws.open != 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let m = s.metrics.snapshot();
    assert_eq!((m.ws.open, m.ws.accepted), (0, 1));
}
