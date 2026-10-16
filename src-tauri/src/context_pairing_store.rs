//! App-owned persistence for the context pairing, independent of any project.
//!
//! Export and persistence commit under one lock; only then may the hub accept
//! the credential. Revocation persists an empty record before clearing the hub.
//! No secret is included in an error, diagnostic, or IPC response.
//!
//! Each replacement is atomic, but the two files are not one crash transaction:
//! process termination between export and state replacement can leave a newer
//! export than the restored token. Pairing again recovers that interrupted save.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::agent::pairing::{stage_bytes, CommitPermit, StagedCredential};
use crate::context::{ContextHub, CONTEXT_PIPE_NAME};
use crate::ipc::model::ContextPairing;

const STATE_FILE: &str = "context-pairing-state.json";
const MAX_STATE_BYTES: u64 = 4_096;
const COMMIT_TIMEOUT: Duration = Duration::from_secs(4);

/// The credential an agent reads at runtime; never passed on argv.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PairingFile {
    version: u32,
    pipe_name: String,
    credential: String,
    created_at: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedPairing {
    version: u32,
    pairing: Option<PairingFile>,
}

/// One instance must be shared by startup and every create/revoke command.
pub(crate) struct ContextPairingStore {
    path: PathBuf,
    operation: Mutex<()>,
}

impl ContextPairingStore {
    pub(crate) fn new(app_data_dir: PathBuf) -> Self {
        Self {
            path: app_data_dir.join(STATE_FILE),
            operation: Mutex::new(()),
        }
    }

    /// Called before the listener starts. Missing/revoked state is unpaired;
    /// invalid state fails closed without reading any user-selected export.
    pub(crate) fn restore(&self, hub: &ContextHub) -> Result<bool, String> {
        let _operation = self.lock()?;
        let token = match self.read_token() {
            Ok(token) => token,
            Err(error) => {
                hub.revoke_token();
                return Err(error);
            }
        };
        if let Some(token) = token {
            hub.activate_token(token);
            Ok(true)
        } else {
            hub.revoke_token();
            Ok(false)
        }
    }

    #[cfg(feature = "desktop")]
    pub(crate) fn create(&self, export: &Path, hub: &ContextHub) -> Result<ContextPairing, String> {
        let (_, token) = crate::context::new_pairing()?;
        self.install(export, hub, token)
    }

    fn install(
        &self,
        export: &Path,
        hub: &ContextHub,
        token: [u8; 32],
    ) -> Result<ContextPairing, String> {
        let _operation = self.lock()?;
        self.ensure_parent()?;
        let export = self.export_path(export)?;
        // The user pastes this value into another tool's configuration, so it must be the
        // ordinary spelling, not the extended-length one `canonicalize` returns.
        let pairing_file = reported_path(&export)?;
        let file = PairingFile {
            version: 1,
            pipe_name: CONTEXT_PIPE_NAME.to_owned(),
            credential: token.iter().map(|byte| format!("{byte:02x}")).collect(),
            created_at: crate::context::now_millis(),
        };
        let encoded =
            serde_json::to_vec_pretty(&file).map_err(|_| "CONTEXT_PAIRING_SERIALIZE_FAILED")?;
        let staged_export = stage_bytes(&export, &encoded)?;
        let state = PersistedPairing {
            version: 1,
            pairing: Some(file),
        };
        let staged_state = self.stage_state(&state)?;
        commit_pairing(&export, staged_export, hub, token, || {
            staged_state.commit(&CommitPermit::new(COMMIT_TIMEOUT))
        })?;
        Ok(ContextPairing {
            pipe_name: CONTEXT_PIPE_NAME.to_owned(),
            pairing_file,
        })
    }

