//! Automatically provisioned credentials; never included in client configuration.
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Credential {
    pub token: [u8; 32],
    pub pipe_name: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    token: String,
    pipe_name: String,
}

pub(crate) fn app_data() -> Result<PathBuf, String> {
    std::env::var_os("APPDATA")
        .map(|path| PathBuf::from(path).join("dev.arkdock.desktop"))
        .ok_or_else(|| "MCP_APPDATA_UNAVAILABLE".into())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn nonce() -> Result<String, String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| "MCP_RANDOM_UNAVAILABLE")?;
    Ok(hex(&bytes))
}

pub(crate) fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}

pub(crate) fn proof(credential: &Credential, challenge: &str) -> String {
    hex(&crate::context::hmac_sha256(
        &credential.token,
        format!("arkdock-mcp-v1-server:{challenge}").as_bytes(),
    ))
}

pub fn load_or_create(app_data: &Path) -> Result<Credential, String> {
    fs::create_dir_all(app_data).map_err(|_| "MCP_CREDENTIAL_DIRECTORY_FAILED")?;
    let directory = app_data.join("mcp-v1");
    fs::create_dir_all(&directory).map_err(|_| "MCP_CREDENTIAL_DIRECTORY_FAILED")?;
    reject_link(&directory)?;
    secure(&directory)?;
    let path = directory.join("credential.json");
    if path.exists() {
        return read(&path);
    }
    let mut token = [0; 32];
    getrandom::fill(&mut token).map_err(|_| "MCP_RANDOM_UNAVAILABLE")?;
    let stored = Stored {
        token: hex(&token),
        pipe_name: format!(r"\\.\pipe\arkdock-mcp-v1-{}", nonce()?),
    };
    let temporary = directory.join(format!("{}.tmp", nonce()?));
    let result = (|| {
        let mut file = private_file(&temporary)?;
        file.write_all(&serde_json::to_vec(&stored).map_err(|_| "MCP_CREDENTIAL_INVALID")?)
            .and_then(|()| file.sync_all())
            .map_err(|_| "MCP_CREDENTIAL_WRITE_FAILED")?;
        drop(file);
        // Atomic publication without replacement: concurrent desktop/client starts
        // must adopt the same complete credential, never overwrite each other.
        if let Err(error) = fs::hard_link(&temporary, &path) {
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err("MCP_CREDENTIAL_WRITE_FAILED".into());
            }
        }
        read(&path)
    })();
    let _ = fs::remove_file(temporary);
    result
}

fn reject_link(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "MCP_CREDENTIAL_UNAVAILABLE")?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("MCP_CREDENTIAL_PATH_INVALID".into());
        }
    }
    if metadata.file_type().is_symlink() {
        return Err("MCP_CREDENTIAL_PATH_INVALID".into());
    }
    Ok(())
}

fn read(path: &Path) -> Result<Credential, String> {
    reject_link(path)?;
    secure(path)?;
    let metadata = fs::metadata(path).map_err(|_| "MCP_CREDENTIAL_UNAVAILABLE")?;
    if !metadata.is_file() || metadata.len() > 1024 {
        return Err("MCP_CREDENTIAL_INVALID".into());
    }
    let stored: Stored =
        serde_json::from_slice(&fs::read(path).map_err(|_| "MCP_CREDENTIAL_UNAVAILABLE")?)
            .map_err(|_| "MCP_CREDENTIAL_INVALID")?;
    let suffix = stored
        .pipe_name
        .strip_prefix(r"\\.\pipe\arkdock-mcp-v1-")
        .ok_or("MCP_CREDENTIAL_INVALID")?;
    if stored.token.len() != 64
        || !stored.token.bytes().all(|b| b.is_ascii_hexdigit())
        || suffix.len() != 64
        || !suffix.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("MCP_CREDENTIAL_INVALID".into());
    }
    let mut token = [0; 32];
    for (index, byte) in token.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&stored.token[index * 2..index * 2 + 2], 16)
            .map_err(|_| "MCP_CREDENTIAL_INVALID")?;
    }
    Ok(Credential {
        token,
        pipe_name: stored.pipe_name,
    })
}

