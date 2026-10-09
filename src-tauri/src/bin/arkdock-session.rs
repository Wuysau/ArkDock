//! Optional, explicitly invoked helper. It never changes a shell profile or launches build tools.
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use arkdock_core::discovery::registration::{
    default_directory, new_session_id, process_started_at, read_record, revoke, session_path,
    ttl_expires_at, write_record, RegistrationRecord,
};
use arkdock_core::ipc::model::{Environment, ProcessRef};

fn main() {
    if let Err(error) = run() {
        eprintln!(
            "{}",
            serde_json::json!({"status":"BLOCKED", "message":error})
        );
        std::process::exit(3);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || matches!(args[0].as_str(), "--help" | "help") {
        println!("arkdock-session register --workspace PATH --target-id TOKEN --target-pid PID --target-started-at TOKEN --pid SENDER_PID [--environment windowsLocal|wsl|remote] [--session-id ID] [--ttl-ms 30000]\narkdock-session heartbeat --session-id ID --pid SENDER_PID [--ttl-ms 30000]\narkdock-session revoke --session-id ID\n\nWindows only. File registrations are WEAK candidates; authentication and active-tab proof are unavailable.");
        return Ok(());
    }
    let directory = default_directory().ok_or(
        "Session helper requires Windows and LOCALAPPDATA; unsupported platforms cannot register",
    )?;
    let command = args[0].as_str();
    let options = options(&args[1..])?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis()
        .try_into()
        .map_err(|_| "Current timestamp exceeds u64")?;
    match command {
        "register" => {
            reject_unknown(
                &options,
                &[
                    "workspace",
                    "target-id",
                    "target-pid",
                    "target-started-at",
                    "pid",
                    "environment",
                    "session-id",
                    "ttl-ms",
                ],
            )?;
            let sender_pid = number::<u32>(&options, "pid")?;
            let target_pid = number::<u32>(&options, "target-pid")?;
            let target_started_at = number::<u64>(&options, "target-started-at")?;
            if process_started_at(target_pid)? != target_started_at {
                return Err(
                    "Target process lifecycle no longer matches the supplied ArkDock snapshot"
                        .into(),
                );
            }
            let environment = match options
                .get("environment")
                .map(String::as_str)
                .unwrap_or("windowsLocal")
            {
                "windowsLocal" => Environment::WindowsLocal,
                "wsl" => Environment::Wsl,
                "remote" => Environment::Remote,
                _ => {
                    return Err("Unknown environment; expected windowsLocal, wsl, or remote".into())
                }
            };
            let target_id = required(&options, "target-id")?.to_owned();
            let session_id = options
                .get("session-id")
                .cloned()
                .unwrap_or_else(|| new_session_id(&target_id, sender_pid, now));
            let ttl_ms = optional_number(&options, "ttl-ms", 30_000)?;
            let record = RegistrationRecord {
                version: 1,
                session_id: session_id.clone(),
                target_id,
                target_process: ProcessRef {
                    pid: target_pid,
                    started_at: target_started_at.to_string(),
                },
                sender_process: ProcessRef {
                    pid: sender_pid,
                    started_at: process_started_at(sender_pid)?.to_string(),
                },
                environment,
                workspace_path: required(&options, "workspace")?.to_owned(),
                observed_at: now,
                expires_at: ttl_expires_at(now, ttl_ms)?,
            };
            write_record(&directory, &record, now)?;
            println!(
                "{}",
                serde_json::json!({"status":"WEAK", "sessionId":session_id, "expiresAt":record.expires_at,
                "message":"Candidate registered; authenticated active-scope association is unavailable"})
            );
        }
        "heartbeat" => {
            reject_unknown(&options, &["session-id", "pid", "ttl-ms"])?;
            let session_id = required(&options, "session-id")?;
            let mut record = read_record(&session_path(&directory, session_id)?)?;
            if record.session_id != session_id {
                return Err("Stored record does not match requested session".into());
            }
            let sender_pid = number::<u32>(&options, "pid")?;
            if record.sender_process.pid != sender_pid
                || process_started_at(sender_pid)?.to_string() != record.sender_process.started_at
            {
                return Err(
                    "Heartbeat sender process lifecycle does not match registered sender".into(),
                );
            }
            record.observed_at = now;
            record.expires_at = ttl_expires_at(now, optional_number(&options, "ttl-ms", 30_000)?)?;
            write_record(&directory, &record, now)?;
            println!(
                "{}",
                serde_json::json!({"status":"WEAK", "sessionId":session_id, "expiresAt":record.expires_at})
            );
        }
        "revoke" => {
            reject_unknown(&options, &["session-id"])?;
            let session_id = required(&options, "session-id")?;
            revoke(&directory, session_id)?;
            println!(
                "{}",
                serde_json::json!({"status":"REVOKED", "sessionId":session_id})
            );
        }
        _ => return Err("Unknown command; expected register, heartbeat, or revoke".into()),
    }
    Ok(())
}

fn options(args: &[String]) -> Result<BTreeMap<String, String>, String> {
    if args.len() % 2 != 0 {
        return Err("Every option requires exactly one value".into());
    }
    let mut options = BTreeMap::new();
    for pair in args.chunks_exact(2) {
        let key = pair[0]
            .strip_prefix("--")
            .filter(|key| !key.is_empty())
            .ok_or("Expected a named --option")?;
        if options.insert(key.to_owned(), pair[1].clone()).is_some() {
            return Err(format!("Duplicate --{key}"));
        }
    }
    Ok(options)
}

fn reject_unknown(options: &BTreeMap<String, String>, allowed: &[&str]) -> Result<(), String> {
    if let Some(key) = options.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("Unknown option --{key}"));
    }
    Ok(())
}

fn required<'a>(options: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, String> {
    options
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("Missing --{name}"))
}

fn number<T: std::str::FromStr>(
    options: &BTreeMap<String, String>,
    name: &str,
) -> Result<T, String> {
    required(options, name)?
        .parse()
        .map_err(|_| format!("Invalid number for --{name}"))
}

fn optional_number(
    options: &BTreeMap<String, String>,
    name: &str,
    default: u64,
) -> Result<u64, String> {
    options
        .get(name)
        .map(|value| {
            value
                .parse()
                .map_err(|_| format!("Invalid number for --{name}"))
        })
        .unwrap_or(Ok(default))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structured_arguments_preserve_literal_workspace_text() {
        let args = vec!["--workspace".into(), "C:\\space dir\\$(literal);app".into()];
        assert_eq!(
            options(&args).unwrap()["workspace"],
            "C:\\space dir\\$(literal);app"
        );
    }
    #[test]
    fn duplicate_missing_and_unknown_flags_are_rejected() {
        assert!(options(&["--pid".into()]).is_err());
        assert!(options(&["--pid".into(), "1".into(), "--pid".into(), "2".into()]).is_err());
        assert!(reject_unknown(
            &options(&["--execute".into(), "command".into()]).unwrap(),
            &["workspace"]
        )
        .is_err());
    }
}
