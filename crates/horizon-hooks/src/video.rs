//! Video a native mod plays on a surface in the world (docs/native-libraries.md). The engine
//! keeps the surfaces, their atlas slots and the mod helpers (`native_video_*`); a higher layer's
//! [`Backend`] ([`Hooks::video`](crate::Hooks::video)) opens the library that decodes the files,
//! libmpv today (`horizon-mpv`, registered by horizon-official). Without one no video opens, and
//! `native_video_open` says why.

use std::path::Path;
use std::sync::Arc;

/// A rendered frame: `width * height` opaque RGBA pixels, top row first. The size travels with
/// the pixels: a new file can change it between two frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// How a player starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Start the file again when it ends.
    pub looped: bool,
    /// The sound is kept for the game to take ([`Player::take_audio`]) instead of played.
    pub capture_audio: bool,
}

/// Rendering statistics of a player.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Frames rendered since the player was made.
    pub frames: u64,
    /// What the last one took on the player's thread, in microseconds.
    pub render_micros: u64,
}

/// One file being played. Every call returns at once: decoding, rendering and tearing down
/// happen on the backend's own threads, and dropping a player never waits for them.
pub trait Player: Send {
    /// Plays the local file `path` from its start, in place of the current one.
    fn load(&self, path: &Path) -> Result<(), String>;
    fn set_paused(&self, paused: bool) -> Result<(), String>;
    /// Jumps to `seconds` from the file's start; captured sound waiting is dropped.
    fn seek(&self, seconds: f64) -> Result<(), String>;
    /// 0 to 100.
    fn set_volume(&self, volume: u8) -> Result<(), String>;
    /// Seconds from the file's start, once it plays.
    fn position(&self) -> Option<f64>;
    /// The file's length in seconds, when known.
    fn duration(&self) -> Option<f64>;
    fn paused(&self) -> bool;
    /// The file played to its end (and is not looped).
    fn ended(&self) -> bool;
    /// The file is open and playing or paused.
    fn loaded(&self) -> bool;
    /// The file could not be played.
    fn failed(&self) -> bool;
    /// The size of the frames being rendered now.
    fn size(&self) -> (u32, u32);
    /// The video's own displayed size, once known.
    fn video_size(&self) -> Option<(u32, u32)>;
    /// The newest frame since the last call, if any.
    fn take_frame(&self) -> Option<Frame>;
    /// Gives a taken frame's buffer back to be rendered into again.
    fn recycle_frame(&self, rgba: Vec<u8>);
    /// The newest frame's average colour; none before the first frame.
    fn frame_color(&self) -> Option<[u8; 3]>;
    /// The newest frame's left, centre and right thirds' average colours.
    fn frame_colors(&self) -> Option<[[u8; 3]; 3]>;
    fn stats(&self) -> Stats;
    /// The rate of the captured sound, in hertz; none when it is not captured.
    fn audio_sample_rate(&self) -> Option<u32>;
    /// Up to `max` mono samples of the captured sound, in step with the picture.
    fn take_audio(&self, max: usize) -> Option<Vec<f32>>;
    /// Up to `max` stereo samples (left, right).
    fn take_audio_stereo(&self, max: usize) -> Option<(Vec<f32>, Vec<f32>)>;
    /// Up to `max` 5.1 frames [FL, FR, FC, LFE, SL, SR].
    fn take_audio_51(&self, max: usize) -> Option<[Vec<f32>; 6]>;
    /// Up to `max` 7.1 frames [FL, FR, FC, LFE, BL, BR, SL, SR].
    fn take_audio_71(&self, max: usize) -> Option<[Vec<f32>; 8]>;
}

/// A loaded video library.
pub trait Library: Send + Sync {
    /// Its version, for the log.
    fn version(&self) -> String;
    /// A player that renders `width` x `height` RGBA frames.
    fn player(&self, width: u32, height: u32, options: &Options)
    -> Result<Box<dyn Player>, String>;
}

/// Opens a video library a native mod declared.
pub trait Backend: Sync {
    /// Library `name` (`libmpv`) as the launcher installed it in the game's version folder
    /// `version_dir`, after checking it is the declared file.
    fn open(&self, version_dir: &Path, name: &str) -> Result<Arc<dyn Library>, String>;
}
