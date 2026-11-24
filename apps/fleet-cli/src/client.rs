use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::{Method, StatusCode};
use serde_json::Value;

/// Thin client over the fleet-admin REST API.
#[derive(Clone)]
pub struct AdminClient {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl AdminClient {
    pub fn new(base: impl Into<String>, token: Option<String>) -> Result<Self> {
        let base = normalize_base(&base.into())?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self { base, token, http })
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.request(Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.request(Method::POST, path, Some(body)).await
    }

    pub async fn put(&self, path: &str, body: Value) -> Result<Value> {
        self.request(Method::PUT, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value> {
        self.request(Method::DELETE, path, None).await
    }

    /// Health is public and does not require a token.
    pub async fn health(&self) -> Result<Value> {
        self.request(Method::GET, "/api/v1/health", None).await
    }

    /// Download a binary response such as a database backup.
    ///
    /// A larger timeout than the JSON client: a snapshot of a full admin
    /// database can take longer than a normal API call.
    pub async fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let url = self.url(path)?;
        let mut request = self
            .http
            .request(Method::GET, &url)
            .timeout(Duration::from_secs(300));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("request to {url} failed"))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .with_context(|| format!("failed to read response from {url}"))?;
        if !status.is_success() {
            let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            bail!(format_error(status, &value));
        }
        Ok(bytes.to_vec())
    }

    async fn request(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let url = self.url(path)?;
        let mut request = self.http.request(method, &url);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("request to {url} failed"))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .with_context(|| format!("failed to read response from {url}"))?;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        if !status.is_success() {
            bail!(format_error(status, &value));
        }
        Ok(value)
    }

    fn url(&self, path: &str) -> Result<String> {
        if !path.starts_with('/') {
            bail!("internal error: API path must start with `/`");
        }
        Ok(format!("{}{path}", self.base))
    }
}

fn normalize_base(input: &str) -> Result<String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        bail!("admin URL must not be empty");
    }
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        bail!("admin URL must start with http:// or https:// (got `{input}`)");
    }
    Ok(trimmed.to_string())
}

fn format_error(status: StatusCode, value: &Value) -> String {
    let code = value.get("code").and_then(Value::as_str);
    let message = value.get("message").and_then(Value::as_str);
    match (code, message) {
        (Some(code), Some(message)) => format!("{status} ({code}): {message}"),
        (None, Some(message)) => format!("{status}: {message}"),
        _ => format!("{status}: request failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_base_urls() {
        assert_eq!(
            normalize_base("http://127.0.0.1:8787/").unwrap(),
            "http://127.0.0.1:8787"
        );
        assert!(normalize_base("127.0.0.1:8787").is_err());
        assert!(normalize_base("  ").is_err());
    }

    #[test]
    fn joins_api_paths() {
        let client = AdminClient::new("http://127.0.0.1:8787", None).unwrap();
        assert_eq!(
            client.url("/api/v1/machines").unwrap(),
            "http://127.0.0.1:8787/api/v1/machines"
        );
        assert!(client.url("machines").is_err());
    }

    #[test]
    fn formats_api_errors() {
        let error = format_error(
            StatusCode::UNAUTHORIZED,
            &serde_json::json!({"code": "unauthorized", "message": "bad token"}),
        );
        assert_eq!(error, "401 Unauthorized (unauthorized): bad token");
    }
}
