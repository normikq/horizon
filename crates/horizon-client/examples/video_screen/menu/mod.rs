//! The 3D menu: two panels and a round button floating in the world in front
//! of the player, worked with the crosshair and the right mouse button.
//!
//! The menu neither pauses the game nor opens a screen: the view turns freely
//! and the panels stay where they opened. The crosshair's ray picks a widget;
//! a right click presses it, and holding the button on a slider drags it.
//! The wheel scrolls the list of saved screens, or nudges the slider under
//! the crosshair, instead of the hotbar.
//!
//! - `layout`: the widgets and the hit test, in each panel's own units;
//! - `math`: where the panels stand, and where the crosshair meets them;
//! - `render_3d`: drawing the panels in the world and the crosshair in the HUD.

pub(crate) mod layout;
pub(crate) mod math;
pub(crate) mod render_3d;

use crate::commands;
use crate::editor;
use crate::storage;
use crate::types::{AspectRatio, AudioMode};
use crate::{State, VideoScreen};
use horizon_client::mod_api::{Host, Text};
use layout::{
    CardModel, Layout, MenuModel, PanelId, SliderKind, SliderModel, Status, ToggleKind, WidgetId,
};
use math::{MenuAnchor, PanelPose, UNIT};
use std::collections::HashMap;
use std::time::Instant;

/// The key that opens and closes the menu while playing.
pub(crate) const MENU_KEY: &str = "key.keyboard.v";
/// The menu closes once the player is this far from it, in blocks.
const CLOSE_DISTANCE: f64 = 12.0;
/// Seeking while a slider is dragged happens at most this often, in seconds:
/// every jump makes the player decode afresh.
const SEEK_EVERY: f64 = 0.15;
/// How far before a block in view the menu opens, in blocks.
const WALL_GAP: f64 = 0.35;
/// How far one step of the wheel moves the list, in layout units.
const SCROLL_STEP: f32 = 24.0;
/// How far one step of the wheel moves a slider, of its whole length.
const WHEEL_SLIDER_STEP: f32 = 0.02;

/// The side panels' centres, from the menu's centre, in blocks.
const SIDE_OFFSET: f64 =
    (layout::LEFT_SIZE[0] as f64 * 0.5 + layout::CENTER_SIZE[0] as f64 * 0.5 + 14.0) * UNIT;
/// How far the side panels turn in to face the eye, in degrees.
const TOE_IN: f64 = 18.0;

#[derive(Default)]
pub(crate) struct MenuState {
    pub(crate) open: bool,
    /// Where the menu stands, fixed when it opened.
    pub(crate) anchor: Option<MenuAnchor>,
    /// The dimension it opened in: it closes in any other.
    dimension: String,
    /// The widget under the crosshair.
    pub(crate) hovered: Option<WidgetId>,
    /// The panel under the crosshair and the point on it, widget or not.
    pub(crate) aim: Option<(PanelId, [f32; 2])>,
    /// The slider held with the right button.
    pub(crate) active_slider: Option<SliderKind>,
    /// The right button went down on the menu: its release is the menu's too.
    rmb_captured: bool,
    /// The saved screen whose card is open.
    pub(crate) expanded: Option<String>,
    /// How far the list is scrolled, in layout units.
    pub(crate) scroll: f32,
    /// What is shown, read from the state once a frame.
    pub(crate) model: MenuModel,
    pub(crate) layout: Layout,
    /// Each switch's knob, 0 off to 1 on, moving toward its value.
    pub(crate) knobs: HashMap<ToggleKind, f32>,
    /// The open card's contents fading in, 0 to 1.
    pub(crate) expand_fade: f32,
    /// A widget just pressed and how much of its flash is left, 1 to 0.
    pub(crate) flash: Option<(WidgetId, f32)>,
    last_frame: Option<Instant>,
    /// The drawing, and what it was built from: rebuilt only when that changes.
    pub(crate) display: render_3d::DisplayList,
    display_key: Option<DisplayKey>,
    display_model: Option<MenuModel>,
    /// A seek the slider asked for and not yet made, in seconds.
    pending_seek: Option<f64>,
    last_seek: Option<Instant>,
    /// A saved screen changed by a slider, to write once it is let go.
    saved_dirty: bool,
}

/// Everything the drawing depends on besides the model, animations rounded
/// to what shows. Small and `Copy`: compared every frame without allocating.
#[derive(Clone, Copy, PartialEq)]
struct DisplayKey {
    hovered: Option<WidgetId>,
    flash: Option<(WidgetId, u8)>,
    active_slider: Option<SliderKind>,
    knobs: [u8; 2],
    fade: u8,
}

