use std::collections::BTreeSet;

use crate::error::DesktopError;
use crate::keymap;

pub const WHEEL_STEP: i32 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    PointerMove {
        x: u32,
        y: u32,
    },
    PointerButton {
        button: PointerButton,
        pressed: bool,
    },
    Scroll {
        horizontal: i32,
        vertical: i32,
    },
    Key {
        usage: u16,
        pressed: bool,
    },
}

pub(crate) trait InputBackend: Send {
    fn apply(&mut self, event: InputEvent) -> Result<(), DesktopError>;
}

pub struct InputInjector {
    backend: Box<dyn InputBackend>,
    width: u32,
    height: u32,
    keys: BTreeSet<u16>,
    buttons: BTreeSet<PointerButton>,
}

impl InputInjector {
    pub(crate) fn new(backend: Box<dyn InputBackend>, width: u32, height: u32) -> Self {
        Self {
            backend,
            width,
            height,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
        }
    }

    pub fn set_size(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    pub fn inject(&mut self, event: InputEvent) -> Result<(), DesktopError> {
        let event = match event {
            InputEvent::PointerMove { x, y } => InputEvent::PointerMove {
                x: x.min(self.width.saturating_sub(1)),
                y: y.min(self.height.saturating_sub(1)),
            },
            InputEvent::Key { usage, .. } if keymap::by_usage(usage).is_none() => {
                return Err(DesktopError::UnmappedKey(usage));
            }
            InputEvent::Key {
                usage,
                pressed: false,
            } if !self.keys.contains(&usage) => return Ok(()),
            InputEvent::PointerButton {
                button,
                pressed: false,
            } if !self.buttons.contains(&button) => return Ok(()),
            InputEvent::Scroll {
                horizontal: 0,
                vertical: 0,
            } => return Ok(()),
            other => other,
        };
        self.backend.apply(event)?;
        match event {
            InputEvent::Key { usage, pressed } => {
                if pressed {
                    self.keys.insert(usage);
                } else {
                    self.keys.remove(&usage);
                }
            }
            InputEvent::PointerButton { button, pressed } => {
                if pressed {
                    self.buttons.insert(button);
                } else {
                    self.buttons.remove(&button);
                }
            }
            InputEvent::PointerMove { .. } | InputEvent::Scroll { .. } => {}
        }
        Ok(())
    }

    pub fn release_all(&mut self) -> Result<(), DesktopError> {
        let mut first_error = None;
        for usage in std::mem::take(&mut self.keys).into_iter().rev() {
            if let Err(error) = self.backend.apply(InputEvent::Key {
                usage,
                pressed: false,
            }) {
                first_error.get_or_insert(error);
            }
        }
        for button in std::mem::take(&mut self.buttons) {
            if let Err(error) = self.backend.apply(InputEvent::PointerButton {
                button,
                pressed: false,
            }) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for InputInjector {
    fn drop(&mut self) {
        let _ = self.release_all();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub(crate) struct Recorder(pub(crate) Arc<Mutex<Vec<InputEvent>>>);

    impl InputBackend for Recorder {
        fn apply(&mut self, event: InputEvent) -> Result<(), DesktopError> {
            self.0.lock().unwrap().push(event);
            Ok(())
        }
    }

    fn injector() -> (InputInjector, Recorder) {
        let recorder = Recorder::default();
        (
            InputInjector::new(Box::new(recorder.clone()), 100, 50),
            recorder,
        )
    }

    #[test]
    fn pointer_positions_are_clamped_to_the_display() {
        let (mut injector, recorder) = injector();
        injector
            .inject(InputEvent::PointerMove { x: 500, y: 20 })
            .unwrap();
        injector
            .inject(InputEvent::PointerMove { x: 5, y: 80 })
            .unwrap();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            [
                InputEvent::PointerMove { x: 99, y: 20 },
                InputEvent::PointerMove { x: 5, y: 49 }
            ]
        );
    }

    #[test]
    fn unknown_keys_are_rejected_and_stray_releases_ignored() {
        let (mut injector, recorder) = injector();
        assert_eq!(
            injector.inject(InputEvent::Key {
                usage: 0x01,
                pressed: true
            }),
            Err(DesktopError::UnmappedKey(0x01))
        );
        injector
            .inject(InputEvent::Key {
                usage: 0x04,
                pressed: false,
            })
            .unwrap();
        injector
            .inject(InputEvent::PointerButton {
                button: PointerButton::Left,
                pressed: false,
            })
            .unwrap();
        injector
            .inject(InputEvent::Scroll {
                horizontal: 0,
                vertical: 0,
            })
            .unwrap();
        assert!(recorder.0.lock().unwrap().is_empty());
    }

    #[test]
    fn held_keys_and_buttons_are_released_when_the_injector_is_dropped() {
        let (mut injector, recorder) = injector();
        for usage in [0xe0, 0x06] {
            injector
                .inject(InputEvent::Key {
                    usage,
                    pressed: true,
                })
                .unwrap();
        }
        injector
            .inject(InputEvent::PointerButton {
                button: PointerButton::Right,
                pressed: true,
            })
            .unwrap();
        injector
            .inject(InputEvent::Key {
                usage: 0x06,
                pressed: false,
            })
            .unwrap();
        drop(injector);
        let events = recorder.0.lock().unwrap();
        assert_eq!(
            events[events.len() - 2..],
            [
                InputEvent::Key {
                    usage: 0xe0,
                    pressed: false
                },
                InputEvent::PointerButton {
                    button: PointerButton::Right,
                    pressed: false
                }
            ]
        );
        assert_eq!(events.len(), 6);
    }

    #[test]
    fn repeated_presses_are_forwarded_for_auto_repeat() {
        let (mut injector, recorder) = injector();
        for _ in 0..3 {
            injector
                .inject(InputEvent::Key {
                    usage: 0x04,
                    pressed: true,
                })
                .unwrap();
        }
        injector.release_all().unwrap();
        assert_eq!(recorder.0.lock().unwrap().len(), 4);
        injector.release_all().unwrap();
        assert_eq!(recorder.0.lock().unwrap().len(), 4);
    }
}
