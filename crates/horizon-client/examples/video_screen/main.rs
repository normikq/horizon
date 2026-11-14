//! A video screen in the world, as a trait mod (docs/native-libraries.md).
//!
//! Features:
//! - `/video create <id>`: creates a new screen with the specified ID in front of you and opens the interactive editor;
//! - `/video edit <id>`: opens the interactive editor for an existing saved screen;
//! - `/video save [id]`: saves current screen position and dimensions to video_screens.json;
//! - `/video list`: lists all saved screens with clickable buttons;
//! - `/video play <id> <file>`: plays a video on the specified saved screen;
//! - `/video remove <id>`: deletes a saved screen configuration;
//! - `/video cancel`: closes the interactive editor;
//! - Surface snapping:
//!   - `/video snap wall` (or `/video attach`): snaps screen flush onto the targeted block surface/wall;
//!   - `/video grid [step|off]`: toggles block grid snapping (e.g. 0.5 or 1.0 block increments);
//! - Curved screen support:
//!   - `/video curve <degrees>`: sets cylindrical curvature (0° flat, up to 120° for immersive curved/IMAX display);
//! - Interactive editor: red wireframe grid in the 3D world with 4 corner handles and 4 border handles;
//!   drag corners via open chat to resize while preserving aspect ratio; drag borders to adjust width/height freely;
//! - Clickable chat buttons: [💾 Сохранить], [🧲 К стене], [📐 0°], [📐 30°], [📐 60°], [16:9], [4:3], [21:9], [1:1], [Auto], [❌ Закрыть];
//! - Sizing and aspect commands: `/video size <diag>`, `/video grow [step]`, `/video shrink [step]`,
//!   `/video aspect <ratio>`, `/video distance <dist>`, `/video reset`;
//! - Playback commands: `/video open <path>`, `/video pause`, `/video play`, `/video seek <sec>`,
//!   `/video volume <0-100>`, `/video close`, `/video info`.

use horizon_client::mod_api::{
    ClientLevelUnloadEvent, CommandEvent, CommandsEvent, Flow, FrameEvent, GuiLayerPreEvent,
    GuiRenderPostEvent, Hooks, Host, InteractionKeyEvent, KeyEvent, MouseButtonPreEvent,
    MouseScrollingEvent, ScreenMouseButtonPressedPreEvent, ScreenMouseButtonReleasedPreEvent,
    ScreenMouseDraggedPreV2Event, ScreenRenderPostEvent, Text, VisualsEvent, WorldPrimitivesEvent,
    WorldSubmitEvent,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

mod audio;
mod commands;
mod editor;
mod menu;
mod render;
mod storage;
mod types;

use audio::{AudioJob, voices};
pub use types::{AspectRatio, DragHandle, SavedScreen, SpeakerId};
use types::{AudioMode, DragTarget, EditState, MOD_ID, Screen, SpeakerLayout, VIDEO_SIZE};

pub(crate) struct State {
    pub(crate) screen: Option<Screen>,
    pub(crate) editing: Option<EditState>,
    pub(crate) saved_screens: HashMap<String, SavedScreen>,
    pub(crate) demo: Option<String>,
    pub(crate) reported: Option<Instant>,
    pub(crate) screen_size: [f32; 2],
    pub(crate) chat_open: bool,
    /// Optional grid snapping increment in blocks (e.g. Some(0.5)).
    pub(crate) grid_snap: Option<f32>,
    /// Dynamic room lighting enabled (default: true).
    pub(crate) light_enabled: bool,
    /// Light brightness multiplier (default: 1.0, 0.0..=2.0).
    pub(crate) light_brightness: f32,
    /// Smoothed ambient light RGB for transitions.
    pub(crate) current_light_rgb: [f32; 3],
    pub(crate) current_light_intensity: f32,
    /// In-game audio mode (default: 5.1 surround sound).
    pub(crate) audio_mode: AudioMode,
    /// Show virtual 3D speaker boxes and tags in world.
    pub(crate) show_speakers: bool,
    /// Currently hovered target by player's crosshair in first-person
    pub(crate) hovered_target: Option<DragTarget>,
    /// Target actively being dragged by holding Right Mouse Button (ПКМ)
    pub(crate) active_drag: Option<DragTarget>,
    /// The screen as the drag found it, to tell a real drag from a mere click.
    pub(crate) drag_from: Option<DragSnapshot>,
    /// From the crosshair's point to the screen's centre when a centre drag began,
    /// so the screen does not jump to the crosshair.
    pub(crate) drag_offset: [f64; 3],
    /// Previous frame's RMB state to detect initial press transition
    pub(crate) prev_right_down: bool,
    /// Where the saved screens are kept; none until a hook has asked the game.
    pub(crate) screens_path: Option<PathBuf>,
    /// When sound was last taken, for how much to take this frame.
    pub(crate) audio_at: Option<Instant>,
    /// The saved screen the shown one was made from or saved as, if any.
    pub(crate) screen_id: Option<String>,
    /// The file last opened, for playing it again on another screen.
    pub(crate) last_path: Option<String>,
    /// The video's volume as last set, 0 to 100: the player does not report it.
    pub(crate) volume: u8,
    /// The 3D menu.
    pub(crate) menu: menu::MenuState,
}

impl Default for State {
    fn default() -> Self {
        Self {
            screen: None,
            editing: None,
            saved_screens: HashMap::new(),
            demo: None,
            reported: None,
            screen_size: [0.0, 0.0],
            chat_open: false,
            grid_snap: None,
            light_enabled: true,
            light_brightness: 1.0,
            current_light_rgb: [0.0, 0.0, 0.0],
            current_light_intensity: 0.0,
            audio_mode: AudioMode::Surround51,
            show_speakers: false,
            hovered_target: None,
            active_drag: None,
            drag_from: None,
            drag_offset: [0.0; 3],
            prev_right_down: false,
            screens_path: None,
            audio_at: None,
            screen_id: None,
            last_path: None,
            volume: 100,
            menu: menu::MenuState::default(),
        }
    }
}

/// What a drag can change: where the screen stands, its size, a speaker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DragSnapshot {
    at: [f64; 3],
    size: (f32, f32),
    speaker: Option<[f64; 3]>,
}

