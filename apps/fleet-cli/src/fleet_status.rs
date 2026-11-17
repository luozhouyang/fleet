//! Top-level `fleet status`: a read-only summary of both roles.

use anyhow::Result;
use serde_json::{json, Value};

use crate::cli::ProcessMode;
use crate::config_paths::RolePaths;
use crate::process::status;

pub async fn run(admin: &RolePaths, daemon: &RolePaths, as_json: bool) -> Result<i32> {
    let (admin_code, admin_report) = status::collect(admin, ProcessMode::Auto).await?;
    let (daemon_code, daemon_report) = status::collect(daemon, ProcessMode::Auto).await?;
    let code = aggregate(admin_code, daemon_code);

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "admin": admin_report,
                "daemon": daemon_report,
                "ok": code == 0,
            }))?
        );
    } else {
        print_role("admin", &admin_report);
        print_role("daemon", &daemon_report);
    }
    Ok(code)
}

fn aggregate(admin: i32, daemon: i32) -> i32 {
    if admin == 0 && daemon == 0 {
        0
    } else if admin == 2 || daemon == 2 {
        2
    } else {
        1
    }
}

fn print_role(name: &str, report: &Value) {
    println!("{name}");
    println!(
        "  service   {} (user {})",
        text(report, "service_level"),
        text(report, "user")
    );
    println!("  mode      {}", text(report, "mode"));
    if report["healthy"].as_bool().unwrap_or(false) {
        let version = report["health"]
            .get("version")
            .or_else(|| report["health"].get("daemon_version"))
            .and_then(Value::as_str)
            .unwrap_or("");
        println!(
            "  health    ok{}",
            if version.is_empty() {
                String::new()
            } else {
                format!(" ({version})")
            }
        );
    } else {
        println!("  health    unavailable");
    }
    println!("  config    {}", text(report, "config"));
    println!("  database  {}", text(report, "database"));
    if let Some(url) = report["url"].as_str() {
        println!("  url       {url}");
    }
    if let Some(daemon) = report.get("daemon") {
        let machine = daemon["machine_name"].as_str().unwrap_or("unknown");
        println!("  machine   {machine}");
        println!(
            "  paired    {}",
            if daemon["paired"].as_bool().unwrap_or(false) {
                "yes"
            } else {
                "no"
            }
        );
        if let Some(server) = daemon["server_url"].as_str() {
            println!("  server    {server}");
        }
    }
}

fn text<'a>(report: &'a Value, key: &str) -> &'a str {
    report[key].as_str().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_prefers_degraded_over_stopped() {
        assert_eq!(aggregate(0, 0), 0);
        assert_eq!(aggregate(0, 1), 1);
        assert_eq!(aggregate(1, 1), 1);
        assert_eq!(aggregate(0, 2), 2);
        assert_eq!(aggregate(1, 2), 2);
    }
}
