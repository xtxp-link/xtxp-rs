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
    assert_eq!(tag, Some(format!("\"{}-live\"", d.snapshot_hash())));
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

#[tokio::test(flavor = "multi_thread")]
async fn bare_root_has_noscript_refresh_and_picker() {
    let s = start().await;
    s.hub.show(&screen(&logo()));
    let (status, _, body) = get(&format!("{}/", s.base), None).await;
    assert_eq!(status, 200);
    assert!(
        body.contains("<noscript><meta http-equiv=\"refresh\" content=\"1\"></noscript>"),
        "{body}"
    );
    assert_eq!(
        body.matches("http-equiv=\"refresh\"").count(),
        1,
        "only inside noscript: {body}"
    );
    assert!(
        body.contains("location.replace") && body.contains("parseHTMLUnsafe"),
        "{body}"
    );
    assert!(body.contains("/live.js"), "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn metrics_flag_adds_the_script() {
    let s = start().await;
    let (_, _, body) = get(&format!("{}/?live=ws&metrics=1", s.base), None).await;
    assert!(
        body.contains("<script src=\"/metrics.js\"></script>"),
        "{body}"
    );
    let (_, _, plain) = get(&format!("{}/?live=ws", s.base), None).await;
    assert!(!plain.contains("metrics.js"));
    let js = reqwest::get(format!("{}/metrics.js", s.base))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(js.contains("sendBeacon") && js.contains("/metrics/client"));
}

#[tokio::test(flavor = "multi_thread")]
async fn stream_with_metrics_marks_each_patch_set() {
    let s = start().await;
    let d = logo();
    s.hub.show(&screen(&d));
    let mut resp = open(&format!("{}/?live=stream&metrics=1", s.base)).await;
    let mut got = String::new();
    assert!(read_until(&mut resp, &mut got, "</main>").await);
    widen(&d, "14");
    s.hub.show(&screen(&d));
    assert!(
        read_until(
            &mut resp,
            &mut got,
            "<i hidden data-xtxp-seq=\"2\" data-xtxp-sent=\""
        )
        .await,
        "{got}"
    );
}

async fn metrics_json(base: &str) -> serde_json::Value {
    serde_json::from_str(
        &reqwest::get(format!("{base}/metrics"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn metrics_counts_add_up() {
    use futures_util::StreamExt;
    let s = start().await;
    let d = logo();
    s.hub.show(&screen(&d));
    let mut stream = open(&format!("{}/?live=stream", s.base)).await;
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("{}/live", s.base.replace("http", "ws")))
            .await
            .unwrap();
    let mut got = String::new();
    let mut ws_bytes = 0;
    for w in ["11", "12", "13"] {
        widen(&d, w);
        s.hub.show(&screen(&d));
        assert!(read_until(&mut stream, &mut got, &format!("stroke-width=\"{w}\"")).await);
    }
    for _ in 0..4 {
        let m = tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        ws_bytes += m.into_text().unwrap().len();
    }
    let m = metrics_json(&s.base).await;
    assert_eq!(m["modes"]["stream"]["open"], 1);
    assert_eq!(m["modes"]["ws"]["open"], 1);
    assert_eq!(m["modes"]["stream"]["patch_sets"], 3);
    assert_eq!(
        m["modes"]["ws"]["patch_sets"], 4,
        "the whole document, then three"
    );
    assert_eq!(m["modes"]["ws"]["bytes"], ws_bytes);
    assert!(m["modes"]["stream"]["bytes"].as_u64().unwrap() >= got.len() as u64);
    assert!(m["render_us"]["p50"].as_u64().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn client_batches_are_aggregated() {
    let s = start().await;
    let post = |body: &'static str| {
        let url = format!("{}/metrics/client", s.base);
        async move {
            reqwest::Client::new()
                .post(url)
                .body(body)
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    assert_eq!(post(r#"{"mode":"ws","grain":"node","samples":[{"seq":2,"latency_ms":10,"vt_ms":5}],"jank":1}"#).await, 204);
    assert_eq!(post(r#"{"mode":"ws","grain":"node","samples":[{"seq":3,"latency_ms":30,"vt_ms":7}],"jank":0}"#).await, 204);
    assert_eq!(post("not json").await, 400);
    let m = metrics_json(&s.base).await;
    let c = &m["client"]["ws/node"];
    assert_eq!(c["samples"], 2);
    assert_eq!(c["latency_ms"]["p50"], 10, "nearest rank");
    assert_eq!(c["latency_ms"]["p95"], 30);
    assert_eq!(c["vt_ms"]["p50"], 5);
    assert_eq!(c["jank"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn process_stats_are_present() {
    let s = start().await;
    let m = metrics_json(&s.base).await;
    assert!(m["process"]["max_rss_kb"].as_u64().unwrap() > 0);
    assert!(m["process"]["cpu_ms"].as_u64().is_some());
    assert_eq!(m["refresh"]["ok"], 0);
}

fn offline(d: &Xmesh) -> Screen {
    Screen {
        connected: false,
        ..screen(d)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn stream_viewer_sees_the_connection_state_change() {
    let s = start().await;
    let d = logo();
    s.hub.show(&offline(&d));
    let mut resp = open(&format!("{}/?live=stream", s.base)).await;
    let mut got = String::new();
    assert!(read_until(&mut resp, &mut got, "</main>").await);
    assert!(
        got.contains("<?start name=\"state\">"),
        "the state is patchable: {got}"
    );
    s.hub.show(&screen(&d));
    assert!(
        read_until(&mut resp, &mut got, "<template for=\"state\"").await,
        "{got}"
    );
    assert!(read_until(&mut resp, &mut got, "● live").await, "{got}");
    s.hub.show(&offline(&d));
    let mut more = String::new();
    assert!(
        read_until(&mut resp, &mut more, "○ connecting<").await,
        "{more}"
    );
    assert!(more.contains("<template for=\"state\""), "{more}");
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_etag_follows_the_connection_state() {
    let s = start().await;
    let d = logo();
    s.hub.show(&screen(&d));
    let url = format!("{}/?live=refresh", s.base);
    let (_, tag, _) = get(&url, None).await;
    s.hub.show(&offline(&d));
    let (status, _, body) = get(&url, tag.as_deref()).await;
    assert_eq!(status, 200, "same document, but the state changed");
    assert!(body.contains("○ connecting"));
}
