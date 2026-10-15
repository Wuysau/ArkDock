//! Commit-boundary persistence for Agent pairing credentials.
//!
//! Replacing a pairing changes two things that cannot be swapped atomically:
//! the credential file the user hands to an Agent, and the token the backend
//! accepts. Workers only stage durable, private files. The actor validates scope
//! and swaps the file inside the bridge transaction, with a rollback backup if
//! cancellation wins during the swap. Installing the checked token then has no
//! fallible I/O. Rollback failures are reported distinctly and retain the backup
//! rather than silently claiming the previous pathname was restored.
//!
//! A file alone grants nothing: only the live pairing inside the bridge
//! validates a token, so a credential left behind by a crash is inert until a
//! later activation installs exactly that credential.
use crate::ipc::model::AgentPairing;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

const WAITING: u8 = 0;
const COMMITTING: u8 = 1;
const COMMITTED: u8 = 2;
const CANCELLED: u8 = 3;

/// One absolute command deadline, including queue and disk staging time.
/// Cancellation is nonblocking even while a local filesystem swap is underway.
pub(crate) struct CommitPermit {
    deadline: Instant,
    state: AtomicU8,
}

impl CommitPermit {
    pub(crate) fn new(timeout: Duration) -> Self {
        Self {
            deadline: Instant::now() + timeout,
            state: AtomicU8::new(WAITING),
        }
    }

    /// False means the commit already won; the caller must collect its reply.
    pub(crate) fn cancel(&self) -> bool {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            if state == COMMITTED {
                return false;
            }
            match self.state.compare_exchange_weak(
                state,
                CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(actual) => state = actual,
            }
        }
    }

    fn begin(&self) -> Result<(), String> {
        if Instant::now() >= self.deadline {
            self.cancel();
        }
        self.state
            .compare_exchange(WAITING, COMMITTING, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| "PAIRING_WRITE_TIMEOUT".into())
    }

    fn finish(&self) -> Result<(), String> {
        if Instant::now() >= self.deadline {
            self.cancel();
        }
        self.state
            .compare_exchange(COMMITTING, COMMITTED, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| "PAIRING_WRITE_TIMEOUT".into())
    }

    pub(crate) fn expired(&self) -> bool {
        self.state.load(Ordering::Acquire) == CANCELLED || Instant::now() >= self.deadline
    }

    pub(crate) fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

/// A worker owns only this unique file. Dropping a late worker's result removes
/// its staging file and can never mutate another request's final credential.
pub(crate) struct StagedCredential {
    temporary: PathBuf,
    path: PathBuf,
}

impl Drop for StagedCredential {
    fn drop(&mut self) {
        if !self.temporary.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

impl StagedCredential {
    pub(crate) fn commit(self, permit: &CommitPermit) -> Result<Option<PathBuf>, String> {
        self.commit_checked(permit, || {})
    }

    fn commit_checked(
        mut self,
        permit: &CommitPermit,
        after_swap: impl FnOnce(),
    ) -> Result<Option<PathBuf>, String> {
        permit.begin()?;
        let backup = sibling(&self.path, "backup")?;
        let replaced = swap(&self.temporary, &self.path, &backup)?;
        self.temporary = PathBuf::new();
        after_swap();
        if let Err(error) = permit.finish() {
            let restored = if replaced {
                swap_back(&backup, &self.path)
            } else {
                fs::remove_file(&self.path).map_err(|_| "PAIRING_ROLLBACK_FAILED".to_owned())
            };
            restored?;
            return Err(error);
        }
        // Cleanup is performed by the actor's caller only after activation.
        // A failed cleanup leaves an inert previous credential, never a partial
        // replacement or a broadened ACL.
        Ok(replaced.then_some(backup))
    }
}

pub(crate) fn stage_credential(
    path: &Path,
    pairing: &AgentPairing,
) -> Result<StagedCredential, String> {
    let bytes = encode(pairing)?;
    stage(path, &bytes)
}

/// Stages arbitrary credential bytes with the same local-disk bounds and private
/// temporary-file discipline. Used by the context-source pairing, whose credential
/// file has its own schema.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub(crate) fn stage_bytes(path: &Path, bytes: &[u8]) -> Result<StagedCredential, String> {
    stage(path, bytes)
}

fn stage(path: &Path, bytes: &[u8]) -> Result<StagedCredential, String> {
    // Resolve junctions off the actor, and reject remote/mapped drives before
    // the final synchronous commit. This is a local disk save, not network I/O.
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or("PAIRING_PATH_INVALID")?;
    #[cfg(windows)]
    ensure_local(parent)?;
    let parent = fs::canonicalize(parent).map_err(|_| "PAIRING_WRITE_FAILED")?;
    #[cfg(windows)]
    ensure_local(&parent)?;
    let path = parent.join(path.file_name().ok_or("PAIRING_PATH_INVALID")?);
    if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err("PAIRING_PATH_INVALID".into());
    }
    let temporary = sibling(&path, "tmp")?;
    let mut file = create_private(&temporary)?;
    let staged = StagedCredential { temporary, path };
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    written.map_err(|_| "PAIRING_WRITE_FAILED")?;
    Ok(staged)
}

