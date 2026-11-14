use std::collections::HashMap;

pub(crate) const SCREENS_FILE: &str = "video_screens.json";
/// The video surface's size in pixels: 4K, the largest a video slot takes.
pub(crate) const VIDEO_SIZE: (u32, u32) = (3840, 2160);
/// The mod's id, as the mixer keys its audio sources by.
pub(crate) const MOD_ID: &str = "horizon_video";
/// The sample rate when the game does not say (`video_audio_rate`).
pub(crate) const FALLBACK_RATE: u32 = 48_000;
/// The most a video may have in its 7.1 layout: one audio source each.
pub(crate) const MAX_CHANNELS: u128 = 8;
pub(crate) const CURVE_SLICES: usize = 24;

/// The default screen diagonal in blocks (~4.589 blocks corresponds to 4.0 x 2.25 at 16:9).
pub(crate) const DEFAULT_DIAGONAL: f32 = 4.58939;
/// Default aspect ratio (16:9).
pub(crate) const DEFAULT_ASPECT: f32 = 16.0 / 9.0;
pub(crate) const DEFAULT_HEIGHT: f32 = 2.25;
/// How far in front of the player it is put by default.
pub(crate) const DEFAULT_DISTANCE: f64 = 4.0;

/// Aspect ratio for the screen geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AspectRatio {
    Auto,
    Ratio(f32),
}

impl AspectRatio {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("auto") {
            return Some(Self::Auto);
        }
        if let Some((w, h)) = s.split_once(':') {
            let w: f32 = w.trim().parse().ok()?;
            let h: f32 = h.trim().parse().ok()?;
            if w > 0.0 && h > 0.0 {
                return Some(Self::Ratio(w / h));
            }
        } else if let Ok(val) = s.parse::<f32>()
            && val > 0.0
        {
            return Some(Self::Ratio(val));
        }
        None
    }

    pub fn value(&self, native_size: Option<(u32, u32)>) -> f32 {
        match self {
            Self::Auto => {
                if let Some((w, h)) = native_size
                    && h > 0
                {
                    return w as f32 / h as f32;
                }
                DEFAULT_ASPECT
            }
            Self::Ratio(r) => *r,
        }
    }

    pub fn as_string(&self) -> String {
        match self {
            Self::Auto => "auto".to_string(),
            Self::Ratio(r) => {
                if (*r - 16.0 / 9.0).abs() < 0.02 {
                    "16:9".to_string()
                } else if (*r - 4.0 / 3.0).abs() < 0.02 {
                    "4:3".to_string()
                } else if (*r - 21.0 / 9.0).abs() < 0.02 {
                    "21:9".to_string()
                } else if (*r - 1.0).abs() < 0.02 {
                    "1:1".to_string()
                } else {
                    format!("{:.2}", r)
                }
            }
        }
    }
}

