//! What a game built with native mods holds for them: the atlas slot of a
//! mod's own image (`native_mod_image`), and the videos it plays
//! (`native_video_open`, docs/native-libraries.md).
//!
//! A video is not in the atlas: each has a texture of its own, the size of
//! its frames and only while it plays (`gfx/video_textures.rs`), which
//! `native_draw_video` draws in the world with the entity batch.

use crate::hud::{MOD_IMAGE_SIZE, MOD_IMAGE_SPRITE};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// How many videos play at once. Each costs ~33 MiB of GPU memory at 4K
/// while it plays, and twice that in staging buffers.
pub const VIDEO_SLOTS: usize = 2;
/// A video frame's largest size: 4K UHD (3840x2160).
pub const VIDEO_WIDTH: u32 = 3840;
pub const VIDEO_HEIGHT: u32 = 2160;

/// The empty slots of a game with native mods, for `HudAtlas::slots`.
pub(crate) fn mod_slots(images: &mut Vec<(String, u32, u32, Vec<u8>)>) {
    images.push((
        MOD_IMAGE_SPRITE.into(),
        MOD_IMAGE_SIZE,
        MOD_IMAGE_SIZE,
        Vec::new(),
    ));
}

/// What a video is doing (`native_video_state`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeVideoState {
    /// Seconds from the file's start, once it plays.
    pub position: Option<f64>,
    /// The file's length in seconds, when known.
    pub duration: Option<f64>,
    pub paused: bool,
    /// The file played to its end and is not looped.
    pub ended: bool,
    /// The file is open.
    pub loaded: bool,
    /// The file could not be played.
    pub failed: bool,
    /// A frame is in the slot: `native_draw_video` draws.
    pub shown: bool,
    /// The surface's size in pixels, as opened.
    pub size: (u32, u32),
    /// The video's own size, once known: it is fitted into `size` with bars.
    pub video_size: Option<(u32, u32)>,
    /// Frames rendered so far.
    pub frames: u64,
    /// Microseconds the last frame took to render, on the video's own thread.
    pub render_micros: u64,
    /// Microseconds of the game's thread the last frame spent taking the
    /// videos' frames, and handing a new one to the GPU.
    pub pump_micros: u64,
    pub upload_micros: u64,
    /// Average RGB color of the current frame, for ambient dynamic lighting.
    pub color: Option<[u8; 3]>,
    /// Left, center and right zone average colors of the current frame.
    pub colors: Option<[[u8; 3]; 3]>,
}

static UPLOAD_MICROS: AtomicU64 = AtomicU64::new(0);

/// Notes what handing the videos' frames to the GPU took the window's thread
/// since `started` (`VideoTextures::prepare`): the game's side of a video
/// frame's cost.
pub(crate) fn uploaded(started: Instant) {
    let micros = started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
    UPLOAD_MICROS.store(micros, Ordering::Relaxed);
}

/// Microseconds the window's thread spent on the last frame that showed a
/// new video frame: recording its copy, not copying its pixels.
pub fn upload_micros() -> u64 {
    UPLOAD_MICROS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Native mod images take one atlas slot while video slots take none.
    #[test]
    fn the_videos_take_no_atlas_slots() {
        let root = std::path::Path::new("/nonexistent/horizon-test-assets");
        let plain = crate::hud::HudAtlas::build(root).unwrap();
        let with_mod = crate::hud::HudAtlas::build_over_with_image(root, root, true).unwrap();

        assert!(with_mod.sprites.contains_key(crate::hud::MOD_IMAGE_SPRITE));
        assert!(!plain.sprites.contains_key(crate::hud::MOD_IMAGE_SPRITE));
        assert!(
            !with_mod
                .sprites
                .keys()
                .any(|k| k.starts_with("horizon/video"))
        );
        assert_eq!(with_mod.sprites.len(), plain.sprites.len() + 1);
    }

    /// Recording an upload stores the elapsed microseconds since it started.
    #[test]
    fn the_upload_time_is_what_passed_since_it_started() {
        uploaded(Instant::now() - std::time::Duration::from_millis(2));
        assert!(upload_micros() >= 2000);
    }
}
