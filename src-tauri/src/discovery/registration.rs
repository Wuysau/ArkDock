//! Optional file-based registration is deliberately weak. These records are not an authenticated
//! named-pipe protocol and cannot prove an active terminal tab. No record may become strong evidence.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ipc::model::{Environment, Evidence, EvidenceSource, ProcessRef, TargetInfo};

use super::resolver::WorkspaceResolver;
use super::{fresh, MAX_EVIDENCE_TTL_MS};

const MAX_RECORD_BYTES: u64 = 16_384;
const MAX_RECORDS: usize = 128;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistrationRecord {
    pub version: u32,
    pub session_id: String,
    pub target_id: String,
    pub target_process: ProcessRef,
    pub sender_process: ProcessRef,
    pub environment: Environment,
    pub workspace_path: String,
    pub observed_at: u64,
    pub expires_at: u64,
}

impl RegistrationRecord {
    pub fn validate(
        &self,
        target: &TargetInfo,
        now: u64,
        sender_alive: bool,
        target_alive: bool,
    ) -> Result<Evidence, String> {
        self.validate_shape(now)?;
        if self.target_id != target.id
            || self.target_process.pid != target.process.pid
            || self.target_process.started_at != target.process.started_at
        {
            return Err("Registration target token or process lifetime mismatch".into());
        }
        if !sender_alive || !target_alive {
            return Err(
                "Registration sender or target process is no longer the recorded lifecycle".into(),
            );
        }
        Ok(self.evidence())
    }

    pub fn validate_shape(&self, now: u64) -> Result<(), String> {
        if self.version != 1 {
            return Err("Unsupported registration version".into());
        }
        if !valid_session_id(&self.session_id) {
            return Err("Invalid registration session ID".into());
        }
        if self.target_id.is_empty() || self.target_id.len() > 128 || self.target_id.contains('\0')
        {
            return Err("Invalid target token".into());
        }
        if self.sender_process.pid == 0
            || self.target_process.pid == 0
            || !valid_process_token(&self.sender_process.started_at)
            || !valid_process_token(&self.target_process.started_at)
        {
            return Err("A nonzero PID and process start time are required".into());
        }
        if self.workspace_path.is_empty()
            || self.workspace_path.len() > 4096
            || self.workspace_path.contains('\0')
        {
            return Err("Invalid registration workspace path".into());
        }
        let path = &self.workspace_path;
        let bytes = path.as_bytes();
        let windows_absolute = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/');
        let detected = super::path_environment(Path::new(path));
        let wsl_unc = matches!(detected, Environment::Wsl)
            && (path.starts_with("\\\\") || path.starts_with("//"));
        let absolute = match self.environment {
            Environment::WindowsLocal => {
                (windows_absolute || path.starts_with("\\\\"))
                    && matches!(detected, Environment::WindowsLocal)
            }
            Environment::Wsl => path.starts_with('/') || wsl_unc || path.starts_with("wsl://"),
            Environment::Remote => {
                path.starts_with("ssh://") || path.starts_with("vscode-remote://")
            }
        };
        if !absolute {
            return Err(
                "Workspace path must retain an absolute path in its declared environment".into(),
            );
        }
        if !fresh(&self.evidence(), now) {
            return Err("Registration expired, future-dated, or exceeds the maximum TTL".into());
        }
        Ok(())
    }

    fn evidence(&self) -> Evidence {
        Evidence {
            source: EvidenceSource::Registration,
            target_id: Some(self.target_id.clone()),
            process: Some(self.target_process.clone()),
            session_id: Some(self.session_id.clone()),
            observed_at: self.observed_at,
            expires_at: self.expires_at,
            environment: self.environment.clone(),
            raw_path: self.workspace_path.clone(),
            verifiable: false,
            active_scope: false,
        }
    }
}

pub struct RegistrationRegistry {
    directory: Option<PathBuf>,
}

impl Default for RegistrationRegistry {
    fn default() -> Self {
        Self {
            directory: default_directory(),
        }
    }
}

impl RegistrationRegistry {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory: Some(directory),
        }
    }

    pub fn directory(&self) -> Option<&Path> {
        self.directory.as_deref()
    }

    fn records(&self) -> Result<Vec<RegistrationRecord>, String> {
        let Some(directory) = &self.directory else {
            return Ok(Vec::new());
        };
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let metadata = fs::symlink_metadata(directory).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("Registration directory must be a regular directory".into());
        }
        let entries = fs::read_dir(directory)
            .map_err(|error| format!("Cannot read registration directory: {error}"))?;
        let mut records = Vec::new();
        for (count, entry) in entries.enumerate() {
            if count >= MAX_RECORDS {
                return Err("Registration directory exceeds its bounded record limit".into());
            }
            let entry = entry.map_err(|error| error.to_string())?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.len() != 69
                || !name.ends_with(".json")
                || !name[..64].bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                continue;
            }
            if !entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_file()
            {
                continue;
            }
            if let Ok(record) = read_record(&entry.path()) {
                if session_path(directory, &record.session_id).ok().as_ref() == Some(&entry.path())
                {
                    records.push(record);
                }
            }
        }
        Ok(records)
    }
}

