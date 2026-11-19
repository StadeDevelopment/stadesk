use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use stadesk_desktop::{CaptureOptions, CapturedFrame, ScreenCapturer, displays};

const MEASURE_FOR: Duration = Duration::from_secs(10);

fn content_share(frame: &CapturedFrame) -> f64 {
    let pixels = frame.bgra.len() / 4;
    let lit = frame
        .bgra
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[0] | pixel[1] | pixel[2] != 0)
        .count();
    lit as f64 * 100.0 / pixels.max(1) as f64
}

fn write_bmp(path: &PathBuf, frame: &CapturedFrame) -> std::io::Result<()> {
    const FILE_HEADER_LEN: u32 = 14;
    const INFO_HEADER_LEN: u32 = 40;
    let pixel_bytes = frame.bgra.len() as u32;
    let offset = FILE_HEADER_LEN + INFO_HEADER_LEN;
    let mut out = Vec::with_capacity((offset + pixel_bytes) as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(offset + pixel_bytes).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&INFO_HEADER_LEN.to_le_bytes());
    out.extend_from_slice(&(frame.width as i32).to_le_bytes());
    out.extend_from_slice(&(-(frame.height as i32)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&pixel_bytes.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&frame.bgra);
    std::fs::write(path, out)
}

fn save_path() -> Result<Option<PathBuf>, String> {
    let mut arguments = std::env::args().skip(1);
    match (
        arguments.next().as_deref(),
        arguments.next(),
        arguments.next(),
    ) {
        (None, _, _) => Ok(None),
        (Some("--save"), Some(path), None) => Ok(Some(PathBuf::from(path))),
        _ => Err("usage: capture_check [--save FILE.bmp]".to_owned()),
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let save = save_path()?;
    let list = displays()?;
    for display in &list {
        println!(
            "display {} {}x{}{}",
            display.id,
            display.width,
            display.height,
            if display.primary { " (primary)" } else { "" }
        );
    }
    let target = list
        .iter()
        .find(|display| display.primary)
        .or_else(|| list.first())
        .ok_or("no display found")?;
    let mut capturer = ScreenCapturer::start(target.id, CaptureOptions::default())?;
    println!(
        "capturing display {} with {} for {} seconds, move the mouse and windows around",
        target.id,
        capturer.method(),
        MEASURE_FOR.as_secs()
    );
    let started = Instant::now();
    let mut frames = 0u32;
    let mut last: Option<CapturedFrame> = None;
    while started.elapsed() < MEASURE_FOR {
        if let Some(frame) = capturer.next_frame(Duration::from_millis(500))? {
            frames += 1;
            last = Some(frame);
        }
    }
    let seconds = started.elapsed().as_secs_f64();
    let last = last.ok_or("no frame was delivered")?;
    println!(
        "{frames} frames of {}x{} in {seconds:.1} s ({:.1} fps), non-black pixels in last frame {:.1}%",
        last.width,
        last.height,
        f64::from(frames) / seconds,
        content_share(&last)
    );
    if let Some(path) = save {
        write_bmp(&path, &last)?;
        println!("last frame saved to {}", path.display());
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("capture check failed: {error}");
            ExitCode::FAILURE
        }
    }
}
