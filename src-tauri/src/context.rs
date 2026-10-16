//! Authenticated context is a source declaration, never proof of a visible conversation.
//! Registrations are connection-owned leases with replay checks and stable backend IDs.
use crate::ipc::model::{Environment, ProcessRef};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
pub const CONTEXT_PIPE_NAME: &str = "\\\\.\\pipe\\arkdock-context";
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;
pub const MAX_CONNECTIONS: usize = 8;
pub const MAX_SESSIONS: usize = 32;
pub const MAX_ROOTS: usize = 8;
const MAX_PATH_LEN: usize = 4096;
const MAX_REPLAY_RECORDS: usize = 512;
pub const HEARTBEAT_TTL_MS: u64 = 15_000;
pub const CONNECTION_IDLE_TIMEOUT_MS: u64 = 20_000;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ContextMessage {
    Hello {
        token: String,
        session: String,
        sequence: u64,
        #[serde(default)]
        roots: Vec<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        agent: Option<String>,
        #[serde(default)]
        host_session: Option<String>,
        #[serde(default)]
        original_project: Option<String>,
        #[serde(default)]
        execution_directory: Option<String>,
        #[serde(default)]
        environment: Option<Environment>,
    },
    Heartbeat {
        token: String,
        session: String,
        sequence: u64,
    },
    Update {
        token: String,
        session: String,
        sequence: u64,
        roots: Vec<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        agent: Option<String>,
        #[serde(default)]
        host_session: Option<String>,
        #[serde(default)]
        original_project: Option<String>,
        #[serde(default)]
        execution_directory: Option<String>,
        #[serde(default)]
        environment: Option<Environment>,
    },
    Revoke {
        token: String,
        session: String,
        sequence: u64,
    },
}
impl ContextMessage {
    pub fn session(&self) -> &str {
        match self {
            Self::Hello { session, .. }
            | Self::Update { session, .. }
            | Self::Heartbeat { session, .. }
            | Self::Revoke { session, .. } => session,
        }
    }
    pub fn sequence(&self) -> u64 {
        match self {
            Self::Hello { sequence, .. }
            | Self::Update { sequence, .. }
            | Self::Heartbeat { sequence, .. }
            | Self::Revoke { sequence, .. } => *sequence,
        }
    }
    fn token(&self) -> &str {
        match self {
            Self::Hello { token, .. }
            | Self::Update { token, .. }
            | Self::Heartbeat { token, .. }
            | Self::Revoke { token, .. } => token,
        }
    }
}
/// Errors deliberately omit input, including any pairing credential.
pub fn parse_message(line: &str) -> Result<ContextMessage, String> {
    if line.len() > MAX_MESSAGE_BYTES {
        return Err("CONTEXT_FRAME_TOO_LARGE".into());
    }
    serde_json::from_str(line).map_err(|_| "CONTEXT_FRAME_INVALID".into())
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextEntry {
    pub session: String,
    pub client: ProcessRef,
    pub connection_id: u64,
    pub sequence: u64,
    /// Changes on reconnect and material update; heartbeat does not invalidate a binding.
    pub registration_id: String,
    pub roots: Vec<String>,
    pub cwd: Option<String>,
    pub agent: Option<String>,
    pub host_session: Option<String>,
    pub original_project: Option<String>,
    pub execution_directory: Option<String>,
    pub environment: Option<Environment>,
    pub observed_at: u64,
    pub expires_at: u64,
}
impl ContextEntry {
    pub fn declared_roots(&self) -> &[String] {
        &self.roots
    }
    pub fn has_root_declaration(&self) -> bool {
        !self.roots.is_empty()
    }
    pub fn effective_execution_directory(&self) -> Option<&str> {
        self.execution_directory
            .as_deref()
            .or(match self.roots.as_slice() {
                [root] => Some(root.as_str()),
                [] => self.cwd.as_deref(),
                _ => None,
            })
    }
    pub fn execution_environment(&self) -> Environment {
        self.environment.clone().unwrap_or_else(|| {
            self.effective_execution_directory()
                .map(inferred_environment)
                .unwrap_or(Environment::Remote)
        })
    }
    pub fn environment_for_path(&self, path: &str) -> Environment {
        self.environment
            .clone()
            .unwrap_or_else(|| inferred_environment(path))
    }
    fn same_context(&self, other: &Self) -> bool {
        self.host_session == other.host_session
            && self.original_project == other.original_project
            && self.execution_directory == other.execution_directory
            && self.environment == other.environment
            // Once explicitly declared, execution context is independent of roots/cwd
            // compatibility hints, which may change as other host conversations run.
            && (self.execution_directory.is_some()
                || (self.roots == other.roots && self.cwd == other.cwd))
    }
}
#[derive(Clone)]
struct ReplayRecord {
    client: ProcessRef,
    sequence: u64,
}
#[derive(Default)]
struct HubInner {
    token: Option<String>,
    entries: HashMap<String, ContextEntry>,
    // Retain after expiry/disconnect/revoke. Reconnect reuses session with higher sequence.
    // Fail closed when full; pairing rotation resets these bounded replay watermarks.
    replay: HashMap<String, ReplayRecord>,
    generation: u64,
    last_event: Option<(String, u64)>,
}
#[derive(Default)]
pub struct ContextHub {
    inner: std::sync::Mutex<HubInner>,
}
impl ContextHub {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn activate_token(&self, token: [u8; 32]) {
        let mut inner = self.lock();
        inner.token = Some(token.iter().map(|byte| format!("{byte:02x}")).collect());
        inner.entries.clear();
        inner.replay.clear();
        inner.note("Context source pairing activated".into());
    }
    pub fn revoke_token(&self) -> bool {
        let mut inner = self.lock();
        let had = inner.token.take().is_some();
        inner.entries.clear();
        inner.replay.clear();
        inner.note("Context source pairing revoked".into());
        had
    }
    pub fn paired(&self) -> bool {
        self.lock().token.is_some()
    }
    /// Proves possession of the pairing secret before a client discloses its token
    /// or project paths. A probe grants no registration and never renews a lease.
    pub fn server_proof(&self, challenge: &str) -> Result<String, String> {
        if challenge.len() != 64
            || !challenge
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("CONTEXT_CHALLENGE_INVALID".into());
        }
        let inner = self.lock();
        let token = inner.token.as_ref().ok_or("CONTEXT_PAIRING_MISSING")?;
        let mut key = [0u8; 32];
        for (index, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&token[index * 2..index * 2 + 2], 16)
                .map_err(|_| "CONTEXT_PAIRING_INVALID")?;
        }
        let mut message = b"arkdock-context-server-v1\0".to_vec();
        message.extend_from_slice(challenge.as_bytes());
        let proof = hmac_sha256(&key, &message);
        Ok(proof.iter().map(|byte| format!("{byte:02x}")).collect())
    }
    /// Client and connection identity come exclusively from the transport.
    pub fn apply(
        &self,
        message: ContextMessage,
        client: ProcessRef,
        connection_id: u64,
        now: u64,
    ) -> Result<(), String> {
        let mut inner = self.lock();
        let token = inner.token.as_ref().ok_or("CONTEXT_PAIRING_MISSING")?;
        if !constant_time_eq(message.token(), token) {
            return Err("CONTEXT_TOKEN_INVALID".into());
        }
        if connection_id == 0 || client.pid == 0 || client.started_at.is_empty() {
            return Err("CONTEXT_CLIENT_INVALID".into());
        }
        let session = message.session().to_owned();
        valid_session(&session)?;
        let sequence = message.sequence();
        if sequence == 0 {
            return Err("CONTEXT_SEQUENCE_INVALID".into());
        }
        // Expiry is enforced even when no discovery reader has visited live().
        inner
            .entries
            .retain(|_, entry| entry.expires_at > now && entry.observed_at <= now);
        if let Some(previous) = inner.replay.get(&session) {
            if previous.client != client {
                return Err("CONTEXT_SESSION_OWNER_MISMATCH".into());
            }
            if sequence <= previous.sequence {
                return Err("CONTEXT_SEQUENCE_REPLAY".into());
            }
        } else if inner.replay.len() >= MAX_REPLAY_RECORDS {
            return Err("CONTEXT_REPLAY_LIMIT".into());
        }
        let hello = matches!(&message, ContextMessage::Hello { .. });
        if hello {
            if inner.entries.contains_key(&session) {
                return Err("CONTEXT_SESSION_ALREADY_REGISTERED".into());
            }
            if inner.entries.len() >= MAX_SESSIONS {
                return Err("CONTEXT_SESSION_LIMIT".into());
            }
        } else {
            let existing = inner
                .entries
                .get(&session)
                .ok_or("CONTEXT_SESSION_UNKNOWN")?;
            if existing.connection_id != connection_id || existing.client != client {
                return Err("CONTEXT_SESSION_OWNER_MISMATCH".into());
            }
        }
        let expires_at = now
            .checked_add(HEARTBEAT_TTL_MS)
            .ok_or("CONTEXT_TIME_OVERFLOW")?;
        match message {
            ContextMessage::Hello {
                roots,
                cwd,
                agent,
                host_session,
                original_project,
                execution_directory,
                environment,
                ..
            }
            | ContextMessage::Update {
                roots,
                cwd,
                agent,
                host_session,
                original_project,
                execution_directory,
                environment,
                ..
            } => {
                let mut entry = ContextEntry {
                    session: session.clone(),
                    client: client.clone(),
                    connection_id,
                    sequence,
                    registration_id: String::new(),
                    roots,
                    cwd,
                    agent,
                    host_session,
                    original_project,
                    execution_directory,
                    environment,
                    observed_at: now,
                    expires_at,
                };
                validate_entry(&mut entry)?;
                if let Some(previous) = inner
                    .entries
                    .get(&session)
                    .filter(|previous| previous.same_context(&entry))
                {
                    entry.registration_id = previous.registration_id.clone();
                } else {
                    inner.generation = inner
                        .generation
                        .checked_add(1)
                        .ok_or("CONTEXT_GENERATION_OVERFLOW")?;
                    entry.registration_id = format!("{connection_id}-{}", inner.generation);
                }
                inner.entries.insert(session.clone(), entry);
                inner.note(format!(
                    "Context session {}: {session}",
                    if hello { "registered" } else { "updated" }
                ));
            }
            ContextMessage::Heartbeat { .. } => {
                let existing = inner
                    .entries
                    .get_mut(&session)
                    .ok_or("CONTEXT_SESSION_UNKNOWN")?;
                existing.sequence = sequence;
                existing.observed_at = now;
                existing.expires_at = expires_at;
            }
            ContextMessage::Revoke { .. } => {
                inner.entries.remove(&session);
                inner.note(format!("Context session revoked: {session}"));
            }
        }
        inner
            .replay
            .insert(session, ReplayRecord { client, sequence });
        Ok(())
    }
    pub fn remove_connection(&self, connection_id: u64) -> usize {
        let mut inner = self.lock();
        let before = inner.entries.len();
        inner
            .entries
            .retain(|_, entry| entry.connection_id != connection_id);
        let removed = before - inner.entries.len();
        if removed > 0 {
            inner.note(format!(
                "Context connection disconnected; {removed} session(s) revoked"
            ));
        }
        removed
    }
    pub fn live(&self, now: u64) -> Vec<ContextEntry> {
        let mut inner = self.lock();
        inner
            .entries
            .retain(|_, entry| entry.observed_at <= now && entry.expires_at > now);
        let mut entries: Vec<_> = inner.entries.values().cloned().collect();
        entries.sort_by(|a, b| a.session.cmp(&b.session));
        entries
    }
    pub fn last_event(&self) -> Option<(String, u64)> {
        self.lock().last_event.clone()
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, HubInner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}
impl HubInner {
    fn note(&mut self, reason: String) {
        self.last_event = Some((reason, now_millis()));
    }
}
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn new_pairing() -> Result<(String, [u8; 32]), String> {
    #[cfg(windows)]
    {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| "CONTEXT_RANDOM_UNAVAILABLE".to_owned())?;
        let hex = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok((hex, bytes))
    }
    #[cfg(not(windows))]
    {
        Err("CONTEXT_RANDOM_UNAVAILABLE".into())
    }
}
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for index in 0..a.len().max(b.len()) {
        diff |= (a.get(index).copied().unwrap_or(0) ^ b.get(index).copied().unwrap_or(0)) as usize;
    }
    diff == 0
}
/// HMAC-SHA256 for our fixed 32-byte secret (shorter than SHA-256's 64-byte block).
fn hmac_sha256(key: &[u8; 32], message: &[u8]) -> [u8; 32] {
    let mut inner_pad = [0x36u8; 64];
    let mut outer_pad = [0x5cu8; 64];
    for index in 0..key.len() {
        inner_pad[index] ^= key[index];
        outer_pad[index] ^= key[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    outer.finalize().into()
}
fn valid_session(session: &str) -> Result<(), String> {
    if (16..=128).contains(&session.len())
        && session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Ok(())
    } else {
        Err("CONTEXT_SESSION_INVALID".into())
    }
}
fn valid_label(value: &Option<String>, limit: usize, field: &str) -> Result<(), String> {
    if value
        .as_ref()
        .is_some_and(|v| v.is_empty() || v.len() > limit || v.chars().any(char::is_control))
    {
        Err(format!("CONTEXT_{field}_INVALID"))
    } else {
        Ok(())
    }
}
fn inferred_environment(path: &str) -> Environment {
    if path.starts_with("vscode-remote://wsl+") {
        Environment::Wsl
    } else if path.starts_with('/') && !path.starts_with("//") {
        Environment::Remote
    } else {
        crate::discovery::path_environment(std::path::Path::new(path))
    }
}
fn validate_path(path: &str, environment: Option<&Environment>) -> Result<(), String> {
    if path.is_empty() || path.len() > MAX_PATH_LEN || path.chars().any(char::is_control) {
        return Err("CONTEXT_PATH_INVALID".into());
    }
    let detected = inferred_environment(path);
    let environment = environment.unwrap_or(&detected);
    let bytes = path.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');
    let posix = path.starts_with('/') && !path.starts_with("//");
    let unc = path.starts_with("\\\\") || path.starts_with("//");
    let absolute = match environment {
        Environment::WindowsLocal => (drive || unc) && detected == Environment::WindowsLocal,
        Environment::Wsl => {
            posix
                || (detected == Environment::Wsl
                    && (unc
                        || path.starts_with("wsl://")
                        || path.starts_with("vscode-remote://wsl+")))
        }
        Environment::Remote => {
            posix
                || path.starts_with("ssh://")
                || (path.starts_with("vscode-remote://") && detected == Environment::Remote)
        }
    };
    if absolute {
        Ok(())
    } else {
        Err("CONTEXT_PATH_ENVIRONMENT_INVALID".into())
    }
}
fn validate_entry(entry: &mut ContextEntry) -> Result<(), String> {
    if entry.roots.len() > MAX_ROOTS {
        return Err("CONTEXT_ROOTS_LIMIT".into());
    }
    let mut roots = Vec::new();
    for root in &entry.roots {
        let environment = if entry.execution_directory.is_some() {
            None
        } else {
            entry.environment.as_ref()
        };
        validate_path(root, environment)?;
        if !roots.contains(root) {
            roots.push(root.clone());
        }
    }
    entry.roots = roots;
    if let Some(path) = &entry.original_project {
        // Display provenance can be local even when execution happens remotely.
        validate_path(path, None)?;
    }
    if let Some(path) = &entry.execution_directory {
        validate_path(path, entry.environment.as_ref())?;
    }
    if let Some(cwd) = &entry.cwd {
        // A local MCP executable may report an explicitly remote session.
        let environment = if entry.execution_directory.is_none() && entry.roots.is_empty() {
            entry.environment.as_ref()
        } else {
            None
        };
        validate_path(cwd, environment)?;
    }
    valid_label(&entry.agent, 64, "AGENT")?;
    valid_label(&entry.host_session, 128, "HOST_SESSION")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn client(pid: u32) -> ProcessRef {
        ProcessRef {
            pid,
            started_at: "1000".into(),
        }
    }
    fn hub() -> ContextHub {
        let hub = ContextHub::new();
        hub.activate_token([0xaa; 32]);
        hub
    }
    fn frame(kind: &str, session: &str, sequence: u64) -> ContextMessage {
        parse_message(&serde_json::json!({"kind":kind,"token":"aa".repeat(32),"session":session,"sequence":sequence,"roots":["C:\\app"]}).to_string()).unwrap()
    }
    fn hello(sequence: u64) -> ContextMessage {
        frame("hello", "session-12345678", sequence)
    }
    fn renewal(kind: &str, sequence: u64) -> ContextMessage {
        parse_message(&serde_json::json!({"kind":kind,"token":"aa".repeat(32),"session":"session-12345678","sequence":sequence}).to_string()).unwrap()
    }
    #[test]
    fn server_proof_matches_independent_hmac_vector_and_is_challenge_bound() {
        let hub = ContextHub::new();
        let key = std::array::from_fn(|index| index as u8);
        hub.activate_token(key);
        let challenge = "0123456789abcdef".repeat(4);
        // Independently calculated with Node's crypto.createHmac('sha256', key).
        let expected = "8499477b5f92d46ff9fd51c48a5a304410a8c243efdbf7ce2ec41ac34e1b9eca";
        assert_eq!(hub.server_proof(&challenge).unwrap(), expected);
        assert_ne!(hub.server_proof(&"0".repeat(64)).unwrap(), expected);
        assert!(hub.live(1000).is_empty());
        hub.activate_token([0xbb; 32]);
        assert_ne!(hub.server_proof(&challenge).unwrap(), expected);
    }
    #[test]
    fn server_proof_rejects_missing_pairing_and_noncanonical_challenges() {
        let hub = ContextHub::new();
        assert_eq!(
            hub.server_proof(&"0".repeat(64)),
            Err("CONTEXT_PAIRING_MISSING".into())
        );
        hub.activate_token([0xaa; 32]);
        for challenge in [
            "secret".into(),
            "0".repeat(63),
            "0".repeat(65),
            "A".repeat(64),
            "g".repeat(64),
        ] {
            assert_eq!(
                hub.server_proof(&challenge),
                Err("CONTEXT_CHALLENGE_INVALID".into())
            );
        }
        hub.apply(hello(1), client(7), 1, 1000).unwrap();
        let entry = hub.live(1000).remove(0);
        hub.server_proof(&"0".repeat(64)).unwrap();
        assert_eq!(hub.live(1000)[0], entry, "proof never changes a live lease");
        hub.revoke_token();
        assert_eq!(
            hub.server_proof(&"0".repeat(64)),
            Err("CONTEXT_PAIRING_MISSING".into())
        );
    }
    #[test]
    fn wire_rejects_missing_sequence_unknown_fields_and_oversize_without_echoing_secrets() {
        assert!(
            parse_message(r#"{"kind":"hello","token":"secret","session":"session-12345678"}"#)
                .is_err()
        );
        assert!(parse_message(r#"{"kind":"hello","token":"secret","session":"session-12345678","sequence":1,"trusted":true}"#).is_err());
        assert!(parse_message(&" ".repeat(MAX_MESSAGE_BYTES + 1)).is_err());
        assert!(!parse_message(r#"{"token":"secret","kind":"secret"}"#)
            .unwrap_err()
            .contains("secret"));
        assert!(parse_message(r#"{"kind":"hello","token":"t","session":"session-12345678","sequence":1,"hostSession":null,"environment":"wsl"}"#).is_ok());
    }
    #[test]
    fn same_process_connections_cannot_modify_or_disconnect_each_others_registration() {
        let hub = hub();
        hub.apply(hello(1), client(7), 1, 1000).unwrap();
        assert_eq!(
            hub.apply(renewal("heartbeat", 2), client(7), 2, 2000),
            Err("CONTEXT_SESSION_OWNER_MISMATCH".into())
        );
        assert_eq!(hub.remove_connection(2), 0);
        assert_eq!(hub.live(2000).len(), 1);
        assert_eq!(hub.remove_connection(1), 1);
    }
    #[test]
    fn monotonic_sequences_reject_replays_live_and_after_disconnect_or_revoke() {
        let hub = hub();
        hub.apply(hello(1), client(7), 1, 1000).unwrap();
        hub.apply(renewal("heartbeat", 2), client(7), 1, 2000)
            .unwrap();
        let id = hub.live(2000)[0].registration_id.clone();
        assert_eq!(
            hub.apply(renewal("heartbeat", 2), client(7), 1, 3000),
            Err("CONTEXT_SEQUENCE_REPLAY".into())
        );
        assert_eq!(
            hub.apply(hello(3), client(7), 2, 3000),
            Err("CONTEXT_SESSION_ALREADY_REGISTERED".into())
        );
        hub.remove_connection(1);
        assert_eq!(
            hub.apply(hello(1), client(7), 2, 3000),
            Err("CONTEXT_SEQUENCE_REPLAY".into())
        );
        hub.apply(hello(3), client(7), 2, 3000).unwrap();
        assert_ne!(hub.live(3000)[0].registration_id, id);
        assert_eq!(
            hub.apply(renewal("revoke", 4), client(7), 1, 3001),
            Err("CONTEXT_SESSION_OWNER_MISMATCH".into())
        );
        hub.apply(renewal("revoke", 4), client(7), 2, 3002).unwrap();
        assert!(hub.live(3002).is_empty());
        assert_eq!(
            hub.apply(hello(3), client(7), 3, 3003),
            Err("CONTEXT_SEQUENCE_REPLAY".into())
        );
    }
    #[test]
    fn expired_lease_cannot_be_renewed_without_a_discovery_read() {
        let hub = hub();
        hub.apply(hello(1), client(7), 1, 1000).unwrap();
        assert_eq!(
            hub.apply(
                renewal("heartbeat", 2),
                client(7),
                1,
                1000 + HEARTBEAT_TTL_MS
            ),
            Err("CONTEXT_SESSION_UNKNOWN".into())
        );
        assert!(hub.live(1000 + HEARTBEAT_TTL_MS).is_empty());
        assert_eq!(
            hub.apply(hello(1), client(7), 2, 20_000),
            Err("CONTEXT_SEQUENCE_REPLAY".into())
        );
    }
    #[test]
    fn material_updates_change_identity_but_renewal_and_identical_update_do_not() {
        let hub = hub();
        hub.apply(hello(1), client(7), 1, 1000).unwrap();
        let original = hub.live(1000)[0].registration_id.clone();
        hub.apply(renewal("heartbeat", 2), client(7), 1, 2000)
            .unwrap();
        hub.apply(frame("update", "session-12345678", 3), client(7), 1, 3000)
            .unwrap();
        assert_eq!(hub.live(3000)[0].registration_id, original);
        let update=parse_message(&serde_json::json!({"kind":"update","token":"aa".repeat(32),"session":"session-12345678","sequence":4,"roots":["C:\\original"],"hostSession":"host-background","originalProject":"C:\\original","executionDirectory":"C:\\worktree","environment":"windowsLocal"}).to_string()).unwrap();
        hub.apply(update, client(7), 1, 4000).unwrap();
        let entry = hub.live(4000).remove(0);
        assert_ne!(entry.registration_id, original);
        assert_eq!(entry.effective_execution_directory(), Some("C:\\worktree"));
        assert_eq!(entry.original_project.as_deref(), Some("C:\\original"));
        assert_eq!(entry.host_session.as_deref(), Some("host-background"));
        hub.apply(frame("update", "session-12345678", 5), client(7), 1, 5000)
            .unwrap();
        assert!(hub.live(5000)[0].execution_directory.is_none());
    }
    #[test]
    fn token_rotation_and_revocation_remove_all_live_context() {
        let hub = hub();
        hub.apply(hello(1), client(7), 1, 1000).unwrap();
        hub.activate_token([0xbb; 32]);
        assert!(hub.live(1000).is_empty());
        assert_eq!(
            hub.apply(hello(2), client(7), 2, 2000),
            Err("CONTEXT_TOKEN_INVALID".into())
        );
        assert!(hub.revoke_token());
        assert!(!hub.paired());
        assert_eq!(
            hub.apply(hello(2), client(7), 2, 2000),
            Err("CONTEXT_PAIRING_MISSING".into())
        );
    }
    #[test]
    fn explicit_execution_identity_survives_unrelated_host_hints() {
        let hub = hub();
        let mut value = serde_json::json!({"kind":"hello","token":"aa".repeat(32),"session":"session-12345678","sequence":1,"roots":["C:\\host"],"cwd":"C:\\tools","agent":"host","hostSession":"visible-session","originalProject":"C:\\original","executionDirectory":"/home/worktree","environment":"wsl"});
        hub.apply(
            parse_message(&value.to_string()).unwrap(),
            client(7),
            1,
            1000,
        )
        .unwrap();
        let original = hub.live(1000).remove(0);
        value["kind"] = "update".into();
        value["sequence"] = 2.into();
        value["roots"] = serde_json::json!(["ssh://unrelated/background"]);
        value["cwd"] = "C:\\new-tools".into();
        value["agent"] = "new-label".into();
        hub.apply(
            parse_message(&value.to_string()).unwrap(),
            client(7),
            1,
            2000,
        )
        .unwrap();
        let updated = hub.live(2000).remove(0);
        assert_eq!(updated.registration_id, original.registration_id);
        assert_eq!(updated.original_project.as_deref(), Some("C:\\original"));
        assert_eq!(updated.execution_environment(), Environment::Wsl);
        value["sequence"] = 3.into();
        value["executionDirectory"] = "/home/other-worktree".into();
        hub.apply(
            parse_message(&value.to_string()).unwrap(),
            client(7),
            1,
            3000,
        )
        .unwrap();
        assert_ne!(hub.live(3000)[0].registration_id, original.registration_id);
    }
    #[test]
    fn ambiguous_roots_never_fall_back_to_cwd() {
        let hub = hub();
        let message = parse_message(&serde_json::json!({"kind":"hello","token":"aa".repeat(32),"session":"session-12345678","sequence":1,"roots":["C:\\one","C:\\two"],"cwd":"C:\\arbitrary"}).to_string()).unwrap();
        hub.apply(message, client(7), 1, 1000).unwrap();
        assert_eq!(hub.live(1000)[0].effective_execution_directory(), None);
    }
    #[test]
    fn environment_identity_never_maps_nonlocal_paths_to_windows() {
        assert!(validate_path("/home/app", Some(&Environment::WindowsLocal)).is_err());
        assert!(validate_path("relative/app", None).is_err());
        assert!(validate_path("C:\\app", Some(&Environment::Remote)).is_err());
        assert!(validate_path(
            "\\\\wsl.localhost\\Ubuntu\\app",
            Some(&Environment::WindowsLocal)
        )
        .is_err());
        assert!(validate_path("ssh://host/app", Some(&Environment::WindowsLocal)).is_err());
        assert!(validate_path("/home/app", Some(&Environment::Wsl)).is_ok());
        assert!(validate_path("/home/app", Some(&Environment::Remote)).is_ok());
        assert_eq!(inferred_environment("/home/app"), Environment::Remote);
        let hub = hub();
        let message=parse_message(&serde_json::json!({"kind":"hello","token":"aa".repeat(32),"session":"session-12345678","sequence":1,"roots":["/home/app"],"executionDirectory":"/home/worktree","originalProject":"/home/app","cwd":"C:\\tools","environment":"wsl"}).to_string()).unwrap();
        hub.apply(message, client(7), 1, 1000).unwrap();
        let entry = hub.live(1000).remove(0);
        assert_eq!(entry.execution_environment(), Environment::Wsl);
        assert_eq!(
            entry.effective_execution_directory(),
            Some("/home/worktree")
        );
    }
    #[test]
    fn bounds_pid_lifetimes_and_time_overflow_fail_closed() {
        let hub = hub();
        assert_eq!(
            hub.apply(hello(0), client(7), 1, 1000),
            Err("CONTEXT_SEQUENCE_INVALID".into())
        );
        assert_eq!(
            hub.apply(hello(1), client(7), 1, u64::MAX),
            Err("CONTEXT_TIME_OVERFLOW".into())
        );
        hub.apply(hello(1), client(7), 1, 1000).unwrap();
        hub.remove_connection(1);
        let mut reused = client(7);
        reused.started_at = "2000".into();
        assert_eq!(
            hub.apply(hello(2), reused, 2, 2000),
            Err("CONTEXT_SESSION_OWNER_MISMATCH".into())
        );
        assert!(validate_path(&format!("C:\\{}", "a".repeat(MAX_PATH_LEN)), None).is_err());
        assert!(valid_label(&Some("a".repeat(129)), 128, "HOST_SESSION").is_err());
        for index in 0..MAX_SESSIONS {
            hub.apply(
                frame("hello", &format!("session-number-{index:04}"), 1),
                client(7),
                2,
                2000,
            )
            .unwrap();
        }
        assert_eq!(
            hub.apply(hello(2), client(7), 2, 2000),
            Err("CONTEXT_SESSION_LIMIT".into())
        );
    }
}