impl DisplayKey {
    fn of(menu: &MenuState) -> Self {
        let step = |v: f32| (v.clamp(0.0, 1.0) * 64.0).round() as u8;
        let knob = |k| step(menu.knobs.get(&k).copied().unwrap_or(0.0));
        Self {
            hovered: menu.hovered,
            flash: menu.flash.map(|(id, left)| (id, step(left))),
            active_slider: menu.active_slider,
            knobs: [knob(ToggleKind::GridSnap), knob(ToggleKind::AutoAspect)],
            fade: step(menu.expand_fade),
        }
    }
}

/// Rebuilds the drawing if what it shows changed.
fn rebuild_display(state: &mut State, host: &dyn Host) {
    let key = DisplayKey::of(&state.menu);
    let menu = &state.menu;
    let same = menu.display_key == Some(key) && menu.display_model.as_ref() == Some(&menu.model);
    if !same {
        state.menu.display = render_3d::build(&state.menu, host);
        state.menu.display_key = Some(key);
        state.menu.display_model = Some(state.menu.model.clone());
    }
}

impl MenuState {
    /// The crosshair is on the menu, or holds one of its sliders.
    pub(crate) fn aiming(&self) -> bool {
        self.open && (self.aim.is_some() || self.active_slider.is_some())
    }
}

/// The three panels where `anchor` stands them.
pub(crate) fn panels(anchor: &MenuAnchor) -> [(PanelId, PanelPose); 3] {
    let size = |s: [f32; 2]| [f64::from(s[0]), f64::from(s[1])];
    [
        (
            PanelId::Left,
            anchor.panel(-SIDE_OFFSET, 0.0, size(layout::LEFT_SIZE), TOE_IN),
        ),
        (
            PanelId::Center,
            anchor.panel(0.0, 0.0, size(layout::CENTER_SIZE), 0.0),
        ),
        (
            PanelId::Right,
            anchor.panel(SIDE_OFFSET, 0.0, size(layout::RIGHT_SIZE), TOE_IN),
        ),
    ]
}

/// Opens the menu in front of the player, or closes it.
pub(crate) fn toggle(state: &mut State, host: &mut dyn Host) {
    if state.menu.open {
        close(state);
    } else {
        open(state, host);
    }
}

pub(crate) fn open(state: &mut State, host: &mut dyn Host) {
    let Some(camera) = host.camera() else {
        return;
    };
    // A wall or a tree nearer than the menu would cut through it: open it
    // in front of the nearest block it would meet, smaller so it looks the same.
    let mut boxes = HashMap::new();
    let scale = fit_scale(camera.position, camera.yaw, camera.pitch, |pos| {
        boxes
            .entry(pos)
            .or_insert_with(|| host.block_collision(pos))
            .clone()
    });
    let distance = scale * math::MENU_DISTANCE;
    state.menu = MenuState {
        open: true,
        anchor: Some(MenuAnchor::at_distance(
            camera.position,
            camera.yaw,
            camera.pitch,
            distance,
        )),
        dimension: camera.dimension.clone(),
        expanded: state.screen_id.clone(),
        knobs: std::mem::take(&mut state.menu.knobs),
        ..MenuState::default()
    };
    refresh(state, &*host);
    if let Some(id) = state.menu.expanded.clone()
        && let Some(index) = state.menu.model.screens.iter().position(|c| c.id == id)
    {
        state.menu.scroll = layout::scroll_to_card(&state.menu.model, index, 0.0);
        refresh(state, &*host);
    }
}

