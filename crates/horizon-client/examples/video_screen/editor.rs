use crate::types::{
    AspectRatio, AudioMode, DragHandle, DragTarget, Screen, SpeakerId, SpeakerLayout,
};
use horizon_client::mod_api::{ClickEvent, Host, NativeCamera, NativeHit, Style, Text};

pub(crate) fn send_chat_editor_prompt(
    host: &mut dyn Host,
    id: &str,
    screen: &Screen,
    light_enabled: bool,
    audio_mode: AudioMode,
    show_speakers: bool,
) {
    let (w, h) = screen.dimensions(screen.applied_native_size);
    let curve_info = if screen.curvature > 0.5 {
        format!(" | изгиб {:.0}°", screen.curvature)
    } else {
        "".to_string()
    };

    host.feedback(Text::colored(
        format!(
            "§6[Horizon Video] §eРежим настройки экрана: §b{id} §7({:.2}x{:.2} бл{})",
            w, h, curve_info
        ),
        0xFFAA00,
    ));
    host.feedback(Text::colored(
        "§eУправление в мире: §fНаведите прицел на угол экрана или колонку и §6зажмите ПКМ§f, чтобы перемещать в 3D!",
        0xFFFF55,
    ));
    host.feedback(Text::colored(
        "§7(Тяните за угол — размер экрана; за край — ширина/высота; за центр — экран целиком; за колонку — перенос звука).",
        0xAAAAAA,
    ));

    // Row 1: Actions & Snapping
    let mut row1 = Text::literal("  ");
    row1 = row1.append(Text::styled(
        "[💾 Сохранить] ",
        Style {
            color: Some(0x55FF55),
            bold: Some(true),
            click: Some(ClickEvent::RunCommand(format!("/video save {id}"))),
            ..Default::default()
        },
    ));
    row1 = row1.append(Text::styled(
        "[🧲 К стене] ",
        Style {
            color: Some(0xFFAA00),
            bold: Some(true),
            click: Some(ClickEvent::RunCommand("/video snap wall".into())),
            ..Default::default()
        },
    ));
    row1 = row1.append(Text::styled(
        "[📐 0°] ",
        Style {
            color: Some(0xAAAAAA),
            bold: Some(true),
            click: Some(ClickEvent::RunCommand("/video curve 0".into())),
            ..Default::default()
        },
    ));
    row1 = row1.append(Text::styled(
        "[📐 30°] ",
        Style {
            color: Some(0xFFFF55),
            bold: Some(true),
            click: Some(ClickEvent::RunCommand("/video curve 30".into())),
            ..Default::default()
        },
    ));
    row1 = row1.append(Text::styled(
        "[📐 60°] ",
        Style {
            color: Some(0xFFFF55),
            bold: Some(true),
            click: Some(ClickEvent::RunCommand("/video curve 60".into())),
            ..Default::default()
        },
    ));
    row1 = row1.append(Text::styled(
        "[❌ Закрыть]",
        Style {
            color: Some(0xFF5555),
            bold: Some(true),
            click: Some(ClickEvent::RunCommand("/video cancel".into())),
            ..Default::default()
        },
    ));
    host.feedback(row1);

    // Row 2: Aspect Ratios, Lighting, Audio
    let mut row2 = Text::literal("  §7Пропорции: ");
    row2 = row2.append(Text::styled(
        "[16:9] ",
        Style {
            color: Some(0x55FFFF),
            bold: Some(true),
            click: Some(ClickEvent::RunCommand("/video aspect 16:9".into())),
            ..Default::default()
        },
    ));
    row2 = row2.append(Text::styled(
        "[4:3] ",
        Style {
            color: Some(0x55FFFF),
            bold: Some(true),
            click: Some(ClickEvent::RunCommand("/video aspect 4:3".into())),
            ..Default::default()
        },
    ));
    row2 = row2.append(Text::styled(
        "[21:9] ",
        Style {
            color: Some(0x55FFFF),
            bold: Some(true),
            click: Some(ClickEvent::RunCommand("/video aspect 21:9".into())),
            ..Default::default()
        },
    ));
    row2 = row2.append(Text::styled(
        "[1:1] ",
        Style {
            color: Some(0x55FFFF),
            bold: Some(true),
            click: Some(ClickEvent::RunCommand("/video aspect 1:1".into())),
            ..Default::default()
        },
    ));
    row2 = row2.append(Text::styled(
        "[Auto]",
        Style {
            color: Some(0xFFFF55),
            bold: Some(true),
            click: Some(ClickEvent::RunCommand("/video aspect auto".into())),
            ..Default::default()
        },
    ));
    let (light_text, light_col, light_cmd) = if light_enabled {
        (" [💡 Свет: Вкл]", 0xFFFF55, "/video light off")
    } else {
        (" [💡 Свет: Выкл]", 0x888888, "/video light on")
    };
    row2 = row2.append(Text::styled(
        light_text,
        Style {
            color: Some(light_col),
            bold: Some(true),
            click: Some(ClickEvent::RunCommand(light_cmd.into())),
            ..Default::default()
        },
    ));

    let (audio_text, audio_col, audio_cmd) = match audio_mode {
        AudioMode::Game3D => (" [🔊 3D]", 0x55FF55, "/video audio stereo"),
        AudioMode::StereoSpeakers => (" [🔊 Стерео]", 0x55FFFF, "/video audio 5.1"),
        AudioMode::Surround51 => (" [🔊 5.1]", 0xFFAA00, "/video audio 7.1"),
        AudioMode::Surround71 => (" [🔊 7.1]", 0xFF55FF, "/video audio direct"),
        AudioMode::Direct => (" [🔊 Direct]", 0xFFFF55, "/video audio mute"),
        AudioMode::Mute => (" [🔇 Выкл]", 0xFF5555, "/video audio 3d"),
    };
    row2 = row2.append(Text::styled(
        audio_text,
        Style {
            color: Some(audio_col),
            bold: Some(true),
            click: Some(ClickEvent::RunCommand(audio_cmd.into())),
            ..Default::default()
        },
    ));

    let (speakers_text, speakers_col) = if show_speakers {
        (" [🔈 Колонки: ВКЛ]", 0x55FF55)
    } else {
        (" [🔈 Колонки: ВЫКЛ]", 0x888888)
    };
    row2 = row2.append(Text::styled(
        speakers_text,
        Style {
            color: Some(speakers_col),
            bold: Some(true),
            click: Some(ClickEvent::RunCommand("/video speakers".into())),
            ..Default::default()
        },
    ));
    host.feedback(row2);
}

