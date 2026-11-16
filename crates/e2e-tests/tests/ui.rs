//! Browser E2E orchestration: start the stack, then drive Playwright.
//!
//! The browser assertions stay in `apps/web/e2e`; this test replaces the old
//! shell script that used to build and supervise the stack.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use fleet_e2e_tests::{Admin, AdminOptions, Daemon, DaemonOptions, Repo, TestEnv};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "browser E2E: run with `pnpm e2e:ui`"]
async fn browser_e2e() -> Result<()> {
    let env = TestEnv::new()?;
    let root = env.workspace_root().to_path_buf();
    let dist = root.join("apps/web/dist");

    if std::env::var("FLEET_E2E_SKIP_WEB_BUILD").as_deref() != Ok("1") {
        let output = tokio::process::Command::new("pnpm")
            .current_dir(&root)
            .args(["--filter", "@agent-fleet/web", "build"])
            .output()
            .await
            .context("run the web build")?;
        if !output.status.success() {
            bail!(
                "web build failed ({})\nstdout:\n{}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }
    if !dist.join("index.html").is_file() {
        bail!("apps/web/dist/index.html was not produced");
    }

    let mut admin = Admin::start(
        &env,
        AdminOptions {
            token: None,
            git_write_token: None,
            web_dir: Some(dist),
        },
    )
    .await?;
    let api = admin.api();

    let repo = Repo::init(
        env.path().join("repo"),
        &[
            ("a.txt", "hello\nworld\n"),
            (
                "src/main.rs",
                "fn main() {\n    println!(\"hello world\");\n}\n",
            ),
            ("notes.txt", "committed notes\n"),
        ],
    )?;
    repo.write("a.txt", "hello\nworld\nchanged\n")?;
    repo.write("new.txt", "untracked\n")?;

    let mut daemon = Daemon::new(&env, &admin, "ui-e2e-machine", DaemonOptions::default())?;
    daemon.pair(&env, &admin).await?;
    daemon.start(&env).await?;
    let machine = daemon.wait_online(&admin).await?;
    let workspace = api
        .register_workspace(&machine.id, &repo.path_string(), "demo")
        .await?;
    if workspace.id.is_empty() {
        bail!("workspace id is missing");
    }

    // Let the workspace watcher settle before the UI asserts on Git state.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let output = tokio::process::Command::new("pnpm")
        .current_dir(root.join("apps/web"))
        .args(["exec", "playwright", "test"])
        .env("E2E_BASE_URL", &admin.url)
        .env("E2E_TOKEN", &admin.token)
        .env("E2E_REPO", repo.path_string())
        .output()
        .await
        .context("run Playwright")?;

    if !output.status.success() {
        bail!(
            "Playwright failed ({})\n--- stdout ---\n{}\n--- stderr ---\n{}\n\
             --- fleet-admin log ---\n{}\n--- daemon log ---\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            admin.logs(),
            daemon.logs(),
        );
    }

    daemon.stop().await?;
    admin.shutdown().await?;
    env.finish();
    Ok(())
}
