//! Native MCP entry point. Discovery works without a running desktop process.
pub mod auth;
mod catalog;
pub mod hub;
pub mod pipe;
mod stdio;

use crate::ipc::model::{McpConfiguration, ProcessRef};

#[derive(Clone, Debug)]
pub struct ConnectionOwner {
    pub connection_id: u64,
    pub process: ProcessRef,
    pub client_name: String,
}

pub fn run_stdio() -> Result<(), String> {
    stdio::run()
}

pub fn configuration() -> Result<McpConfiguration, String> {
    let command = std::env::current_exe()
        .map_err(|_| "MCP_EXECUTABLE_UNAVAILABLE")?
        .to_string_lossy()
        .into_owned();
    let args = vec!["--mcp".to_owned()];
    let json = serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {"arkdock": {"command": command, "args": args}}
    }))
    .map_err(|_| "MCP_CONFIGURATION_INVALID")?;
    Ok(McpConfiguration {
        command,
        args,
        json,
    })
}

pub(crate) const MAX_REQUEST: usize = 64 * 1024;
pub(crate) const MAX_RESPONSE: usize = 4 * 1024 * 1024;

pub(crate) fn tool_error(message: &str) -> serde_json::Value {
    serde_json::json!({"content":[{"type":"text","text":message}],"isError":true})
}
