//! Measures what a video frame costs (docs/native-libraries.md, "Frame cost").
//!
//! `cargo run --release -p horizon-mpv --example mpv_bench -- <libmpv> [video|-] [seconds] [WxH]`
//!
//! Plays `video` (`-` or none: a generated 30 fps test file of the surface's
//! size, one second looped) on a `WxH` surface (1280x720 by default; 3840x2160
//! for a 4K slot) for `seconds` while a loop stands in for the game's frames
//! at 60 a second.
//! Each frame it does what the game's thread does for a video: takes the
//! newest frame and copies its pixels once, as staging them for the GPU does.
//! The rendering itself runs on the player's thread and is reported apart.

use horizon_mpv::{Mpv, Options};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

const FPS: u32 = 30;

/// `seconds` of moving bars, `width` x `height`, as uncompressed YUV4MPEG2 (4:2:0).
fn write_video(path: &Path, seconds: u32, width: u32, height: u32) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(
        out,
        "YUV4MPEG2 W{width} H{height} F{FPS}:1 Ip A1:1 C420jpeg"
    )?;
    let (w, h) = (width as usize, height as usize);
    let mut luma = vec![0u8; w * h];
    let chroma = vec![128u8; (w / 2) * (h / 2)];
    for frame in 0..(seconds * FPS) as usize {
        for (i, pixel) in luma.iter_mut().enumerate() {
            *pixel = (((i % w) + frame * 8) / 4 % 220) as u8 + 16;
        }
        out.write_all(b"FRAME\n")?;
        out.write_all(&luma)?;
        out.write_all(&chroma)?;
        out.write_all(&chroma)?;
    }
    out.flush()
}

fn percentile(sorted: &[u64], percent: usize) -> u64 {
    sorted[(sorted.len() * percent / 100).min(sorted.len() - 1)]
}

fn report(what: &str, mut micros: Vec<u64>) {
    if micros.is_empty() {
        println!("{what}: nothing measured");
        return;
    }
    micros.sort_unstable();
    let mean = micros.iter().sum::<u64>() as f64 / micros.len() as f64;
    println!(
        "{what}: mean {:.3} ms, median {:.3} ms, 99% {:.3} ms, worst {:.3} ms ({} samples)",
        mean / 1000.0,
        percentile(&micros, 50) as f64 / 1000.0,
        percentile(&micros, 99) as f64 / 1000.0,
        micros[micros.len() - 1] as f64 / 1000.0,
        micros.len()
    );
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(library) = args.first() else {
        eprintln!("usage: mpv_bench <full path to libmpv> [video|-] [seconds] [WxH]");
        return ExitCode::from(2);
    };
    let seconds: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
    let size = args.get(3).map_or(Some((1280, 720)), |s| {
        let (w, h) = s.split_once('x')?;
        Some((w.parse().ok()?, h.parse().ok()?))
    });
    let Some((width, height)) = size else {
        eprintln!("the size is WxH, such as 3840x2160");
        return ExitCode::from(2);
    };
    let generated =
        std::env::temp_dir().join(format!("horizon-mpv-bench-{}.y4m", std::process::id()));
    let video = match args.get(1).filter(|a| *a != "-") {
        Some(path) => PathBuf::from(path),
        None => {
            // One looped second: uncompressed 4K is 12 MB a frame.
            if let Err(e) = write_video(&generated, 1, width, height) {
                eprintln!("{}: {e}", generated.display());
                return ExitCode::FAILURE;
            }
            generated.clone()
        }
    };
    let run = || -> Result<(), horizon_mpv::Error> {
        let mpv = Mpv::open(Path::new(library))?;
        let options = Options {
            looped: true,
            muted: true,
            ..Options::default()
        };
        let player = mpv.player(width, height, &options)?;
        player.load(&video)?;

        let frame_time = Duration::from_micros(16_667);
        let mut staging = vec![0u8; (width * height * 4) as usize];
        let (mut with_video, mut without, mut render) = (Vec::new(), Vec::new(), Vec::new());
        let mut last_frames = 0;
        let started = Instant::now();
        let mut next = started;
        while started.elapsed() < Duration::from_secs(u64::from(seconds)) {
            let frame_started = Instant::now();
            let frame = player.take_frame();
            let taken = frame.is_some();
            if let Some(frame) = frame {
                // A smaller video is rendered at its own size, not stretched.
                let len = frame.rgba.len().min(staging.len());
                staging[..len].copy_from_slice(&frame.rgba[..len]);
            }
            std::hint::black_box(&staging);
            let cost = frame_started.elapsed().as_micros() as u64;
            if taken { &mut with_video } else { &mut without }.push(cost);
            let stats = player.stats();
            if stats.frames != last_frames {
                last_frames = stats.frames;
                render.push(stats.render_micros);
            }
            next += frame_time;
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
        let elapsed = started.elapsed().as_secs_f64();
        println!(
            "{width}x{height}, {} frames rendered in {elapsed:.1} s ({:.1} a second), video {:?}",
            last_frames,
            last_frames as f64 / elapsed,
            player.video_size()
        );
        report("game thread, a frame with a new video frame", with_video);
        report("game thread, a frame without one", without);
        report("video thread, rendering a frame", render);
        player.close();
        Ok(())
    };
    let result = run();
    let _ = std::fs::remove_file(&generated);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
