//! Reviewed command plans, not an execution or deployment authorization API.
//! HDC reconstructs its arguments and device `shell` interprets the command;
//! keep all fields constrained even when the host uses structured arguments.

use crate::ipc::model::{DeviceRuntimeKind, DeviceRuntimeTarget};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const TARGET_STDOUT_LIMIT: usize = 64 * 1024;
pub const TARGET_COUNT_LIMIT: usize = 64;
pub const CLIENT_TIMEOUT_MS: u64 = 5_000;
pub const COMMAND_ARGUMENT_BYTES_LIMIT: usize = 4096;

/// Every targeted operation requires the exact connect key. There is no raw
/// shell, implicit single-device fallback, server kill, privilege or reboot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HdcOperation {
    ListTargets,
    UdId {
        target: String,
    },
    SerialNumber {
        target: String,
    },
    WindowInfo {
        target: String,
    },
    ProcessInfo {
        target: String,
        bundle: String,
    },
    RemoveScreenshot {
        target: String,
        nonce: String,
    },
    KeyEvent {
        target: String,
        code: u32,
    },
    DoubleClick {
        target: String,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    LongClick {
        target: String,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    Install {
        target: String,
        hap: PathBuf,
    },
    Start {
        target: String,
        bundle: String,
        module: String,
        ability: String,
    },
    Stop {
        target: String,
        bundle: String,
    },
    BundleInfo {
        target: String,
        bundle: String,
    },
    Screenshot {
        target: String,
        nonce: String,
    },
    ReceiveScreenshot {
        target: String,
        nonce: String,
        destination: PathBuf,
    },
    Click {
        target: String,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    Swipe {
        target: String,
        from: (u32, u32),
        to: (u32, u32),
        velocity: u32,
        width: u32,
        height: u32,
    },
    Drag {
        target: String,
        from: (u32, u32),
        to: (u32, u32),
        velocity: u32,
        width: u32,
        height: u32,
    },
    Text {
        target: String,
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HdcCommandPlan {
    pub args: Vec<String>,
    pub timeout_ms: u64,
    pub stdout_limit: usize,
    /// Callers must never elevate exit(0) into an installation/input/frame claim.
    pub completion_requires_runtime_evidence: bool,
}

pub fn command_plan(operation: HdcOperation) -> Result<HdcCommandPlan, String> {
    let timeout_ms = match &operation {
        HdcOperation::Install { .. } => 120_000,
        HdcOperation::Screenshot { .. } | HdcOperation::ReceiveScreenshot { .. } => 20_000,
        HdcOperation::Start { .. } => 20_000,
        _ => CLIENT_TIMEOUT_MS,
    };
    let stdout_limit = match &operation {
        HdcOperation::BundleInfo { .. } | HdcOperation::WindowInfo { .. } => 2 * 1024 * 1024,
        _ => TARGET_STDOUT_LIMIT,
    };
    let args = match operation {
        HdcOperation::ListTargets => vec!["list".into(), "targets".into(), "-v".into()],
        HdcOperation::UdId { target } => targeted(
            &target,
            vec!["shell".into(), "bm".into(), "get".into(), "-u".into()],
        )?,
        // startup_init/services/modules/udid/udid_comm.c GetSerial_ reads this
        // exact parameter. The HDC connect key is not a device serial number.
        HdcOperation::SerialNumber { target } => targeted(
            &target,
            vec![
                "shell".into(),
                "param".into(),
                "get".into(),
                "ohos.boot.sn".into(),
            ],
        )?,
        HdcOperation::WindowInfo { target } => targeted(
            &target,
            vec![
                "shell".into(),
                "hidumper".into(),
                "-s".into(),
                "WindowManagerService".into(),
                "-a".into(),
                "-a".into(),
            ],
        )?,
        HdcOperation::ProcessInfo { target, bundle } => {
            validate_identifier(&bundle)?;
            targeted(&target, vec!["shell".into(), "pidof".into(), bundle])?
        }
        HdcOperation::RemoveScreenshot { target, nonce } => targeted(
            &target,
            vec![
                "shell".into(),
                "rm".into(),
                "-f".into(),
                "--".into(),
                remote_screenshot(&nonce)?,
            ],
        )?,
        HdcOperation::KeyEvent { target, code } => {
            if code == 0 || code > 10_000 {
                return Err("INVALID_DEVICE_KEY_CODE".into());
            }
            targeted(
                &target,
                vec![
                    "shell".into(),
                    "uitest".into(),
                    "uiInput".into(),
                    "keyEvent".into(),
                    code.to_string(),
                ],
            )?
        }
        HdcOperation::DoubleClick {
            target,
            x,
            y,
            width,
            height,
        } => {
            validate_point((x, y), width, height)?;
            targeted(
                &target,
                vec![
                    "shell".into(),
                    "uitest".into(),
                    "uiInput".into(),
                    "doubleClick".into(),
                    x.to_string(),
                    y.to_string(),
                ],
            )?
        }
        HdcOperation::LongClick {
            target,
            x,
            y,
            width,
            height,
        } => {
            validate_point((x, y), width, height)?;
            targeted(
                &target,
                vec![
                    "shell".into(),
                    "uitest".into(),
                    "uiInput".into(),
                    "longClick".into(),
                    x.to_string(),
                    y.to_string(),
                ],
            )?
        }
        HdcOperation::Install { target, hap } => {
            if hap.extension().and_then(|value| value.to_str()) != Some("hap") {
                return Err("DEVICE_INSTALL_REQUIRES_HAP".into());
            }
            targeted(&target, vec!["install".into(), local_path(&hap)?])?
        }
        HdcOperation::Start {
            target,
            bundle,
            module,
            ability,
        } => {
            validate_identifier(&bundle)?;
            validate_identifier(&module)?;
            validate_identifier(&ability)?;
            targeted(
                &target,
                vec![
                    "shell".into(),
                    "aa".into(),
                    "start".into(),
                    "-a".into(),
                    ability,
                    "-b".into(),
                    bundle,
                    "-m".into(),
                    module,
                ],
            )?
        }
        HdcOperation::Stop { target, bundle } => {
            validate_identifier(&bundle)?;
            targeted(
                &target,
                vec!["shell".into(), "aa".into(), "force-stop".into(), bundle],
            )?
        }
        HdcOperation::BundleInfo { target, bundle } => {
            validate_identifier(&bundle)?;
            targeted(
                &target,
                vec![
                    "shell".into(),
                    "bm".into(),
                    "dump".into(),
                    "-n".into(),
                    bundle,
                ],
            )?
        }
        HdcOperation::Screenshot { target, nonce } => targeted(
            &target,
            vec![
                "shell".into(),
                "uitest".into(),
                "screenCap".into(),
                "-p".into(),
                remote_screenshot(&nonce)?,
            ],
        )?,
        HdcOperation::ReceiveScreenshot {
            target,
            nonce,
            destination,
        } => {
            if destination.extension().and_then(|value| value.to_str()) != Some("png") {
                return Err("DEVICE_CAPTURE_REQUIRES_PNG_DESTINATION".into());
            }
            targeted(
                &target,
                vec![
                    "file".into(),
                    "recv".into(),
                    remote_screenshot(&nonce)?,
                    local_path(&destination)?,
                ],
            )?
        }
        HdcOperation::Click {
            target,
            x,
            y,
            width,
            height,
        } => {
            validate_point((x, y), width, height)?;
            targeted(
                &target,
                vec![
                    "shell".into(),
                    "uitest".into(),
                    "uiInput".into(),
                    "click".into(),
                    x.to_string(),
                    y.to_string(),
                ],
            )?
        }
        HdcOperation::Swipe {
            target,
            from,
            to,
            velocity,
            width,
            height,
        } => gesture(&target, "swipe", from, to, velocity, width, height)?,
        HdcOperation::Drag {
            target,
            from,
            to,
            velocity,
            width,
            height,
        } => gesture(&target, "drag", from, to, velocity, width, height)?,
        HdcOperation::Text { target, text } => {
            if text.is_empty() || text.len() > 512 || text.chars().any(char::is_control) {
                return Err(
                    "INVALID_DEVICE_TEXT: bounded text without control characters is required"
                        .into(),
                );
            }
            targeted(
                &target,
                vec![
                    "shell".into(),
                    "uitest".into(),
                    "uiInput".into(),
                    "text".into(),
                    text,
                ],
            )?
        }
    };
    if args.iter().map(String::len).sum::<usize>() > COMMAND_ARGUMENT_BYTES_LIMIT {
        return Err("HDC_COMMAND_LIMIT_EXCEEDED".into());
    }
    Ok(HdcCommandPlan {
        args,
        timeout_ms,
        stdout_limit,
        completion_requires_runtime_evidence: true,
    })
}

fn targeted(target: &str, args: Vec<String>) -> Result<Vec<String>, String> {
    validate_target_key(target)?;
    // HDC reconstructs command argv and double-quotes a token containing spaces.
    // Its daemon ShellCmdTrim removes that single surrounding pair. Supplying
    // the WHOLE typed remote command as one argv preserves our POSIX quoting:
    // no individual user text is exposed to shell expansion or field splitting.
    // Sources: developtools_hdc src/host/main.cpp, src/common/base.cpp and
    // src/common/async_cmd.cpp. This implements their documented transport; it
    // does not copy source or offer a raw shell IPC endpoint.
    let args = if args.first().is_some_and(|a| a == "shell") {
        vec![
            "shell".into(),
            args[1..]
                .iter()
                .map(|arg| shell_quote(arg))
                .collect::<Vec<_>>()
                .join(" "),
        ]
    } else {
        args
    };
    Ok([vec!["-t".into(), target.into()], args].concat())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub(super) fn validate_target_key(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 256
        || value.starts_with('-')
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'[' | b']')
        })
    {
        return Err("INVALID_HDC_TARGET_KEY".into());
    }
    Ok(())
}

pub(super) fn validate_identifier(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 256
        || !value.as_bytes()[0].is_ascii_alphabetic()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
    {
        return Err("INVALID_DEVICE_APPLICATION_IDENTIFIER".into());
    }
    Ok(())
}

fn remote_screenshot(nonce: &str) -> Result<String, String> {
    if !(16..=64).contains(&nonce.len()) || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("INVALID_DEVICE_CAPTURE_NONCE".into());
    }
    Ok(format!("/data/local/tmp/arkdock_{nonce}.png"))
}

fn local_path(path: &Path) -> Result<String, String> {
    let path = path.to_str().ok_or("HDC_PATH_ENCODING_UNSUPPORTED")?;
    let path = path.strip_prefix("\\\\?\\").unwrap_or(path);
    let bytes = path.as_bytes();
    if bytes.len() < 4
        || path.len() > 2048
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'/' | b'\\')
        || path.chars().any(|ch| {
            ch.is_control()
                || matches!(
                    ch,
                    '"' | '\'' | '<' | '>' | '|' | '?' | '*' | '$' | '`' | ';' | '&'
                )
        })
        || path
            .split(['/', '\\'])
            .any(|component| matches!(component, "." | "..") || component.ends_with([' ', '.']))
        || bytes[2..].contains(&b':')
    {
        return Err("INVALID_HDC_LOCAL_PATH: a bounded absolute Windows drive path without ambiguous path syntax is required".into());
    }
    Ok(path.into())
}

