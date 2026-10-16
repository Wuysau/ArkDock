//! Public HDC lifecycle: attach to a matching existing server using `-p`, or
//! own a foreground `-m` child. Never restart/kill shared servers by name.
use super::plans::{command_plan, parse_target_stdout, HdcCommandPlan, HdcOperation};
use super::process::{check_cancel, OwnedToolProcess, ToolCommandRunner, ToolOutput};
use crate::ipc::model::DeviceRuntimeTarget;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

pub struct HdcSession {
    tool: PathBuf,
    endpoint: String,
    server: Option<OwnedToolProcess>,
    version: String,
}

impl HdcSession {
    /// Called only after connection consent. Merely constructing the adapter or
    /// reading its state must not call this method.
    pub fn connect(tool: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let port = match std::env::var("OHOS_HDC_SERVER_PORT") {
            Ok(value) => value
                .parse::<u16>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or("INVALID_HDC_SERVER_PORT")?,
            Err(std::env::VarError::NotPresent) => 8710,
            Err(_) => return Err("INVALID_HDC_SERVER_PORT".into()),
        };
        Self::connect_at(tool, &format!("127.0.0.1:{port}"), cancel)
    }

    pub fn connect_at(tool: &Path, endpoint: &str, cancel: &AtomicBool) -> Result<Self, String> {
        check_cancel(cancel)?;
        let address = loopback_endpoint(endpoint)?;
        if !tool.is_absolute() || !tool.is_file() {
            return Err("HDC_TOOL_MISSING".into());
        }
        let cwd = tool.parent().ok_or("HDC_TOOL_DIRECTORY")?;
        let local = ToolCommandRunner::run(
            tool,
            &["-v".into()],
            cwd,
            &[],
            Duration::from_secs(5),
            cancel,
        )?;
        ensure_transport_success(&local)?;
        let version = parse_version(local.stdout_text()?)?;
        let mut session = Self {
            tool: tool.into(),
            endpoint: endpoint.into(),
            server: None,
            version,
        };
        let existing = match TcpStream::connect_timeout(&address, Duration::from_millis(300)) {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => false,
            Err(e) => return Err(format!("HDC_SERVER_ENDPOINT_UNAVAILABLE: {e}")),
        };
        if existing {
            session.verify_server(cancel)?;
            return Ok(session);
        }
        // A server elsewhere may own HDC's global mutex. Let the official tool
        // reject this spawn; never stop it or try to bypass the mutex.
        session.server = Some(OwnedToolProcess::spawn_service(
            tool,
            &[
                "-s".into(),
                endpoint.into(),
                "-l".into(),
                "2".into(),
                "-m".into(),
            ],
            cwd,
            &[],
        )?);
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            check_cancel(cancel)?;
            let server = session.server.as_ref().unwrap();
            server.health()?;
            if let Some(status) = server.exit_code()? {
                let diagnostics = service_diagnostic(server);
                return Err(format!(
                    "HDC_SERVER_START_FAILED: exit {status}; {diagnostics}"
                ));
            }
            if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
                session.verify_server(cancel)?;
                // Verify our child did not lose a bind/mutex race to another
                // server between the connect and the protocol exchange.
                if session.server.as_ref().unwrap().exit_code()?.is_some() {
                    return Err("HDC_SERVER_START_RACE".into());
                }
                return Ok(session);
            }
            if Instant::now() >= deadline {
                return Err("HDC_SERVER_START_TIMEOUT".into());
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    pub fn execute(
        &mut self,
        plan: &HdcCommandPlan,
        cancel: &AtomicBool,
    ) -> Result<ToolOutput, String> {
        self.check_health()?;
        let output = self.client(
            &plan.args,
            Duration::from_millis(plan.timeout_ms),
            plan.stdout_limit,
            cancel,
        )?;
        ensure_transport_success(&output)?;
        Ok(output)
    }

    pub fn discover(&mut self, cancel: &AtomicBool) -> Result<Vec<DeviceRuntimeTarget>, String> {
        let output = self.execute(&command_plan(HdcOperation::ListTargets)?, cancel)?;
        parse_target_stdout(&output.stdout)
    }

    /// Used only for a selected emulator's explicit endpoint (or an explicitly
    /// authorized local connection). This is not a LAN scan or default target.
    pub fn connect_target(&mut self, endpoint: &str, cancel: &AtomicBool) -> Result<(), String> {
        loopback_endpoint(endpoint)?;
        self.check_health()?;
        let output = self.client(
            &["tconn".into(), endpoint.into()],
            Duration::from_secs(8),
            64 * 1024,
            cancel,
        )?;
        ensure_transport_success(&output)?;
        // tconn receipt alone does not establish a connected target. Always
        // demand that exact connect key in an authoritative new enumeration.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            check_cancel(cancel)?;
            if self
                .discover(cancel)?
                .iter()
                .any(|t| t.id == endpoint && t.connection_state == "Connected")
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("HDC_TARGET_CONNECT_TIMEOUT".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn check_health(&self) -> Result<(), String> {
        if let Some(server) = &self.server {
            server.health()?;
            if server.exit_code()?.is_some() {
                return Err(format!(
                    "HDC_OWNED_SERVER_EXITED: {}",
                    service_diagnostic(server)
                ));
            }
        }
        Ok(())
    }

    fn client(
        &self,
        args: &[String],
        timeout: Duration,
        limit: usize,
        cancel: &AtomicBool,
    ) -> Result<ToolOutput, String> {
        let mut command = vec!["-p".into(), "-s".into(), self.endpoint.clone()];
        command.extend_from_slice(args);
        ToolCommandRunner::run_limited(
            &self.tool,
            &command,
            self.tool.parent().ok_or("HDC_TOOL_DIRECTORY")?,
            &[],
            timeout,
            cancel,
            limit,
            64 * 1024,
        )
    }

    fn verify_server(&self, cancel: &AtomicBool) -> Result<(), String> {
        let output = self.client(
            &["checkserver".into()],
            Duration::from_secs(5),
            8192,
            cancel,
        )?;
        ensure_transport_success(&output)?;
        let (client, server) = parse_server_versions(output.stdout_text()?)?;
        if client != self.version || server != self.version {
            return Err("HDC_SERVER_VERSION_MISMATCH: 请使用与现有服务匹配的 SDK；ArkDock 不会重启共享服务。".into());
        }
        Ok(())
    }
}

fn service_diagnostic(process: &OwnedToolProcess) -> String {
    let stderr = process.stderr();
    let stdout = process.stdout();
    let bytes = if stderr.is_empty() { stdout } else { stderr };
    String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(2048)..]).into_owned()
}

/// Server endpoints must be exact IPv4 loopback addresses. No DNS, wildcard
/// listener, remote server, inferred port, zero or overflow is accepted.
pub fn loopback_endpoint(value: &str) -> Result<SocketAddr, String> {
    let suffix = value
        .strip_prefix("127.0.0.1:")
        .ok_or("INVALID_HDC_LOOPBACK_ENDPOINT")?;
    if suffix.is_empty() || suffix.len() > 5 || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return Err("INVALID_HDC_LOOPBACK_ENDPOINT".into());
    }
    let port = suffix
        .parse::<u16>()
        .ok()
        .filter(|p| *p > 0)
        .ok_or("INVALID_HDC_LOOPBACK_ENDPOINT")?;
    Ok(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)))
}

