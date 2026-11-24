//! Multi-machine stress and recovery E2E.
//!
//! Topology: one admin, three daemons and six Git workspaces (two per
//! machine). Exercises concurrent reads, sessions, permissions, interrupts,
//! organization scoping, offline/restart recovery and key rotation.

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use fleet_core_protocol::agent::SessionStatus;
use fleet_core_protocol::machine::MachineStatus;
use fleet_e2e_tests::harness::wait::wait_until;
use fleet_e2e_tests::{Admin, AdminOptions, Daemon, DaemonOptions, Repo, TestEnv};
use reqwest::Method;
use serde_json::json;

const MACHINES: usize = 3;
const WORKSPACES_PER_MACHINE: usize = 2;
const WORKSPACES: usize = MACHINES * WORKSPACES_PER_MACHINE;
const ECHO: &str = "hello-w$i";
const OTHER_ECHO: &str = "hello-w$j";

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "stress test: run with `pnpm e2e:stress`"]
async fn multi_machine_stress() -> Result<()> {
    let env = TestEnv::new()?;
    let mut admin = Admin::start(
        &env,
        AdminOptions {
            token: Some("stress-token".to_owned()),
            ..Default::default()
        },
    )
    .await?;
    let api = admin.api();

    // Two repositories per machine, each with a distinct owner marker.
    let mut repos = Vec::with_capacity(WORKSPACES);
    for index in 0..WORKSPACES {
        let machine = index / WORKSPACES_PER_MACHINE;
        let repo = Repo::init(
            env.path().join(format!("repo{index}")),
            &[
                (
                    "owner.txt",
                    &format!("machine-{machine} workspace-{index}\n"),
                ),
                ("src/main.rs", "fn main() {}\n"),
            ],
        )?;
        repo.write(
            "owner.txt",
            &format!("machine-{machine} workspace-{index}\nchanged\n"),
        )?;
        repos.push(repo);
    }

    // Pair and start three daemons.
    let mut daemons = Vec::with_capacity(MACHINES);
    for index in 0..MACHINES {
        let daemon = Daemon::new(
            &env,
            &admin,
            &format!("stress-m{index}"),
            DaemonOptions::default(),
        )?;
        daemon.pair(&env, &admin).await?;
        daemons.push(daemon);
    }
    for daemon in &mut daemons {
        daemon.start(&env).await?;
    }
    wait_for_online(&api, MACHINES).await?;

    let machines = api.machines().await?;
    let mut machine_ids: HashMap<String, String> = HashMap::new();
    for machine in &machines {
        machine_ids.insert(machine.name.clone(), machine.id.clone());
    }
    let mut machine_id = Vec::with_capacity(MACHINES);
    for index in 0..MACHINES {
        let name = format!("stress-m{index}");
        machine_id.push(
            machine_ids
                .get(&name)
                .cloned()
                .ok_or_else(|| anyhow!("machine {name} is missing"))?,
        );
    }

    // Register all workspaces concurrently.
    let registrations = (0..WORKSPACES).map(|index| {
        let api = api.clone();
        let machine = machine_id[index / WORKSPACES_PER_MACHINE].clone();
        let root = repos[index].path_string();
        async move {
            let workspace = api
                .register_workspace(&machine, &root, &format!("stress-w{index}"))
                .await?;
            Ok::<_, anyhow::Error>(workspace.id)
        }
    });
    let workspace_ids = join_all_ok(registrations).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Concurrent tree/file reads: 3 rounds x 6 workspaces x 2 requests.
    let mut reads = Vec::new();
    for _ in 0..3 {
        for (index, workspace_id) in workspace_ids.iter().enumerate() {
            let api = api.clone();
            let workspace = workspace_id.clone();
            reads.push(async move {
                let tree = api.tree(&workspace, "").await?;
                if !tree.entries.iter().any(|entry| entry.name == "owner.txt") {
                    bail!("workspace {workspace} tree is missing owner.txt");
                }
                let file = api.file(&workspace, "owner.txt").await?;
                let expected = format!(
                    "machine-{} workspace-{index}",
                    index / WORKSPACES_PER_MACHINE
                );
                if !file.content.contains(&expected) {
                    bail!(
                        "workspace {workspace} returned the wrong owner: {}",
                        file.content
                    );
                }
                Ok::<_, anyhow::Error>(())
            });
        }
    }
    join_all_ok(reads).await?;

    // One session per workspace, created concurrently.
    let creations = (0..WORKSPACES).map(|index| {
        let api = api.clone();
        let machine = machine_id[index / WORKSPACES_PER_MACHINE].clone();
        let workspace = workspace_ids[index].clone();
        async move {
            let session = api
                .create_session(&machine, &workspace, &format!("stress-{index}"))
                .await?;
            Ok::<_, anyhow::Error>(session.id)
        }
    });
    let sessions = join_all_ok(creations).await?;

    // Concurrent streaming and echo waits.
    let sends = sessions.iter().map(|session| {
        let api = api.clone();
        async move {
            api.send_message(session, ECHO).await?;
            Ok::<_, anyhow::Error>(())
        }
    });
    join_all_ok(sends).await?;
    let waits = sessions.iter().map(|session| {
        let api = api.clone();
        async move {
            wait_for_text(
                &api,
                session,
                &format!("Echo: {ECHO}"),
                Duration::from_secs(20),
            )
            .await
        }
    });
    join_all_ok(waits).await?;

    // No cross-session leakage: each session sees its own echo but not the
    // placeholder used by the isolation probe below.
    for (index, session) in sessions.iter().enumerate() {
        let detail = api.session(session).await?;
        let text = text_of(&detail);
        if !text.contains(&format!("Echo: {ECHO}")) {
            bail!("session {index} is missing its own echo");
        }
        if text.contains(OTHER_ECHO) {
            bail!("session {index} leaked another session's output");
        }
    }

    // Concurrent permission approvals.
    let sends = sessions.iter().map(|session| {
        let api = api.clone();
        async move {
            api.send_message(session, "/tool").await?;
            Ok::<_, anyhow::Error>(())
        }
    });
    join_all_ok(sends).await?;
    let approvals = sessions.iter().map(|session| {
        let api = api.clone();
        async move { approve_pending(&api, session).await }
    });
    join_all_ok(approvals).await?;

    // Concurrent interrupts on three long-running turns.
    let slow_sends = sessions.iter().take(3).map(|session| {
        let api = api.clone();
        async move {
            api.send_message(session, "/slow").await?;
            Ok::<_, anyhow::Error>(())
        }
    });
    join_all_ok(slow_sends).await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let interrupts = sessions.iter().take(3).map(|session| {
        let api = api.clone();
        async move {
            api.interrupt(session).await?;
            wait_for_interrupted(&api, session).await
        }
    });
    join_all_ok(interrupts).await?;

    // Organizations, roles and token rotation.
    let other_org = api.create_org("other-org").await?;
    let other_org_id = other_org["organization"]["id"]
        .as_str()
        .ok_or_else(|| anyhow!("organization id missing: {other_org}"))?
        .to_owned();

    let ada = api
        .create_user("org_default", "ada@example.com", "Ada", "member")
        .await?;
    if ada["membership"]["role"] != "member" {
        bail!("member creation failed: {ada}");
    }
    let ada_id = ada["user"]["id"]
        .as_str()
        .ok_or_else(|| anyhow!("Ada id missing"))?
        .to_owned();
    let ada_token = ada["token"]
        .as_str()
        .ok_or_else(|| anyhow!("Ada token missing"))?
        .to_owned();

    let root_user = api
        .create_user("org_default", "root@example.com", "Root", "admin")
        .await?;
    let root_token = root_user["token"]
        .as_str()
        .ok_or_else(|| anyhow!("Root token missing"))?
        .to_owned();
    let vera = api
        .create_user("org_default", "vera@example.com", "Vera", "viewer")
        .await?;
    let vera_token = vera["token"]
        .as_str()
        .ok_or_else(|| anyhow!("Vera token missing"))?
        .to_owned();
    let other = api
        .create_user(&other_org_id, "other@example.com", "Other", "member")
        .await?;
    let other_token = other["token"]
        .as_str()
        .ok_or_else(|| anyhow!("Other token missing"))?
        .to_owned();

    let ada_api = api.with_token(&ada_token);
    let root_api = api.with_token(&root_token);
    let vera_api = api.with_token(&vera_token);
    let other_api = api.with_token(&other_token);

    if ada_api.machines().await?.len() != MACHINES {
        bail!("member cannot see default-org machines");
    }
    if !other_api.machines().await?.is_empty() {
        bail!("other-org member sees default-org machines");
    }

    let moved_machine = 2;
    api.assign_machine_org(&machine_id[moved_machine], &other_org_id)
        .await?;
    if ada_api.machines().await?.len() != MACHINES - 1 {
        bail!("former org still sees the moved machine");
    }
    if other_api.machines().await?.len() != 1 {
        bail!("new org cannot see the moved machine");
    }

    let moved_workspace = workspace_ids[4].clone();
    other_api
        .git_status(&moved_workspace)
        .await
        .map_err(|error| anyhow!("new org cannot read its moved workspace: {error}"))?;
    let (status, _) = ada_api
        .expect_status(
            Method::GET,
            &format!("/api/v1/workspaces/{moved_workspace}"),
            None,
            &[],
        )
        .await?;
    if status.as_u16() != 404 {
        bail!("cross-org workspace returned {status}, expected 404");
    }
    let (status, _) = ada_api
        .expect_status(
            Method::GET,
            &format!("/api/v1/workspaces/{moved_workspace}/git/status"),
            None,
            &[],
        )
        .await?;
    if status.as_u16() != 404 {
        bail!("cross-org git status returned {status}, expected 404");
    }

    let (status, _) = vera_api
        .expect_status(
            Method::POST,
            &format!("/api/v1/workspaces/{}/git/stage", workspace_ids[0]),
            Some(json!({ "paths": ["owner.txt"] })),
            &[],
        )
        .await?;
    if status.as_u16() != 403 {
        bail!("viewer write returned {status}, expected 403");
    }
    let (status, _) = ada_api
        .expect_status(Method::GET, "/api/v1/users", None, &[])
        .await?;
    if status.as_u16() != 403 {
        bail!("member listing users returned {status}, expected 403");
    }
    let users = root_api.list_users().await?;
    if !users
        .as_array()
        .is_some_and(|users| users.iter().any(|user| user["name"] == "Ada"))
    {
        bail!("admin cannot list users: {users}");
    }

    let rotated = root_api.rotate_user_token(&ada_id).await?;
    let new_ada_token = rotated["token"]
        .as_str()
        .ok_or_else(|| anyhow!("rotated token missing"))?
        .to_owned();
    let (status, _) = ada_api
        .expect_status(Method::GET, "/api/v1/machines", None, &[])
        .await?;
    if status.as_u16() != 401 {
        bail!("old user token returned {status}, expected 401");
    }
    let new_ada_api = api.with_token(new_ada_token);
    if new_ada_api.machines().await?.len() != MACHINES - 1 {
        bail!("rotated user token does not work");
    }
    if root_api.audit(100).await?.entries.is_empty() {
        bail!("org admin cannot read the audit trail");
    }

    // One machine goes offline; the others keep serving.
    let offline = 1;
    daemons[offline].stop().await?;
    wait_until("one machine to go offline", Duration::from_secs(20), || {
        let api = api.clone();
        async move {
            let machines = api.machines().await?;
            let online = machines
                .iter()
                .filter(|machine| machine.status == MachineStatus::Online)
                .count();
            if online == MACHINES - 1 {
                Ok(())
            } else {
                Err(anyhow!("{online} machines are still online"))
            }
        }
    })
    .await?;

    let (status, body) = api
        .expect_status(
            Method::POST,
            "/api/v1/sessions",
            Some(json!({
                "machine_id": machine_id[offline],
                "provider": "mock",
                "cwd": "/tmp",
            })),
            &[],
        )
        .await?;
    if status.as_u16() != 503 || body["code"] != "machine_offline" {
        bail!("session on an offline machine returned {status} {body}");
    }

    let survivor = api
        .create_session(&machine_id[0], &workspace_ids[0], "survivor")
        .await?;
    api.send_message(&survivor.id, "offline-survivor").await?;
    wait_for_text(
        &api,
        &survivor.id,
        "Echo: offline-survivor",
        Duration::from_secs(20),
    )
    .await?;
    api.delete_session(&survivor.id).await?;

    // Restart the offline daemon and verify its workspaces recover.
    daemons[offline].start(&env).await?;
    wait_for_online(&api, MACHINES).await?;
    let recovered = api.machine(&machine_id[offline]).await?;
    if recovered.workspaces.len() != WORKSPACES_PER_MACHINE {
        bail!(
            "machine {} lost its workspaces after restart: {:?}",
            recovered.id,
            recovered.workspaces
        );
    }
    for (index, workspace_id) in workspace_ids.iter().enumerate().skip(2).take(2) {
        let tree = api.tree(workspace_id, "").await?;
        if !tree.entries.iter().any(|entry| entry.name == "owner.txt") {
            bail!("workspace {index} is not readable after reconnect");
        }
        let file = api.file(workspace_id, "owner.txt").await?;
        if !file
            .content
            .contains(&format!("machine-1 workspace-{index}"))
        {
            bail!("workspace {index} returned the wrong owner after reconnect");
        }
    }

    // Rotate machine 0's key and reconnect without creating a new identity.
    daemons[0].stop().await?;
    let rotation = daemons[0].rotate_key(&env).await?;
    if rotation["previous_fingerprint"] == rotation["new_fingerprint"] {
        bail!("rotate-key output is missing distinct fingerprints: {rotation}");
    }
    if !daemons[0].previous_key_path().is_file() {
        bail!("previous key was not kept");
    }
    daemons[0].start(&env).await?;
    wait_for_online(&api, MACHINES).await?;
    wait_until(
        "the previous machine key to be removed",
        Duration::from_secs(10),
        || {
            let previous = daemons[0].previous_key_path();
            async move {
                if previous.is_file() {
                    Err(anyhow!("previous key still exists"))
                } else {
                    Ok(())
                }
            }
        },
    )
    .await?;
    if api.machines().await?.len() != MACHINES {
        bail!("key rotation created a second machine identity");
    }
    api.tree(&workspace_ids[0], "").await?;

    // Post-restart soak: 60 concurrent reads.
    let mut soak = Vec::new();
    for _ in 0..10 {
        for workspace in &workspace_ids {
            let api = api.clone();
            let workspace = workspace.clone();
            soak.push(async move {
                let tree = api.tree(&workspace, "").await?;
                if !tree.entries.iter().any(|entry| entry.name == "owner.txt") {
                    bail!("soak read for {workspace} failed");
                }
                Ok::<_, anyhow::Error>(())
            });
        }
    }
    join_all_ok(soak).await?;

    // Best-effort cleanup.
    for session in &sessions {
        let _ = api.delete_session(session).await;
    }
    for workspace in &workspace_ids {
        let _ = api.delete_workspace(workspace).await;
    }
    for daemon in &mut daemons {
        let _ = daemon.stop().await;
    }
    admin.shutdown().await?;
    env.finish();
    Ok(())
}

