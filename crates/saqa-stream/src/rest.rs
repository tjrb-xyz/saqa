//! The stream service's REST API, mounted at `/stream/v1`.
//!
//! | Route | Body | Reply |
//! |---|---|---|
//! | `GET /state` | | `{available, detail, links}`: whether libroc is here, and every link |
//! | `GET /links` | | `[LinkView]` |
//! | `PUT /links/{id}` | `LinkSpec` | `LinkView`; 400 invalid, 422 refused (a receive link not into an allowed input), 503 no libroc |
//! | `DELETE /links/{id}` | | `null`, 404 when unknown |

use crate::service::{err, health};
use crate::{LinkSpec, Refused, StreamService};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

type S = State<Arc<StreamService>>;

async fn state(State(s): S) -> Json<Value> {
    let available = StreamService::available();
    Json(json!({
        "available": available.is_ok(),
        "detail": available.err(),
        "links": s.links(),
    }))
}

async fn links(State(s): S) -> Json<Value> {
    Json(json!(s.links()))
}

async fn put(State(s): S, Path(id): Path<String>, Json(spec): Json<LinkSpec>) -> Response {
    // Opening devices and sockets blocks: off the async runtime.
    let r = tokio::task::spawn_blocking(move || s.put(&id, spec)).await;
    match r {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(Refused::NotAnInput(m))) => err(StatusCode::UNPROCESSABLE_ENTITY, m),
        Ok(Err(Refused::Invalid(m))) => err(StatusCode::BAD_REQUEST, m),
        Ok(Err(Refused::Unavailable(m))) => err(StatusCode::SERVICE_UNAVAILABLE, m),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn delete(State(s): S, Path(id): Path<String>) -> Response {
    let found = tokio::task::spawn_blocking(move || s.delete(&id))
        .await
        .unwrap_or(false);
    if found {
        Json(Value::Null).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "no such link")
    }
}

pub fn router(s: Arc<StreamService>) -> Router {
    Router::new()
        .route("/state", get(state))
        .route("/links", get(links))
        .route("/links/{id}", axum::routing::put(put).delete(delete))
        .with_state(s)
        .merge(health("stream", 1))
}
