use crate::State;
use crate::editor;
use crate::types::{AudioMode, CURVE_SLICES, DragHandle, DragTarget, SpeakerId, SpeakerLayout};
use horizon_client::mod_api::{
    Host, NativeColoredLight, PrimitiveShape, VisualsEvent, WorldPrimitivesEvent, WorldSubmitEvent,
};

pub(crate) fn world_submit(state: &State, host: &dyn Host, event: &mut WorldSubmitEvent<'_>) {
    let Some(screen) = &state.screen else { return };
    if let Some(video) = screen.video {
        if let Some(camera) = host.camera() {
            let (w, h) = screen.dimensions(screen.applied_native_size);
            let center = [screen.at[0], screen.at[1] + h as f64 * 0.5, screen.at[2]];
            let radius = (screen.diagonal as f64 * 0.6).max(w as f64 * 0.6);
            if !editor::is_screen_visible(&camera, center, radius) {
                return;
            }
        }
        event.draw.push();
        if event.draw.translate_world(screen.at) {
            if screen.curvature <= 0.5 {
                host.draw_video(event.draw, video, screen.corners);
            } else {
                let native_size = host.video_state(video).and_then(|s| s.video_size);
                for (slice_corners, uv) in screen.slices(native_size) {
                    host.draw_video_part(event.draw, video, slice_corners, uv);
                }
            }
        }
        event.draw.pop();
    }
}

