//! One mpv core playing one local file into RGBA frames.
//!
//! The calling thread only queues commands and takes finished frames; every
//! call that can wait on mpv (rendering, events, tearing down) runs on the
//! player's own thread, so a frame of the game pays for a mutex and nothing
//! else.

use crate::Error;
use crate::sys::{self, Api};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

/// The longest side a player renders.
pub const MAX_SIDE: u32 = 4096;

/// How a player starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Start the file again when it ends.
    pub looped: bool,
    /// No sound at all: no audio device is opened.
    pub muted: bool,
    /// 0 to 100.
    pub volume: u8,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            looped: false,
            muted: false,
            volume: 100,
        }
    }
}

/// Whether `text` names a file on a local drive as Windows writes it
/// (`C:\videos\a.mkv`, or with `/`): not a URL, a device or a network share.
fn windows_local(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() > 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && matches!(b[2], b'\\' | b'/')
        && !text.contains("://")
}

/// Whether `text` is an absolute path as Unix writes it, and not a URL.
fn unix_local(text: &str) -> bool {
    text.starts_with('/') && !text.starts_with("//") && !text.contains("://")
}

/// Whether mpv may open `name`: an absolute path of this system, nothing with a protocol.
fn local_name(name: &str) -> bool {
    if cfg!(windows) {
        windows_local(name)
    } else {
        unix_local(name)
    }
}

/// `path` as mpv is given it: an existing file, by its full path. Anything
/// else (a URL, a network share, a device, a folder) is refused, so a mod
/// cannot make the game open a connection through mpv.
pub fn local_file(path: &Path) -> Result<String, Error> {
    let refused = || Error::NotLocal(path.to_owned());
    let full = std::fs::canonicalize(path).map_err(|_| refused())?;
    if !full.is_file() {
        return Err(refused());
    }
    let text = full.to_str().ok_or_else(refused)?;
    // Windows' verbatim prefix, which mpv does not take. A share stays
    // `UNC\server\…` without it and is refused below.
    let text = text.strip_prefix(r"\\?\").unwrap_or(text);
    if local_name(text) {
        Ok(text.to_owned())
    } else {
        Err(refused())
    }
}

const NONE: u64 = u64::MAX;

#[derive(Default)]
struct Wake {
    pending: bool,
    stop: bool,
}

/// What the player's thread and the game share.
struct Shared {
    wake: Mutex<Wake>,
    woken: Condvar,
    /// The newest frame not taken yet.
    frame: Mutex<Option<Vec<u8>>>,
    frames: AtomicU64,
    render_micros: AtomicU64,
    /// f64 bits, [`NONE`] when mpv has none.
    position: AtomicU64,
    duration: AtomicU64,
    /// `width << 32 | height` of the video as displayed, [`NONE`] before it is known.
    video_size: AtomicU64,
    paused: AtomicBool,
    ended: AtomicBool,
    loaded: AtomicBool,
    failed: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    fn wake(&self, stop: bool) {
        let mut wake = lock(&self.wake);
        wake.pending = true;
        wake.stop |= stop;
        self.woken.notify_one();
    }
}

/// mpv's wakeup and render update callbacks: called on mpv's threads, where
/// no mpv function may be called.
unsafe extern "C" fn woken(shared: *mut c_void) {
    // SAFETY: `shared` is the `Shared` the player's thread keeps alive until
    // after it removed the callbacks and destroyed the core.
    unsafe { (*(shared as *const Shared)).wake(false) }
}

/// mpv's pointers, moved to the player's thread.
struct Core {
    api: Arc<Api>,
    handle: *mut sys::Handle,
    render: *mut sys::RenderContext,
}
// SAFETY: mpv's client API may be called from any thread; the render context
// is used by the player's thread alone once it is moved there.
unsafe impl Send for Core {}

const OBSERVE_POSITION: u64 = 1;
const OBSERVE_DURATION: u64 = 2;
const OBSERVE_PAUSE: u64 = 3;
const OBSERVE_ENDED: u64 = 4;
const OBSERVE_WIDTH: u64 = 5;
const OBSERVE_HEIGHT: u64 = 6;

/// Rendering statistics of a player.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Frames rendered since the player was made.
    pub frames: u64,
    /// What the last one took on the player's thread, in microseconds.
    pub render_micros: u64,
}