/// How far the menu may come before it meets a block, as a part of its full
/// distance (1 when nothing is in the way). The menu scales about the eye, so
/// each of a few points of every panel is looked for along its own ray from
/// the eye: the first block with collision on it (`boxes`, in block-local
/// coordinates) stops that ray, `WALL_GAP` before it.
pub(crate) fn fit_scale(
    eye: math::Vec3,
    yaw: f32,
    pitch: f32,
    mut boxes: impl FnMut([i32; 3]) -> Vec<[f64; 6]>,
) -> f64 {
    const STEP: f64 = 0.05;
    let full = MenuAnchor::at_distance(eye, yaw, pitch, math::MENU_DISTANCE);
    let mut scale: f64 = 1.0;
    for (_, pose) in panels(&full) {
        for fx in [0.0, 0.25, 0.5, 0.75, 1.0] {
            for fy in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let point = pose.to_world(pose.size[0] * fx, pose.size[1] * fy, 0.0);
                let ray = math::sub(point, eye);
                let len = math::dot(ray, ray).sqrt();
                if len < 1e-6 {
                    continue;
                }
                let steps = (len / STEP).ceil() as usize;
                for i in 1..=steps {
                    let t = (i as f64 * STEP).min(len);
                    let q = math::add(eye, math::scale(ray, t / len));
                    let block = q.map(|v| v.floor() as i32);
                    let local = [0, 1, 2].map(|k| q[k] - f64::from(block[k]));
                    let inside = boxes(block)
                        .iter()
                        .any(|b| (0..3).all(|k| local[k] >= b[k] && local[k] <= b[k + 3]));
                    if inside {
                        scale = scale.min((t - WALL_GAP) / len);
                        break;
                    }
                }
            }
        }
    }
    scale.clamp(math::MIN_DISTANCE / math::MENU_DISTANCE, 1.0)
}

pub(crate) fn close(state: &mut State) {
    if state.menu.saved_dirty {
        let _ = storage::save_saved_screens(state);
    }
    let knobs = std::mem::take(&mut state.menu.knobs);
    state.menu = MenuState {
        knobs,
        ..MenuState::default()
    };
}

/// Once a frame: follows the crosshair, drags the held slider, animates.
pub(crate) fn frame(video: &VideoScreen, host: &mut dyn Host, state: &mut State) {
    if !state.menu.open {
        return;
    }
    let now = Instant::now();
    let dt = state
        .menu
        .last_frame
        .replace(now)
        .map_or(0.0, |at| now.duration_since(at).as_secs_f32().min(0.1));
    let (Some(camera), Some(anchor)) = (host.camera(), state.menu.anchor) else {
        return;
    };
    let gone = math::sub(camera.position, anchor.origin);
    if camera.dimension != state.menu.dimension || math::dot(gone, gone).sqrt() > CLOSE_DISTANCE {
        close(state);
        return;
    }

    refresh(state, &*host);

    // What the crosshair points at, when nothing covers the world.
    let dir = editor::crosshair_ray(&camera);
    let eye = camera.position;
    let poses = panels(&anchor);
    state.menu.aim = if host.playing() {
        poses
            .iter()
            .filter_map(|(id, pose)| pose.raycast(eye, dir).map(|(p, t)| (*id, p, t)))
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(id, p, _)| (id, [p[0] as f32, p[1] as f32]))
    } else {
        None
    };

    if let Some(kind) = state.menu.active_slider {
        // The held slider follows the crosshair along its panel's plane, past its ends too.
        state.menu.hovered = Some(WidgetId::Slider(kind));
        let right = poses.iter().find(|(id, _)| *id == PanelId::Right).map(|p| p.1);
        if let (Some(pose), Some(widget)) =
            (right, state.menu.layout.widget(WidgetId::Slider(kind)).copied())
            && let Some((p, _)) = pose.project(eye, dir)
        {
            let fraction = layout::slider_fraction(widget.rect, p[0] as f32);
            apply_slider(video, host, state, kind, fraction);
        }
    } else {
        state.menu.hovered = state
            .menu
            .aim
            .and_then(|(panel, p)| layout::hit(&state.menu.layout, panel, p));
    }
    flush_seek(video, host, state, false);

    // Animation: switches slide, an opened card fades in, a press flashes.
    let model = &state.menu.model;
    let targets = [
        (ToggleKind::GridSnap, model.grid_snap),
        (ToggleKind::AutoAspect, model.auto_aspect),
    ];
    for (kind, on) in targets {
        let target = if on { 1.0 } else { 0.0 };
        let knob = state.menu.knobs.entry(kind).or_insert(target);
        *knob = approach(*knob, target, dt * 8.0);
    }
    state.menu.expand_fade = approach(state.menu.expand_fade, 1.0, dt * 7.0);
    if let Some((id, left)) = state.menu.flash {
        let left = left - dt * 4.0;
        state.menu.flash = (left > 0.0).then_some((id, left));
    }
    rebuild_display(state, &*host);
}

/// `value` moved toward `target` by at most `step`.
pub(crate) fn approach(value: f32, target: f32, step: f32) -> f32 {
    if (target - value).abs() <= step {
        target
    } else {
        value + step.copysign(target - value)
    }
}

