use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::error::DesktopError;
use crate::frame::CapturedFrame;

enum SlotState {
    Empty,
    Frame(CapturedFrame),
    Failed(DesktopError),
}

pub(crate) struct LatestFrame {
    state: Mutex<SlotState>,
    ready: Condvar,
}

impl LatestFrame {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(SlotState::Empty),
            ready: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn publish(&self, frame: CapturedFrame) {
        let mut state = self.lock();
        if !matches!(*state, SlotState::Failed(_)) {
            *state = SlotState::Frame(frame);
            self.ready.notify_all();
        }
    }

    pub(crate) fn fail(&self, error: DesktopError) {
        *self.lock() = SlotState::Failed(error);
        self.ready.notify_all();
    }

    pub(crate) fn take(&self, timeout: Duration) -> Result<Option<CapturedFrame>, DesktopError> {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            match std::mem::replace(&mut *state, SlotState::Empty) {
                SlotState::Frame(frame) => return Ok(Some(frame)),
                SlotState::Failed(error) => {
                    *state = SlotState::Failed(error.clone());
                    return Err(error);
                }
                SlotState::Empty => {}
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            state = self
                .ready
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::BGRA_BYTES;
    use std::sync::Arc;

    fn frame(width: u32) -> CapturedFrame {
        CapturedFrame {
            width,
            height: 1,
            bgra: vec![0; width as usize * BGRA_BYTES],
        }
    }

    #[test]
    fn take_returns_none_after_timeout_when_nothing_was_published() {
        let slot = LatestFrame::new();
        assert_eq!(slot.take(Duration::from_millis(20)).unwrap(), None);
    }

    #[test]
    fn newest_frame_replaces_unconsumed_older_frame() {
        let slot = LatestFrame::new();
        slot.publish(frame(1));
        slot.publish(frame(2));
        assert_eq!(slot.take(Duration::ZERO).unwrap().unwrap().width, 2);
        assert_eq!(slot.take(Duration::ZERO).unwrap(), None);
    }

    #[test]
    fn waiting_consumer_wakes_when_frame_arrives() {
        let slot = Arc::new(LatestFrame::new());
        let producer = Arc::clone(&slot);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            producer.publish(frame(7));
        });
        let received = slot.take(Duration::from_secs(5)).unwrap().unwrap();
        handle.join().unwrap();
        assert_eq!(received.width, 7);
    }

    #[test]
    fn failure_is_sticky_and_later_frames_are_ignored() {
        let slot = LatestFrame::new();
        slot.fail(DesktopError::Stopped("display removed".to_owned()));
        slot.publish(frame(3));
        for _ in 0..2 {
            assert!(matches!(
                slot.take(Duration::ZERO),
                Err(DesktopError::Stopped(_))
            ));
        }
    }
}
