//! Role setup workflows.

use std::process::Command as ProcessCommand;

use anyhow::{bail, Context, Result};

use crate::prompt::{self, PromptOptions};

pub mod admin;
pub mod daemon;

/// The role an interactive `fleet setup` dispatches to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupRole {
    Admin,
    Daemon,
    Cli,
}

/// Build the argv for the role-specific setup command. `--yes` is only
/// forwarded to the admin and daemon workflows; `config init` has no such
/// flag.
pub fn child_args(role: SetupRole, yes: bool, args: &[String]) -> Vec<String> {
    let mut out: Vec<String> = match role {
        SetupRole::Admin => vec!["admin".into(), "setup".into()],
        SetupRole::Daemon => vec!["daemon".into(), "setup".into()],
        SetupRole::Cli => vec!["config".into(), "init".into()],
    };
    if yes && role != SetupRole::Cli {
        out.push("--yes".into());
    }
    out.extend(args.iter().cloned());
    out
}

fn prompt_role(yes: bool) -> Result<SetupRole> {
    let choice = prompt::select(
        PromptOptions {
            yes,
            dry_run: false,
        },
        "What would you like to set up?",
        &[
            "admin (control plane)",
            "daemon (worker machine)",
            "cli (admin URL and token)",
        ],
        0,
    )?;
    Ok(match choice {
        0 => SetupRole::Admin,
        1 => SetupRole::Daemon,
        _ => SetupRole::Cli,
    })
}

/// Prompt for a role when one was not given, then re-exec the current `fleet`
/// binary with the role-specific setup arguments and inherited stdio.
pub async fn dispatch(role: Option<SetupRole>, yes: bool, args: Vec<String>) -> Result<()> {
    let role = match role {
        Some(role) => role,
        None => prompt_role(yes)?,
    };
    let exe = std::env::current_exe().context("failed to locate the fleet executable")?;
    let child = child_args(role, yes, &args);
    println!("running {} {}", exe.display(), child.join(" "));
    let status = ProcessCommand::new(&exe)
        .args(&child)
        .status()
        .with_context(|| format!("failed to run {}", exe.display()))?;
    if !status.success() {
        bail!("`fleet {}` exited with status {}", child.join(" "), status);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn child_args_maps_each_role() {
        assert_eq!(
            child_args(SetupRole::Admin, false, &[]),
            strings(&["admin", "setup"])
        );
        assert_eq!(
            child_args(SetupRole::Daemon, false, &[]),
            strings(&["daemon", "setup"])
        );
        assert_eq!(
            child_args(SetupRole::Cli, false, &[]),
            strings(&["config", "init"])
        );
    }

    #[test]
    fn child_args_forwards_yes_only_for_admin_and_daemon() {
        assert_eq!(
            child_args(SetupRole::Admin, true, &[]),
            strings(&["admin", "setup", "--yes"])
        );
        assert_eq!(
            child_args(SetupRole::Daemon, true, &[]),
            strings(&["daemon", "setup", "--yes"])
        );
        assert_eq!(
            child_args(SetupRole::Cli, true, &[]),
            strings(&["config", "init"])
        );
    }

    #[test]
    fn child_args_appends_forwarded_args() {
        assert_eq!(
            child_args(
                SetupRole::Daemon,
                true,
                &strings(&["--state-dir", "/tmp/fleet", "--name", "box"]),
            ),
            strings(&[
                "daemon",
                "setup",
                "--yes",
                "--state-dir",
                "/tmp/fleet",
                "--name",
                "box"
            ])
        );
    }
}
