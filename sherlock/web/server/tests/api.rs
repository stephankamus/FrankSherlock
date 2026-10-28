//! End-to-end tests of the HTTP surface against a real snapshot.
//!
//! The fixture is built the way the desktop builds one: a migrated database, a few rows,
//! a thumbnail on disk and a `current.json` pointing at the file. Nothing is mocked, so
//! these tests fail if the read path and the published layout ever disagree.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use sherlock_web::{api, AppState, Config, MediaMap};
use tempfile::TempDir;
use tower::ServiceExt;

use sherlock_core::db;
use sherlock_core::models::FileRecordUpsert;

const TOKEN: &str = "test-token";

struct Fixture {
    _temp: TempDir,
    library: PathBuf,
    /// Where `?w=` derivatives are written.
    cache: PathBuf,
    state: Arc<AppState>,
    visible_id: i64,
    hidden_id: i64,
    unmapped_id: i64,
    escaping_id: i64,
}

fn record(root_id: i64, rel: &str, index: i64) -> FileRecordUpsert {
    FileRecordUpsert {
        root_id,
        rel_path: rel.to_string(),
        abs_path: format!("/stored/{rel}"),
        filename: format!("file{index}.jpg"),
        media_type: "photo".to_string(),
        description: format!("photo number {index}"),
        extracted_text: String::new(),
        canonical_mentions: String::new(),
        confidence: 0.6,
        lang_hint: "en".to_string(),
        mtime_ns: 1_700_000_000_000_000_000 + index,
        size_bytes: 128,
        fingerprint: format!("fp-{index}"),
        scan_marker: 1,
        location_text: String::new(),
        dhash: None,
        duration_secs: None,
        video_width: None,
        video_height: None,
        video_codec: None,
        audio_codec: None,
    }
}

fn build() -> Fixture {
    let temp = tempfile::tempdir().expect("temp");
    let library = temp.path().join("library");
    let media = temp.path().join("media");
    std::fs::create_dir_all(media.join("a")).expect("media dirs");
    std::fs::create_dir_all(&library).expect("library");

    let db_file = library.join("index.rev-001.sqlite");
    db::init_database(&db_file).expect("init");

    let stored_root = db::upsert_root(&db_file, "/stored").expect("root");
    let other_root = db::upsert_root(&db_file, "/elsewhere").expect("root");

    let visible_id = db::upsert_file_record(&db_file, &record(stored_root, "a/visible.jpg", 1))
        .expect("visible");
    let hidden_id =
        db::upsert_file_record(&db_file, &record(stored_root, "a/hidden.jpg", 2)).expect("hidden");
    db::set_files_hidden(&db_file, &[hidden_id], true).expect("hide");
    let unmapped_id =
        db::upsert_file_record(&db_file, &record(other_root, "b/other.jpg", 3)).expect("other");
    let escaping_id =
        db::upsert_file_record(&db_file, &record(stored_root, "../escape.jpg", 4)).expect("escape");

    // Thumbnail where the published layout says it is.
    let root_key = sherlock_core::thumbnail::root_key("/stored");
    let thumb_dir = library.join("thumbs").join(&root_key).join("a");
    std::fs::create_dir_all(&thumb_dir).expect("thumb dir");
    std::fs::write(thumb_dir.join("visible.jpg"), b"jpeg-bytes").expect("thumb");

    // A real JPEG behind the visible row: `/api/media?w=` needs something the decoder
    // can read, and the `Range` tests need a body with more than a handful of bytes.
    let media_file = media.join("a").join("visible.jpg");
    image::RgbImage::from_pixel(120, 60, image::Rgb([200, 40, 40]))
        .save_with_format(&media_file, image::ImageFormat::Jpeg)
        .expect("media file");

    let pointer = serde_json::json!({
        "format": 1,
        "revision": "rev-001",
        "generatedAtUnix": 1_700_000_000,
        "schemaVersion": 21,
        "database": { "file": "index.rev-001.sqlite", "bytes": 4096, "sha256": "00" },
        "library": { "files": 4, "roots": 2, "hiddenFiles": 1 },
        "thumbnails": { "dir": "thumbs" },
        "faceCrops": { "dir": "" },
        "roots": [
            { "id": stored_root, "name": "Fotos", "path": "/stored", "rootKey": root_key, "files": 3 },
            { "id": other_root, "name": "Outros", "path": "/elsewhere", "rootKey": "zzz", "files": 1 }
        ],
    });
    std::fs::write(
        library.join("current.json"),
        serde_json::to_vec(&pointer).expect("pointer"),
    )
    .expect("pointer file");

    let media_map = MediaMap::from_json(&format!(
        r#"[{{"storedPath":"/stored","mountedPath":"{}"}}]"#,
        media.display()
    ))
    .expect("media map");

    let cache = temp.path().join("cache");
    let config = Config {
        library_dir: library.clone(),
        media_map,
        auth_token: Some(TOKEN.to_string()),
        cache_dir: cache.clone(),
        port: 0,
    };

    Fixture {
        _temp: temp,
        library,
        cache,
        state: AppState::new(config),
        visible_id,
        hidden_id,
        unmapped_id,
        escaping_id,
    }
}