pub(crate) fn attach_to_surface(
    screen: &mut Screen,
    camera: &NativeCamera,
    hit: &NativeHit,
) -> String {
    let (_scr_w, scr_h) = screen.dimensions(screen.applied_native_size);
    let bx = hit.block_x as f64;
    let by = hit.block_y as f64;
    let bz = hit.block_z as f64;

    let d_west = (hit.x - bx).abs();
    let d_east = (hit.x - (bx + 1.0)).abs();
    let d_north = (hit.z - bz).abs();
    let d_south = (hit.z - (bz + 1.0)).abs();
    let d_down = (hit.y - by).abs();
    let d_up = (hit.y - (by + 1.0)).abs();

    let min_d = d_west
        .min(d_east)
        .min(d_north)
        .min(d_south)
        .min(d_down)
        .min(d_up);
    let offset = 0.02; // Small gap to prevent z-fighting with block surface

    let (normal, yaw, face_name) = if min_d == d_west {
        ([-1.0, 0.0, 0.0], 270.0, "западная стена (West)")
    } else if min_d == d_east {
        ([1.0, 0.0, 0.0], 90.0, "восточная стена (East)")
    } else if min_d == d_north {
        ([0.0, 0.0, -1.0], 0.0, "северная стена (North)")
    } else if min_d == d_south {
        ([0.0, 0.0, 1.0], 180.0, "южная стена (South)")
    } else if min_d == d_up {
        // Looking at floor: place facing player's horizontal yaw
        (
            [0.0, 1.0, 0.0],
            (camera.yaw / 90.0).round() * 90.0,
            "пол (Floor)",
        )
    } else {
        (
            [0.0, -1.0, 0.0],
            (camera.yaw / 90.0).round() * 90.0,
            "потолок (Ceiling)",
        )
    };

    screen.yaw = yaw;
    if min_d == d_up {
        // Standing on floor
        screen.at = [hit.x, hit.y + offset, hit.z];
    } else if min_d == d_down {
        // Hanging from ceiling
        screen.at = [hit.x, hit.y - f64::from(scr_h) - offset, hit.z];
    } else {
        // Mounted flush on wall
        let wall_x = hit.x + normal[0] * offset;
        let wall_y = hit.y - f64::from(scr_h) * 0.5;
        let wall_z = hit.z + normal[2] * offset;
        screen.at = [wall_x, wall_y, wall_z];
    }

    // Keep distance and camera_pos consistent
    let dx = camera.position[0] - screen.at[0];
    let dy = camera.position[1] - (screen.at[1] + f64::from(scr_h) * 0.5);
    let dz = camera.position[2] - screen.at[2];
    screen.distance = (dx * dx + dy * dy + dz * dz).sqrt().clamp(1.0, 100.0);
    screen.camera_pos = camera.position;
    screen.recalculate(screen.applied_native_size);

    format!(
        "блок {} [{:.0}, {:.0}, {:.0}] - {}",
        hit.block, bx, by, bz, face_name
    )
}