impl DragSnapshot {
    fn of(screen: &Screen, target: DragTarget, native_size: Option<(u32, u32)>) -> Self {
        Self {
            at: screen.at,
            size: screen.dimensions(native_size),
            speaker: match target {
                DragTarget::Speaker(id, _) => screen.custom_speakers.get(&id).copied(),
                _ => None,
            },
        }
    }

    /// Moved by more than a hundredth of a block, or resized as much.
    fn differs(&self, other: &Self) -> bool {
        let far = |a: [f64; 3], b: [f64; 3]| (0..3).any(|i| (a[i] - b[i]).abs() > 0.01);
        far(self.at, other.at)
            || (self.size.0 - other.size.0).abs() > 0.01
            || (self.size.1 - other.size.1).abs() > 0.01
            || match (self.speaker, other.speaker) {
                (Some(a), Some(b)) => far(a, b),
                (a, b) => a.is_some() != b.is_some(),
            }
    }
}

pub struct VideoScreen(Mutex<State>);

impl Default for VideoScreen {
    fn default() -> Self {
        Self(Mutex::new(State {
            demo: std::env::var("HORIZON_VIDEO_DEMO").ok(),
            ..State::default()
        }))
    }
}

impl VideoScreen {
    /// The mod's state. A panic in an earlier hook leaves it as it was, not
    /// poisoned: the next hook goes on with it rather than failing too.
    pub(crate) fn state(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A frame's work on the state: the screen, the editor, the light. Gives
    /// the sound to take and play, which `frame` does after the lock.
    fn frame_locked(&self, host: &mut dyn Host, state: &mut State) -> Option<AudioJob> {
        storage::ensure_screens(&*host, state);
        menu::frame(self, host, state);
        // A video the game closed (the world was left) is gone: forget its id.
        if let Some(screen) = state.screen.as_mut()
            && let Some(video) = screen.video
            && host.video_state(video).is_none()
        {
            screen.video = None;
            host.clear_voices(MOD_ID, &voices(video));
        }
        if let Some(path) = state.demo.take() {
            match self.open(host, state, &path) {
                Ok(()) => host.log("info", &format!("video: playing {path}")),
                Err(_) if host.camera().is_none() => state.demo = Some(path),
                Err(why) => host.log("warn", &format!("video: {why}")),
            }
        }

        // Automatic aspect ratio detection from video stream metadata
        if let Some(screen) = state.screen.as_mut()
            && screen.aspect == AspectRatio::Auto
            && let Some(video) = screen.video
            && let Some(native_size) = host.video_state(video).and_then(|s| s.video_size)
            && Some(native_size) != screen.applied_native_size
        {
            screen.recalculate(Some(native_size));
            let (w, h) = screen.dimensions(Some(native_size));
            let (nw, nh) = native_size;
            host.log(
                                "info",
                                &format!(
                                    "video: auto-detected video stream resolution {}x{} -> screen adjusted to {:.2}x{:.2} blocks",
                                    nw, nh, w, h
                                ),
                            );
        }

        // The video's sound for the game's mixer: what to take is decided
        // here, the taking and playing happen once the lock is let go.
        let now = Instant::now();
        let elapsed = state
            .audio_at
            .replace(now)
            .map_or(1.0 / 60.0, |at| now.duration_since(at).as_secs_f64());
        let mut audio = None;
        if let Some(screen) = &state.screen
            && let Some(video) = screen.video
            && state.audio_mode != AudioMode::Mute
        {
            let native_size = host.video_state(video).and_then(|s| s.video_size);
            let (w, h) = screen.dimensions(native_size);
            audio = Some(AudioJob {
                video,
                mode: state.audio_mode,
                speakers: SpeakerLayout::compute(
                    screen.at,
                    screen.yaw,
                    w,
                    h,
                    &screen.custom_speakers,
                ),
                elapsed,
            });
        }

        // 3D crosshair raycasting and RMB (ПКМ) dragging of screen handles, center, and speakers
        let right_down = host.has_right_down();
        let prev_right_down = state.prev_right_down;
        state.prev_right_down = right_down;

        let is_editing = state.editing.is_some();
        let show_speakers = state.show_speakers || is_editing;
        let grid_snap = state.grid_snap;
        let audio_mode = state.audio_mode;
        let mut hovered_target = state.hovered_target;
        let mut active_drag = state.active_drag;

        let camera_opt = host.camera();
        if let (Some(camera), Some(screen)) = (camera_opt.as_ref(), state.screen.as_mut()) {
            let ray_dir = editor::crosshair_ray(camera);
            let native_size = screen
                .video
                .and_then(|v| host.video_state(v))
                .and_then(|s| s.video_size);

            if right_down {
                if !prev_right_down && active_drag.is_none() {
                    // Pressed RMB on this frame: grab hovered target
                    if let Some(target) = hovered_target {
                        active_drag = Some(target);
                        state.drag_from = Some(DragSnapshot::of(screen, target, native_size));
                        if let DragTarget::ScreenCenter { initial_dist } = target {
                            let (_, h) = screen.dimensions(native_size);
                            let center = [screen.at[0], screen.at[1] + f64::from(h) * 0.5, screen.at[2]];
                            state.drag_offset = [0, 1, 2].map(|i| {
                                center[i] - (camera.position[i] + ray_dir[i] * initial_dist)
                            });
                        }
                    }
                }

                if let Some(active) = active_drag {
                    match active {
                        DragTarget::Handle(handle) => {
                            if let Some((u, v)) =
                                editor::ray_intersect_screen_plane(screen, camera.position, ray_dir)
                            {
                                editor::apply_drag(
                                    screen,
                                    handle,
                                    u as f32,
                                    v as f32,
                                    native_size,
                                    grid_snap,
                                );
                            }
                        }
                        DragTarget::Speaker(spk_id, dist) => {
                            let mut new_pos = [
                                camera.position[0] + ray_dir[0] * dist,
                                camera.position[1] + ray_dir[1] * dist,
                                camera.position[2] + ray_dir[2] * dist,
                            ];
                            if let Some(step) = grid_snap {
                                let s = f64::from(step);
                                new_pos[0] = (new_pos[0] / s).round() * s;
                                new_pos[1] = (new_pos[1] / s).round() * s;
                                new_pos[2] = (new_pos[2] / s).round() * s;
                            }
                            screen.custom_speakers.insert(spk_id, new_pos);
                        }
                        DragTarget::ScreenCenter { initial_dist } => {
                            let (_w, h) = screen.dimensions(native_size);
                            let offset = state.drag_offset;
                            let new_center = [0, 1, 2].map(|i| {
                                camera.position[i] + ray_dir[i] * initial_dist + offset[i]
                            });
                            let mut new_at = [
                                new_center[0],
                                new_center[1] - f64::from(h) * 0.5,
                                new_center[2],
                            ];
                            if let Some(step) = grid_snap {
                                let s = f64::from(step);
                                new_at[0] = (new_at[0] / s).round() * s;
                                new_at[1] = (new_at[1] / s).round() * s;
                                new_at[2] = (new_at[2] / s).round() * s;
                            }
                            screen.at = new_at;
                        }
                    }
                }
            } else {
                // Released RMB
                let drag_from = state.drag_from.take();
                // A click that moved nothing is no drag: nothing to report.
                if let Some(finished) = active_drag.take()
                    && drag_from.is_none_or(|from| {
                        from.differs(&DragSnapshot::of(screen, finished, native_size))
                    })
                {
                    match finished {
                        DragTarget::Handle(_) => {
                            let (w, h) = screen.dimensions(native_size);
                            host.feedback(Text::colored(
                                format!("§6[Horizon Video] §aРазмер экрана зафиксирован: §b{:.2}x{:.2} бл §7(диагональ {:.2}).", w, h, screen.diagonal),
                                0x55FF55,
                            ));
                        }
                        DragTarget::Speaker(id, _) => {
                            if let Some(pos) = screen.custom_speakers.get(&id) {
                                host.feedback(Text::colored(
                                    format!("§6[Horizon Video] §aПозиция колонки §e{} §aзафиксирована: §7[{:.2}, {:.2}, {:.2}].", id.name(), pos[0], pos[1], pos[2]),
                                    0x55FF55,
                                ));
                            }
                        }
                        DragTarget::ScreenCenter { .. } => {
                            host.feedback(Text::colored(
                                format!("§6[Horizon Video] §aПозиция экрана зафиксирована: §7[{:.2}, {:.2}, {:.2}].", screen.at[0], screen.at[1], screen.at[2]),
                                0x55FF55,
                            ));
                        }
                    }
                }

                // Update hovered target for visual highlighting
                hovered_target = editor::find_crosshair_target(
                    screen,
                    camera.position,
                    ray_dir,
                    is_editing,
                    show_speakers,
                    audio_mode,
                );
            }
        }
        state.active_drag = active_drag;
        state.hovered_target = hovered_target;

        let due = state
            .reported
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(5));
        if let (true, Some(screen)) = (due, &state.screen) {
            host.log("info", &Self::info(host, screen));
            state.reported = Some(Instant::now());
        }
        audio
    }