/// A video being played. Dropping it stops the playback; the core is torn
/// down on its own thread.
pub struct Player {
    api: Arc<Api>,
    handle: *mut sys::Handle,
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    width: u32,
    height: u32,
}
// SAFETY: `handle` is only used for mpv's thread-safe client calls.
unsafe impl Send for Player {}

impl Player {
    pub(crate) fn new(
        api: Arc<Api>,
        width: u32,
        height: u32,
        options: &Options,
    ) -> Result<Self, Error> {
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return Err(Error::Size(width, height));
        }
        // SAFETY: plain calls of mpv's client API as its headers describe
        // them; `handle` is destroyed on every early return.
        unsafe {
            let handle = (api.mpv_create)();
            if handle.is_null() {
                return Err(Error::Mpv("mpv_create", "no core".into()));
            }
            let fail = |error: Error| {
                (api.mpv_terminate_destroy)(handle);
                error
            };
            let volume = options.volume.min(100).to_string();
            let settings: [(&CStr, &str); 15] = [
                // Frames go to the render API, never to a window of mpv's own.
                (c"vo", "libmpv"),
                // Nothing of the player's mpv setup, no scripts, no downloader.
                (c"config", "no"),
                (c"load-scripts", "no"),
                (c"ytdl", "no"),
                (c"osc", "no"),
                (c"terminal", "no"),
                (c"input-default-bindings", "no"),
                (c"input-vo-keyboard", "no"),
                (c"load-unsafe-playlists", "no"),
                // Decoded on mpv's threads; the software renderer takes system memory.
                (c"hwdec", "no"),
                (c"idle", "yes"),
                // The last frame stays up at the end (`ended`).
                (c"keep-open", "yes"),
                (c"loop-file", if options.looped { "inf" } else { "no" }),
                (c"audio", if options.muted { "no" } else { "auto" }),
                (c"volume", &volume),
            ];
            for (name, value) in settings {
                let value = CString::new(value).unwrap_or_default();
                let code = (api.mpv_set_option_string)(handle, name.as_ptr(), value.as_ptr());
                api.check("mpv_set_option_string", code).map_err(fail)?;
            }
            api.check("mpv_initialize", (api.mpv_initialize)(handle))
                .map_err(fail)?;

            let shared = Arc::new(Shared {
                wake: Mutex::default(),
                woken: Condvar::new(),
                frame: Mutex::new(None),
                frames: AtomicU64::new(0),
                render_micros: AtomicU64::new(0),
                position: AtomicU64::new(NONE),
                duration: AtomicU64::new(NONE),
                video_size: AtomicU64::new(NONE),
                paused: AtomicBool::new(false),
                ended: AtomicBool::new(false),
                loaded: AtomicBool::new(false),
                failed: AtomicBool::new(false),
            });
            let context = Arc::as_ptr(&shared) as *mut c_void;
            (api.mpv_set_wakeup_callback)(handle, Some(woken), context);
            for (id, name, format) in [
                (OBSERVE_POSITION, c"time-pos", sys::FORMAT_DOUBLE),
                (OBSERVE_DURATION, c"duration", sys::FORMAT_DOUBLE),
                (OBSERVE_PAUSE, c"pause", sys::FORMAT_FLAG),
                (OBSERVE_ENDED, c"eof-reached", sys::FORMAT_FLAG),
                (OBSERVE_WIDTH, c"dwidth", sys::FORMAT_INT64),
                (OBSERVE_HEIGHT, c"dheight", sys::FORMAT_INT64),
            ] {
                let code = (api.mpv_observe_property)(handle, id, name.as_ptr(), format);
                api.check("mpv_observe_property", code).map_err(fail)?;
            }
            // Every file mpv is about to open passes `allow_load` first.
            let code = (api.mpv_hook_add)(handle, 0, c"on_load".as_ptr(), 0);
            api.check("mpv_hook_add", code).map_err(fail)?;

            let mut render = std::ptr::null_mut();
            let mut params = [
                sys::RenderParam {
                    kind: sys::RENDER_PARAM_API_TYPE,
                    data: sys::RENDER_API_SW.as_ptr() as *mut c_void,
                },
                sys::RenderParam {
                    kind: sys::RENDER_PARAM_INVALID,
                    data: std::ptr::null_mut(),
                },
            ];
            let code = (api.mpv_render_context_create)(&mut render, handle, params.as_mut_ptr());
            api.check("mpv_render_context_create", code).map_err(fail)?;
            (api.mpv_render_context_set_update_callback)(render, Some(woken), context);

            let core = Core {
                api: api.clone(),
                handle,
                render,
            };
            let theirs = shared.clone();
            let thread = std::thread::Builder::new()
                .name("horizon-mpv".into())
                .spawn(move || run(core, &theirs, width, height))
                .map_err(|e| {
                    (api.mpv_render_context_free)(render);
                    fail(Error::Mpv("thread", e.to_string()))
                })?;
            Ok(Self {
                api,
                handle,
                shared,
                thread: Some(thread),
                width,
                height,
            })
        }
    }

    /// The size of the frames [`take_frame`](Self::take_frame) gives.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Queues an mpv command; mpv runs it on its own thread.
    fn command(&self, args: &[&str]) -> Result<(), Error> {
        let owned: Vec<CString> = args
            .iter()
            .map(|a| CString::new(*a).map_err(|_| Error::Mpv("command", "NUL in argument".into())))
            .collect::<Result<_, _>>()?;
        let mut pointers: Vec<*const c_char> = owned.iter().map(|a| a.as_ptr()).collect();
        pointers.push(std::ptr::null());
        // SAFETY: a NULL-terminated array of C strings, which mpv copies before it returns.
        let code = unsafe { (self.api.mpv_command_async)(self.handle, 0, pointers.as_mut_ptr()) };
        self.api.check("mpv_command_async", code)
    }

    /// Plays the local file `path` from its start, in place of the current one.
    pub fn load(&self, path: &Path) -> Result<(), Error> {
        let file = local_file(path)?;
        self.shared.loaded.store(false, Ordering::Relaxed);
        self.shared.ended.store(false, Ordering::Relaxed);
        self.shared.failed.store(false, Ordering::Relaxed);
        let file = CString::new(file).map_err(|_| Error::NotLocal(path.to_owned()))?;
        let mut args = [c"loadfile".as_ptr(), file.as_ptr(), std::ptr::null()];
        // SAFETY: a NULL-terminated array of C strings. `loadfile` only puts the
        // file on mpv's playlist: opening it happens on mpv's threads.
        let code = unsafe { (self.api.mpv_command)(self.handle, args.as_mut_ptr()) };
        self.api.check("mpv_command", code)
    }

    pub fn set_paused(&self, paused: bool) -> Result<(), Error> {
        self.command(&["set", "pause", if paused { "yes" } else { "no" }])
    }

    /// Jumps to `seconds` from the file's start.
    pub fn seek(&self, seconds: f64) -> Result<(), Error> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(Error::Mpv("seek", format!("{seconds} is not a time")));
        }
        self.command(&["seek", &format!("{seconds:.3}"), "absolute"])
    }

    /// 0 to 100.
    pub fn set_volume(&self, volume: u8) -> Result<(), Error> {
        self.command(&["set", "volume", &volume.min(100).to_string()])
    }

    pub fn set_looped(&self, looped: bool) -> Result<(), Error> {
        self.command(&["set", "loop-file", if looped { "inf" } else { "no" }])
    }

    /// Stops the playback and forgets the file.
    pub fn stop(&self) -> Result<(), Error> {
        self.command(&["stop"])
    }

    fn seconds(value: &AtomicU64) -> Option<f64> {
        match value.load(Ordering::Relaxed) {
            NONE => None,
            bits => Some(f64::from_bits(bits)),
        }
    }

    /// Seconds from the file's start, once it plays.
    pub fn position(&self) -> Option<f64> {
        Self::seconds(&self.shared.position)
    }

    /// The file's length in seconds, when mpv knows it.
    pub fn duration(&self) -> Option<f64> {
        Self::seconds(&self.shared.duration)
    }

    pub fn paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    /// The file played to its end (and is not looped).
    pub fn ended(&self) -> bool {
        self.shared.ended.load(Ordering::Relaxed)
    }

    /// The file is open and playing or paused.
    pub fn loaded(&self) -> bool {
        self.shared.loaded.load(Ordering::Relaxed)
    }

    /// The file could not be played, or mpv was refused something that is not a local file.
    pub fn failed(&self) -> bool {
        self.shared.failed.load(Ordering::Relaxed)
    }

    /// The video's own displayed size, once known: frames are this picture
    /// fitted into [`size`](Self::size) with black bars.
    pub fn video_size(&self) -> Option<(u32, u32)> {
        match self.shared.video_size.load(Ordering::Relaxed) {
            NONE => None,
            size => Some(((size >> 32) as u32, size as u32)),
        }
    }

    /// The newest frame since the last call: `width * height` RGBA pixels,
    /// top row first, opaque. None when mpv showed nothing new.
    pub fn take_frame(&self) -> Option<Vec<u8>> {
        lock(&self.shared.frame).take()
    }

    pub fn stats(&self) -> Stats {
        Stats {
            frames: self.shared.frames.load(Ordering::Relaxed),
            render_micros: self.shared.render_micros.load(Ordering::Relaxed),
        }
    }

    /// Stops the playback and waits until mpv is torn down.
    pub fn close(mut self) {
        self.shared.wake(true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // Not joined: destroying the core can take a while, and the game's
        // thread must not wait for it.
        self.shared.wake(true);
    }
}

