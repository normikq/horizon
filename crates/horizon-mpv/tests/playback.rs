//! Plays a generated test file through a real libmpv.
//!
//! Needs `HORIZON_MPV_LIBRARY`, the full path of a libmpv (client API 2);
//! without it the tests pass without playing anything, and say so. The video
//! is written here as uncompressed YUV4MPEG2, so no file is kept in the tree.

use horizon_mpv::{Mpv, Options};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;
const FPS: u32 = 30;

fn library() -> Option<PathBuf> {
    match std::env::var_os("HORIZON_MPV_LIBRARY") {
        Some(path) => Some(PathBuf::from(path)),
        None => {
            eprintln!("skipped: set HORIZON_MPV_LIBRARY to the full path of a libmpv");
            None
        }
    }
}

/// The left half red, the right half blue, as full-range sRGB bytes.
const LEFT: [u8; 3] = [200, 30, 30];
const RIGHT: [u8; 3] = [30, 30, 200];

/// BT.601 limited-range Y'CbCr of an sRGB colour, as SD video carries it.
fn ycbcr([r, g, b]: [u8; 3]) -> [u8; 3] {
    let (r, g, b) = (f64::from(r), f64::from(g), f64::from(b));
    let y = 16.0 + (65.481 * r + 128.553 * g + 24.966 * b) / 255.0;
    let cb = 128.0 + (-37.797 * r - 74.203 * g + 112.0 * b) / 255.0;
    let cr = 128.0 + (112.0 * r - 93.786 * g - 18.214 * b) / 255.0;
    [y.round() as u8, cb.round() as u8, cr.round() as u8]
}

