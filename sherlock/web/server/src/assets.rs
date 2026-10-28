//! The SPA, embedded when the crate is built with `--features embed-ui`.
//!
//! The container image builds `sherlock/web/ui` first and compiles it into the binary,
//! so deployment is one file and one process. The feature is off by default: `cargo
//! test` on a checkout without a frontend build must still work.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

#[cfg(feature = "embed-ui")]
#[derive(rust_embed::RustEmbed)]
#[folder = "../ui/dist/"]
struct Assets;

/// Long cache for the hashed build output, revalidation for the shell.
#[cfg_attr(not(feature = "embed-ui"), allow(dead_code))]
fn cache_control(path: &str) -> &'static str {
    if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

#[cfg(feature = "embed-ui")]
fn asset(path: &str) -> Option<Response> {
    let path = path.trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let file = Assets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let content_type = HeaderValue::from_str(mime.as_ref()).ok()?;
    Some(
        (
            [
                (header::CONTENT_TYPE, content_type),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static(cache_control(path)),
                ),
            ],
            file.data.into_owned(),
        )
            .into_response(),
    )
}

/// Serve a path, falling back to `index.html` for client-side routes.
///
/// A missing file with an extension is a 404: answering HTML for a missing `.js`
/// would make the browser report a syntax error instead of a missing asset.
pub fn serve(path: &str) -> Response {
    #[cfg(feature = "embed-ui")]
    {
        if let Some(response) = asset(path) {
            return response;
        }
        let looks_like_asset = std::path::Path::new(path).extension().is_some();
        if !looks_like_asset {
            if let Some(response) = asset("index.html") {
                return response;
            }
        }
        StatusCode::NOT_FOUND.into_response()
    }

    #[cfg(not(feature = "embed-ui"))]
    {
        let _ = path;
        (
            StatusCode::NOT_FOUND,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            r#"{"error":"this build has no embedded web UI (compile with --features embed-ui)"}"#,
        )
            .into_response()
    }
}

/// The `/` page: the SPA when embedded, otherwise the API pointer it always was.
pub fn index() -> Response {
    #[cfg(feature = "embed-ui")]
    {
        let response = serve("/");
        if response.status() != StatusCode::NOT_FOUND {
            return response;
        }
    }
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
        "<!doctype html><title>Frank Sherlock</title><p>Library API is up. \
         Try <a href=\"/api/health\">/api/health</a>.</p>",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hashed_assets_are_cached_forever_and_the_shell_is_not() {
        assert!(cache_control("assets/index-abc123.js").contains("immutable"));
        assert_eq!(cache_control("index.html"), "no-cache");
        assert_eq!(cache_control("manifest.webmanifest"), "no-cache");
    }

    #[test]
    fn a_build_without_the_feature_still_answers_the_index() {
        // The placeholder must stay available: the server is useful without the SPA.
        let response = index();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn an_unknown_path_is_not_a_silent_index() {
        assert_eq!(serve("/nope.js").status(), StatusCode::NOT_FOUND);
    }
}
