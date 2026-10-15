//! Preview session lifecycle: what starts, what stops, and in which order.
//!
//! The Engine stays the single coordinator. This module owns the *rules* of teardown so
//! the same order is used by every path that ends a session: a workspace change, a
//! revoked trust, an unavailable toolchain, a runtime failure, or an explicit Stop.
//!
//! One order, one place, because the order is load-bearing:
//! 1. invalidate queued assisted/Agent work, so no asynchronous reply can arrive later;
//! 2. cancel native controls and reset their state before the Previewer goes away;
//! 3. drop any pending Inspector query and answer it as a stale session;
//! 4. release the frame mailbox, so stale pixels cannot be read as the current build;
//! 5. drop the file watcher, so no further build request can be queued;
//! 6. stop the owned Previewer and close the build epoch, which rejects every late
//!    completion, frame and receipt that belonged to the previous epoch.
use super::*;

/// Outcome of the execution gate.
///
/// `teardown` is returned instead of being performed by the gate itself, so the decision
/// is pure and the coordinator performs the side effect exactly once.
struct ExecutionGate {
    phase: Phase,
    trusted: bool,
    teardown: bool,
}

/// Decides whether execution may start, and whether a live session must be closed first.
///
/// Closing execution never changes the independently verified project permission: an
/// unavailable toolchain blocks the phase but does not revoke trust.
fn execution_gate(trusted: bool, available: bool, reason: &str) -> ExecutionGate {
    if !trusted {
        return ExecutionGate {
            phase: Phase::Untrusted,
            trusted: false,
            teardown: true,
        };
    }
    if !available {
        return ExecutionGate {
            phase: if reason.starts_with("SDK_MISSING") {
                Phase::SdkMissing
            } else {
                Phase::ProtocolBlocked
            },
            trusted: true,
            teardown: true,
        };
    }
    ExecutionGate {
        phase: Phase::Building,
        trusted: true,
        teardown: false,
    }
}

impl Engine {
    /// Releases every resource owned by the current session. See the module comment for
    /// why the order is fixed. Session identity is left to the caller, because "stopped"
    /// and "suspended" differ in what they keep.
    fn teardown_resources(&mut self) {
        // 1. Automatic workspace changes also invalidate user-confirmed assisted offers.
        self.cancel_assisted();
        // 2. Native controls belong to the surface that is about to disappear.
        self.backend.cancel_controls();
        self.controls.reset();
        self.device_runtime.cancel();
        // 3. A pending Inspector query must not report into a replaced session.
        self.inspection_tree = None;
        self.inspection_anchor = None;
        if let Some(pending) = self.inspection.take() {
            let _ = pending.reply.send(Err("STALE_INSPECTOR_SESSION".into()));
        }
        // 4. Pixels are released before the transport that produced them.
        if let Ok(mut frames) = self.frames.lock() {
            frames.clear();
        }
        // 5. The watcher is dropped before the build epoch closes, so a queued change
        //    cannot start a build in the new epoch.
        self.watcher.take();
        // 6. Previewer, then the build epoch. Both reject late results by construction.
        self.backend.stop();
        self.scheduler.stop();
        self.watch_pending = Arc::new(Mutex::new(PendingWatch::default()));
    }

    /// Ends the session and revokes execution. Used when trust is gone, the workspace
    /// changed, discovery failed, the toolchain is unavailable, or the runtime crashed.
    pub(super) fn suspend_execution(&mut self) {
        self.teardown_resources();
        self.snapshot.session_id = None;
        self.snapshot.session_ref = None;
        self.snapshot.trusted = false;
    }

    /// Ends the session on an explicit user stop. Trust and the associated project are
    /// kept: stopping preview is not a statement about the project itself.
    pub(super) fn stop_execution(&mut self) {
        self.teardown_resources();
        self.snapshot.session_id = None;
        self.snapshot.session_ref = None;
        self.snapshot.phase = Phase::Stopped;
    }

    /// Ends both the session and the workspace association.
    pub(super) fn close_session(&mut self) {
        self.suspend_execution();
        self.snapshot.project = None;
        self.associated_manually = false;
        self.context_binding = None;
        self.context_confirmation_required = false;
        self.directory_candidates.clear();
    }

