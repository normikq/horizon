//! Captures decoded raw PCM audio samples from mpv via a local pipe/stream
//! and stores them in a thread-safe ring buffer with A/V sync protection.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Maximum audio buffer capacity in samples (e.g. 5 seconds at 48000 Hz = 240,000 samples).
const MAX_BUFFER_SAMPLES: usize = 240_000;

/// A thread-safe ring buffer holding decoded PCM audio samples.
#[derive(Default)]
pub(crate) struct AudioBuffer {
    /// 8 discrete channels:
    /// 0: FL (Front Left)
    /// 1: FR (Front Right)
    /// 2: FC (Front Center)
    /// 3: LFE (Subwoofer)
    /// 4: BL (Back Left)
    /// 5: BR (Back Right)
    /// 6: SL (Side Left)
    /// 7: SR (Side Right)
    pub channels: [VecDeque<f32>; 8],
    /// Total samples read from this buffer so far.
    pub samples_read: u64,
    /// Total samples pushed to this buffer so far.
    pub samples_written: u64,
}

impl AudioBuffer {
    /// Pushes interleaved 7.1 32-bit float samples `[FL, FR, FC, LFE, BL, BR, SL, SR, ...]`.
    /// If channels 2..7 are silent (stereo source), performs Dolby Pro Logic II style
    /// matrix upmixing so dialogue is centered and ambient surround is directed to side/back speakers.
    pub fn push_71_interleaved(&mut self, samples: &[f32]) {
        let frames = samples.len() / 8;
        if frames == 0 {
            return;
        }

        // Check if channels 2..7 have any audio content (or if it's stereo upmixed to 7.1 with silent surrounds)
        let mut has_surround = false;
        for i in 0..frames {
            for ch in 2..8 {
                if samples[i * 8 + ch].abs() > 1e-4 {
                    has_surround = true;
                    break;
                }
            }
            if has_surround {
                break;
            }
        }

        for i in 0..frames {
            let base = i * 8;
            let fl = samples[base];
            let fr = samples[base + 1];

            let (fc, lfe, bl, br, sl, sr) = if has_surround {
                (
                    samples[base + 2],
                    samples[base + 3],
                    samples[base + 4],
                    samples[base + 5],
                    samples[base + 6],
                    samples[base + 7],
                )
            } else {
                // Dolby Pro Logic II cinema matrix upmix for stereo sources:
                // Dialogue extraction for center:
                let fc = (fl + fr) * 0.7071;
                // Low-pass / bass energy for subwoofer:
                let lfe = (fl + fr) * 0.5;
                // Side surrounds: phase difference (L - R)
                let diff = (fl - fr) * 0.6;
                let diff_r = (fr - fl) * 0.6;
                let sl = diff;
                let sr = diff_r;
                // Back surrounds: slightly reduced phase difference
                let bl = diff * 0.7;
                let br = diff_r * 0.7;
                (fc, lfe, bl, br, sl, sr)
            };

            self.channels[0].push_back(fl);
            self.channels[1].push_back(fr);
            self.channels[2].push_back(fc);
            self.channels[3].push_back(lfe);
            self.channels[4].push_back(bl);
            self.channels[5].push_back(br);
            self.channels[6].push_back(sl);
            self.channels[7].push_back(sr);
        }
        self.samples_written += frames as u64;

        // Discard overflow if caller isn't reading fast enough
        while self.channels[0].len() > MAX_BUFFER_SAMPLES {
            for ch in &mut self.channels {
                ch.pop_front();
            }
        }
    }

    /// Pushes interleaved stereo 32-bit float samples `[L, R, L, R, ...]`.
    #[allow(dead_code)]
    pub fn push_stereo_interleaved(&mut self, samples: &[f32]) {
        let pairs = samples.len() / 2;
        for i in 0..pairs {
            let fl = samples[i * 2];
            let fr = samples[i * 2 + 1];
            let fc = (fl + fr) * 0.7071;
            let lfe = (fl + fr) * 0.5;
            let sl = (fl - fr) * 0.6;
            let sr = (fr - fl) * 0.6;
            let bl = (fl - fr) * 0.42;
            let br = (fr - fl) * 0.42;

            self.channels[0].push_back(fl);
            self.channels[1].push_back(fr);
            self.channels[2].push_back(fc);
            self.channels[3].push_back(lfe);
            self.channels[4].push_back(bl);
            self.channels[5].push_back(br);
            self.channels[6].push_back(sl);
            self.channels[7].push_back(sr);
        }
        self.samples_written += pairs as u64;

        while self.channels[0].len() > MAX_BUFFER_SAMPLES {
            for ch in &mut self.channels {
                ch.pop_front();
            }
        }
    }