async fn wait_for_online(api: &fleet_e2e_tests::Api, expected: usize) -> Result<()> {
    let api = api.clone();
    wait_until(
        &format!("all {expected} machines to be online"),
        Duration::from_secs(30),
        || {
            let api = api.clone();
            async move {
                let machines = api.machines().await?;
                let online = machines
                    .iter()
                    .filter(|machine| machine.status == MachineStatus::Online)
                    .count();
                if machines.len() == expected && online == expected {
                    Ok(())
                } else {
                    Err(anyhow!(
                        "{online}/{expected} machines online ({} registered)",
                        machines.len()
                    ))
                }
            }
        },
    )
    .await
}

async fn wait_for_text(
    api: &fleet_e2e_tests::Api,
    session: &str,
    needle: &str,
    timeout: Duration,
) -> Result<()> {
    let api = api.clone();
    let session = session.to_owned();
    let needle = needle.to_owned();
    wait_until(
        &format!("session {session} to contain {needle:?}"),
        timeout,
        || {
            let api = api.clone();
            let session = session.clone();
            let needle = needle.clone();
            async move {
                let detail = api.session(&session).await?;
                if text_of(&detail).contains(&needle) {
                    Ok(())
                } else {
                    Err(anyhow!("text not present yet"))
                }
            }
        },
    )
    .await
}