    pub(crate) fn revoke(&self, hub: &ContextHub) -> Result<bool, String> {
        let _operation = self.lock()?;
        self.ensure_parent()?;
        let state = PersistedPairing {
            version: 1,
            pairing: None,
        };
        let backup = self
            .stage_state(&state)?
            .commit(&CommitPermit::new(COMMIT_TIMEOUT))?;
        let revoked = hub.revoke_token();
        remove_backup(backup);
        Ok(revoked)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ()>, String> {
        self.operation
            .lock()
            .map_err(|_| "CONTEXT_PAIRING_STATE_UNAVAILABLE".into())
    }

    fn ensure_parent(&self) -> Result<(), String> {
        fs::create_dir_all(self.path.parent().ok_or("CONTEXT_PAIRING_PATH_INVALID")?)
            .map_err(|_| "CONTEXT_PAIRING_WRITE_FAILED".into())
    }

    fn export_path(&self, export: &Path) -> Result<PathBuf, String> {
        let parent = export
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or("CONTEXT_PAIRING_PATH_INVALID")?;
        let parent = fs::canonicalize(parent).map_err(|_| "CONTEXT_PAIRING_WRITE_FAILED")?;
        let export = parent.join(export.file_name().ok_or("CONTEXT_PAIRING_PATH_INVALID")?);
        let owned = fs::canonicalize(self.path.parent().ok_or("CONTEXT_PAIRING_PATH_INVALID")?)
            .map_err(|_| "CONTEXT_PAIRING_WRITE_FAILED")?
            .join(STATE_FILE);
        // Existing paths may use a short Windows filename or a symlink alias;
        // resolve those too before allowing the user-selected export destination.
        let resolved_export = fs::canonicalize(&export).unwrap_or_else(|_| export.clone());
        let resolved_owned = fs::canonicalize(&owned).unwrap_or(owned);
        #[cfg(windows)]
        let same_path = resolved_export.to_string_lossy().to_lowercase()
            == resolved_owned.to_string_lossy().to_lowercase();
        #[cfg(not(windows))]
        let same_path = resolved_export == resolved_owned;
        if same_path {
            return Err("CONTEXT_PAIRING_PATH_INVALID".into());
        }
        Ok(export)
    }

    fn stage_state(&self, state: &PersistedPairing) -> Result<StagedCredential, String> {
        let encoded =
            serde_json::to_vec_pretty(state).map_err(|_| "CONTEXT_PAIRING_SERIALIZE_FAILED")?;
        stage_bytes(&self.path, &encoded)
    }

    fn read_token(&self) -> Result<Option<[u8; 32]>, String> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("CONTEXT_PAIRING_READ_FAILED".into()),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("CONTEXT_PAIRING_STATE_INVALID".into());
        }
        let mut bytes = Vec::new();
        File::open(&self.path)
            .and_then(|file| file.take(MAX_STATE_BYTES + 1).read_to_end(&mut bytes))
            .map_err(|_| "CONTEXT_PAIRING_READ_FAILED")?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            return Err("CONTEXT_PAIRING_STATE_INVALID".into());
        }
        let state: PersistedPairing =
            serde_json::from_slice(&bytes).map_err(|_| "CONTEXT_PAIRING_STATE_INVALID")?;
        if state.version != 1 {
            return Err("CONTEXT_PAIRING_STATE_INVALID".into());
        }
        let Some(pairing) = state.pairing else {
            return Ok(None);
        };
        if pairing.version != 1 || pairing.pipe_name != CONTEXT_PIPE_NAME {
            return Err("CONTEXT_PAIRING_STATE_INVALID".into());
        }
        decode_token(&pairing.credential).map(Some)
    }
}

/// The path this backend reports for a user-selected export.
///
/// Windows `canonicalize` returns an extended-length path (`\\?\D:\...`). That spelling
/// is correct for our own file access, but the pairing path is not ours to consume: the
/// user must paste it into another tool's configuration, where quoting, JSON/TOML
/// escaping and host-side normalization all become error-prone and the prefix carries no
/// ordinary-path meaning. Report the ordinary spelling instead; every filesystem
/// operation still uses the canonical path.
///
/// This mirrors `src/path-display.ts::displayPath` on the frontend. The contract must be
/// clean on its own, so both layers apply the rule.
fn reported_path(path: &Path) -> Result<String, String> {
    let value = path.to_str().ok_or("CONTEXT_PAIRING_PATH_INVALID")?;
    #[cfg(windows)]
    {
        Ok(ordinary_windows_path(value))
    }
    #[cfg(not(windows))]
    {
        Ok(value.to_owned())
    }
}

/// Pure form of the Windows rule, so it is testable on every host.
///
/// `\\?\C:\dir\x.json` becomes `C:\dir\x.json`, and `\\?\UNC\server\share\x.json`
/// becomes `\\server\share\x.json` (a network remainder needs both a server and a
/// share). Device and volume namespaces (`\\?\Volume{...}`, `\\?\GLOBALROOT\...`) have
/// no ordinary equivalent and stay unchanged, as does anything that is not an
/// extended-length path.
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
    let drive_letter = matches!((characters.next(), characters.next()), (Some(letter), Some(':')) if letter.is_ascii_alphabetic());
    if drive_letter {
        remainder.to_owned()
    } else {
        value.to_owned()
    }
}

