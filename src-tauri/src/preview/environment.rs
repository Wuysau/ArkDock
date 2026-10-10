//! Read-only observations for the native SDK experiment and CLI diagnostics.
//!
//! File presence and pinned hashes identify an SDK configuration. They never prove
//! a usable desktop, a successful build, legal acquisition, or current protocol
//! behavior. This report is internal Rust/CLI data, not a frontend IPC contract.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const MANIFEST_LIMIT: u64 = 64 * 1024;
const FINGERPRINT_LIMIT: u64 = 16 * 1024 * 1024;
const PE_HEADER_LIMIT: u64 = 1024 * 1024;
const PREVIEWER_BIN: &str = "sdk/default/openharmony/previewer/common/bin";
const PREVIEWER: &str = "sdk/default/openharmony/previewer/common/bin/Previewer.exe";
const BUILD_WRAPPER: &str = "hvigor/bin/hvigorw.js";
const BUILD_PLUGIN: &str = "hvigor/hvigor-ohos-plugin/index.js";
const SDK_ROOT_ENV_KEYS: [&str; 4] = [
    "ARKDOCK_SDK",
    "DEVECO_SDK_HOME",
    "OHOS_SDK_HOME",
    "HARMONYOS_SDK_HOME",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DependencyStatus {
    Available,
    Missing,
    Incompatible,
    Unverified,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyCheck {
    pub dependency: String,
    pub status: DependencyStatus,
    pub detail: String,
    pub path: Option<String>,
    pub version: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentReport {
    pub host_os: String,
    pub host_arch: String,
    pub configured_sdk: Option<String>,
    pub checks: Vec<DependencyCheck>,
}

impl EnvironmentReport {
    /// Unknown dynamic loading is settled by first-frame acceptance, but a known
    /// missing or incompatible required dependency must block before execution.
    pub fn execution_blocker(&self) -> Option<&DependencyCheck> {
        self.checks.iter().find(|check| {
            matches!(
                check.status,
                DependencyStatus::Missing
                    | DependencyStatus::Incompatible
                    | DependencyStatus::Error
            )
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedProfile {
    sdk_version: String,
    api_version: u32,
    hvigor_version: String,
    files: BTreeMap<String, String>,
}

struct MetadataExpectation<'a> {
    dependency: &'a str,
    relative: &'a str,
    version: &'a str,
    name: Option<&'a str>,
    api: Option<u32>,
    nested_data: bool,
}

/// Inspect only the configured Command Line Tools root; never launch any tool.
pub fn inspect_environment(sdk: Option<&Path>) -> EnvironmentReport {
    inspect_for_host(sdk, std::env::consts::OS, std::env::consts::ARCH)
}

/// Only known root variables are observed. Explicit/store configuration wins at callers.
pub fn auto_detect_sdk() -> Option<PathBuf> {
    let candidate = select_sdk(sdk_candidates(|key| std::env::var_os(key)), |candidate| {
        super::probe(Some(candidate)).available
    })?;
    // DEVECO_SDK_HOME may identify sdk/; resolve only an already verified local
    // bundle so fixed diagnostic paths and the selected executable agree.
    #[cfg(windows)]
    {
        let status = super::probe(Some(&candidate));
        super::windows::verified_root(&candidate, status.sdk_version.as_deref()?)
    }
    #[cfg(not(windows))]
    {
        Some(candidate)
    }
}

fn sdk_candidates(mut read: impl FnMut(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut candidates = Vec::with_capacity(SDK_ROOT_ENV_KEYS.len());
    for key in SDK_ROOT_ENV_KEYS {
        if let Some(value) = read(key).filter(|value| !value.is_empty()) {
            let candidate = PathBuf::from(value);
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    candidates
}

fn select_sdk(
    candidates: Vec<PathBuf>,
    mut available: impl FnMut(&Path) -> bool,
) -> Option<PathBuf> {
    candidates
        .into_iter()
        .find(|candidate| !unsupported_path_identity(candidate) && available(candidate))
}

fn inspect_for_host(sdk: Option<&Path>, os: &str, arch: &str) -> EnvironmentReport {
    let mut report = EnvironmentReport {
        host_os: os.into(),
        host_arch: arch.into(),
        configured_sdk: sdk.map(display_path),
        checks: vec![check(
            "hostPlatform",
            if os == "windows" && arch == "x86_64" {
                DependencyStatus::Available
            } else {
                DependencyStatus::Incompatible
            },
            format!("Observed {os}/{arch}; the pinned SDK supports Windows x64 only"),
            None,
        )],
    };
    let expected: ExpectedProfile =
        match serde_json::from_str(include_str!("../../../protocol/native-profile.json")) {
            Ok(expected) => expected,
            Err(error) => {
                report.checks.push(check(
                    "sdkProfile",
                    DependencyStatus::Error,
                    format!("The bundled fingerprint profile cannot be read: {error}"),
                    None,
                ));
                return report;
            }
        };
    let Some(sdk) = sdk else {
        report.checks.push(check(
            "sdkRoot",
            DependencyStatus::Missing,
            "No SDK root was configured; no disk or PATH discovery was performed",
            None,
        ));
        append_runtime_requirements(&mut report);
        return report;
    };
    if unsupported_path_identity(sdk) {
        report.checks.push(check(
            "sdkRoot",
            DependencyStatus::Incompatible,
            "WSL and remote SDK paths retain their environment identity and cannot execute through this Windows-local backend",
            Some(sdk),
        ));
        append_runtime_requirements(&mut report);
        return report;
    }
    let root = match sdk.canonicalize() {
        Ok(root) if root.is_dir() => root,
        Ok(_) => {
            report.checks.push(check(
                "sdkRoot",
                DependencyStatus::Incompatible,
                "Configured SDK root is not a directory",
                Some(sdk),
            ));
            append_runtime_requirements(&mut report);
            return report;
        }
        Err(error) => {
            report.checks.push(io_check("sdkRoot", sdk, error));
            append_runtime_requirements(&mut report);
            return report;
        }
    };
    report.checks.push(check(
        "sdkRoot",
        DependencyStatus::Available,
        "Explicit SDK directory is accessible; this is not an acquisition or runtime claim",
        Some(&root),
    ));
    inspect_sdk_files(&root, &expected, &mut report.checks);
    append_runtime_requirements(&mut report);
    report
}

fn inspect_sdk_files(root: &Path, expected: &ExpectedProfile, checks: &mut Vec<DependencyCheck>) {
    for metadata in [
        MetadataExpectation {
            dependency: "sdkMetadata",
            relative: "sdk/default/sdk-pkg.json",
            version: &expected.sdk_version,
            name: None,
            api: Some(expected.api_version),
            nested_data: true,
        },
        MetadataExpectation {
            dependency: "etsComponent",
            relative: "sdk/default/openharmony/ets/oh-uni-package.json",
            version: &expected.sdk_version,
            name: Some("ets"),
            api: Some(expected.api_version),
            nested_data: false,
        },
        MetadataExpectation {
            dependency: "previewerComponent",
            relative: "sdk/default/openharmony/previewer/oh-uni-package.json",
            version: &expected.sdk_version,
            name: Some("previewer"),
            api: Some(expected.api_version),
            nested_data: false,
        },
        MetadataExpectation {
            dependency: "hvigorEngine",
            relative: "hvigor/hvigor/package.json",
            version: &expected.hvigor_version,
            name: Some("@ohos/hvigor"),
            api: None,
            nested_data: false,
        },
        MetadataExpectation {
            dependency: "hvigorPlugin",
            relative: "hvigor/hvigor-ohos-plugin/package.json",
            version: &expected.hvigor_version,
            name: Some("@ohos/hvigor-ohos-plugin"),
            api: None,
            nested_data: false,
        },
    ] {
        checks.push(inspect_metadata(root, metadata));
    }
    for (dependency, relative) in [
        ("previewerFingerprint", PREVIEWER),
        ("buildWrapperFingerprint", BUILD_WRAPPER),
        ("buildPluginFingerprint", BUILD_PLUGIN),
    ] {
        checks.push(inspect_fingerprint(
            root,
            dependency,
            relative,
            expected.files.get(relative).map(String::as_str),
        ));
    }
    checks.push(inspect_pe(root, "previewerBinary", PREVIEWER));
    inspect_previewer_runtime(root, checks);
    checks.push(inspect_pe(root, "sdkNodeBinary", "tool/node/node.exe"));
    checks.push(check(
        "sdkNodeVersion",
        DependencyStatus::Unverified,
        "Node was not executed by this read-only report; record its actual version during the bounded SDK experiment",
        Some(&root.join("tool/node/node.exe")),
    ));
}

/// Observe known runtime files without loading a DLL or reading large resource data.
fn inspect_previewer_runtime(root: &Path, checks: &mut Vec<DependencyCheck>) {
    for (dependency, name) in [
        ("previewerAceLibrary", "libace_compatible.dll"),
        ("previewerArkRuntimeLibrary", "libark_jsruntime.dll"),
        (
            "previewerResourceManagerLibrary",
            "libglobal_resmgr_win.dll",
        ),
    ] {
        checks.push(inspect_pe(
            root,
            dependency,
            &format!("{PREVIEWER_BIN}/{name}"),
        ));
    }
    for (dependency, name, directory) in [
        ("previewerIcuData", "icudt74l.dat", false),
        ("previewerFontConfig", "fontconfig.json", false),
        ("previewerFontsDirectory", "fonts", true),
        ("previewerModuleDirectory", "module", true),
    ] {
        checks.push(inspect_runtime_resource(
            root,
            dependency,
            &format!("{PREVIEWER_BIN}/{name}"),
            directory,
        ));
    }
    checks.push(check(
        "previewerLoadability",
        DependencyStatus::Unverified,
        "DLL presence/x64 headers and resource metadata do not establish dynamic dependencies or loadability; require the actual Previewer first-frame experiment",
        Some(&root.join(PREVIEWER_BIN)),
    ));
}

fn inspect_runtime_resource(
    root: &Path,
    dependency: &str,
    relative: &str,
    directory: bool,
) -> DependencyCheck {
    let path = root.join(relative);
    let metadata = (|| {
        let canonical = path.canonicalize()?;
        if !canonical.starts_with(root) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Runtime resource resolves outside the explicitly selected SDK root",
            ));
        }
        std::fs::metadata(canonical)
    })();
    let metadata = match metadata {
        Ok(metadata) => metadata,
        Err(error) => return io_check(dependency, &path, error),
    };
    let compatible = if directory {
        metadata.is_dir()
    } else {
        metadata.is_file() && metadata.len() > 0
    };
    check(
        dependency,
        if compatible {
            DependencyStatus::Available
        } else {
            DependencyStatus::Incompatible
        },
        if directory {
            "Expected a runtime directory; metadata-only presence does not verify its contents or runtime loadability".into()
        } else {
            format!(
                "Expected a nonempty regular runtime resource; observed {} bytes; content and runtime loadability remain unverified",
                metadata.len()
            )
        },
        Some(&path),
    )
}

fn append_runtime_requirements(report: &mut EnvironmentReport) {
    report.checks.extend([
        check(
            "interactiveDesktop",
            DependencyStatus::Unverified,
            "A usable Windows desktop must be established by the actual native smoke run",
            None,
        ),
        check(
            "nativeProtocol",
            DependencyStatus::Unverified,
            "Pinned files do not prove this run: require actual PreviewBuild artifacts, first frame, input response, source update and owned-process cleanup",
            None,
        ),
    ]);
}

fn inspect_metadata(root: &Path, expected: MetadataExpectation<'_>) -> DependencyCheck {
    let path = root.join(expected.relative);
    let result = read_bounded(root, &path, MANIFEST_LIMIT).and_then(|bytes| {
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    });
    let value = match result {
        Ok(value) => value,
        Err(error) => return io_check(expected.dependency, &path, error),
    };
    let metadata = if expected.nested_data {
        value.get("data").unwrap_or(&serde_json::Value::Null)
    } else {
        &value
    };
    let Some(version) = metadata.get("version").and_then(serde_json::Value::as_str) else {
        return check(
            expected.dependency,
            DependencyStatus::Error,
            "Manifest has no string version field",
            Some(&path),
        );
    };
    let name = metadata
        .get(if expected.api.is_some() {
            "path"
        } else {
            "name"
        })
        .and_then(serde_json::Value::as_str);
    let api = metadata.get("apiVersion").and_then(|value| {
        value
            .as_u64()
            .and_then(|number| u32::try_from(number).ok())
            .or_else(|| value.as_str()?.parse::<u32>().ok())
    });
    let matches = version == expected.version
        && expected.name.is_none_or(|expected| name == Some(expected))
        && expected.api.is_none_or(|expected| api == Some(expected));
    let mut observed = check(
        expected.dependency,
        if matches {
            DependencyStatus::Available
        } else {
            DependencyStatus::Incompatible
        },
        format!(
            "Observed version {version}, component {name:?}, API {api:?}; expected version {}, component {:?}, API {:?}",
            expected.version, expected.name, expected.api
        ),
        Some(&path),
    );
    observed.version = Some(version.into());
    observed
}

fn inspect_fingerprint(
    root: &Path,
    dependency: &str,
    relative: &str,
    expected: Option<&str>,
) -> DependencyCheck {
    let path = root.join(relative);
    let Some(expected) = expected else {
        return check(
            dependency,
            DependencyStatus::Unverified,
            "The bundled profile contains no fingerprint for this required file",
            Some(&path),
        );
    };
    let bytes = match read_bounded(root, &path, FINGERPRINT_LIMIT) {
        Ok(bytes) => bytes,
        Err(error) => return io_check(dependency, &path, error),
    };
    let actual = format!("{:x}", Sha256::digest(bytes));
    let mut observed = check(
        dependency,
        if actual == expected {
            DependencyStatus::Available
        } else {
            DependencyStatus::Incompatible
        },
        if actual == expected {
            "SHA256 matches the specific locally verified SDK profile"
        } else {
            "SHA256 differs from the locally verified SDK profile; native execution must remain blocked"
        },
        Some(&path),
    );
    observed.sha256 = Some(actual);
    observed
}

fn inspect_pe(root: &Path, dependency: &str, relative: &str) -> DependencyCheck {
    let path = root.join(relative);
    let mut file = match open_contained(root, &path) {
        Ok(file) => file,
        Err(error) => return io_check(dependency, &path, error),
    };
    let result = (|| {
        let mut dos = [0_u8; 64];
        file.read_exact(&mut dos)?;
        if &dos[..2] != b"MZ" {
            return Ok(false);
        }
        let offset = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
        if !(64..=PE_HEADER_LIMIT).contains(&offset) {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut pe = [0_u8; 6];
        file.read_exact(&mut pe)?;
        Ok(&pe[..4] == b"PE\0\0" && u16::from_le_bytes([pe[4], pe[5]]) == 0x8664)
    })();
    match result {
        Ok(true) => check(
            dependency,
            DependencyStatus::Available,
            "Bounded PE header identifies a Windows x64 image; dependencies and runtime behavior were not executed",
            Some(&path),
        ),
        Ok(false) => check(
            dependency,
            DependencyStatus::Incompatible,
            "File is not a supported Windows x64 PE image",
            Some(&path),
        ),
        Err(error) => io_check(dependency, &path, error),
    }
}

fn open_contained(root: &Path, path: &Path) -> io::Result<File> {
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "SDK file resolves outside the explicitly selected root",
        ));
    }
    let file = File::open(canonical)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Expected a regular SDK file",
        ));
    }
    Ok(file)
}

fn read_bounded(root: &Path, path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let file = open_contained(root, path)?;
    if file.metadata()?.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SDK file exceeds the read-only observation limit",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SDK file grew beyond the read-only observation limit",
        ));
    }
    Ok(bytes)
}

