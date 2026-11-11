//! The atlas slots only a game built with native mods holds: a mod's own
//! image (`native_mod_image`) and the frames of the video it plays
//! (`native_video_open`, docs/native-libraries.md).
//!
//! A video slot is written like the minimap's: the frame is a patch into
//! the shared atlas, so `native_draw_video` draws it in the world with the
//! entity batch and no pipeline of its own.

use crate::hud::{MOD_IMAGE_SIZE, MOD_IMAGE_SPRITE};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// How many videos play at once. Each slot costs 3.5 MiB of atlas.
pub const VIDEO_SLOTS: usize = 2;
/// A video frame's largest size: 1080p (Full HD native).
pub const VIDEO_WIDTH: u32 = 1920;
pub const VIDEO_HEIGHT: u32 = 1080;

/// The atlas sprite of video slot `slot` (`horizon/video{slot}`).
pub fn video_slot_sprite(slot: usize) -> Option<&'static str> {
    static NAMES: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
        (0..VIDEO_SLOTS)
            .map(|i| format!("horizon/video{i}"))
            .collect()
    });
    NAMES.get(slot).map(String::as_str)
}

/// The empty slots of a game with native mods, for `HudAtlas::slots`.
pub(crate) fn mod_slots(images: &mut Vec<(String, u32, u32, Vec<u8>)>) {
    images.push((
        MOD_IMAGE_SPRITE.into(),
        MOD_IMAGE_SIZE,
        MOD_IMAGE_SIZE,
        Vec::new(),
    ));
    for slot in 0..VIDEO_SLOTS {
        let name = video_slot_sprite(slot).unwrap_or_default();
        images.push((name.into(), VIDEO_WIDTH, VIDEO_HEIGHT, Vec::new()));
    }
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
    /// videos' frames, and staging their pixels for the GPU.
    pub pump_micros: u64,
    pub upload_micros: u64,
    /// Average RGB color of the current frame, for ambient dynamic lighting.
    pub color: Option<[u8; 3]>,
    /// Left, center and right zone average colors of the current frame.
    pub colors: Option<[[u8; 3]; 3]>,
}

/// A patch this large is a video frame or a mod's image, not a glyph or a map.
const LARGE_PATCH: usize = 256 * 1024;
static UPLOAD_MICROS: AtomicU64 = AtomicU64::new(0);

/// Notes what staging `patches` took since `started`, when a large one was
/// among them (`HudRenderer::upload_patches`): the game's side of a video
/// frame's cost.
pub(crate) fn uploaded(patches: &[crate::font::Patch], started: Instant) {
    if patches.iter().any(|p| p.rgba.len() >= LARGE_PATCH) {
        let micros = started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
        UPLOAD_MICROS.store(micros, Ordering::Relaxed);
    }
}

/// Microseconds the last frame with a large patch spent staging its patches.
pub fn upload_micros() -> u64 {
    UPLOAD_MICROS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::HudAtlas;
    use std::path::Path;

    /// The video slots are paid only by a game with a native mod loaded.
    #[test]
    fn the_video_slots_exist_only_when_a_mod_is_loaded() {
        let root = Path::new("/nonexistent/horizon-test-assets");
        let plain = HudAtlas::build(root).unwrap();
        let with_mod = HudAtlas::build_over_with_image(root, root, true).unwrap();
        for slot in 0..VIDEO_SLOTS {
            let name = video_slot_sprite(slot).unwrap();
            assert!(!plain.sprites.contains_key(name));
            let rect = with_mod.sprites[name];
            assert_eq!((rect.w, rect.h), (VIDEO_WIDTH, VIDEO_HEIGHT));
        }
        assert!(video_slot_sprite(VIDEO_SLOTS).is_none());
    }

    #[test]
    fn only_a_large_patch_is_timed() {
        let patch = |bytes: usize| crate::font::Patch {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            rgba: vec![0; bytes],
        };
        UPLOAD_MICROS.store(7, Ordering::Relaxed);
        uploaded(&[patch(64)], Instant::now());
        assert_eq!(upload_micros(), 7);
        let started = Instant::now() - std::time::Duration::from_millis(2);
        uploaded(&[patch(64), patch(LARGE_PATCH)], started);
        assert!(upload_micros() >= 2000);
    }
}
