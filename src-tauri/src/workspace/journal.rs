//! Fallback for filesystems that cannot rename preference files (observed Windows error 17).
//! Immutable payloads are synced before their commit markers; no copy/delete replacement.
use super::{read_bounded, Settings, SETTINGS_LIMIT};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

fn records(directory: &Path) -> Result<Vec<(u64, String)>, String> {
    let journal = directory.join("preferences");
    if !journal.exists() {
        return Ok(Vec::new());
    }
    if fs::symlink_metadata(&journal)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("SETTINGS_SYMLINK_REJECTED".into());
    }
    let mut records = Vec::new();
    for (index, entry) in fs::read_dir(&journal)
        .map_err(|e| e.to_string())?
        .enumerate()
    {
        if index >= 128 {
            return Err("SETTINGS_JOURNAL_LIMIT".into());
        }
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".ready") else {
            continue;
        };
        let Some((ordinal, digest)) = stem.split_once('-') else {
            return Err("SETTINGS_COMMIT_INVALID".into());
        };
        if ordinal.len() != 16
            || digest.len() != 64
            || !digest.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("SETTINGS_COMMIT_INVALID".into());
        }
        let ordinal = u64::from_str_radix(ordinal, 16).map_err(|_| "SETTINGS_COMMIT_INVALID")?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file()
            || entry.metadata().map_err(|e| e.to_string())?.len() != 0
        {
            return Err("SETTINGS_COMMIT_INVALID".into());
        }
        records.push((ordinal, stem.to_owned()));
    }
    records.sort();
    if records.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err("SETTINGS_CONCURRENT_COMMIT".into());
    }
    Ok(records)
}

pub(super) fn load(directory: &Path) -> Result<Option<Settings>, String> {
    let records = records(directory)?;
    let Some((_, stem)) = records.last() else {
        return Ok(None);
    };
    let bytes = read_bounded(
        &directory.join("preferences").join(format!("{stem}.json")),
        SETTINGS_LIMIT,
    )?;
    if format!("{:x}", Sha256::digest(&bytes)) != stem[17..] {
        return Err("SETTINGS_CHECKSUM_INVALID: committed record preserved".into());
    }
    let settings: Settings = serde_json::from_slice(&bytes)
        .map_err(|_| "SETTINGS_INVALID: committed record preserved")?;
    if settings.version != 1 {
        return Err("SETTINGS_VERSION_UNSUPPORTED: committed record preserved".into());
    }
    if settings.history.len() > 64 || settings.trusts.len() > 512 {
        return Err("SETTINGS_LIMIT_EXCEEDED".into());
    }
    Ok(Some(settings))
}

pub(super) fn commit(directory: &Path, bytes: &[u8]) -> Result<(), String> {
    let journal = directory.join("preferences");
    let records = records(directory)?;
    fs::create_dir_all(&journal).map_err(|e| e.to_string())?;
    let mut previous = records.last().map_or(0, |(ordinal, _)| *ordinal);
    // A crash between payload sync and marker creation leaves an immutable orphan.
    // Skip its ordinal; never overwrite it or let an identical retry collide forever.
    for (index, entry) in fs::read_dir(&journal)
        .map_err(|e| e.to_string())?
        .enumerate()
    {
        if index >= 128 {
            return Err("SETTINGS_JOURNAL_LIMIT".into());
        }
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".json") else {
            continue;
        };
        let Some((ordinal, digest)) = stem.split_once('-') else {
            continue;
        };
        if ordinal.len() == 16
            && digest.len() == 64
            && digest.bytes().all(|b| b.is_ascii_hexdigit())
        {
            if let Ok(value) = u64::from_str_radix(ordinal, 16) {
                previous = previous.max(value);
            }
        }
    }
    let ordinal = previous
        .checked_add(1)
        .ok_or("SETTINGS_SEQUENCE_EXHAUSTED")?;
    let stem = format!("{ordinal:016x}-{:x}", Sha256::digest(bytes));
    let payload = journal.join(format!("{stem}.json"));
    let marker = journal.join(format!("{stem}.ready"));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&payload)
        .map_err(|e| e.to_string())?;
    let result = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(&payload);
        return Err(error.to_string());
    }
    let ready = match OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&marker)
    {
        Ok(file) => file,
        Err(error) => {
            let _ = fs::remove_file(&payload);
            return Err(error.to_string());
        }
    };
    let result = ready.sync_all();
    drop(ready);
    if let Err(error) = result {
        let _ = fs::remove_file(&marker);
        let _ = fs::remove_file(&payload);
        return Err(error.to_string());
    }
    // Preserve current/previous complete snapshots and the legacy settings.json untouched.
    for (_, old) in records.iter().rev().skip(1) {
        let _ = fs::remove_file(journal.join(format!("{old}.ready")));
        let _ = fs::remove_file(journal.join(format!("{old}.json")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_update_after_payload_only_crash_can_commit_without_overwriting_orphan() {
        let directory = tempfile::tempdir().unwrap();
        let bytes = br#"{"version":1,"sdkPath":null,"history":[],"trusts":[]}"#;
        let journal = directory.path().join("preferences");
        fs::create_dir(&journal).unwrap();
        let orphan = journal.join(format!("0000000000000001-{:x}.json", Sha256::digest(bytes)));
        fs::write(&orphan, bytes).unwrap();
        assert!(load(directory.path()).unwrap().is_none());
        commit(directory.path(), bytes).unwrap();
        assert!(load(directory.path()).unwrap().is_some());
        assert_eq!(fs::read(&orphan).unwrap(), bytes);
    }
}
