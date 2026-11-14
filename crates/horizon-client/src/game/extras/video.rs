//! Video a native mod plays on a surface in the world
//! (docs/native-libraries.md): libmpv, which the mod declared and the
//! launcher installed beside the game, decodes and renders on its own
//! threads, through the video backend a higher layer registers
//! (`horizon_hooks::video`: `horizon-mpv`, from horizon-official); each new frame is a patch into the video's atlas
//! slot (`video_slots`), and `native_draw_video` draws that slot on a quad
//! from `native_block_entity_submit`. mpv's sound is captured, not played
//! by mpv: the mod takes it (`native_video_audio*`) and plays it through the
//! game's mixer (`push_voices`), where it is placed in the world.
//!
//! The game's thread pays for taking a finished frame and staging its
//! pixels; `NativeVideoState` says what both cost.

use crate::game::{Game, native_block_entity_draw::BlockEntityDraw};
use horizon_client::video_slots::{
    NativeVideoState, VIDEO_HEIGHT, VIDEO_SLOTS, VIDEO_WIDTH, upload_micros, video_slot_sprite,
};
use horizon_hooks::video::{Library, Options, Player};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// The library's name in a mod's `native-libraries.toml`.
const LIBRARY: &str = "libmpv";

struct Video {
    player: Box<dyn Player>,
    /// The id the mod was given: its slot and a generation ([`video_id`]).
    id: u32,
    /// The size of the frame of this video in its slot; none before one is.
    shown: Option<(u32, u32)>,
}

/// A video's id: its slot in the low byte, a generation above, so an id kept
/// after its video closed (leaving the world closes them all) names no video,
/// not whichever video took the slot next.
fn video_id(slot: usize, generation: u32) -> u32 {
    (generation << 8) | slot as u32
}

/// The slot of video id `id`.
fn video_slot(id: u32) -> usize {
    (id & 0xff) as usize
}

/// The videos playing, by slot.
#[derive(Default)]
pub(in crate::game) struct Videos {
    /// libmpv, opened and checked when the first video is: or why it was not.
    library: Option<Result<Arc<dyn Library>, String>>,
    slots: [Option<Video>; VIDEO_SLOTS],
    /// The atlas the slots' frames were written to: a reload starts it empty.
    atlas: u64,
    /// Microseconds the last frame spent taking the videos' frames.
    pump_micros: u64,
    /// The last generation given to an id; never 0, so no id is a bare slot number.
    generation: u32,
}

/// A surface no larger than a video slot, with pixels to show.
fn fits(width: u32, height: u32) -> bool {
    (1..=VIDEO_WIDTH).contains(&width) && (1..=VIDEO_HEIGHT).contains(&height)
}

/// The corners of a `width` x `height` frame in its slot, as fractions of the
/// slot, half a pixel in: the slot's unused part is never sampled.
fn frame_uv(width: u32, height: u32) -> [[f32; 2]; 4] {
    let (u0, v0) = (0.5 / VIDEO_WIDTH as f32, 0.5 / VIDEO_HEIGHT as f32);
    let u1 = (width as f32 - 0.5) / VIDEO_WIDTH as f32;
    let v1 = (height as f32 - 0.5) / VIDEO_HEIGHT as f32;
    [[u0, v0], [u0, v1], [u1, v1], [u1, v0]]
}

