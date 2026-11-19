use std::process::ExitCode;
use std::time::Duration;

use stadesk_desktop::{
    CaptureOptions, InputEvent, PointerButton, ScreenCapturer, WHEEL_STEP, displays,
    usage_from_code,
};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let list = displays()?;
    let target = list
        .iter()
        .find(|display| display.primary)
        .or_else(|| list.first())
        .ok_or("no display found")?;
    let mut capturer = ScreenCapturer::start(target.id, CaptureOptions::default())?;
    let first = capturer
        .next_frame(Duration::from_secs(10))?
        .ok_or("no frame arrived")?;
    let mut input = capturer.input()?;
    input.set_size(first.width, first.height);
    println!(
        "injecting through {} on a {}x{} display",
        capturer.method(),
        first.width,
        first.height
    );
    let key_a = usage_from_code("KeyA").ok_or("KeyA is not mapped")?;
    let sequence = [
        InputEvent::PointerMove { x: 10, y: 20 },
        InputEvent::PointerMove {
            x: first.width - 10,
            y: first.height - 10,
        },
        InputEvent::PointerButton {
            button: PointerButton::Left,
            pressed: true,
        },
        InputEvent::PointerButton {
            button: PointerButton::Left,
            pressed: false,
        },
        InputEvent::Scroll {
            horizontal: 0,
            vertical: 2 * WHEEL_STEP,
        },
        InputEvent::Scroll {
            horizontal: -WHEEL_STEP,
            vertical: 0,
        },
        InputEvent::Key {
            usage: key_a,
            pressed: true,
        },
        InputEvent::Key {
            usage: key_a,
            pressed: false,
        },
    ];
    for event in sequence {
        input.inject(event)?;
    }
    std::thread::sleep(Duration::from_millis(500));
    println!("injected {} events", sequence.len());
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("input check failed: {error}");
            ExitCode::FAILURE
        }
    }
}
