//! HTTP metrics middleware.
//!
//! The route label comes from [`MatchedPath`], which axum populates with the
//! matched route template (for example `/api/v1/sessions/{id}`). Raw request
//! paths are never used as labels, so session and machine identifiers cannot
//! create unbounded series.

use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;

use crate::metrics::Metrics;

/// Route label used for fallback requests that are not SPA navigations.
pub const UNMATCHED_ROUTE: &str = "unmatched";
/// Route label used for the single-page application fallback.
pub const SPA_ROUTE: &str = "spa";

/// Record one HTTP request after the inner service finishes.
pub async fn track_metrics(
    metrics: Metrics,
    spa_fallback: bool,
    request: Request,
    next: Next,
) -> Response {
    let method = method_label(request.method().as_str()).to_string();
    let route = route_label(&request, spa_fallback);
    let _in_flight = metrics.http_in_flight_guard();
    let started = Instant::now();

    let response = next.run(request).await;

    let status = response.status().as_u16();
    metrics.observe_http(&method, &route, status, started.elapsed());
    response
}

/// Matched route template, or a bounded fallback label.
fn route_label(request: &Request, spa_fallback: bool) -> String {
    if let Some(matched) = request.extensions().get::<MatchedPath>() {
        return matched.as_str().to_string();
    }
    if spa_fallback && !is_api_or_websocket_path(request.uri().path()) {
        SPA_ROUTE.to_string()
    } else {
        UNMATCHED_ROUTE.to_string()
    }
}

fn is_api_or_websocket_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/") || path == "/ws" || path.starts_with("/ws/")
}

/// Keep the method label within the standard HTTP method set so clients
/// cannot mint arbitrary label values.
fn method_label(method: &str) -> &'static str {
    match method {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "DELETE" => "DELETE",
        "PATCH" => "PATCH",
        "HEAD" => "HEAD",
        "OPTIONS" => "OPTIONS",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(uri: &str) -> Request {
        Request::builder()
            .uri(uri)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    #[test]
    fn fallback_labels_never_include_the_raw_path() {
        let api = request("/api/v1/machines/m_secret");
        assert_eq!(route_label(&api, true), UNMATCHED_ROUTE);
        assert_eq!(route_label(&api, false), UNMATCHED_ROUTE);

        let spa = request("/machines/m_secret");
        assert_eq!(route_label(&spa, true), SPA_ROUTE);
        assert_eq!(route_label(&spa, false), UNMATCHED_ROUTE);
    }

    #[test]
    fn non_standard_methods_are_bucketed() {
        assert_eq!(method_label("GET"), "GET");
        assert_eq!(method_label("PROPFIND"), "other");
    }
}
