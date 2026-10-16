//! Official installed Emulator CLI. No image download, licence acceptance,
//! data reset, global settings edit, or ownership inferred from loopback alone.
use super::hdc::loopback_endpoint;
use super::process::{check_cancel, OwnedToolProcess, ToolCommandRunner, ToolOutput};
use std::collections::{BTreeMap, HashSet};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmulatorInstance {
    pub name: String,
    /// True only with live process/CLI evidence; false does not assert that an
    /// unowned instance is stopped. `reason` distinguishes unobserved state.
    pub running: bool,
    pub owned: bool,
    pub hdc_target: Option<String>,
    pub reason: String,
}

struct OwnedInstance {
    process: OwnedToolProcess,
    endpoint: String,
}
pub struct EmulatorManager {
    executable: PathBuf,
    owned: BTreeMap<String, OwnedInstance>,
}

impl EmulatorManager {
    /// Resolve a sibling tool only beneath the selected SDK installation.
    /// Candidate search never traverses beyond four installation ancestors.
    pub fn from_sdk(sdk: &Path) -> Result<Self, String> {
        if !sdk.is_absolute() {
            return Err("EMULATOR_SDK_ROOT_NOT_ABSOLUTE".into());
        }
        let mut candidates = Vec::new();
        for base in sdk.ancestors().take(5) {
            for relative in ["emulator/Emulator.exe", "tools/emulator/Emulator.exe"] {
                let path = base.join(relative);
                if path.is_file() {
                    let path = path
                        .canonicalize()
                        .map_err(|e| format!("EMULATOR_PATH: {e}"))?;
                    if !candidates.contains(&path) {
                        candidates.push(path);
                    }
                }
            }
        }
        if candidates.len() > 1 {
            return Err("EMULATOR_TOOL_AMBIGUOUS: 请指定对应的 SDK 安装。".into());
        }
        let executable = candidates
            .pop()
            .ok_or("EMULATOR_TOOL_MISSING: 请安装官方 Command Line Tools 的 Emulator 组件。")?;
        Ok(Self {
            executable,
            owned: BTreeMap::new(),
        })
    }

    pub fn list(&mut self, cancel: &AtomicBool) -> Result<Vec<EmulatorInstance>, String> {
        let output = self.command(
            &["-list".into(), "-details".into()],
            Duration::from_secs(15),
            cancel,
        )?;
        let mut instances = parse_instances(&output.stdout)?;
        self.owned.retain(|_, owned| {
            owned
                .process
                .active_processes()
                .map(|n| n > 0)
                .unwrap_or(false)
        });
        for instance in &mut instances {
            if let Some(owned) = self.owned.get(&instance.name) {
                instance.running = true;
                instance.owned = true;
                instance.hdc_target = Some(owned.endpoint.clone());
                instance.reason =
                    "ArkDock 启动的模拟器进程仍在运行；设备连接状态需由 HDC 枚举确认。".into();
            }
        }
        Ok(instances)
    }

