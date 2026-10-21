//! XTXP on a Cloudflare Worker: one Durable Object per document is both the
//! host that muppets sync with, speaking y-sweet v0.9.1's protocol so
//! `xtxp-tui`, `xtxp-orbit` and `xtxp-muppet` connect unchanged, and the
//! browser server, rendering the document in `xtxp-serve`'s three delivery
//! modes.
//!
//! | Path | What |
//! | --- | --- |
//! | `POST /doc/new` | y-sweet's create: every name exists, so it only checks the name. |
//! | `/d/{doc}/ws/{doc}` | A muppet's WebSocket to the host. |
//! | `/d/{doc}/` | The page: `?live=refresh`, `stream` or `ws`, as in `xtxp-serve`. |
//! | `/d/{doc}/live` | The WebSocket mode's patches. |
//! | `/d/{doc}/live.js` | The polyfill. |

mod room;

use worker::{event, Context, Env, Headers, Method, Request, Response, Result};
use xtxp_serve::render::LIVE_JS;

pub use room::Room;

/// y-sweet's rule: ASCII letters, digits, `-` and `_`, not empty.
fn valid_doc_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The document a `/d/{doc}/...` path is about, and the rest of the path.
pub(crate) fn doc_path(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/d/")?;
    let (doc, rest) = rest.split_once('/').unwrap_or((rest, ""));
    valid_doc_name(doc).then_some((doc, rest))
}

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let path = req.path();
    if path == "/doc/new" && req.method() == Method::Post {
        #[derive(serde::Deserialize)]
        struct New {
            #[serde(rename = "docId")]
            doc_id: String,
        }
        return match serde_json::from_str::<New>(&req.text().await?) {
            Ok(New { doc_id }) if valid_doc_name(&doc_id) => {
                Response::from_json(&serde_json::json!({ "docId": doc_id }))
            }
            _ => Response::error("docId must be letters, digits, - and _\n", 400),
        };
    }
    let Some((doc, rest)) = doc_path(&path) else {
        return if path == "/" {
            Response::ok(
                "XTXP on a Worker. Open /d/{doc}/ in a browser; point a muppet's --server here.\n",
            )
        } else {
            Response::error("not found\n", 404)
        };
    };
    match rest {
        // Relative script and socket URLs need the page under a directory.
        "" if !path.ends_with('/') => {
            let mut url = req.url()?;
            url.set_path(&format!("{path}/"));
            Response::redirect_with_status(url, 301)
        }
        "live.js" => {
            let headers = Headers::new();
            headers.set("content-type", "text/javascript; charset=utf-8")?;
            Ok(Response::ok(LIVE_JS)?.with_headers(headers))
        }
        _ => {
            let stub = env.durable_object("ROOM")?.get_by_name(doc)?;
            stub.fetch_with_request(req).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_paths() {
        assert_eq!(doc_path("/d/logo/"), Some(("logo", "")));
        assert_eq!(doc_path("/d/logo"), Some(("logo", "")));
        assert_eq!(doc_path("/d/logo/ws/logo"), Some(("logo", "ws/logo")));
        assert_eq!(doc_path("/d/bad name/"), None);
        assert_eq!(doc_path("/d//"), None);
        assert_eq!(doc_path("/x/logo/"), None);
    }
}