/// The right button went down or up. True takes the click from the game.
pub(crate) fn mouse_button(
    video: &VideoScreen,
    host: &mut dyn Host,
    state: &mut State,
    button: i32,
    pressed: bool,
) -> bool {
    // GLFW's numbering: 0 left, 1 right.
    if !state.menu.open || button != 1 {
        return false;
    }
    if !pressed {
        let captured = std::mem::take(&mut state.menu.rmb_captured);
        if state.menu.active_slider.take().is_some() {
            flush_seek(video, host, state, true);
            if state.menu.saved_dirty {
                state.menu.saved_dirty = false;
                if let Err(why) = storage::save_saved_screens(state) {
                    host.log("warn", &format!("video: {why}"));
                }
            }
        }
        return captured;
    }
    if !host.playing() || state.menu.aim.is_none() {
        return false;
    }
    state.menu.rmb_captured = true;
    let Some(id) = state.menu.hovered else {
        return true;
    };
    if let Some(kind) = id.slider() {
        if slider(&state.menu.model, kind).is_some_and(|s| s.enabled) {
            state.menu.active_slider = Some(kind);
            if let (Some((PanelId::Right, p)), Some(widget)) =
                (state.menu.aim, state.menu.layout.widget(id).copied())
            {
                let fraction = layout::slider_fraction(widget.rect, p[0]);
                apply_slider(video, host, state, kind, fraction);
            }
        }
    } else {
        state.menu.flash = Some((id, 1.0));
        activate(video, host, state, id);
        refresh(state, &*host);
    }
    true
}

/// The wheel turned `steps` (positive up) while playing. True keeps it from the hotbar.
pub(crate) fn scroll(video: &VideoScreen, host: &mut dyn Host, state: &mut State, steps: f32) -> bool {
    if !state.menu.open {
        return false;
    }
    let Some((panel, _)) = state.menu.aim else {
        return false;
    };
    match state.menu.hovered {
        Some(WidgetId::Slider(kind)) => {
            if let Some(s) = slider(&state.menu.model, kind).filter(|s| s.enabled) {
                let fraction = (s.fraction + steps * WHEEL_SLIDER_STEP).clamp(0.0, 1.0);
                apply_slider(video, host, state, kind, fraction);
                flush_seek(video, host, state, true);
                if std::mem::take(&mut state.menu.saved_dirty) {
                    let _ = storage::save_saved_screens(state);
                }
            }
        }
        _ if panel == PanelId::Right => {
            let max = state.menu.layout.max_scroll;
            state.menu.scroll = (state.menu.scroll - steps * SCROLL_STEP).clamp(0.0, max);
        }
        _ => {}
    }
    refresh(state, &*host);
    true
}

fn slider(model: &MenuModel, kind: SliderKind) -> Option<&SliderModel> {
    model.sliders.iter().find(|s| s.kind == kind)
}

/// Rebuilds what the menu shows from the state.
pub(crate) fn refresh(state: &mut State, host: &dyn Host) {
    let mut model = build_model(state, host);
    let max = layout::max_scroll(&model);
    state.menu.scroll = state.menu.scroll.clamp(0.0, max);
    model.scroll = state.menu.scroll;
    state.menu.layout = layout::layout(&model);
    state.menu.model = model;
}

pub(crate) fn audio_label(mode: AudioMode) -> &'static str {
    match mode {
        AudioMode::Game3D => "3D",
        AudioMode::StereoSpeakers => "Stereo",
        AudioMode::Surround51 => "5.1",
        AudioMode::Surround71 => "7.1",
        AudioMode::Direct => "Direct",
        AudioMode::Mute => "Mute",
    }
}

/// The sound mode after `mode`, as `/video audio` names it.
pub(crate) fn next_audio(mode: AudioMode) -> &'static str {
    match mode {
        AudioMode::Game3D => "stereo",
        AudioMode::StereoSpeakers => "5.1",
        AudioMode::Surround51 => "7.1",
        AudioMode::Surround71 => "direct",
        AudioMode::Direct => "mute",
        AudioMode::Mute => "3d",
    }
}

/// `seconds` as `m:ss`, or `h:mm:ss` from an hour.
pub(crate) fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// A slider's value at `fraction` of its length. Size and distance grow with
/// the square: the small values most screens use get most of the track.
pub(crate) fn slider_value(kind: SliderKind, fraction: f32, duration: f64) -> f64 {
    let t = f64::from(fraction.clamp(0.0, 1.0));
    match kind {
        SliderKind::Volume => (t * 100.0).round(),
        SliderKind::Seek => t * duration.max(0.0),
        SliderKind::Size => 0.5 + 49.5 * t * t,
        SliderKind::Curvature => (t * 120.0).round(),
        SliderKind::Distance => 1.0 + 99.0 * t * t,
    }
}