    pub(crate) fn open(
        &self,
        host: &mut dyn Host,
        state: &mut State,
        path: &str,
    ) -> Result<(), String> {
        let camera = host.camera().ok_or("no world is shown yet")?;
        let video = host.video_open(path, VIDEO_SIZE.0, VIDEO_SIZE.1, true)?;
        if let Some(old) = state.screen.take()
            && let Some(v) = old.video
        {
            audio::stop_video(host, v);
        }
        state.screen = Some(Screen::new(Some(video), camera.position, camera.yaw));
        state.screen_id = None;
        state.last_path = Some(path.to_string());
        host.video_volume(video, state.volume);
        Ok(())
    }

    pub(crate) fn open_on_screen(
        &self,
        host: &mut dyn Host,
        state: &mut State,
        saved: &SavedScreen,
        path: &str,
    ) -> Result<(), String> {
        let video = host.video_open(path, VIDEO_SIZE.0, VIDEO_SIZE.1, true)?;
        if let Some(old) = state.screen.take()
            && let Some(v) = old.video
        {
            audio::stop_video(host, v);
        }
        state.screen = Some(Screen::from_saved(saved, Some(video)));
        state.screen_id = Some(saved.id.clone());
        state.last_path = Some(path.to_string());
        host.video_volume(video, state.volume);
        Ok(())
    }

