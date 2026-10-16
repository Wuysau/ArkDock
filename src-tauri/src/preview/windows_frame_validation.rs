//! One JPEG decode in flight, one replaceable candidate and one completed result.
//! The socket/command owner never waits for pixel decoding.
use super::super::frames::{validate_jpeg_pixels, BinaryFrame};
use crate::scheduler::BuildTask;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

pub(super) struct Candidate {
    pub frame: BinaryFrame,
    pub context: (u64, u64),
    pub navigation_revision: Option<u64>,
    pub observing_at_receive: bool,
}

impl Candidate {
    pub fn matches(&self, task: &BuildTask, context: (u64, u64), surface: (u32, u32)) -> bool {
        let frame = &self.frame.metadata;
        self.context == context
            && (frame.width, frame.height) == surface
            && frame.workspace_id == task.workspace_id
            && frame.session_id == task.session_id
            && frame.build_id == task.build_id
            && frame.generation == task.generation
    }
}

pub(super) struct Validated {
    pub candidate: Candidate,
    pub result: Result<(), String>,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Metrics {
    pub submitted: u64,
    pub replaced: u64,
    pub validated: u64,
    pub validation_micros: u64,
    pub max_validation_micros: u64,
}

#[derive(Default)]
struct State {
    pending: Option<Candidate>,
    completed: Option<Validated>,
    stopped: bool,
    metrics: Metrics,
}

pub(super) struct Validator {
    state: Arc<(Mutex<State>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl Validator {
    pub fn new() -> Self {
        Self::with_decoder(validate_jpeg_pixels)
    }

    fn with_decoder(decode: impl Fn(&BinaryFrame) -> Result<(), String> + Send + 'static) -> Self {
        let state = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let shared = state.clone();
        let worker = thread::spawn(move || loop {
            let (lock, ready) = &*shared;
            let mut state = lock.lock().unwrap_or_else(|error| error.into_inner());
            while state.pending.is_none() && !state.stopped {
                state = ready.wait(state).unwrap_or_else(|error| error.into_inner());
            }
            if state.stopped {
                break;
            }
            let candidate = state.pending.take().expect("pending validation candidate");
            drop(state);
            let started = Instant::now();
            let result = decode(&candidate.frame);
            let micros = started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
            let mut state = lock.lock().unwrap_or_else(|error| error.into_inner());
            if state.stopped {
                break;
            }
            state.metrics.validated = state.metrics.validated.saturating_add(1);
            state.metrics.validation_micros =
                state.metrics.validation_micros.saturating_add(micros);
            state.metrics.max_validation_micros = state.metrics.max_validation_micros.max(micros);
            // Do not let a later successful JPEG erase an unconsumed decode error.
            if state.completed.as_ref().is_none_or(|value| {
                value.result.is_ok() || value.candidate.context != candidate.context
            }) {
                state.completed = Some(Validated { candidate, result });
            }
        });
        Self {
            state,
            worker: Some(worker),
        }
    }

    pub fn submit(&self, candidate: Candidate) {
        let (lock, ready) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(|error| error.into_inner());
        state.metrics.submitted = state.metrics.submitted.saturating_add(1);
        if state.pending.replace(candidate).is_some() {
            state.metrics.replaced = state.metrics.replaced.saturating_add(1);
        }
        ready.notify_one();
    }

    pub fn take(&self) -> Option<Validated> {
        self.state
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .completed
            .take()
    }

    pub fn metrics(&self) -> Metrics {
        self.state
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .metrics
    }
}

impl Drop for Validator {
    fn drop(&mut self) {
        let (lock, ready) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(|error| error.into_inner());
        state.stopped = true;
        state.pending = None;
        state.completed = None;
        ready.notify_one();
        drop(state);
        // The decoder has bounded input, dimensions and allocation; never leave
        // detached decoders accumulating across preview restarts.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::model::{BinaryFrameMetadata, NativeFrameFormat};
    use std::sync::mpsc;
    use std::time::Duration;

    fn task() -> BuildTask {
        BuildTask {
            epoch: 1,
            workspace_id: "w".into(),
            session_id: "s".into(),
            build_id: "b".into(),
            generation: 1,
            started_at: 1,
        }
    }

    fn candidate(sequence: u64) -> Candidate {
        Candidate {
            frame: BinaryFrame {
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
            },
            context: (2, 3),
            navigation_revision: Some(3),
            observing_at_receive: true,
        }
    }

    fn receive(validator: &Validator) -> Validated {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(result) = validator.take() {
                return result;
            }
            assert!(Instant::now() < deadline, "validator did not return");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn slow_validation_retains_only_latest_pending_candidate_and_preserves_context() {
        let (started, started_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let validator = Validator::with_decoder(move |frame| {
            started.send(frame.metadata.sequence).unwrap();
            release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            Ok(())
        });
        validator.submit(candidate(1));
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(3)).unwrap(), 1);
        for sequence in 2..=100 {
            validator.submit(candidate(sequence));
        }
        assert_eq!(validator.metrics().replaced, 98);
        assert!(validator.take().is_none());
        release.send(()).unwrap();
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            100
        );
        let first = receive(&validator);
        assert_eq!(first.candidate.frame.metadata.sequence, 1);
        assert!(first.candidate.matches(&task(), (2, 3), (360, 640)));
        assert!(
            !first.candidate.matches(&task(), (3, 3), (360, 640)),
            "configuration changed during decode"
        );
        assert!(
            !first.candidate.matches(&task(), (2, 4), (360, 640)),
            "router changed during decode"
        );
        assert!(
            !first.candidate.matches(&task(), (2, 3), (640, 360)),
            "surface changed during decode"
        );
        let mut newer = task();
        newer.generation += 1;
        assert!(!first.candidate.matches(&newer, (2, 3), (360, 640)));
        release.send(()).unwrap();
        let latest = receive(&validator);
        assert_eq!(latest.candidate.frame.metadata.sequence, 100);
        assert_eq!(latest.candidate.navigation_revision, Some(3));
        assert_eq!(validator.metrics().validated, 2);
    }

    #[test]
    fn complete_jpeg_validation_still_rejects_header_only_payload() {
        let validator = Validator::new();
        validator.submit(candidate(1));
        assert!(receive(&validator)
            .result
            .unwrap_err()
            .starts_with("FRAME_DECODE:"));
    }

    #[test]
    fn a_success_cannot_erase_current_decode_error_but_new_context_replaces_it() {
        let validator = Validator::with_decoder(|frame| {
            if frame.metadata.sequence == 1 {
                Err("fixture decode error".into())
            } else {
                Ok(())
            }
        });
        let wait_for = |count| {
            let deadline = Instant::now() + Duration::from_secs(3);
            while validator.metrics().validated < count {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(1));
            }
        };
        validator.submit(candidate(1));
        wait_for(1);
        validator.submit(candidate(2));
        wait_for(2);
        assert!(validator
            .state
            .0
            .lock()
            .unwrap()
            .completed
            .as_ref()
            .unwrap()
            .result
            .is_err());
        let mut next = candidate(3);
        next.context.0 += 1;
        validator.submit(next);
        wait_for(3);
        let latest = receive(&validator);
        assert_eq!(latest.candidate.frame.metadata.sequence, 3);
        assert!(latest.result.is_ok());
    }
}
