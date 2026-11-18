//! Black-box smoke test: admin + daemon + mock provider + a real Git repo.

use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use fleet_core_protocol::agent::{
    AgentQuestionAnswer, MessageRole, PermissionStatus, QuestionStatus, SessionStatus,
    ToolCallStatus,
};
use fleet_e2e_tests::harness::wait::wait_until;
use fleet_e2e_tests::{Admin, AdminOptions, Daemon, DaemonOptions, Repo, TestEnv};
use reqwest::{Method, StatusCode};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "end-to-end test: run with `pnpm e2e`"]
async fn unpaired_daemon_is_rejected() -> Result<()> {
    let env = TestEnv::new()?;
    let mut admin = Admin::start(&env, AdminOptions::default()).await?;
    let mut daemon = Daemon::new(&env, &admin, "unpaired", DaemonOptions::default())?;
    daemon.start(&env).await?;

    wait_until(
        "the unpaired daemon to be rejected",
        Duration::from_secs(15),
        || {
            let logs = daemon.logs();
            async move {
                if logs.contains("machine is not paired") && logs.contains("machine_not_paired") {
                    Ok(())
                } else {
                    Err(anyhow!(
                        "daemon log does not contain the machine_not_paired rejection yet"
                    ))
                }
            }
        },
    )
    .await?;

    daemon.stop().await?;
    admin.shutdown().await?;
    env.finish();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "end-to-end test: run with `pnpm e2e`"]