/// Where `value` sits along the slider, the inverse of `slider_value`.
pub(crate) fn slider_fraction_of(kind: SliderKind, value: f64, duration: f64) -> f32 {
    let t = match kind {
        SliderKind::Volume => value / 100.0,
        SliderKind::Seek if duration > 0.0 => value / duration,
        SliderKind::Seek => 0.0,
        SliderKind::Size => ((value - 0.5) / 49.5).max(0.0).sqrt(),
        SliderKind::Curvature => value / 120.0,
        SliderKind::Distance => ((value - 1.0) / 99.0).max(0.0).sqrt(),
    };
    t.clamp(0.0, 1.0) as f32
}

fn build_model(state: &State, host: &dyn Host) -> MenuModel {
    let video = state.screen.as_ref().and_then(|s| s.video);
    let video_state = video.and_then(|v| host.video_state(v));
    let status = match (&state.screen, &video_state) {
        (None, _) => Status::NoScreen,
        (Some(_), None) => Status::Idle,
        (Some(_), Some(v)) if v.paused || v.ended => Status::Paused,
        (Some(_), Some(_)) => Status::Playing,
    };
    let title = match (&state.screen_id, &state.screen) {
        (Some(id), _) => id.clone(),
        (None, Some(_)) => "Quick screen".to_string(),
        (None, None) => "No screen".to_string(),
    };
    let file = video_state.as_ref().and(state.last_path.as_deref()).map(|path| {
        std::path::Path::new(path)
            .file_name()
            .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
    });

    let mut ids: Vec<&String> = state.saved_screens.keys().collect();
    ids.sort();
    let screens: Vec<CardModel> = ids
        .iter()
        .map(|id| {
            let s = &state.saved_screens[*id];
            let curve = if s.curvature > 0.5 {
                format!(" · {:.0}°", s.curvature)
            } else {
                String::new()
            };
            let audio = s.audio_mode.map_or("", audio_label);
            CardModel {
                id: (*id).clone(),
                detail: format!("{:.1}x{:.1} · {}{curve} · {audio}", s.width, s.height, s.aspect),
                active: state.screen_id.as_ref() == Some(*id),
            }
        })
        .collect();
    let expanded = state
        .menu
        .expanded
        .as_ref()
        .and_then(|id| screens.iter().position(|c| &c.id == id));

    let mut sliders = Vec::new();
    if let Some(index) = expanded {
        let card = &screens[index];
        let live = card.active && state.screen.is_some();
        let playing = live && video_state.is_some();
        let duration = video_state.as_ref().and_then(|v| v.duration).unwrap_or(0.0);
        let position = state
            .menu
            .pending_seek
            .or_else(|| video_state.as_ref().and_then(|v| v.position))
            .unwrap_or(0.0);
        let saved = &state.saved_screens[&card.id];
        let (diagonal, curvature, distance) = match state.screen.as_ref().filter(|_| live) {
            Some(s) => (f64::from(s.diagonal), f64::from(s.curvature), s.distance),
            None => (
                f64::from(saved.diagonal),
                f64::from(saved.curvature),
                saved.distance,
            ),
        };
        for kind in SliderKind::ALL {
            let (value, text, enabled) = match kind {
                SliderKind::Volume => (
                    f64::from(state.volume),
                    format!("{}%", state.volume),
                    playing,
                ),
                SliderKind::Seek => (
                    position,
                    if duration > 0.0 {
                        format!("{} / {}", clock(position), clock(duration))
                    } else {
                        "—".to_string()
                    },
                    playing && duration > 0.0,
                ),
                SliderKind::Size => (diagonal, format!("{diagonal:.1} blk"), true),
                SliderKind::Curvature => (curvature, format!("{curvature:.0}°"), true),
                SliderKind::Distance => (distance, format!("{distance:.1} blk"), true),
            };
            sliders.push(SliderModel {
                kind,
                fraction: slider_fraction_of(kind, value, duration),
                text,
                enabled,
            });
        }
    }

    MenuModel {
        title,
        status,
        file,
        editing: state.editing.is_some(),
        grid_snap: state.grid_snap.is_some(),
        auto_aspect: state
            .screen
            .as_ref()
            .is_none_or(|s| s.aspect == AspectRatio::Auto),
        audio: audio_label(state.audio_mode).to_string(),
        screens,
        expanded,
        sliders,
        scroll: state.menu.scroll,
    }
}

