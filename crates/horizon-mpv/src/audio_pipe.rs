//! Captures the decoded PCM mpv writes with `ao=pcm` (7.1, 32-bit float,
//! [`SAMPLE_RATE`]) through a named pipe (Windows) or a FIFO (Unix), and keeps
//! it in a buffer the game takes it from, in step with the picture.
//!
//! The buffer keeps the channels as mpv gave them and mixes them only when
//! they are taken: a stereo file is upmixed for surround speakers, a surround
//! file is folded down for stereo ones.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The rate mpv is told to resample to (`audio-samplerate`), and so the rate
/// of every sample the buffer gives.
pub const SAMPLE_RATE: u32 = 48_000;
/// The most audio the buffer holds, in sample frames: 5 seconds. Older
/// frames are dropped (and counted as played) when nobody takes them.
const MAX_BUFFER_SAMPLES: usize = 5 * SAMPLE_RATE as usize;
/// How far audio may lag the picture before the lagging part is dropped, in seconds.
const SYNC_TOLERANCE: f64 = 0.15;
/// How far audio may run ahead of the picture before it is held, in seconds:
/// wider than the lag side, as the game takes sound in blocks of a frame or more.
const AHEAD_TOLERANCE: f64 = 0.3;
/// Audio this far ahead of the picture is not waited for: the count is wrong
/// (a file whose audio starts late or has gaps), so the clock is set from the
/// picture instead.
const RESYNC_AHEAD: f64 = 2.0;
/// One sample frame of the pipe: 8 channels of 32-bit floats.
const FRAME_BYTES: usize = 8 * 4;
/// A sample that is not silence, for telling a surround file from stereo.
const AUDIBLE: f32 = 1e-4;
/// The synthetic subwoofer's low-pass corner when a stereo file is upmixed, in hertz.
const LFE_CUTOFF: f32 = 120.0;
/// -3 dB, the usual centre and surround weight of an up- or downmix.
const MINUS_3DB: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// How far the reader lets mpv's sound run ahead of the wall clock, in seconds.
const PACE_LEAD: f64 = 2.0;

/// Decoded audio waiting to be played, with the media time of its oldest frame.
#[derive(Default)]
pub(crate) struct AudioBuffer {
    /// 8 channels as mpv wrote them: FL, FR, FC, LFE, BL, BR, SL, SR.
    channels: [VecDeque<f32>; 8],
    /// Media seconds of the first frame written since the last [`reset`](Self::reset).
    anchor: f64,
    /// Frames removed from the front since then: taken, or dropped to catch up
    /// or for room. `anchor + consumed / rate` is the oldest frame's time.
    consumed: u64,
    /// The file has sound in other channels than FL and FR. Set once and kept
    /// until the next file, so a quiet stretch of a 5.1 film is not upmixed.
    surround: bool,
    /// State of the synthetic subwoofer's low-pass filter.
    lfe: f32,
    /// How many times the buffer was reset: the reader's pacing starts over
    /// with each, as mpv refills its own buffer after a seek or a new file.
    resets: u64,
}

impl AudioBuffer {
    /// Forgets what is waiting: the next frame mpv writes is at `seconds`.
    /// `new_file` also forgets whether the sound is surround (after `load`).
    pub fn reset(&mut self, seconds: f64, new_file: bool) {
        for ch in &mut self.channels {
            ch.clear();
        }
        self.anchor = if seconds.is_finite() {
            seconds.max(0.0)
        } else {
            0.0
        };
        self.consumed = 0;
        self.lfe = 0.0;
        self.resets += 1;
        if new_file {
            self.surround = false;
        }
    }

    /// Media seconds of the oldest frame waiting (or of the next one to come).
    pub fn front_seconds(&self) -> f64 {
        self.anchor + self.consumed as f64 / f64::from(SAMPLE_RATE)
    }

    /// Frames waiting.
    pub fn len(&self) -> usize {
        self.channels[0].len()
    }

