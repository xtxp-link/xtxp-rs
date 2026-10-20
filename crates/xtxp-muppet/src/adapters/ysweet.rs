//! Talking to y-sweet v0.9.1 with no auth (P2).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// The WebSocket URL of a document.
pub fn ws_url(server: &str, doc: &str) -> String {
    format!("ws://{server}/d/{doc}/ws/{doc}")
}

/// y-sweet's own rule: ASCII letters, digits, `-` and `_`, not empty.
pub fn valid_doc_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Validates a document name; returns an error message if it's invalid.
/// Used by all app adapters to give consistent error messages to the user.
pub fn check_doc(name: &str) -> Result<(), String> {
    if valid_doc_name(name) {
        Ok(())
    } else {
        Err(format!(
            "{name:?} isn't a valid document name (letters, digits, - and _)"
        ))
    }
}

/// `POST /doc/new {"docId": name}`: creates the document, or finds it.
pub fn create_doc(server: &str, name: &str) -> Result<(), String> {
    let body = format!("{{\"docId\":\"{name}\"}}");
    let request = format!(
        "POST /doc/new HTTP/1.1\r\nhost: {server}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let unreachable = |e: std::io::Error| format!("could not reach y-sweet at {server}: {e}");
    let mut sock = TcpStream::connect(server).map_err(unreachable)?;
    sock.set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(unreachable)?;
    sock.write_all(request.as_bytes()).map_err(unreachable)?;
    let mut response = String::new();
    let _ = sock.read_to_string(&mut response);
    let status = response.split_whitespace().nth(1).unwrap_or("");
    if status == "200" {
        Ok(())
    } else {
        Err(format!("y-sweet refused to create {name:?}: HTTP {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn ws_url_shape() {
        assert_eq!(
            ws_url("localhost:8080", "demo"),
            "ws://localhost:8080/d/demo/ws/demo"
        );
    }

    #[test]
    fn create_doc_body() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = listener.local_addr().unwrap().to_string();
        let stub = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = vec![0u8; 4096];
            let n = sock.read(&mut buf).unwrap();
            sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 16\r\n\r\n{\"docId\":\"demo\"}")
                .unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        create_doc(&server, "demo").unwrap();
        let request = stub.join().unwrap();
        assert!(
            request.starts_with("POST /doc/new HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(
            request.ends_with("\r\n\r\n{\"docId\":\"demo\"}"),
            "{request}"
        );
        assert!(request.contains("content-type: application/json\r\n"));
    }

    #[test]
    fn create_doc_reports_http_errors() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = sock.read(&mut buf);
            sock.write_all(b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\n\r\n")
                .unwrap();
        });
        let err = create_doc(&server, "bad name").unwrap_err();
        assert!(err.contains("400"), "{err}");
    }

    #[test]
    fn doc_names_are_checked_like_y_sweet() {
        assert!(valid_doc_name("demo_1-x"));
        assert!(!valid_doc_name("bad name"));
        assert!(!valid_doc_name(""));
    }
}