fn decode_token(credential: &str) -> Result<[u8; 32], String> {
    if credential.len() != 64
        || !credential
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("CONTEXT_PAIRING_STATE_INVALID".into());
    }
    let mut token = [0; 32];
    for (index, value) in token.iter_mut().enumerate() {
        *value = u8::from_str_radix(&credential[index * 2..index * 2 + 2], 16)
            .map_err(|_| "CONTEXT_PAIRING_STATE_INVALID")?;
    }
    Ok(token)
}

fn commit_pairing(
    export: &Path,
    staged_export: StagedCredential,
    hub: &ContextHub,
    token: [u8; 32],
    commit_state: impl FnOnce() -> Result<Option<PathBuf>, String>,
) -> Result<(), String> {
    let export_backup = staged_export.commit(&CommitPermit::new(COMMIT_TIMEOUT))?;
    let state_backup = match commit_state() {
        Ok(backup) => backup,
        Err(error) => {
            rollback_export(export, export_backup.as_deref())?;
            return Err(error);
        }
    };
    hub.activate_token(token);
    remove_backup(export_backup);
    remove_backup(state_backup);
    Ok(())
}

fn remove_backup(backup: Option<PathBuf>) {
    if let Some(backup) = backup {
        let _ = fs::remove_file(backup);
    }
}