    /// Whether the file's sound has been seen to be surround.
    #[cfg(test)]
    pub fn surround(&self) -> bool {
        self.surround
    }

    /// Pushes interleaved 7.1 frames `[FL, FR, FC, LFE, BL, BR, SL, SR, ...]`;
    /// a trailing partial frame is ignored.
    pub fn push_71_interleaved(&mut self, samples: &[f32]) {
        let (frames, _) = samples.as_chunks::<8>();
        if !self.surround {
            self.surround = frames
                .iter()
                .any(|f| f[2..].iter().any(|s| s.abs() > AUDIBLE));
        }
        for frame in frames {
            for (ch, &s) in self.channels.iter_mut().zip(frame) {
                ch.push_back(s);
            }
        }
        let over = self.len().saturating_sub(MAX_BUFFER_SAMPLES);
        self.drop_front(over);
    }

    /// Pushes interleaved stereo frames `[L, R, L, R, ...]`, as mpv writes a
    /// stereo file into 7.1: silent everywhere else.
    #[cfg(test)]
    pub fn push_stereo_interleaved(&mut self, samples: &[f32]) {
        let mut wide = Vec::with_capacity(samples.len() * 4);
        for pair in samples.as_chunks::<2>().0 {
            wide.extend_from_slice(&[pair[0], pair[1], 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        }
        self.push_71_interleaved(&wide);
    }

    /// Removes `count` frames from the front, counting them as played.
    fn drop_front(&mut self, count: usize) {
        let count = count.min(self.len());
        for ch in &mut self.channels {
            ch.drain(..count);
        }
        self.consumed += count as u64;
    }

    /// Lines the buffer up with the picture at `video_seconds`. Lagging audio
    /// is dropped; audio ahead of the picture by more than the tolerance is
    /// held: false then, and nothing should be taken this time. `loop_length`
    /// is the file's duration when it loops, so a lap's wrap is followed.
    pub fn sync(&mut self, video_seconds: f64, loop_length: Option<f64>) -> bool {
        if !video_seconds.is_finite() || video_seconds < 0.0 {
            return true;
        }
        let rate = f64::from(SAMPLE_RATE);
        if let Some(length) = loop_length.filter(|l| l.is_finite() && *l > 0.0) {
            // Audio counted past the end is the next lap's start; audio
            // "far ahead" right after the picture wrapped is the last lap's
            // tail, which lags.
            let ahead = self.front_seconds() - video_seconds;
            if ahead > length / 2.0 {
                self.anchor -= (ahead / length).round() * length;
            }
        }
        let lag = video_seconds - self.front_seconds();
        if lag > SYNC_TOLERANCE {
            self.drop_front((lag * rate).round() as usize);
            if self.front_seconds() < video_seconds - SYNC_TOLERANCE {
                // Everything waiting was too old: what comes next is now.
                self.anchor = video_seconds - self.consumed as f64 / rate;
            }
            return true;
        }
        if -lag > RESYNC_AHEAD {
            self.anchor = video_seconds - self.consumed as f64 / rate;
            return true;
        }
        -lag <= AHEAD_TOLERANCE
    }

    /// The next waiting frame, as written.
    fn pop(&mut self) -> Option<[f32; 8]> {
        let mut frame = [0.0; 8];
        for (out, ch) in frame.iter_mut().zip(&mut self.channels) {
            *out = ch.pop_front()?;
        }
        self.consumed += 1;
        Some(frame)
    }

    /// `frame` as 7.1 speakers: as written for a surround file, upmixed for a
    /// stereo one (dialogue to the centre, the difference to the surrounds,
    /// the low end of the sum to the subwoofer).
    fn speakers(&mut self, frame: [f32; 8]) -> [f32; 8] {
        if self.surround {
            return frame;
        }
        let [fl, fr, ..] = frame;
        let alpha = 1.0 - (-2.0 * std::f32::consts::PI * LFE_CUTOFF / SAMPLE_RATE as f32).exp();
        self.lfe += alpha * ((fl + fr) * 0.5 - self.lfe);
        let side = (fl - fr) * 0.6;
        [
            fl,
            fr,
            (fl + fr) * MINUS_3DB,
            self.lfe,
            side * 0.7,
            -side * 0.7,
            side,
            -side,
        ]
    }

    /// `frame` folded down to stereo: as written for a stereo file; for a
    /// surround one ITU-R BS.775 (centre and surrounds at -3 dB, no LFE),
    /// not normalized, as mpv's own downmix (`audio-normalize-downmix=no`).
    fn stereo(&self, frame: [f32; 8]) -> [f32; 2] {
        let [fl, fr, fc, _lfe, bl, br, sl, sr] = frame;
        if !self.surround {
            return [fl, fr];
        }
        let c = fc * MINUS_3DB;
        [
            (fl + c + (sl + bl) * MINUS_3DB).clamp(-1.0, 1.0),
            (fr + c + (sr + br) * MINUS_3DB).clamp(-1.0, 1.0),
        ]
    }

    /// Takes up to `max` frames, each mixed by `mix` into `N` channels.
    fn take<const N: usize>(
        &mut self,
        max: usize,
        mut mix: impl FnMut(&mut Self, [f32; 8]) -> [f32; N],
    ) -> [Vec<f32>; N] {
        let count = self.len().min(max);
        let mut out: [Vec<f32>; N] = std::array::from_fn(|_| Vec::with_capacity(count));
        for _ in 0..count {
            let Some(frame) = self.pop() else { break };
            for (buf, s) in out.iter_mut().zip(mix(self, frame)) {
                buf.push(s);
            }
        }
        out
    }

    /// Takes up to `max_samples` mono samples: the stereo fold-down's average.
    pub fn take_mono(&mut self, max_samples: usize) -> Vec<f32> {
        let [mono] = self.take(max_samples, |b, f| {
            let [l, r] = b.stereo(f);
            [(l + r) * 0.5]
        });
        mono
    }

    /// Takes up to `max_samples` stereo (left, right) samples.
    pub fn take_stereo(&mut self, max_samples: usize) -> (Vec<f32>, Vec<f32>) {
        let [l, r] = self.take(max_samples, |b, f| b.stereo(f));
        (l, r)
    }

    /// Takes up to `max_samples` 5.1 frames [FL, FR, FC, LFE, SL, SR]: each
    /// back channel goes to its side at -3 dB.
    pub fn take_51(&mut self, max_samples: usize) -> [Vec<f32>; 6] {
        self.take(max_samples, |b, f| {
            let [fl, fr, fc, lfe, bl, br, sl, sr] = b.speakers(f);
            [
                fl,
                fr,
                fc,
                lfe,
                (sl + bl * MINUS_3DB).clamp(-1.0, 1.0),
                (sr + br * MINUS_3DB).clamp(-1.0, 1.0),
            ]
        })
    }

    /// Takes up to `max_samples` 7.1 frames [FL, FR, FC, LFE, BL, BR, SL, SR].
    pub fn take_71(&mut self, max_samples: usize) -> [Vec<f32>; 8] {
        self.take(max_samples, |b, f| b.speakers(f))
    }
}

/// Splits the pipe's byte stream into whole 7.1 frames.
#[derive(Default)]
struct Frames {
    /// Bytes of a frame not complete yet.
    partial: Vec<u8>,
    /// Whole frames' samples, reused between reads.
    samples: Vec<f32>,
}

impl Frames {
    /// Adds `bytes` read from the pipe and pushes every whole frame into
    /// `buffer`: how many frames that was, and the buffer's reset count.
    fn feed(&mut self, bytes: &[u8], buffer: &Mutex<AudioBuffer>) -> (usize, u64) {
        self.partial.extend_from_slice(bytes);
        let whole = self.partial.len() / FRAME_BYTES * FRAME_BYTES;
        if whole == 0 {
            return (0, 0);
        }
        self.samples.clear();
        // Bytes have no alignment: read each float by value, never through a cast pointer.
        self.samples.extend(
            self.partial[..whole]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&b| f32::from_ne_bytes(b)),
        );
        self.partial.drain(..whole);
        let mut lock = buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lock.push_71_interleaved(&self.samples);
        (whole / FRAME_BYTES, lock.resets)
    }
}

/// Holds mpv's sound to real time. `ao=pcm` has no clock of its own: mpv
/// writes as fast as the pipe takes it (a whole file in a fraction of a
/// second), and its playback clock follows. Reading no faster than real time,
/// plus [`PACE_LEAD`], makes mpv's writes block and so paces its playback.
#[derive(Default)]
struct Pacer {
    /// When the current run of sound started, and how many frames it has had.
    start: Option<Instant>,
    frames: u64,
    /// The buffer's reset count the run belongs to.
    resets: u64,
}

impl Pacer {
    /// How long to wait after reading `frames` more at `now`. A gap in the
    /// sound (pause, seek, a new file) starts a new run rather than letting
    /// the next sound race to make up for it.
    fn delay(&mut self, frames: usize, now: Instant) -> Duration {
        let rate = f64::from(SAMPLE_RATE);
        let mut start = *self.start.get_or_insert(now);
        if now.duration_since(start).as_secs_f64() - self.frames as f64 / rate > PACE_LEAD {
            start = now;
            self.start = Some(now);
            self.frames = 0;
        }
        self.frames += frames as u64;
        let ahead = self.frames as f64 / rate - now.duration_since(start).as_secs_f64();
        Duration::from_secs_f64((ahead - PACE_LEAD).max(0.0))
    }