    /// Re-evaluates trust and toolchain availability before any build can start.
    pub(super) fn gate_preview(&mut self) {
        let Some(project) = self.snapshot.project.as_ref() else {
            return;
        };
        self.snapshot.trusted = self
            .store
            .as_ref()
            .is_some_and(|store| store.is_trusted(project));
        let reason = self.snapshot.backend.reason.clone();
        let decision = execution_gate(
            self.snapshot.trusted,
            self.snapshot.backend.available,
            &reason,
        );
        if decision.teardown {
            self.suspend_execution();
        }
        self.snapshot.phase = decision.phase;
        self.snapshot.trusted = decision.trusted;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::FrameReject;

    #[test]
    fn an_untrusted_gate_closes_execution_before_entering_untrusted() {
        let mut scheduler = BuildCoordinator::new(0);
        scheduler.set_workspace("workspace", "session", 1);
        scheduler.request_refresh(1);
        let old = scheduler.next_task(1).unwrap();
        let decision = execution_gate(false, true, "verified");
        assert!(decision.teardown);
        assert_eq!(decision.phase, Phase::Untrusted);
        assert!(!decision.trusted);
        // The coordinator closes the epoch; every late result is rejected afterwards.
        scheduler.stop();
        assert!(!scheduler.is_building());
        assert_eq!(
            scheduler.complete(&old, BuildOutcome::Success, 2),
            CompletionDisposition::RejectedStopped
        );
    }

    #[test]
    fn source_only_change_keeps_the_trusted_execution_open() {
        let decision = execution_gate(true, true, "verified");
        assert_eq!(decision.phase, Phase::Building);
        assert!(decision.trusted);
        assert!(!decision.teardown);
    }

    #[test]
    fn sdk_block_does_not_revoke_valid_project_trust() {
        for (reason, expected) in [
            ("SDK_MISSING: not configured", Phase::SdkMissing),
            ("PROTOCOL_UNVERIFIED", Phase::ProtocolBlocked),
        ] {
            let decision = execution_gate(true, false, reason);
            assert!(
                decision.teardown,
                "a blocked toolchain must close execution"
            );
            assert_eq!(decision.phase, expected);
            assert!(
                decision.trusted,
                "toolchain state cannot revoke project trust"
            );
        }
        let decision = execution_gate(false, false, "SDK_MISSING");
        assert_eq!(decision.phase, Phase::Untrusted);
        assert!(
            !decision.trusted,
            "revoked or changed-config trust stays revoked"
        );
    }

    #[test]
    fn a_stopped_session_rejects_late_frames_until_the_next_epoch() {
        use crate::ipc::model::{BinaryFrameMetadata, NativeFrameFormat};
        use crate::preview::frames::BinaryFrame;
        let mut scheduler = BuildCoordinator::new(0);
        scheduler.set_workspace("workspace", "session", 1);
        scheduler.request_refresh(1);
        let task = scheduler.next_task(1).unwrap();
        scheduler.complete(&task, BuildOutcome::Success, 2);
        // A user stop closes the epoch that owned the running Previewer.
        scheduler.stop();
        let late = BinaryFrame {
            metadata: BinaryFrameMetadata {
                protocol_version: 1,
                workspace_id: "workspace".into(),
                session_id: task.session_id.clone(),
                build_id: task.build_id.clone(),
                generation: task.generation,
                sequence: 1,
                width: 360,
                height: 640,
                captured_at: 1,
                rotation: 0,
                format: NativeFrameFormat::Jpeg,
                payload_length: 4,
            },
            payload: vec![0xff, 0xd8, 0xff, 0xd9],
        };
        assert_eq!(
            scheduler.accept_binary_frame(&late),
            Err(FrameReject::Stopped),
            "a stopped session must not accept a late frame"
        );
        // Selecting the same workspace again opens a new epoch with a new build id.
        scheduler.set_workspace("workspace", "session-2", 3);
        scheduler.request_refresh(3);
        let next = scheduler.next_task(3).unwrap();
        assert_ne!(next.build_id, task.build_id);
        assert_ne!(next.epoch, task.epoch);
        assert!(scheduler.accept_binary_frame(&late).is_err());
    }
}
