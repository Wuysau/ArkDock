//! Bounded, latest-first binary frame mailbox.
//!
//! The mailbox owns exactly one copy of each accepted frame and never grows without
//! bound: at most eight retained frames and four mebibytes of payload. `latest` is the
//! frame a scoped read returns first, so a slow consumer can only ever skip ahead to
//! newer pixels instead of building an unbounded backlog.
//!
//! Ownership rules kept here, not in the session coordinator:
//! - only the current consumer of the current session may read or disable the mailbox;
//! - a read revalidates the frame against trust, phase, workspace, session, build,
//!   generation and displayed generation before any pixel leaves the backend;
//! - a replaced session clears the mailbox instead of retaining foreign pixels.
use super::*;

/// Bounded retention: at most eight frames and four mebibytes of payload.
const MAX_RETAINED_FRAMES: usize = 8;
const MAX_RETAINED_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub(super) struct FrameDelivery {
    binary_enabled: bool,
    binary_session: Option<String>,
    consumer_id: Option<String>,
    latest: Option<BinaryFrame>,
    retained: VecDeque<BinaryFrame>,
}

impl FrameDelivery {
    pub(super) fn enable(
        &mut self,
        protocol_version: u32,
        session_id: &str,
        consumer_id: &str,
    ) -> Result<(), String> {
        if protocol_version != BINARY_FRAME_PROTOCOL_VERSION {
            return Err("PROTOCOL_VERSION_UNSUPPORTED".into());
        }
        if session_id.is_empty() || session_id.len() > 256 {
            return Err("PREVIEW_SESSION_REQUIRED".into());
        }
        if consumer_id.is_empty() || consumer_id.len() > 128 {
            return Err("INVALID_FRAME_CONSUMER".into());
        }
        self.binary_enabled = true;
        self.binary_session = Some(session_id.to_owned());
        self.consumer_id = Some(consumer_id.to_owned());
        Ok(())
    }
    pub(super) fn push(&mut self, frame: BinaryFrame) {
        if self.retained.len() >= MAX_RETAINED_FRAMES {
            self.retained.pop_front();
        }
        while !self.retained.is_empty()
            && self.retained.iter().map(|f| f.payload.len()).sum::<usize>() + frame.payload.len()
                > MAX_RETAINED_BYTES
        {
            self.retained.pop_front();
        }
        self.retained.push_back(frame.clone());
        self.latest = Some(frame);
    }
    pub(super) fn clear(&mut self) {
        self.latest = None;
        self.binary_enabled = false;
        self.binary_session = None;
        self.consumer_id = None;
        self.retained.clear();
    }
    pub(super) fn disable(&mut self, consumer_id: &str) -> Result<(), String> {
        if self.consumer_id.as_deref() != Some(consumer_id) {
            return Err("STALE_FRAME_CONSUMER".into());
        }
        self.binary_enabled = false;
        self.binary_session = None;
        self.consumer_id = None;
        // Keep the most recent real frame for explicitly chosen screenshot contexts.
        Ok(())
    }
    /// True when the session has no binary consumer and therefore still needs the
    /// legacy base64 frame event for compatibility.
    pub(super) fn wants_legacy_delivery(&self, session_id: &str) -> bool {
        !self.binary_enabled || self.binary_session.as_deref() != Some(session_id)
    }
    /// Returns the newest real frame for the current build, or an empty packet when the
    /// mailbox holds nothing yet. Any identity mismatch invalidates the mailbox first.
    pub(super) fn take_current(
        &mut self,
        state: &AppSnapshot,
        consumer_id: &str,
    ) -> Result<Vec<u8>, String> {
        if !self.binary_enabled {
            return Err("BINARY_TRANSPORT_NOT_ENABLED".into());
        }
        if self.consumer_id.as_deref() != Some(consumer_id) {
            return Err("STALE_FRAME_CONSUMER".into());
        }
        if self.binary_session.as_deref() != state.session_id.as_deref() {
            self.clear();
            return Err("STALE_SESSION".into());
        }
        let Some(frame) = self.latest.as_ref() else {
            return Ok(Vec::new());
        };
        let metadata = &frame.metadata;
        if !state.trusted
            || state.phase != Phase::Running
            || state.build.stale
            || state.session_id.as_deref() != Some(&metadata.session_id)
            || state.project.as_ref().map(|project| project.id.as_str())
                != Some(metadata.workspace_id.as_str())
            || state.build.build_id.as_deref() != Some(&metadata.build_id)
            || state.build.generation != metadata.generation
            || state.build.displayed_generation != Some(metadata.generation)
        {
            self.latest = None;
            return Err("STALE_FRAME".into());
        }
        self.latest.take().ok_or("FRAME_UNAVAILABLE")?.packet()
    }
    /// Retained frames are the bounded set a selection may anchor on.
    pub(super) fn retained(&self) -> &VecDeque<BinaryFrame> {
        &self.retained
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_teardown_releases_pixels_and_restores_legacy_delivery() {
        let mut delivery = FrameDelivery::default();
        delivery.enable(1, "s", "consumer").unwrap();
        delivery.clear();
        assert!(!delivery.binary_enabled);
        assert!(delivery.wants_legacy_delivery("s"));
    }
    #[test]
    fn obsolete_consumer_cannot_close_same_session_successor() {
        let mut delivery = FrameDelivery::default();
        delivery.enable(1, "session", "old").unwrap();
        delivery.enable(1, "session", "new").unwrap();
        assert_eq!(delivery.disable("old").unwrap_err(), "STALE_FRAME_CONSUMER");
        assert!(delivery.binary_enabled);
        assert_eq!(delivery.consumer_id.as_deref(), Some("new"));
        delivery.disable("new").unwrap();
        assert!(!delivery.binary_enabled);
        assert!(delivery.enable(1, "session", "").is_err());
    }

    #[test]
    fn binary_delivery_is_bounded_versioned_and_rejects_a_replaced_session() {
        use crate::preview::frames::BinaryFrame;
        let mut state = initial(crate::preview::probe(None));
        state.session_id = Some("s".into());
        state.trusted = true;
        state.phase = Phase::Running;
        state.build.build_id = Some("b".into());
        state.build.generation = 1;
        state.build.displayed_generation = Some(1);
        state.project = Some(ProjectRef {
            id: "w".into(),
            name: "test".into(),
            environment: Environment::WindowsLocal,
            workspace_root: "test".into(),
            app_root: "test".into(),
            module: "entry".into(),
            variant: "default".into(),
            sdk_id: None,
        });
        let frame = |sequence| BinaryFrame {
            metadata: BinaryFrameMetadata {
                protocol_version: 1,
                workspace_id: "w".into(),
                session_id: "s".into(),
                build_id: "b".into(),
                generation: 1,
                sequence,
                width: 360,
                height: 640,
                captured_at: 1,
                rotation: 0,
                format: NativeFrameFormat::Jpeg,
                payload_length: 4,
            },
            payload: vec![0xff, 0xd8, 0xff, 0xd9],
        };
        let mut delivery = FrameDelivery::default();
        assert_eq!(
            delivery.enable(2, "s", "consumer").unwrap_err(),
            "PROTOCOL_VERSION_UNSUPPORTED"
        );
        assert_eq!(
            delivery.enable(1, "", "consumer").unwrap_err(),
            "PREVIEW_SESSION_REQUIRED"
        );
        delivery.enable(1, "s", "consumer").unwrap();
        delivery.push(frame(1));
        delivery.push(frame(2));
        let packet = delivery.take_current(&state, "consumer").unwrap();
        let length = u32::from_le_bytes(packet[..4].try_into().unwrap()) as usize;
        let metadata: BinaryFrameMetadata = serde_json::from_slice(&packet[4..4 + length]).unwrap();
        assert_eq!(metadata.sequence, 2);
        assert_eq!(packet[4 + length..], [0xff, 0xd8, 0xff, 0xd9]);
        assert!(delivery
            .take_current(&state, "consumer")
            .unwrap()
            .is_empty());
        for sequence in 3..=20 {
            delivery.push(frame(sequence));
        }
        assert_eq!(delivery.retained.len(), MAX_RETAINED_FRAMES);
        assert_eq!(delivery.retained.front().unwrap().metadata.sequence, 13);
        for sequence in 21..=28 {
            let mut large = frame(sequence);
            large.payload = vec![0; crate::preview::frames::MAX_JPEG_PAYLOAD_BYTES];
            large.metadata.payload_length = large.payload.len() as u32;
            delivery.push(large);
        }
        assert!(
            delivery
                .retained
                .iter()
                .map(|item| item.payload.len())
                .sum::<usize>()
                <= MAX_RETAINED_BYTES
        );
        assert!(delivery.retained.len() <= MAX_RETAINED_FRAMES);
        delivery.push(frame(3));
        state.session_id = Some("new".into());
        assert_eq!(
            delivery.take_current(&state, "consumer").unwrap_err(),
            "STALE_SESSION"
        );
        delivery.clear();
        assert_eq!(
            delivery.take_current(&state, "consumer").unwrap_err(),
            "BINARY_TRANSPORT_NOT_ENABLED"
        );
    }

    #[test]
    fn a_stale_build_cannot_hand_its_pixels_to_the_reader() {
        let mut state = initial(crate::preview::probe(None));
        state.session_id = Some("s".into());
        state.trusted = true;
        state.phase = Phase::Running;
        state.build.build_id = Some("b".into());
        state.build.generation = 2;
        state.build.displayed_generation = Some(1);
        state.project = Some(ProjectRef {
            id: "w".into(),
            name: "test".into(),
            environment: Environment::WindowsLocal,
            workspace_root: "test".into(),
            app_root: "test".into(),
            module: "entry".into(),
            variant: "default".into(),
            sdk_id: None,
        });
        let mut delivery = FrameDelivery::default();
        delivery.enable(1, "s", "consumer").unwrap();
        delivery.push(BinaryFrame {
            metadata: BinaryFrameMetadata {
                protocol_version: 1,
                workspace_id: "w".into(),
                session_id: "s".into(),
                build_id: "b".into(),
                generation: 1,
                sequence: 1,
                width: 360,
                height: 640,
                captured_at: 1,
                rotation: 0,
                format: NativeFrameFormat::Jpeg,
                payload_length: 4,
            },
            payload: vec![0xff, 0xd8, 0xff, 0xd9],
        });
        state.build.stale = true;
        assert_eq!(
            delivery.take_current(&state, "consumer").unwrap_err(),
            "STALE_FRAME"
        );
        // A rejected read invalidates the mailbox instead of retaining stale pixels.
        assert!(delivery.latest.is_none());
    }
}
