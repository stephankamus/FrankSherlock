//! The HTTP surface: the same reads the desktop does, over the published snapshot.
//!
//! Two rules are enforced here rather than trusted to the clients:
//!
//! 1. `include_hidden` is **always** `false`, and the media endpoints answer `404` for a
//!    hidden row. Ids are enumerable, so filtering only the search would still leave a
//!    hidden photo reachable by guessing `/api/media/1234`.
//! 2. No path ever comes from the request. Every file is resolved from a row id, through
//!    the root mapping, with the relative path validated against escapes.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

use sherlock_core::db;
use sherlock_core::models::{SearchItem, SearchRequest, SortField, SortOrder};

use crate::media::MediaError;
use crate::AppState;

// ── Errors ──────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ApiError {
    NotFound,
    /// The row exists but its root has no mount configured in `MEDIA_MAP`.
    UnmappedRoot(String),
    BadRequest(String),
    /// The library pointer exists but the snapshot is unusable.
    Unavailable(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            ApiError::UnmappedRoot(root) => (StatusCode::CONFLICT, root),
            ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            ApiError::Unavailable(message) => (StatusCode::SERVICE_UNAVAILABLE, message),
            ApiError::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        if status.is_server_error() {
            log::error!("{message}");
        }
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

impl From<MediaError> for ApiError {
    fn from(error: MediaError) -> Self {
        // The `Display` impl already says what to fix; the variant only picks the status.
        match error {
            MediaError::RootUnmapped(_) => ApiError::UnmappedRoot(error.to_string()),
            MediaError::Escapes(_) => ApiError::BadRequest(error.to_string()),
            MediaError::Missing(_) => ApiError::Unavailable(error.to_string()),
        }
    }
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(error.to_string())
}

// ── Router ──────────────────────────────────────────────────────────

pub fn app(state: Arc<AppState>) -> Router {
    let mut api = Router::new()
        .route("/health", get(health))
        .route("/library", get(library))
        .route("/roots", get(roots))
        .route("/search", get(search))
        .route("/subdirs", get(subdirs))
        .route("/item/{id}", get(item))
        .route("/thumb/{id}", get(thumb))
        .route("/face-crop/{id}", get(face_crop))
        // A person card needs its representative crop; serving it by person id keeps
        // the stored path of the chosen face out of the payload.
        .route("/person-crop/{id}", get(person_crop))
        .route("/media/{id}", get(media))
        .route("/albums", get(albums))
        .route("/smart-folders", get(smart_folders))
        .route("/people", get(people))
        .route("/faces/{person_id}", get(faces_for_person))
        .route("/duplicates", get(duplicates))
        .with_state(state.clone());

    if let Some(token) = state.config.auth_token.clone() {
        api = api.layer(axum::middleware::from_fn_with_state(token, require_token));
    }

    Router::new()
        .nest("/api", api)
        .route("/", get(index))
        .route("/healthz", get(|| async { "ok" }))
        // Applied to every route, including the nested API and the media responses.
        .layer(axum::middleware::from_fn(security_headers))
}

/// Headers that keep a browser from reinterpreting the bytes or framing the app.
///
/// `nosniff` stops a JPEG served with the wrong extension from being treated as HTML;
/// `frame-ancestors 'self'` allows the SPA to embed its own PDFs in an `<iframe>` while
/// refusing to be embedded by another site. `X-Frame-Options` stays for old browsers
/// that do not implement CSP.
async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("frame-ancestors 'self'"),
    );
    headers.insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("SAMEORIGIN"),
    );
    response
}

/// Placeholder until the SPA is embedded: proving the server is up beats a 404.
async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        "<!doctype html><title>Frank Sherlock</title><p>Library API is up. \
         Try <a href=\"/api/health\">/api/health</a>.</p>",
    )
}