fn rollback_export(export: &Path, backup: Option<&Path>) -> Result<(), String> {
    const ERROR: &str = "CONTEXT_PAIRING_EXPORT_ROLLBACK_FAILED";
    let Some(backup) = backup else {
        return fs::remove_file(export).map_err(|_| ERROR.to_owned());
    };
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use std::ptr::null;
        use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, ReplaceFileW};
        let wide = |path: &Path| {
            path.as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>()
        };
        let restored = if export.exists() {
            unsafe {
                ReplaceFileW(
                    wide(export).as_ptr(),
                    wide(backup).as_ptr(),
                    null(),
                    0,
                    null(),
                    null(),
                )
            }
        } else {
            unsafe { MoveFileExW(wide(backup).as_ptr(), wide(export).as_ptr(), 0) }
        };
        if restored == 0 {
            return Err(ERROR.into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::rename(backup, export).map_err(|_| ERROR.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextMessage;
    use crate::ipc::model::ProcessRef;

    fn accepts(hub: &ContextHub, byte: u8) -> bool {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        hub.apply(
            ContextMessage::Hello {
                token: format!("{byte:02x}").repeat(32),
                session: format!("pairing-test-session-{serial}"),
                sequence: 1,
                roots: vec![],
                cwd: None,
                agent: None,
                host_session: None,
                original_project: None,
                execution_directory: None,
                environment: None,
            },
            ProcessRef {
                pid: 42,
                started_at: "1".into(),
            },
            1,
            1_000,
        )
        .is_ok()
    }

    #[test]
    fn restart_restores_the_exported_credential_without_the_export_file() {
        let directory = tempfile::tempdir().unwrap();
        let store = ContextPairingStore::new(directory.path().join("app-data"));
        let export = directory.path().join("agent.json");
        let hub = ContextHub::new();
        store.install(&export, &hub, [0xaa; 32]).unwrap();
        let file: PairingFile = serde_json::from_slice(&fs::read(&export).unwrap()).unwrap();
        assert_eq!(file.credential, "aa".repeat(32));
        assert!(accepts(&hub, 0xaa));
        fs::remove_file(export).unwrap();
        let restarted = ContextHub::new();
        let reopened = ContextPairingStore::new(directory.path().join("app-data"));
        assert!(reopened.restore(&restarted).unwrap());
        assert!(accepts(&restarted, 0xaa));
        assert!(!accepts(&restarted, 0xbb));
    }

    #[test]
    fn revocation_survives_restart_even_when_the_export_still_exists() {
        let directory = tempfile::tempdir().unwrap();
        let store = ContextPairingStore::new(directory.path().to_owned());
        let export = directory.path().join("agent.json");
        let hub = ContextHub::new();
        store.install(&export, &hub, [0xaa; 32]).unwrap();
        assert!(store.revoke(&hub).unwrap());
        assert!(!store.revoke(&hub).unwrap());
        assert!(export.exists());
        let restarted = ContextHub::new();
        assert!(!store.restore(&restarted).unwrap());
        assert!(!accepts(&restarted, 0xaa));
    }

    #[test]
    fn failed_export_preserves_the_previous_live_and_persisted_pairing() {
        let directory = tempfile::tempdir().unwrap();
        let store = ContextPairingStore::new(directory.path().to_owned());
        let export = directory.path().join("agent.json");
        let hub = ContextHub::new();
        store.install(&export, &hub, [0xaa; 32]).unwrap();
        let before = fs::read(&store.path).unwrap();
        assert!(store.install(directory.path(), &hub, [0xbb; 32]).is_err());
        assert_eq!(fs::read(&store.path).unwrap(), before);
        assert!(accepts(&hub, 0xaa));
        assert!(!accepts(&hub, 0xbb));
    }

    #[test]
    fn failed_persistence_restores_existing_export_and_never_activates() {
        let directory = tempfile::tempdir().unwrap();
        let store = ContextPairingStore::new(directory.path().to_owned());
        let export = directory.path().join("agent.json");
        let hub = ContextHub::new();
        store.install(&export, &hub, [0xaa; 32]).unwrap();
        let previous = fs::read(&export).unwrap();
        let staged = stage_bytes(&export, b"replacement").unwrap();
        assert_eq!(
            commit_pairing(&export, staged, &hub, [0xbb; 32], || {
                Err("STATE_COMMIT_FAILED".into())
            }),
            Err("STATE_COMMIT_FAILED".into())
        );
        assert_eq!(fs::read(&export).unwrap(), previous);
        assert!(accepts(&hub, 0xaa));
        assert!(!accepts(&hub, 0xbb));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn failed_persistence_removes_a_new_export() {
        let directory = tempfile::tempdir().unwrap();
        let export = directory.path().join("agent.json");
        let hub = ContextHub::new();
        let staged = stage_bytes(&export, b"replacement").unwrap();
        assert!(commit_pairing(&export, staged, &hub, [0xbb; 32], || {
            Err("STATE_COMMIT_FAILED".into())
        })
        .is_err());
        assert!(!export.exists());
        assert!(!hub.paired());
    }

    #[test]
    fn failed_revoke_persistence_keeps_the_live_pairing() {
        let directory = tempfile::tempdir().unwrap();
        let store = ContextPairingStore::new(directory.path().to_owned());
        let hub = ContextHub::new();
        hub.activate_token([0xaa; 32]);
        // A directory cannot be replaced by the persisted revocation record.
        fs::create_dir(&store.path).unwrap();
        assert!(store.revoke(&hub).is_err());
        assert!(accepts(&hub, 0xaa));
    }

    #[test]
    fn app_owned_state_cannot_be_selected_as_the_export() {
        let directory = tempfile::tempdir().unwrap();
        let store = ContextPairingStore::new(directory.path().to_owned());
        assert!(store
            .install(&store.path, &ContextHub::new(), [0xaa; 32])
            .is_err());
        assert!(!store.path.exists());
    }

    #[test]
    fn the_reported_path_drops_only_a_removable_extended_length_prefix() {
        // An ordinary drive path is what the host configuration needs.
        assert_eq!(
            ordinary_windows_path(r"\\?\D:\Projects\demo\arkdock-context-pairing.json"),
            r"D:\Projects\demo\arkdock-context-pairing.json"
        );
        assert_eq!(
            ordinary_windows_path(r"\\?\UNC\server\share\arkdock-context-pairing.json"),
            r"\\server\share\arkdock-context-pairing.json"
        );
        // Device and volume namespaces have no ordinary spelling.
        for value in [
            r"\\?\Volume{0f9a1b2c-0000-0000-0000-100000000000}\arkdock-context-pairing.json",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\arkdock-context-pairing.json",
            // A network part without a server and a share is not a UNC path.
            r"\\?\UNC\\share\arkdock-context-pairing.json",
        ] {
            assert_eq!(ordinary_windows_path(value), value, "{value}");
        }
        // Already ordinary paths, UNC paths and POSIX paths are unchanged.
        for value in [
            r"D:\arkdock-context-pairing.json",
            r"\\server\share\arkdock-context-pairing.json",
            r"\\.\pipe\arkdock-context",
            "/home/user/arkdock-context-pairing.json",
        ] {
            assert_eq!(ordinary_windows_path(value), value, "{value}");
        }
    }

    #[test]
    fn the_reported_pairing_path_names_the_exported_credential() {
        let directory = tempfile::tempdir().unwrap();
        let store = ContextPairingStore::new(directory.path().join("app-data"));
        let export = directory.path().join("arkdock-context-pairing.json");
        let hub = ContextHub::new();
        let pairing = store.install(&export, &hub, [0xaa; 32]).unwrap();
        // The path the user copies must be usable as-is, without the extended-length
        // prefix that Windows canonicalization would otherwise introduce.
        assert!(
            !pairing.pairing_file.starts_with(r"\\?\"),
            "{}",
            pairing.pairing_file
        );
        let file: PairingFile =
            serde_json::from_slice(&fs::read(&pairing.pairing_file).unwrap()).unwrap();
        assert_eq!(file.credential, "aa".repeat(32));
        assert_eq!(file.pipe_name, CONTEXT_PIPE_NAME);
    }

    #[cfg(windows)]
    #[test]
    fn windows_case_and_short_name_aliases_cannot_overwrite_app_owned_state() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

        let directory = tempfile::tempdir().unwrap();
        let store = ContextPairingStore::new(directory.path().to_owned());
        let hub = ContextHub::new();
        store
            .install(&directory.path().join("agent.json"), &hub, [0xaa; 32])
            .unwrap();
        let before = fs::read(&store.path).unwrap();
        let upper = directory.path().join(STATE_FILE.to_uppercase());
        assert!(store.install(&upper, &hub, [0xbb; 32]).is_err());

        let wide: Vec<_> = store
            .path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut short = vec![0; 32_768];
        let length =
            unsafe { GetShortPathNameW(wide.as_ptr(), short.as_mut_ptr(), short.len() as u32) };
        assert!(length > 0 && (length as usize) < short.len());
        let alias = PathBuf::from(std::ffi::OsString::from_wide(&short[..length as usize]));
        assert!(store.install(&alias, &hub, [0xbb; 32]).is_err());
        assert_eq!(fs::read(&store.path).unwrap(), before);
        assert!(accepts(&hub, 0xaa));
    }

    #[test]
    fn invalid_or_oversized_persistence_never_activates_a_token() {
        let directory = tempfile::tempdir().unwrap();
        let store = ContextPairingStore::new(directory.path().to_owned());
        for contents in [
            b"not-json".to_vec(),
            br#"{"version":2,"pairing":null}"#.to_vec(),
            br#"{"version":1,"pairing":null,"extra":true}"#.to_vec(),
            vec![b' '; MAX_STATE_BYTES as usize + 1],
        ] {
            fs::write(&store.path, contents).unwrap();
            let hub = ContextHub::new();
            assert!(store.restore(&hub).is_err());
            assert!(!hub.paired());
        }
        for token in ["f".repeat(63), "g".repeat(64), "é".repeat(32)] {
            assert!(decode_token(&token).is_err());
        }
    }

    #[test]
    fn concurrent_rotations_leave_export_persistence_and_hub_in_agreement() {
        let directory = tempfile::tempdir().unwrap();
        let store = ContextPairingStore::new(directory.path().to_owned());
        let export = directory.path().join("agent.json");
        let hub = ContextHub::new();
        std::thread::scope(|scope| {
            for token in [0xaa, 0xbb, 0xcc, 0xdd] {
                let store = &store;
                let export = &export;
                let hub = &hub;
                scope.spawn(move || store.install(export, hub, [token; 32]).unwrap());
            }
        });
        let file: PairingFile = serde_json::from_slice(&fs::read(&export).unwrap()).unwrap();
        let token = decode_token(&file.credential).unwrap();
        assert!(accepts(&hub, token[0]));
        let restarted = ContextHub::new();
        assert!(store.restore(&restarted).unwrap());
        assert!(accepts(&restarted, token[0]));
    }

    #[test]
    fn concurrent_revoke_and_create_leave_persistence_and_hub_in_agreement() {
        let directory = tempfile::tempdir().unwrap();
        let store = ContextPairingStore::new(directory.path().to_owned());
        let export = directory.path().join("agent.json");
        let hub = ContextHub::new();
        store.install(&export, &hub, [0xaa; 32]).unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| store.revoke(&hub).unwrap());
            scope.spawn(|| store.install(&export, &hub, [0xbb; 32]).unwrap());
        });
        let restarted = ContextHub::new();
        assert_eq!(store.restore(&restarted).unwrap(), hub.paired());
        assert_eq!(accepts(&hub, 0xbb), accepts(&restarted, 0xbb));
        assert!(!accepts(&hub, 0xaa));
    }
}
