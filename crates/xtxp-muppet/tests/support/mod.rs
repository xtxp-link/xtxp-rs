//! Running real y-sweet, the chaos proxy and muppet processes for the
//! end-to-end tests.

#![allow(dead_code)]

use chaos_proxy::Chaos;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// `YSWEET_BIN`, then `.tools/bin/y-sweet` in the repo.
pub fn ysweet_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("YSWEET_BIN") {
        return Some(PathBuf::from(p));
    }
    let local = repo_root().join(".tools/bin/y-sweet");
    local.exists().then_some(local)
}

/// Skips the test when there's no y-sweet, except in CI, where it fails.
#[macro_export]
macro_rules! require_ysweet {
    () => {
        match support::ysweet_bin() {
            Some(bin) => bin,
            None if std::env::var_os("CI").is_some() => {
                panic!("no y-sweet binary: set YSWEET_BIN or build .tools/bin/y-sweet")
            }
            None => {
                eprintln!("skipped: no y-sweet (set YSWEET_BIN)");
                return;
            }
        }
    };
}

pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "xtxp-e2e-{name}-{}-{}",
        std::process::id(),
        free_port()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A y-sweet process storing into `dir`, with GC off and a 1 s checkpoint.
pub struct YSweet {
    bin: PathBuf,
    pub dir: PathBuf,
    pub port: u16,
    child: Option<Child>,
}

impl YSweet {
    pub fn start(bin: &Path, dir: &Path) -> YSweet {
        let mut ys = YSweet {
            bin: bin.to_path_buf(),
            dir: dir.to_path_buf(),
            port: free_port(),
            child: None,
        };
        ys.spawn();
        ys
    }

    fn spawn(&mut self) {
        let store = self.dir.join("store");
        std::fs::create_dir_all(&store).unwrap();
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("y-sweet.log"))
            .unwrap();
        let child = Command::new(&self.bin)
            .args([
                "serve",
                store.to_str().unwrap(),
                "--port",
                &self.port.to_string(),
                "--skip-gc",
                "--checkpoint-freq-seconds",
                "1",
            ])
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("start y-sweet");
        self.child = Some(child);
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if http_get(self.addr(), "/ready").is_some_and(|r| r.starts_with("HTTP/1.1 200")) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("y-sweet didn't become ready on port {}", self.port);
    }

    pub fn addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.port))
    }

    pub fn server(&self) -> String {
        self.addr().to_string()
    }

    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// Kills y-sweet and starts it again on the same folder and port.
    pub fn restart(&mut self) {
        self.stop();
        self.spawn();
    }
}

impl Drop for YSweet {
    fn drop(&mut self) {
        self.stop();
    }
}

fn http_get(addr: SocketAddr, path: &str) -> Option<String> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(300)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\r\n"
    )
    .ok()?;
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    Some(out)
}

/// The chaos proxy, in-process on its own thread and runtime; its log goes to this process's stderr.
pub struct Proxy {
    pub port: u16,
}

impl Proxy {
    pub fn start(upstream: SocketAddr, chaos: Chaos) -> Proxy {
        let port = free_port();
        let listen = SocketAddr::from(([127, 0, 0, 1], port));
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let _ = rt.block_on(chaos_proxy::serve(listen, upstream, chaos));
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while TcpStream::connect(listen).is_err() {
            assert!(Instant::now() < deadline, "proxy didn't start");
            std::thread::sleep(Duration::from_millis(20));
        }
        Proxy { port }
    }

    pub fn server(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
}

/// Runs the muppet binary.
pub fn muppet(server: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtxp-muppet"))
        .arg("--server")
        .arg(server)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run xtxp-muppet")
}

/// Starts the muppet binary without waiting.
pub fn spawn_muppet(server: &str, args: &[String]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_xtxp-muppet"))
        .arg("--server")
        .arg(server)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn xtxp-muppet")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hashes {
    pub doc_hash: String,
    pub snapshot_hash: String,
    pub updates_before_synced: u64,
}

pub fn hashes(out: &Output) -> Hashes {
    assert!(
        out.status.success(),
        "muppet failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let line = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value =
        serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("{e}: {line}"));
    Hashes {
        doc_hash: v["doc_hash"].as_str().unwrap().into(),
        snapshot_hash: v["snapshot_hash"].as_str().unwrap().into(),
        updates_before_synced: v["updates_before_synced"].as_u64().unwrap(),
    }
}

/// `XTXP_E2E_SABOTAGE=1` gives one peer the wrong batches, to prove the tests can fail.
pub fn sabotage() -> bool {
    std::env::var_os("XTXP_E2E_SABOTAGE").is_some()
}
