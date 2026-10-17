//! Native MCP entry point. Discovery works without a running desktop process.
pub mod auth;
mod catalog;
pub mod hub;
pub mod pipe;
mod stdio;
mod wait;

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

/// The executable path this backend reports to an MCP client configuration.
///
/// The user pastes this value into another tool's configuration, where quoting,
/// JSON/TOML escaping and host-side normalization are error-prone, so it must be the
/// ordinary spelling rather than the extended-length one a canonical path can carry.
/// This is the same rule the retired file-pairing store applied to its exported path
/// (`reported_path` in the former `context_pairing_store.rs`), and it mirrors
/// `src/path-display.ts::displayPath` on the frontend. Only the reported value changes.
fn reported_path(value: &str) -> String {
    #[cfg(windows)]
    {
        ordinary_windows_path(value)
    }
    #[cfg(not(windows))]
    {
        value.to_owned()
    }
}

/// Pure form of the Windows rule, so it is testable on every host.
///
/// `\\?\C:\dir\x.exe` becomes `C:\dir\x.exe`, and `\\?\UNC\server\share\x.exe` becomes
/// `\\server\share\x.exe` (a network remainder needs both a server and a share). Device
/// and volume namespaces (`\\?\Volume{...}`, `\\?\GLOBALROOT\...`) have no ordinary
/// equivalent and stay unchanged, as does anything that is not an extended-length path.
#[cfg_attr(not(windows), allow(dead_code))]
fn ordinary_windows_path(value: &str) -> String {
    let Some(remainder) = value.strip_prefix(r"\\?\") else {
        return value.to_owned();
    };
    if let Some(network) = remainder.strip_prefix(r"UNC\") {
        // `\\?\UNC\server\share` names the same file as `\\server\share`; a remainder
        // without both parts is not a network path and is left alone.
        let mut parts = network.split('\\');
        let (Some(server), Some(share)) = (parts.next(), parts.next()) else {
            return value.to_owned();
        };
        if server.is_empty() || share.is_empty() {
            return value.to_owned();
        }
        return format!(r"\\{network}");
    }
    let mut characters = remainder.chars();
    let drive_letter = matches!(
        (characters.next(), characters.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    );
    if drive_letter {
        remainder.to_owned()
    } else {
        value.to_owned()
    }
}

pub fn configuration() -> Result<McpConfiguration, String> {
    let command = reported_path(
        &std::env::current_exe()
            .map_err(|_| "MCP_EXECUTABLE_UNAVAILABLE")?
            .to_string_lossy(),
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reported_executable_path_is_the_ordinary_spelling_not_the_extended_length_one() {
        // A canonical Windows path must never reach a client configuration as `\\?\...`.
        assert_eq!(
            ordinary_windows_path(r"\\?\C:\dir\arkdock.exe"),
            r"C:\dir\arkdock.exe"
        );
        assert_eq!(
            ordinary_windows_path(r"\\?\UNC\server\share\arkdock.exe"),
            r"\\server\share\arkdock.exe"
        );
    }

    #[test]
    fn a_path_without_an_ordinary_equivalent_is_reported_unchanged() {
        for value in [
            r"C:\dir\arkdock.exe",
            r"\\server\share\arkdock.exe",
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\arkdock.exe",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\arkdock.exe",
            // A network remainder needs both a server and a share.
            r"\\?\UNC\server",
            r"\\?\UNC\server\",
            r"\\?\relative\path",
            "",
        ] {
            assert_eq!(ordinary_windows_path(value), value, "{value}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_reported_path_applies_the_windows_rule_and_still_names_the_same_file() {
        assert_eq!(
            reported_path(r"\\?\C:\dir\arkdock.exe"),
            r"C:\dir\arkdock.exe"
        );
        assert_eq!(reported_path(r"C:\dir\arkdock.exe"), r"C:\dir\arkdock.exe");
    }
}