    /// Only names returned by a fresh official listing can be started. The
    /// explicit port establishes a known endpoint for subsequent HDC evidence.
    pub fn start(
        &mut self,
        name: &str,
        port: u16,
        cancel: &AtomicBool,
    ) -> Result<EmulatorInstance, String> {
        validate_name(name)?;
        check_cancel(cancel)?;
        if !(10000..=16555).contains(&port) {
            return Err("INVALID_EMULATOR_HDC_PORT".into());
        }
        let instances = self.list(cancel)?;
        let instance = instances
            .into_iter()
            .find(|i| i.name == name)
            .ok_or("EMULATOR_INSTANCE_NOT_OBSERVED")?;
        if instance.running {
            return Err("EMULATOR_INSTANCE_ALREADY_RUNNING".into());
        }
        let endpoint = format!("127.0.0.1:{port}");
        // Do not connect to or adopt an existing listener as our new instance.
        let reservation =
            TcpListener::bind(&endpoint).map_err(|e| format!("EMULATOR_PORT_BUSY: {e}"))?;
        let args = vec![
            "-start".into(),
            name.into(),
            "-bootMode".into(),
            "coldboot".into(),
            "-hdcPort".into(),
            port.to_string(),
        ];
        drop(reservation);
        let process = OwnedToolProcess::spawn_service(
            &self.executable,
            &args,
            self.executable.parent().ok_or("EMULATOR_TOOL_DIRECTORY")?,
            &[],
        )?;
        let deadline = Instant::now() + Duration::from_secs(90);
        let address = loopback_endpoint(&endpoint)?;
        loop {
            check_cancel(cancel)?;
            process.health()?;
            reject_failure(&ToolOutput {
                status: 0,
                stdout: process.stdout(),
                stderr: process.stderr(),
            })?;
            if process.active_processes()? == 0 {
                let output = ToolOutput {
                    status: process.exit_code()?.unwrap_or(1),
                    stdout: process.stdout(),
                    stderr: process.stderr(),
                };
                return Err(format!("EMULATOR_START_FAILED: {}", output.diagnostic()));
            }
            if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
                break;
            }
            if Instant::now() >= deadline {
                return Err(
                    "EMULATOR_START_TIMEOUT: 请检查镜像、虚拟化支持和首次使用协议。".into(),
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.owned.insert(
            name.into(),
            OwnedInstance {
                process,
                endpoint: endpoint.clone(),
            },
        );
        Ok(EmulatorInstance {
            name: name.into(),
            running: true,
            owned: true,
            hdc_target: Some(endpoint),
            reason: "模拟器进程已启动；等待 HDC 确认目标连接。".into(),
        })
    }

    pub fn stop_owned(&mut self, name: &str, cancel: &AtomicBool) -> Result<(), String> {
        validate_name(name)?;
        check_cancel(cancel)?;
        let owned = self.owned.get(name).ok_or("EMULATOR_INSTANCE_NOT_OWNED")?;
        if owned.process.active_processes()? == 0 {
            self.owned.remove(name);
            return Ok(());
        }
        // Stop by exclusive Job ownership rather than a reused instance name.
        // This cannot close a DevEco/user-owned process that happened to replace
        // our instance after it exited. No emulator data is removed or reset.
        self.owned.remove(name);
        Ok(())
    }

    fn command(
        &self,
        args: &[String],
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<ToolOutput, String> {
        let output = ToolCommandRunner::run_limited(
            &self.executable,
            args,
            self.executable.parent().ok_or("EMULATOR_TOOL_DIRECTORY")?,
            &[],
            timeout,
            cancel,
            256 * 1024,
            64 * 1024,
        )?;
        reject_failure(&output)?;
        Ok(output)
    }
}

fn reject_failure(output: &ToolOutput) -> Result<(), String> {
    if output.status != 0 {
        return Err(format!("EMULATOR_COMMAND_FAILED: {}", output.diagnostic()));
    }
    for bytes in [&output.stdout, &output.stderr] {
        if String::from_utf8_lossy(bytes).lines().any(|line| {
            let line = line.trim().to_ascii_lowercase();
            line.starts_with("error:")
                || line.starts_with("[error]")
                || line.starts_with("[fail]")
                || line.contains("must accept")
                || line.contains("agreement must be accepted")
        }) {
            return Err(format!("EMULATOR_COMMAND_FAILED: {}", output.diagnostic()));
        }
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), String> {
    // SDK 26 creation rules allow ASCII letters, numbers, underscores, spaces.
    if name.is_empty()
        || name.len() > 128
        || name.trim() != name
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b' '))
    {
        return Err("INVALID_EMULATOR_NAME".into());
    }
    Ok(())
}

/// Accept structured official instance metadata or its INI-style detail
/// listing. Unknown/log/error formats reject the whole observation. Names and
/// states are never inferred from HDC USB/TCP transport or project fixtures.
pub fn parse_instances(stdout: &[u8]) -> Result<Vec<EmulatorInstance>, String> {
    if stdout.len() > 256 * 1024 {
        return Err("EMULATOR_LIST_LIMIT".into());
    }
    let text = std::str::from_utf8(stdout)
        .map_err(|_| "EMULATOR_LIST_ENCODING")?
        .trim();
    if text == "[]" {
        return Ok(Vec::new());
    }
    if text.is_empty() {
        return Err("EMULATOR_LIST_RECEIPT_MISSING".into());
    }
    let mut result = Vec::new();
    if text.starts_with('[') || text.starts_with('{') {
        let json: serde_json::Value =
            serde_json::from_str(text).map_err(|_| "EMULATOR_LIST_FORMAT")?;
        let rows = json
            .as_array()
            .or_else(|| json.get("instances").and_then(|v| v.as_array()))
            .ok_or("EMULATOR_LIST_FORMAT")?;
        for row in rows {
            let name = row
                .get("name")
                .or_else(|| row.get("hvd.name"))
                .and_then(|v| v.as_str())
                .ok_or("EMULATOR_LIST_NAME_MISSING")?;
            let running = row.get("running").and_then(|v| v.as_bool());
            let running = running.or_else(|| {
                row.get("status")
                    .and_then(|v| v.as_str())
                    .and_then(|s| match s {
                        "running" | "Running" => Some(true),
                        "stopped" | "Stopped" => Some(false),
                        _ => None,
                    })
            });
            result.push(instance(name, running)?);
        }
    } else {
        // Detail blocks use the SDK's hvd.name / hardware configuration keys.
        // Reject prose and diagnostic lines instead of mistaking them for names.
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
            if let Some(name) = line.strip_prefix("hvd.name=") {
                result.push(instance(name.trim(), None)?);
            } else if line.split_once('=').is_some_and(|(key, _)| {
                key.starts_with("hw.")
                    || key.starts_with("hvd.")
                    || key.starts_with("image.")
                    || key.starts_with("disk.")
                    || key.starts_with("skin.")
                    || key.starts_with("runtime.")
            }) {
                continue;
            } else {
                return Err(
                    "EMULATOR_LIST_FORMAT: 当前 SDK 列表格式无法确认，未选择或启动实例。".into(),
                );
            }
        }
        if result.is_empty() {
            return Err("EMULATOR_LIST_RECEIPT_MISSING".into());
        }
    }
    let mut seen = HashSet::new();
    if result.len() > 64 || result.iter().any(|row| !seen.insert(row.name.clone())) {
        return Err("EMULATOR_LIST_DUPLICATE_OR_LIMIT".into());
    }
    Ok(result)
}

fn instance(name: &str, running: Option<bool>) -> Result<EmulatorInstance, String> {
    validate_name(name)?;
    Ok(EmulatorInstance {
        name: name.into(),
        running: running == Some(true),
        owned: false,
        hdc_target: None,
        reason: match running {
            Some(true) => "官方工具报告正在运行；尚未绑定 HDC 目标。",
            Some(false) => "官方工具报告已停止。",
            None => "已发现本地模拟器实例；运行状态尚未确认。",
        }
        .into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lists_require_real_structured_receipts() {
        assert_eq!(
            parse_instances(br#"[{"name":"Phone 1","running":true}]"#).unwrap()[0].name,
            "Phone 1"
        );
        assert!(!parse_instances(b"hvd.name=Phone\nhw.hdc.port=10000\n").unwrap()[0].owned);
        for input in [
            "",
            "[Fail] no images",
            "Phone is running",
            "[{\"name\":\"Phone\"},{\"name\":\"Phone\"}]",
        ] {
            assert!(parse_instances(input.as_bytes()).is_err());
        }
    }
    #[test]
    fn names_do_not_accept_flags_or_paths() {
        for name in ["-license", "../Phone", "Phone;reset", "Phone\n"] {
            assert!(validate_name(name).is_err());
        }
    }
}