    pub(crate) fn info(host: &dyn Host, screen: &Screen) -> String {
        let native_size = screen
            .video
            .and_then(|v| host.video_state(v))
            .and_then(|s| s.video_size);
        let (w, h) = screen.dimensions(native_size);
        let aspect_mode = match screen.aspect {
            AspectRatio::Auto => format!("auto ({:.2})", screen.aspect.value(native_size)),
            AspectRatio::Ratio(r) => format!("{:.2}", r),
        };
        let curve_str = if screen.curvature > 0.5 {
            format!(", curve {:.0}°", screen.curvature)
        } else {
            "".to_string()
        };
        let size_str = format!(
            "{:.2}x{:.2} blocks (diag {:.2}, aspect {}{})",
            w, h, screen.diagonal, aspect_mode, curve_str
        );
        let video_info = screen.video.and_then(|v| host.video_state(v));
        match video_info {
            Some(s) => format!(
                "video: {:.1}/{:.1} s, size {}, dist {:.1} blk, {}{}{} frames; render {} us, game thread {} us taking + {} us staging",
                s.position.unwrap_or(0.0),
                s.duration.unwrap_or(0.0),
                size_str,
                screen.distance,
                if s.paused { "paused, " } else { "" },
                if s.failed { "FAILED, " } else { "" },
                s.frames,
                s.render_micros,
                s.pump_micros,
                s.upload_micros,
            ),
            None => format!(
                "screen: size {}, dist {:.1} blk (no video playing)",
                size_str, screen.distance
            ),
        }
    }
}