/// A screen configuration saved on disk.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SavedScreen {
    pub id: String,
    pub at: [f64; 3],
    pub camera_pos: [f64; 3],
    pub yaw: f32,
    pub diagonal: f32,
    pub aspect: String,
    pub distance: f64,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub curvature: f32,
    #[serde(default)]
    pub speakers: HashMap<SpeakerId, [f64; 3]>,
    /// The audio mode it plays with; none in files from before it was kept.
    #[serde(default)]
    pub audio_mode: Option<AudioMode>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragHandle {
    CornerTL,
    CornerTR,
    CornerBL,
    CornerBR,
    EdgeLeft,
    EdgeRight,
    EdgeTop,
    EdgeBottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SpeakerId {
    FL,
    FR,
    C,
    LFE,
    SL,
    SR,
    BL,
    BR,
}

impl SpeakerId {
    pub fn name(self) -> &'static str {
        match self {
            Self::FL => "Левый фронт (FL)",
            Self::FR => "Правый фронт (FR)",
            Self::C => "Центральный (C)",
            Self::LFE => "Сабвуфер (LFE)",
            Self::SL => "Боковой/тыловой левый (SL)",
            Self::SR => "Боковой/тыловой правый (SR)",
            Self::BL => "Задний левый (BL)",
            Self::BR => "Задний правый (BR)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragTarget {
    Handle(DragHandle),
    ScreenCenter { initial_dist: f64 },
    Speaker(SpeakerId, f64),
}

pub struct EditState {
    pub id: String,
    pub active_handle: Option<DragHandle>,
}

pub struct Screen {
    pub(crate) video: Option<u32>,
    pub(crate) camera_pos: [f64; 3],
    pub(crate) yaw: f32,
    pub(crate) diagonal: f32,
    pub(crate) aspect: AspectRatio,
    pub(crate) distance: f64,
    /// Cylindrical curvature in degrees (0.0 = flat, up to 120.0).
    pub curvature: f32,
    pub(crate) applied_native_size: Option<(u32, u32)>,
    pub(crate) at: [f64; 3],
    pub(crate) corners: [[f32; 3]; 4],
    pub custom_speakers: HashMap<SpeakerId, [f64; 3]>,
}

impl Screen {
    pub fn new(video: Option<u32>, camera_pos: [f64; 3], yaw: f32) -> Self {
        let (sin, cos) = yaw.to_radians().sin_cos();
        let [x, y, z] = camera_pos;
        let mut screen = Self {
            video,
            camera_pos,
            yaw,
            diagonal: DEFAULT_DIAGONAL,
            aspect: AspectRatio::Auto,
            distance: DEFAULT_DISTANCE,
            curvature: 0.0,
            applied_native_size: None,
            at: [
                x - f64::from(sin) * DEFAULT_DISTANCE,
                y - f64::from(DEFAULT_HEIGHT) / 2.0,
                z + f64::from(cos) * DEFAULT_DISTANCE,
            ],
            corners: [[0.0; 3]; 4],
            custom_speakers: HashMap::new(),
        };
        screen.recalculate(None);
        screen
    }

    pub fn from_saved(saved: &SavedScreen, video: Option<u32>) -> Self {
        let mut screen = Self {
            video,
            camera_pos: saved.camera_pos,
            yaw: saved.yaw,
            diagonal: saved.diagonal,
            aspect: AspectRatio::parse(&saved.aspect).unwrap_or(AspectRatio::Auto),
            distance: saved.distance,
            curvature: saved.curvature,
            applied_native_size: None,
            at: saved.at,
            corners: [[0.0; 3]; 4],
            custom_speakers: saved.speakers.clone(),
        };
        screen.recalculate(None);
        screen
    }

    pub fn dimensions(&self, native_size: Option<(u32, u32)>) -> (f32, f32) {
        let r = self.aspect.value(native_size);
        let height = self.diagonal / (r * r + 1.0).sqrt();
        let width = r * height;
        (width, height)
    }

    pub fn recalculate(&mut self, native_size: Option<(u32, u32)>) {
        self.applied_native_size = native_size;
        let (width, height) = self.dimensions(native_size);
        let (sin, cos) = self.yaw.to_radians().sin_cos();

        let right = [-cos, -sin];
        let side = |sign: f32, y_val: f32| {
            [
                right[0] * sign * width / 2.0,
                y_val,
                right[1] * sign * width / 2.0,
            ]
        };

        self.corners = [
            side(-1.0, height),
            side(-1.0, 0.0),
            side(1.0, 0.0),
            side(1.0, height),
        ];
    }

    /// Computes mesh slices for curved or flat screen rendering.
    /// Each slice returns `(corners, uv)` where `corners` are in world-pose relative to `self.at`.
    pub fn slices(&self, native_size: Option<(u32, u32)>) -> Vec<([[f32; 3]; 4], [f32; 4])> {
        let (width, height) = self.dimensions(native_size);
        if self.curvature <= 0.5 {
            return vec![(self.corners, [0.0, 0.0, 1.0, 1.0])];
        }

        let (sin, cos) = self.yaw.to_radians().sin_cos();
        let right = [-cos, -sin];
        let normal = [sin, -cos]; // Normal facing towards camera

        let alpha = self.curvature.to_radians();
        let radius = width / alpha;
        let n = CURVE_SLICES;
        let mut slices = Vec::with_capacity(n);

        let pt_at = |x_rel: f32, y_val: f32, z_rel: f32| -> [f32; 3] {
            [
                right[0] * x_rel + normal[0] * z_rel,
                y_val,
                right[1] * x_rel + normal[1] * z_rel,
            ]
        };

        for i in 0..n {
            let f0 = i as f32 / n as f32;
            let f1 = (i + 1) as f32 / n as f32;
            let theta0 = -alpha * 0.5 + alpha * f0;
            let theta1 = -alpha * 0.5 + alpha * f1;

            let x0 = radius * theta0.sin();
            let z0 = radius * (1.0 - theta0.cos());
            let x1 = radius * theta1.sin();
            let z1 = radius * (1.0 - theta1.cos());

            let corners = [
                pt_at(x0, height, z0),
                pt_at(x0, 0.0, z0),
                pt_at(x1, 0.0, z1),
                pt_at(x1, height, z1),
            ];
            slices.push((corners, [f0, 0.0, f1, 1.0]));
        }

        slices
    }
}

/// Calculated positions for 3D surround sound speakers in the room.
#[derive(Clone, Copy, Debug)]
pub struct SpeakerLayout {
    pub fl: [f64; 3],
    pub fr: [f64; 3],
    pub c: [f64; 3],
    pub lfe: [f64; 3],
    pub sl: [f64; 3],
    pub sr: [f64; 3],
    pub bl: [f64; 3],
    pub br: [f64; 3],
}

impl SpeakerLayout {
    pub fn compute(
        screen_at: [f64; 3],
        yaw: f32,
        width: f32,
        height: f32,
        custom: &HashMap<SpeakerId, [f64; 3]>,
    ) -> Self {
        let (sin, cos) = yaw.to_radians().sin_cos();
        let right = [f64::from(-cos), 0.0, f64::from(-sin)];
        let normal = [f64::from(sin), 0.0, f64::from(-cos)];
        let up = [0.0, 1.0, 0.0];

        let hw = f64::from(width) * 0.5;
        let h = f64::from(height);
        // Room depth estimated from screen width
        let room_depth = (hw * 2.4).clamp(5.0, 18.0);

        let pt = |r: f64, u: f64, n: f64| -> [f64; 3] {
            [
                screen_at[0] + right[0] * r + up[0] * u + normal[0] * n,
                screen_at[1] + right[1] * r + up[1] * u + normal[1] * n,
                screen_at[2] + right[2] * r + up[2] * u + normal[2] * n,
            ]
        };

        // Front Center: behind screen center at dialogue level
        let mut c = pt(0.0, h * 0.45, -0.15);
        // Front Left: outside left screen edge, ear level
        let mut fl = pt(-(hw + 0.6), h * 0.4, 0.2);
        // Front Right: outside right screen edge, ear level
        let mut fr = pt(hw + 0.6, h * 0.4, 0.2);
        // Subwoofer (LFE): on floor near front corner
        let mut lfe = pt(-(hw * 0.7), 0.25, 0.4);
        // Side Left: along left room wall at viewer row height
        let mut sl = pt(-(hw + 1.8), 1.5, room_depth * 0.5);
        // Side Right: along right room wall at viewer row height
        let mut sr = pt(hw + 1.8, 1.5, room_depth * 0.5);
        // Back Left: rear room wall
        let mut bl = pt(-(hw + 0.8), 1.7, room_depth * 0.95);
        // Back Right: rear room wall
        let mut br = pt(hw + 0.8, 1.7, room_depth * 0.95);

        if let Some(&p) = custom.get(&SpeakerId::FL) {
            fl = p;
        }
        if let Some(&p) = custom.get(&SpeakerId::FR) {
            fr = p;
        }
        if let Some(&p) = custom.get(&SpeakerId::C) {
            c = p;
        }
        if let Some(&p) = custom.get(&SpeakerId::LFE) {
            lfe = p;
        }
        if let Some(&p) = custom.get(&SpeakerId::SL) {
            sl = p;
        }
        if let Some(&p) = custom.get(&SpeakerId::SR) {
            sr = p;
        }
        if let Some(&p) = custom.get(&SpeakerId::BL) {
            bl = p;
        }
        if let Some(&p) = custom.get(&SpeakerId::BR) {
            br = p;
        }

        Self {
            fl,
            fr,
            c,
            lfe,
            sl,
            sr,
            bl,
            br,
        }
    }
}

/// Audio output modes for screen playback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AudioMode {
    /// 3D Spatial Audio: positioned at the screen center with 3D HRTF panning,
    /// distance attenuation up to 48 blocks, and wall occlusion muffling.
    Game3D,
    /// Dual 3D Speakers: separate left and right spatial emitters placed at the
    /// edges of wide cinema screens for realistic room stereo separation.
    StereoSpeakers,
    /// 5.1 Surround Sound: 6 discrete/upmixed 3D speaker sources in world:
    /// FL (Front Left), FR (Front Right), C (Center screen), LFE (Subwoofer),
    /// SL (Surround Left), SR (Surround Right).
    Surround51,
    /// 7.1 Surround Sound: 8 discrete/upmixed 3D speaker sources in world:
    /// FL, FR, C, LFE, BL (Back Left), BR (Back Right), SL (Side Left), SR (Side Right).
    Surround71,
    /// Non-spatial direct audio routed through Horizon's game mixer (controlled by
    /// in-game master volume and pause menu attenuation).
    Direct,
    /// Mute audio.
    Mute,
}
