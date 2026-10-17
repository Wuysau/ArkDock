//! A bounded wait in the stdio process, never in the desktop actor. Each short
//! probe uses the existing atomic, trusted claim on the same native connection.
use super::tool_error;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const MAX_TIMEOUT_MS: u64 = 25_000;
const POLL: Duration = Duration::from_secs(1);
const CANCEL_POLL: Duration = Duration::from_millis(50);

pub(super) fn run(
    arguments: Value,
    cancelled: &AtomicBool,
    stopped: &AtomicBool,
    mut claim: impl FnMut(Value) -> Value,
) -> Value {
    let (context, timeout) = match parse(&arguments) {
        Ok(request) => request,
        Err(error) => return tool_error(error),
    };
    let deadline = Instant::now() + timeout;
    loop {
        if cancelled.load(Ordering::Acquire) || stopped.load(Ordering::Acquire) {
            return tool_error("MCP_REQUEST_CANCELLED");
        }
        let result = claim(json!({"contextId":context}));
        // Only an authoritative empty queue permits another attempt. Transport
        // failures, uncertain results, revoked authority and disconnects stop.
        if result["isError"] != true
            || result["content"].as_array().is_none_or(|content| {
                content.len() != 1 || content[0]["text"] != "MCP_TASK_UNAVAILABLE"
            })
        {
            return result;
        }
        let next = (Instant::now() + POLL).min(deadline);
        while Instant::now() < next {
            if cancelled.load(Ordering::Acquire) || stopped.load(Ordering::Acquire) {
                return tool_error("MCP_REQUEST_CANCELLED");
            }
            std::thread::sleep(CANCEL_POLL.min(next.saturating_duration_since(Instant::now())));
        }
        if Instant::now() >= deadline {
            let value = json!({"status":"waiting","contextId":context,"timeoutMs":timeout.as_millis(),"message":"No task was claimed. In the user-enabled automatic processing loop, call arkdock_wait_for_task again. Stop on user cancellation or any error."});
            return json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false});
        }
    }
}

fn parse(arguments: &Value) -> Result<(String, Duration), &'static str> {
    let object = arguments.as_object().ok_or("MCP_ARGUMENTS_INVALID")?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "contextId" | "timeoutMs"))
    {
        return Err("MCP_ARGUMENTS_INVALID");
    }
    let context = object
        .get("contextId")
        .and_then(Value::as_str)
        .filter(|context| !context.is_empty() && context.len() <= 4096)
        .ok_or("MCP_ARGUMENT_REQUIRED")?;
    let timeout = match object.get("timeoutMs") {
        None => MAX_TIMEOUT_MS,
        Some(value) => value
            .as_u64()
            .filter(|timeout| *timeout <= MAX_TIMEOUT_MS)
            .ok_or("MCP_ARGUMENT_INVALID")?,
    };
    Ok((context.into(), Duration::from_millis(timeout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_queue_times_out_without_claiming_or_signalling_execution() {
        let mut probes = 0;
        let result = run(
            json!({"contextId":"context","timeoutMs":10}),
            &AtomicBool::new(false),
            &AtomicBool::new(false),
            |args| {
                probes += 1;
                assert_eq!(args, json!({"contextId":"context"}));
                tool_error("MCP_TASK_UNAVAILABLE")
            },
        );
        assert_eq!(probes, 1);
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["status"], "waiting");
        assert!(result["structuredContent"].get("claimId").is_none());
    }

    #[test]
    fn errors_and_uncertain_results_are_never_retried() {
        for error in [
            "MCP_CONTEXT_CONFIRMATION_REQUIRED",
            "MCP_CONTEXT_NOT_OWNED",
            "TRUST_REQUIRED",
            "MCP_DESKTOP_UNAVAILABLE",
            "MCP_RESULT_UNKNOWN",
        ] {
            let mut probes = 0;
            let result = run(
                json!({"contextId":"context"}),
                &AtomicBool::new(false),
                &AtomicBool::new(false),
                |_| {
                    probes += 1;
                    tool_error(error)
                },
            );
            assert_eq!(result, tool_error(error));
            assert_eq!(probes, 1);
        }
    }

    #[test]
    fn invalid_arguments_never_reach_the_desktop() {
        for args in [
            json!({}),
            json!({"contextId":""}),
            json!({"contextId":"context","timeoutMs":25_001}),
            json!({"contextId":"context","timeoutMs":-1}),
            json!({"contextId":"context","timeoutMs":1.5}),
            json!({"contextId":"context","timeoutMs":null}),
            json!({"contextId":"context","taskId":"other"}),
        ] {
            assert_eq!(
                run(
                    args,
                    &AtomicBool::new(false),
                    &AtomicBool::new(false),
                    |_| { panic!("invalid wait must not claim") }
                )["isError"],
                true
            );
        }
    }

    #[test]
    fn cancellation_and_shutdown_prevent_a_later_claim() {
        for shutdown in [false, true] {
            let cancelled = AtomicBool::new(false);
            let stopped = AtomicBool::new(false);
            let mut probes = 0;
            let result = run(json!({"contextId":"context"}), &cancelled, &stopped, |_| {
                probes += 1;
                if shutdown { &stopped } else { &cancelled }.store(true, Ordering::Release);
                tool_error("MCP_TASK_UNAVAILABLE")
            });
            assert_eq!(probes, 1);
            assert_eq!(result, tool_error("MCP_REQUEST_CANCELLED"));
        }
    }
}
