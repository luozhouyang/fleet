//! Minimal Cloudflare API client for Cloudflare Access provisioning.
//!
//! Tunnel creation and DNS use the official `cloudflared` browser login. The
//! Access application/policy APIs require a scoped API token with:
//! `Access: Apps and Policies:Edit`, `Access: Organizations:Read`,
//! `Access: Identity Providers:Read` and `Account Settings:Read`.

use anyhow::{bail, Context, Result};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const API_BASE: &str = "https://api.cloudflare.com/client/v4";

#[derive(Debug, Clone)]
pub struct Account {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct AccessApp {
    pub id: String,
    pub domain: String,
}

#[derive(Debug, Clone)]
pub enum AccessAllow {
    Email(String),
    EmailDomain(String),
    Everyone,
}

impl AccessAllow {
    pub fn include(&self) -> Value {
        match self {
            Self::Email(email) => json!([{"email": {"email": email}}]),
            Self::EmailDomain(domain) => json!([{"email_domain": {"domain": domain}}]),
            Self::Everyone => json!([{"everyone": {}}]),
        }
    }

    pub fn description(&self) -> String {
        match self {
            Self::Email(email) => format!("email {email}"),
            Self::EmailDomain(domain) => format!("email domain {domain}"),
            Self::Everyone => "everyone".to_string(),
        }
    }
}

pub struct CloudflareApi {
    token: String,
    http: reqwest::Client,
}

impl CloudflareApi {
    pub fn new(token: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .context("failed to build Cloudflare HTTP client")?;
        Ok(Self { token, http })
    }

    pub async fn accounts(&self) -> Result<Vec<Account>> {
        let value = self
            .request(Method::GET, "/accounts?per_page=50", None)
            .await?;
        let items = value
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("unexpected accounts response"))?;
        Ok(items
            .iter()
            .filter_map(|item| {
                Some(Account {
                    id: item.get("id")?.as_str()?.to_string(),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                })
            })
            .collect())
    }

    pub async fn resolve_account(&self, requested: Option<&str>) -> Result<Account> {
        if let Some(id) = requested.map(str::trim).filter(|id| !id.is_empty()) {
            return Ok(Account {
                id: id.to_string(),
                name: id.to_string(),
            });
        }
        let accounts = self.accounts().await?;
        match accounts.len() {
            0 => bail!("the Cloudflare token cannot list any account"),
            1 => Ok(accounts.into_iter().next().expect("one account")),
            _ => {
                let names = accounts
                    .iter()
                    .map(|account| format!("{} ({})", account.name, account.id))
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!(
                    "multiple Cloudflare accounts are visible; pass --cloudflare-account-id ({names})"
                )
            }
        }
    }

    pub async fn find_access_app(
        &self,
        account_id: &str,
        hostname: &str,
    ) -> Result<Option<AccessApp>> {
        let path = format!("/accounts/{account_id}/access/apps?per_page=100");
        let value = self.request(Method::GET, &path, None).await?;
        let Some(items) = value.as_array() else {
            return Ok(None);
        };
        Ok(items.iter().find_map(|item| {
            let domain = item.get("domain").and_then(Value::as_str)?;
            let matches = domain == hostname
                || item
                    .get("self_hosted_domains")
                    .and_then(Value::as_array)
                    .map(|domains| domains.iter().any(|entry| entry.as_str() == Some(hostname)))
                    .unwrap_or(false);
            if !matches {
                return None;
            }
            Some(AccessApp {
                id: item.get("id")?.as_str()?.to_string(),
                domain: domain.to_string(),
            })
        }))
    }

    pub async fn create_access_app(&self, account_id: &str, hostname: &str) -> Result<AccessApp> {
        let path = format!("/accounts/{account_id}/access/apps");
        let body = json!({
            "name": "Fleet",
            "domain": hostname,
            "type": "self_hosted",
            "session_duration": "24h",
        });
        let value = self.request(Method::POST, &path, Some(body)).await?;
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Cloudflare did not return an Access app id"))?;
        Ok(AccessApp {
            id: id.to_string(),
            domain: hostname.to_string(),
        })
    }

    pub async fn policy_count(&self, account_id: &str, app_id: &str) -> Result<usize> {
        let path = format!("/accounts/{account_id}/access/apps/{app_id}/policies?per_page=50");
        let value = self.request(Method::GET, &path, None).await?;
        Ok(value.as_array().map(Vec::len).unwrap_or(0))
    }

    pub async fn create_policy(
        &self,
        account_id: &str,
        app_id: &str,
        allow: &AccessAllow,
    ) -> Result<()> {
        let path = format!("/accounts/{account_id}/access/apps/{app_id}/policies");
        let body = json!({
            "name": format!("Allow {}", allow.description()),
            "decision": "allow",
            "include": allow.include(),
        });
        self.request(Method::POST, &path, Some(body)).await?;
        Ok(())
    }

    async fn request(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let url = format!("{API_BASE}{path}");
        let mut request = self.http.request(method, &url).bearer_auth(&self.token);
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
            .context("failed to read Cloudflare response")?;
        let value: Value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        if !status.is_success() || value.get("success").and_then(Value::as_bool) == Some(false) {
            bail!("{}", format_error(status, &value));
        }
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }
}

fn format_error(status: StatusCode, value: &Value) -> String {
    let details = value
        .get("errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .map(|error| {
                    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error");
                    format!("{code}: {message}")
                })
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|details| !details.is_empty());
    match details {
        Some(details) => format!("Cloudflare API {status}: {details}"),
        None => format!("Cloudflare API {status}: request failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_access_includes() {
        assert_eq!(
            AccessAllow::Email("ada@example.com".into()).include(),
            json!([{"email": {"email": "ada@example.com"}}])
        );
        assert_eq!(
            AccessAllow::EmailDomain("example.com".into()).include(),
            json!([{"email_domain": {"domain": "example.com"}}])
        );
        assert_eq!(AccessAllow::Everyone.include(), json!([{"everyone": {}}]));
    }

    #[test]
    fn formats_cloudflare_errors() {
        let error = format_error(
            StatusCode::FORBIDDEN,
            &json!({"success": false, "errors": [{"code": 10000, "message": "Authentication error"}]}),
        );
        assert!(error.contains("10000: Authentication error"));
    }
}