/// `frames` frames of the two-colour picture as a YUV4MPEG2 file (4:4:4).
fn write_video(path: &Path, frames: u32) {
    let mut out =
        format!("YUV4MPEG2 W{WIDTH} H{HEIGHT} F{FPS}:1 Ip A1:1 C444 XCOLORRANGE=LIMITED\n")
            .into_bytes();
    let (left, right) = (ycbcr(LEFT), ycbcr(RIGHT));
    let mut frame = Vec::new();
    for plane in 0..3 {
        for _ in 0..HEIGHT {
            for x in 0..WIDTH {
                frame.push(if x < WIDTH / 2 { left } else { right }[plane]);
            }
        }
    }
    for _ in 0..frames {
        out.extend_from_slice(b"FRAME\n");
        out.extend_from_slice(&frame);
    }
    std::fs::write(path, out).unwrap();
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("horizon-mpv-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Waits until `done` or fails after `secs`: a test never hangs on mpv.
fn wait(what: &str, secs: u64, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn near(pixel: &[u8], want: [u8; 3]) -> bool {
    pixel[..3]
        .iter()
        .zip(want)
        .all(|(got, want)| got.abs_diff(want) <= 6)
}

#[test]
fn the_library_reports_client_api_2() {
    let Some(library) = library() else { return };
    let mpv = Mpv::open(&library).unwrap();
    let (major, minor) = mpv.client_api_version();
    println!("mpv_client_api_version: {major}.{minor}");
    assert_eq!(major, 2);
}

#[test]
fn a_file_plays_into_frames_of_the_right_colours() {
    let Some(library) = library() else { return };
    let dir = scratch("play");
    let file = dir.join("bars.y4m");
    write_video(&file, FPS * 2);

    let mpv = Mpv::open(&library).unwrap();
    let options = Options {
        muted: true,
        ..Options::default()
    };
    let player = mpv.player(WIDTH, HEIGHT, &options).unwrap();
    player.load(&file).unwrap();

    let mut frame = None;
    wait("the first frame", 20, || {
        frame = player.take_frame();
        frame.is_some()
    });
    let frame = frame.unwrap();
    assert_eq!((frame.width, frame.height), (WIDTH, HEIGHT));
    let frame = frame.rgba;
    assert_eq!(frame.len(), (WIDTH * HEIGHT * 4) as usize);
    let at = |x: u32, y: u32| {
        let i = ((y * WIDTH + x) * 4) as usize;
        &frame[i..i + 4]
    };
    for y in [10, HEIGHT / 2, HEIGHT - 10] {
        let (left, right) = (at(WIDTH / 4, y), at(WIDTH * 3 / 4, y));
        assert!(near(left, LEFT), "left is {left:?}");
        assert!(near(right, RIGHT), "right is {right:?}");
        assert_eq!((left[3], right[3]), (255, 255), "frames are opaque");
    }
    wait("the file's facts", 20, || {
        player.loaded() && player.duration().is_some() && player.video_size().is_some()
    });
    assert_eq!(player.video_size(), Some((WIDTH, HEIGHT)));
    assert!((player.duration().unwrap() - 2.0).abs() < 0.2);

    // Pause, seek and resume, each seen in mpv's own state.
    player.set_paused(true).unwrap();
    wait("the pause", 20, || player.paused());
    player.seek(1.0).unwrap();
    wait("the seek", 20, || {
        player.position().is_some_and(|p| (p - 1.0).abs() < 0.2)
    });
    player.set_paused(false).unwrap();
    wait("the end", 20, || player.ended());
    assert!(!player.failed());
    let stats = player.stats();
    assert!(stats.frames >= 10, "{stats:?}");
    println!(
        "{} frames, the last rendered in {} us",
        stats.frames, stats.render_micros
    );
    player.close();
    let _ = std::fs::remove_dir_all(&dir);
}

/// `seconds` of a 440 Hz tone in both channels, as a 48 kHz 16-bit WAV file.
fn write_tone(path: &Path, seconds: u32) {
    let rate = 48_000u32;
    let frames = rate * seconds;
    let data = frames * 4;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let t = f64::from(i) / f64::from(rate);
        let s = ((t * 440.0 * std::f64::consts::TAU).sin() * 16_000.0) as i16;
        out.extend_from_slice(&s.to_le_bytes());
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
}

/// Audible samples taken over `secs` of playback, taking as a game frame does.
fn audible_for(player: &horizon_mpv::Player, secs: f64) -> usize {
    let deadline = Instant::now() + Duration::from_secs_f64(secs);
    let mut audible = 0;
    while Instant::now() < deadline {
        if let Some((left, _)) = player.take_audio_stereo(4096) {
            audible += left.iter().filter(|s| s.abs() > 0.05).count();
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    audible
}

#[test]
fn captured_sound_survives_seeks_and_a_new_file() {
    let Some(library) = library() else { return };
    let dir = scratch("audio");
    let file = dir.join("tone.wav");
    write_tone(&file, 8);

    let mpv = Mpv::open(&library).unwrap();
    let options = Options {
        capture_audio: true,
        ..Options::default()
    };
    let player = mpv.player(64, 64, &options).unwrap();
    assert_eq!(player.audio_sample_rate(), Some(horizon_mpv::SAMPLE_RATE));
    player.load(&file).unwrap();
    wait("the file", 20, || player.loaded());
    // A tone at 48 kHz: most of what plays in half a second is audible.
    let rate = horizon_mpv::SAMPLE_RATE as usize;
    assert!(
        audible_for(&player, 1.0) > rate / 4,
        "no sound from the start"
    );

    // Away from the start, forward and back: the sound must keep coming.
    for to in [5.0, 2.0] {
        player.seek(to).unwrap();
        let heard = audible_for(&player, 1.0);
        assert!(heard > rate / 4, "no sound after seeking to {to}: {heard}");
    }

    // The same file again, from its start.
    player.load(&file).unwrap();
    wait("the file again", 20, || player.loaded());
    assert!(
        audible_for(&player, 1.0) > rate / 4,
        "no sound after a new load"
    );
    player.close();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_is_no_video_fails_without_hanging() {
    let Some(library) = library() else { return };
    let dir = scratch("bad");
    let file = dir.join("not-a-video.mkv");
    std::fs::write(&file, b"this is not a video").unwrap();
    let mpv = Mpv::open(&library).unwrap();
    let player = mpv
        .player(
            64,
            64,
            &Options {
                muted: true,
                ..Options::default()
            },
        )
        .unwrap();
    player.load(&file).unwrap();
    wait("the failure", 20, || player.failed());
    player.close();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_playlist_cannot_make_mpv_open_an_address() {
    let Some(library) = library() else { return };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let dir = scratch("playlist");
    let file = dir.join("list.m3u");
    std::fs::write(&file, format!("#EXTM3U\nhttp://127.0.0.1:{port}/a.mkv\n")).unwrap();

    let mpv = Mpv::open(&library).unwrap();
    let player = mpv
        .player(
            64,
            64,
            &Options {
                muted: true,
                ..Options::default()
            },
        )
        .unwrap();
    player.load(&file).unwrap();
    wait("the refusal", 20, || player.failed());
    // Nothing connected while mpv had the playlist, nor shortly after.
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        listener.accept().is_err(),
        "mpv opened a connection for a playlist entry"
    );
    player.close();
    let _ = std::fs::remove_dir_all(&dir);
}