    /// Reads `frames` more, waiting as long as they are ahead. A reset of
    /// the buffer (seek, new file) starts a new run: mpv refills at once then.
    fn pace(&mut self, (frames, resets): (usize, u64)) {
        if frames == 0 {
            return;
        }
        if resets != self.resets {
            self.resets = resets;
            self.start = None;
            self.frames = 0;
        }
        let wait = self.delay(frames, Instant::now());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }
}

static PIPE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// The pipe mpv writes its sound into and the thread that reads it. Dropping
/// it never waits for the thread: it ends once mpv lets go of the pipe.
pub(crate) struct AudioPipe {
    /// What mpv is given as `ao-pcm-file`.
    pub path: String,
    pub buffer: Arc<Mutex<AudioBuffer>>,
    stop: Arc<AtomicBool>,
}

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;

    pub const PIPE_ACCESS_INBOUND: u32 = 0x00000001;
    pub const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x00080000;
    pub const PIPE_TYPE_BYTE: u32 = 0x00000000;
    pub const PIPE_READMODE_BYTE: u32 = 0x00000000;
    pub const PIPE_WAIT: u32 = 0x00000000;
    pub const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x00000008;
    pub const PIPE_UNLIMITED_INSTANCES: u32 = 255;

    pub const GENERIC_WRITE: u32 = 0x40000000;
    pub const OPEN_EXISTING: u32 = 3;