pub(crate) fn raycast_screen_plane(
    screen: &Screen,
    camera: &NativeCamera,
    mouse: [f32; 2],
    gui_size: [f32; 2],
) -> Option<(f32, f32)> {
    let w = gui_size[0].max(1.0);
    let h = gui_size[1].max(1.0);
    let nx = 2.0 * (mouse[0] / w) - 1.0;
    let ny = 1.0 - 2.0 * (mouse[1] / h);

    let fov_rad = camera.fov_degrees.to_radians();
    let half_tan = (fov_rad * 0.5).tan();
    let aspect = w / h;

    let yaw_rad = camera.yaw.to_radians();
    let pitch_rad = camera.pitch.to_radians();
    let (sin_y, cos_y) = yaw_rad.sin_cos();
    let (sin_p, cos_p) = pitch_rad.sin_cos();

    // Forward vector in Minecraft camera space
    let f_x = -cos_p * sin_y;
    let f_y = -sin_p;
    let f_z = cos_p * cos_y;

    // Right vector
    let r_x = -cos_y;
    let r_y = 0.0;
    let r_z = -sin_y;

    // Up vector: R x F
    let u_x = -sin_y * sin_p;
    let u_y = cos_p;
    let u_z = cos_y * sin_p;

    // Ray direction
    let dx = f_x + r_x * (nx * half_tan * aspect) + u_x * (ny * half_tan);
    let dy = f_y + r_y * (nx * half_tan * aspect) + u_y * (ny * half_tan);
    let dz = f_z + r_z * (nx * half_tan * aspect) + u_z * (ny * half_tan);
    let len = (dx * dx + dy * dy + dz * dz).sqrt().max(1e-6);
    let (dir_x, dir_y, dir_z) = (dx / len, dy / len, dz / len);

    // Screen center
    let (_scr_w, scr_h) = screen.dimensions(screen.applied_native_size);
    let center = [
        screen.at[0],
        screen.at[1] + f64::from(scr_h) * 0.5,
        screen.at[2],
    ];

    let (scr_sin, scr_cos) = screen.yaw.to_radians().sin_cos();
    // Screen normal facing the camera
    let norm_x = f64::from(scr_sin);
    let norm_y = 0.0;
    let norm_z = f64::from(-scr_cos);

    let denom = f64::from(dir_x) * norm_x + f64::from(dir_y) * norm_y + f64::from(dir_z) * norm_z;
    if denom >= -1e-4 {
        return None;
    }

    let to_center_x = center[0] - camera.position[0];
    let to_center_y = center[1] - camera.position[1];
    let to_center_z = center[2] - camera.position[2];
    let numer = to_center_x * norm_x + to_center_y * norm_y + to_center_z * norm_z;
    let t = numer / denom;
    if t <= 0.0 {
        return None;
    }

    let hit_x = camera.position[0] + t * f64::from(dir_x);
    let hit_y = camera.position[1] + t * f64::from(dir_y);
    let hit_z = camera.position[2] + t * f64::from(dir_z);

    let rel_x = (hit_x - center[0]) as f32;
    let rel_y = (hit_y - center[1]) as f32;
    let rel_z = (hit_z - center[2]) as f32;

    let scr_r_x = -scr_cos;
    let scr_r_z = -scr_sin;

    let u = rel_x * scr_r_x + rel_z * scr_r_z;
    let v = rel_y;

    Some((u, v))
}

