use crate::{State, VideoScreen};
use crate::audio::{self, voices};
use crate::editor;
use crate::storage;
use crate::types::{
    AspectRatio, AudioMode, DEFAULT_DIAGONAL, DEFAULT_DISTANCE, EditState, MOD_ID, SavedScreen,
    Screen,
};
use horizon_client::mod_api::{ClickEvent, CommandEvent, CommandsEvent, Flow, Host, Style, Text};

pub(crate) fn register_commands(event: &mut CommandsEvent<'_>) {
    event.commands.push("video");
}

pub(crate) fn handle_command(
    video: &VideoScreen,
    host: &mut dyn Host,
    event: &mut CommandEvent<'_>,
) -> Flow {
    let Some(rest) = event.command.strip_prefix("video") else {
        return Flow::Continue;
    };
    if !rest.is_empty() && !rest.starts_with(' ') {
        return Flow::Continue;
    }
    let rest = rest.trim();
    let (verb, argument) = rest.split_once(' ').unwrap_or((rest, ""));
    let argument = argument.trim().trim_matches('"');
    let mut state = video.state();
    storage::ensure_screens(&*host, &mut state);
    let done = run(video, host, &mut state, verb, argument);
    report(host, &state, verb, &done);
    Flow::Cancel
}

/// Runs `/video <verb> <argument>` on the state: the commands and the 3D
/// menu share it, so a menu button does exactly what its command does.
pub(crate) fn run(
    video: &VideoScreen,
    host: &mut dyn Host,
    state: &mut State,
    verb: &str,
    argument: &str,
) -> Result<(), String> {
    let done = match verb {
        "ui" | "menu" => {
            crate::menu::toggle(state, host);
            Ok(())
        }
        "create" => (|| -> Result<(), String> {
            let id = if argument.is_empty() {
                format!("screen_{}", state.saved_screens.len() + 1)
            } else {
                argument.to_string()
            };
            let camera = host.camera().ok_or("no world is shown yet")?;
            let mut screen = Screen::new(None, camera.position, camera.yaw);
            screen.recalculate(None);
            editor::send_chat_editor_prompt(host, &id, &screen, state.light_enabled, state.audio_mode, state.show_speakers);
            state.screen = Some(screen);
            state.screen_id = Some(id.clone());
            state.editing = Some(EditState { id, active_handle: None });
            Ok(())
        })(),
        "edit" => {
            let id = if argument.is_empty() {
                state.editing.as_ref().map(|e| e.id.clone()).unwrap_or_else(|| "main".into())
            } else {
                argument.to_string()
            };
            if let Some(saved) = state.saved_screens.get(&id) {
                let mut screen = Screen::from_saved(saved, state.screen.as_ref().and_then(|s| s.video));
                screen.recalculate(None);
                editor::send_chat_editor_prompt(host, &id, &screen, state.light_enabled, state.audio_mode, state.show_speakers);
                state.screen = Some(screen);
                state.screen_id = Some(id.clone());
                state.editing = Some(EditState { id, active_handle: None });
                Ok(())
            } else if let Some(screen) = state.screen.as_ref() {
                editor::send_chat_editor_prompt(host, &id, screen, state.light_enabled, state.audio_mode, state.show_speakers);
                state.editing = Some(EditState { id, active_handle: None });
                Ok(())
            } else {
                Err(format!("screen '{id}' not found: use /video create {id}"))
            }
        },
        "save" => (|| -> Result<(), String> {
            let id = if !argument.is_empty() {
                argument.to_string()
            } else if let Some(e) = &state.editing {
                e.id.clone()
            } else {
                "main".to_string()
            };
            let screen = state.screen.as_mut().ok_or("no screen to save: use /video create <id>")?;
            let native_size = screen.video.and_then(|v| host.video_state(v)).and_then(|s| s.video_size);
            let (w, h) = screen.dimensions(native_size);
            let saved = SavedScreen {
                id: id.clone(),
                at: screen.at,
                camera_pos: screen.camera_pos,
                yaw: screen.yaw,
                diagonal: screen.diagonal,
                aspect: screen.aspect.as_string(),
                distance: screen.distance,
                width: w,
                height: h,
                curvature: screen.curvature,
                speakers: screen.custom_speakers.clone(),
                audio_mode: Some(state.audio_mode),
            };
            state.saved_screens.insert(id.clone(), saved);
            state.screen_id = Some(id.clone());
            storage::save_saved_screens(state)?;
            state.editing = None;
            host.feedback(Text::colored(
                format!("§a[Horizon Video] Экран '§e{}§a' сохранен! ({:.2}x{:.2} бл). Запуск: §b/video play {} <файл>", id, w, h, id),
                0x55FF55,
            ));
            Ok(())
        })(),
        "snap" | "attach" => (|| -> Result<(), String> {
            let s = state.screen.as_mut().ok_or("no screen active: use /video create <id> or /video open <file>")?;
            let camera = host.camera().ok_or("no camera available")?;
            let hit = host.raycast(64.0).ok_or("no block in view within 64 blocks")?;
            let details = editor::attach_to_surface(s, &camera, &hit);
            host.feedback(Text::colored(
                format!("§a[Horizon Video] Экран привязан к поверхности: §e{details}"),
                0x55FF55,
            ));
            Ok(())
        })(),
        "curve" | "curvature" => (|| -> Result<(), String> {
            let s = state.screen.as_mut().ok_or("no screen active")?;
            if argument.is_empty() {
                host.feedback(Text::colored(
                    format!("§a[Horizon Video] Текущее искривление экрана: §e{:.1}°{}", s.curvature, if s.curvature <= 0.1 { " (плоский)" } else { " (цилиндрический)" }),
                    0x55FF55,
                ));
                return Ok(());
            }
            let degrees: f32 = argument
                .parse()
                .map_err(|_| "usage: /video curve <degrees> (0 to 120)")?;
            s.curvature = degrees.clamp(0.0, 120.0);
            let native_size = s.video.and_then(|v| host.video_state(v)).and_then(|st| st.video_size);
            s.recalculate(native_size);
            host.feedback(Text::colored(
                format!("§a[Horizon Video] Искривление экрана: §e{:.1}°{}", s.curvature, if s.curvature <= 0.1 { " (плоский)" } else { " (цилиндрический)" }),
                0x55FF55,
            ));
            Ok(())
        })(),
        "grid" => (|| -> Result<(), String> {
            if argument.eq_ignore_ascii_case("off") {
                state.grid_snap = None;
                host.feedback(Text::colored("§7[Horizon Video] Привязка к сетке выключена.", 0xAAAAAA));
            } else {
                let step: f32 = if argument.is_empty() {
                    0.5
                } else {
                    argument.parse().map_err(|_| "usage: /video grid [step|off] (e.g. 0.5, 1.0, off)")?
                };
                state.grid_snap = Some(step.clamp(0.1, 5.0));
                host.feedback(Text::colored(
                    format!("§a[Horizon Video] Привязка к сетке блоков: §e{:.2} бл.", step),
                    0x55FF55,
                ));
            }
            Ok(())
        })(),
        "cancel" => {
            state.editing = None;
            host.feedback(Text::colored("§7[Horizon Video] Режим редактирования завершен.", 0xAAAAAA));
            Ok(())
        }
        "list" => {
            if state.saved_screens.is_empty() {
                host.feedback(Text::colored("§7[Horizon Video] Нет сохраненных экранов. Создать: §b/video create <id>", 0xAAAAAA));
            } else {
                host.feedback(Text::colored(format!("§6[Horizon Video] Сохраненные экраны ({}):", state.saved_screens.len()), 0xFFAA00));
                for (id, s) in &state.saved_screens {
                    let curve_str = if s.curvature > 0.5 { format!(", curve {:.0}°", s.curvature) } else { "".into() };
                    let mut line = Text::literal(format!("  • §e{}§7: {:.2}x{:.2} бл (диаг {:.2}, {}{}) ", id, s.width, s.height, s.diagonal, s.aspect, curve_str));
                    line = line.append(Text::styled("[✏ Редакт] ", Style {
                        color: Some(0x55FFFF),
                        bold: Some(true),
                        click: Some(ClickEvent::RunCommand(format!("/video edit {id}"))),
                        ..Default::default()
                    }));
                    line = line.append(Text::styled("[🗑 Удалить]", Style {
                        color: Some(0xFF5555),
                        bold: Some(true),
                        click: Some(ClickEvent::RunCommand(format!("/video remove {id}"))),
                        ..Default::default()
                    }));
                    host.feedback(line);
                }
            }
            Ok(())
        }
        "remove" | "delete" => (|| -> Result<(), String> {
            if argument.is_empty() {
                return Err("usage: /video remove <id>".into());
            }
            if state.saved_screens.remove(argument).is_some() {
                if state.screen_id.as_deref() == Some(argument) {
                    state.screen_id = None;
                }
                storage::save_saved_screens(state)?;
                host.feedback(Text::colored(format!("§a[Horizon Video] Экран '{}' удален.", argument), 0x55FF55));
                Ok(())
            } else {
                Err(format!("screen '{argument}' not found"))
            }
        })(),
        "play" => (|| -> Result<(), String> {
            if let Some((id, path)) = argument.split_once(' ') {
                let path = path.trim().trim_matches('"');
                let id = id.trim();
                if let Some(saved) = state.saved_screens.get(id).cloned() {
                    if let Some(mode) = saved.audio_mode {
                        state.audio_mode = mode;
                    }
                    return video.open_on_screen(host, state, &saved, path);
                } else {
                    return Err(format!("экран '{id}' не найден. Сохраните его командой /video save {id}"));
                }
            }
            if let Some(screen) = state.screen.as_mut()
                && let Some(v) = screen.video {
                    host.video_pause(v, false);
                    return Ok(());
                }
            Err("usage: /video play <screen_id> <path> | /video play (unpause)".into())
        })(),
        "open" => video.open(host, state, argument),
        "size" => (|| -> Result<(), String> {
            let s = state.screen.as_mut().ok_or("no screen active: use /video create <id> or /video open <file>")?;
            let diag: f32 = argument
                .parse()
                .map_err(|_| "usage: /video size <diagonal in blocks> (e.g. 5.0)")?;
            if !(0.5..=50.0).contains(&diag) {
                return Err("diagonal must be between 0.5 and 50.0 blocks".into());
            }
            s.diagonal = diag;
            let native_size = s.video.and_then(|v| host.video_state(v)).and_then(|st| st.video_size);
            s.recalculate(native_size);
            Ok(())
        })(),
        "grow" => (|| -> Result<(), String> {
            let s = state.screen.as_mut().ok_or("no screen active")?;
            let step: f32 = if argument.is_empty() {
                0.5
            } else {
                argument
                    .parse()
                    .map_err(|_| "usage: /video grow [step in blocks]")?
            };
            s.diagonal = (s.diagonal + step).clamp(0.5, 50.0);
            let native_size = s.video.and_then(|v| host.video_state(v)).and_then(|st| st.video_size);
            s.recalculate(native_size);
            Ok(())
        })(),
        "shrink" => (|| -> Result<(), String> {
            let s = state.screen.as_mut().ok_or("no screen active")?;
            let step: f32 = if argument.is_empty() {
                0.5
            } else {
                argument
                    .parse()
                    .map_err(|_| "usage: /video shrink [step in blocks]")?
            };
            s.diagonal = (s.diagonal - step).clamp(0.5, 50.0);
            let native_size = s.video.and_then(|v| host.video_state(v)).and_then(|st| st.video_size);
            s.recalculate(native_size);
            Ok(())
        })(),
        "distance" => (|| -> Result<(), String> {
            let s = state.screen.as_mut().ok_or("no screen active")?;
            let dist: f64 = argument
                .parse()
                .map_err(|_| "usage: /video distance <blocks> (e.g. 5.0)")?;
            if !(1.0..=100.0).contains(&dist) {
                return Err("distance must be between 1.0 and 100.0 blocks".into());
            }
            s.distance = dist;
            if let Some(camera) = host.camera() {
                let (_, h) = s.dimensions(s.applied_native_size);
                let (sin, cos) = camera.yaw.to_radians().sin_cos();
                let [x, y, z] = camera.position;
                s.yaw = camera.yaw;
                s.camera_pos = camera.position;
                s.at = [
                    x - f64::from(sin) * dist,
                    y - f64::from(h) / 2.0,
                    z + f64::from(cos) * dist,
                ];
            }
            let native_size = s.video.and_then(|v| host.video_state(v)).and_then(|st| st.video_size);
            s.recalculate(native_size);
            Ok(())
        })(),
        "aspect" => (|| -> Result<(), String> {
            let s = state.screen.as_mut().ok_or("no screen active")?;
            if argument.is_empty() {
                let native_size = s.video.and_then(|v| host.video_state(v)).and_then(|st| st.video_size);
                let (w, h) = s.dimensions(native_size);
                host.feedback(Text::colored(
                    format!("§a[Horizon Video] Текущие пропорции: §e{} ({:.2}x{:.2} бл)", s.aspect.as_string(), w, h),
                    0x55FF55,
                ));
                return Ok(());
            }
            let aspect = AspectRatio::parse(argument)
                .ok_or("usage: /video aspect <16:9 | 4:3 | 21:9 | 1:1 | auto | W:H>")?;
            s.aspect = aspect;
            let native_size = s.video.and_then(|v| host.video_state(v)).and_then(|st| st.video_size);
            s.recalculate(native_size);
            Ok(())
        })(),
        "reset" => (|| -> Result<(), String> {
            let s = state.screen.as_mut().ok_or("no screen active")?;
            match argument.to_ascii_lowercase().as_str() {
                "speakers" | "spk" | "audio" => {
                    s.custom_speakers.clear();
                    host.feedback(Text::colored(
                        "§6[Horizon Video] §aПозиции колонок сброшены к стандартной расстановке.",
                        0x55FF55,
                    ));
                }
                _ => {
                    s.diagonal = DEFAULT_DIAGONAL;
                    s.aspect = AspectRatio::Auto;
                    s.distance = DEFAULT_DISTANCE;
                    s.curvature = 0.0;
                    s.custom_speakers.clear();
                    let native_size = s.video.and_then(|v| host.video_state(v)).and_then(|st| st.video_size);
                    s.recalculate(native_size);
                    host.feedback(Text::colored(
                        "§6[Horizon Video] §aЭкран и колонки сброшены к начальным настройкам.",
                        0x55FF55,
                    ));
                }
            }
            Ok(())
        })(),
        "pause" => (|| -> Result<(), String> {
            let s = state.screen.as_mut().ok_or("no video playing")?;
            let v = s.video.ok_or("no video playing")?;
            host.video_pause(v, true).then_some(()).ok_or("failed".into())
        })(),
        "play_resume" => (|| -> Result<(), String> {
            let s = state.screen.as_mut().ok_or("no video playing")?;
            let v = s.video.ok_or("no video playing")?;
            host.video_pause(v, false).then_some(()).ok_or("failed".into())
        })(),
        "seek" => (|| -> Result<(), String> {
            let s = state.screen.as_mut().ok_or("no video playing")?;
            let v = s.video.ok_or("no video playing")?;
            argument
                .parse()
                .ok()
                .filter(|&seconds| {
                    let jumped = host.video_seek(v, seconds);
                    if jumped {
                        // What the mixer still holds is from before the jump.
                        host.clear_voices(MOD_ID, &voices(v));
                    }
                    jumped
                })
                .map(|_| ())
                .ok_or("usage: /video seek <seconds>".into())
        })(),
        "volume" => (|| -> Result<(), String> {
            let s = state.screen.as_mut().ok_or("no video playing")?;
            let v = s.video.ok_or("no video playing")?;
            argument
                .parse()
                .ok()
                .filter(|&volume| host.video_volume(v, volume))
                .map(|volume: u8| state.volume = volume)
                .ok_or("usage: /video volume <0-100>".into())
        })(),
        "close" => {
            if let Some(s) = state.screen.take()
                && let Some(v) = s.video {
                    audio::stop_video(host, v);
                }
            state.editing = None;
            state.screen_id = None;
            Ok(())
        }
        "light" | "lighting" => (|| -> Result<(), String> {
            if argument.is_empty() {
                let status = if state.light_enabled { "включено" } else { "выключено" };
                let pct = (state.light_brightness * 100.0).round() as u32;
                host.feedback(Text::colored(
                    format!("§6[Horizon Video] §eДинамическое освещение: §b{} §7(яркость §f{}%§7). Используйте §f/video light on|off|<0-200>", status, pct),
                    0xFFAA00,
                ));
                return Ok(());
            }
            match argument.to_ascii_lowercase().as_str() {
                "on" | "true" | "1" | "вкл" => {
                    state.light_enabled = true;
                    host.feedback(Text::colored("§6[Horizon Video] §aДинамическое освещение комнаты включено.", 0x55FF55));
                }
                "off" | "false" | "0" | "выкл" => {
                    state.light_enabled = false;
                    state.current_light_intensity = 0.0;
                    host.feedback(Text::colored("§6[Horizon Video] §cДинамическое освещение комнаты выключено.", 0xFF5555));
                }
                other => {
                    if let Ok(pct) = other.trim_end_matches('%').parse::<f32>() {
                        let b = (pct / 100.0).clamp(0.0, 2.0);
                        state.light_brightness = b;
                        state.light_enabled = b > 0.0;
                        host.feedback(Text::colored(
                            format!("§6[Horizon Video] §eЯркость динамического освещения: §b{:.0}%§e.", b * 100.0),
                            0xFFAA00,
                        ));
                    } else {
                        return Err("используйте /video light on, /video light off или /video light <0-200>".into());
                    }
                }
            }
            Ok(())
        })(),
        "speakers" | "visuals" => {
            let show = match argument.to_ascii_lowercase().as_str() {
                "on" | "1" | "true" | "вкл" => true,
                "off" | "0" | "false" | "выкл" => false,
                _ => !state.show_speakers,
            };
            state.show_speakers = show;
            let status_msg = if show {
                "§aВКЛЮЧЕНА (в комнате отображаются виртуальные 3D-динамики и сабвуфер)."
            } else {
                "§cВЫКЛЮЧЕНА."
            };
            host.feedback(Text::colored(
                format!("§6[Horizon Video] §eВизуализация виртуальных колонок: {status_msg}"),
                0xFFAA00,
            ));
            Ok(())
        },
        "audio" | "sound" => (|| -> Result<(), String> {
            if argument.is_empty() {
                let mode_str = match state.audio_mode {
                    AudioMode::Game3D => "3D (центральный динамик в мире)",
                    AudioMode::StereoSpeakers => "Стерео (левая и правая 3D колонки)",
                    AudioMode::Surround51 => "5.1 Surround (6 виртуальных 3D колонок)",
                    AudioMode::Surround71 => "7.1 IMAX Surround (8 виртуальных 3D колонок)",
                    AudioMode::Direct => "Direct (стерео без 3D-затухания)",
                    AudioMode::Mute => "Без звука (Mute)",
                };
                host.feedback(Text::colored(
                    format!("§6[Horizon Video] §eРежим звука: §b{}§e. Используйте §f/video audio 3d | stereo | 5.1 | 7.1 | direct | mute", mode_str),
                    0xFFAA00,
                ));
                return Ok(());
            }
            match argument.to_ascii_lowercase().as_str() {
                "3d" | "spatial" | "center" => {
                    state.audio_mode = AudioMode::Game3D;
                    host.feedback(Text::colored(
                        "§6[Horizon Video] §aВключен 3D-звук в микшере игры (позиционирование в центре экрана, затухание по расстоянию, приглушение за стенами).",
                        0x55FF55,
                    ));
                }
                "stereo" | "speakers" | "2ch" => {
                    state.audio_mode = AudioMode::StereoSpeakers;
                    host.feedback(Text::colored(
                        "§6[Horizon Video] §bВключены раздельные 3D-динамики (стерео-база по краям экрана с пространственным позиционированием).",
                        0x55FFFF,
                    ));
                }
                "5.1" | "surround" | "surround51" => {
                    state.audio_mode = AudioMode::Surround51;
                    host.feedback(Text::colored(
                        "§6[Horizon Video] §aВключен звук 5.1 Surround (6 виртуальных 3D-колонок: Левая, Правая, Центр, Сабвуфер, Тыловые).",
                        0x55FF55,
                    ));
                }
                "7.1" | "surround71" | "imax" => {
                    state.audio_mode = AudioMode::Surround71;
                    host.feedback(Text::colored(
                        "§6[Horizon Video] §dВключен звук 7.1 IMAX Surround (8 виртуальных 3D-колонок: Фронтальные, Центр, Сабвуфер, Боковые и Задние).",
                        0xFF55FF,
                    ));
                }
                "direct" | "flat" => {
                    state.audio_mode = AudioMode::Direct;
                    host.feedback(Text::colored(
                        "§6[Horizon Video] §eВключен прямой звук игры (без 3D-затухания по расстоянию, через общий микшер игры).",
                        0xFFFF55,
                    ));
                }
                "mute" | "off" | "0" | "выкл" => {
                    state.audio_mode = AudioMode::Mute;
                    host.feedback(Text::colored("§6[Horizon Video] §cЗвук видео выключен.", 0xFF5555));
                }
                _ => {
                    return Err("используйте /video audio 3d | stereo | 5.1 | 7.1 | direct | mute".into());
                }
            }
            Ok(())
        })(),
        "info" => Ok(()),
        _ => Err("usage: /video ui | create <id> | edit <id> | save [id] | play <id> <file> | snap wall | curve <deg> | grid [step|off] | light [on|off|0-200] | audio [3d|stereo|5.1|7.1|direct|mute] | speakers [on|off] | list | remove <id> | cancel | open <file> | size <diag> | grow | shrink | aspect <ratio> | distance <dist> | reset | pause | seek | volume | close | info".into()),
    };
    done
}

/// Logs what a command did, as the game log shows it.
pub(crate) fn report(host: &mut dyn Host, state: &State, verb: &str, done: &Result<(), String>) {
    match (done, &state.screen) {
        (Ok(()), Some(screen)) => match verb {
            "size" | "grow" | "shrink" | "aspect" => {
                let native_size = screen
                    .video
                    .and_then(|v| host.video_state(v))
                    .and_then(|st| st.video_size);
                let (w, h) = screen.dimensions(native_size);
                host.log(
                    "info",
                    &format!(
                        "video: resized to {:.2}x{:.2} blocks (diagonal {:.2})",
                        w, h, screen.diagonal
                    ),
                );
            }
            "distance" => {
                host.log(
                    "info",
                    &format!("video: distance set to {:.1} blocks", screen.distance),
                );
            }
            "reset" => {
                host.log("info", "video: screen reset to default size and distance");
            }
            "info" => host.log("info", &VideoScreen::info(host, screen)),
            _ => {}
        },
        // Only closing says so: the other commands leave no screen to report on.
        (Ok(()), None) if verb == "close" => host.log("info", "video: closed"),
        (Ok(()), None) => {}
        (Err(why), _) => host.log("warn", &format!("video: {why}")),
    }
}