fn validate_point(point: (u32, u32), width: u32, height: u32) -> Result<(), String> {
    if width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || u64::from(width) * u64::from(height) > 8_388_608
        || point.0 == 0
        || point.1 == 0
        || point.0 >= width
        || point.1 >= height
    {
        return Err("INVALID_DEVICE_INPUT_COORDINATE: dimensions and nonzero coordinates require current device display evidence".into());
    }
    Ok(())
}

fn gesture(
    target: &str,
    operation: &str,
    from: (u32, u32),
    to: (u32, u32),
    velocity: u32,
    width: u32,
    height: u32,
) -> Result<Vec<String>, String> {
    validate_point(from, width, height)?;
    validate_point(to, width, height)?;
    if !(200..=40_000).contains(&velocity) {
        return Err("INVALID_DEVICE_GESTURE_VELOCITY".into());
    }
    targeted(
        target,
        vec![
            "shell".into(),
            "uitest".into(),
            "uiInput".into(),
            operation.into(),
            from.0.to_string(),
            from.1.to_string(),
            to.0.to_string(),
            to.1.to_string(),
            velocity.to_string(),
        ],
    )
}

/// Parse only separated stdout from a verified bounded `list targets -v`
/// invocation. Reject the entire observation on unknown rows, duplicate keys,
/// mixed log/error lines, overflow or malformed UTF-8. Never parse stderr or a
/// combined process log. Format acceptance does not grant a device execution.
pub fn parse_target_stdout(stdout: &[u8]) -> Result<Vec<DeviceRuntimeTarget>, String> {
    if stdout.len() > TARGET_STDOUT_LIMIT {
        return Err("HDC_TARGET_STDOUT_LIMIT_EXCEEDED".into());
    }
    let text = std::str::from_utf8(stdout).map_err(|_| "HDC_TARGET_STDOUT_ENCODING")?;
    let text = text.trim_matches(['\r', '\n', ' ', '\t']);
    if text == "[Empty]" {
        return Ok(Vec::new());
    }
    if text.is_empty() {
        return Err("HDC_TARGET_RECEIPT_MISSING".into());
    }
    let mut targets = Vec::new();
    let mut keys = HashSet::new();
    for line in text.lines() {
        if line.len() > 1024
            || line
                .chars()
                .any(|ch| ch.is_control() && !matches!(ch, '\t' | '\r'))
        {
            return Err("HDC_TARGET_ROW_INVALID".into());
        }
        // 2022 server.cpp: key / transport / status / device name.
        // New public docs append an `hdc` identifier. Unknown profiles block.
        let fields: Vec<&str> = if line.contains('\t') {
            line.split('\t')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .collect()
        } else {
            line.split_ascii_whitespace().collect()
        };
        if fields.len() != 4 && !(fields.len() == 5 && fields[4] == "hdc") {
            return Err("HDC_TARGET_FORMAT_UNVERIFIED".into());
        }
        validate_target_key(fields[0])?;
        if !matches!(
            fields[1],
            "USB" | "TCP" | "UART" | "BT" | "UNKNOW" | "Unknown"
        ) || !matches!(
            fields[2],
            "Ready" | "Connected" | "Offline" | "Unauthorized" | "Unknown" | "UNKNOW"
        ) || fields[3].is_empty()
            || fields[3].len() > 128
        {
            return Err("HDC_TARGET_FORMAT_UNVERIFIED".into());
        }
        if targets.len() >= TARGET_COUNT_LIMIT || !keys.insert(fields[0].to_owned()) {
            return Err("HDC_TARGET_COUNT_OR_DUPLICATE_KEY".into());
        }
        targets.push(DeviceRuntimeTarget {
            id: fields[0].into(),
            label: format!("{} · {} · {}", fields[3], fields[1], fields[0]),
            kind: DeviceRuntimeKind::Unknown,
            connection_state: fields[2].into(),
            reason: "来自 HDC 的目标；设备类型由用户选择或自有模拟器实例关联确认。".into(),
        });
    }
    Ok(targets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_number_uses_os_parameter_not_hdc_connect_key() {
        let plan = command_plan(HdcOperation::SerialNumber {
            target: "127.0.0.1:1234".into(),
        })
        .unwrap();
        assert_eq!(
            plan.args,
            vec![
                "-t",
                "127.0.0.1:1234",
                "shell",
                "'param' 'get' 'ohos.boot.sn'"
            ]
        );
        assert!(plan.timeout_ms <= CLIENT_TIMEOUT_MS);
    }
    #[test]
    fn whole_remote_command_quotes_unicode_and_metacharacters_as_data() {
        let plan = command_plan(HdcOperation::Text {
            target: "device1".into(),
            text: "中文 空格 ' \" $(touch hacked); &".into(),
        })
        .unwrap();
        assert_eq!(&plan.args[..3], ["-t", "device1", "shell"]);
        assert_eq!(plan.args.len(), 4);
        assert_eq!(
            plan.args[3],
            "'uitest' 'uiInput' 'text' '中文 空格 '\"'\"' \" $(touch hacked); &'"
        );
        // Mirror HDC's two transport steps, not the shell quote implementation:
        // host argv reconstruction wraps the single command; daemon strips one
        // outer pair. The original fully quoted remote program is preserved.
        let reconstructed = format!("\"{}\"", plan.args[3]);
        assert_eq!(&reconstructed[1..reconstructed.len() - 1], plan.args[3]);
    }
    #[test]
    fn local_paths_support_spaces_and_unicode_but_reject_ambiguous_syntax() {
        assert_eq!(
            local_path(Path::new("D:\\中文 工程\\entry-signed.hap")).unwrap(),
            "D:\\中文 工程\\entry-signed.hap"
        );
        for path in [
            "D:\\ok\\..\\secret.hap",
            "D:\\a\"b.hap",
            "D:\\x:stream.hap",
            "D:\\folder.\\a.hap",
            "D:\\a;echo.hap",
        ] {
            assert!(local_path(Path::new(path)).is_err());
        }
    }
    #[test]
    fn tab_delimited_device_names_can_contain_spaces_without_parsing_logs() {
        let targets = parse_target_stdout(b"abc\t\tUSB\tConnected\tMy Phone\thdc\n").unwrap();
        assert_eq!(targets[0].id, "abc");
        assert!(targets[0].label.contains("My Phone"));
        assert!(parse_target_stdout(b"abc\tUSB\tConnected\tPhone\n[Fail] error\n").is_err());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Requires an explicitly selected ARKDOCK_TEST_POSIX_SHELL (Git bash), never a device shell"]
    fn real_posix_shell_preserves_text_after_hdc_transport_reconstruction() {
        use super::super::process::ToolCommandRunner;
        use std::sync::atomic::AtomicBool;
        use std::time::Duration;
        let executable = std::path::PathBuf::from(
            std::env::var_os("ARKDOCK_TEST_POSIX_SHELL").expect("select an isolated host shell"),
        );
        let text = "中文 hello ' \" $(printf INJECTED); & `printf INJECTED` \\";
        let plan = command_plan(HdcOperation::Text {
            target: "fixture".into(),
            text: text.into(),
        })
        .unwrap();
        let typed_command = plan.args[3].replacen("'uitest' 'uiInput' 'text'", "printf '%s'", 1);
        let wrapped = format!("\"{typed_command}\"");
        let device_command = &wrapped[1..wrapped.len() - 1];
        let result = ToolCommandRunner::run(
            &executable,
            &[
                "--noprofile".into(),
                "--norc".into(),
                "-c".into(),
                device_command.into(),
            ],
            executable.parent().unwrap(),
            &[],
            Duration::from_secs(10),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.status, 0);
        assert_eq!(result.stdout, text.as_bytes());
        assert!(result.stderr.is_empty());
    }
}