pub(crate) fn find_hit_handle(
    screen: &Screen,
    camera: &NativeCamera,
    mouse: [f32; 2],
    gui_size: [f32; 2],
) -> Option<(DragHandle, (f32, f32))> {
    let (u, v) = raycast_screen_plane(screen, camera, mouse, gui_size)?;
    let (w, h) = screen.dimensions(screen.applied_native_size);
    let hw = w * 0.5;
    let hh = h * 0.5;

    let r_corner = (w.min(h) * 0.25).clamp(0.3, 0.9);
    let r_edge = (w.min(h) * 0.16).clamp(0.2, 0.6);

    // Corners:
    if (u - (-hw)).powi(2) + (v - hh).powi(2) <= r_corner.powi(2) {
        return Some((DragHandle::CornerTL, (u, v)));
    }
    if (u - hw).powi(2) + (v - hh).powi(2) <= r_corner.powi(2) {
        return Some((DragHandle::CornerTR, (u, v)));
    }
    if (u - (-hw)).powi(2) + (v - (-hh)).powi(2) <= r_corner.powi(2) {
        return Some((DragHandle::CornerBL, (u, v)));
    }
    if (u - hw).powi(2) + (v - (-hh)).powi(2) <= r_corner.powi(2) {
        return Some((DragHandle::CornerBR, (u, v)));
    }

    // Edges:
    if (u - (-hw)).abs() <= r_edge && v.abs() <= hh + r_edge {
        return Some((DragHandle::EdgeLeft, (u, v)));
    }
    if (u - hw).abs() <= r_edge && v.abs() <= hh + r_edge {
        return Some((DragHandle::EdgeRight, (u, v)));
    }
    if (v - hh).abs() <= r_edge && u.abs() <= hw + r_edge {
        return Some((DragHandle::EdgeTop, (u, v)));
    }
    if (v - (-hh)).abs() <= r_edge && u.abs() <= hw + r_edge {
        return Some((DragHandle::EdgeBottom, (u, v)));
    }

    None
}

pub fn crosshair_ray(camera: &NativeCamera) -> [f64; 3] {
    let yaw_rad = f64::from(camera.yaw).to_radians();
    let pitch_rad = f64::from(camera.pitch).to_radians();
    let (sin_y, cos_y) = yaw_rad.sin_cos();
    let (sin_p, cos_p) = pitch_rad.sin_cos();

    let dx = -cos_p * sin_y;
    let dy = -sin_p;
    let dz = cos_p * cos_y;
    let len = (dx * dx + dy * dy + dz * dz).sqrt().max(1e-6);
    [dx / len, dy / len, dz / len]
}

pub fn point_ray_dist(point: [f64; 3], ray_orig: [f64; 3], ray_dir: [f64; 3]) -> (f64, f64) {
    let vx = point[0] - ray_orig[0];
    let vy = point[1] - ray_orig[1];
    let vz = point[2] - ray_orig[2];
    let t = vx * ray_dir[0] + vy * ray_dir[1] + vz * ray_dir[2];
    if t <= 0.0 {
        return (f64::MAX, t);
    }
    let qx = ray_orig[0] + ray_dir[0] * t;
    let qy = ray_orig[1] + ray_dir[1] * t;
    let qz = ray_orig[2] + ray_dir[2] * t;
    let dist_sq = (point[0] - qx).powi(2) + (point[1] - qy).powi(2) + (point[2] - qz).powi(2);
    (dist_sq.sqrt(), t)
}

pub fn is_screen_visible(
    camera: &NativeCamera,
    screen_center: [f64; 3],
    screen_radius: f64,
) -> bool {
    let dx = screen_center[0] - camera.position[0];
    let dy = screen_center[1] - camera.position[1];
    let dz = screen_center[2] - camera.position[2];
    let dist_sq = dx * dx + dy * dy + dz * dz;
    if dist_sq <= screen_radius * screen_radius {
        return true;
    }
    let dist = dist_sq.sqrt();
    let ray = crosshair_ray(camera);
    let dot = (dx * ray[0] + dy * ray[1] + dz * ray[2]) / dist;
    let angular_radius = (screen_radius / dist).min(1.0);
    dot + angular_radius >= 0.05
}