fn sibling(path: &Path, suffix: &str) -> Result<PathBuf, String> {
    let parent = path.parent().ok_or("PAIRING_PATH_INVALID")?;
    #[cfg(windows)]
    let nonce = {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| "PAIRING_RANDOM_FAILED")?;
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    #[cfg(not(windows))]
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    );
    Ok(parent.join(format!(".arkdock-{nonce}.{suffix}")))
}

/// Writes credential bytes to an explicit, user-chosen path.
///
/// Tests inject deterministic failures; production uses staged credentials and
/// the bridge commit transaction instead of this synchronous fixture helper.
#[cfg(test)]
pub(crate) trait CredentialStore: Send + Sync {
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), String>;
}

/// Same-directory temporary file replaced onto the target path.
///
/// A partial write can therefore never be observed as a credential, an existing
/// credential is either fully replaced or untouched, and the bytes reach the
/// disk (not only the file cache) before the caller activates the pairing.
#[cfg(test)]
pub(crate) struct FileCredentials;

#[cfg(not(windows))]
static SERIAL: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
impl CredentialStore for FileCredentials {
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        if let Some(backup) =
            stage(path, bytes)?.commit(&CommitPermit::new(Duration::from_secs(4)))?
        {
            let _ = fs::remove_file(backup);
        }
        Ok(())
    }
}

#[cfg(not(windows))]
fn create_private(path: &Path) -> Result<File, String> {
    use std::fs::OpenOptions;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
        .open(path)
        .map_err(|_| "PAIRING_WRITE_FAILED".into())
}

#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(windows)]
fn ensure_local(path: &Path) -> Result<(), String> {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    const DRIVE_FIXED: u32 = 3;
    let text = path.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let bytes = text.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
    {
        return Err("PAIRING_LOCAL_DISK_REQUIRED".into());
    }
    let root: Vec<u16> = format!("{}:\\", bytes[0] as char)
        .encode_utf16()
        .chain(Some(0))
        .collect();
    if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_FIXED {
        return Err("PAIRING_LOCAL_DISK_REQUIRED".into());
    }
    Ok(())
}

#[cfg(windows)]
fn create_private(path: &Path) -> Result<File, String> {
    use std::os::windows::io::FromRawHandle;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{LocalFree, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL};
    let sddl: Vec<u16> = "D:P(A;;GA;;;OW)".encode_utf16().chain(Some(0)).collect();
    unsafe {
        let mut descriptor = null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        ) == 0
        {
            return Err("PAIRING_PRIVATE_ACL_FAILED".into());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let handle = CreateFileW(
            wide(path).as_ptr(),
            GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        );
        LocalFree(descriptor);
        if handle == INVALID_HANDLE_VALUE {
            return Err("PAIRING_WRITE_FAILED".into());
        }
        Ok(File::from_raw_handle(handle))
    }
}

