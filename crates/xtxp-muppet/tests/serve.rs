//! The server in process: screens go straight into the hub, no y-sweet.

use std::net::SocketAddr;
use xmesh::{Op, Xmesh};
use xtxp_muppet::adapters::serve::{router, Hub, Metrics};
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
    let app = router(hub.clone(), metrics.clone());
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