async fn approve_pending(api: &fleet_e2e_tests::Api, session: &str) -> Result<()> {
    let api = api.clone();
    let session = session.to_owned();
    let permission = wait_until(
        &format!("a pending permission for {session}"),
        Duration::from_secs(20),
        || {
            let api = api.clone();
            let session = session.clone();
            async move {
                let permissions = api.permissions(&session).await?;
                permissions
                    .into_iter()
                    .find(|permission| {
                        permission.status == fleet_core_protocol::agent::PermissionStatus::Pending
                    })
                    .ok_or_else(|| anyhow!("no pending permission yet"))
            }
        },
    )
    .await?;
    api.approve_permission(&permission.session_id, &permission.id)
        .await?;
    wait_for_text(&api, &session, "Tool finished.", Duration::from_secs(20)).await
}

async fn wait_for_interrupted(api: &fleet_e2e_tests::Api, session: &str) -> Result<()> {
    let api = api.clone();
    let session = session.to_owned();
    wait_until(
        &format!("session {session} to be interrupted"),
        Duration::from_secs(20),
        || {
            let api = api.clone();
            let session = session.clone();
            async move {
                let detail = api.session(&session).await?;
                match detail.session.status {
                    SessionStatus::Interrupted | SessionStatus::WaitingForInput => Ok(()),
                    status => Err(anyhow!("session is still {status:?}")),
                }
            }
        },
    )
    .await
}

fn text_of(detail: &fleet_core_protocol::rpc::SessionDetail) -> String {
    detail
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect()
}

async fn join_all_ok<F, T>(futures: impl IntoIterator<Item = F>) -> Result<Vec<T>>
where
    F: Future<Output = Result<T>>,
{
    let results = futures::future::join_all(futures).await;
    results.into_iter().collect()
}