pub(crate) fn world_primitives(
    state: &State,
    host: &dyn Host,
    event: &mut WorldPrimitivesEvent<'_>,
) {
    let Some(screen) = &state.screen else { return };
    // The editor's frame and speakers show through walls, but not through the
    // open 3D menu: with a depth test, the menu's panels in front hide them.
    let through = !state.menu.open;

    let native_size = screen
        .video
        .and_then(|v| host.video_state(v))
        .and_then(|s| s.video_size);
    let (w, h) = screen.dimensions(native_size);

    // Draw 3D Surround Speakers in world when editing or when speaker visualization is toggled on
    if state.editing.is_some() || state.show_speakers {
        let spk = SpeakerLayout::compute(screen.at, screen.yaw, w, h, &screen.custom_speakers);
        let is_spk_active = |id: SpeakerId| -> bool {
            matches!(state.active_drag, Some(DragTarget::Speaker(spk_id, _)) if spk_id == id)
        };
        let is_spk_hovered = |id: SpeakerId| -> bool {
            matches!(state.hovered_target, Some(DragTarget::Speaker(spk_id, _)) if spk_id == id)
        };
        let mut draw_spk = |id: SpeakerId, pos: [f64; 3], label: &str, color: u32, is_sub: bool| {
            let active = is_spk_active(id);
            let hovered = is_spk_hovered(id);
            let sz = if active {
                if is_sub { 0.30 } else { 0.22 }
            } else if hovered {
                if is_sub { 0.26 } else { 0.19 }
            } else {
                if is_sub { 0.22 } else { 0.16 }
            };
            let (outline_col, outline_w) = if active {
                (0xFF55FF55, 4.0)
            } else if hovered {
                (0xFFFFFF00, 3.5)
            } else {
                (color, 2.0)
            };

            let min = [pos[0] - sz, pos[1] - sz, pos[2] - sz];
            let max = [pos[0] + sz, pos[1] + sz, pos[2] + sz];
            event
                .primitives
                .box_outline(min, max, outline_col, outline_w)
                .through_walls = through;
            if active || hovered {
                event
                    .primitives
                    .filled_box(min, max, (outline_col & 0x00FFFFFF) | 0x35000000)
                    .through_walls = through;
            }
            event
                .primitives
                .line(screen.at, pos, (outline_col & 0x00FFFFFF) | 0x35000000, 1.0)
                .through_walls = through;
            let text_pos = [pos[0], pos[1] + sz + 0.15, pos[2]];
            let text = if active {
                format!(
                    "§a[Тяните {}] ({:.1}, {:.1}, {:.1})",
                    id.name(),
                    pos[0],
                    pos[1],
                    pos[2]
                )
            } else if hovered {
                format!("§e[Зажмите ПКМ: переместить {}]", id.name())
            } else {
                label.to_string()
            };
            event
                .primitives
                .text(text_pos, &text, outline_col, 0.8)
                .through_walls = through;
        };

        match state.audio_mode {
            AudioMode::Surround71 => {
                draw_spk(SpeakerId::FL, spk.fl, "[FL] Левый фронт", 0xFF00FFCC, false);
                draw_spk(
                    SpeakerId::FR,
                    spk.fr,
                    "[FR] Правый фронт",
                    0xFF00FFCC,
                    false,
                );
                draw_spk(SpeakerId::C, spk.c, "[C] Центр (Голос)", 0xFFFFDD33, false);
                draw_spk(SpeakerId::LFE, spk.lfe, "[LFE] Сабвуфер", 0xFFFF6600, true);
                draw_spk(
                    SpeakerId::SL,
                    spk.sl,
                    "[SL] Боковой левый",
                    0xFF3399FF,
                    false,
                );
                draw_spk(
                    SpeakerId::SR,
                    spk.sr,
                    "[SR] Боковой правый",
                    0xFF3399FF,
                    false,
                );
                draw_spk(
                    SpeakerId::BL,
                    spk.bl,
                    "[BL] Задний левый",
                    0xFFCC33FF,
                    false,
                );
                draw_spk(
                    SpeakerId::BR,
                    spk.br,
                    "[BR] Задний правый",
                    0xFFCC33FF,
                    false,
                );
            }
            AudioMode::Surround51 => {
                draw_spk(SpeakerId::FL, spk.fl, "[FL] Левый фронт", 0xFF00FFCC, false);
                draw_spk(
                    SpeakerId::FR,
                    spk.fr,
                    "[FR] Правый фронт",
                    0xFF00FFCC,
                    false,
                );
                draw_spk(SpeakerId::C, spk.c, "[C] Центр (Голос)", 0xFFFFDD33, false);
                draw_spk(SpeakerId::LFE, spk.lfe, "[LFE] Сабвуфер", 0xFFFF6600, true);
                draw_spk(
                    SpeakerId::SL,
                    spk.sl,
                    "[SL] Тыловой левый",
                    0xFF3399FF,
                    false,
                );
                draw_spk(
                    SpeakerId::SR,
                    spk.sr,
                    "[SR] Тыловой правый",
                    0xFF3399FF,
                    false,
                );
            }
            AudioMode::StereoSpeakers => {
                draw_spk(
                    SpeakerId::FL,
                    spk.fl,
                    "[L] Левая колонка",
                    0xFF00FFCC,
                    false,
                );
                draw_spk(
                    SpeakerId::FR,
                    spk.fr,
                    "[R] Правая колонка",
                    0xFF00FFCC,
                    false,
                );
            }
            AudioMode::Game3D => {
                draw_spk(SpeakerId::C, spk.c, "[3D] Центр экрана", 0xFF55FF55, false);
            }
            _ => {}
        }
    }

    let Some(editing) = &state.editing else {
        return;
    };
    let hw = f64::from(w) * 0.5;
    let hh = f64::from(h) * 0.5;

    let center = [
        screen.at[0],
        screen.at[1] + f64::from(h) * 0.5,
        screen.at[2],
    ];

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

    let is_handle_active = |h: DragHandle| -> bool {
        matches!(state.active_drag, Some(DragTarget::Handle(handle)) if handle == h)
    };
    let is_handle_hovered = |h: DragHandle| -> bool {
        matches!(state.hovered_target, Some(DragTarget::Handle(handle)) if handle == h)
    };
    let has_any_active_screen_drag = matches!(
        state.active_drag,
        Some(DragTarget::Handle(_) | DragTarget::ScreenCenter { .. })
    );
    let wireframe_color = if has_any_active_screen_drag {
        0xFF55FF55
    } else {
        0xFFFF2222
    };

    if screen.curvature <= 0.5 {
        // Flat screen wireframe
        let tl = point_at(-hw, hh, 0.0);
        let tr = point_at(hw, hh, 0.0);
        let br = point_at(hw, -hh, 0.0);
        let bl = point_at(-hw, -hh, 0.0);

        // 1. Outer wireframe border
        event
            .primitives
            .line(tl, tr, wireframe_color, 3.0)
            .through_walls = through;
        event
            .primitives
            .line(tr, br, wireframe_color, 3.0)
            .through_walls = through;
        event
            .primitives
            .line(br, bl, wireframe_color, 3.0)
            .through_walls = through;
        event
            .primitives
            .line(bl, tl, wireframe_color, 3.0)
            .through_walls = through;

        // 2. Interior alignment grid (4x4 subdivisions)
        for i in 1..4 {
            let f = f64::from(i) / 4.0;
            let u = -hw + f64::from(w) * f;
            event
                .primitives
                .line(
                    point_at(u, hh, 0.0),
                    point_at(u, -hh, 0.0),
                    (wireframe_color & 0x00FFFFFF) | 0x60000000,
                    1.5,
                )
                .through_walls = through;

            let v = -hh + f64::from(h) * f;
            event
                .primitives
                .line(
                    point_at(-hw, v, 0.0),
                    point_at(hw, v, 0.0),
                    (wireframe_color & 0x00FFFFFF) | 0x60000000,
                    1.5,
                )
                .through_walls = through;
        }

        // 3. Corner handles
        let corner_s = (w.min(h) * 0.035).clamp(0.08, 0.35) as f64;
        for (handle, p) in [
            (DragHandle::CornerTL, tl),
            (DragHandle::CornerTR, tr),
            (DragHandle::CornerBL, bl),
            (DragHandle::CornerBR, br),
        ] {
            let active = is_handle_active(handle);
            let hovered = is_handle_hovered(handle);
            let color = if active {
                0xFF55FF55
            } else if hovered {
                0xFFFFFF00
            } else {
                0xFFFF1111
            };
            let s = if active {
                corner_s * 1.4
            } else if hovered {
                corner_s * 1.2
            } else {
                corner_s
            };
            let min = [p[0] - s, p[1] - s, p[2] - s];
            let max = [p[0] + s, p[1] + s, p[2] + s];
            event.primitives.filled_box(min, max, color).through_walls = through;
            event
                .primitives
                .box_outline(min, max, 0xFFFFFFFF, 2.0)
                .through_walls = through;
            if active {
                event
                    .primitives
                    .text(
                        [p[0], p[1] + s + 0.2, p[2]],
                        format!("§a[Тяните размер] {:.2}x{:.2} бл", w, h),
                        0xFF55FF55,
                        0.85,
                    )
                    .through_walls = through;
            } else if hovered {
                event
                    .primitives
                    .text(
                        [p[0], p[1] + s + 0.2, p[2]],
                        "§e[Зажмите ПКМ: изменить размер]",
                        0xFFFFFF00,
                        0.85,
                    )
                    .through_walls = through;
            }
        }

        // 4. Edge handles
        let edge_s = (w.min(h) * 0.025).clamp(0.06, 0.28) as f64;
        for (handle, p) in [
            (DragHandle::EdgeTop, point_at(0.0, hh, 0.0)),
            (DragHandle::EdgeBottom, point_at(0.0, -hh, 0.0)),
            (DragHandle::EdgeLeft, point_at(-hw, 0.0, 0.0)),
            (DragHandle::EdgeRight, point_at(hw, 0.0, 0.0)),
        ] {
            let active = is_handle_active(handle);
            let hovered = is_handle_hovered(handle);
            let color = if active {
                0xFF55FF55
            } else if hovered {
                0xFFFFFF00
            } else {
                0xFFFF6622
            };
            let s = if active {
                edge_s * 1.4
            } else if hovered {
                edge_s * 1.2
            } else {
                edge_s
            };
            let min = [p[0] - s, p[1] - s, p[2] - s];
            let max = [p[0] + s, p[1] + s, p[2] + s];
            event.primitives.filled_box(min, max, color).through_walls = through;
            event
                .primitives
                .box_outline(min, max, 0xFFFFFFFF, 1.5)
                .through_walls = through;
            if active {
                event
                    .primitives
                    .text(
                        [p[0], p[1] + s + 0.2, p[2]],
                        format!("§a[Тяните пропорции] {:.2}x{:.2} бл", w, h),
                        0xFF55FF55,
                        0.85,
                    )
                    .through_walls = through;
            } else if hovered {
                event
                    .primitives
                    .text(
                        [p[0], p[1] + s + 0.2, p[2]],
                        "§e[Зажмите ПКМ: изменить пропорции]",
                        0xFFFFFF00,
                        0.85,
                    )
                    .through_walls = through;
            }
        }
    } else {
        // Curved cylindrical wireframe
        let alpha = f64::from(screen.curvature).to_radians();
        let radius = f64::from(w) / alpha;
        let n = CURVE_SLICES;

        let arc_pt = |f: f64, y: f64| -> [f64; 3] {
            let theta = -alpha * 0.5 + alpha * f;
            let x = radius * theta.sin();
            let z = radius * (1.0 - theta.cos());
            point_at(x, y, z)
        };

        // Draw curved top and bottom arcs
        for i in 0..n {
            let f0 = i as f64 / n as f64;
            let f1 = (i + 1) as f64 / n as f64;
            let top0 = arc_pt(f0, hh);
            let top1 = arc_pt(f1, hh);
            let bot0 = arc_pt(f0, -hh);
            let bot1 = arc_pt(f1, -hh);

            event
                .primitives
                .line(top0, top1, wireframe_color, 3.0)
                .through_walls = through;
            event
                .primitives
                .line(bot0, bot1, wireframe_color, 3.0)
                .through_walls = through;

            // Interior vertical ribs (every 6 slices)
            if i > 0 && i % (n / 4) == 0 {
                event
                    .primitives
                    .line(top0, bot0, (wireframe_color & 0x00FFFFFF) | 0x60000000, 1.5)
                    .through_walls = through;
            }
        }

        // Left and right vertical borders
        let tl = arc_pt(0.0, hh);
        let bl = arc_pt(0.0, -hh);
        let tr = arc_pt(1.0, hh);
        let br = arc_pt(1.0, -hh);
        event
            .primitives
            .line(tl, bl, wireframe_color, 3.0)
            .through_walls = through;
        event
            .primitives
            .line(tr, br, wireframe_color, 3.0)
            .through_walls = through;

        // Curved corner handles
        let corner_s = (w.min(h) * 0.035).clamp(0.08, 0.35) as f64;
        for (handle, p) in [
            (DragHandle::CornerTL, tl),
            (DragHandle::CornerTR, tr),
            (DragHandle::CornerBL, bl),
            (DragHandle::CornerBR, br),
        ] {
            let active = is_handle_active(handle);
            let hovered = is_handle_hovered(handle);
            let color = if active {
                0xFF55FF55
            } else if hovered {
                0xFFFFFF00
            } else {
                0xFFFF1111
            };
            let s = if active {
                corner_s * 1.4
            } else if hovered {
                corner_s * 1.2
            } else {
                corner_s
            };
            let min = [p[0] - s, p[1] - s, p[2] - s];
            let max = [p[0] + s, p[1] + s, p[2] + s];
            event.primitives.filled_box(min, max, color).through_walls = through;
            event
                .primitives
                .box_outline(min, max, 0xFFFFFFFF, 2.0)
                .through_walls = through;
            if active {
                event
                    .primitives
                    .text(
                        [p[0], p[1] + s + 0.2, p[2]],
                        format!("§a[Тяните размер] {:.2}x{:.2} бл", w, h),
                        0xFF55FF55,
                        0.85,
                    )
                    .through_walls = through;
            } else if hovered {
                event
                    .primitives
                    .text(
                        [p[0], p[1] + s + 0.2, p[2]],
                        "§e[Зажмите ПКМ: изменить размер]",
                        0xFFFFFF00,
                        0.85,
                    )
                    .through_walls = through;
            }
        }

        // Edge handles
        let edge_s = (w.min(h) * 0.025).clamp(0.06, 0.28) as f64;
        for (handle, p) in [
            (DragHandle::EdgeTop, arc_pt(0.5, hh)),
            (DragHandle::EdgeBottom, arc_pt(0.5, -hh)),
            (DragHandle::EdgeLeft, arc_pt(0.0, 0.0)),
            (DragHandle::EdgeRight, arc_pt(1.0, 0.0)),
        ] {
            let active = is_handle_active(handle);
            let hovered = is_handle_hovered(handle);
            let color = if active {
                0xFF55FF55
            } else if hovered {
                0xFFFFFF00
            } else {
                0xFFFF6622
            };
            let s = if active {
                edge_s * 1.4
            } else if hovered {
                edge_s * 1.2
            } else {
                edge_s
            };
            let min = [p[0] - s, p[1] - s, p[2] - s];
            let max = [p[0] + s, p[1] + s, p[2] + s];
            event.primitives.filled_box(min, max, color).through_walls = through;
            event
                .primitives
                .box_outline(min, max, 0xFFFFFFFF, 1.5)
                .through_walls = through;
            if active {
                event
                    .primitives
                    .text(
                        [p[0], p[1] + s + 0.2, p[2]],
                        format!("§a[Тяните пропорции] {:.2}x{:.2} бл", w, h),
                        0xFF55FF55,
                        0.85,
                    )
                    .through_walls = through;
            } else if hovered {
                event
                    .primitives
                    .text(
                        [p[0], p[1] + s + 0.2, p[2]],
                        "§e[Зажмите ПКМ: изменить пропорции]",
                        0xFFFFFF00,
                        0.85,
                    )
                    .through_walls = through;
            }
        }
    }

    // 5. Screen center handle
    let center_active = matches!(state.active_drag, Some(DragTarget::ScreenCenter { .. }));
    let center_hovered = matches!(state.hovered_target, Some(DragTarget::ScreenCenter { .. }));
    let cs = if center_active {
        0.16
    } else if center_hovered {
        0.12
    } else {
        0.08
    };
    let ccol = if center_active {
        0xFF55FF55
    } else if center_hovered {
        0xFFFFFF00
    } else {
        0x80FFFFFF
    };
    let min = [center[0] - cs, center[1] - cs, center[2] - cs];
    let max = [center[0] + cs, center[1] + cs, center[2] + cs];
    event.primitives.filled_box(min, max, ccol).through_walls = through;
    event
        .primitives
        .box_outline(min, max, 0xFFFFFFFF, 1.5)
        .through_walls = through;
    if center_active {
        event
            .primitives
            .text(
                [center[0], center[1] + 0.35, center[2]],
                "§a[Тяните: перемещение экрана]",
                0xFF55FF55,
                0.85,
            )
            .through_walls = through;
    } else if center_hovered {
        event
            .primitives
            .text(
                [center[0], center[1] + 0.35, center[2]],
                "§e[Зажмите ПКМ: переместить экран]",
                0xFFFFFF00,
                0.85,
            )
            .through_walls = through;
    }

    // Overhead floating label
    let label_pos = point_at(0.0, hh + 0.35, 0.0);
    let id_str = &editing.id;
    let aspect_str = screen.aspect.as_string();
    let curve_label = if screen.curvature > 0.5 {
        format!(" | изгиб {:.0}°", screen.curvature)
    } else {
        "".into()
    };
    event
        .primitives
        .push(PrimitiveShape::Text {
            at: label_pos,
            text: format!(
                "[{}: {:.2}x{:.2} бл | {}{}]",
                id_str, w, h, aspect_str, curve_label
            ),
            color: 0xFFFFFFFF,
            scale: 1.0,
            scale_with_distance: false,
            background: true,
        })
        .through_walls = through;

    let hint_pos = point_at(0.0, hh + 0.70, 0.0);
    event
        .primitives
        .push(PrimitiveShape::Text {
            at: hint_pos,
            text: "§e[Наведите прицел на угол или колонку и зажмите ПКМ, чтобы двигать в 3D]"
                .into(),
            color: 0xFFFFFF55,
            scale: 0.85,
            scale_with_distance: false,
            background: true,
        })
        .through_walls = through;
}