    unsafe extern "system" {
        pub fn CreateNamedPipeW(
            lpName: *const u16,
            dwOpenMode: u32,
            dwPipeMode: u32,
            nMaxInstances: u32,
            nOutBufferSize: u32,
            nInBufferSize: u32,
            nDefaultTimeOut: u32,
            lpSecurityAttributes: *mut c_void,
        ) -> *mut c_void;

        pub fn ConnectNamedPipe(hNamedPipe: *mut c_void, lpOverlapped: *mut c_void) -> i32;
        pub fn DisconnectNamedPipe(hNamedPipe: *mut c_void) -> i32;

        pub fn ReadFile(
            hFile: *mut c_void,
            lpBuffer: *mut c_void,
            nNumberOfBytesToRead: u32,
            lpNumberOfBytesRead: *mut u32,
            lpOverlapped: *mut c_void,
        ) -> i32;

        pub fn CreateFileW(
            lpFileName: *const u16,
            dwDesiredAccess: u32,
            dwShareMode: u32,
            lpSecurityAttributes: *mut c_void,
            dwCreationDisposition: u32,
            dwFlagsAndAttributes: u32,
            hTemplateFile: *mut c_void,
        ) -> *mut c_void;

        pub fn CloseHandle(hObject: *mut c_void) -> i32;
    }