impl Hooks for VideoScreen {
    fn commands(&self, _: &mut dyn Host, event: &mut CommandsEvent<'_>) {
        commands::register_commands(event);
    }

    fn command(&self, host: &mut dyn Host, event: &mut CommandEvent<'_>) -> Flow {
        commands::handle_command(self, host, event)
    }

    fn frame(&self, host: &mut dyn Host, _: &mut FrameEvent) {
        let audio = self.frame_locked(host, &mut self.state());
        if let Some(audio) = audio {
            audio.play(host);
        }
    }

    /// Once the world is left the screen goes with it; its video is closed
    /// and its sound dropped, the saved screens stay.
    fn client_level_unload(&self, host: &mut dyn Host, _: &mut ClientLevelUnloadEvent<'_>) {
        let mut state = self.state();
        if let Some(video) = state.screen.take().and_then(|s| s.video) {
            audio::stop_video(host, video);
        }
        state.editing = None;
        state.hovered_target = None;
        state.active_drag = None;
        state.audio_at = None;
        state.screen_id = None;
        menu::close(&mut state);
    }

    fn world_submit(&self, host: &dyn Host, event: &mut WorldSubmitEvent<'_>) {
        let state = self.state();
        render::world_submit(&state, host, event);
        menu::render_3d::world_submit(&state, host, event);
    }

    /// `V` while playing opens the 3D menu in front of the player, or closes it.
    fn key(&self, host: &mut dyn Host, event: &mut KeyEvent<'_>) {
        if event.pressed && event.key == menu::MENU_KEY && host.playing() {
            menu::toggle(&mut self.state(), host);
        }
    }