#[cfg(windows)]
pub(crate) fn private_attributes<T>(
    action: impl FnOnce(&windows_sys::Win32::Security::SECURITY_ATTRIBUTES) -> T,
) -> Result<T, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    let sddl: Vec<u16> = "D:P(A;OICI;GA;;;OW)"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        let mut descriptor = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err("MCP_PRIVATE_ACL_FAILED".into());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let result = action(&attributes);
        LocalFree(descriptor);
        Ok(result)
    }
}

#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(windows)]
fn secure(path: &Path) -> Result<(), String> {
    use windows_sys::Win32::Security::{
        SetFileSecurityW, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    };
    if private_attributes(|attributes| unsafe {
        SetFileSecurityW(
            wide(path).as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            attributes.lpSecurityDescriptor,
        )
    })? == 0
    {
        return Err("MCP_PRIVATE_ACL_FAILED".into());
    }
    Ok(())
}

#[cfg(windows)]
fn private_file(path: &Path) -> Result<File, String> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL};
    let handle = private_attributes(|attributes| unsafe {
        CreateFileW(
            wide(path).as_ptr(),
            GENERIC_WRITE,
            0,
            attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    })?;
    if handle == INVALID_HANDLE_VALUE {
        return Err("MCP_CREDENTIAL_WRITE_FAILED".into());
    }
    Ok(unsafe { File::from_raw_handle(handle) })
}

#[cfg(not(windows))]
fn private_file(path: &Path) -> Result<File, String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|_| "MCP_CREDENTIAL_WRITE_FAILED".into())
}

#[cfg(not(windows))]
fn secure(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if path.is_dir() { 0o700 } else { 0o600 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|_| "MCP_PRIVATE_ACL_FAILED")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_provisioning_publishes_one_complete_credential() {
        let directory = tempfile::tempdir().unwrap();
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let path = directory.path().to_owned();
                std::thread::spawn(move || load_or_create(&path).unwrap())
            })
            .collect();
        let credentials: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert!(credentials
            .iter()
            .all(|c| c.token == credentials[0].token && c.pipe_name == credentials[0].pipe_name));
        assert_eq!(
            fs::read_dir(directory.path().join("mcp-v1"))
                .unwrap()
                .count(),
            1
        );
    }
    #[test]
    fn invalid_credential_is_not_replaced_or_disclosed() {
        let directory = tempfile::tempdir().unwrap();
        load_or_create(directory.path()).unwrap();
        let path = directory.path().join("mcp-v1/credential.json");
        fs::write(&path, b"private-invalid-token").unwrap();
        assert_eq!(
            load_or_create(directory.path()).err().unwrap(),
            "MCP_CREDENTIAL_INVALID"
        );
        assert_eq!(fs::read(path).unwrap(), b"private-invalid-token");
    }

    #[cfg(windows)]
    #[test]
    fn credential_directory_and_file_have_only_protected_owner_rights() {
        use windows_sys::Win32::Security::{
            GetFileSecurityW, GetSecurityDescriptorDacl, DACL_SECURITY_INFORMATION,
        };
        let directory = tempfile::tempdir().unwrap();
        load_or_create(directory.path()).unwrap();
        for path in [
            directory.path().join("mcp-v1"),
            directory.path().join("mcp-v1/credential.json"),
        ] {
            let mut needed = 0;
            unsafe {
                GetFileSecurityW(
                    wide(&path).as_ptr(),
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    0,
                    &mut needed,
                );
            }
            assert!(needed > 0);
            let mut descriptor = vec![0u8; needed as usize];
            assert_ne!(
                unsafe {
                    GetFileSecurityW(
                        wide(&path).as_ptr(),
                        DACL_SECURITY_INFORMATION,
                        descriptor.as_mut_ptr().cast(),
                        needed,
                        &mut needed,
                    )
                },
                0
            );
            assert_ne!(
                descriptor[3] & 0x10,
                0,
                "DACL is protected from inherited grants"
            );
            let mut present = 0;
            let mut defaulted = 0;
            let mut acl = std::ptr::null_mut();
            assert_ne!(
                unsafe {
                    GetSecurityDescriptorDacl(
                        descriptor.as_mut_ptr().cast(),
                        &mut present,
                        &mut acl,
                        &mut defaulted,
                    )
                },
                0
            );
            assert_ne!(present, 0);
            assert!(!acl.is_null());
            assert_eq!(unsafe { (*acl).AceCount }, 1, "one owner rights grant only");
        }
    }
}
