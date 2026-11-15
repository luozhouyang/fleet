//! Command specification and conversion to the runtime command builders.

use std::collections::BTreeMap;
use std::path::PathBuf;

use portable_pty::CommandBuilder;
use tokio::process::Command;

/// A fully resolved description of a command to run.
///
/// Commands are always represented as a program plus an argument vector; no
/// shell interpolation is performed. This keeps user-controlled values from
/// being interpreted as shell syntax.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    /// Executable name or path.
    pub program: String,
    /// Arguments passed verbatim to the executable.
    pub args: Vec<String>,
    /// Working directory for the child process.
    pub cwd: Option<PathBuf>,
    /// Environment variables added on top of the inherited environment.
    pub env: BTreeMap<String, String>,
    /// When `true`, the child starts with an empty environment.
    pub clear_env: bool,
}

impl CommandSpec {
    /// Create a spec for `program` with no arguments.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            clear_env: false,
        }
    }

    /// Append a single argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append several arguments.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set the child working directory.
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Set an environment variable for the child.
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Start the child from an empty environment.
    #[must_use]
    pub fn clear_env(mut self) -> Self {
        self.clear_env = true;
        self
    }

    /// Build a Tokio command. The caller is responsible for configuring I/O.
    ///
    /// On Unix the child is placed in a new process group so that the whole
    /// process tree can be signalled later.
    pub(crate) fn to_tokio_command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        if self.clear_env {
            command.env_clear();
        }
        command.envs(&self.env);

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // `process_group(0)` asks the kernel to make the child the leader
            // of a new process group whose id equals the child pid.
            command.as_std_mut().process_group(0);
        }

        command
    }

    /// Build a portable-pty command builder.
    pub(crate) fn to_pty_command(&self) -> CommandBuilder {
        let mut builder = CommandBuilder::new(&self.program);
        for arg in &self.args {
            builder.arg(arg);
        }
        if let Some(cwd) = &self.cwd {
            builder.cwd(cwd);
        }
        if self.clear_env {
            builder.env_clear();
        }
        for (key, value) in &self.env {
            builder.env(key, value);
        }
        builder
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_collects_fields() {
        let spec = CommandSpec::new("git")
            .arg("status")
            .args(["--porcelain", "-z"])
            .cwd("/tmp")
            .env("A", "1")
            .clear_env();
        assert_eq!(spec.program, "git");
        assert_eq!(spec.args, vec!["status", "--porcelain", "-z"]);
        assert_eq!(spec.cwd.as_deref(), Some(std::path::Path::new("/tmp")));
        assert_eq!(spec.env.get("A").map(String::as_str), Some("1"));
        assert!(spec.clear_env);
    }
}
