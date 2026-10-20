//! Each mode against a real y-sweet and an in-process serve app.

use bench_browser::{run, Config, Mode};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use xtxp_muppet::core::Mode as CoreMode;
use xtxp_muppet::ports::{Outcome, Screen, View};

const LOGO: &str = include_str!("../../../crates/xmesh/tests/fixtures/xtxp-logo.svg");

fn ysweet_bin() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::env::var_os("YSWEET_BIN")
        .map(PathBuf::from)
        .or_else(|| Some(root.join(".tools/bin/y-sweet")))
        .filter(|p| p.exists())
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Quiet;
impl View for Quiet {
    fn show(&mut self, _: &Screen) {}
    fn status(&mut self, _: &str) {}
    fn finished(&mut self, _: &Outcome) {}
}

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

#[test]
fn smoke_each_mode() {
    let Some(bin) = ysweet_bin() else {
        assert!(std::env::var_os("CI").is_none(), "no y-sweet binary");
        eprintln!("skipped: no y-sweet (set YSWEET_BIN)");
        return;
    };
    let dir = std::env::temp_dir().join(format!("bench-smoke-{}", std::process::id()));
    let port = free_port();
    let _ys = Kill(
        Command::new(bin)
            .args([
                "serve",
                dir.to_str().unwrap(),
                "--port",
                &port.to_string(),
                "--skip-gc",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let ysweet = format!("127.0.0.1:{port}");
    for _ in 0..100 {
        if std::net::TcpStream::connect(&ysweet).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let seed = xmesh::Xmesh::seed(LOGO).unwrap();
    xtxp_muppet::adapters::ysweet::create_doc(&ysweet, "logo").unwrap();
    let out = xtxp_muppet::adapters::ws::run(CoreMode::Init { seed }, &ysweet, "logo", Quiet, None);
    assert!(out.ok, "{out:?}");

    let (hub, metrics) = xtxp_serve::start(&ysweet, "logo");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let url = rt.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = xtxp_serve::router(hub, metrics);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    });
    std::thread::sleep(Duration::from_millis(500));
    for (mode, grain) in [
        (Mode::Refresh, "region"),
        (Mode::Stream, "region"),
        (Mode::Stream, "node"),
        (Mode::Ws, "node"),
        (Mode::Ws, "region"),
    ] {
        let cfg = Config {
            url: url.clone(),
            mode,
            grain: grain.into(),
            viewers: 10,
            edits_per_sec: 2.0,
            duration: Duration::from_secs(3),
            ysweet: ysweet.clone(),
            doc: "logo".into(),
        };
        let row = rt.block_on(run(&cfg)).unwrap();
        eprintln!("{row:?}");
        assert_eq!(
            (row.errors, row.missed),
            (0, 0),
            "{mode:?}/{grain}: {row:?}"
        );
        assert!(row.p50_ms > 0.0, "{row:?}");
        assert!(row.bytes_per_viewer_s > 0.0, "{row:?}");
    }
}