impl WorkspaceResolver for RegistrationRegistry {
    fn evidence(&self, target: &TargetInfo, now: u64) -> Result<Vec<Evidence>, String> {
        let mut result = Vec::new();
        for record in self.records()? {
            let sender_alive = process_started_at(record.sender_process.pid)
                .ok()
                .map(|value| value.to_string())
                == Some(record.sender_process.started_at.clone());
            let target_alive = process_started_at(record.target_process.pid)
                .ok()
                .map(|value| value.to_string())
                == Some(record.target_process.started_at.clone());
            if let Ok(evidence) = record.validate(target, now, sender_alive, target_alive) {
                result.push(evidence);
            }
        }
        Ok(result)
    }
}

pub fn default_directory() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map(|root| PathBuf::from(root).join("ArkDock/sessions"))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

pub fn session_path(directory: &Path, session_id: &str) -> Result<PathBuf, String> {
    if !valid_session_id(session_id) {
        return Err("Invalid session ID".into());
    }
    let hash = Sha256::digest(session_id.as_bytes());
    Ok(directory.join(format!("{hash:x}.json")))
}

pub fn read_record(path: &Path) -> Result<RegistrationRecord, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_RECORD_BYTES
    {
        return Err("Registration must be a bounded regular file".into());
    }
    let mut contents = String::new();
    File::open(path)
        .map_err(|error| error.to_string())?
        .take(MAX_RECORD_BYTES + 1)
        .read_to_string(&mut contents)
        .map_err(|error| error.to_string())?;
    if contents.len() as u64 > MAX_RECORD_BYTES {
        return Err("Registration exceeds the byte limit".into());
    }
    serde_json::from_str(&contents).map_err(|error| format!("Invalid registration JSON: {error}"))
}