async fn get_with_headers(
    state: &Arc<AppState>,
    uri: &str,
    extra: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder()
        .uri(uri)
        .header("Authorization", format!("Bearer {TOKEN}"));
    for (name, value) in extra {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(Body::empty()).expect("request");
    let response = api::app(Arc::clone(state))
        .oneshot(request)
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, headers, body)
}

async fn get(state: &Arc<AppState>, uri: &str) -> (StatusCode, Vec<u8>) {
    let (status, _, body) = get_with_headers(state, uri, &[]).await;
    (status, body)
}

fn json(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body).expect("json body")
}

#[tokio::test]
async fn health_reports_the_revision_and_the_unmapped_roots() {
    let fixture = build();
    let (status, body) = get(&fixture.state, "/api/health").await;
    assert_eq!(status, StatusCode::OK);
    let health = json(&body);
    assert_eq!(health["revision"], "rev-001");
    assert_eq!(health["hiddenFiles"], 1);
    assert_eq!(health["mappedRoots"], 1);
    assert_eq!(health["unmappedRoots"][0], "/elsewhere");
}

#[tokio::test]
async fn the_api_requires_the_token() {
    let fixture = build();
    let request = Request::builder()
        .uri("/api/health")
        .body(Body::empty())
        .expect("request");
    let response = api::app(Arc::clone(&fixture.state))
        .oneshot(request)
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_token_is_also_accepted_as_a_query_parameter() {
    // `<img src>` cannot send a header, so the grid depends on this form.
    let fixture = build();
    let (status, _) = get(&fixture.state, &format!("/api/search?token={TOKEN}")).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn search_never_returns_hidden_rows_even_when_asked() {
    let fixture = build();
    let (status, body) = get(&fixture.state, "/api/search?includeHidden=true").await;
    assert_eq!(status, StatusCode::OK);
    let response = json(&body);
    let ids: Vec<i64> = response["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["id"].as_i64().expect("id"))
        .collect();
    assert!(ids.contains(&fixture.visible_id));
    assert!(
        !ids.contains(&fixture.hidden_id),
        "includeHidden is not part of the web contract"
    );
    assert_eq!(response["total"], 3, "the hidden row is not counted either");
}

#[tokio::test]
async fn search_items_carry_urls_and_no_host_paths() {
    let fixture = build();
    let (_, body) = get(&fixture.state, "/api/search").await;
    let response = json(&body);
    // The default order is by date modified, so the row is looked up by id rather than
    // by position.
    let item = response["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["id"].as_i64() == Some(fixture.visible_id))
        .expect("visible item");
    assert_eq!(
        item["thumbUrl"],
        format!("/api/thumb/{}", fixture.visible_id)
    );
    assert_eq!(
        item["mediaUrl"],
        format!("/api/media/{}", fixture.visible_id)
    );
    assert!(item.get("absPath").is_none(), "host paths must not leak");
    assert!(item.get("thumbnailPath").is_none());
}

#[tokio::test]
async fn hidden_rows_are_404_on_every_id_based_endpoint() {
    let fixture = build();
    for uri in [
        format!("/api/thumb/{}", fixture.hidden_id),
        format!("/api/media/{}", fixture.hidden_id),
        // The derivative takes the same path, so it must be refused before decoding.
        format!("/api/media/{}?w=60", fixture.hidden_id),
        format!("/api/item/{}", fixture.hidden_id),
    ] {
        let (status, _) = get(&fixture.state, &uri).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{uri} must not serve a hidden row"
        );
    }
}

#[tokio::test]
async fn the_thumbnail_comes_from_the_published_layout() {
    let fixture = build();
    let (status, body) = get(
        &fixture.state,
        &format!("/api/thumb/{}", fixture.visible_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"jpeg-bytes");
}

#[tokio::test]
async fn media_is_served_through_the_mount() {
    let fixture = build();
    let (status, body) = get(
        &fixture.state,
        &format!("/api/media/{}", fixture.visible_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let original = image::load_from_memory(&body).expect("jpeg");
    assert_eq!((original.width(), original.height()), (120, 60));
}

#[tokio::test]
async fn an_unmapped_root_is_a_conflict_with_the_reason() {
    let fixture = build();
    let (status, body) = get(
        &fixture.state,
        &format!("/api/media/{}", fixture.unmapped_id),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let error = json(&body);
    assert!(
        error["error"]
            .as_str()
            .expect("error")
            .contains("MEDIA_MAP"),
        "{error}"
    );
}

#[tokio::test]
async fn a_rel_path_that_climbs_out_is_refused() {
    let fixture = build();
    let (status, _) = get(
        &fixture.state,
        &format!("/api/media/{}", fixture.escaping_id),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn item_returns_metadata_without_host_paths() {
    let fixture = build();
    let (status, body) = get(&fixture.state, &format!("/api/item/{}", fixture.visible_id)).await;
    assert_eq!(status, StatusCode::OK);
    let item = json(&body);
    assert_eq!(item["rootName"], "Fotos");
    assert_eq!(item["relPath"], "a/visible.jpg");
    assert!(item.get("absPath").is_none());
}

#[tokio::test]
async fn the_library_endpoint_matches_the_pointer() {
    let fixture = build();
    let (status, body) = get(&fixture.state, "/api/library").await;
    assert_eq!(status, StatusCode::OK);
    let library = json(&body);
    assert_eq!(library["revision"], "rev-001");
    assert_eq!(library["databaseFile"], "index.rev-001.sqlite");
    assert_eq!(library["hiddenFiles"], 1);
    assert!(
        Path::new(&library["databaseFile"].as_str().expect("file")).is_relative(),
        "the pointer must only ever name a file inside the library"
    );
    // Silence the unused-field lint of the fixture while keeping the field for debugging.
    let _ = &fixture.library;
}

#[tokio::test]
async fn media_preview_is_downscaled_and_reused_from_the_cache() {
    let fixture = build();
    let uri = format!("/api/media/{}?w=60", fixture.visible_id);
    let (status, headers, body) = get_with_headers(&fixture.state, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "image/jpeg");

    let preview = image::load_from_memory(&body).expect("preview jpeg");
    assert_eq!(
        (preview.width(), preview.height()),
        (60, 30),
        "the aspect ratio is preserved"
    );

    let cached = fixture
        .cache
        .join("rev-001")
        .join(format!("{}_60.jpg", fixture.visible_id));
    assert!(cached.is_file(), "the derivative must be cached on disk");

    // Overwrite the cache with a marker: a second request must serve it instead of
    // paying for another decode, which is the whole point of the cache.
    std::fs::write(&cached, b"cached-marker").expect("overwrite cache");
    let (status, _, body) = get_with_headers(&fixture.state, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"cached-marker");
}

#[tokio::test]
async fn a_huge_preview_width_is_clamped_to_the_server_ceiling() {
    let fixture = build();
    let (status, _, body) = get_with_headers(
        &fixture.state,
        &format!("/api/media/{}?w=99999", fixture.visible_id),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The source is 120 px wide, smaller than the ceiling, so it is not upscaled.
    let preview = image::load_from_memory(&body).expect("preview jpeg");
    assert_eq!(preview.width(), 120);

    let cached = fixture
        .cache
        .join("rev-001")
        .join(format!("{}_4096.jpg", fixture.visible_id));
    assert!(
        cached.is_file(),
        "the cache is namespaced by the clamped width"
    );
}

#[tokio::test]
async fn thumbnails_carry_an_etag_and_answer_if_none_match() {
    let fixture = build();
    let uri = format!("/api/thumb/{}", fixture.visible_id);
    let (status, headers, body) = get_with_headers(&fixture.state, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"jpeg-bytes");
    let etag = headers["etag"].to_str().expect("etag header").to_string();
    assert!(etag.starts_with('"') && etag.ends_with('"'));

    let (status, headers, body) =
        get_with_headers(&fixture.state, &uri, &[("if-none-match", etag.as_str())]).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert_eq!(headers["etag"], etag.as_str());
    assert!(body.is_empty());
}

#[tokio::test]
async fn responses_carry_the_security_headers() {
    let fixture = build();
    let (_, headers, _) = get_with_headers(&fixture.state, "/api/health", &[]).await;
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["x-frame-options"], "SAMEORIGIN");
    let csp = headers["content-security-policy"]
        .to_str()
        .expect("csp header");
    assert!(csp.contains("frame-ancestors 'self'"), "{csp}");
}

#[tokio::test]
async fn media_answers_range_requests() {
    let fixture = build();
    let (status, headers, body) = get_with_headers(
        &fixture.state,
        &format!("/api/media/{}", fixture.visible_id),
        &[("range", "bytes=0-3")],
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body.len(), 4);
    assert!(headers.contains_key("content-range"));
}

#[tokio::test]
async fn an_invalid_range_is_416_not_a_panic() {
    // `ServeFile` treats both an unsatisfiable range and an unparsable one as 416 and
    // sends `Content-Range: bytes */len`; it never reads the whole file to answer a
    // broken request. That is the behaviour a mobile player can act on.
    let fixture = build();
    for range in ["bytes=99999999-", "bytes=abc", "bytes=5-1"] {
        let (status, headers, _) = get_with_headers(
            &fixture.state,
            &format!("/api/media/{}", fixture.visible_id),
            &[("range", range)],
        )
        .await;
        assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE, "{range}");
        let content_range = headers["content-range"]
            .to_str()
            .expect("content-range header");
        assert!(content_range.starts_with("bytes */"), "{content_range}");
    }
}
