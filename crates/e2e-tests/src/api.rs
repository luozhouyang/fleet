//! Typed client for the fleet-admin REST API.

use anyhow::{bail, Context, Result};
use fleet_core_protocol::agent::{
    AgentPermissionRequest, AgentQuestionAnswer, AgentQuestionRequest, AgentSession, ProviderInfo,
};
use fleet_core_protocol::audit::AuditLogPage;
use fleet_core_protocol::git::{
    GitBranch, GitCommitResult, GitDiff, GitDiffSummary, GitLogEntry, GitStatus,
};
use fleet_core_protocol::machine::Machine;
use fleet_core_protocol::rpc::SessionDetail;
use fleet_core_protocol::workspace::{DirectoryListing, FileContent, Workspace};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

/// Bearer-authenticated HTTP client for one admin instance.
#[derive(Clone)]
pub struct Api {
    base: String,
    token: String,
    client: reqwest::Client,
}

impl Api {
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            token: token.into(),
            client: reqwest::Client::new(),
        }
    }

    /// A client with the same admin URL but a different bearer token.
    pub fn with_token(&self, token: impl Into<String>) -> Self {
        Self {
            base: self.base.clone(),
            token: token.into(),
            client: self.client.clone(),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_owned()
        } else {
            format!("{}{}", self.base, path)
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> Result<reqwest::Response> {
        let mut request = self
            .client
            .request(method, self.url(path))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request
            .send()
            .await
            .with_context(|| format!("request {path}"))
    }

    async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
        let status = response.status();
        let text = response.text().await.context("read the response body")?;
        if !status.is_success() {
            bail!("HTTP {status}: {text}");
        }
        serde_json::from_str(&text).with_context(|| format!("decode {text}"))
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Self::decode(self.send(Method::GET, path, None, &[]).await?).await
    }

    pub async fn get_value(&self, path: &str) -> Result<Value> {
        self.get(path).await
    }

    pub async fn get_response(
        &self,
        path: &str,
        headers: &[(&str, &str)],
    ) -> Result<reqwest::Response> {
        self.send(Method::GET, path, None, headers).await
    }

    pub async fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        let body = serde_json::to_value(body).context("encode the request body")?;
        Self::decode(self.send(Method::POST, path, Some(body), &[]).await?).await
    }

    pub async fn post_value(&self, path: &str, body: &Value) -> Result<Value> {
        self.post(path, body).await
    }

    pub async fn post_empty(&self, path: &str) -> Result<Value> {
        self.post(path, &json!({})).await
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        let response = self.send(Method::DELETE, path, None, &[]).await?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("DELETE {path} returned {status}: {text}");
        }
        Ok(())
    }

    /// Send a request and keep the raw status plus decoded payload. Used for
    /// the deliberate negative-path assertions.
    pub async fn expect_status(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> Result<(StatusCode, Value)> {
        let response = self.send(method, path, body, headers).await?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let value = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        Ok((status, value))
    }

    // -- machines ---------------------------------------------------------

    pub async fn machines(&self) -> Result<Vec<Machine>> {
        self.get("/api/v1/machines").await
    }

    pub async fn machine(&self, machine_id: &str) -> Result<Machine> {
        self.get(&format!("/api/v1/machines/{machine_id}")).await
    }

    pub async fn providers(&self, machine_id: &str) -> Result<Vec<ProviderInfo>> {
        self.get(&format!("/api/v1/machines/{machine_id}/providers"))
            .await
    }

    pub async fn revoked_machines(&self) -> Result<Value> {
        self.get_value("/api/v1/machines/revoked").await
    }

    pub async fn unrevoke_machine(&self, machine_id: &str) -> Result<Value> {
        self.post(
            &format!("/api/v1/machines/{machine_id}/unrevoke"),
            &json!({}),
        )
        .await
    }

    pub async fn delete_machine(&self, machine_id: &str) -> Result<()> {
        self.delete(&format!("/api/v1/machines/{machine_id}")).await
    }

    // -- workspaces -------------------------------------------------------

    pub async fn register_workspace(
        &self,
        machine_id: &str,
        root_path: &str,
        name: &str,
    ) -> Result<Workspace> {
        self.post(
            "/api/v1/workspaces",
            &json!({
                "machine_id": machine_id,
                "root_path": root_path,
                "name": name,
            }),
        )
        .await
    }

    pub async fn delete_workspace(&self, workspace_id: &str) -> Result<()> {
        self.delete(&format!("/api/v1/workspaces/{workspace_id}"))
            .await
    }

    pub async fn tree(&self, workspace_id: &str, path: &str) -> Result<DirectoryListing> {
        self.get(&format!(
            "/api/v1/workspaces/{workspace_id}/tree?path={path}"
        ))
        .await
    }

    pub async fn file(&self, workspace_id: &str, path: &str) -> Result<FileContent> {
        self.get(&format!(
            "/api/v1/workspaces/{workspace_id}/files?path={path}"
        ))
        .await
    }

    /// Read a file with `If-None-Match`; returns the raw response so callers
    /// can assert on `304 Not Modified`.
    pub async fn file_with_etag(
        &self,
        workspace_id: &str,
        path: &str,
        etag: &str,
    ) -> Result<reqwest::Response> {
        self.get_response(
            &format!("/api/v1/workspaces/{workspace_id}/files?path={path}&etag={etag}"),
            &[("If-None-Match", etag)],
        )
        .await
    }

    // -- git --------------------------------------------------------------

    pub async fn git_status(&self, workspace_id: &str) -> Result<GitStatus> {
        self.get(&format!("/api/v1/workspaces/{workspace_id}/git/status"))
            .await
    }

    pub async fn git_diff_summary(&self, workspace_id: &str) -> Result<GitDiffSummary> {
        self.get(&format!(
            "/api/v1/workspaces/{workspace_id}/git/diff/summary"
        ))
        .await
    }

    pub async fn git_diff(
        &self,
        workspace_id: &str,
        path: &str,
        include_untracked: bool,
    ) -> Result<GitDiff> {
        self.get(&format!(
            "/api/v1/workspaces/{workspace_id}/git/diff?path={path}&include_untracked={include_untracked}"
        ))
        .await
    }

    pub async fn git_branches(&self, workspace_id: &str) -> Result<Vec<GitBranch>> {
        self.get(&format!("/api/v1/workspaces/{workspace_id}/git/branches"))
            .await
    }

    pub async fn git_log(&self, workspace_id: &str) -> Result<Vec<GitLogEntry>> {
        self.get(&format!("/api/v1/workspaces/{workspace_id}/git/log"))
            .await
    }

    pub async fn git_stage(
        &self,
        workspace_id: &str,
        paths: &[&str],
        write_token: Option<&str>,
    ) -> Result<Value> {
        self.git_write(
            workspace_id,
            "stage",
            json!({ "paths": paths }),
            write_token,
        )
        .await
    }

    pub async fn git_unstage(
        &self,
        workspace_id: &str,
        paths: &[&str],
        write_token: Option<&str>,
    ) -> Result<Value> {
        self.git_write(
            workspace_id,
            "unstage",
            json!({ "paths": paths }),
            write_token,
        )
        .await
    }

    pub async fn git_commit(
        &self,
        workspace_id: &str,
        message: &str,
        paths: Option<&[&str]>,
        write_token: Option<&str>,
    ) -> Result<GitCommitResult> {
        let body = match paths {
            Some(paths) => json!({ "message": message, "paths": paths }),
            None => json!({ "message": message }),
        };
        let response = self
            .git_write_response(workspace_id, "commit", body, write_token)
            .await?;
        Self::decode(response).await
    }

    pub async fn git_checkout(
        &self,
        workspace_id: &str,
        branch: &str,
        create: bool,
        write_token: Option<&str>,
    ) -> Result<Value> {
        self.git_write(
            workspace_id,
            "checkout",
            json!({ "branch": branch, "create": create }),
            write_token,
        )
        .await
    }

    async fn git_write(
        &self,
        workspace_id: &str,
        action: &str,
        body: Value,
        write_token: Option<&str>,
    ) -> Result<Value> {
        let response = self
            .git_write_response(workspace_id, action, body, write_token)
            .await?;
        Self::decode(response).await
    }

    async fn git_write_response(
        &self,
        workspace_id: &str,
        action: &str,
        body: Value,
        write_token: Option<&str>,
    ) -> Result<reqwest::Response> {
        let path = format!("/api/v1/workspaces/{workspace_id}/git/{action}");
        let headers: Vec<(&str, &str)> = match write_token {
            Some(token) => vec![("X-Fleet-Git-Token", token)],
            None => Vec::new(),
        };
        self.send(Method::POST, &path, Some(body), &headers).await
    }

    // -- sessions ---------------------------------------------------------

    pub async fn sessions(&self, include_closed: bool) -> Result<Vec<AgentSession>> {
        let query = if include_closed {
            "?include_closed=true"
        } else {
            ""
        };
        self.get(&format!("/api/v1/sessions{query}")).await
    }

    pub async fn session(&self, session_id: &str) -> Result<SessionDetail> {
        self.get(&format!("/api/v1/sessions/{session_id}")).await
    }

    pub async fn create_session(
        &self,
        machine_id: &str,
        workspace_id: &str,
        title: &str,
    ) -> Result<AgentSession> {
        self.post(
            "/api/v1/sessions",
            &json!({
                "machine_id": machine_id,
                "provider": "mock",
                "workspace_id": workspace_id,
                "title": title,
            }),
        )
        .await
    }

    pub async fn send_message(&self, session_id: &str, content: &str) -> Result<Value> {
        self.post(
            &format!("/api/v1/sessions/{session_id}/messages"),
            &json!({ "content": content }),
        )
        .await
    }

    pub async fn permissions(&self, session_id: &str) -> Result<Vec<AgentPermissionRequest>> {
        self.get(&format!("/api/v1/sessions/{session_id}/permissions"))
            .await
    }

    pub async fn approve_permission(&self, session_id: &str, permission_id: &str) -> Result<Value> {
        self.post(
            &format!("/api/v1/sessions/{session_id}/permissions/{permission_id}/approve"),
            &json!({}),
        )
        .await
    }

    pub async fn questions(&self, session_id: &str) -> Result<Vec<AgentQuestionRequest>> {
        self.get(&format!("/api/v1/sessions/{session_id}/questions"))
            .await
    }

    pub async fn answer_question(
        &self,
        session_id: &str,
        question_id: &str,
        answers: &[AgentQuestionAnswer],
    ) -> Result<Value> {
        self.post(
            &format!("/api/v1/sessions/{session_id}/questions/{question_id}/answer"),
            &json!({ "answers": answers }),
        )
        .await
    }

    pub async fn reject_question(&self, session_id: &str, question_id: &str) -> Result<Value> {
        self.post(
            &format!("/api/v1/sessions/{session_id}/questions/{question_id}/reject"),
            &json!({}),
        )
        .await
    }

    pub async fn interrupt(&self, session_id: &str) -> Result<Value> {
        self.post_empty(&format!("/api/v1/sessions/{session_id}/interrupt"))
            .await
    }

    pub async fn close_session(&self, session_id: &str) -> Result<Value> {
        self.post_empty(&format!("/api/v1/sessions/{session_id}/close"))
            .await
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        self.delete(&format!("/api/v1/sessions/{session_id}")).await
    }

    // -- users, orgs, audit ----------------------------------------------

    pub async fn create_org(&self, name: &str) -> Result<Value> {
        self.post("/api/v1/orgs", &json!({ "name": name })).await
    }

    pub async fn create_user(
        &self,
        org_id: &str,
        email: &str,
        name: &str,
        role: &str,
    ) -> Result<Value> {
        self.post(
            "/api/v1/users",
            &json!({ "org_id": org_id, "email": email, "name": name, "role": role }),
        )
        .await
    }

    pub async fn list_users(&self) -> Result<Value> {
        self.get_value("/api/v1/users").await
    }

    pub async fn rotate_user_token(&self, user_id: &str) -> Result<Value> {
        self.post_empty(&format!("/api/v1/users/{user_id}/rotate-token"))
            .await
    }

    pub async fn assign_machine_org(&self, machine_id: &str, org_id: &str) -> Result<Value> {
        let response = self
            .send(
                Method::PUT,
                &format!("/api/v1/machines/{machine_id}/org"),
                Some(json!({ "org_id": org_id })),
                &[],
            )
            .await?;
        Self::decode(response).await
    }

    pub async fn audit(&self, limit: usize) -> Result<AuditLogPage> {
        self.get(&format!("/api/v1/audit?limit={limit}")).await
    }
}
