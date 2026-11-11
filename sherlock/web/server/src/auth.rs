//! Authentication for the read-only web app.
//!
//! Two principals are accepted:
//!
//! 1. the optional `AUTH_TOKEN`, as a bearer header or `?token=` (scripts, media URLs
//!    and the transition period before the first user is published);
//! 2. a user session created by `POST /api/auth/login`, carried in an HttpOnly cookie.
//!
//! Sessions live in memory because the container has a read-only root filesystem and
//! no writable database. A restart logs everyone out, which is a fair trade for a
//! viewer. The session is revalidated against the current published revision: deleting
//! a user or changing a password invalidates the old session on the next publish.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Extension, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use sherlock_core::auth as passwords;
use sherlock_core::db;
use sherlock_core::models::UserCredential;

use crate::api::ApiError;
use crate::library::Snapshot;
use crate::AppState;

pub const SESSION_COOKIE: &str = "fs_session";
const SESSION_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Debug, Clone)]
pub struct SessionUser {
    pub id: i64,
    pub name: String,
    pub login: String,
}

/// What `require_auth` puts in the request extensions.
#[derive(Debug, Clone)]
pub enum Principal {
    ApiToken,
    User(SessionUser),
}

#[derive(Debug, Clone)]
struct Session {
    user: SessionUser,
    /// The hash the session was created with. A later publish that changes it must
    /// invalidate this session, not leave a password change ineffective until expiry.
    password_hash: String,
    expires_at: Instant,
}

#[derive(Debug)]
struct UserDirectory {
    by_id: HashMap<i64, UserCredential>,
    by_login: HashMap<String, UserCredential>,
}

impl UserDirectory {
    fn new(users: Vec<UserCredential>) -> Self {
        let mut by_id = HashMap::with_capacity(users.len());
        let mut by_login = HashMap::with_capacity(users.len());
        for user in users {
            // SQLite's NOCASE is ASCII-only; match that exactly so the directory and
            // the unique index cannot disagree about two unusual Unicode spellings.
            by_login.insert(user.login.to_ascii_lowercase(), user.clone());
            by_id.insert(user.id, user);
        }
        Self { by_id, by_login }
    }

    fn get(&self, id: i64) -> Option<&UserCredential> {
        self.by_id.get(&id)
    }

    fn find_login(&self, login: &str) -> Option<&UserCredential> {
        self.by_login.get(&login.to_ascii_lowercase())
    }

    fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

#[derive(Debug)]
struct CachedDirectory {
    revision: String,
    users: Arc<UserDirectory>,
}

/// Sessions plus the users of the revision they were verified against.
#[derive(Debug, Default)]
pub struct AuthState {
    sessions: Mutex<HashMap<String, Session>>,
    directory: Mutex<Option<CachedDirectory>>,
}

impl AuthState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load the users of `snapshot`, reusing the cached copy while the revision is the
    /// same. One database read per publish, not one per media request.
    fn directory_for(&self, snapshot: &Snapshot) -> Result<Arc<UserDirectory>, String> {
        let revision = snapshot.pointer.revision.clone();
        if let Ok(guard) = self.directory.lock() {
            if let Some(cached) = guard.as_ref() {
                if cached.revision == revision {
                    return Ok(Arc::clone(&cached.users));
                }
            }
        }

        let users = db::list_user_credentials(&snapshot.db_path).map_err(|error| {
            format!(
                "the published revision has no readable users table ({error}); publish a \
                 revision from an updated desktop build"
            )
        })?;
        let directory = Arc::new(UserDirectory::new(users));
        if let Ok(mut guard) = self.directory.lock() {
            *guard = Some(CachedDirectory {
                revision,
                users: Arc::clone(&directory),
            });
        }
        Ok(directory)
    }

    pub fn create_session(&self, user: &UserCredential) -> String {
        let id = Uuid::new_v4().to_string();
        let now = Instant::now();
        let session = Session {
            user: SessionUser {
                id: user.id,
                name: user.name.clone(),
                login: user.login.clone(),
            },
            password_hash: user.password_hash.clone(),
            expires_at: now + SESSION_TTL,
        };
        if let Ok(mut sessions) = self.sessions.lock() {
            // Expired entries are cheap to sweep here and nowhere else needs the work.
            sessions.retain(|_, session| session.expires_at > now);
            sessions.insert(id.clone(), session);
        }
        id
    }