    /// A handle Windows returns on failure.
    pub fn valid(handle: *mut c_void) -> bool {
        !handle.is_null() && handle != -1isize as *mut c_void
    }

    /// One more instance of pipe `name`, waiting for a client.
    pub fn instance(name: &[u16], first: bool) -> Option<*mut c_void> {
        let first = if first {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            0
        };
        // SAFETY: `name` is NUL-terminated UTF-16.
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_INBOUND | first,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                65536,
                65536,
                0,
                std::ptr::null_mut(),
            )
        };
        valid(handle).then_some(handle)
    }
}

#[cfg(unix)]
mod unix {
    use std::ffi::{CString, c_char, c_int};

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    type Mode = u16;
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    type Mode = u32;

    /// `O_NONBLOCK`.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub const NONBLOCK: i32 = 0o4000;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub const NONBLOCK: i32 = 0x0004;

    unsafe extern "C" {
        fn mkfifo(path: *const c_char, mode: Mode) -> c_int;
    }

    /// Makes FIFO `path`, readable and writable by this user only.
    pub fn make_fifo(path: &str) -> std::io::Result<()> {
        let path = CString::new(path).map_err(std::io::Error::other)?;
        // SAFETY: `path` is NUL-terminated.
        if unsafe { mkfifo(path.as_ptr(), 0o600) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

/// Reads one connection of mpv's until mpv closes it, then lets it go.
#[cfg(windows)]
fn read_connection(handle: *mut std::ffi::c_void, buffer: &Mutex<AudioBuffer>) {
    let mut frames = Frames::default();
    let mut pacer = Pacer::default();
    let mut buf = vec![0u8; 8192];
    loop {
        let mut read: u32 = 0;
        // SAFETY: `handle` is this thread's own connection; `buf` holds `buf.len()` bytes.
        let ok = unsafe {
            win32::ReadFile(
                handle,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || read == 0 {
            break;
        }
        pacer.pace(frames.feed(&buf[..read as usize], buffer));
    }
    // SAFETY: owned, not used again.
    unsafe {
        win32::DisconnectNamedPipe(handle);
        win32::CloseHandle(handle);
    }
}

impl AudioPipe {
    /// Makes the pipe and starts the thread that reads it.
    pub fn new() -> Result<Self, crate::Error> {
        let id = PIPE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let buffer = Arc::new(Mutex::new(AudioBuffer::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_buffer = buffer.clone();
        let thread_stop = stop.clone();

        #[cfg(windows)]
        let (path, reader) = {
            let path = format!(r"\\.\pipe\horizon_mpv_audio_{}_{}", std::process::id(), id);
            let name: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            let first = win32::instance(&name, true).ok_or_else(|| {
                crate::Error::Mpv("CreateNamedPipeW", "failed to create audio pipe".into())
            })?;
            let first = first as usize;
            // Listens for mpv and gives each connection a reader of its own:
            // mpv may open its output again (a seek, a new file or track)
            // before it closes the old one, and a single reader still waiting
            // on the old connection would leave mpv blocked on the new one.
            let reader = move || {
                let mut waiting = Some(first as *mut std::ffi::c_void);
                loop {
                    let Some(handle) = waiting.take().or_else(|| win32::instance(&name, false))
                    else {
                        log::warn!("the video's audio pipe could not be reopened");
                        return;
                    };
                    // SAFETY: `handle` is an instance this thread made and owns.
                    // A client that came first fails it with ERROR_PIPE_CONNECTED,
                    // which is still a connection: reading tells.
                    unsafe { win32::ConnectNamedPipe(handle, std::ptr::null_mut()) };
                    if thread_stop.load(Ordering::Relaxed) {
                        // SAFETY: owned, not used again.
                        unsafe { win32::CloseHandle(handle) };
                        return;
                    }
                    let handle = handle as usize;
                    let buffer = thread_buffer.clone();
                    let spawned = std::thread::Builder::new()
                        .name("horizon-mpv-audio-read".into())
                        .spawn(move || read_connection(handle as *mut std::ffi::c_void, &buffer));
                    if spawned.is_err() {
                        // SAFETY: owned, not used again.
                        unsafe { win32::CloseHandle(handle as *mut std::ffi::c_void) };
                    }
                }
            };
            (path, reader)
        };

        #[cfg(unix)]
        let (path, reader) = {
            let path = std::env::temp_dir()
                .join(format!(
                    "horizon_mpv_audio_{}_{}.fifo",
                    std::process::id(),
                    id
                ))
                .to_string_lossy()
                .into_owned();
            let _ = std::fs::remove_file(&path);
            // Without a FIFO mpv would write a plain file and fill the disk.
            unix::make_fifo(&path).map_err(|e| crate::Error::Io(path.clone().into(), e))?;
            let fifo = path.clone();
            let reader = move || {
                use std::io::Read;
                let mut pacer = Pacer::default();
                let mut buf = vec![0u8; 8192];
                while !thread_stop.load(Ordering::Relaxed) {
                    // Blocks until mpv opens its output; `Drop` opens it too, to end this.
                    let Ok(mut file) = std::fs::File::open(&fifo) else {
                        break;
                    };
                    if thread_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    // A new connection starts on a frame boundary.
                    let mut frames = Frames::default();
                    loop {
                        match file.read(&mut buf) {
                            Ok(0) => break,
                            Ok(read) => pacer.pace(frames.feed(&buf[..read], &thread_buffer)),
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(_) => break,
                        }
                    }
                }
                let _ = std::fs::remove_file(&fifo);
            };
            (path, reader)
        };

        std::thread::Builder::new()
            .name("horizon-mpv-audio".into())
            .spawn(reader)
            .map_err(|e| crate::Error::Mpv("thread", e.to_string()))?;
        Ok(Self { path, buffer, stop })
    }
}

impl Drop for AudioPipe {
    /// Tells the reader to stop and wakes it if it waits for mpv to connect.
    /// Never joins: a reader in the middle of a read ends when mpv, torn down
    /// on the player's thread, closes its end.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);

        #[cfg(windows)]
        {
            let wide: Vec<u16> = self.path.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: `wide` is NUL-terminated; the handle is closed at once.
            unsafe {
                let client = win32::CreateFileW(
                    wide.as_ptr(),
                    win32::GENERIC_WRITE,
                    0,
                    std::ptr::null_mut(),
                    win32::OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                );
                if win32::valid(client) {
                    win32::CloseHandle(client);
                }
            }
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Non-blocking: succeeds only when the reader waits in `open`,
            // which it then leaves; never waits itself.
            let _ = std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(unix::NONBLOCK)
                .open(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_1_SQRT_2;
    use std::sync::Mutex;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn closef(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    fn stereo_seconds(seconds: f64) -> Vec<f32> {
        let frames = (seconds * 48_000.0).round() as usize;
        let mut out = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            out.push(0.1);
            out.push(0.2);
        }
        out
    }

    #[test]
    fn seek_away_from_zero_keeps_sound() {
        let mut buf = AudioBuffer::default();
        buf.reset(60.0, false);
        buf.push_stereo_interleaved(&stereo_seconds(1.0));
        assert!(buf.sync(60.0, None));
        assert_eq!(buf.len(), 48_000);

        for i in 0..20 {
            let t = 60.0 + 0.1 * i as f64;
            buf.push_stereo_interleaved(&stereo_seconds(0.1));
            assert!(buf.sync(t, None));
            let (l, _) = buf.take_stereo(4800);
            assert!(!l.is_empty());
        }
    }

    #[test]
    fn lagging_audio_is_dropped() {
        let mut buf = AudioBuffer::default();
        buf.reset(0.0, true);
        buf.push_stereo_interleaved(&stereo_seconds(1.0));
        assert!(buf.sync(0.3, None));
        assert_eq!(buf.len(), 48_000 - 14_400);
        assert!(close(buf.front_seconds(), 0.3));
    }

    #[test]
    fn audio_ahead_is_held() {
        let mut buf = AudioBuffer::default();
        buf.reset(10.0, false);
        buf.push_stereo_interleaved(&stereo_seconds(0.5));
        assert!(!buf.sync(9.5, None));
        assert_eq!(buf.len(), 24_000);
        assert!(buf.sync(9.9, None));
    }

    #[test]
    fn audio_far_ahead_follows_the_picture() {
        let mut buf = AudioBuffer::default();
        buf.reset(10.0, false);
        buf.push_stereo_interleaved(&stereo_seconds(0.5));
        assert!(buf.sync(5.0, None));
        assert!(close(buf.front_seconds(), 5.0));
    }

    #[test]
    fn lag_past_the_buffer_reanchors() {
        let mut buf = AudioBuffer::default();
        buf.reset(0.0, true);
        buf.push_stereo_interleaved(&stereo_seconds(0.5));
        assert!(buf.sync(3.0, None));
        assert_eq!(buf.len(), 0);
        assert!(close(buf.front_seconds(), 3.0));
    }

    #[test]
    fn overflow_keeps_the_clock() {
        let mut buf = AudioBuffer::default();
        buf.reset(0.0, true);
        for _ in 0..6 {
            buf.push_stereo_interleaved(&stereo_seconds(1.0));
        }
        assert_eq!(buf.len(), MAX_BUFFER_SAMPLES);
        assert!(close(buf.front_seconds(), 1.0));
    }

    #[test]
    fn loop_wrap_is_followed() {
        let mut buf = AudioBuffer::default();
        buf.reset(0.0, true);
        for _ in 0..10 {
            buf.push_stereo_interleaved(&stereo_seconds(1.0));
            buf.take_stereo(48_000);
        }
        buf.push_stereo_interleaved(&stereo_seconds(0.05));
        buf.take_stereo(2400);
        assert!(close(buf.front_seconds(), 10.05));

        buf.push_stereo_interleaved(&stereo_seconds(1.0));
        assert!(buf.sync(0.05, Some(10.0)));
        assert!(close(buf.front_seconds(), 0.05));
    }

    #[test]
    fn loop_tail_lag_is_dropped() {
        let mut buf = AudioBuffer::default();
        buf.reset(9.9, false);
        buf.push_stereo_interleaved(&stereo_seconds(1.0));
        assert!(buf.sync(0.3, Some(10.0)));
        assert!(close(buf.front_seconds(), 0.3));
    }

    #[test]
    fn stereo_file_is_upmixed() {
        let mut buf = AudioBuffer::default();
        buf.reset(0.0, true);
        buf.push_stereo_interleaved(&[0.4, 0.2]);
        assert!(!buf.surround());
        let (l, r) = buf.take_stereo(1);
        assert_eq!(l, vec![0.4]);
        assert_eq!(r, vec![0.2]);

        buf.push_stereo_interleaved(&[0.4, 0.2]);
        let ch = buf.take_51(1);
        assert!(closef(ch[2][0], 0.6 * FRAC_1_SQRT_2));
        assert!(ch[3][0].abs() < 0.1 * 0.3);
    }

    #[test]
    fn surround_downmix_keeps_dialogue() {
        let mut buf = AudioBuffer::default();
        let frame = [0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0];
        buf.push_71_interleaved(&frame);
        buf.push_71_interleaved(&frame);
        assert!(buf.surround());

        let (l, r) = buf.take_stereo(1);
        assert!(closef(l[0], 0.5 * FRAC_1_SQRT_2));
        assert!(closef(r[0], 0.5 * FRAC_1_SQRT_2));

        let m = buf.take_mono(1);
        assert!(closef(m[0], 0.5 * FRAC_1_SQRT_2));
    }

    #[test]
    fn surround_detection_is_sticky() {
        let mut buf = AudioBuffer::default();
        buf.push_71_interleaved(&[0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0]);
        buf.push_71_interleaved(&[0.3, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        buf.take_71(1);
        let s = buf.take_71(1);
        assert_eq!(s[2][0], 0.0);
        assert_eq!(s[0][0], 0.3);

        buf.reset(0.0, false);
        assert!(buf.surround());
        buf.reset(0.0, true);
        assert!(!buf.surround());
    }

    #[test]
    fn real_71_passes_through_and_51_folds_backs() {
        let mut buf = AudioBuffer::default();
        let f = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
        buf.push_71_interleaved(&f);
        buf.push_71_interleaved(&f);

        let out71 = buf.take_71(1);
        for (ch, &expected) in out71.iter().zip(&f) {
            assert_eq!(ch[0], expected);
        }

        let out51 = buf.take_51(1);
        for (ch, &expected) in out51[..4].iter().zip(&f[..4]) {
            assert_eq!(ch[0], expected);
        }
        assert!(closef(
            out51[4][0],
            (0.7 + 0.5 * FRAC_1_SQRT_2).clamp(-1.0, 1.0)
        ));
        assert!(closef(
            out51[5][0],
            (0.8 + 0.6 * FRAC_1_SQRT_2).clamp(-1.0, 1.0)
        ));
    }

    #[test]
    fn pacer_holds_sound_to_real_time() {
        let mut pacer = Pacer::default();
        let t0 = Instant::now();
        let second = SAMPLE_RATE as usize;
        // Three seconds of sound at once may run PACE_LEAD ahead, no more.
        let wait = pacer.delay(3 * second, t0);
        assert!(
            (wait.as_secs_f64() - (3.0 - PACE_LEAD)).abs() < 1e-3,
            "{wait:?}"
        );
        // Read at the pace it plays: no wait.
        let t1 = t0 + Duration::from_secs(3);
        assert_eq!(pacer.delay(second / 10, t1), Duration::ZERO);
        // After a pause the next sound does not race to make up for it.
        let t2 = t1 + Duration::from_secs(5);
        assert_eq!(pacer.delay(second / 10, t2), Duration::ZERO);
        let wait = pacer.delay(3 * second, t2);
        assert!(
            (wait.as_secs_f64() - (3.1 - PACE_LEAD)).abs() < 1e-3,
            "{wait:?}"
        );
    }

    #[test]
    fn frames_split_unaligned_reads() {
        let bytes: Vec<u8> = (0..16)
            .flat_map(|i| ((i as f32) * 0.01).to_ne_bytes())
            .collect();
        let buf = Mutex::new(AudioBuffer::default());
        let mut fr = Frames::default();

        fr.feed(&bytes[..5], &buf);
        fr.feed(&bytes[5..18], &buf);
        fr.feed(&bytes[18..], &buf);

        {
            let mut lock = buf.lock().unwrap();
            assert_eq!(lock.len(), 2);
            let ch = lock.take_71(2);
            for (c, chan) in ch.iter().enumerate() {
                for (k, &val) in chan.iter().enumerate() {
                    assert_eq!(val, ((k * 8 + c) as f32) * 0.01);
                }
            }
        }

        fr.feed(&bytes[..10], &buf);
        // A new connection's reader starts on a frame boundary.
        let mut fr = Frames::default();
        fr.feed(&bytes[..32], &buf);
        assert_eq!(buf.lock().unwrap().len(), 1);
    }
}