pub fn ray_intersect_screen_plane(
    screen: &Screen,
    cam_pos: [f64; 3],
    ray_dir: [f64; 3],
) -> Option<(f64, f64)> {
    let (_scr_w, scr_h) = screen.dimensions(screen.applied_native_size);
    let center = [
        screen.at[0],
        screen.at[1] + f64::from(scr_h) * 0.5,
        screen.at[2],
    ];

    let (scr_sin, scr_cos) = screen.yaw.to_radians().sin_cos();
    let norm = [f64::from(scr_sin), 0.0, f64::from(-scr_cos)];

    let denom = ray_dir[0] * norm[0] + ray_dir[1] * norm[1] + ray_dir[2] * norm[2];
    if denom.abs() <= 1e-4 {
        return None;
    }

    let to_center = [
        center[0] - cam_pos[0],
        center[1] - cam_pos[1],
        center[2] - cam_pos[2],
    ];
    let numer = to_center[0] * norm[0] + to_center[1] * norm[1] + to_center[2] * norm[2];
    let t = numer / denom;
    if t <= 0.0 {
        return None;
    }

    let hit = [
        cam_pos[0] + t * ray_dir[0],
        cam_pos[1] + t * ray_dir[1],
        cam_pos[2] + t * ray_dir[2],
    ];

    let rel = [hit[0] - center[0], hit[1] - center[1], hit[2] - center[2]];

    let scr_r = [f64::from(-scr_cos), 0.0, f64::from(-scr_sin)];
    let u = rel[0] * scr_r[0] + rel[1] * scr_r[1] + rel[2] * scr_r[2];
    let v = rel[1];

    Some((u, v))
}

pub fn find_crosshair_target(
    screen: &Screen,
    cam_pos: [f64; 3],
    ray_dir: [f64; 3],
    is_editing: bool,
    show_speakers: bool,
    audio_mode: AudioMode,
) -> Option<DragTarget> {
    let (w, h) = screen.dimensions(screen.applied_native_size);
    let hw = f64::from(w) * 0.5;
    let hh = f64::from(h) * 0.5;
    let center = [screen.at[0], screen.at[1] + hh, screen.at[2]];

    let mut candidates: Vec<(DragTarget, f64)> = Vec::new();

    // 1. Speakers
    if show_speakers || is_editing {
        let spk = SpeakerLayout::compute(screen.at, screen.yaw, w, h, &screen.custom_speakers);
        let speaker_list: &[(SpeakerId, [f64; 3])] = match audio_mode {
            AudioMode::Surround71 => &[
                (SpeakerId::FL, spk.fl),
                (SpeakerId::FR, spk.fr),
                (SpeakerId::C, spk.c),
                (SpeakerId::LFE, spk.lfe),
                (SpeakerId::SL, spk.sl),
                (SpeakerId::SR, spk.sr),
                (SpeakerId::BL, spk.bl),
                (SpeakerId::BR, spk.br),
            ],
            AudioMode::Surround51 => &[
                (SpeakerId::FL, spk.fl),
                (SpeakerId::FR, spk.fr),
                (SpeakerId::C, spk.c),
                (SpeakerId::LFE, spk.lfe),
                (SpeakerId::SL, spk.sl),
                (SpeakerId::SR, spk.sr),
            ],
            AudioMode::StereoSpeakers => &[(SpeakerId::FL, spk.fl), (SpeakerId::FR, spk.fr)],
            AudioMode::Game3D => &[(SpeakerId::C, spk.c)],
            AudioMode::Direct | AudioMode::Mute => &[],
        };

        for &(id, pos) in speaker_list {
            let (perp, t) = point_ray_dist(pos, cam_pos, ray_dir);
            if t > 0.1 && t < 60.0 {
                let max_perp = (0.50 + t * 0.035).min(1.2);
                if perp <= max_perp {
                    let score = perp / max_perp;
                    candidates.push((DragTarget::Speaker(id, t), score));
                }
            }
        }
    }

    // 2. Handles & screen center (in edit mode)
    if is_editing {
        let (sin, cos) = screen.yaw.to_radians().sin_cos();
        let right = [f64::from(-cos), 0.0, f64::from(-sin)];
        let normal = [f64::from(sin), 0.0, f64::from(-cos)];
        let up = [0.0, 1.0, 0.0];

        let point_at = |x: f64, y: f64, z: f64| -> [f64; 3] {
            [
                center[0] + right[0] * x + up[0] * y + normal[0] * z,
                center[1] + right[1] * x + up[1] * y + normal[1] * z,
                center[2] + right[2] * x + up[2] * y + normal[2] * z,
            ]
        };

        let (tl, tr, bl, br, edge_top, edge_bot, edge_left, edge_right) = if screen.curvature <= 0.5
        {
            (
                point_at(-hw, hh, 0.0),
                point_at(hw, hh, 0.0),
                point_at(-hw, -hh, 0.0),
                point_at(hw, -hh, 0.0),
                point_at(0.0, hh, 0.0),
                point_at(0.0, -hh, 0.0),
                point_at(-hw, 0.0, 0.0),
                point_at(hw, 0.0, 0.0),
            )
        } else {
            let alpha = f64::from(screen.curvature).to_radians();
            let radius = f64::from(w) / alpha;
            let arc_pt = |f: f64, y: f64| -> [f64; 3] {
                let theta = -alpha * 0.5 + alpha * f;
                let x = radius * theta.sin();
                let z = radius * (1.0 - theta.cos());
                point_at(x, y, z)
            };
            (
                arc_pt(0.0, hh),
                arc_pt(1.0, hh),
                arc_pt(0.0, -hh),
                arc_pt(1.0, -hh),
                arc_pt(0.5, hh),
                arc_pt(0.5, -hh),
                arc_pt(0.0, 0.0),
                arc_pt(1.0, 0.0),
            )
        };

        let handle_list = [
            (DragHandle::CornerTL, tl),
            (DragHandle::CornerTR, tr),
            (DragHandle::CornerBL, bl),
            (DragHandle::CornerBR, br),
            (DragHandle::EdgeTop, edge_top),
            (DragHandle::EdgeBottom, edge_bot),
            (DragHandle::EdgeLeft, edge_left),
            (DragHandle::EdgeRight, edge_right),
        ];

        for (handle, pos) in handle_list {
            let (perp, t) = point_ray_dist(pos, cam_pos, ray_dir);
            if t > 0.1 && t < 60.0 {
                let max_perp = (0.45 + t * 0.035).min(1.1);
                if perp <= max_perp {
                    let score = perp / max_perp;
                    candidates.push((DragTarget::Handle(handle), score));
                }
            }
        }

        // Screen center
        let (perp, t) = point_ray_dist(center, cam_pos, ray_dir);
        if t > 0.1 && t < 60.0 {
            let max_perp = (0.50 + t * 0.035).min(1.5);
            if perp <= max_perp {
                let score = (perp / max_perp) + 0.15;
                candidates.push((DragTarget::ScreenCenter { initial_dist: t }, score));
            }
        }
    }

    candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    candidates.first().map(|c| c.0)
}