    /// The right button on the menu presses its widgets, not the item in hand.
    fn mouse_button_pre(&self, host: &mut dyn Host, event: &mut MouseButtonPreEvent) -> Flow {
        let mut state = self.state();
        if menu::mouse_button(self, host, &mut state, event.button, event.pressed) {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    /// Belt and braces: no use of the item in hand while the crosshair is on the menu.
    fn interaction_key(&self, _host: &mut dyn Host, event: &mut InteractionKeyEvent<'_>) -> Flow {
        if event.button == 1 && self.state().menu.aiming() {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    /// The wheel over the menu scrolls it, not the hotbar.
    fn mouse_scrolling(&self, host: &mut dyn Host, event: &mut MouseScrollingEvent) -> Flow {
        let steps = event.accumulated[1] as f32;
        let mut state = self.state();
        if steps != 0.0 && menu::scroll(self, host, &mut state, steps) {
            Flow::Cancel
        } else if state.menu.aiming() {
            // Part of a step over the menu: still not the hotbar's.
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    /// Over the menu the vanilla crosshair makes way for the menu's own.
    fn gui_layer_pre(&self, _host: &mut dyn Host, event: &mut GuiLayerPreEvent<'_>) -> Flow {
        if event.layer.ends_with("crosshair") && self.state().menu.aiming() {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    fn gui_render_post(&self, host: &dyn Host, event: &mut GuiRenderPostEvent<'_>) {
        menu::render_3d::gui_render_post(&self.state(), host, event);
    }

    fn world_primitives(&self, host: &dyn Host, event: &mut WorldPrimitivesEvent<'_>) {
        render::world_primitives(&self.state(), host, event);
    }

    fn visuals(&self, host: &dyn Host, event: &mut VisualsEvent<'_>) {
        render::visuals(&mut self.state(), host, event);
    }

    fn screen_render_post(&self, _host: &dyn Host, event: &mut ScreenRenderPostEvent<'_>) {
        let mut state = self.state();
        state.screen_size = [event.screen.rect[2], event.screen.rect[3]];
        state.chat_open = event.screen.id == "chat";
    }

    fn screen_mouse_button_pressed_pre(
        &self,
        host: &dyn Host,
        event: &mut ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        if event.button != 0 {
            return Flow::Continue;
        }
        let mut state = self.state();
        let Some(screen) = &state.screen else {
            return Flow::Continue;
        };
        let Some(camera) = host.camera() else {
            return Flow::Continue;
        };
        let gui_size = [event.screen.rect[2], event.screen.rect[3]];

        if let Some((handle, _)) =
            editor::find_hit_handle(screen, &camera, event.screen.mouse, gui_size)
            && let Some(editing) = state.editing.as_mut()
        {
            editing.active_handle = Some(handle);
            return Flow::Cancel;
        }
        Flow::Continue
    }

    fn screen_mouse_dragged_pre_v2(
        &self,
        host: &mut dyn Host,
        event: &mut ScreenMouseDraggedPreV2Event<'_>,
    ) -> Flow {
        let mut state = self.state();
        let Some(editing) = &state.editing else {
            return Flow::Continue;
        };
        let Some(active_handle) = editing.active_handle else {
            return Flow::Continue;
        };
        let gui_size = state.screen_size;
        let grid_snap = state.grid_snap;
        let Some(screen) = state.screen.as_mut() else {
            return Flow::Continue;
        };
        let Some(camera) = host.camera() else {
            return Flow::Continue;
        };

        if let Some((u, v)) = editor::raycast_screen_plane(screen, &camera, event.mouse, gui_size) {
            let native_size = screen
                .video
                .and_then(|v| host.video_state(v))
                .and_then(|s| s.video_size);
            editor::apply_drag(screen, active_handle, u, v, native_size, grid_snap);
            return Flow::Cancel;
        }
        Flow::Continue
    }

    fn screen_mouse_button_released_pre(
        &self,
        _host: &dyn Host,
        event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
    ) -> Flow {
        if event.button != 0 {
            return Flow::Continue;
        }
        let mut state = self.state();
        if let Some(editing) = state.editing.as_mut()
            && editing.active_handle.take().is_some()
        {
            return Flow::Cancel;
        }
        Flow::Continue
    }
}

#[unsafe(no_mangle)]
pub fn hooks() -> Box<dyn Hooks> {
    let state = State {
        demo: std::env::var("HORIZON_VIDEO_DEMO").ok(),
        ..State::default()
    };
    Box::new(VideoScreen(Mutex::new(state)))
}


// ---------------------------------------------------------------------------
// The example: the horizon-video mod (its api/src, copied by tools/sync_example.py)
// as a game of its own. Change the mod, then sync it again.

fn main() {
    // The game's video backend, as horizon-official registers it for the game.
    if let Err(e) = horizon_hooks::register(&horizon_mpv::hooks::HOOKS) {
        eprintln!("video backend: {e}");
    }
    horizon_client::entry::main_with(vec![hooks()].into());
}
