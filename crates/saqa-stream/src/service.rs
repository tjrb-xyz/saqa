//! What saqa's REST API needs to be served on its own, copied from dsper's
//! `dsper-service` (dsper serves `/stream/v1` behind exactly these rules):
//!
//! - every route needs `Authorization: Bearer <token>` except `/health`.
//!   No cookies: no CSRF;
//! - a `Host` not in the allowlist gets 421 (defeats DNS rebinding);
//! - a request with a foreign `Origin` gets 403; allowed extension origins
//!   get CORS headers;
//! - errors are `{"error": "…"}` with a status that says whose fault it was.
//!
//! saqad serves plain HTTP on loopback only; dsper's TLS for other
//! addresses did not move (see docs/API.md).

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;
use std::sync::Arc;

/// Who may call, from where.
#[derive(Debug, Clone)]
pub struct Access {
    pub token: String,
    /// `Host` header values accepted, e.g. `127.0.0.1:8480`.
    pub hosts: Vec<String>,
    /// Browser origins allowed besides the service's own (extensions).
    pub origins: Vec<String>,
    /// Path prefixes the token guards (one per mounted service).
    pub protected: Vec<String>,
    /// Paths inside those prefixes that are open anyway.
    pub open: Vec<String>,
}

impl Access {
    /// Loopback on `port`, guarding `protected` prefixes.
    pub fn loopback(token: impl Into<String>, port: u16, protected: &[&str]) -> Self {
        Self {
            token: token.into(),
            hosts: vec![format!("127.0.0.1:{port}"), format!("localhost:{port}")],
            origins: vec![],
            protected: protected.iter().map(|p| p.to_string()).collect(),
            open: protected.iter().map(|p| format!("{p}health")).collect(),
        }
    }

    pub fn host_ok(&self, headers: &HeaderMap) -> bool {
        headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|h| self.hosts.iter().any(|a| a == h))
    }

    pub fn origin_ok(&self, headers: &HeaderMap) -> bool {
        match headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) {
            None => true,
            Some(o) => {
                self.origins.iter().any(|a| a == o)
                    || self.hosts.iter().any(|h| {
                        o.strip_prefix("http://")
                            .or_else(|| o.strip_prefix("https://"))
                            == Some(h.as_str())
                    })
            }
        }
    }

    pub fn token_ok(&self, headers: &HeaderMap) -> bool {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|t| eq_ct(t, &self.token))
    }

    fn cors(&self, headers: &HeaderMap, resp: &mut Response) {
        let Some(o) = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) else {
            return;
        };
        if !self.origins.iter().any(|a| a == o) {
            return;
        }
        let h = resp.headers_mut();
        h.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_str(o).expect("origin is ascii"),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("authorization, content-type"),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE"),
        );
        h.insert(header::VARY, HeaderValue::from_static("origin"));
    }
}

/// Constant-time comparison, so the token cannot be guessed byte by byte.
pub fn eq_ct(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

pub fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({"error": msg.into()}))).into_response()
}

async fn guard(State(a): State<Arc<Access>>, req: Request, next: Next) -> Response {
    let headers = req.headers().clone();
    if !a.host_ok(&headers) {
        return err(StatusCode::MISDIRECTED_REQUEST, "unknown Host");
    }
    if !a.origin_ok(&headers) {
        return err(StatusCode::FORBIDDEN, "Origin not allowed");
    }
    let path = req.uri().path();
    let guarded = a.protected.iter().any(|p| path.starts_with(p.as_str()))
        && !a.open.iter().any(|o| o == path);
    let mut resp = if req.method() == Method::OPTIONS {
        StatusCode::NO_CONTENT.into_response()
    } else if !guarded || a.token_ok(&headers) {
        next.run(req).await
    } else {
        err(StatusCode::UNAUTHORIZED, "missing or wrong token")
    };
    a.cors(&headers, &mut resp);
    resp
}

/// Wraps a router (one service or several mounted together) in the guard.
pub fn guarded(router: Router, access: Arc<Access>) -> Router {
    router.layer(middleware::from_fn_with_state(access, guard))
}

/// `GET <prefix>health`: open, says which service and API version answer.
pub fn health(service: &'static str, api: u32) -> Router {
    Router::new().route(
        "/health",
        get(move || async move { Json(json!({"ok": true, "service": service, "api": api})) }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;

    fn app() -> Router {
        let access = Arc::new(Access::loopback("0123456789abcdef", 9000, &["/x/v1/"]));
        let x = Router::new()
            .route("/secret", get(|| async { "ok" }))
            .merge(health("x", 1));
        guarded(
            Router::new()
                .nest("/x/v1", x)
                .route("/ui", get(|| async { "page" })),
            access,
        )
    }

    async fn status(path: &str, host: &str, token: Option<&str>, origin: Option<&str>) -> u16 {
        let mut b = axum::http::Request::builder()
            .uri(path)
            .header(header::HOST, host);
        if let Some(t) = token {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        app()
            .oneshot(b.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    #[tokio::test]
    async fn one_set_of_rules_for_every_service() {
        let h = "127.0.0.1:9000";
        assert_eq!(
            status("/x/v1/health", h, None, None).await,
            200,
            "health is open"
        );
        assert_eq!(status("/x/v1/secret", h, None, None).await, 401);
        assert_eq!(
            status("/x/v1/secret", h, Some("wrong-wrong-wrong"), None).await,
            401
        );
        assert_eq!(
            status("/x/v1/secret", h, Some("0123456789abcdef"), None).await,
            200
        );
        assert_eq!(
            status("/ui", h, None, None).await,
            200,
            "outside every service prefix"
        );
        assert_eq!(
            status(
                "/x/v1/secret",
                "evil.example:9000",
                Some("0123456789abcdef"),
                None
            )
            .await,
            421
        );
        assert_eq!(
            status("/x/v1/health", h, None, Some("https://evil.example")).await,
            403
        );
        assert_eq!(
            status("/x/v1/health", h, None, Some("http://127.0.0.1:9000")).await,
            200
        );
    }

    #[test]
    fn tokens_compare_in_constant_time_and_exactly() {
        assert!(eq_ct("abc", "abc"));
        assert!(!eq_ct("abc", "abd") && !eq_ct("abc", "ab"));
    }
}