fn unsupported_path_identity(path: &Path) -> bool {
    let value = path.to_string_lossy().replace('/', "\\");
    let value = value.strip_prefix("\\\\?\\").unwrap_or(&value);
    value.starts_with("\\\\") || value.to_ascii_lowercase().starts_with("unc\\")
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn check(
    dependency: &str,
    status: DependencyStatus,
    detail: impl Into<String>,
    path: Option<&Path>,
) -> DependencyCheck {
    DependencyCheck {
        dependency: dependency.into(),
        status,
        detail: detail.into(),
        path: path.map(display_path),
        version: None,
        sha256: None,
    }
}

fn io_check(dependency: &str, path: &Path, error: io::Error) -> DependencyCheck {
    check(
        dependency,
        if error.kind() == io::ErrorKind::NotFound {
            DependencyStatus::Missing
        } else {
            DependencyStatus::Error
        },
        format!("Read-only SDK observation failed: {error}"),
        Some(path),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn pe_fixture() -> Vec<u8> {
        let mut bytes = vec![0_u8; 70];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&64_u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&0x8664_u16.to_le_bytes());
        bytes
    }

    #[test]
    fn automatic_candidates_preserve_priority_dedupe_and_remote_identity() {
        let environment = BTreeMap::from([
            ("ARKDOCK_SDK", OsString::from("D:/first")),
            ("DEVECO_SDK_HOME", OsString::from("D:/first")),
            (
                "OHOS_SDK_HOME",
                OsString::from("\\\\wsl.localhost\\Ubuntu\\sdk"),
            ),
            ("HARMONYOS_SDK_HOME", OsString::new()),
            ("PATH", OsString::from("D:/unrelated")),
        ]);
        let candidates = sdk_candidates(|key| environment.get(key).cloned());
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("D:/first"),
                PathBuf::from("\\\\wsl.localhost\\Ubuntu\\sdk")
            ]
        );
    }

    #[test]
    fn automatic_selection_skips_unverified_and_remote_candidates_without_execution() {
        let candidates = vec![
            PathBuf::from("D:/unknown"),
            PathBuf::from("\\\\server\\sdk"),
            PathBuf::from("D:/verified"),
            PathBuf::from("D:/later"),
        ];
        let mut inspected = Vec::new();
        let selected = select_sdk(candidates, |candidate| {
            inspected.push(candidate.to_owned());
            candidate == Path::new("D:/verified")
        });
        assert_eq!(selected, Some(PathBuf::from("D:/verified")));
        assert_eq!(
            inspected,
            vec![PathBuf::from("D:/unknown"), PathBuf::from("D:/verified")]
        );
        assert!(select_sdk(vec![PathBuf::from("D:/unknown")], |_| false).is_none());
        assert!(select_sdk(Vec::new(), |_| panic!(
            "empty candidates must not be probed"
        ))
        .is_none());
    }

    #[test]
    fn absent_sdk_and_other_platforms_are_distinct_from_unverified_runtime() {
        let report = inspect_for_host(None, "linux", "x86_64");
        assert_eq!(report.checks[0].status, DependencyStatus::Incompatible);
        assert_eq!(report.checks[1].status, DependencyStatus::Missing);
        assert!(report.checks.iter().any(|check| {
            check.dependency == "nativeProtocol" && check.status == DependencyStatus::Unverified
        }));
        assert_eq!(
            inspect_for_host(None, "windows", "aarch64").checks[0].status,
            DependencyStatus::Incompatible
        );
    }

    #[test]
    fn remote_identity_is_rejected_before_local_path_resolution() {
        for path in [
            "\\\\wsl.localhost\\Ubuntu\\sdk",
            "\\\\wsl$\\Ubuntu\\sdk",
            "\\\\server\\sdk",
            "\\\\?\\UNC\\server\\sdk",
        ] {
            let report = inspect_for_host(Some(Path::new(path)), "windows", "x86_64");
            assert_eq!(report.configured_sdk.as_deref(), Some(path));
            assert_eq!(report.checks[1].status, DependencyStatus::Incompatible);
            assert!(!report
                .checks
                .iter()
                .any(|check| check.dependency == "sdkMetadata"));
        }
        assert!(!unsupported_path_identity(Path::new("\\\\?\\D:\\sdk")));
    }

    #[test]
    fn versions_are_observed_and_unsupported_versions_are_not_enabled() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let path = root.join("manifest.json");
        fs::write(
            &path,
            br#"{"path":"ets","version":"future","apiVersion":"26"}"#,
        )
        .unwrap();
        let observed = inspect_metadata(
            &root,
            MetadataExpectation {
                dependency: "ets",
                relative: "manifest.json",
                version: "26.0.0.105",
                name: Some("ets"),
                api: Some(26),
                nested_data: false,
            },
        );
        assert_eq!(observed.status, DependencyStatus::Incompatible);
        assert_eq!(observed.version.as_deref(), Some("future"));
    }

    #[test]
    fn fingerprints_detect_changes_and_do_not_replace_runtime_evidence() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        fs::write(root.join("tool.js"), b"original").unwrap();
        let expected = format!("{:x}", Sha256::digest(b"original"));
        let observed = inspect_fingerprint(&root, "tool", "tool.js", Some(&expected));
        assert_eq!(observed.status, DependencyStatus::Available);
        fs::write(root.join("tool.js"), b"changed").unwrap();
        assert_eq!(
            inspect_fingerprint(&root, "tool", "tool.js", Some(&expected)).status,
            DependencyStatus::Incompatible
        );
        assert_eq!(
            inspect_fingerprint(&root, "tool", "missing", Some(&expected)).status,
            DependencyStatus::Missing
        );
    }

    #[test]
    fn malformed_oversized_and_directory_files_fail_with_bounded_reads() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        fs::write(root.join("large"), [0_u8; 64]).unwrap();
        assert!(read_bounded(&root, &root.join("large"), 16).is_err());
        assert!(read_bounded(&root, &root, 16).is_err());
        fs::write(root.join("invalid.exe"), b"not PE").unwrap();
        assert_eq!(
            inspect_pe(&root, "pe", "invalid.exe").status,
            DependencyStatus::Error
        );
        let mut pe = pe_fixture();
        pe[60..64].copy_from_slice(&(PE_HEADER_LIMIT as u32 + 1).to_le_bytes());
        fs::write(root.join("offset.exe"), pe).unwrap();
        assert_eq!(
            inspect_pe(&root, "pe", "offset.exe").status,
            DependencyStatus::Incompatible
        );
    }

    #[test]
    fn pe_architecture_is_checked_without_executing_the_file() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let mut pe = pe_fixture();
        fs::write(root.join("node.exe"), &pe).unwrap();
        assert_eq!(
            inspect_pe(&root, "node", "node.exe").status,
            DependencyStatus::Available
        );
        pe[68..70].copy_from_slice(&0xaa64_u16.to_le_bytes());
        fs::write(root.join("node.exe"), pe).unwrap();
        assert_eq!(
            inspect_pe(&root, "node", "node.exe").status,
            DependencyStatus::Incompatible
        );
    }

    #[test]
    fn previewer_libraries_detect_missing_wrong_arch_and_read_only_large_pe_headers() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let directory = root.join(PREVIEWER_BIN);
        fs::create_dir_all(&directory).unwrap();
        let mut checks = Vec::new();
        inspect_previewer_runtime(&root, &mut checks);
        assert_eq!(
            checks
                .iter()
                .find(|check| check.dependency == "previewerAceLibrary")
                .unwrap()
                .status,
            DependencyStatus::Missing
        );

        fs::write(directory.join("libace_compatible.dll"), pe_fixture()).unwrap();
        File::options()
            .write(true)
            .open(directory.join("libace_compatible.dll"))
            .unwrap()
            .set_len(104_776_632)
            .unwrap();
        let mut arm = pe_fixture();
        arm[68..70].copy_from_slice(&0xaa64_u16.to_le_bytes());
        fs::write(directory.join("libark_jsruntime.dll"), arm).unwrap();
        fs::write(directory.join("libglobal_resmgr_win.dll"), pe_fixture()).unwrap();
        checks.clear();
        inspect_previewer_runtime(&root, &mut checks);
        for (dependency, expected) in [
            ("previewerAceLibrary", DependencyStatus::Available),
            ("previewerArkRuntimeLibrary", DependencyStatus::Incompatible),
            (
                "previewerResourceManagerLibrary",
                DependencyStatus::Available,
            ),
            ("previewerLoadability", DependencyStatus::Unverified),
        ] {
            let observed = checks
                .iter()
                .find(|check| check.dependency == dependency)
                .unwrap();
            assert_eq!(observed.status, expected);
            assert!(observed.sha256.is_none());
        }
    }

    #[test]
    fn runtime_resources_detect_missing_wrong_kind_and_empty_files_without_content_reads() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let directory = root.join(PREVIEWER_BIN);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("icudt74l.dat"), b"fixture").unwrap();
        File::options()
            .write(true)
            .open(directory.join("icudt74l.dat"))
            .unwrap()
            .set_len(31_981_600)
            .unwrap();
        fs::write(directory.join("fontconfig.json"), b"").unwrap();
        fs::write(directory.join("fonts"), b"file instead of directory").unwrap();
        let mut checks = Vec::new();
        inspect_previewer_runtime(&root, &mut checks);
        for (dependency, expected) in [
            ("previewerIcuData", DependencyStatus::Available),
            ("previewerFontConfig", DependencyStatus::Incompatible),
            ("previewerFontsDirectory", DependencyStatus::Incompatible),
            ("previewerModuleDirectory", DependencyStatus::Missing),
        ] {
            assert_eq!(
                checks
                    .iter()
                    .find(|check| check.dependency == dependency)
                    .unwrap()
                    .status,
                expected
            );
        }
        fs::create_dir(directory.join("module")).unwrap();
        let observed =
            inspect_runtime_resource(&root, "module", &format!("{PREVIEWER_BIN}/module"), true);
        assert_eq!(observed.status, DependencyStatus::Available);
        assert!(observed.detail.contains("metadata-only"));
        assert!(observed.sha256.is_none());
    }

    #[test]
    fn runtime_resource_metadata_rejects_paths_outside_the_selected_sdk() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("selected");
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(temporary.path().join("outside"), b"external").unwrap();
        let observed = inspect_runtime_resource(&root, "resource", "../outside", false);
        assert_eq!(observed.status, DependencyStatus::Error);
        assert!(observed.detail.contains("outside"));
        assert!(observed.sha256.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn escaped_symlinks_are_not_read_or_hashed() {
        let selected = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = selected.path().canonicalize().unwrap();
        fs::write(outside.path().join("outside"), b"external").unwrap();
        std::os::unix::fs::symlink(outside.path().join("outside"), root.join("tool")).unwrap();
        let observed = inspect_fingerprint(&root, "tool", "tool", Some("unused"));
        assert_eq!(observed.status, DependencyStatus::Error);
        assert!(observed.sha256.is_none());
    }
}
