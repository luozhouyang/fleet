//! Minimal OpenCode v2 HTTP/SSE client.

use std::time::Duration;

use serde_json::{json, Value};

use crate::error::{Result, RuntimeError};

/// Basic-auth username expected by `opencode serve`.
const HTTP_USERNAME: &str = "opencode";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Session descriptor returned by `POST /api/session`.
#[derive(Debug, Clone, PartialEq)]
pub struct NativeSessionInfo {
    /// Provider-native session id (`ses_...`).
    pub id: String,
    /// Title assigned by the server, when returned.
    pub title: Option<String>,
}

/// Authenticated HTTP + SSE client for one OpenCode server.
#[derive(Clone)]
pub struct OpenCodeClient {
    http: reqwest::Client,
    sse: reqwest::Client,
    base_url: String,
    password: String,
}

impl OpenCodeClient {
    /// Create a client for `base_url` (with or without the `/api` suffix).
    pub fn new(base_url: impl Into<String>, password: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        // SSE responses never complete, so no overall timeout.
        let sse = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        Self {
            http,
            sse,
            base_url: base_url.into(),
            password: password.into(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    fn auth(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        builder.basic_auth(HTTP_USERNAME, Some(&self.password))
    }

    async fn send_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut builder = self.auth(self.http.request(method.clone(), self.url(path)));
        if let Some(body) = body {
            builder = builder.json(&body);
        }
        let response = builder.send().await.map_err(|err| {
            RuntimeError::ProviderUnavailable(format!(
                "opencode request {method} {path} failed: {err}"
            ))
        })?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(RuntimeError::provider(
                "opencode_http",
                format!("{method} {path} returned {status}: {text}"),
            ));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        let value: Value = serde_json::from_str(&text).map_err(|err| {
            RuntimeError::provider(
                "opencode_protocol",
                format!("{method} {path} returned invalid JSON: {err}"),
            )
        })?;
        Ok(match value {
            Value::Object(mut map) => match map.remove("data") {
                Some(data) => data,
                None => Value::Object(map),
            },
            other => other,
        })
    }

    /// `POST /api/session`
    pub async fn create_session(
        &self,
        directory: &str,
        title: Option<&str>,
        agent: Option<&str>,
        model: Option<&Value>,
    ) -> Result<NativeSessionInfo> {
        let mut body = serde_json::Map::new();
        body.insert("location".into(), json!({ "directory": directory }));
        if let Some(title) = title {
            body.insert("title".into(), json!(title));
        }
        if let Some(agent) = agent {
            body.insert("agent".into(), json!(agent));
        }
        if let Some(model) = model {
            body.insert("model".into(), model.clone());
        }
        let value = self
            .send_json(reqwest::Method::POST, "/session", Some(Value::Object(body)))
            .await?;
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                RuntimeError::provider("opencode_protocol", "session.create response has no id")
            })?
            .to_string();
        Ok(NativeSessionInfo {
            id,
            title: value
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    /// `POST /api/session/{id}/prompt`
    pub async fn prompt(&self, session_id: &str, text: &str) -> Result<()> {
        self.send_json(
            reqwest::Method::POST,
            &format!("/session/{session_id}/prompt"),
            Some(json!({ "text": text })),
        )
        .await?;
        Ok(())
    }

    /// `POST /api/session/{id}/interrupt`
    pub async fn interrupt(&self, session_id: &str) -> Result<bool> {
        let value = self
            .send_json(
                reqwest::Method::POST,
                &format!("/session/{session_id}/interrupt"),
                None,
            )
            .await?;
        Ok(value
            .get("interrupted")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

    /// `POST /api/session/{id}/permission/{request_id}/reply`
    pub async fn reply_permission(
        &self,
        session_id: &str,
        request_id: &str,
        approve: bool,
        message: Option<&str>,
    ) -> Result<()> {
        let decision = if approve { "once" } else { "reject" };
        let mut body = serde_json::Map::new();
        body.insert("decision".into(), json!(decision));
        if let Some(message) = message {
            body.insert("message".into(), json!(message));
        }
        self.send_json(
            reqwest::Method::POST,
            &format!("/session/{session_id}/permission/{request_id}/reply"),
            Some(Value::Object(body)),
        )
        .await?;
        Ok(())
    }

    /// `POST /api/session/{id}/question/{request_id}/reply`
    pub async fn reply_question(
        &self,
        session_id: &str,
        request_id: &str,
        answers: Vec<Vec<String>>,
    ) -> Result<()> {
        self.send_json(
            reqwest::Method::POST,
            &format!("/session/{session_id}/question/{request_id}/reply"),
            Some(json!({ "answers": answers })),
        )
        .await?;
        Ok(())
    }

    /// `POST /api/session/{id}/question/{request_id}/reject`
    pub async fn reject_question(&self, session_id: &str, request_id: &str) -> Result<()> {
        self.send_json(
            reqwest::Method::POST,
            &format!("/session/{session_id}/question/{request_id}/reject"),
            None,
        )
        .await?;
        Ok(())
    }

    /// `DELETE /api/session/{id}`
    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        let response = self
            .auth(
                self.http
                    .delete(self.url(&format!("/session/{session_id}"))),
            )
            .send()
            .await
            .map_err(|err| {
                RuntimeError::ProviderUnavailable(format!("opencode delete session failed: {err}"))
            })?;
        if !response.status().is_success() && response.status() != reqwest::StatusCode::NOT_FOUND {
            return Err(RuntimeError::provider(
                "opencode_http",
                format!("DELETE session returned {}", response.status()),
            ));
        }
        Ok(())
    }

    /// `GET /api/event` (SSE). The caller parses `data:` frames.
    pub async fn event_stream(&self) -> Result<reqwest::Response> {
        let response = self
            .auth(self.sse.get(self.url("/event")))
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await
            .map_err(|err| {
                RuntimeError::ProviderUnavailable(format!(
                    "opencode event stream connection failed: {err}"
                ))
            })?;
        if !response.status().is_success() {
            return Err(RuntimeError::provider(
                "opencode_http",
                format!("GET /event returned {}", response.status()),
            ));
        }
        Ok(response)
    }

    /// `GET /api/info`, used for availability probes.
    pub async fn info(&self) -> Result<Value> {
        self.send_json(reqwest::Method::GET, "/info", None).await
    }
}