pub(crate) fn visuals(state: &mut State, host: &dyn Host, event: &mut VisualsEvent<'_>) {
    if !state.light_enabled || state.light_brightness <= 0.001 {
        return;
    }
    let Some(screen) = state.screen.as_ref() else {
        return;
    };
    let Some(video) = screen.video else {
        return;
    };
    let screen_at = screen.at;
    let screen_yaw = screen.yaw;
    let screen_diagonal = screen.diagonal;
    let screen_curvature = screen.curvature;
    let (width, height) = screen.dimensions(screen.applied_native_size);

    let Some(camera) = host.camera() else {
        return;
    };
    let center = [
        screen_at[0],
        screen_at[1] + height as f64 * 0.5,
        screen_at[2],
    ];
    let screen_radius = (screen_diagonal as f64 * 0.6).max(width as f64 * 0.6);
    if !editor::is_screen_visible(&camera, center, screen_radius) {
        return;
    }

    let Some(color) = host.video_color(video) else {
        return;
    };
    let zone_colors = host.video_colors(video);

    let r = color[0] as f32;
    let g = color[1] as f32;
    let b = color[2] as f32;
    let max_val = r.max(g).max(b);

    if max_val < 3.0 {
        // Dark scene: smoothly decay ambient light to dark
        state.current_light_intensity *= 0.75;
        if state.current_light_intensity < 0.01 {
            state.current_light_intensity = 0.0;
            return;
        }
    } else {
        // Perceptual color tuning: slight saturation boost to prevent grayish washout
        let mean = (r + g + b) / 3.0;
        let sat_boost = 1.35;
        let br = (mean + (r - mean) * sat_boost).clamp(0.0, 255.0);
        let bg = (mean + (g - mean) * sat_boost).clamp(0.0, 255.0);
        let bb = (mean + (b - mean) * sat_boost).clamp(0.0, 255.0);
        let top = br.max(bg).max(bb).max(1.0);
        let target_rgb = [br / top * 255.0, bg / top * 255.0, bb / top * 255.0];

        // Perceptual intensity with gamma response (lum^0.5)
        let lum = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0;
        let target_intensity =
            (lum.sqrt() * (top / 255.0) * state.light_brightness).clamp(0.0, 1.0);

        // Responsive exponential smoothing
        let diff = (target_rgb[0] - state.current_light_rgb[0])
            .abs()
            .max((target_rgb[1] - state.current_light_rgb[1]).abs())
            .max((target_rgb[2] - state.current_light_rgb[2]).abs());
        let int_diff = (target_intensity - state.current_light_intensity).abs();

        let alpha = if diff > 70.0 || int_diff > 0.35 {
            0.85 // rapid reaction for explosions, lightning, cuts
        } else {
            0.35 // smooth transitions for continuous movie scenes
        };

        state.current_light_rgb[0] += (target_rgb[0] - state.current_light_rgb[0]) * alpha;
        state.current_light_rgb[1] += (target_rgb[1] - state.current_light_rgb[1]) * alpha;
        state.current_light_rgb[2] += (target_rgb[2] - state.current_light_rgb[2]) * alpha;
        state.current_light_intensity += (target_intensity - state.current_light_intensity) * alpha;
    }

    let intensity = state.current_light_intensity;
    if intensity < 0.01 {
        return;
    }

    let (sin, cos) = screen_yaw.to_radians().sin_cos();
    let right = [-cos as f64, -sin as f64];
    let normal = [sin as f64, -cos as f64]; // Normal facing towards viewers/room

    let forward_offset = (0.8_f64).max(width as f64 * 0.03).min(2.0_f64);
    let pack_rgb = |cr: f32, cg: f32, cb: f32| -> u32 {
        let r = (cr.round() as u32).min(255);
        let g = (cg.round() as u32).min(255);
        let b = (cb.round() as u32).min(255);
        (r << 16) | (g << 8) | b
    };

    let tune_color = |cr: f32, cg: f32, cb: f32| -> u32 {
        let m = (cr + cg + cb) / 3.0;
        let sat_boost = 1.35;
        let br = (m + (cr - m) * sat_boost).clamp(0.0, 255.0);
        let bg = (m + (cg - m) * sat_boost).clamp(0.0, 255.0);
        let bb = (m + (cb - m) * sat_boost).clamp(0.0, 255.0);
        let top = br.max(bg).max(bb).max(1.0);
        pack_rgb(br / top * 255.0, bg / top * 255.0, bb / top * 255.0)
    };

    let center_y = screen_at[1] + height as f64 * 0.5;
    let base_radius = (screen_diagonal * 1.5).max(width * 1.2).clamp(16.0, 64.0);

    // If screen is wide or curved, emit 3 directional lights across the screen for immersive surround ambiance
    if (width > 6.0 || screen_curvature > 5.0)
        && let Some(zones) = zone_colors
    {
        let spread = (width as f64 * 0.3).min(6.0);
        let side_radius = (base_radius * 0.85).clamp(4.0, 48.0);
        let side_intensity = (intensity * 0.85).clamp(0.0, 1.0);

        let tune_zone = |z: [u8; 3]| -> u32 { tune_color(z[0] as f32, z[1] as f32, z[2] as f32) };

        // Left light
        let left_pos = [
            screen_at[0] - right[0] * spread + normal[0] * forward_offset,
            center_y,
            screen_at[2] - right[1] * spread + normal[1] * forward_offset,
        ];
        let left_rgb = tune_zone(zones[0]);
        event.lights.push(NativeColoredLight {
            at: left_pos,
            rgb: left_rgb,
            radius: side_radius,
            intensity: side_intensity,
        });

        // Center light
        let center_pos = [
            screen_at[0] + normal[0] * forward_offset,
            center_y,
            screen_at[2] + normal[1] * forward_offset,
        ];
        let center_rgb = pack_rgb(
            state.current_light_rgb[0],
            state.current_light_rgb[1],
            state.current_light_rgb[2],
        );
        event.lights.push(NativeColoredLight {
            at: center_pos,
            rgb: center_rgb,
            radius: base_radius,
            intensity,
        });

        // Right light
        let right_pos = [
            screen_at[0] + right[0] * spread + normal[0] * forward_offset,
            center_y,
            screen_at[2] + right[1] * spread + normal[1] * forward_offset,
        ];
        let right_rgb = tune_zone(zones[2]);
        event.lights.push(NativeColoredLight {
            at: right_pos,
            rgb: right_rgb,
            radius: side_radius,
            intensity: side_intensity,
        });
    } else {
        // Single center light
        let center_pos = [
            screen_at[0] + normal[0] * forward_offset,
            center_y,
            screen_at[2] + normal[1] * forward_offset,
        ];
        let center_rgb = pack_rgb(
            state.current_light_rgb[0],
            state.current_light_rgb[1],
            state.current_light_rgb[2],
        );
        event.lights.push(NativeColoredLight {
            at: center_pos,
            rgb: center_rgb,
            radius: base_radius,
            intensity,
        });
    }
}
