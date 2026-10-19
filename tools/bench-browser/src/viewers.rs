//! Lightweight viewers: readers of each delivery mode that note which edit
//! (`data-bench`) they've seen, when, and how many bytes arrived.

use futures_util::StreamExt;
use std::time::{Duration, Instant};

/// What one viewer saw.
#[derive(Debug, Default)]
pub struct Seen {
    /// Each new highest edit number, and when it arrived.
    pub obs: Vec<(u64, Instant)>,
    pub bytes: u64,
    pub error: Option<String>,
}

impl Seen {
    fn saw(&mut self, text: &str) {
        let now = Instant::now();
        let mut best = None;
        for part in text.split("data-bench=\"").skip(1) {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(k) = digits.parse::<u64>() {
                best = best.max(Some(k));
            }
        }
        if let Some(k) = best {
            if self.obs.last().is_none_or(|(last, _)| k > *last) {
                self.obs.push((k, now));
            }
        }
    }

    /// When this viewer first saw edit `k` or a later one.
    pub fn first_at_least(&self, k: u64) -> Option<Instant> {
        self.obs
            .iter()
            .find(|(seen, _)| *seen >= k)
            .map(|(_, at)| *at)
    }
}

pub async fn refresh(url: &str, until: Instant) -> Seen {
    let mut seen = Seen::default();
    let client = reqwest::Client::new();
    let mut etag: Option<String> = None;
    while Instant::now() < until {
        let mut req = client.get(format!("{url}/?live=refresh"));
        if let Some(e) = &etag {
            req = req.header("if-none-match", e);
        }
        match req.send().await {
            Ok(resp) if resp.status().as_u16() == 304 => {}
            Ok(resp) => {
                etag = resp
                    .headers()
                    .get("etag")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                match resp.text().await {
                    Ok(body) => {
                        seen.bytes += body.len() as u64;
                        seen.saw(&body);
                    }
                    Err(e) => seen.error = Some(e.to_string()),
                }
            }
            Err(e) => seen.error = Some(e.to_string()),
        }
        // The page's own meta refresh: once a second.
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    seen
}

pub async fn stream(url: &str, grain: &str, until: Instant) -> Seen {
    let mut seen = Seen::default();
    let resp = match reqwest::get(format!("{url}/?live=stream&grain={grain}")).await {
        Ok(r) => r,
        Err(e) => {
            seen.error = Some(e.to_string());
            return seen;
        }
    };
    let mut body = resp.bytes_stream();
    // Keep the end of the last chunk, in case a number was split across two.
    let mut carry = String::new();
    loop {
        let left = until.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, body.next()).await {
            Ok(Some(Ok(chunk))) => {
                seen.bytes += chunk.len() as u64;
                let text = carry + &String::from_utf8_lossy(&chunk);
                seen.saw(&text);
                let keep = text.len().saturating_sub(40);
                carry = text[text.floor_char_boundary(keep)..].to_string();
            }
            Ok(Some(Err(e))) => {
                seen.error = Some(e.to_string());
                return seen;
            }
            Ok(None) | Err(_) => return seen,
        }
    }
}

pub async fn ws(url: &str, grain: &str, until: Instant) -> Seen {
    let mut seen = Seen::default();
    let url = format!("{}/live?grain={grain}", url.replacen("http", "ws", 1));
    let mut socket = match tokio_tungstenite::connect_async(url).await {
        Ok((s, _)) => s,
        Err(e) => {
            seen.error = Some(e.to_string());
            return seen;
        }
    };
    loop {
        let left = until.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, socket.next()).await {
            Ok(Some(Ok(m))) => {
                if let Ok(text) = m.into_text() {
                    seen.bytes += text.len() as u64;
                    seen.saw(&text);
                }
            }
            Ok(Some(Err(e))) => {
                seen.error = Some(e.to_string());
                return seen;
            }
            Ok(None) | Err(_) => return seen,
        }
    }
}