/// Runs `/video <verb> <argument>`, saying in chat why it failed.
fn exec(video: &VideoScreen, host: &mut dyn Host, state: &mut State, verb: &str, argument: &str) -> bool {
    let done = commands::run(video, host, state, verb, argument);
    commands::report(host, state, verb, &done);
    if let Err(why) = &done {
        host.feedback(Text::colored(format!("§c[Horizon Video] {why}"), 0xFF5555));
    }
    done.is_ok()
}

/// Pauses the shown video if it plays, plays it if it is paused.
fn toggle_pause(video: &VideoScreen, host: &mut dyn Host, state: &mut State) -> bool {
    let Some(v) = state.screen.as_ref().and_then(|s| s.video) else {
        return false;
    };
    let paused = host.video_state(v).is_some_and(|s| s.paused || s.ended);
    exec(video, host, state, if paused { "play_resume" } else { "pause" }, "")
}

/// Plays the file last opened on saved screen `id`, or says how to choose one.
fn play_on(video: &VideoScreen, host: &mut dyn Host, state: &mut State, id: &str) {
    match state.last_path.clone() {
        Some(path) => {
            exec(video, host, state, "play", &format!("{id} {path}"));
        }
        None => host.feedback(Text::colored(
            format!("§e[Horizon Video] Нет открытого файла: §b/video play {id} <файл>"),
            0xFFFF55,
        )),
    }
}

/// A widget pressed with the right button.
fn activate(video: &VideoScreen, host: &mut dyn Host, state: &mut State, id: WidgetId) {
    let card_id = |state: &State, i: usize| state.menu.model.screens.get(i).map(|c| c.id.clone());
    match id {
        WidgetId::Close => close(state),
        WidgetId::Configuration => {
            if state.editing.is_some() {
                exec(video, host, state, "cancel", "");
            } else if state.screen.is_some() {
                let id = state.screen_id.clone().unwrap_or_default();
                exec(video, host, state, "edit", &id);
            } else {
                exec(video, host, state, "create", "");
            }
        }
        WidgetId::SnapToWall => {
            exec(video, host, state, "snap", "");
        }
        WidgetId::Save => {
            let id = state.screen_id.clone().unwrap_or_default();
            if exec(video, host, state, "save", &id) {
                state.menu.expanded = state.screen_id.clone();
            }
        }
        WidgetId::Toggle(ToggleKind::GridSnap) => {
            let argument = if state.grid_snap.is_some() { "off" } else { "0.5" };
            exec(video, host, state, "grid", argument);
        }
        WidgetId::Toggle(ToggleKind::AutoAspect) => {
            let auto = state.screen.as_ref().is_some_and(|s| s.aspect == AspectRatio::Auto);
            exec(video, host, state, "aspect", if auto { "16:9" } else { "auto" });
        }
        WidgetId::Toggle(ToggleKind::Audio) => {
            exec(video, host, state, "audio", next_audio(state.audio_mode));
        }
        WidgetId::PlayPause => {
            if !toggle_pause(video, host, state) {
                match state.menu.expanded.clone() {
                    Some(id) => play_on(video, host, state, &id),
                    None => host.feedback(Text::colored(
                        "§e[Horizon Video] Нет видео: §b/video open <файл>§e или откройте карточку экрана справа.",
                        0xFFFF55,
                    )),
                }
            }
        }
        WidgetId::Card(i) => {
            let Some(id) = card_id(state, i) else { return };
            if state.menu.expanded.as_ref() == Some(&id) {
                state.menu.expanded = None;
            } else {
                state.menu.expanded = Some(id);
                state.menu.expand_fade = 0.0;
                refresh(state, &*host);
                state.menu.scroll =
                    layout::scroll_to_card(&state.menu.model, i, state.menu.scroll);
            }
        }
        WidgetId::NewScreen => {
            if exec(video, host, state, "create", "") {
                state.menu.expanded = None;
            }
        }
        WidgetId::CardPlay(i) => {
            let Some(id) = card_id(state, i) else { return };
            let live = state.screen_id.as_ref() == Some(&id);
            if !(live && toggle_pause(video, host, state)) {
                play_on(video, host, state, &id);
            }
        }
        WidgetId::CardEdit(i) => {
            if let Some(id) = card_id(state, i) {
                exec(video, host, state, "edit", &id);
            }
        }
        WidgetId::CardDelete(i) => {
            if let Some(id) = card_id(state, i)
                && exec(video, host, state, "remove", &id)
            {
                state.menu.expanded = None;
            }
        }
        WidgetId::Slider(_) => {}
    }
}

