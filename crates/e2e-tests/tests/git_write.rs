//! Git write E2E: authorization, stage/unstage/commit/checkout and audit.

use anyhow::{bail, Result};
use fleet_e2e_tests::{Admin, AdminOptions, Daemon, DaemonOptions, Repo, TestEnv};
use reqwest::Method;
use serde_json::json;

const WRITE_TOKEN: &str = "git-write-secret";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "end-to-end test: run with `pnpm e2e`"]
async fn git_writes_are_authorized_and_audited() -> Result<()> {
    let env = TestEnv::new()?;
    let mut admin = Admin::start(
        &env,
        AdminOptions {
            token: Some("git-token".to_owned()),
            git_write_token: Some(WRITE_TOKEN.to_owned()),
            web_dir: None,
        },
    )
    .await?;
    let api = admin.api();

    let repo = Repo::init(
        env.path().join("repo"),
        &[
            ("a.txt", "one\n"),
            ("b.txt", "keep\n"),
            ("src/main.rs", "fn main() {}\n"),
        ],
    )?;
    let base_branch = repo.branch()?;
    repo.write("a.txt", "two\n")?;
    repo.write("new.txt", "new\n")?;

    let mut daemon = Daemon::new(&env, &admin, "git-write-machine", DaemonOptions::default())?;
    daemon.pair(&env, &admin).await?;
    daemon.start(&env).await?;
    let machine = daemon.wait_online(&admin).await?;

    let workspace = api
        .register_workspace(&machine.id, &repo.path_string(), "git-write")
        .await?;
    let workspace_id = workspace.id.clone();
    let stage_path = format!("/api/v1/workspaces/{workspace_id}/git/stage");
    let commit_path = format!("/api/v1/workspaces/{workspace_id}/git/commit");

    // Writes without the dedicated token are rejected.
    let (status, body) = api
        .expect_status(
            Method::POST,
            &stage_path,
            Some(json!({ "paths": ["a.txt"] })),
            &[],
        )
        .await?;
    if status.as_u16() != 403 || body["code"] != "forbidden" {
        bail!("stage without the write token returned {status} {body}");
    }

    // Stage and unstage.
    let staged = api
        .git_stage(&workspace_id, &["a.txt", "new.txt"], Some(WRITE_TOKEN))
        .await?;
    if staged["ok"] != true {
        bail!("stage failed: {staged}");
    }
    let names = repo.staged_names()?;
    if !names.iter().any(|name| name == "a.txt") || !names.iter().any(|name| name == "new.txt") {
        bail!("expected both files to be staged, got {names:?}");
    }

    let unstaged = api
        .git_unstage(&workspace_id, &["new.txt"], Some(WRITE_TOKEN))
        .await?;
    if unstaged["ok"] != true {
        bail!("unstage failed: {unstaged}");
    }
    let names = repo.staged_names()?;
    if !names.iter().any(|name| name == "a.txt") {
        bail!("a.txt lost its staged state");
    }
    if names.iter().any(|name| name == "new.txt") {
        bail!("new.txt is still staged");
    }

    // Commit only a.txt.
    let before = repo.head()?;
    let commit = api
        .git_commit(
            &workspace_id,
            "e2e: commit a",
            Some(&["a.txt"]),
            Some(WRITE_TOKEN),
        )
        .await?;
    let after = repo.head()?;
    if commit.summary != "e2e: commit a" {
        bail!("commit summary mismatch: {commit:?}");
    }
    if after == before {
        bail!("HEAD did not advance");
    }
    if commit.commit != after {
        bail!(
            "returned commit hash {} does not match HEAD {after}",
            commit.commit
        );
    }
    if repo.last_subject()? != "e2e: commit a" {
        bail!("commit message is missing");
    }
    if commit.branch != base_branch {
        bail!(
            "commit branch {} does not match {base_branch}",
            commit.branch
        );
    }
    if !repo.show_stat()?.contains("a.txt") {
        bail!("commit does not contain a.txt");
    }

    // Committing an empty index is a conflict.
    let (status, body) = api
        .expect_status(
            Method::POST,
            &commit_path,
            Some(json!({ "message": "noop" })),
            &[("X-Fleet-Git-Token", WRITE_TOKEN)],
        )
        .await?;
    if status.as_u16() != 409 || body["code"] != "conflict" {
        bail!("empty commit returned {status} {body}");
    }

    // Checkout: create a branch and switch back.
    let created = api
        .git_checkout(&workspace_id, "feature/e2e", true, Some(WRITE_TOKEN))
        .await?;
    if created["branch"] != "feature/e2e" {
        bail!("create checkout mismatch: {created}");
    }
    if repo.branch()? != "feature/e2e" {
        bail!("branch was not switched");
    }
    let back = api
        .git_checkout(&workspace_id, &base_branch, false, Some(WRITE_TOKEN))
        .await?;
    if back["branch"] != base_branch.as_str() {
        bail!("switch-back mismatch: {back}");
    }

    // Invalid paths are rejected.
    let (status, body) = api
        .expect_status(
            Method::POST,
            &stage_path,
            Some(json!({ "paths": ["../escape"] })),
            &[("X-Fleet-Git-Token", WRITE_TOKEN)],
        )
        .await?;
    if status.as_u16() != 400 || body["code"] != "invalid_request" {
        bail!("invalid path returned {status} {body}");
    }

    // Audit trail records every attempt.
    let audit = api.audit(100).await?;
    let expected = [
        ("git.stage", "denied"),
        ("git.stage", "ok"),
        ("git.unstage", "ok"),
        ("git.commit", "ok"),
        ("git.checkout", "ok"),
        ("git.commit", "error"),
        ("git.stage", "error"),
    ];
    for (action, outcome) in expected {
        if !audit
            .entries
            .iter()
            .any(|entry| entry.action == action && entry.details["outcome"] == outcome)
        {
            bail!("missing audit record {action}/{outcome}: {audit:?}");
        }
    }
    let entry = audit
        .entries
        .iter()
        .find(|entry| entry.action == "git.commit" && entry.details["outcome"] == "ok")
        .expect("successful commit audit entry");
    if entry.subject != workspace_id {
        bail!(
            "audit subject {} does not match {workspace_id}",
            entry.subject
        );
    }
    if entry.actor != "admin" {
        bail!("audit actor {} does not match admin", entry.actor);
    }
    if entry.details["commit"] != after.as_str() {
        bail!("audit commit hash does not match {after}");
    }

    daemon.stop().await?;
    admin.shutdown().await?;
    env.finish();
    Ok(())
}