/// Bearer token, or `?token=` for the requests a browser cannot add headers to.
///
/// `<img src>` and `<video src>` cannot send an `Authorization` header, so the query form
/// is what makes the grid work behind nothing but this token. It is a weaker secret — URLs
/// end up in logs — which is why the plan puts an authenticating proxy in front.
async fn require_token(State(token): State<String>, request: Request, next: Next) -> Response {
    let header_ok = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| value == token);
    let query_ok = request
        .uri()
        .query()
        .map(|query| {
            query.split('&').any(|pair| {
                pair.strip_prefix("token=")
                    .map(|value| value == token)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);

    if header_ok || query_ok {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "unauthorized" })),
        )
            .into_response()
    }
}

// ── Handlers ────────────────────────────────────────────────────────

async fn health(State(state): State<Arc<AppState>>) -> Result<Json<Health>, ApiError> {
    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    let mut unmapped = Vec::new();
    for root in &snapshot.pointer.roots {
        if state.config.media_map.mount_for(&root.path).is_none() {
            unmapped.push(root.path.clone());
        }
    }
    Ok(Json(Health {
        revision: snapshot.pointer.revision.clone(),
        schema_version: snapshot.pointer.schema_version,
        generated_at_unix: snapshot.pointer.generated_at_unix,
        files: snapshot.pointer.library.files,
        roots: snapshot.pointer.library.roots,
        hidden_files: snapshot.pointer.library.hidden_files,
        mapped_roots: snapshot.pointer.roots.len() - unmapped.len(),
        unmapped_roots: unmapped,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Health {
    revision: String,
    schema_version: u32,
    generated_at_unix: i64,
    files: u64,
    roots: u64,
    /// Published so an operator can tell "no hidden photos" from "the filter is off".
    hidden_files: u64,
    mapped_roots: usize,
    unmapped_roots: Vec<String>,
}

async fn library(State(state): State<Arc<AppState>>) -> Result<Json<LibraryInfo>, ApiError> {
    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    Ok(Json(LibraryInfo {
        revision: snapshot.pointer.revision.clone(),
        schema_version: snapshot.pointer.schema_version,
        generated_at_unix: snapshot.pointer.generated_at_unix,
        database_file: snapshot.pointer.database.file.clone(),
        database_bytes: snapshot.pointer.database.bytes,
        files: snapshot.pointer.library.files,
        hidden_files: snapshot.pointer.library.hidden_files,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryInfo {
    revision: String,
    schema_version: u32,
    generated_at_unix: i64,
    database_file: String,
    database_bytes: u64,
    files: u64,
    hidden_files: u64,
}

async fn roots(State(state): State<Arc<AppState>>) -> Result<Json<Vec<WebRoot>>, ApiError> {
    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    let roots = snapshot
        .pointer
        .roots
        .iter()
        .map(|root| WebRoot {
            id: root.id,
            name: root.name.clone(),
            path: root.path.clone(),
            root_key: root.root_key.clone(),
            file_count: root.files,
            mapped: state.config.media_map.mount_for(&root.path).is_some(),
        })
        .collect();
    Ok(Json(roots))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebRoot {
    id: i64,
    name: String,
    /// The path as the desktop recorded it — the key of a `MEDIA_MAP` entry.
    path: String,
    root_key: String,
    file_count: u64,
    /// False means the photos of this root cannot be opened here, only listed.
    mapped: bool,
}

/// Query string of `/api/search`.
///
/// `include_hidden` is absent on purpose: it is never read, and a client that sends it is
/// ignored rather than obeyed.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchParams {
    #[serde(default)]
    q: String,
    #[serde(default)]
    root_id: Option<i64>,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default)]
    min_confidence: Option<f32>,
    #[serde(default)]
    date_from: Option<String>,
    #[serde(default)]
    date_to: Option<String>,
    #[serde(default)]
    sort_by: Option<SortField>,
    #[serde(default)]
    sort_order: Option<SortOrder>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    offset: Option<u32>,
}

async fn search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Result<Json<WebSearchResponse>, ApiError> {
    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    let request = SearchRequest {
        query: params.q,
        limit: params.limit,
        offset: params.offset,
        root_scope: params.root_id.into_iter().collect(),
        media_types: params.media_type.into_iter().collect(),
        min_confidence: params.min_confidence,
        date_from: params.date_from,
        date_to: params.date_to,
        sort_by: params.sort_by.unwrap_or_default(),
        sort_order: params.sort_order.unwrap_or_default(),
        include_hidden: false,
    };

    let db_path = snapshot.db_path.clone();
    let response = tokio::task::spawn_blocking(move || db::search_images(&db_path, &request))
        .await
        .map_err(internal)?
        .map_err(internal)?;

    let roots: HashMap<i64, &crate::library::PointerRoot> = snapshot
        .pointer
        .roots
        .iter()
        .map(|root| (root.id, root))
        .collect();

    let items = response
        .items
        .iter()
        .map(|item| WebItem::from_search_item(item, roots.get(&item.root_id).copied()))
        .collect();

    Ok(Json(WebSearchResponse {
        total: response.total,
        limit: response.limit,
        offset: response.offset,
        parsed_query: response.parsed_query,
        items,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebSearchResponse {
    total: u64,
    limit: u32,
    offset: u32,
    parsed_query: sherlock_core::models::ParsedQuery,
    items: Vec<WebItem>,
}

/// A search hit with the paths replaced by URLs.
///
/// `abs_path` and `thumb_path` are deliberately absent: they are paths of the machine that
/// scanned the library, meaningless here and not something to hand to a browser.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebItem {
    id: i64,
    root_id: i64,
    rel_path: String,
    media_type: String,
    description: String,
    confidence: f32,
    mtime_ns: i64,
    size_bytes: i64,
    face_count: Option<i64>,
    thumb_url: String,
    media_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_rel_path: Option<String>,
}

impl WebItem {
    fn from_search_item(item: &SearchItem, _root: Option<&crate::library::PointerRoot>) -> Self {
        Self {
            id: item.id,
            root_id: item.root_id,
            rel_path: item.rel_path.clone(),
            media_type: item.media_type.clone(),
            description: item.description.clone(),
            confidence: item.confidence,
            mtime_ns: item.mtime_ns,
            size_bytes: item.size_bytes,
            face_count: item.face_count,
            thumb_url: format!("/api/thumb/{}", item.id),
            media_url: format!("/api/media/{}", item.id),
            source_format: item.source_format.clone(),
            source_rel_path: item.source_rel_path.clone(),
        }
    }
}

async fn item(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<WebItemDetail>, ApiError> {
    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    let row = blocking_serve_row(&snapshot.db_path, id).await?;
    let root = root_for(&snapshot, &row)?;
    let metadata = blocking(&snapshot.db_path, move |db_path| {
        db::get_file_metadata(db_path, id)
    })
    .await?;

    Ok(Json(WebItemDetail {
        id,
        root_id: root.id,
        root_name: root.name.clone(),
        rel_path: row.rel_path,
        media_type: metadata.media_type,
        description: metadata.description,
        extracted_text: metadata.extracted_text,
        canonical_mentions: metadata.canonical_mentions,
        location_text: metadata.location_text,
        thumb_url: format!("/api/thumb/{id}"),
        media_url: format!("/api/media/{id}"),
    }))
}

/// One row's metadata, with the host paths replaced by URLs.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebItemDetail {
    id: i64,
    root_id: i64,
    root_name: String,
    rel_path: String,
    media_type: String,
    description: String,
    extracted_text: String,
    canonical_mentions: String,
    location_text: String,
    thumb_url: String,
    media_url: String,
}

async fn subdirs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RootPrefix>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let root_id = params
        .root_id
        .ok_or_else(|| ApiError::BadRequest("rootId is required".to_string()))?;
    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    let prefix = params.prefix.unwrap_or_default();
    let entries = blocking(&snapshot.db_path, move |db_path| {
        db::list_subdirectories(db_path, root_id, &prefix)
    })
    .await?;
    Ok(Json(serde_json::to_value(entries).map_err(internal)?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RootPrefix {
    #[serde(default)]
    root_id: Option<i64>,
    #[serde(default)]
    prefix: Option<String>,
}

async fn albums(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    let albums = blocking(&snapshot.db_path, db::list_albums).await?;
    Ok(Json(serde_json::to_value(albums).map_err(internal)?))
}

async fn smart_folders(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    let folders = blocking(&snapshot.db_path, db::list_smart_folders).await?;
    Ok(Json(serde_json::to_value(folders).map_err(internal)?))
}

async fn people(State(state): State<Arc<AppState>>) -> Result<Json<Vec<WebPerson>>, ApiError> {
    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    let people = blocking(&snapshot.db_path, |db_path| db::list_persons(db_path, &[])).await?;
    Ok(Json(
        people.into_iter().map(WebPerson::from_person).collect(),
    ))
}

/// A person with its crop replaced by the id-based URL.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebPerson {
    id: i64,
    name: String,
    face_count: u64,
    /// Null when no visible face of this person has a crop yet.
    crop_url: Option<String>,
}

impl WebPerson {
    fn from_person(person: sherlock_core::models::PersonInfo) -> Self {
        // Only the *presence* of a crop leaves the database: the URL is built from the
        // person id, so the stored path of the representative face never travels.
        let crop_url = person
            .crop_path
            .as_ref()
            .map(|_| format!("/api/person-crop/{}", person.id));
        Self {
            id: person.id,
            name: person.name,
            face_count: person.face_count,
            crop_url,
        }
    }
}

async fn faces_for_person(
    State(state): State<Arc<AppState>>,
    Path(person_id): Path<i64>,
) -> Result<Json<Vec<WebFace>>, ApiError> {
    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    let faces = blocking(&snapshot.db_path, move |db_path| {
        db::list_faces_for_person(db_path, person_id)
    })
    .await?;
    Ok(Json(faces.into_iter().map(WebFace::from_face).collect()))
}

/// One face of a person, with its crop replaced by the id-based URL.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebFace {
    id: i64,
    person_id: Option<i64>,
    file_id: i64,
    rel_path: String,
    filename: String,
    confidence: f32,
    crop_url: Option<String>,
}

impl WebFace {
    fn from_face(face: sherlock_core::models::FaceInfo) -> Self {
        let crop_url = face
            .crop_path
            .as_ref()
            .map(|_| format!("/api/face-crop/{}", face.id));
        Self {
            id: face.id,
            person_id: face.person_id,
            file_id: face.file_id,
            rel_path: face.rel_path,
            filename: face.filename,
            confidence: face.confidence,
            crop_url,
        }
    }
}

async fn duplicates(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DuplicatesParams>,
) -> Result<Json<WebDuplicatesResponse>, ApiError> {
    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    let scope: Vec<i64> = params.root_id.into_iter().collect();
    let response = blocking(&snapshot.db_path, move |db_path| {
        db::find_duplicates(db_path, &scope, None)
    })
    .await?;
    Ok(Json(WebDuplicatesResponse::from_response(response)))
}

/// Duplicate groups are informative on the web: no host path, just URLs.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebDuplicatesResponse {
    total_groups: u64,
    total_duplicate_files: u64,
    total_wasted_bytes: i64,
    groups: Vec<WebDuplicateGroup>,
}

impl WebDuplicatesResponse {
    fn from_response(response: sherlock_core::models::DuplicatesResponse) -> Self {
        Self {
            total_groups: response.total_groups,
            total_duplicate_files: response.total_duplicate_files,
            total_wasted_bytes: response.total_wasted_bytes,
            groups: response
                .groups
                .into_iter()
                .map(WebDuplicateGroup::from_group)
                .collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebDuplicateGroup {
    fingerprint: String,
    file_count: u64,
    total_size_bytes: i64,
    wasted_bytes: i64,
    files: Vec<WebDuplicateFile>,
    group_type: String,
    avg_similarity: Option<f32>,
}

impl WebDuplicateGroup {
    fn from_group(group: sherlock_core::models::DuplicateGroup) -> Self {
        Self {
            fingerprint: group.fingerprint,
            file_count: group.file_count,
            total_size_bytes: group.total_size_bytes,
            wasted_bytes: group.wasted_bytes,
            files: group
                .files
                .into_iter()
                .map(WebDuplicateFile::from_file)
                .collect(),
            group_type: group.group_type,
            avg_similarity: group.avg_similarity,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebDuplicateFile {
    id: i64,
    root_id: i64,
    rel_path: String,
    media_type: String,
    description: String,
    confidence: f32,
    mtime_ns: i64,
    size_bytes: i64,
    is_keeper: bool,
    similarity_score: Option<f32>,
    group_type: String,
    thumb_url: String,
    media_url: String,
}

impl WebDuplicateFile {
    fn from_file(file: sherlock_core::models::DuplicateFile) -> Self {
        Self {
            thumb_url: format!("/api/thumb/{}", file.id),
            media_url: format!("/api/media/{}", file.id),
            id: file.id,
            root_id: file.root_id,
            rel_path: file.rel_path,
            media_type: file.media_type,
            description: file.description,
            confidence: file.confidence,
            mtime_ns: file.mtime_ns,
            size_bytes: file.size_bytes,
            is_keeper: file.is_keeper,
            similarity_score: file.similarity_score,
            group_type: file.group_type,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DuplicatesParams {
    #[serde(default)]
    root_id: Option<i64>,
}

// ── Media ───────────────────────────────────────────────────────────

async fn thumb(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    let row = blocking_serve_row(&snapshot.db_path, id).await?;
    let root = root_for(&snapshot, &row)?;
    let key = sherlock_core::thumbnail::root_key(&root.path);
    let path =
        sherlock_core::thumbnail::thumb_path_for(&snapshot.thumbnails_dir, &key, &row.rel_path);
    serve_jpeg_file(&path, &headers).await
}

async fn face_crop(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    let crops_dir = snapshot.face_crops_dir.clone().ok_or(ApiError::NotFound)?;
    let (crop_path, hidden) = blocking(&snapshot.db_path, move |db_path| {
        db::face_crop_for_serving(db_path, id)
    })
    .await?
    .ok_or(ApiError::NotFound)?;
    if hidden {
        // A crop is a picture of a hidden photo; the same rule applies.
        return Err(ApiError::NotFound);
    }
    let file_name = crop_path
        .as_deref()
        .and_then(|path| std::path::Path::new(path).file_name())
        .and_then(|name| name.to_str())
        .ok_or(ApiError::NotFound)?;
    serve_jpeg_file(&crops_dir.join(file_name), &headers).await
}

/// A person card's crop: the crop of one of its visible faces, by person id.
///
/// `list_faces_for_person` already excludes files that were deleted or hidden, so a
/// person whose every photo is hidden answers `404` — the same rule the crops follow.
async fn person_crop(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    let crops_dir = snapshot.face_crops_dir.clone().ok_or(ApiError::NotFound)?;
    let faces = blocking(&snapshot.db_path, move |db_path| {
        db::list_faces_for_person(db_path, id)
    })
    .await?;
    let file_name = faces
        .iter()
        .filter_map(|face| face.crop_path.as_deref())
        .filter_map(|path| std::path::Path::new(path).file_name())
        .filter_map(|name| name.to_str())
        .next()
        .ok_or(ApiError::NotFound)?;
    serve_jpeg_file(&crops_dir.join(file_name), &headers).await
}

/// Query string of `/api/media`.
#[derive(Debug, Deserialize)]
struct MediaParams {
    /// Width of the preview derivative. Absent, zero or garbage means the original.
    #[serde(default)]
    w: Option<String>,
}

async fn media(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Query(params): Query<MediaParams>,
    request: Request,
) -> Result<Response, ApiError> {
    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    let row = blocking_serve_row(&snapshot.db_path, id).await?;
    let root = root_for(&snapshot, &row)?;
    let path = state.config.media_map.resolve(&root.path, &row.rel_path)?;
    if !path.is_file() {
        return Err(ApiError::Unavailable(format!(
            "{} is not available on this host (share unmounted?)",
            row.rel_path
        )));
    }

    if let Some(width) = crate::preview::requested_width(params.w.as_deref()) {
        if crate::preview::is_resizable(&row.rel_path) {
            let target = crate::preview::cache_path(
                &state.config.cache_dir,
                &snapshot.pointer.revision,
                id,
                width,
            );
            if !target.is_file() {
                let source = path.clone();
                let destination = target.clone();
                let rendered = tokio::task::spawn_blocking(move || {
                    crate::preview::render(&source, &destination, width)
                })
                .await
                .map_err(internal)?;
                if let Err(error) = rendered {
                    // An exotic or corrupt container still has a usable original:
                    // degrading to it is better than failing the whole request.
                    log::warn!(
                        "could not build a {width}px preview for {}: {error}",
                        row.rel_path
                    );
                }
            }
            if target.is_file() {
                return serve_jpeg_file(&target, request.headers()).await;
            }
        }
    }

    // `ServeFile` is what makes seeking work: it answers `Range` and `HEAD` correctly,
    // which a `<video>` element needs and a hand-rolled `fs::read` would not.
    match tower_http::services::ServeFile::new(&path)
        .oneshot(request)
        .await
    {
        Ok(response) => Ok(response.map(Body::new)),
        Err(error) => Err(internal(error)),
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

async fn blocking<T, F>(db_path: &std::path::Path, work: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&std::path::Path) -> sherlock_core::error::AppResult<T> + Send + 'static,
{
    let db_path = db_path.to_path_buf();
    tokio::task::spawn_blocking(move || work(&db_path))
        .await
        .map_err(internal)?
        .map_err(internal)
}

/// Look up a row and turn "missing" and "hidden" into the same `404`.
async fn blocking_serve_row(db_path: &std::path::Path, id: i64) -> Result<db::ServeRow, ApiError> {
    let row = blocking(db_path, move |path| db::serve_row(path, id)).await?;
    match row {
        Some(row) if !row.hidden => Ok(row),
        // Indistinguishable on purpose: telling them apart would confirm that a hidden
        // photo exists, which is exactly what hiding it is for.
        _ => Err(ApiError::NotFound),
    }
}

/// The pointer entry for a row's root.
///
/// `serve_row` carries the stored root path, and the pointer lists the same path; a row
/// whose root is missing from the pointer means the pointer and the snapshot disagree, and
/// answering `404` is better than guessing at a mount.
fn root_for<'a>(
    snapshot: &'a crate::library::Snapshot,
    row: &db::ServeRow,
) -> Result<&'a crate::library::PointerRoot, ApiError> {
    snapshot
        .pointer
        .roots
        .iter()
        .find(|root| root.path == row.root_path)
        .ok_or(ApiError::NotFound)
}

/// A long cache is safe because a published revision is immutable; the `ETag` is what
/// lets a client that *does* revalidate skip the body.
const CACHE_CONTROL_VALUE: &str = "public, max-age=604800";

/// A thumbnail, a crop or a `?w=` derivative: JPEG, immutable within one revision.
///
/// `ETag` comes from the file's size and mtime: the published tree is replaced wholesale
/// when a new revision is published, and a repaired thumbnail gets a new mtime. A client
/// sending `If-None-Match` gets `304` and downloads no bytes at all.
async fn serve_jpeg_file(
    path: &std::path::Path,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|_| ApiError::NotFound)?;
    let etag = etag_for(&metadata);
    let etag_header = HeaderValue::from_str(&etag).map_err(internal)?;
    if matches_etag(headers, &etag) {
        return Ok((
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag_header),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static(CACHE_CONTROL_VALUE),
                ),
            ],
        )
            .into_response());
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| ApiError::NotFound)?;
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg")),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static(CACHE_CONTROL_VALUE),
            ),
            (header::ETAG, etag_header),
        ],
        bytes,
    )
        .into_response())
}

fn etag_for(metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!("\"{:x}-{:x}\"", metadata.len(), modified)
}

fn matches_etag(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .any(|candidate| candidate == "*" || candidate == etag)
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn include_hidden_never_reaches_the_request() {
        // A client sending it must not be obeyed: the field is not part of SearchParams.
        let params: SearchParams =
            serde_json::from_str(r#"{"q":"dog","includeHidden":true}"#).expect("params");
        assert_eq!(params.q, "dog");
    }

    #[test]
    fn search_params_accept_camel_case_sorting() {
        let params: SearchParams =
            serde_json::from_str(r#"{"sortBy":"name","sortOrder":"asc"}"#).expect("params");
        assert_eq!(params.sort_by, Some(SortField::Name));
        assert_eq!(params.sort_order, Some(SortOrder::Asc));
    }

    #[test]
    fn a_media_error_becomes_conflict_or_bad_request() {
        let unmapped = ApiError::from(MediaError::RootUnmapped("/a".to_string()));
        assert!(matches!(unmapped, ApiError::UnmappedRoot(_)));
        let escape = ApiError::from(MediaError::Escapes("../x".to_string()));
        assert!(matches!(escape, ApiError::BadRequest(_)));
    }

    #[test]
    fn paths_are_built_from_ids_not_from_the_client() {
        let item = SearchItem {
            id: 7,
            root_id: 2,
            rel_path: "a/b.jpg".to_string(),
            abs_path: "/stored/a/b.jpg".to_string(),
            media_type: "photo".to_string(),
            description: String::new(),
            confidence: 0.5,
            mtime_ns: 0,
            size_bytes: 1,
            thumbnail_path: Some("/cache/thumbs/x.jpg".to_string()),
            face_count: None,
            hidden_at: None,
            source_format: None,
            source_rel_path: None,
        };
        let web = WebItem::from_search_item(&item, None);
        assert_eq!(web.thumb_url, "/api/thumb/7");
        assert_eq!(web.media_url, "/api/media/7");
        let json = serde_json::to_value(&web).expect("json");
        assert!(json.get("absPath").is_none(), "host paths must not leak");
        assert!(json.get("thumbnailPath").is_none());
    }

    #[test]
    fn duplicate_files_are_served_by_id() {
        let file = sherlock_core::models::DuplicateFile {
            id: 11,
            root_id: 2,
            rel_path: "a/b.jpg".to_string(),
            abs_path: "/stored/a/b.jpg".to_string(),
            root_path: "/stored".to_string(),
            media_type: "photo".to_string(),
            description: "duplicate".to_string(),
            confidence: 0.5,
            mtime_ns: 0,
            size_bytes: 1,
            thumbnail_path: Some("/cache/thumbs/x.jpg".to_string()),
            is_keeper: true,
            similarity_score: None,
            group_type: "exact".to_string(),
        };
        let json = serde_json::to_value(WebDuplicateFile::from_file(file)).expect("json");
        assert_eq!(json["thumbUrl"], "/api/thumb/11");
        assert_eq!(json["mediaUrl"], "/api/media/11");
        assert!(json.get("absPath").is_none());
        assert!(json.get("rootPath").is_none());
        assert!(json.get("thumbnailPath").is_none());
    }

    #[test]
    fn a_person_crop_url_is_built_from_the_person_id() {
        let person = sherlock_core::models::PersonInfo {
            id: 9,
            name: "Ana".to_string(),
            face_count: 3,
            crop_path: Some("/cache/face_crops/42.jpg".to_string()),
            thumbnail_path: None,
        };
        let json = serde_json::to_value(WebPerson::from_person(person)).expect("json");
        assert_eq!(json["cropUrl"], "/api/person-crop/9");
        assert!(json.get("cropPath").is_none(), "host paths must not leak");
    }

    #[test]
    fn a_face_crop_url_is_built_from_the_face_id() {
        let face = sherlock_core::models::FaceInfo {
            id: 5,
            person_id: Some(9),
            file_id: 7,
            rel_path: "a/b.jpg".to_string(),
            filename: "b.jpg".to_string(),
            confidence: 0.9,
            crop_path: Some("/cache/face_crops/5.jpg".to_string()),
        };
        let json = serde_json::to_value(WebFace::from_face(face)).expect("json");
        assert_eq!(json["cropUrl"], "/api/face-crop/5");
        assert!(json.get("cropPath").is_none(), "host paths must not leak");
    }
}