/// Moves slider `kind` of the open card to `fraction` of its length.
fn apply_slider(
    video: &VideoScreen,
    host: &mut dyn Host,
    state: &mut State,
    kind: SliderKind,
    fraction: f32,
) {
    let Some(card) = state.menu.model.expanded.and_then(|i| state.menu.model.screens.get(i)) else {
        return;
    };
    let id = card.id.clone();
    let live = card.active && state.screen.is_some();
    let v = state.screen.as_ref().and_then(|s| s.video);
    let native_size = v.and_then(|v| host.video_state(v)).and_then(|s| s.video_size);
    let duration = v
        .and_then(|v| host.video_state(v))
        .and_then(|s| s.duration)
        .unwrap_or(0.0);
    let value = slider_value(kind, fraction, duration);
    match kind {
        SliderKind::Volume => {
            let volume = value as u8;
            if live && volume != state.volume {
                exec(video, host, state, "volume", &volume.to_string());
            }
        }
        SliderKind::Seek => {
            if live && duration > 0.0 {
                state.menu.pending_seek = Some(value);
            }
        }
        SliderKind::Size | SliderKind::Curvature | SliderKind::Distance => {
            if live {
                if let Some(screen) = state.screen.as_mut() {
                    match kind {
                        SliderKind::Size => screen.diagonal = value as f32,
                        SliderKind::Curvature => screen.curvature = value as f32,
                        _ => {
                            screen.distance = value;
                            screen.at = place_at(screen.camera_pos, screen.yaw, value, screen.at[1]);
                        }
                    }
                    screen.recalculate(native_size);
                }
            } else if let Some(saved) = state.saved_screens.get_mut(&id) {
                match kind {
                    SliderKind::Size => {
                        saved.diagonal = value as f32;
                        let ratio = AspectRatio::parse(&saved.aspect)
                            .unwrap_or(AspectRatio::Auto)
                            .value(None);
                        saved.height = saved.diagonal / (ratio * ratio + 1.0).sqrt();
                        saved.width = saved.height * ratio;
                    }
                    SliderKind::Curvature => saved.curvature = value as f32,
                    _ => {
                        saved.distance = value;
                        saved.at = place_at(saved.camera_pos, saved.yaw, value, saved.at[1]);
                    }
                }
                state.menu.saved_dirty = true;
            }
        }
    }
    refresh(state, &*host);
}

/// A screen's foot `distance` blocks along `yaw` from where it was placed from, at height `y`.
pub(crate) fn place_at(from: [f64; 3], yaw: f32, distance: f64, y: f64) -> [f64; 3] {
    let f = math::yaw_forward(f64::from(yaw));
    [from[0] + f[0] * distance, y, from[2] + f[2] * distance]
}

