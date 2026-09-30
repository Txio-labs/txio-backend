//! The browser session end to end at the extractor level: cookie or bearer
//! token, CSRF header on writes, and a revoked session ending the token. Runs
//! only with `TXIO_TEST_MONGO_URI` set (see workspace_collaboration.rs).

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    routing::{get, post},
    Extension, Json, Router,
};
use mongodb::{bson::oid::ObjectId, Client};
use serde_json::{json, Value};
use tower::ServiceExt;
use txio_api::model::session::Session;
use txio_api::repositories::session_repository::SessionRepository;
use txio_api::utils::auth_jwt::{Claims, JwtHelper};
use txio_api::utils::session_cookie::{session_cookie, SESSION_COOKIE};

async fn whoami(claims: Claims) -> Json<Value> {
    Json(json!({ "email": claims.email }))
}

async fn write(claims: Claims) -> Json<Value> {
    Json(json!({ "wrote_as": claims.email }))
}

struct Setup {
    app: Router,
    sessions: SessionRepository,
    token: String,
    jti: String,
    db: mongodb::Database,
}

async fn setup() -> Option<Setup> {
    let uri = std::env::var("TXIO_TEST_MONGO_URI").ok()?;
    let client = Client::with_uri_str(&uri).await.unwrap();
    let db = client.database(&format!("txio_it_{}", ObjectId::new().to_hex()));
    let sessions = SessionRepository::new(&db);
    sessions.ensure_indexes().await.unwrap();

    let jwt = JwtHelper::new("a-test-secret-that-is-at-least-32-characters".into());
    let user = ObjectId::new();
    let (token, jti) = jwt.generate_token(&user.to_hex(), "ada@example.com").unwrap();
    sessions
        .save(&Session::new(user, jti.clone(), "Chrome on Linux".into(), "127.0.0.1".into()))
        .await
        .unwrap();

    let app = Router::new()
        .route("/me", get(whoami))
        .route("/write", post(write))
        .layer(Extension(jwt))
        .layer(Extension(sessions.clone()));
    Some(Setup { app, sessions, token, jti, db })
}

fn request(method: Method, path: &str) -> axum::http::request::Builder {
    Request::builder().method(method).uri(path)
}

async fn status(app: &Router, req: Request<Body>) -> StatusCode {
    app.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn cookie_bearer_csrf_and_revocation() {
    let Some(s) = setup().await else {
        eprintln!("TXIO_TEST_MONGO_URI not set; skipping");
        return;
    };
    let cookie = format!("{SESSION_COOKIE}={}", s.token);

    // Both credentials work for reads; nothing at all does not.
    assert_eq!(status(&s.app, request(Method::GET, "/me").header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await, StatusCode::OK);
    assert_eq!(
        status(&s.app, request(Method::GET, "/me").header(header::AUTHORIZATION, format!("Bearer {}", s.token)).body(Body::empty()).unwrap()).await,
        StatusCode::OK
    );
    assert_eq!(status(&s.app, request(Method::GET, "/me").body(Body::empty()).unwrap()).await, StatusCode::UNAUTHORIZED);

    // A cookie-authenticated write needs the CSRF header (a cross-site form cannot set it).
    assert_eq!(status(&s.app, request(Method::POST, "/write").header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await, StatusCode::FORBIDDEN);
    assert_eq!(
        status(&s.app, request(Method::POST, "/write").header(header::COOKIE, &cookie).header("x-requested-with", "txio").body(Body::empty()).unwrap()).await,
        StatusCode::OK
    );
    // Bearer callers (CLI, SDK) are not browsers and need no CSRF header.
    assert_eq!(
        status(&s.app, request(Method::POST, "/write").header(header::AUTHORIZATION, format!("Bearer {}", s.token)).body(Body::empty()).unwrap()).await,
        StatusCode::OK
    );

    // Garbage is rejected, not treated as anonymous success.
    assert_eq!(status(&s.app, request(Method::GET, "/me").header(header::COOKIE, format!("{SESSION_COOKIE}=nope")).body(Body::empty()).unwrap()).await, StatusCode::UNAUTHORIZED);

    // Logout / revoke deletes the session row: the same, still-unexpired JWT stops working everywhere.
    s.sessions.delete_by_jti(&s.jti).await.unwrap();
    assert_eq!(status(&s.app, request(Method::GET, "/me").header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        status(&s.app, request(Method::GET, "/me").header(header::AUTHORIZATION, format!("Bearer {}", s.token)).body(Body::empty()).unwrap()).await,
        StatusCode::UNAUTHORIZED
    );

    s.db.drop(None).await.unwrap();
}

#[test]
fn the_cookie_the_server_sets_cannot_be_read_by_script() {
    std::env::set_var("COOKIE_SECURE", "true");
    let c = session_cookie("t");
    assert!(c.contains("HttpOnly"), "{c}");
    assert!(c.contains("Secure"), "{c}");
    assert!(c.contains("SameSite=Lax"), "{c}");
    assert!(c.contains("Path=/"), "{c}");
}