pub(crate) fn apply_drag(
    screen: &mut Screen,
    handle: DragHandle,
    u: f32,
    v: f32,
    native_size: Option<(u32, u32)>,
    grid_snap: Option<f32>,
) {
    let (cur_w, cur_h) = screen.dimensions(native_size);
    match handle {
        DragHandle::CornerTL
        | DragHandle::CornerTR
        | DragHandle::CornerBL
        | DragHandle::CornerBR => {
            let half_diag = (u * u + v * v).sqrt();
            let mut new_diag = (2.0 * half_diag).clamp(0.5, 250.0);
            if let Some(step) = grid_snap {
                new_diag = (new_diag / step).round() * step;
            }
            screen.diagonal = new_diag.clamp(0.5, 250.0);
            screen.recalculate(native_size);
        }
        DragHandle::EdgeLeft | DragHandle::EdgeRight => {
            let mut new_w = (2.0 * u.abs()).clamp(0.5, 250.0);
            if let Some(step) = grid_snap {
                new_w = (new_w / step).round() * step;
            }
            let new_aspect = new_w / cur_h.max(0.1);
            screen.aspect = AspectRatio::Ratio(new_aspect);
            screen.diagonal = (new_w * new_w + cur_h * cur_h).sqrt().clamp(0.5, 250.0);
            screen.recalculate(native_size);
        }
        DragHandle::EdgeTop | DragHandle::EdgeBottom => {
            let mut new_h = (2.0 * v.abs()).clamp(0.5, 250.0);
            if let Some(step) = grid_snap {
                new_h = (new_h / step).round() * step;
            }
            let new_aspect = cur_w / new_h.max(0.1);
            screen.aspect = AspectRatio::Ratio(new_aspect);
            screen.diagonal = (cur_w * cur_w + new_h * new_h).sqrt().clamp(0.5, 250.0);
            screen.recalculate(native_size);
        }
    }
}