/// Makes the seek the slider asked for, if it is due or `now`.
fn flush_seek(video: &VideoScreen, host: &mut dyn Host, state: &mut State, now: bool) {
    let Some(seconds) = state.menu.pending_seek else {
        return;
    };
    let due = now
        || state
            .menu
            .last_seek
            .is_none_or(|at| at.elapsed().as_secs_f64() >= SEEK_EVERY);
    if due {
        state.menu.pending_seek = None;
        state.menu.last_seek = Some(Instant::now());
        exec(video, host, state, "seek", &format!("{seconds:.2}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_values_round_trip() {
        for kind in SliderKind::ALL {
            for f in [0.0f32, 0.1, 0.25, 0.5, 0.9, 1.0] {
                let v = slider_value(kind, f, 300.0);
                let back = slider_fraction_of(kind, v, 300.0);
                // Volume and curvature round to whole numbers.
                assert!((back - f).abs() < 0.01, "{kind:?} {f} -> {v} -> {back}");
            }
        }
    }

    #[test]
    fn slider_ranges_match_the_commands() {
        assert_eq!(slider_value(SliderKind::Size, 0.0, 0.0), 0.5);
        assert_eq!(slider_value(SliderKind::Size, 1.0, 0.0), 50.0);
        assert_eq!(slider_value(SliderKind::Distance, 0.0, 0.0), 1.0);
        assert_eq!(slider_value(SliderKind::Distance, 1.0, 0.0), 100.0);
        assert_eq!(slider_value(SliderKind::Curvature, 1.0, 0.0), 120.0);
        assert_eq!(slider_value(SliderKind::Volume, 1.0, 0.0), 100.0);
        assert_eq!(slider_value(SliderKind::Seek, 0.5, 90.0), 45.0);
        assert_eq!(slider_fraction_of(SliderKind::Seek, 10.0, 0.0), 0.0);
    }

    #[test]
    fn audio_modes_cycle_through_all_six() {
        let mut mode = AudioMode::Game3D;
        let mut seen = Vec::new();
        for _ in 0..6 {
            seen.push(audio_label(mode));
            mode = match next_audio(mode) {
                "3d" => AudioMode::Game3D,
                "stereo" => AudioMode::StereoSpeakers,
                "5.1" => AudioMode::Surround51,
                "7.1" => AudioMode::Surround71,
                "direct" => AudioMode::Direct,
                _ => AudioMode::Mute,
            };
        }
        assert_eq!(mode, AudioMode::Game3D);
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 6);
    }

    #[test]
    fn clock_reads_minutes_and_hours() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(83.9), "1:23");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(clock(-5.0), "0:00");
    }

    #[test]
    fn approach_stops_at_the_target() {
        assert_eq!(approach(0.0, 1.0, 0.25), 0.25);
        assert_eq!(approach(0.9, 1.0, 0.25), 1.0);
        assert_eq!(approach(1.0, 0.0, 0.5), 0.5);
    }

    #[test]
    fn a_screen_is_placed_along_its_yaw() {
        let at = place_at([10.0, 64.0, 10.0], 0.0, 4.0, 63.0);
        assert!((at[0] - 10.0).abs() < 1e-9 && (at[2] - 14.0).abs() < 1e-9 && at[1] == 63.0);
    }

    #[test]
    fn nothing_in_the_way_keeps_the_full_size() {
        assert_eq!(fit_scale([0.5, 64.0, 0.5], 0.0, 0.0, |_| Vec::new()), 1.0);
    }

    fn cube_at(z: i32) -> impl FnMut([i32; 3]) -> Vec<[f64; 6]> {
        move |p: [i32; 3]| {
            if p[2] == z {
                vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]
            } else {
                Vec::new()
            }
        }
    }

    #[test]
    fn a_wall_ahead_brings_the_menu_before_it() {
        // A wall of whole blocks at z = 2, the eye at z = 0.5: 1.5 blocks ahead.
        let eye = [0.5, 64.6, 0.5];
        let s = fit_scale(eye, 0.0, 0.0, cube_at(2));
        assert!(s < 1.0);
        let anchor = MenuAnchor::at_distance(eye, 0.0, 0.0, s * math::MENU_DISTANCE);
        // Every panel stays in front of the wall.
        for (_, pose) in panels(&anchor) {
            for (x, y) in [(0.0, 0.0), (pose.size[0], pose.size[1]), (pose.size[0], 0.0)] {
                assert!(pose.to_world(x, y, 0.0)[2] < 2.0, "{s}");
            }
        }
    }

    #[test]
    fn a_block_beside_the_view_shrinks_the_menu_too() {
        // One block only where the left panel's outer top corner would be.
        let eye = [0.5, 64.6, 0.5];
        let anchor = MenuAnchor::in_front(eye, 0.0, 0.0);
        let corner = panels(&anchor)[0].1.to_world(0.0, 0.0, 0.0);
        let hit = corner.map(|v| v.floor() as i32);
        let s = fit_scale(eye, 0.0, 0.0, |p| {
            if p == hit {
                vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]
            } else {
                Vec::new()
            }
        });
        assert!(s < 1.0, "{s}");
    }

    #[test]
    fn the_side_panels_do_not_overlap_the_button() {
        let anchor = MenuAnchor::in_front([0.0, 64.0, 0.0], 0.0, 0.0);
        let [(_, left), (_, center), (_, right)] = panels(&anchor);
        // The centre button's middle is hit by the straight-ahead ray, not the side panels.
        let dir = math::look_dir(0.0, 0.0);
        assert!(center.raycast([0.0, 64.0, 0.0], dir).is_some());
        assert!(left.raycast([0.0, 64.0, 0.0], dir).is_none());
        assert!(right.raycast([0.0, 64.0, 0.0], dir).is_none());
    }
}
