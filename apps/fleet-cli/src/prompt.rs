//! Interactive prompt helpers for setup workflows.
//!
//! Every helper falls back to a sane default when prompts cannot be shown
//! (`--yes`, or stdin/stdout is not a terminal), so setup commands stay
//! scriptable.

use std::io::{self, BufRead, IsTerminal, Write};

use anyhow::{bail, Context, Result};

/// Options that control whether a workflow may prompt the user.
#[derive(Debug, Clone, Copy)]
pub struct PromptOptions {
    /// Accept every default without asking.
    pub yes: bool,
    /// Report the plan instead of applying it.
    pub dry_run: bool,
}

impl PromptOptions {
    /// True when prompts can be shown and the user has not passed `--yes`.
    pub fn interactive(&self) -> bool {
        !self.yes && io::stdin().is_terminal() && io::stdout().is_terminal()
    }
}

/// Ask for a line of text, returning `default` when the answer is empty or
/// prompting is disabled.
pub fn input(o: PromptOptions, question: &str, default: &str) -> Result<String> {
    if o.yes || !o.interactive() {
        return Ok(default.to_string());
    }
    if default.is_empty() {
        print!("{question}: ");
    } else {
        print!("{question} [{default}]: ");
    }
    io::stdout().flush().context("failed to flush stdout")?;
    let mut line = String::new();
    if io::stdin()
        .lock()
        .read_line(&mut line)
        .context("failed to read stdin")?
        == 0
    {
        bail!("stdin closed");
    }
    let answer = line.trim();
    Ok(if answer.is_empty() {
        default.to_string()
    } else {
        answer.to_string()
    })
}

/// Ask a yes/no question, returning `default` when prompting is disabled.
///
/// `--yes` always answers yes, which is what destructive-confirmation call
/// sites expect.
pub fn confirm(o: PromptOptions, question: &str, default: bool) -> Result<bool> {
    if o.yes {
        return Ok(true);
    }
    if !o.interactive() {
        return Ok(default);
    }
    loop {
        let hint = if default { "Y/n" } else { "y/N" };
        print!("{question} [{hint}]: ");
        io::stdout().flush().context("failed to flush stdout")?;
        let mut line = String::new();
        if io::stdin()
            .lock()
            .read_line(&mut line)
            .context("failed to read stdin")?
            == 0
        {
            bail!("stdin closed");
        }
        if let Some(answer) = parse_yes_no(&line, default) {
            return Ok(answer);
        }
        eprintln!("please answer yes or no");
    }
}

/// Ask the user to pick one of `choices`, returning its zero-based index.
pub fn select(o: PromptOptions, question: &str, choices: &[&str], default: usize) -> Result<usize> {
    if o.yes || !o.interactive() {
        return Ok(default);
    }
    loop {
        println!("{question}:");
        for (index, choice) in choices.iter().enumerate() {
            println!("  {}) {choice}", index + 1);
        }
        print!("Choose [{}]: ", default + 1);
        io::stdout().flush().context("failed to flush stdout")?;
        let mut line = String::new();
        if io::stdin()
            .lock()
            .read_line(&mut line)
            .context("failed to read stdin")?
            == 0
        {
            bail!("stdin closed");
        }
        if let Some(choice) = parse_choice(&line, choices.len(), default) {
            return Ok(choice);
        }
        eprintln!("please choose between 1 and {}", choices.len());
    }
}

/// Read a secret without echoing it.
pub fn secret(question: &str) -> Result<String> {
    rpassword::prompt_password(format!("{question}: ")).context("failed to read secret")
}

/// Parse a one-based menu choice. Empty input selects `default`; out-of-range
/// or non-numeric input returns `None`.
fn parse_choice(input: &str, len: usize, default: usize) -> Option<usize> {
    let input = input.trim();
    if input.is_empty() {
        return (default < len).then_some(default);
    }
    let choice: usize = input.parse().ok()?;
    if (1..=len).contains(&choice) {
        Some(choice - 1)
    } else {
        None
    }
}

/// Parse a yes/no answer. Empty input selects `default`; unrecognized input
/// returns `None`.
fn parse_yes_no(input: &str, default: bool) -> Option<bool> {
    match input.trim().to_ascii_lowercase().as_str() {
        "" => Some(default),
        "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_parsing_is_one_based_and_bounded() {
        assert_eq!(parse_choice("1", 3, 0), Some(0));
        assert_eq!(parse_choice("3", 3, 0), Some(2));
        assert_eq!(parse_choice("4", 3, 0), None);
        assert_eq!(parse_choice("0", 3, 0), None);
        assert_eq!(parse_choice("nope", 3, 0), None);
        assert_eq!(parse_choice("  2  ", 3, 0), Some(1));
    }

    #[test]
    fn choice_empty_input_uses_default() {
        assert_eq!(parse_choice("", 3, 1), Some(1));
        assert_eq!(parse_choice("", 0, 0), None);
    }

    #[test]
    fn yes_no_defaults_and_answers() {
        assert_eq!(parse_yes_no("", true), Some(true));
        assert_eq!(parse_yes_no("", false), Some(false));
        assert_eq!(parse_yes_no("yes", false), Some(true));
        assert_eq!(parse_yes_no("Y", false), Some(true));
        assert_eq!(parse_yes_no("no", true), Some(false));
        assert_eq!(parse_yes_no("N", true), Some(false));
        assert_eq!(parse_yes_no("maybe", true), None);
    }

    #[test]
    fn yes_short_circuits_prompts() {
        let options = PromptOptions {
            yes: true,
            dry_run: false,
        };
        assert!(confirm(options, "Continue?", false).unwrap());
        assert_eq!(input(options, "Name", "default").unwrap(), "default");
        assert_eq!(select(options, "Pick", &["a", "b"], 0).unwrap(), 0);
    }
}