/// Writes only the helper's explicit session record. File registrations are never authenticated.
pub fn write_record(
    directory: &Path,
    record: &RegistrationRecord,
    now: u64,
) -> Result<PathBuf, String> {
    record.validate_shape(now)?;
    if process_started_at(record.sender_process.pid)?.to_string()
        != record.sender_process.started_at
        || process_started_at(record.target_process.pid)?.to_string()
            != record.target_process.started_at
    {
        return Err("Sender or target process lifetime changed before registration".into());
    }
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    if fs::symlink_metadata(directory)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("Refusing a symlink registration directory".into());
    }
    let path = session_path(directory, &record.session_id)?;
    if path.exists() {
        let existing = read_record(&path)?;
        if existing.session_id != record.session_id
            || existing.sender_process != record.sender_process
            || existing.target_id != record.target_id
            || existing.target_process != record.target_process
            || existing.environment != record.environment
            || existing.workspace_path != record.workspace_path
        {
            return Err(
                "An existing session cannot be reassigned; revoke it and register a new session"
                    .into(),
            );
        }
    }
    let contents = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    if contents.len() as u64 > MAX_RECORD_BYTES {
        return Err("Registration exceeds the byte limit".into());
    }
    // A unique temporary file prevents partial JSON from becoming a candidate. Windows rename
    // cannot replace an existing destination, so refresh removes only this explicit session file.
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("Cannot create registration temporary file: {error}"))?;
    let written = file.write_all(&contents).and_then(|_| file.sync_all());
    drop(file);
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    if path.exists() {
        if let Err(error) = fs::remove_file(&path) {
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
    }
    if let Err(error) = fs::rename(&temporary, &path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(path)
}

pub fn revoke(directory: &Path, session_id: &str) -> Result<(), String> {
    let path = session_path(directory, session_id)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub fn new_session_id(target_id: &str, sender_pid: u32, now: u64) -> String {
    let mut hash = Sha256::new();
    hash.update(target_id.as_bytes());
    hash.update(sender_pid.to_le_bytes());
    hash.update(now.to_le_bytes());
    hash.update(std::process::id().to_le_bytes());
    format!("session-{:x}", hash.finalize())
}

fn valid_session_id(session_id: &str) -> bool {
    (16..=128).contains(&session_id.len())
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn valid_process_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 20
        && token.bytes().all(|byte| byte.is_ascii_digit())
        && token
            .parse::<u64>()
            .ok()
            .is_some_and(|value| value > 0 && value.to_string() == token)
}

/// The native process start time is a lifecycle token (FILETIME ticks), not Unix milliseconds.
#[cfg(windows)]
pub fn process_started_at(pid: u32) -> Result<u64, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return Err(format!("Cannot verify process lifecycle for PID {pid}"));
        }
        let mut creation = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit = creation;
        let mut kernel = creation;
        let mut user = creation;
        let success = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        CloseHandle(handle);
        if success == 0 {
            return Err(format!("Cannot read process start time for PID {pid}"));
        }
        // GetProcessTimes can succeed for a terminated process while an external handle is open.
        if exit.dwLowDateTime != 0 || exit.dwHighDateTime != 0 {
            return Err(format!("PID {pid} has exited"));
        }
        Ok(((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64)
    }
}

#[cfg(not(windows))]
pub fn process_started_at(_pid: u32) -> Result<u64, String> {
    Err(
        "Session lifecycle verification is supported only on Windows; registration is unavailable"
            .into(),
    )
}

pub fn ttl_expires_at(now: u64, ttl_ms: u64) -> Result<u64, String> {
    if ttl_ms == 0 || ttl_ms > MAX_EVIDENCE_TTL_MS {
        return Err(format!(
            "TTL must be in 1..={MAX_EVIDENCE_TTL_MS} milliseconds"
        ));
    }
    now.checked_add(ttl_ms).ok_or("Timestamp overflow".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> RegistrationRecord {
        RegistrationRecord {
            version: 1,
            session_id: "session-testing-123".into(),
            target_id: "target-a".into(),
            target_process: ProcessRef {
                pid: 40,
                started_at: "70".into(),
            },
            sender_process: ProcessRef {
                pid: 41,
                started_at: "80".into(),
            },
            environment: Environment::WindowsLocal,
            workspace_path: "C:\\app".into(),
            observed_at: 100,
            expires_at: 200,
        }
    }
    fn target() -> TargetInfo {
        TargetInfo {
            id: "target-a".into(),
            title: "Terminal".into(),
            app: "terminal".into(),
            process: record().target_process,
        }
    }
    #[test]
    fn a_live_matching_file_registration_still_has_no_active_tab_authority() {
        let evidence = record().validate(&target(), 150, true, true).unwrap();
        assert!(!evidence.verifiable);
        assert!(!evidence.active_scope);
    }
    #[test]
    fn stale_forged_lifetime_wrong_scope_and_dead_sender_are_rejected() {
        assert!(record().validate(&target(), 200, true, true).is_err());
        assert!(record().validate(&target(), 150, false, true).is_err());
        assert!(record().validate(&target(), 150, true, false).is_err());
        let mut changed = record();
        changed.target_process.started_at = "71".into();
        assert!(changed.validate(&target(), 150, true, true).is_err());
        changed = record();
        changed.target_id = "other-target".into();
        assert!(changed.validate(&target(), 150, true, true).is_err());
    }
    #[test]
    fn unknown_claimed_authentication_fields_are_rejected() {
        let mut value = serde_json::to_value(record()).unwrap();
        value["activeScope"] = serde_json::json!(true);
        assert!(serde_json::from_value::<RegistrationRecord>(value).is_err());
    }
    #[test]
    fn ttl_bounds_paths_and_session_ids_are_validated() {
        assert!(ttl_expires_at(100, 0).is_err());
        assert!(ttl_expires_at(100, MAX_EVIDENCE_TTL_MS + 1).is_err());
        assert!(ttl_expires_at(u64::MAX, 10).is_err());
        let mut changed = record();
        changed.workspace_path = "relative/path".into();
        assert!(changed.validate_shape(150).is_err());
        assert!(session_path(Path::new("sessions"), "../outside/session").is_err());
        for path in [
            r"\\wsl$\Ubuntu\app",
            r"\\wsl.localhost\Ubuntu\app",
            r"\\?\UNC\wsl$\Ubuntu\app",
            r"\\?\UNC\wsl.localhost\Ubuntu\app",
        ] {
            changed = record();
            changed.workspace_path = path.into();
            assert!(changed.validate_shape(150).is_err());
            changed.environment = Environment::Wsl;
            assert!(changed.validate_shape(150).is_ok());
        }
    }
    #[test]
    fn oversized_and_symlink_records_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("oversized.json");
        fs::write(&file, vec![b'x'; MAX_RECORD_BYTES as usize + 1]).unwrap();
        assert!(read_record(&file).is_err());
        #[cfg(unix)]
        {
            let link = directory.path().join("link.json");
            std::os::unix::fs::symlink(file, &link).unwrap();
            assert!(read_record(&link).is_err());
        }
    }
}
