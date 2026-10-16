//! Bounded lifecycle for native preview snapshots. Cleanup runs off the Engine
//! thread and never traverses a junction, symlink or other reparse directory.
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, OnceLock};
use std::time::Duration;

const MAX_OWNED_RUNS: usize = 8;
const MAX_CLEANUP_ENTRIES: usize = 200_000;
static OWNED_RUNS: AtomicUsize = AtomicUsize::new(0);
static CLEANER: OnceLock<Result<mpsc::SyncSender<Cleanup>, String>> = OnceLock::new();

struct Permit;
impl Permit {
    fn acquire() -> Result<Self, String> {
        OWNED_RUNS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_OWNED_RUNS).then_some(count + 1)
            })
            .map(|_| Self)
            .map_err(|_| "NATIVE_TEMP_CLEANUP_BUSY: 旧运行目录尚未清理，暂不创建更多快照。".into())
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        OWNED_RUNS.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Cleanup {
    root: PathBuf,
    path: PathBuf,
    _permit: Permit,
}

fn cleaner() -> Result<&'static mpsc::SyncSender<Cleanup>, String> {
    CLEANER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::sync_channel::<Cleanup>(MAX_OWNED_RUNS);
            std::thread::Builder::new()
                .name("arkdock-preview-cleanup".into())
                .spawn(move || {
                    let mut retry = Vec::new();
                    loop {
                        match receiver.recv_timeout(Duration::from_millis(500)) {
                            Ok(job) => retry.push(job),
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                        }
                        let mut pending = Vec::new();
                        for job in retry.drain(..) {
                            if remove_owned_run(&job.root, &job.path).is_err() {
                                // Sharing violations can outlive Job termination briefly.
                                // Retaining the permit caps both retries and total disk
                                // snapshots; a permanently locked tree blocks new copies.
                                pending.push(job);
                            }
                        }
                        retry = pending;
                    }
                })
                .map_err(|error| format!("NATIVE_TEMP_CLEANER_UNAVAILABLE: {error}"))?;
            Ok(sender)
        })
        .as_ref()
        .map_err(Clone::clone)
}

pub(super) struct OwnedRunDirectory {
    cleanup: Option<Cleanup>,
    sender: mpsc::SyncSender<Cleanup>,
}
impl OwnedRunDirectory {
    pub fn create(nonce: &str) -> Result<Self, String> {
        if !valid_nonce(nonce) {
            return Err("NATIVE_TEMP_NONCE_INVALID".into());
        }
        let sender = cleaner()?.clone();
        let permit = Permit::acquire()?;
        let temporary = std::env::temp_dir()
            .canonicalize()
            .map_err(|error| format!("NATIVE_TEMP_ROOT: {error}"))?;
        let root = temporary.join("ArkDock-native");
        match fs::create_dir(&root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("NATIVE_TEMP_ROOT: {error}")),
        }
        let metadata =
            fs::symlink_metadata(&root).map_err(|error| format!("NATIVE_TEMP_ROOT: {error}"))?;
        if !metadata.is_dir() || reparse(&metadata) {
            return Err("NATIVE_TEMP_ROOT_REPARSE".into());
        }
        let root = root
            .canonicalize()
            .map_err(|error| format!("NATIVE_TEMP_ROOT: {error}"))?;
        if root.parent() != Some(temporary.as_path()) {
            return Err("NATIVE_TEMP_ROOT_CHANGED".into());
        }
        let path = root.join(nonce);
        // Never adopt an existing directory, even when the generated name collides.
        fs::create_dir(&path).map_err(|error| format!("NATIVE_TEMP_CREATE: {error}"))?;
        Ok(Self {
            cleanup: Some(Cleanup {
                root,
                path,
                _permit: permit,
            }),
            sender,
        })
    }
    pub fn path(&self) -> &Path {
        &self.cleanup.as_ref().expect("live owned directory").path
    }
}
impl Drop for OwnedRunDirectory {
    fn drop(&mut self) {
        if let Some(job) = self.cleanup.take() {
            // There are at most eight permits across live runs, the queue and
            // the worker, so this eight-slot queue cannot fill here. Never wait
            // for disk work on the Engine thread.
            if let Err(error) = self.sender.try_send(job) {
                // Fail closed if the cleaner unexpectedly died: retaining this
                // permit prevents subsequent runs from growing disk unboundedly.
                let (mpsc::TrySendError::Full(job) | mpsc::TrySendError::Disconnected(job)) = error;
                std::mem::forget(job);
            }
        }
    }
}