    /// Takes up to `max_samples` mono samples.
    pub fn take_mono(&mut self, max_samples: usize) -> Vec<f32> {
        let count = self.channels[0].len().min(max_samples);
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let fl = self.channels[0].pop_front().unwrap_or(0.0);
            let fr = self.channels[1].pop_front().unwrap_or(0.0);
            for ch in 2..8 {
                self.channels[ch].pop_front();
            }
            out.push((fl + fr) * 0.5);
        }
        self.samples_read += count as u64;
        out
    }

    /// Takes up to `max_samples` stereo (left, right) samples.
    pub fn take_stereo(&mut self, max_samples: usize) -> (Vec<f32>, Vec<f32>) {
        let count = self.channels[0].len().min(max_samples);
        let mut out_l = Vec::with_capacity(count);
        let mut out_r = Vec::with_capacity(count);
        for _ in 0..count {
            let fl = self.channels[0].pop_front().unwrap_or(0.0);
            let fr = self.channels[1].pop_front().unwrap_or(0.0);
            for ch in 2..8 {
                self.channels[ch].pop_front();
            }
            out_l.push(fl);
            out_r.push(fr);
        }
        self.samples_read += count as u64;
        (out_l, out_r)
    }

    /// Takes up to `max_samples` 5.1 surround channels [FL, FR, FC, LFE, SL, SR].
    pub fn take_51(&mut self, max_samples: usize) -> [Vec<f32>; 6] {
        let count = self.channels[0].len().min(max_samples);
        let mut out = [
            Vec::with_capacity(count),
            Vec::with_capacity(count),
            Vec::with_capacity(count),
            Vec::with_capacity(count),
            Vec::with_capacity(count),
            Vec::with_capacity(count),
        ];
        for _ in 0..count {
            let fl = self.channels[0].pop_front().unwrap_or(0.0);
            let fr = self.channels[1].pop_front().unwrap_or(0.0);
            let fc = self.channels[2].pop_front().unwrap_or(0.0);
            let lfe = self.channels[3].pop_front().unwrap_or(0.0);
            let bl = self.channels[4].pop_front().unwrap_or(0.0);
            let br = self.channels[5].pop_front().unwrap_or(0.0);
            let sl = self.channels[6].pop_front().unwrap_or(0.0);
            let sr = self.channels[7].pop_front().unwrap_or(0.0);

            out[0].push(fl);
            out[1].push(fr);
            out[2].push(fc);
            out[3].push(lfe);
            out[4].push((sl + bl * 0.7).clamp(-1.0, 1.0));
            out[5].push((sr + br * 0.7).clamp(-1.0, 1.0));
        }
        self.samples_read += count as u64;
        out
    }

    /// Takes up to `max_samples` 7.1 surround channels [FL, FR, FC, LFE, BL, BR, SL, SR].
    pub fn take_71(&mut self, max_samples: usize) -> [Vec<f32>; 8] {
        let count = self.channels[0].len().min(max_samples);
        let mut out = [
            Vec::with_capacity(count),
            Vec::with_capacity(count),
            Vec::with_capacity(count),
            Vec::with_capacity(count),
            Vec::with_capacity(count),
            Vec::with_capacity(count),
            Vec::with_capacity(count),
            Vec::with_capacity(count),
        ];
        for _ in 0..count {
            for (ch, buf) in self.channels.iter_mut().zip(out.iter_mut()) {
                buf.push(ch.pop_front().unwrap_or(0.0));
            }
        }
        self.samples_read += count as u64;
        out
    }

    /// Clears queues and resets sample counters (e.g. after a seek).
    pub fn clear(&mut self) {
        for ch in &mut self.channels {
            ch.clear();
        }
        self.samples_read = 0;
        self.samples_written = 0;
    }

    /// Protects against A/V desync by realigning audio buffer position with video timestamp.
    /// If drift exceeds `threshold_sec` (e.g. 0.15s = 150ms), stale samples are dropped.
    pub fn resync(&mut self, video_seconds: f64, sample_rate: u32) {
        if sample_rate == 0 || video_seconds < 0.0 {
            return;
        }
        let audio_seconds = self.samples_read as f64 / sample_rate as f64;
        let diff = video_seconds - audio_seconds;

        // If audio is lagging behind video by > 150ms, drop lagging samples
        if diff > 0.15 {
            let drop_count = ((diff * sample_rate as f64) as usize).min(self.channels[0].len());
            for ch in &mut self.channels {
                for _ in 0..drop_count {
                    ch.pop_front();
                }
            }
            self.samples_read += drop_count as u64;
        }
    }
}