async fn full_smoke() -> Result<()> {
    let env = TestEnv::new()?;
    let mut admin = Admin::start(&env, AdminOptions::default()).await?;
    let api = admin.api();

    let repo = Repo::init(
        env.path().join("repo"),
        &[
            ("a.txt", "hello\nworld\n"),
            ("src/main.rs", "fn main() {}\n"),
        ],
    )?;
    repo.write("a.txt", "hello\nworld\nchanged\n")?;
    repo.write("new.txt", "untracked\n")?;

    let mut daemon = Daemon::new(&env, &admin, "e2e-machine", DaemonOptions::default())?;
    daemon.pair(&env, &admin).await?;

    let token_path = daemon.token_path();
    if !token_path.is_file() {
        bail!("pairing did not write {}", token_path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&token_path)?.permissions().mode() & 0o777;
        if mode != 0o600 {
            bail!("machine token mode is {mode:o}, expected 600");
        }
    }
    if fleet_e2e_tests::harness::daemon::config_has_token(&daemon.config)? {
        bail!("daemon config still contains a machine token");
    }

    daemon.start(&env).await?;
    let machine = daemon.wait_online(&admin).await?;
    let machine_id = machine.id.clone();

    let providers = api.providers(&machine_id).await?;
    if !providers.iter().any(|provider| provider.id == "mock") {
        bail!("mock provider missing from {providers:?}");
    }

    let agents = api
        .get_value(&format!("/api/v1/agents?machine_id={machine_id}"))
        .await?;
    if !agents
        .as_array()
        .is_some_and(|agents| agents.iter().any(|agent| agent["id"] == "mock"))
    {
        bail!("mock agent missing from the agent catalog: {agents:?}");
    }
    let drivers = api
        .get_value(&format!("/api/v1/drivers?machine_id={machine_id}"))
        .await?;
    if !drivers
        .as_array()
        .is_some_and(|drivers| drivers.iter().any(|driver| driver["id"] == "mock"))
    {
        bail!("mock driver missing from the driver catalog: {drivers:?}");
    }
    let probe = api
        .post_empty(&format!(
            "/api/v1/agents/mock/probe?machine_id={machine_id}"
        ))
        .await?;
    if probe["available"] != true {
        bail!("mock agent probe is not available: {probe:?}");
    }

    let workspace = api
        .register_workspace(&machine_id, &repo.path_string(), "e2e")
        .await?;
    let workspace_id = workspace.id.clone();

    let tree = api.tree(&workspace_id, "").await?;
    if !tree.entries.iter().any(|entry| entry.name == "a.txt")
        || !tree.entries.iter().any(|entry| entry.name == "src")
    {
        bail!("tree is missing expected entries: {tree:?}");
    }
    if tree.entries.first().is_none_or(|entry| {
        entry.entry_type != fleet_core_protocol::workspace::EntryType::Directory
    }) {
        bail!("directories must sort first: {tree:?}");
    }

    let file = api.file(&workspace_id, "a.txt").await?;
    if !file.content.contains("changed") {
        bail!("file content mismatch: {:?}", file.content);
    }
    let response = api
        .file_with_etag(&workspace_id, "a.txt", &file.etag)
        .await?;
    if response.status() != StatusCode::NOT_MODIFIED {
        bail!("expected 304 Not Modified, got {}", response.status());
    }

    let git_status = api.git_status(&workspace_id).await?;
    let changed: Vec<&str> = git_status
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    if !changed.contains(&"a.txt") || !changed.contains(&"new.txt") {
        bail!("git status is missing changes: {changed:?}");
    }
    let summary = api.git_diff_summary(&workspace_id).await?;
    if !summary.files.iter().any(|file| file.path == "a.txt") {
        bail!("diff summary is missing a.txt: {summary:?}");
    }
    let diff = api.git_diff(&workspace_id, "a.txt", true).await?;
    if diff.files.first().is_none_or(|file| file.hunks.is_empty()) {
        bail!("diff has no hunks for a.txt: {diff:?}");
    }
    if api.git_branches(&workspace_id).await?.is_empty() {
        bail!("no git branches");
    }
    if api.git_log(&workspace_id).await?.is_empty() {
        bail!("no git log entries");
    }

    let session = api
        .create_session(&machine_id, &workspace_id, "e2e")
        .await?;
    let session_id = session.id.clone();
    api.send_message(&session_id, "hello e2e").await?;
    wait_session_text(&api, &session_id, "Echo: hello e2e").await?;

    // Permission approval round-trip.
    api.send_message(&session_id, "/tool").await?;
    let permission = wait_until("a pending permission", Duration::from_secs(20), || {
        let api = api.clone();
        let session_id = session_id.clone();
        async move {
            let permissions = api.permissions(&session_id).await?;
            permissions
                .into_iter()
                .find(|permission| permission.status == PermissionStatus::Pending)
                .ok_or_else(|| anyhow!("no pending permission yet"))
        }
    })
    .await?;
    let approved = api
        .approve_permission(&permission.session_id, &permission.id)
        .await?;
    if approved["status"] != "approved" && approved["ok"] != true {
        bail!("permission approve failed: {approved}");
    }
    wait_session_text(&api, &session_id, "Tool finished").await?;

    let detail = api.session(&session_id).await?;
    if !detail.tool_calls.iter().any(|call| {
        call.name == "Bash"
            && call.status == ToolCallStatus::Completed
            && call.output["stdout"] == "hello\n"
    }) {
        bail!("tool call history is missing the completed Bash call");
    }
    if !detail
        .messages
        .iter()
        .any(|message| message.role == MessageRole::User && message.content == "/tool")
    {
        bail!("user message history is missing /tool");
    }

    // User question round-trip through the session-scoped admin route.
    api.send_message(&session_id, "/question").await?;
    let question = wait_until("a pending question", Duration::from_secs(20), || {
        let api = api.clone();
        let session_id = session_id.clone();
        async move {
            api.questions(&session_id)
                .await?
                .into_iter()
                .find(|question| question.status == QuestionStatus::Pending)
                .ok_or_else(|| anyhow!("no pending question yet"))
        }
    })
    .await?;
    let answered = api
        .answer_question(
            &session_id,
            &question.id,
            &[
                AgentQuestionAnswer {
                    id: "target".into(),
                    selected: vec!["staging".into()],
                    custom: None,
                },
                AgentQuestionAnswer {
                    id: "notes".into(),
                    selected: vec![],
                    custom: Some("deploy after lunch".into()),
                },
            ],
        )
        .await?;
    if answered["ok"] != true {
        bail!("question answer failed: {answered}");
    }
    wait_session_text(&api, &session_id, "Question answered").await?;
    let detail = api.session(&session_id).await?;
    if !detail.questions.iter().any(|question| {
        question.status == QuestionStatus::Answered
            && question.questions.iter().any(|item| item.id == "target")
    }) {
        bail!("question history is missing the answered question");
    }

    // Interrupting a slow turn.
    let second = api
        .create_session(&machine_id, &workspace_id, "interrupt")
        .await?;
    let second_id = second.id.clone();
    api.send_message(&second_id, "/slow").await?;
    tokio::time::sleep(Duration::from_millis(600)).await;
    api.interrupt(&second_id).await?;
    wait_until(
        "the second session to be interrupted",
        Duration::from_secs(20),
        || {
            let api = api.clone();
            let second_id = second_id.clone();
            async move {
                let detail = api.session(&second_id).await?;
                match detail.session.status {
                    SessionStatus::Interrupted | SessionStatus::WaitingForInput => Ok(()),
                    status => Err(anyhow!("session is still {status:?}")),
                }
            }
        },
    )
    .await?;

    // Durable outbox is acknowledged and pruned.
    let pool = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&daemon.db)
        .read_only(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(pool)
        .await?;
    wait_until(
        "the daemon outbox to drain",
        Duration::from_secs(20),
        || {
            let pool = pool.clone();
            async move {
                let (rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM event_outbox")
                    .fetch_one(&pool)
                    .await?;
                let cursor: Option<(i64,)> = sqlx::query_as(
                    "SELECT acked_sequence FROM event_cursors WHERE consumer = 'admin'",
                )
                .fetch_optional(&pool)
                .await?;
                let cursor = cursor.map(|(value,)| value).unwrap_or(0);
                if rows == 0 && cursor > 0 {
                    Ok(())
                } else {
                    Err(anyhow!("outbox rows={rows}, admin cursor={cursor}"))
                }
            }
        },
    )
    .await?;
    pool.close().await;

    // Deleting a busy workspace is a conflict.
    let (status, _) = api
        .expect_status(
            Method::DELETE,
            &format!("/api/v1/workspaces/{workspace_id}"),
            None,
            &[],
        )
        .await?;
    if status != StatusCode::CONFLICT {
        bail!("deleting a busy workspace returned {status}, expected 409");
    }

    // Daemon crash + restart: recovered sessions are interrupted, read-only
    // and keep their transcript.
    let restart_session = api
        .create_session(&machine_id, &workspace_id, "restart")
        .await?;
    let restart_id = restart_session.id.clone();
    api.send_message(&restart_id, "hello restart").await?;
    wait_session_text(&api, &restart_id, "Echo: hello restart").await?;

    daemon.crash().await?;
    daemon.start(&env).await?;
    daemon.wait_online(&admin).await?;
    wait_until(
        "the recovered session to become interrupted",
        Duration::from_secs(20),
        || {
            let api = api.clone();
            let restart_id = restart_id.clone();
            async move {
                let detail = api.session(&restart_id).await?;
                if detail.session.status == SessionStatus::Interrupted {
                    Ok(detail)
                } else {
                    Err(anyhow!("session is {:?}", detail.session.status))
                }
            }
        },
    )
    .await?;
    let recovered = api.session(&restart_id).await?;
    if recovered.session.metadata["restored"] != true {
        bail!("recovered session is missing the restored marker");
    }
    let recovered_text = recovered
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<String>();
    if !recovered_text.contains("Echo: hello restart") {
        bail!("recovered session lost its transcript");
    }
    if api.send_message(&restart_id, "again").await.is_ok() {
        bail!("a restored session accepted a new message");
    }
    api.delete_session(&restart_id).await?;

    // Close archives a session but keeps its history.
    api.close_session(&session_id).await?;
    wait_until("the session to archive", Duration::from_secs(20), || {
        let api = api.clone();
        let session_id = session_id.clone();
        async move {
            let detail = api.session(&session_id).await?;
            if detail.session.status == SessionStatus::Terminated {
                Ok(detail)
            } else {
                Err(anyhow!("session is {:?}", detail.session.status))
            }
        }
    })
    .await?;
    let open = api.sessions(false).await?;
    if open.iter().any(|session| session.id == session_id) {
        bail!("archived session leaked into the default list");
    }
    let all = api.sessions(true).await?;
    if !all
        .iter()
        .any(|session| session.id == session_id && session.status == SessionStatus::Terminated)
    {
        bail!("archived session is missing from include_closed");
    }
    let archived = api.session(&session_id).await?;
    let text = archived
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<String>();
    if !text.contains("Echo: hello e2e") {
        bail!("archived session lost its history");
    }

    // Permanent deletion.
    api.delete_session(&session_id).await?;
    if api
        .sessions(true)
        .await?
        .iter()
        .any(|session| session.id == session_id)
    {
        bail!("deleted session still appears");
    }
    api.delete_session(&second_id).await?;
    api.delete_workspace(&workspace_id).await?;

    // Revocation is visible and can be undone.
    api.delete_machine(&machine_id).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    if api
        .machines()
        .await?
        .iter()
        .any(|machine| machine.id == machine_id)
    {
        bail!("revoked machine came back online");
    }
    let revoked = api.revoked_machines().await?;
    if !revoked.as_array().is_some_and(|entries| {
        entries
            .iter()
            .any(|entry| entry["machine_id"] == machine_id)
    }) {
        bail!("revoked machine missing from the revocation list: {revoked}");
    }
    let unrevoked = api.unrevoke_machine(&machine_id).await?;
    if unrevoked["ok"] != true {
        bail!("machine unrevoke failed: {unrevoked}");
    }

    daemon.stop().await?;
    admin.shutdown().await?;
    env.finish();
    Ok(())
}

async fn wait_session_text(
    api: &fleet_e2e_tests::Api,
    session_id: &str,
    needle: &str,
) -> Result<()> {
    let api = api.clone();
    let session_id = session_id.to_owned();
    let needle = needle.to_owned();
    wait_until(
        &format!("session text to contain {needle:?}"),
        Duration::from_secs(20),
        || {
            let api = api.clone();
            let session_id = session_id.clone();
            let needle = needle.clone();
            async move {
                let detail = api.session(&session_id).await?;
                let text = detail
                    .messages
                    .iter()
                    .map(|message| message.content.as_str())
                    .collect::<String>();
                if text.contains(&needle) {
                    Ok(())
                } else {
                    Err(anyhow!("session text does not contain {needle:?} yet"))
                }
            }
        },
    )
    .await
}