/// Exit(0) is not success in HDC. These are transport-level failures only;
/// install/start/input/capture still need their own positive operation receipt.
pub fn ensure_transport_success(output: &ToolOutput) -> Result<(), String> {
    if output.status != 0 {
        return Err(format!(
            "HDC_CLIENT_EXIT: {}; {}",
            output.status,
            output.diagnostic()
        ));
    }
    for bytes in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(bytes);
        if text.lines().any(|line| {
            let line = line.trim().to_ascii_lowercase();
            line.starts_with("[fail]")
                || line.starts_with("[error]")
                || line.starts_with("error:")
                || line.starts_with("connect server failed")
                || line.starts_with("initial failed")
                || line.starts_with("[empty]no target")
                || line.starts_with("no any target")
        }) {
            return Err(format!("HDC_COMMAND_FAILED: {}", output.diagnostic()));
        }
    }
    Ok(())
}

fn parse_version(value: &str) -> Result<String, String> {
    let value = value
        .trim()
        .strip_prefix("Ver:")
        .ok_or("HDC_VERSION_RECEIPT_MISSING")?
        .trim();
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err("HDC_VERSION_FORMAT".into());
    }
    Ok(value.to_owned())
}

fn parse_server_versions(value: &str) -> Result<(String, String), String> {
    let value = value
        .trim()
        .strip_prefix("Client version:")
        .ok_or("HDC_SERVER_RECEIPT_MISSING")?;
    let (client, server) = value
        .split_once(", Server version:")
        .ok_or("HDC_SERVER_RECEIPT_MISSING")?;
    Ok((parse_version(client)?, parse_version(server)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deceptive_zero_exit_and_unexpected_version_are_rejected() {
        assert!(ensure_transport_success(&ToolOutput {
            status: 0,
            stdout: b"Connect server failed.\n".to_vec(),
            stderr: vec![]
        })
        .is_err());
        assert!(parse_server_versions(
            "Client version: Ver: 3.1.0e, Server version: Ver: 3.1.0e\n[Fail] bad"
        )
        .is_err());
        assert_eq!(
            parse_server_versions("Client version: Ver: 3.1.0e, Server version: Ver: 3.1.0e\r\n")
                .unwrap(),
            ("3.1.0e".into(), "3.1.0e".into())
        );
    }
    #[test]
    fn endpoint_has_no_remote_or_shell_escape() {
        assert!(loopback_endpoint("127.0.0.1:8710").is_ok());
        for value in [
            "0.0.0.0:8710",
            "localhost:8710",
            "127.0.0.1:0",
            "127.0.0.1:65536",
            "127.0.0.1:8710;echo",
            "127.0.0.1:-1",
        ] {
            assert!(loopback_endpoint(value).is_err(), "{value}");
        }
    }
}
