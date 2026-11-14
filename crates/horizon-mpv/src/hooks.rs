//! libmpv as the game's video backend (`horizon_hooks::video`): horizon-official registers
//! [`HOOKS`], and native mods' `native_video_*` helpers then play through it.

use crate::{Mpv, Options, Player};
use horizon_hooks::video;
use std::path::Path;
use std::sync::Arc;

/// What horizon-official registers.
pub static HOOKS: VideoHooks = VideoHooks;

/// Offers [`Backend`] as the game's video backend.
pub struct VideoHooks;

impl horizon_hooks::Hooks for VideoHooks {
    fn video(&self) -> Option<&dyn video::Backend> {
        Some(&Backend)
    }
}

/// Opens libmpv from the version folder, checked by its SHA-256 ([`Mpv::open_installed`]).
pub struct Backend;

impl video::Backend for Backend {
    fn open(&self, version_dir: &Path, name: &str) -> Result<Arc<dyn video::Library>, String> {
        let mpv = Mpv::open_installed(version_dir, name).map_err(|e| e.to_string())?;
        Ok(Arc::new(mpv))
    }
}

impl video::Library for Mpv {
    fn version(&self) -> String {
        let (major, minor) = self.client_api_version();
        format!("libmpv client API {major}.{minor}")
    }

    fn player(
        &self,
        width: u32,
        height: u32,
        options: &video::Options,
    ) -> Result<Box<dyn video::Player>, String> {
        let options = Options {
            looped: options.looped,
            capture_audio: options.capture_audio,
            ..Options::default()
        };
        let player = Mpv::player(self, width, height, &options).map_err(|e| e.to_string())?;
        Ok(Box::new(player))
    }
}

fn text<T>(result: Result<T, crate::Error>) -> Result<T, String> {
    result.map_err(|e| e.to_string())
}

impl video::Player for Player {
    fn load(&self, path: &Path) -> Result<(), String> {
        text(Player::load(self, path))
    }
    fn set_paused(&self, paused: bool) -> Result<(), String> {
        text(Player::set_paused(self, paused))
    }
    fn seek(&self, seconds: f64) -> Result<(), String> {
        text(Player::seek(self, seconds))
    }
    fn set_volume(&self, volume: u8) -> Result<(), String> {
        text(Player::set_volume(self, volume))
    }
    fn position(&self) -> Option<f64> {
        Player::position(self)
    }
    fn duration(&self) -> Option<f64> {
        Player::duration(self)
    }
    fn paused(&self) -> bool {
        Player::paused(self)
    }
    fn ended(&self) -> bool {
        Player::ended(self)
    }
    fn loaded(&self) -> bool {
        Player::loaded(self)
    }
    fn failed(&self) -> bool {
        Player::failed(self)
    }
    fn size(&self) -> (u32, u32) {
        Player::size(self)
    }
    fn video_size(&self) -> Option<(u32, u32)> {
        Player::video_size(self)
    }
    fn take_frame(&self) -> Option<video::Frame> {
        Player::take_frame(self).map(|f| video::Frame {
            rgba: f.rgba,
            width: f.width,
            height: f.height,
        })
    }
    fn recycle_frame(&self, rgba: Vec<u8>) {
        Player::recycle_frame(self, rgba);
    }
    fn frame_color(&self) -> Option<[u8; 3]> {
        Player::frame_color(self)
    }
    fn frame_colors(&self) -> Option<[[u8; 3]; 3]> {
        Player::frame_colors(self)
    }
    fn stats(&self) -> video::Stats {
        let stats = Player::stats(self);
        video::Stats {
            frames: stats.frames,
            render_micros: stats.render_micros,
        }
    }
    fn audio_sample_rate(&self) -> Option<u32> {
        Player::audio_sample_rate(self)
    }
    fn take_audio(&self, max: usize) -> Option<Vec<f32>> {
        Player::take_audio(self, max)
    }
    fn take_audio_stereo(&self, max: usize) -> Option<(Vec<f32>, Vec<f32>)> {
        Player::take_audio_stereo(self, max)
    }
    fn take_audio_51(&self, max: usize) -> Option<[Vec<f32>; 6]> {
        Player::take_audio_51(self, max)
    }
    fn take_audio_71(&self, max: usize) -> Option<[Vec<f32>; 8]> {
        Player::take_audio_71(self, max)
    }
}