#[cfg(windows)]
fn swap(temporary: &Path, path: &Path, backup: &Path) -> Result<bool, String> {
    use std::ptr::null;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, ReplaceFileW};
    let exists = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => true,
        Ok(_) => return Err("PAIRING_REPLACE_FAILED".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => return Err("PAIRING_REPLACE_FAILED".into()),
    };
    // ReplaceFile retains the existing destination's DACL; ordinary rename
    // would transfer the parent-inherited permissions of the temporary file.
    let success = unsafe {
        if exists {
            ReplaceFileW(
                wide(path).as_ptr(),
                wide(temporary).as_ptr(),
                wide(backup).as_ptr(),
                0,
                null(),
                null(),
            )
        } else {
            MoveFileExW(wide(temporary).as_ptr(), wide(path).as_ptr(), 0)
        }
    };
    if success == 0 {
        // ReplaceFile documents partial rename failures: if Windows created
        // the backup before failing, restore it instead of assuming the old
        // pathname still contains the previous credential.
        if exists && backup.exists() {
            swap_back(backup, path)?;
        }
        return Err("PAIRING_REPLACE_FAILED".into());
    }
    Ok(exists)
}

#[cfg(windows)]
fn swap_back(backup: &Path, path: &Path) -> Result<(), String> {
    use std::ptr::null;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, ReplaceFileW};
    let restored = if path.exists() {
        unsafe {
            ReplaceFileW(
                wide(path).as_ptr(),
                wide(backup).as_ptr(),
                null(),
                0,
                null(),
                null(),
            )
        }
    } else {
        unsafe { MoveFileExW(wide(backup).as_ptr(), wide(path).as_ptr(), 0) }
    };
    if restored == 0 {
        return Err("PAIRING_ROLLBACK_FAILED".into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn swap(temporary: &Path, path: &Path, backup: &Path) -> Result<bool, String> {
    let exists = path.exists();
    if exists {
        fs::hard_link(path, backup).map_err(|_| "PAIRING_REPLACE_FAILED")?;
        fs::set_permissions(
            temporary,
            fs::metadata(path)
                .map_err(|_| "PAIRING_REPLACE_FAILED")?
                .permissions(),
        )
        .map_err(|_| "PAIRING_REPLACE_FAILED")?;
    }
    if fs::rename(temporary, path).is_err() {
        if exists {
            let _ = fs::remove_file(backup);
        }
        return Err("PAIRING_REPLACE_FAILED".into());
    }
    Ok(exists)
}

#[cfg(not(windows))]
fn swap_back(backup: &Path, path: &Path) -> Result<(), String> {
    fs::rename(backup, path).map_err(|_| "PAIRING_ROLLBACK_FAILED".into())
}

/// The exact bytes an Agent is asked to read: the same struct the backend
/// activates, including the path it was saved to.
pub(crate) fn encode(pairing: &AgentPairing) -> Result<Vec<u8>, String> {
    serde_json::to_vec_pretty(pairing).map_err(|_| "PAIRING_SERIALIZE_FAILED".to_owned())
}

/// Persists a credential. The caller must treat `Ok` as the permission to
/// activate the pairing and any error as "the previous pairing is unchanged".
#[cfg(test)]
pub(crate) fn write_credential(path: &Path, pairing: &AgentPairing) -> Result<(), String> {
    FileCredentials.write(path, &encode(pairing)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::model::AgentKind;

    fn pairing() -> AgentPairing {
        AgentPairing {
            pairing_id: "pairing-fixture".into(),
            endpoint: "http://127.0.0.1:1".into(),
            token: "a".repeat(64),
            agent_kind: AgentKind::Claude,
            workspace_id: "workspace".into(),
            workspace_root: "C:/workspace".into(),
            expires_at: 1_000,
            config_path: None,
        }
    }

    fn commit_now(staged: StagedCredential) {
        if let Some(backup) = staged
            .commit(&CommitPermit::new(Duration::from_secs(2)))
            .unwrap()
        {
            fs::remove_file(backup).unwrap();
        }
    }

    #[test]
    fn late_cancelled_stage_cannot_replace_a_successful_retry() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pairing.json");
        write_credential(&path, &pairing()).unwrap();
        let a = stage_credential(&path, &pairing()).unwrap();
        let permit = CommitPermit::new(Duration::from_secs(2));
        assert!(permit.cancel());
        let mut retry = pairing();
        retry.pairing_id = "retry-b".into();
        commit_now(stage_credential(&path, &retry).unwrap());
        assert_eq!(a.commit(&permit).unwrap_err(), "PAIRING_WRITE_TIMEOUT");
        assert_eq!(fs::read(&path).unwrap(), encode(&retry).unwrap());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn cancellation_during_swap_restores_the_previous_credential() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pairing.json");
        let previous = pairing();
        write_credential(&path, &previous).unwrap();
        let permit = CommitPermit::new(Duration::from_secs(2));
        let mut next = pairing();
        next.pairing_id = "cancelled".into();
        let error = stage_credential(&path, &next)
            .unwrap()
            .commit_checked(&permit, || {
                assert!(permit.cancel());
            })
            .unwrap_err();
        assert_eq!(error, "PAIRING_WRITE_TIMEOUT");
        assert_eq!(fs::read(&path).unwrap(), encode(&previous).unwrap());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn cancellation_during_first_swap_removes_the_unactivated_credential() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pairing.json");
        let permit = CommitPermit::new(Duration::from_secs(2));
        assert_eq!(
            stage_credential(&path, &pairing())
                .unwrap()
                .commit_checked(&permit, || {
                    permit.cancel();
                })
                .unwrap_err(),
            "PAIRING_WRITE_TIMEOUT"
        );
        assert!(!path.exists());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn queued_request_deadline_and_exclusive_creation_are_enforced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pairing.json");
        let staged = stage_credential(&path, &pairing()).unwrap();
        let expired = CommitPermit::new(Duration::ZERO);
        assert!(expired.expired());
        assert_eq!(expired.remaining(), Duration::ZERO);
        assert_eq!(
            staged.commit(&expired).unwrap_err(),
            "PAIRING_WRITE_TIMEOUT"
        );
        assert!(!path.exists());
        let occupied = directory.path().join("occupied.tmp");
        fs::write(&occupied, b"existing private data").unwrap();
        assert!(create_private(&occupied).is_err());
        assert_eq!(fs::read(&occupied).unwrap(), b"existing private data");
    }

    #[cfg(windows)]
    fn dacl(path: &Path) -> Vec<u8> {
        use std::ptr::null_mut;
        use windows_sys::Win32::Security::{GetFileSecurityW, DACL_SECURITY_INFORMATION};
        let mut needed = 0;
        unsafe {
            GetFileSecurityW(
                wide(path).as_ptr(),
                DACL_SECURITY_INFORMATION,
                null_mut(),
                0,
                &mut needed,
            );
        }
        assert!(needed > 0);
        let mut descriptor = vec![0; needed as usize];
        assert_ne!(
            unsafe {
                GetFileSecurityW(
                    wide(path).as_ptr(),
                    DACL_SECURITY_INFORMATION,
                    descriptor.as_mut_ptr().cast(),
                    needed,
                    &mut needed,
                )
            },
            0
        );
        // Moving the file can set SE_DACL_AUTO_INHERITED even for a protected
        // DACL. Its access entries and SE_DACL_PROTECTED must remain identical.
        assert_ne!(descriptor[3] & 0x10, 0, "DACL must be protected");
        descriptor[3] &= !0x04;
        descriptor
    }

    #[cfg(windows)]
    #[test]
    fn windows_private_stage_and_replacement_preserve_the_restricted_dacl() {
        use std::ptr::null_mut;
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
        use windows_sys::Win32::Security::{GetAce, GetSecurityDescriptorDacl, ACCESS_ALLOWED_ACE};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pairing.json");
        let staged = stage_credential(&path, &pairing()).unwrap();
        let private = dacl(&staged.temporary);
        let mut present = 0;
        let mut acl = null_mut();
        let mut defaulted = 0;
        let mut descriptor = private.clone();
        unsafe {
            assert_ne!(
                GetSecurityDescriptorDacl(
                    descriptor.as_mut_ptr().cast(),
                    &mut present,
                    &mut acl,
                    &mut defaulted
                ),
                0
            );
            assert_ne!(present, 0);
            assert!(!acl.is_null());
            assert_eq!(
                (*acl).AceCount,
                1,
                "only owner rights may read staged credentials"
            );
            let mut ace = null_mut();
            assert_ne!(GetAce(acl, 0, &mut ace), 0);
            let ace = &*(ace as *const ACCESS_ALLOWED_ACE);
            assert_eq!(ace.Header.AceType, 0, "owner ACE must allow access");
            let mut sid_text = null_mut();
            assert_ne!(
                ConvertSidToStringSidW(
                    (&ace.SidStart as *const u32).cast_mut().cast(),
                    &mut sid_text
                ),
                0
            );
            let mut length = 0;
            while *sid_text.add(length) != 0 {
                length += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(sid_text, length));
            LocalFree(sid_text.cast());
            assert_eq!(sid, "S-1-3-4", "the ACL must use OWNER RIGHTS");
        }
        commit_now(staged);
        assert_eq!(dacl(&path), private);
        let original = dacl(&path);
        commit_now(stage_credential(&path, &pairing()).unwrap());
        assert_eq!(dacl(&path), original);
        assert_eq!(
            ensure_local(Path::new(r"\\server\share\pairing.json")).unwrap_err(),
            "PAIRING_LOCAL_DISK_REQUIRED"
        );
    }

    #[test]
    fn timeout_cancellation_never_waits_for_an_in_progress_swap() {
        use std::sync::{mpsc, Arc};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pairing.json");
        write_credential(&path, &pairing()).unwrap();
        let before = fs::read(&path).unwrap();
        let staged = stage_credential(&path, &pairing()).unwrap();
        let permit = Arc::new(CommitPermit::new(Duration::from_secs(2)));
        let worker_permit = permit.clone();
        let (swapped, ready) = mpsc::channel();
        let (resume, continue_commit) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            staged.commit_checked(&worker_permit, || {
                swapped.send(()).unwrap();
                continue_commit
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap();
            })
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        // The worker is held after the swap. Cancellation must return while it
        // is still held, instead of waiting for a filesystem transaction lock.
        assert!(permit.cancel());
        resume.send(()).unwrap();
        assert_eq!(worker.join().unwrap().unwrap_err(), "PAIRING_WRITE_TIMEOUT");
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn credential_replacement_leaves_no_temporary_and_keeps_only_the_new_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("arkdock-pairing.json");
        let first = pairing();
        let mut second = pairing();
        second.pairing_id = "pairing-second".into();
        write_credential(&path, &first).unwrap();
        write_credential(&path, &second).unwrap();
        let stored: AgentPairing = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored.pairing_id, "pairing-second");
        let leftovers: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "arkdock-pairing.json")
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files remained: {leftovers:?}"
        );
    }

    #[test]
    fn unwritable_directory_and_directory_target_fail_without_touching_the_previous_credential() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("arkdock-pairing.json");
        write_credential(&path, &pairing()).unwrap();
        let before = fs::read(&path).unwrap();
        // A missing parent directory cannot be written into.
        let missing = directory.path().join("absent").join("arkdock-pairing.json");
        assert_eq!(
            write_credential(&missing, &pairing()).unwrap_err(),
            "PAIRING_WRITE_FAILED"
        );
        assert!(!missing.exists());
        // A directory target cannot be replaced by a file.
        let occupied = directory.path().join("occupied.json");
        fs::create_dir(&occupied).unwrap();
        assert_eq!(
            write_credential(&occupied, &pairing()).unwrap_err(),
            "PAIRING_REPLACE_FAILED"
        );
        assert!(occupied.is_dir());
        // The failing writes above never disturbed the credential that was there.
        assert_eq!(fs::read(&path).unwrap(), before);
        let leftovers: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with('.'))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files remained: {leftovers:?}"
        );
    }

    #[test]
    fn encoded_credential_round_trips_as_the_activated_struct() {
        let mut pairing = pairing();
        pairing.config_path = Some("C:/saved/arkdock-pairing.json".into());
        let bytes = encode(&pairing).unwrap();
        assert!(!bytes.contains(&0));
        assert!(bytes.len() < 16 * 1024);
        let decoded: AgentPairing = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded, pairing);
    }
}
