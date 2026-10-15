//! Brand-agnostic agent context registration: an authenticated local channel where a
//! configured agent tool reports the workspace of its current session.
//!
//! Trust chain, decided entirely in this backend:
//! 1. a pairing token created by the user in ArkDock (256-bit, never passed on argv);
//! 2. the OS-proven pipe client identity (`GetNamedPipeClientProcessId` plus the process
//!    start-time lifetime token) — a message can never claim a different sender;
//! 3. bounded messages, short heartbeat TTLs, revocation on disconnect and pairing
//!    revocation, replay and duplicate-session rejection.
//!
//! A registration alone is context, not authority. It grants no message sending, no
//! Inspector image access and no execution; `discovery::context_source` binds entries to
//! a docked target by process ancestry and `automatic_candidate` still decides whether
//! anything auto-associates. Discovery stays read-only: this module never launches a
//! build, reads chat content, or trusts any caller-supplied "trusted" flag.

use std::collections::HashMap;

use serde::Deserialize;

use crate::ipc::model::ProcessRef;

/// The local pipe every configured agent tool connects to. The name is fixed; the
/// per-instance pairing token is what authorizes a connection, not the path.
pub const CONTEXT_PIPE_NAME: &str = "\\\\.\\pipe\\arkdock-context";

/// One wire message may not exceed this size; the reader drops the connection beyond it.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;
pub const MAX_CONNECTIONS: usize = 8;
pub const MAX_SESSIONS: usize = 32;
/// Hosts declare at most this many workspace roots per session.
pub const MAX_ROOTS: usize = 8;
const MAX_PATH_LEN: usize = 4096;
const MAX_AGENT_LEN: usize = 64;
/// Heartbeats must arrive inside this window; evidence issued from a registration never
/// outlives it and stays far below [`crate::discovery::MAX_EVIDENCE_TTL_MS`].
pub const HEARTBEAT_TTL_MS: u64 = 15_000;
/// A connection that sends nothing for this long is cancelled by the listener watchdog.
pub const CONNECTION_IDLE_TIMEOUT_MS: u64 = 20_000;