// Native mods use these; nothing in Horizon does.
#[allow(dead_code, reason = "native mod API (docs/native-libraries.md)")]
impl Game<'_> {
    /// libmpv from the version folder, checked against the hash the mod
    /// declared. Tried once: a failure is kept, with its reason.
    fn video_library(&mut self) -> Result<Arc<dyn Library>, String> {
        self.extras
            .videos
            .library
            .get_or_insert_with(|| {
                let backend = horizon_hooks::engine::video()
                    .ok_or("this game has no video backend (horizon-official registers libmpv)")?;
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                let dir = exe.parent().ok_or("the game has no folder")?;
                let library = backend.open(dir, LIBRARY)?;
                log::info!("{} loaded from the version folder", library.version());
                Ok(library)
            })
            .clone()
    }

    fn video(&self, video: u32) -> Option<&Video> {
        let v = self.extras.videos.slots.get(video_slot(video))?.as_ref()?;
        (v.id == video).then_some(v)
    }

    fn video_mut(&mut self, video: u32) -> Option<&mut Video> {
        let v = self
            .extras
            .videos
            .slots
            .get_mut(video_slot(video))?
            .as_mut()?;
        (v.id == video).then_some(v)
    }

    /// Opens a video surface of `width` x `height` pixels (at most 1280 x
    /// 720) and plays the local file `path` on it, again from the start when
    /// `looped`. Returns the surface, for the helpers below, or why there is
    /// none: no free surface (two play at once), a file that is not a local
    /// file, or no libmpv installed by a mod's `native-libraries.toml`. The
    /// first video checks and loads the library, which takes a moment. Its
    /// sound is captured for the mod to take (`native_video_audio*`) and play
    /// through the game's mixer; nothing plays it otherwise. A video stops
    /// when the world is left, and its id then names nothing (ids are not slot
    /// numbers: a closed video's id is never reused). API @API:world_video@.
    pub(in crate::game) fn native_video_open(
        &mut self,
        path: &str,
        mut width: u32,
        mut height: u32,
        looped: bool,
    ) -> Result<u32, String> {
        if width == 0 || height == 0 {
            width = VIDEO_WIDTH;
            height = VIDEO_HEIGHT;
        }
        if !fits(width, height) {
            return Err(format!(
                "a video surface is at most {VIDEO_WIDTH}x{VIDEO_HEIGHT}, not {width}x{height}"
            ));
        }
        let slot = (0..VIDEO_SLOTS)
            .find(|&i| self.extras.videos.slots[i].is_none())
            .ok_or_else(|| format!("{VIDEO_SLOTS} videos are playing already"))?;
        let known = video_slot_sprite(slot).is_some_and(|s| self.gui.atlas.sprites.contains_key(s));
        if !known {
            return Err("this game was built without native mods: it has no video surfaces".into());
        }
        let options = Options {
            looped,
            capture_audio: true,
        };
        let player = self.video_library()?.player(width, height, &options)?;
        player.load(Path::new(path))?;
        let videos = &mut self.extras.videos;
        videos.generation = (videos.generation % 0x00ff_ffff) + 1;
        let id = video_id(slot, videos.generation);
        videos.slots[slot] = Some(Video {
            player,
            id,
            shown: None,
        });
        Ok(id)
    }

    /// Plays another local file on `video`, from its start. API @API:world_video@.
    pub(in crate::game) fn native_video_load(
        &mut self,
        video: u32,
        path: &str,
    ) -> Result<(), String> {
        let video = self.video_mut(video).ok_or("no such video surface")?;
        video.player.load(Path::new(path))?;
        video.shown = None;
        Ok(())
    }

    /// Pauses or resumes `video`. False for no such surface. API @API:world_video@.
    pub(in crate::game) fn native_video_pause(&mut self, video: u32, paused: bool) -> bool {
        self.video(video)
            .is_some_and(|v| v.player.set_paused(paused).is_ok())
    }

    /// Jumps to `seconds` from the file's start; the sound waiting to be taken
    /// is dropped, and what comes next is from there (a mod also drops what it
    /// queued, `clear_voices`). False for no such surface or a time that is
    /// none. API @API:world_video@.
    pub(in crate::game) fn native_video_seek(&mut self, video: u32, seconds: f64) -> bool {
        self.video(video)
            .is_some_and(|v| v.player.seek(seconds).is_ok())
    }

    /// The loudness of `video`'s sound, 0 to 100. API @API:world_video@.
    pub(in crate::game) fn native_video_volume(&mut self, video: u32, volume: u8) -> bool {
        self.video(video)
            .is_some_and(|v| v.player.set_volume(volume).is_ok())
    }

    /// Stops `video` and frees its surface. False when there was none.
    /// API @API:world_video@.
    pub(in crate::game) fn native_video_close(&mut self, video: u32) -> bool {
        if self.video(video).is_none() {
            return false;
        }
        // Dropping the player never waits for mpv: it is torn down on its own thread.
        self.extras.videos.slots[video_slot(video)].take().is_some()
    }

    /// What `video` is doing and what its frames cost; none for no such
    /// surface. API @API:world_video@.
    pub(in crate::game) fn native_video_state(&self, video: u32) -> Option<NativeVideoState> {
        let v = self.video(video)?;
        let stats = v.player.stats();
        Some(NativeVideoState {
            position: v.player.position(),
            duration: v.player.duration(),
            paused: v.player.paused(),
            ended: v.player.ended(),
            loaded: v.player.loaded(),
            failed: v.player.failed(),
            shown: v.shown.is_some(),
            size: v.shown.unwrap_or_else(|| v.player.size()),
            video_size: v.player.video_size(),
            frames: stats.frames,
            render_micros: stats.render_micros,
            pump_micros: self.extras.videos.pump_micros,
            upload_micros: upload_micros(),
            color: v.shown.and_then(|_| v.player.frame_color()),
            colors: v.shown.and_then(|_| v.player.frame_colors()),
        })
    }

    /// The average RGB color ([r, g, b] in 0..=255) of `video`'s current frame,
    /// or None if no frame is shown. Useful for ambient dynamic room lighting.
    /// API @API:world_video@.
    pub(in crate::game) fn native_video_color(&self, video: u32) -> Option<[u8; 3]> {
        let v = self.video(video).filter(|v| v.shown.is_some())?;
        v.player.frame_color()
    }

    /// Left, center and right zone average RGB colors of `video`'s current frame.
    /// Useful for multi-zone dynamic ambient lighting on wide or curved screens.
    /// API @API:world_video@.
    pub(in crate::game) fn native_video_colors(&self, video: u32) -> Option<[[u8; 3]; 3]> {
        let v = self.video(video).filter(|v| v.shown.is_some())?;
        v.player.frame_colors()
    }

    /// The rate of the samples the `native_video_audio*` calls give, in hertz;
    /// none for no such surface. API @API:world_video@.
    pub(in crate::game) fn native_video_audio_rate(&self, video: u32) -> Option<u32> {
        self.video(video)?.player.audio_sample_rate()
    }

    /// Takes up to `max_samples` mono samples (-1.0..=1.0) of `video`'s sound,
    /// a surround file folded down first, kept in step with the picture: sound
    /// that lags is dropped, sound ahead of the picture is held (none then).
    /// The rate is `native_video_audio_rate`'s. Take about a frame's worth each
    /// frame and play it with `host.push_voice` at the screen for 3D sound
    /// through the game's mixer. API @API:world_video@.
    pub(in crate::game) fn native_video_audio(
        &mut self,
        video: u32,
        max_samples: u32,
    ) -> Option<Vec<f32>> {
        let v = self.video(video)?;
        v.player.take_audio(max_samples as usize)
    }

    /// Takes up to `max_samples` stereo (left, right) samples from `video`, a
    /// surround file folded down with its centre (dialogue) and surrounds at
    /// -3 dB; for left and right speakers across a wide screen. Synced as
    /// `native_video_audio`. API @API:world_video@.
    pub(in crate::game) fn native_video_audio_stereo(
        &mut self,
        video: u32,
        max_samples: u32,
    ) -> Option<(Vec<f32>, Vec<f32>)> {
        let v = self.video(video)?;
        v.player.take_audio_stereo(max_samples as usize)
    }

    /// Takes up to `max_samples` 5.1 frames [FL, FR, FC, LFE, SL, SR] from
    /// `video`, a stereo file upmixed. Play the channels together with
    /// `host.push_voices` so they stay lined up. API @API:world_video@.
    pub(in crate::game) fn native_video_audio_51(
        &mut self,
        video: u32,
        max_samples: u32,
    ) -> Option<[Vec<f32>; 6]> {
        let v = self.video(video)?;
        v.player.take_audio_51(max_samples as usize)
    }

    /// Takes up to `max_samples` 7.1 frames [FL, FR, FC, LFE, BL, BR, SL, SR]
    /// from `video`, a stereo file upmixed. Play the channels together with
    /// `host.push_voices`. API @API:world_video@.
    pub(in crate::game) fn native_video_audio_71(
        &mut self,
        video: u32,
        max_samples: u32,
    ) -> Option<[Vec<f32>; 8]> {
        let v = self.video(video)?;
        v.player.take_audio_71(max_samples as usize)
    }

    /// Plays one block of each of several of a mod's sources together, as one
    /// multichannel sound (client, `host.push_voices`): `channels[i]` goes to `sources[i]`,
    /// placed at `at[i]`, all mono at `sample_rate` hertz. All are queued under one lock and
    /// start in the same output buffer, or none is: a channel past its source's two seconds
    /// refuses the whole call, so the channels never drift apart. False when nothing was
    /// queued: the lengths of `sources`, `channels` and `at` differ, a channel is empty, a
    /// source is named twice, or what `native_push_voice` refuses. API @API:push_voices@.
    pub(in crate::game) fn native_push_voices(
        &mut self,
        mod_id: &str,
        sources: &[u128],
        channels: &[Vec<f32>],
        sample_rate: u32,
        at: &[Option<[f64; 3]>],
    ) -> bool {
        if sources.len() != channels.len() || sources.len() != at.len() {
            return false;
        }
        let voices: Vec<(u128, &[f32], Option<[f64; 3]>)> = sources
            .iter()
            .zip(channels)
            .zip(at)
            .map(|((&source, samples), &at)| (source, samples.as_slice(), at))
            .collect();
        self.native_window() && self.audio.push_voices(mod_id, &voices, sample_rate)
    }
    /// Drops what `sources` of the mod have waiting to play (client, `host.clear_voices`):
    /// after a video jumped elsewhere, its old sound does not play on. API @API:push_voices@.
    pub(in crate::game) fn native_clear_voices(&mut self, mod_id: &str, sources: &[u128]) {
        if self.native_window() {
            self.audio.clear_voices(mod_id, sources);
        }
    }

    /// Draws `video`'s current frame on a quad with `corners` in blocks under
    /// `draw`'s pose, in `native_draw_quad`'s order (top left, bottom left,
    /// bottom right, top right as seen from its front), as bright as the
    /// video is whatever the light. False, and nothing drawn, for no such
    /// surface or before its first frame. API @API:world_video@.
    pub(in crate::game) fn native_draw_video(
        &self,
        draw: &mut BlockEntityDraw,
        video: u32,
        corners: [[f32; 3]; 4],
    ) -> bool {
        self.native_draw_video_part(draw, video, corners, [0.0, 0.0, 1.0, 1.0])
    }

    /// Draws a sub-rectangle `uv` ([u0, v0, u1, v1] in 0.0..=1.0) of `video`'s current frame
    /// on a quad with `corners` under `draw`'s pose. Allows drawing curved, cylindrical,
    /// or multi-panel screen meshes. API @API:world_video@.
    pub(in crate::game) fn native_draw_video_part(
        &self,
        draw: &mut BlockEntityDraw,
        video: u32,
        corners: [[f32; 3]; 4],
        uv: [f32; 4],
    ) -> bool {
        let Some((v, (width, height))) = self.video(video).and_then(|v| Some((v, v.shown?))) else {
            return false;
        };
        let sprite = video_slot_sprite(video_slot(v.id));
        let Some(rect) = sprite.and_then(|s| self.gui.atlas.sprites.get(s)).copied() else {
            return false;
        };
        let [tl, bl, br, _tr] = frame_uv(width, height);
        let du = br[0] - tl[0];
        let dv = bl[1] - tl[1];
        let slice_uv = [
            [tl[0] + du * uv[0], tl[1] + dv * uv[1]],
            [tl[0] + du * uv[0], tl[1] + dv * uv[3]],
            [tl[0] + du * uv[2], tl[1] + dv * uv[3]],
            [tl[0] + du * uv[2], tl[1] + dv * uv[1]],
        ];
        draw.bright_quad(corners, slice_uv, rect);
        true
    }

    /// Once a frame: each video's new frame goes to its atlas slot. Leaving
    /// the world for the title closes the videos.
    pub(in crate::game) fn videos_frame(&mut self) {
        let videos = &mut self.extras.videos;
        if videos.slots.iter().all(Option::is_none) {
            return;
        }
        if self.title.is_some() {
            videos.slots = Default::default();
            return;
        }
        let started = Instant::now();
        let atlas = &self.gui.atlas;
        if videos.atlas != atlas.revision {
            // A new atlas has empty slots. A playing video fills its slot with
            // its next frame; a paused one is asked for its frame again by a
            // seek to where it is (its sound is paused too: nothing is lost).
            videos.atlas = atlas.revision;
            for video in videos.slots.iter_mut().flatten() {
                video.shown = None;
                if video.player.paused()
                    && let Some(position) = video.player.position()
                {
                    let _ = video.player.seek(position);
                }
            }
        }
        let (w, h) = (atlas.width as f32, atlas.height as f32);
        for (slot, video) in videos.slots.iter_mut().enumerate() {
            let Some(video) = video else { continue };
            let rect = video_slot_sprite(slot).and_then(|s| atlas.sprites.get(s));
            let (Some(rect), Some(frame)) = (rect, video.player.take_frame()) else {
                continue;
            };
            let (width, height) = (frame.width, frame.height);
            // The size comes with the pixels; a frame that does not match it
            // or the slot is never uploaded.
            if !fits(width, height) || frame.rgba.len() != width as usize * height as usize * 4 {
                log::warn!(
                    "a {width}x{height} video frame of {} bytes was not shown",
                    frame.rgba.len()
                );
                video.player.recycle_frame(frame.rgba);
                continue;
            }
            self.map_patches.push(horizon_client::font::Patch {
                x: (rect.u0 * w).round() as u32,
                y: (rect.v0 * h).round() as u32,
                width,
                height,
                rgba: frame.rgba,
            });
            video.shown = Some((width, height));
        }
        videos.pump_micros = started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_surface_is_no_larger_than_its_slot() {
        assert!(fits(VIDEO_WIDTH, VIDEO_HEIGHT));
        assert!(fits(1, 1));
        assert!(!fits(0, 720));
        assert!(!fits(VIDEO_WIDTH + 1, 720));
        assert!(!fits(1280, VIDEO_HEIGHT + 1));
    }

    #[test]
    fn a_closed_videos_id_names_no_later_video_in_its_slot() {
        let first = video_id(1, 1);
        let next = video_id(1, 2);
        assert_eq!((video_slot(first), video_slot(next)), (1, 1));
        assert_ne!(first, next);
        // No id is a bare slot number, as the ids mods kept before were.
        assert!((0..VIDEO_SLOTS as u32).all(|slot| slot != first && slot != next));
    }

    #[test]
    fn a_frame_is_sampled_inside_its_own_pixels() {
        let [top_left, bottom_left, bottom_right, top_right] = frame_uv(640, 360);
        // Texel centres of the frame's first and last pixels, as slot fractions.
        assert_eq!(
            top_left,
            [0.5 / VIDEO_WIDTH as f32, 0.5 / VIDEO_HEIGHT as f32]
        );
        assert_eq!(
            bottom_right,
            [639.5 / VIDEO_WIDTH as f32, 359.5 / VIDEO_HEIGHT as f32]
        );
        assert_eq!(bottom_left, [top_left[0], bottom_right[1]]);
        assert_eq!(top_right, [bottom_right[0], top_left[1]]);
        // A full-size frame stays inside the slot.
        let [_, _, corner, _] = frame_uv(VIDEO_WIDTH, VIDEO_HEIGHT);
        assert!(corner[0] < 1.0 && corner[1] < 1.0);
    }
}