    pub fn remove_session(&self, id: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(id);
        }
    }

    fn session(&self, id: &str) -> Option<Session> {
        let mut sessions = self.sessions.lock().ok()?;
        match sessions.get(id) {
            Some(session) if session.expires_at > Instant::now() => Some(session.clone()),
            Some(_) => {
                sessions.remove(id);
                None
            }
            None => None,
        }
    }

    /// A live session whose user still exists with the same password hash.
    pub fn authenticate(
        &self,
        id: &str,
        snapshot: &Snapshot,
    ) -> Result<Option<SessionUser>, String> {
        let Some(session) = self.session(id) else {
            return Ok(None);
        };
        let directory = self.directory_for(snapshot)?;
        let Some(user) = directory.get(session.user.id) else {
            self.remove_session(id);
            return Ok(None);
        };
        if user.password_hash != session.password_hash {
            self.remove_session(id);
            return Ok(None);
        }
        Ok(Some(session.user))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub login: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeResponse {
    pub kind: &'static str,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
}

impl MeResponse {
    fn user(user: &SessionUser) -> Self {
        Self {
            kind: "user",
            name: user.name.clone(),
            id: Some(user.id),
            login: Some(user.login.clone()),
        }
    }

    fn from_principal(principal: &Principal) -> Self {
        match principal {
            Principal::ApiToken => Self {
                kind: "token",
                name: "Access token".to_string(),
                id: None,
                login: None,
            },
            Principal::User(user) => Self::user(user),
        }
    }
}

/// `POST /api/auth/login`.
///
/// The response is deliberately generic on failure: an unknown login and a wrong
/// password get the same `401`, so the form cannot be used to enumerate accounts. The
/// unknown path still spends one Argon2 verification against a dummy hash.
pub async fn login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let login = body.login.trim();
    if login.is_empty() || body.password.is_empty() {
        return Err(ApiError::Unauthorized("invalid credentials".to_string()));
    }

    let snapshot = state.library.snapshot().map_err(ApiError::Unavailable)?;
    let directory = state
        .auth
        .directory_for(&snapshot)
        .map_err(ApiError::Unavailable)?;
    if directory.is_empty() {
        return Err(ApiError::Unavailable(
            "no user accounts are published yet; create one in the desktop app (Web users) \
             and publish the library"
                .to_string(),
        ));
    }

    let Some(user) = directory.find_login(login) else {
        verify_dummy(&body.password);
        return Err(ApiError::Unauthorized("invalid credentials".to_string()));
    };
    if !passwords::verify_password(&body.password, &user.password_hash) {
        return Err(ApiError::Unauthorized("invalid credentials".to_string()));
    }

    let session_user = SessionUser {
        id: user.id,
        name: user.name.clone(),
        login: user.login.clone(),
    };
    let session_id = state.auth.create_session(user);
    let mut response = Json(MeResponse::user(&session_user)).into_response();
    let secure = cookie_is_secure(&state, &headers);
    response.headers_mut().append(
        header::SET_COOKIE,
        header_value(&session_cookie(&session_id, secure))?,
    );
    Ok(response)
}

/// `POST /api/auth/logout`. Public on purpose: a stale cookie must still be clearable.
pub async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(id) = session_cookie_value(&headers) {
        state.auth.remove_session(&id);
    }
    let secure = cookie_is_secure(&state, &headers);
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Ok(value) = HeaderValue::from_str(&clear_session_cookie(secure)) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

/// `GET /api/auth/me`, protected by [`require_auth`].
pub async fn me(Extension(principal): Extension<Principal>) -> Json<MeResponse> {
    Json(MeResponse::from_principal(&principal))
}

/// Accept a token or a live session; insert the principal for `/api/auth/me`.
pub async fn require_auth(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    if let Some(token) = state.config.auth_token.as_deref() {
        if token_matches(&request, token) {
            request.extensions_mut().insert(Principal::ApiToken);
            return next.run(request).await;
        }
    }

    if let Some(id) = session_cookie_value(request.headers()) {
        let snapshot = match state.library.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => return ApiError::Unavailable(error).into_response(),
        };
        match state.auth.authenticate(&id, &snapshot) {
            Ok(Some(user)) => {
                request.extensions_mut().insert(Principal::User(user));
                return next.run(request).await;
            }
            Ok(None) => {}
            Err(error) => return ApiError::Unavailable(error).into_response(),
        }
    }

    ApiError::Unauthorized("unauthorized".to_string()).into_response()
}

/// Bearer token, or `?token=` for requests a browser cannot add headers to.
fn token_matches(request: &Request, token: &str) -> bool {
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
    header_ok || query_ok
}

pub fn session_cookie_value(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        if name == SESSION_COOKIE {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

fn session_cookie(id: &str, secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}={id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{}",
        SESSION_TTL.as_secs(),
        if secure { "; Secure" } else { "" }
    )
}

fn clear_session_cookie(secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0; \
         Expires=Thu, 01 Jan 1970 00:00:00 GMT{}",
        if secure { "; Secure" } else { "" }
    )
}

fn cookie_is_secure(state: &AppState, headers: &HeaderMap) -> bool {
    state.config.cookie_secure
        || headers
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok())
            .map(|value| {
                value
                    .split(',')
                    .any(|part| part.trim().eq_ignore_ascii_case("https"))
            })
            .unwrap_or(false)
}

fn header_value(raw: &str) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(raw).map_err(|error| ApiError::Internal(error.to_string()))
}

fn verify_dummy(password: &str) {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let hash = DUMMY
        .get_or_init(|| passwords::hash_password("dummy-password-for-timing").unwrap_or_default());
    if !hash.is_empty() {
        let _ = passwords::verify_password(password, hash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_session_cookie_among_others() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("theme=dark; fs_session=abc-123; other=1"),
        );
        assert_eq!(session_cookie_value(&headers).as_deref(), Some("abc-123"));
    }

    #[test]
    fn ignores_an_unrelated_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_static("theme=dark"));
        assert!(session_cookie_value(&headers).is_none());
    }

    #[test]
    fn cookie_flags_are_explicit() {
        let value = session_cookie("id", true);
        assert!(value.contains("HttpOnly"));
        assert!(value.contains("SameSite=Strict"));
        assert!(value.contains("Secure"));
        assert!(clear_session_cookie(false).contains("Max-Age=0"));
    }
}