// ---------------------------------------------------------------------------
// Wire messages
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextMessage {
    Hello {
        token: String,
        session: String,
        roots: Vec<String>,
        cwd: Option<String>,
        agent: Option<String>,
    },
    Heartbeat {
        token: String,
        session: String,
    },
    Update {
        token: String,
        session: String,
        roots: Vec<String>,
    },
    Revoke {
        token: String,
        session: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloFrame {
    // The kind tag is consumed by parse_message before this struct is built; it is
    // required on the wire so unknown-field rejection stays exact.
    #[allow(dead_code)]
    kind: String,
    token: String,
    session: String,
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    agent: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionFrame {
    #[allow(dead_code)]
    kind: String,
    token: String,
    session: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateFrame {
    #[allow(dead_code)]
    kind: String,
    token: String,
    session: String,
    roots: Vec<String>,
}

/// Parses one bounded JSON line. Unknown kinds and unknown fields are rejected so an
/// upgraded client cannot silently smuggle extra semantics into an older backend.
pub fn parse_message(line: &str) -> Result<ContextMessage, String> {
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|error| format!("CONTEXT_FRAME_INVALID: {error}"))?;
    let kind = value
        .get("kind")
        .and_then(|kind| kind.as_str())
        .ok_or("CONTEXT_FRAME_KIND_MISSING")?;
    match kind {
        "hello" => {
            let frame: HelloFrame = serde_json::from_value(value)
                .map_err(|error| format!("CONTEXT_FRAME_INVALID: {error}"))?;
            Ok(ContextMessage::Hello {
                token: frame.token,
                session: frame.session,
                roots: frame.roots,
                cwd: frame.cwd,
                agent: frame.agent,
            })
        }
        "heartbeat" => {
            let frame: SessionFrame = serde_json::from_value(value)
                .map_err(|error| format!("CONTEXT_FRAME_INVALID: {error}"))?;
            Ok(ContextMessage::Heartbeat {
                token: frame.token,
                session: frame.session,
            })
        }
        "update" => {
            let frame: UpdateFrame = serde_json::from_value(value)
                .map_err(|error| format!("CONTEXT_FRAME_INVALID: {error}"))?;
            Ok(ContextMessage::Update {
                token: frame.token,
                session: frame.session,
                roots: frame.roots,
            })
        }
        "revoke" => {
            let frame: SessionFrame = serde_json::from_value(value)
                .map_err(|error| format!("CONTEXT_FRAME_INVALID: {error}"))?;
            Ok(ContextMessage::Revoke {
                token: frame.token,
                session: frame.session,
            })
        }
        other => Err(format!("CONTEXT_FRAME_KIND_UNKNOWN: {other}")),
    }
}

// ---------------------------------------------------------------------------
// Registry state
// ---------------------------------------------------------------------------

/// One live agent session registration. `client` is the OS-proven pipe client identity;
/// it is never taken from the message body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextEntry {
    pub session: String,
    pub client: ProcessRef,
    /// Host-declared workspace roots (`roots/list` when the host supports it).
    pub roots: Vec<String>,
    /// Spawn-time working directory as a weak hint when the host has no roots support.
    pub cwd: Option<String>,
    /// Bounded display hint, never an identity claim.
    pub agent: Option<String>,
    pub observed_at: u64,
    pub expires_at: u64,
}

impl ContextEntry {
    /// The root this entry proposes for discovery. Roots are host declarations; the
    /// spawn-time working directory is only a fallback hint and never a host claim.
    pub fn declared_roots(&self) -> &[String] {
        &self.roots
    }

    /// Whether this entry carries a host root declaration at all.
    pub fn has_root_declaration(&self) -> bool {
        !self.roots.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TokenState {
    bytes: [u8; 32],
    hex: String,
}

#[derive(Default)]
struct HubInner {
    token: Option<TokenState>,
    entries: HashMap<String, ContextEntry>,
    last_event: Option<(String, u64)>,
}

/// Shared registration state. The pipe listener is the only writer besides the
/// association actor's pairing commands; discovery reads a pruned snapshot per pass.
pub struct ContextHub {
    inner: std::sync::Mutex<HubInner>,
}

impl Default for ContextHub {
    fn default() -> Self {
        Self {
            inner: std::sync::Mutex::new(HubInner::default()),
        }
    }
}

impl ContextHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs a freshly generated pairing token. A previous token stays valid until
    /// this call replaces it, so staging a credential cannot break a live connection.
    pub fn activate_token(&self, token: [u8; 32]) {
        let hex = token.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut inner = self.lock();
        inner.token = Some(TokenState { bytes: token, hex });
        inner.note("Context source pairing activated".to_owned());
    }

    /// Revokes the pairing and every live registration. Existing pipe connections fail
    /// on their next message; no entry survives a revoked pairing.
    pub fn revoke_token(&self) -> bool {
        let mut inner = self.lock();
        let had = inner.token.take().is_some();
        inner.entries.clear();
        inner.note("Context source pairing revoked".to_owned());
        had
    }

    pub fn paired(&self) -> bool {
        self.lock().token.is_some()
    }

    /// Applies one authenticated-by-connection message. `client` must come from the
    /// OS query on the pipe handle, never from the frame.
    pub fn apply(
        &self,
        message: ContextMessage,
        client: ProcessRef,
        now: u64,
    ) -> Result<(), String> {
        let mut inner = self.lock();
        let token = inner
            .token
            .as_ref()
            .ok_or("CONTEXT_PAIRING_MISSING")?
            .clone();
        let (session, kind, roots, cwd, agent) = match message {
            ContextMessage::Hello {
                token: claimed,
                session,
                roots,
                cwd,
                agent,
            } => {
                if !constant_time_eq(&claimed, &token.hex) {
                    return Err("CONTEXT_TOKEN_INVALID".into());
                }
                (session, "hello", roots, cwd, agent)
            }
            ContextMessage::Heartbeat {
                token: claimed,
                session,
            } => {
                if !constant_time_eq(&claimed, &token.hex) {
                    return Err("CONTEXT_TOKEN_INVALID".into());
                }
                (session, "heartbeat", Vec::new(), None, None)
            }
            ContextMessage::Update {
                token: claimed,
                session,
                roots,
            } => {
                if !constant_time_eq(&claimed, &token.hex) {
                    return Err("CONTEXT_TOKEN_INVALID".into());
                }
                (session, "update", roots, None, None)
            }
            ContextMessage::Revoke {
                token: claimed,
                session,
            } => {
                if !constant_time_eq(&claimed, &token.hex) {
                    return Err("CONTEXT_TOKEN_INVALID".into());
                }
                if let Some(existing) = inner.entries.get(&session) {
                    if existing.client != client {
                        return Err("CONTEXT_SESSION_OWNER_MISMATCH".into());
                    }
                    inner.entries.remove(&session);
                    inner.note(format!("Context session revoked: {session}"));
                }
                return Ok(());
            }
        };
        valid_session(&session)?;
        let roots = validate_paths(roots, "roots")?;
        let cwd = match cwd {
            Some(cwd) => Some(
                validate_paths(vec![cwd], "cwd")?
                    .pop()
                    .ok_or("CONTEXT_PATH_EMPTY")?,
            ),
            None => None,
        };
        if let Some(agent) = agent.as_deref() {
            if agent.is_empty()
                || agent.len() > MAX_AGENT_LEN
                || agent.chars().any(char::is_control)
            {
                return Err("CONTEXT_AGENT_INVALID".into());
            }
        }
        match kind {
            "hello" => {
                if let Some(existing) = inner.entries.get(&session) {
                    if existing.client != client {
                        // A second process claiming a live session is a replay, not a reconnect.
                        return Err("CONTEXT_SESSION_OWNER_MISMATCH".into());
                    }
                } else if inner.entries.len() >= MAX_SESSIONS {
                    return Err("CONTEXT_SESSION_LIMIT".into());
                }
                let expires_at = now
                    .checked_add(HEARTBEAT_TTL_MS)
                    .ok_or("CONTEXT_TIME_OVERFLOW")?;
                inner.insert(
                    session.clone(),
                    ContextEntry {
                        session: session.clone(),
                        client,
                        roots,
                        cwd,
                        agent,
                        observed_at: now,
                        expires_at,
                    },
                );
                inner.note(format!("Context session registered: {session}"));
                Ok(())
            }
            _ => {
                let Some(existing) = inner.entries.get_mut(&session) else {
                    return Err("CONTEXT_SESSION_UNKNOWN".into());
                };
                if existing.client != client {
                    return Err("CONTEXT_SESSION_OWNER_MISMATCH".into());
                }
                let expires_at = now
                    .checked_add(HEARTBEAT_TTL_MS)
                    .ok_or("CONTEXT_TIME_OVERFLOW")?;
                if kind == "update" {
                    existing.roots = roots;
                }
                if let Some(cwd) = cwd {
                    existing.cwd = Some(cwd);
                }
                existing.observed_at = now;
                existing.expires_at = expires_at;
                inner.note(format!("Context session renewed: {session}"));
                Ok(())
            }
        }
    }

    /// Removes every registration owned by one connection (disconnect or timeout).
    pub fn remove_client(&self, client: &ProcessRef) -> usize {
        let mut inner = self.lock();
        let before = inner.entries.len();
        inner.entries.retain(|_, entry| entry.client != *client);
        let removed = before - inner.entries.len();
        if removed > 0 {
            inner.note(format!(
                "Context client disconnected; {removed} session(s) revoked"
            ));
        }
        removed
    }

    /// Live, pruned registrations in deterministic order.
    pub fn live(&self, now: u64) -> Vec<ContextEntry> {
        let mut inner = self.lock();
        inner.entries.retain(|_, entry| entry.expires_at > now);
        let mut entries: Vec<_> = inner.entries.values().cloned().collect();
        entries.sort_by(|a, b| a.session.cmp(&b.session));
        entries
    }

    pub fn last_event(&self) -> Option<(String, u64)> {
        self.lock().last_event.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HubInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl HubInner {
    fn note(&mut self, reason: String) {
        self.last_event = Some((reason, crate::context::now_millis()));
    }

    fn insert(&mut self, session: String, entry: ContextEntry) {
        self.entries.insert(session, entry);
    }
}

/// Wall-clock milliseconds. Kept here so the hub and its tests share one time source.
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Generates a fresh pairing secret without activating it: `(hex, bytes)`. Randomness
/// is a Windows runtime capability; the hub and its tests accept explicit secrets so
/// every rule stays cross-platform testable.
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

/// Length-independent comparison; the loop always walks the longer input so the answer
/// does not leak where the first difference was.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for index in 0..a.len().max(b.len()) {
        let left = a.get(index).copied().unwrap_or(0);
        let right = b.get(index).copied().unwrap_or(0);
        diff |= (left ^ right) as usize;
    }
    diff == 0
}

fn valid_session(session: &str) -> Result<(), String> {
    if (16..=128).contains(&session.len())
        && session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        Ok(())
    } else {
        Err("CONTEXT_SESSION_INVALID".into())
    }
}

fn validate_paths(paths: Vec<String>, field: &str) -> Result<Vec<String>, String> {
    if paths.len() > MAX_ROOTS {
        return Err(format!("CONTEXT_{field}_LIMIT"));
    }
    let mut validated = Vec::with_capacity(paths.len());
    for path in paths {
        if path.is_empty() || path.len() > MAX_PATH_LEN || path.contains('\0') {
            return Err(format!("CONTEXT_{field}_INVALID"));
        }
        if path.chars().any(char::is_control) {
            return Err(format!("CONTEXT_{field}_INVALID"));
        }
        if validated.contains(&path) {
            continue;
        }
        validated.push(path);
    }
    Ok(validated)
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

    fn token() -> String {
        "a".repeat(64)
    }

    fn hub_with_token() -> ContextHub {
        let hub = ContextHub::new();
        hub.activate_token([0xaa; 32]);
        assert!(hub.paired());
        hub
    }

    fn hello(session: &str, roots: Vec<String>) -> ContextMessage {
        ContextMessage::Hello {
            token: token(),
            session: session.into(),
            roots,
            cwd: None,
            agent: None,
        }
    }

    #[test]
    fn parse_accepts_the_four_kinds_and_rejects_unknown_fields_and_kinds() {
        let parsed = parse_message(
            r#"{"kind":"hello","token":"t","session":"s","roots":["C:\\a"],"agent":"claude"}"#,
        )
        .unwrap();
        assert!(matches!(parsed, ContextMessage::Hello { .. }));
        assert!(matches!(
            parse_message(r#"{"kind":"heartbeat","token":"t","session":"s"}"#),
            Ok(ContextMessage::Heartbeat { .. })
        ));
        assert!(parse_message(r#"{"kind":"status","token":"t","session":"s"}"#).is_err());
        assert!(parse_message(
            r#"{"kind":"hello","token":"t","session":"s","roots":[],"trusted":true}"#
        )
        .is_err());
        assert!(parse_message("not json").is_err());
    }

    #[test]
    fn hello_registers_and_heartbeat_renews_only_with_a_valid_token() {
        let hub = hub_with_token();
        let now = 1_000;
        hub.apply(
            hello("session-abc-1234", vec!["C:\\app".into()]),
            client(7),
            now,
        )
        .unwrap();
        let live = hub.live(now + 1);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].roots, vec!["C:\\app"]);
        assert_eq!(live[0].expires_at, now + HEARTBEAT_TTL_MS);

        let renew = ContextMessage::Heartbeat {
            token: token(),
            session: "session-abc-1234".into(),
        };
        hub.apply(renew.clone(), client(7), now + 5_000).unwrap();
        assert_eq!(
            hub.live(now + 1)[0].expires_at,
            now + 5_000 + HEARTBEAT_TTL_MS
        );

        // Wrong token: rejected, and nothing changes.
        let forged = ContextMessage::Heartbeat {
            token: "b".repeat(64),
            session: "session-abc-1234".into(),
        };
        assert_eq!(
            hub.apply(forged, client(7), now + 6_000),
            Err("CONTEXT_TOKEN_INVALID".into())
        );
        assert_eq!(
            hub.live(now + 1)[0].expires_at,
            now + 5_000 + HEARTBEAT_TTL_MS
        );

        // A different process cannot renew or take over the session (replay guard).
        assert_eq!(
            hub.apply(renew, client(8), now + 6_000),
            Err("CONTEXT_SESSION_OWNER_MISMATCH".into())
        );
    }

    #[test]
    fn expired_sessions_are_pruned_and_disconnect_removes_the_whole_client() {
        let hub = hub_with_token();
        let now = 1_000;
        hub.apply(
            hello("session-abc-1234", vec!["C:\\app".into()]),
            client(7),
            now,
        )
        .unwrap();
        hub.apply(
            hello("session-def-4567", vec!["C:\\other".into()]),
            client(8),
            now,
        )
        .unwrap();
        assert_eq!(hub.live(now + HEARTBEAT_TTL_MS + 1).len(), 0);

        hub.apply(
            hello("session-abc-1234", vec!["C:\\app".into()]),
            client(7),
            now,
        )
        .unwrap();
        hub.apply(
            hello("session-def-4567", vec!["C:\\other".into()]),
            client(8),
            now,
        )
        .unwrap();
        assert_eq!(hub.remove_client(&client(7)), 1);
        let live = hub.live(now + 1);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].session, "session-def-4567");
    }

    #[test]
    fn update_replaces_roots_and_revoke_is_idempotent_but_owner_checked() {
        let hub = hub_with_token();
        let now = 1_000;
        hub.apply(
            hello("session-abc-1234", vec!["C:\\app".into()]),
            client(7),
            now,
        )
        .unwrap();
        let update = ContextMessage::Update {
            token: token(),
            session: "session-abc-1234".into(),
            roots: vec!["C:\\moved".into()],
        };
        hub.apply(update.clone(), client(7), now + 1_000).unwrap();
        assert_eq!(hub.live(now + 1_000)[0].roots, vec!["C:\\moved"]);

        let revoke = ContextMessage::Revoke {
            token: token(),
            session: "session-abc-1234".into(),
        };
        hub.apply(revoke.clone(), client(7), now + 2_000).unwrap();
        assert!(hub.live(now + 2_000).is_empty());
        // Revoking an unknown session is accepted (idempotent), but a foreign client
        // cannot revoke someone else's session.
        hub.apply(
            hello("session-abc-1234", vec!["C:\\app".into()]),
            client(7),
            now + 3_000,
        )
        .unwrap();
        assert_eq!(
            hub.apply(revoke, client(9), now + 3_000),
            Err("CONTEXT_SESSION_OWNER_MISMATCH".into())
        );
        assert_eq!(hub.live(now + 3_000).len(), 1);
    }

    #[test]
    fn missing_pairing_rejects_everything_and_revocation_clears_all_sessions() {
        let hub = ContextHub::new();
        assert_eq!(
            hub.apply(hello("session-abc-1234", vec![]), client(7), 1_000),
            Err("CONTEXT_PAIRING_MISSING".into())
        );
        let hub = hub_with_token();
        hub.apply(
            hello("session-abc-1234", vec!["C:\\app".into()]),
            client(7),
            1_000,
        )
        .unwrap();
        assert!(hub.revoke_token());
        assert!(hub.live(1_001).is_empty());
        assert!(!hub.paired());
        assert_eq!(
            hub.apply(
                hello("session-abc-1234", vec!["C:\\app".into()]),
                client(7),
                2_000
            ),
            Err("CONTEXT_PAIRING_MISSING".into())
        );
    }

    #[test]
    fn session_shape_path_bounds_and_unproven_cwd_are_enforced() {
        let hub = hub_with_token();
        let now = 1_000;
        for session in ["short", "with space", "with/slash", ""] {
            assert_eq!(
                hub.apply(hello(session, vec![]), client(7), now),
                Err("CONTEXT_SESSION_INVALID".into())
            );
        }
        assert_eq!(
            hub.apply(
                hello("session-abc-1234", vec![String::new()]),
                client(7),
                now
            ),
            Err("CONTEXT_roots_INVALID".into())
        );
        assert_eq!(
            hub.apply(
                hello("session-abc-1234", vec!["bad\u{0}path".into()]),
                client(7),
                now
            ),
            Err("CONTEXT_roots_INVALID".into())
        );
        let many = (0..9).map(|index| format!("C:\\app{index}")).collect();
        assert_eq!(
            hub.apply(hello("session-abc-1234", many), client(7), now),
            Err("CONTEXT_roots_LIMIT".into())
        );
        // A declared path keeps its own environment; validation never rewrites it.
        hub.apply(
            hello(
                "session-abc-1234",
                vec![r"\\wsl.localhost\Ubuntu\app".into()],
            ),
            client(7),
            now,
        )
        .unwrap();
        assert_eq!(
            hub.live(now + 1)[0].roots,
            vec![r"\\wsl.localhost\Ubuntu\app".to_string()]
        );
    }

    #[test]
    fn the_session_limit_is_bounded() {
        let hub = hub_with_token();
        let now = 1_000;
        for index in 0..MAX_SESSIONS {
            let session = format!("session-{index:016}");
            hub.apply(
                hello(&session, vec!["C:\\app".into()]),
                client(index as u32 + 1),
                now,
            )
            .unwrap();
        }
        assert_eq!(
            hub.apply(
                hello("session-overflow-000", vec!["C:\\app".into()]),
                client(999),
                now
            ),
            Err("CONTEXT_SESSION_LIMIT".into())
        );
        assert_eq!(hub.live(now + 1).len(), MAX_SESSIONS);
    }

    #[test]
    fn declared_roots_and_the_cwd_fallback_are_distinguishable() {
        let hub = hub_with_token();
        let now = 1_000;
        let message = ContextMessage::Hello {
            token: token(),
            session: "session-abc-1234".into(),
            roots: Vec::new(),
            cwd: Some("C:\\fallback".into()),
            agent: Some("claude".into()),
        };
        hub.apply(message, client(7), now).unwrap();
        let entry = &hub.live(now + 1)[0];
        assert!(!entry.has_root_declaration());
        assert_eq!(entry.declared_roots(), &[] as &[String]);
        assert_eq!(entry.cwd.as_deref(), Some("C:\\fallback"));
    }

    #[test]
    fn token_comparison_is_length_independent_and_exact() {
        assert!(constant_time_eq(&token(), &token()));
        assert!(!constant_time_eq(&token(), &"a".repeat(63)));
        assert!(!constant_time_eq(&token(), &"b".repeat(64)));
        assert!(!constant_time_eq("", "x"));
    }
}
