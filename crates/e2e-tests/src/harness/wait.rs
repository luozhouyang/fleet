//! Bounded polling helpers.

use std::fmt::Display;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

/// Default deadline for short readiness checks.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// Poll `attempt` until it succeeds or `timeout` elapses.
///
/// The final error includes every failed attempt's last message, which makes
/// flaky startup failures diagnosable without dumping the whole log.
pub async fn wait_until<T, E, F, Fut>(
    description: &str,
    timeout: Duration,
    mut attempt: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, E>>,
    E: Display,
{
    let started = Instant::now();
    loop {
        let last = match attempt().await {
            Ok(value) => return Ok(value),
            Err(error) => error.to_string(),
        };
        if started.elapsed() >= timeout {
            return Err(anyhow!(
                "timed out after {timeout:?} waiting for {description}; last observation: {last}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
