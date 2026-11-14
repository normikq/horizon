use crate::types::{AudioMode, FALLBACK_RATE, MAX_CHANNELS, MOD_ID, SpeakerLayout};
use horizon_client::mod_api::Host;

/// The first of `video`'s audio sources in the mixer; a channel's is this plus its index.
pub(crate) fn voice_base(video: u32) -> u128 {
    u128::from(video) << 4
}

/// Every audio source `video` may use.
pub(crate) fn voices(video: u32) -> Vec<u128> {
    (0..MAX_CHANNELS).map(|i| voice_base(video) + i).collect()
}

/// Closes `video` and drops what it still had waiting in the mixer.
pub(crate) fn stop_video(host: &mut dyn Host, video: u32) {
    host.video_close(video);
    host.clear_voices(MOD_ID, &voices(video));
}

/// A frame's worth of a video's sound, found under the lock and taken and
/// played after it, so the host calls never run while the lock is held.
pub(crate) struct AudioJob {
    pub(crate) video: u32,
    pub(crate) mode: AudioMode,
    pub(crate) speakers: SpeakerLayout,
    /// Seconds since sound was last taken.
    pub(crate) elapsed: f64,
}

impl AudioJob {
    pub(crate) fn play(self, host: &mut dyn Host) {
        let rate = host.video_audio_rate(self.video).unwrap_or(FALLBACK_RATE);
        // Twice what played since the last frame, and at least a 30 FPS
        // frame's: a slow frame takes its whole share and nothing piles up.
        let elapsed = self.elapsed.clamp(0.0, 0.25);
        let max = ((f64::from(rate) * elapsed * 2.0).ceil() as u32).max(rate / 30);
        let (video, spk) = (self.video, &self.speakers);
        let (channels, places): (Vec<Vec<f32>>, Vec<Option<[f64; 3]>>) = match self.mode {
            AudioMode::Surround71 => match host.video_audio_71(video, max) {
                // FL, FR, FC, LFE, BL, BR, SL, SR
                Some(ch) => (
                    Vec::from(ch),
                    [
                        spk.fl, spk.fr, spk.c, spk.lfe, spk.bl, spk.br, spk.sl, spk.sr,
                    ]
                    .map(Some)
                    .to_vec(),
                ),
                None => return,
            },
            AudioMode::Surround51 => match host.video_audio_51(video, max) {
                // FL, FR, FC, LFE, SL, SR
                Some(ch) => (
                    Vec::from(ch),
                    [spk.fl, spk.fr, spk.c, spk.lfe, spk.sl, spk.sr]
                        .map(Some)
                        .to_vec(),
                ),
                None => return,
            },
            AudioMode::StereoSpeakers => match host.video_audio_stereo(video, max) {
                Some((left, right)) => (vec![left, right], vec![Some(spk.fl), Some(spk.fr)]),
                None => return,
            },
            AudioMode::Game3D => match host.video_audio(video, max) {
                Some(mono) => (vec![mono], vec![Some(spk.c)]),
                None => return,
            },
            AudioMode::Direct => match host.video_audio(video, max) {
                Some(mono) => (vec![mono], vec![None]),
                None => return,
            },
            AudioMode::Mute => return,
        };
        // Whatever number of channels came, each has its own source and place;
        // all are queued together so they stay lined up.
        let count = channels.len().min(places.len());
        let sources: Vec<u128> = (0..count as u128).map(|i| voice_base(video) + i).collect();
        host.push_voices(MOD_ID, &sources, &channels[..count], rate, &places[..count]);
    }
}