fn valid_nonce(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}

fn remove_owned_run(root: &Path, path: &Path) -> Result<(), String> {
    if root.file_name().and_then(|name| name.to_str()) != Some("ArkDock-native")
        || path.parent() != Some(root)
        || !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(valid_nonce)
    {
        return Err("NATIVE_TEMP_CLEANUP_SCOPE".into());
    }
    let metadata = fs::symlink_metadata(root).map_err(|_| "NATIVE_TEMP_CLEANUP_ROOT")?;
    if !metadata.is_dir() || reparse(&metadata) || root.canonicalize().ok().as_deref() != Some(root)
    {
        return Err("NATIVE_TEMP_CLEANUP_ROOT_CHANGED".into());
    }
    remove_entry(path, 0, &mut 0)
}

fn remove_entry(path: &Path, depth: usize, entries: &mut usize) -> Result<(), String> {
    if depth > 80 || *entries >= MAX_CLEANUP_ENTRIES {
        return Err("NATIVE_TEMP_CLEANUP_LIMIT".into());
    }
    *entries += 1;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("NATIVE_TEMP_CLEANUP_READ: {error}")),
    };
    if reparse(&metadata) {
        use std::os::windows::fs::MetadataExt;
        // RemoveDirectory removes a junction itself, never its target. Do not
        // enumerate or canonicalize this entry before unlinking it.
        return if metadata.file_attributes() & 0x10 != 0 {
            fs::remove_dir(path)
        } else {
            fs::remove_file(path)
        }
        .map_err(|error| format!("NATIVE_TEMP_CLEANUP_LINK: {error}"));
    }
    if metadata.is_dir() {
        for entry in
            fs::read_dir(path).map_err(|error| format!("NATIVE_TEMP_CLEANUP_LIST: {error}"))?
        {
            let entry = entry.map_err(|error| format!("NATIVE_TEMP_CLEANUP_LIST: {error}"))?;
            remove_entry(&entry.path(), depth + 1, entries)?;
        }
        fs::remove_dir(path).map_err(|error| format!("NATIVE_TEMP_CLEANUP_DIRECTORY: {error}"))
    } else if metadata.is_file() {
        fs::remove_file(path).map_err(|error| format!("NATIVE_TEMP_CLEANUP_FILE: {error}"))
    } else {
        Err("NATIVE_TEMP_CLEANUP_TYPE".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("ArkDock-native");
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let path = root.join("0123456789abcdef0123456789abcdef");
        fs::create_dir(&path).unwrap();
        (temporary, root, path)
    }
    #[test]
    fn cleanup_only_accepts_direct_nonce_children_of_its_fixed_root() {
        let (_temporary, root, path) = fixture();
        fs::create_dir_all(path.join("project/src/build")).unwrap();
        fs::write(path.join("project/src/build/source.ets"), "owned").unwrap();
        assert!(remove_owned_run(&root, &root).is_err());
        assert!(remove_owned_run(&root, &path.join("project")).is_err());
        assert!(remove_owned_run(root.parent().unwrap(), &root).is_err());
        assert!(path.exists());
        remove_owned_run(&root, &path).unwrap();
        assert!(!path.exists());
        assert!(root.exists());
    }
    #[test]
    fn cleanup_unlinks_junction_without_deleting_external_target() {
        use std::os::windows::process::CommandExt;
        let (temporary, root, path) = fixture();
        let external = temporary.path().join("external");
        fs::create_dir(&external).unwrap();
        fs::write(external.join("keep.txt"), "user data").unwrap();
        let link = path.join("installed-package");
        // Node is a repository test prerequisite. This creates only a fixture
        // junction; no shell, SDK tool, device or GUI is launched.
        let result = std::process::Command::new("node")
            .args([
                "-e",
                "require('node:fs').symlinkSync(process.argv[1], process.argv[2], 'junction')",
            ])
            .arg(&external)
            .arg(&link)
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(reparse(&fs::symlink_metadata(&link).unwrap()));
        remove_owned_run(&root, &path).unwrap();
        assert_eq!(
            fs::read_to_string(external.join("keep.txt")).unwrap(),
            "user data"
        );
    }
    #[test]
    fn releasing_a_run_eventually_reclaims_its_private_snapshot() {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).unwrap();
        let nonce = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let owned = OwnedRunDirectory::create(&nonce).unwrap();
        let path = owned.path().to_owned();
        fs::write(path.join("report.json"), "owned").unwrap();
        drop(owned);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!path.exists());
    }
}