/// The player's thread: renders each new frame, follows mpv's events, and
/// tears mpv down when the player is dropped.
fn run(core: Core, shared: &Shared, width: u32, height: u32) {
    let Core {
        api,
        handle,
        render,
    } = core;
    let mut video = (None, None);
    loop {
        {
            let mut wake = lock(&shared.wake);
            while !wake.pending && !wake.stop {
                wake = shared
                    .woken
                    .wait(wake)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            if wake.stop {
                break;
            }
            wake.pending = false;
        }
        events(&api, handle, shared, &mut video);
        // SAFETY: the context is this thread's own until it frees it below.
        let update = unsafe { (api.mpv_render_context_update)(render) };
        if update & sys::RENDER_UPDATE_FRAME != 0 {
            let started = Instant::now();
            if let Some(frame) = render_frame(&api, render, width, height) {
                shared.render_micros.store(
                    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
                    Ordering::Relaxed,
                );
                shared.frames.fetch_add(1, Ordering::Relaxed);
                *lock(&shared.frame) = Some(frame);
            }
        }
    }
    // SAFETY: the render context goes before its core (render.h); after the
    // core is destroyed no callback runs, so `shared` may go once this returns.
    unsafe {
        (api.mpv_render_context_set_update_callback)(render, None, std::ptr::null_mut());
        (api.mpv_render_context_free)(render);
        (api.mpv_set_wakeup_callback)(handle, None, std::ptr::null_mut());
        (api.mpv_terminate_destroy)(handle);
    }
}

/// The current frame as `width * height` opaque RGBA pixels. Waits until the
/// frame is due, as mpv's renderer does by default: that is what paces the video.
fn render_frame(
    api: &Api,
    render: *mut sys::RenderContext,
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    let mut size = [width as c_int, height as c_int];
    let mut stride = width as usize * 4;
    let mut pixels = vec![0u8; stride * height as usize];
    let mut params = [
        sys::RenderParam {
            kind: sys::RENDER_PARAM_SW_SIZE,
            data: size.as_mut_ptr().cast(),
        },
        sys::RenderParam {
            kind: sys::RENDER_PARAM_SW_FORMAT,
            data: c"rgb0".as_ptr() as *mut c_void,
        },
        sys::RenderParam {
            kind: sys::RENDER_PARAM_SW_STRIDE,
            data: (&raw mut stride).cast(),
        },
        sys::RenderParam {
            kind: sys::RENDER_PARAM_SW_POINTER,
            data: pixels.as_mut_ptr().cast(),
        },
        sys::RenderParam {
            kind: sys::RENDER_PARAM_INVALID,
            data: std::ptr::null_mut(),
        },
    ];
    // SAFETY: `pixels` holds `height` rows of `stride` bytes, and every
    // parameter outlives the call.
    let code = unsafe { (api.mpv_render_context_render)(render, params.as_mut_ptr()) };
    if code < 0 {
        log::warn!("mpv_render_context_render: {}", api.error(code));
        return None;
    }
    // `rgb0` leaves the fourth byte to the renderer; the atlas wants it opaque.
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[3] = 0xff;
    }
    Some(pixels)
}

