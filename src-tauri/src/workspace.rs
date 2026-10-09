//! Persistent choices contain no HWND/PID/session tokens. Discovery never grants execution trust.
mod journal;
use crate::ipc::model::{Environment, ProjectRef};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const SETTINGS_LIMIT: u64 = 128 * 1024;
const CONFIG_LIMIT: u64 = 1024 * 1024;
static TEMP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrustRecord {
    project_id: String,
    fingerprint: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings {
    version: u32,
    #[serde(default)]
    sdk_path: Option<PathBuf>,
    #[serde(default)]
    trusts: Vec<TrustRecord>,
    #[serde(default)]
    history: Vec<ProjectRef>,
}

pub struct WorkspaceStore {
    directory: PathBuf,
    settings: Settings,
}

impl WorkspaceStore {
    pub fn open(directory: &Path) -> Result<Self, String> {
        fs::create_dir_all(directory).map_err(|e| format!("SETTINGS_DIRECTORY: {e}"))?;
        let path = directory.join("settings.json");
        let settings = if path.exists() {
            let raw = read_bounded(&path, SETTINGS_LIMIT)?;
            let parsed: Settings = serde_json::from_slice(&raw)
                .map_err(|e| format!("SETTINGS_INVALID: {e}; original file preserved"))?;
            if parsed.version != 1 {
                return Err("SETTINGS_VERSION_UNSUPPORTED: original file preserved".into());
            }
            if parsed.history.len() > 64 || parsed.trusts.len() > 512 {
                return Err("SETTINGS_LIMIT_EXCEEDED".into());
            }
            parsed
        } else {
            Settings {
                version: 1,
                sdk_path: None,
                trusts: Vec::new(),
                history: Vec::new(),
            }
        };
        let settings = journal::load(directory)?.unwrap_or(settings);
        Ok(Self {
            directory: directory.to_path_buf(),
            settings,
        })
    }

    pub fn sdk_path(&self) -> Option<PathBuf> {
        self.settings.sdk_path.clone()
    }

    pub fn set_sdk_path(&mut self, path: PathBuf) -> Result<(), String> {
        let path = path
            .canonicalize()
            .map_err(|e| format!("SDK_PATH_INACCESSIBLE: {e}"))?;
        if !path.is_dir() {
            return Err("SDK_PATH_NOT_DIRECTORY".into());
        }
        let mut next = self.settings.clone();
        next.sdk_path = Some(path);
        self.persist(next)
    }

    pub fn is_trusted(&self, project: &ProjectRef) -> bool {
        fingerprint(project).ok().is_some_and(|fp| {
            self.settings
                .trusts
                .iter()
                .any(|record| record.project_id == project.id && record.fingerprint == fp)
        })
    }

    pub fn set_trust(&mut self, project: &ProjectRef, trusted: bool) -> Result<(), String> {
        let mut next = self.settings.clone();
        next.trusts.retain(|record| record.project_id != project.id);
        if trusted {
            if project.environment != Environment::WindowsLocal {
                return Err("UNSUPPORTED_ENVIRONMENT".into());
            }
            if next.trusts.len() >= 512 {
                return Err("TRUST_RECORD_LIMIT: revoke unused records first".into());
            }
            next.trusts.push(TrustRecord {
                project_id: project.id.clone(),
                fingerprint: fingerprint(project)?,
            });
        }
        self.persist(next)
    }

    pub fn remember(&mut self, project: &ProjectRef) -> Result<(), String> {
        let mut next = self.settings.clone();
        next.history.retain(|p| p.id != project.id);
        next.history.insert(0, project.clone());
        // This is a disclosed 64-item preference history, never a discovery candidate limit.
        next.history.truncate(64);
        self.persist(next)
    }

    pub fn history(&self) -> &[ProjectRef] {
        &self.settings.history
    }

    fn persist(&mut self, next: Settings) -> Result<(), String> {
        let path = self.directory.join("settings.json");
        if path.exists()
            && fs::symlink_metadata(&path)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
        {
            return Err("SETTINGS_SYMLINK_REJECTED".into());
        }
        let temporary = self.directory.join(format!(
            "settings-{}-{}.tmp",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let bytes = serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > SETTINGS_LIMIT {
            return Err("SETTINGS_LIMIT_EXCEEDED".into());
        }
        let result = (|| -> Result<(), String> {
            if self.directory.join("preferences").exists() {
                return journal::commit(&self.directory, &bytes);
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|e| e.to_string())?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|e| e.to_string())?;
            // Close the durable temporary file before publishing it.
            drop(file);
            if path.exists() {
                fs::copy(&path, self.directory.join("settings.json.bak"))
                    .map_err(|e| e.to_string())?;
            }
            if let Err(error) = fs::rename(&temporary, &path) {
                if cfg!(windows) && error.raw_os_error() == Some(17) {
                    journal::commit(&self.directory, &bytes)?;
                    let _ = fs::remove_file(&temporary);
                } else {
                    return Err(format!("SETTINGS_REPLACE: {error}"));
                }
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        self.settings = next;
        Ok(())
    }
}

pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    if fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("CONFIG_SYMLINK_REJECTED".into());
    }
    let file = File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > limit {
        return Err("CONFIG_TOO_LARGE".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("CONFIG_TOO_LARGE".into());
    }
    Ok(bytes)
}

/// Read bytes only: never evaluate ArkTS/JavaScript build configuration.
pub fn fingerprint(project: &ProjectRef) -> Result<String, String> {
    let root = Path::new(&project.app_root)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !root.is_dir() {
        return Err("PROJECT_INACCESSIBLE".into());
    }
    let mut hash = Sha256::new();
    hash.update(serde_json::to_vec(project).map_err(|e| e.to_string())?);
    let mut configurations: Vec<PathBuf> = [
        "build-profile.json5",
        "oh-package.json5",
        "oh-package-lock.json5",
        "hvigorfile.ts",
        "hvigor/hvigor-config.json5",
        "hvigor/hvigor-wrapper.js",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    let profile = root.join("build-profile.json5");
    if profile.exists() {
        let raw = read_bounded(&profile, CONFIG_LIMIT)?;
        let profile: serde_json::Value =
            json5::from_str(std::str::from_utf8(&raw).map_err(|e| e.to_string())?)
                .map_err(|e| format!("BUILD_PROFILE_INVALID: {e}"))?;
        if let Some(modules) = profile.get("modules").and_then(serde_json::Value::as_array) {
            if modules.len() > 128 {
                return Err("MODULE_LIMIT_EXCEEDED".into());
            }
            for module in modules {
                let source = module
                    .get("srcPath")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("MODULE_PATH_MISSING")?;
                let source = Path::new(source);
                if source.is_absolute()
                    || source.components().any(|c| {
                        matches!(
                            c,
                            std::path::Component::ParentDir | std::path::Component::Prefix(_)
                        )
                    })
                {
                    return Err("MODULE_OUTSIDE_PROJECT".into());
                }
                let module_root = root
                    .join(source)
                    .canonicalize()
                    .map_err(|e| e.to_string())?;
                if !module_root.starts_with(&root) {
                    return Err("MODULE_OUTSIDE_PROJECT".into());
                }
                for name in [
                    "hvigorfile.ts",
                    "oh-package.json5",
                    "oh-package-lock.json5",
                    "build-profile.json5",
                    "src/main/module.json5",
                    "hvigor/hvigor-config.json5",
                ] {
                    configurations.push(source.join(name));
                }
            }
        }
    }
    configurations.sort();
    configurations.dedup();
    for name in configurations {
        hash.update(name.to_string_lossy().as_bytes());
        let path = root.join(&name);
        if path.exists() {
            let canonical = path.canonicalize().map_err(|e| e.to_string())?;
            if !canonical.starts_with(&root) {
                return Err("CONFIG_OUTSIDE_PROJECT".into());
            }
            hash.update(read_bounded(&path, CONFIG_LIMIT)?);
        } else {
            hash.update(b"<absent>");
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}