static PIPE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// The audio capturing pipe and worker thread.
pub(crate) struct AudioPipe {
    pub path: String,
    pub buffer: Arc<Mutex<AudioBuffer>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;

    pub const PIPE_ACCESS_INBOUND: u32 = 0x00000001;
    pub const PIPE_TYPE_BYTE: u32 = 0x00000000;
    pub const PIPE_READMODE_BYTE: u32 = 0x00000000;
    pub const PIPE_WAIT: u32 = 0x00000000;

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
}

impl AudioPipe {
    /// Creates a new audio pipe and launches background ingestion worker.
    pub fn new() -> Result<Self, crate::Error> {
        let id = PIPE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let buffer = Arc::new(Mutex::new(AudioBuffer::default()));
        let stop = Arc::new(AtomicBool::new(false));

        #[cfg(windows)]
        {
            let pipe_name = format!(r"\\.\pipe\horizon_mpv_audio_{}_{}", std::process::id(), id);
            let wide_name: Vec<u16> = pipe_name.encode_utf16().chain(std::iter::once(0)).collect();

            let handle = unsafe {
                win32::CreateNamedPipeW(
                    wide_name.as_ptr(),
                    win32::PIPE_ACCESS_INBOUND,
                    win32::PIPE_TYPE_BYTE | win32::PIPE_READMODE_BYTE | win32::PIPE_WAIT,
                    1,
                    65536,
                    65536,
                    0,
                    std::ptr::null_mut(),
                )
            };

            if handle.is_null() || handle == -1isize as *mut std::ffi::c_void {
                return Err(crate::Error::Mpv("CreateNamedPipeW", "failed to create audio pipe".into()));
            }

            let handle_addr = handle as usize;
            let thread_buffer = buffer.clone();
            let thread_stop = stop.clone();
            let thread = std::thread::Builder::new()
                .name("horizon-mpv-audio".into())
                .spawn(move || {
                    let handle = handle_addr as *mut std::ffi::c_void;
                    while !thread_stop.load(Ordering::Relaxed) {
                        let _ = unsafe { win32::ConnectNamedPipe(handle, std::ptr::null_mut()) };
                        if thread_stop.load(Ordering::Relaxed) {
                            break;
                        }

                        // Align to 32 bytes (1 frame of 8-channel 32-bit floats = 32 bytes)
                        let mut remainder = Vec::with_capacity(32);
                        let mut buf = [0u8; 8192];
                        while !thread_stop.load(Ordering::Relaxed) {
                            let mut bytes_read: u32 = 0;
                            let res = unsafe {
                                win32::ReadFile(
                                    handle,
                                    buf.as_mut_ptr().cast(),
                                    buf.len() as u32,
                                    &mut bytes_read,
                                    std::ptr::null_mut(),
                                )
                            };
                            if res == 0 || bytes_read == 0 {
                                break;
                            }
                            let total_bytes = remainder.len() + bytes_read as usize;
                            let usable_bytes = (total_bytes / 32) * 32;
                            if usable_bytes == 0 {
                                remainder.extend_from_slice(&buf[..bytes_read as usize]);
                                continue;
                            }

                            let mut packet = Vec::with_capacity(usable_bytes);
                            packet.extend_from_slice(&remainder);
                            let from_buf = usable_bytes - remainder.len();
                            packet.extend_from_slice(&buf[..from_buf]);

                            remainder.clear();
                            remainder.extend_from_slice(&buf[from_buf..bytes_read as usize]);

                            let float_count = packet.len() / 4;
                            let floats: &[f32] = unsafe {
                                std::slice::from_raw_parts(packet.as_ptr() as *const f32, float_count)
                            };
                            if let Ok(mut lock) = thread_buffer.lock() {
                                lock.push_71_interleaved(floats);
                            }
                        }
                        let _ = unsafe { win32::DisconnectNamedPipe(handle) };
                    }
                    unsafe { win32::CloseHandle(handle) };
                })
                .map_err(|e| crate::Error::Mpv("thread", e.to_string()))?;

            Ok(Self {
                path: pipe_name,
                buffer,
                stop,
                thread: Some(thread),
            })
        }

        #[cfg(not(windows))]
        {
            let pipe_name = std::env::temp_dir()
                .join(format!("horizon_mpv_audio_{}_{}.fifo", std::process::id(), id))
                .to_string_lossy()
                .into_owned();
            Ok(Self {
                path: pipe_name,
                buffer,
                stop,
                thread: None,
            })
        }
    }
}