/// Takes mpv's waiting events into `shared`.
fn events(
    api: &Api,
    handle: *mut sys::Handle,
    shared: &Shared,
    video: &mut (Option<u32>, Option<u32>),
) {
    loop {
        // SAFETY: only this thread waits for this handle's events; the event
        // and what it points to stay valid until the next call.
        let event = unsafe { &*(api.mpv_wait_event)(handle, 0.0) };
        match event.id {
            sys::EVENT_NONE => return,
            sys::EVENT_SHUTDOWN => {
                shared.failed.store(true, Ordering::Relaxed);
                return;
            }
            sys::EVENT_FILE_LOADED => shared.loaded.store(true, Ordering::Relaxed),
            sys::EVENT_END_FILE if !event.data.is_null() => {
                // SAFETY: an END_FILE event's data is an `mpv_event_end_file`.
                let end = unsafe { &*(event.data as *const sys::EventEndFile) };
                shared.loaded.store(false, Ordering::Relaxed);
                if end.reason == sys::END_FILE_ERROR {
                    log::warn!("mpv could not play the file: {}", api.error(end.error));
                    shared.failed.store(true, Ordering::Relaxed);
                }
            }
            sys::EVENT_HOOK if !event.data.is_null() => {
                // SAFETY: a HOOK event's data is an `mpv_event_hook`.
                let hook = unsafe { &*(event.data as *const sys::EventHook) };
                if !allow_load(api, handle) {
                    shared.failed.store(true, Ordering::Relaxed);
                }
                // SAFETY: continues the hook this event reported.
                unsafe { (api.mpv_hook_continue)(handle, hook.id) };
            }
            sys::EVENT_PROPERTY_CHANGE if !event.data.is_null() => {
                // SAFETY: a PROPERTY_CHANGE event's data is an `mpv_event_property`,
                // whose own data has the format it names.
                let (format, data) = unsafe {
                    let property = &*(event.data as *const sys::EventProperty);
                    (property.format, property.data)
                };
                let known = format != sys::FORMAT_NONE && !data.is_null();
                let double = || {
                    // SAFETY: observed as a double.
                    (known && format == sys::FORMAT_DOUBLE)
                        .then(|| unsafe { *(data as *const f64) })
                        .map_or(NONE, f64::to_bits)
                };
                // SAFETY: observed as a flag (an int).
                let flag = || {
                    known && format == sys::FORMAT_FLAG && unsafe { *(data as *const c_int) } != 0
                };
                let side = || {
                    // SAFETY: observed as an int64.
                    (known && format == sys::FORMAT_INT64)
                        .then(|| unsafe { *(data as *const i64) })
                        .and_then(|v| u32::try_from(v).ok())
                };
                match event.reply_userdata {
                    OBSERVE_POSITION => shared.position.store(double(), Ordering::Relaxed),
                    OBSERVE_DURATION => shared.duration.store(double(), Ordering::Relaxed),
                    OBSERVE_PAUSE => shared.paused.store(flag(), Ordering::Relaxed),
                    OBSERVE_ENDED => shared.ended.store(flag(), Ordering::Relaxed),
                    OBSERVE_WIDTH => video.0 = side(),
                    OBSERVE_HEIGHT => video.1 = side(),
                    _ => {}
                }
                let size = match *video {
                    (Some(w), Some(h)) => u64::from(w) << 32 | u64::from(h),
                    _ => NONE,
                };
                shared.video_size.store(size, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

/// mpv's `on_load` hook: whether what it is about to open
/// (`stream-open-filename`) is a local file. Anything else, a URL a playlist
/// file named for one, is stopped before mpv opens it.
fn allow_load(api: &Api, handle: *mut sys::Handle) -> bool {
    // SAFETY: the returned string is mpv's to free, and is freed below.
    let name = unsafe {
        let text = (api.mpv_get_property_string)(handle, c"stream-open-filename".as_ptr());
        if text.is_null() {
            None
        } else {
            let name = CStr::from_ptr(text).to_string_lossy().into_owned();
            (api.mpv_free)(text.cast());
            Some(name)
        }
    };
    if name.as_deref().is_some_and(local_name) {
        return true;
    }
    log::warn!("mpv was stopped from opening {name:?}: only local files are played");
    let mut stop = [c"stop".as_ptr(), std::ptr::null()];
    // SAFETY: a NULL-terminated array of C strings.
    unsafe { (api.mpv_command_async)(handle, 0, stop.as_mut_ptr()) };
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_paths_on_a_local_drive_are_local_on_windows() {
        assert!(windows_local(r"C:\videos\a.mkv"));
        assert!(windows_local("d:/videos/a.mkv"));
        for refused in [
            "https://example.com/a.mkv",
            r"\\server\share\a.mkv",
            r"UNC\server\share\a.mkv",
            "//server/share/a.mkv",
            "a.mkv",
            r"..\a.mkv",
            "-",
            "av://lavfi:testsrc",
            "C:",
            r"C:\a://b",
            "memory://x",
        ] {
            assert!(!windows_local(refused), "{refused}");
        }
    }

    #[test]
    fn only_absolute_paths_are_local_on_unix() {
        assert!(unix_local("/home/me/a.mkv"));
        for refused in [
            "https://example.com/a.mkv",
            "//server/a.mkv",
            "a.mkv",
            "-",
            "/tmp/a://b",
        ] {
            assert!(!unix_local(refused), "{refused}");
        }
    }

    #[test]
    fn a_file_that_is_not_there_or_is_a_folder_is_refused() {
        let dir = std::env::temp_dir();
        assert!(matches!(local_file(&dir), Err(Error::NotLocal(_))));
        assert!(matches!(
            local_file(&dir.join("horizon-mpv-no-such-file.mkv")),
            Err(Error::NotLocal(_))
        ));
        assert!(matches!(
            local_file(Path::new("https://example.com/a.mkv")),
            Err(Error::NotLocal(_))
        ));
    }

    #[test]
    fn an_existing_file_is_given_by_its_full_path() {
        let file =
            std::env::temp_dir().join(format!("horizon-mpv-local-{}.bin", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let name = local_file(&file).unwrap();
        assert!(local_name(&name), "{name}");
        assert!(!name.starts_with(r"\\?\"));
        let _ = std::fs::remove_file(&file);
    }
}