impl Drop for AudioPipe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);

        #[cfg(windows)]
        {
            // Wake up ConnectNamedPipe by briefly opening the pipe as a client
            let wide: Vec<u16> = self.path.encode_utf16().chain(std::iter::once(0)).collect();
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
                if !client.is_null() && client != -1isize as *mut std::ffi::c_void {
                    win32::CloseHandle(client);
                }
            }
        }

        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_buffer_downmixes_and_tracks_samples() {
        let mut buf = AudioBuffer::default();
        let input = [0.2f32, 0.4f32, -0.6f32, 0.8f32];
        buf.push_stereo_interleaved(&input);

        assert_eq!(buf.samples_written, 2);
        assert_eq!(buf.channels[0].len(), 2);

        let mono = buf.take_mono(1);
        assert_eq!(mono.len(), 1);
        assert!((mono[0] - 0.3).abs() < 1e-6);
        assert_eq!(buf.samples_read, 1);

        let (left, right) = buf.take_stereo(1);
        assert_eq!(left.len(), 1);
        assert_eq!(right.len(), 1);
        assert!((left[0] - (-0.6)).abs() < 1e-6);
        assert!((right[0] - 0.8).abs() < 1e-6);
        assert_eq!(buf.samples_read, 2);
    }

    #[test]
    fn audio_buffer_surround_51_and_71() {
        let mut buf = AudioBuffer::default();
        let mut frame_71 = [0.0f32; 16]; // 2 frames of 7.1
        // Frame 0: FL=0.1, FR=0.2, FC=0.3, LFE=0.4, BL=0.5, BR=0.6, SL=0.7, SR=0.8
        frame_71[0] = 0.1;
        frame_71[1] = 0.2;
        frame_71[2] = 0.3;
        frame_71[3] = 0.4;
        frame_71[4] = 0.5;
        frame_71[5] = 0.6;
        frame_71[6] = 0.7;
        frame_71[7] = 0.8;
        // Frame 1
        frame_71[8] = -0.1;
        frame_71[9] = -0.2;
        frame_71[10] = -0.3;
        frame_71[11] = -0.4;
        frame_71[12] = -0.5;
        frame_71[13] = -0.6;
        frame_71[14] = -0.7;
        frame_71[15] = -0.8;

        buf.push_71_interleaved(&frame_71);
        assert_eq!(buf.samples_written, 2);

        let s51 = buf.take_51(1);
        assert_eq!(s51[0].len(), 1);
        assert_eq!(s51[0][0], 0.1); // FL
        assert_eq!(s51[1][0], 0.2); // FR
        assert_eq!(s51[2][0], 0.3); // FC
        assert_eq!(s51[3][0], 0.4); // LFE

        let s71 = buf.take_71(1);
        assert_eq!(s71[0].len(), 1);
        assert_eq!(s71[0][0], -0.1); // FL
        assert_eq!(s71[4][0], -0.5); // BL
        assert_eq!(s71[6][0], -0.7); // SL
    }

    #[test]
    fn audio_buffer_resync_drops_lagging_samples() {
        let mut buf = AudioBuffer::default();
        // Push 1 second of audio at 48000 Hz (48000 stereo pairs)
        let samples = vec![0.1f32; 48000 * 2];
        buf.push_stereo_interleaved(&samples);

        // Suppose caller hasn't read any samples (samples_read = 0),
        // but video timestamp is already 0.3 seconds ahead (300ms drift > 150ms threshold)
        buf.resync(0.3, 48000);

        // 0.3 * 48000 = 14400 samples should have been dropped to catch up
        assert_eq!(buf.samples_read, 14400);
        assert_eq!(buf.channels[0].len(), 48000 - 14400);
    }

    #[test]
    fn audio_buffer_clear_resets_state() {
        let mut buf = AudioBuffer::default();
        buf.push_stereo_interleaved(&[0.5, 0.5]);
        buf.take_mono(1);
        assert_eq!(buf.samples_read, 1);

        buf.clear();
        assert_eq!(buf.samples_read, 0);
        assert_eq!(buf.samples_written, 0);
        assert!(buf.channels[0].is_empty());
    }
}
