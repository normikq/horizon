//! Drawing the menu: its panels as flat quads and text in the world, through
//! the game's own renderer (`world_quad` and `world_text`: depth tested, with
//! the world's TAA and shader packs, no pipeline of our own), and the target
//! crosshair in the HUD.
//!
//! Each panel is drawn in its own layout units: the pose is moved to the
//! panel's top-left corner, turned to face the eye and scaled by `UNIT` with
//! `y` flipped, so a layout point `(x, y)` is drawn at `(x, y)` and one font
//! pixel is one unit. Layers are kept apart by small steps toward the eye.
//!
//! Rounded corners are horizontal strips whose ends follow the corner's
//! circle: a handful of quads per card, and no texture to upload.

use super::layout::{
    self, ACTIVE_CARD, CardFrame, MenuModel, PanelId, Status, TOGGLES_CARD,
    ToggleKind, WidgetId,
};
use super::{MenuState, panels};
use crate::State;
use horizon_client::mod_api::{
    GuiRenderPostEvent, Host, NativeWorldDraw, Text, WorldSubmitEvent,
};

/// A sprite of plain white in the game's atlas, tinted to any colour.
const WHITE: &str = "horizon/white";
/// The middle of the sprite, clear of its edges' filtering.
const WHITE_UV: [f32; 4] = [4.0, 4.0, 12.0, 12.0];

type Rgba = [u8; 4];

const PANEL: Rgba = [242, 242, 247, 232];
const PANEL_SHADOW: Rgba = [0, 0, 0, 46];
const CARD: Rgba = [255, 255, 255, 255];
const CARD_HOVER: Rgba = [232, 241, 255, 255];
const CARD_SHADOW: Rgba = [0, 0, 0, 22];
const SEPARATOR: Rgba = [216, 216, 222, 255];
const TRACK: Rgba = [229, 229, 234, 255];
const ACCENT: Rgba = [10, 132, 255, 255];
const ACCENT_HOVER: Rgba = [64, 160, 255, 255];
const ACCENT_LIGHT: Rgba = [229, 241, 255, 255];
const GREEN: Rgba = [52, 199, 89, 255];
const ORANGE: Rgba = [255, 159, 10, 255];
const RED: Rgba = [255, 59, 48, 255];
const RED_LIGHT: Rgba = [255, 233, 231, 255];
const GRAY: Rgba = [142, 142, 147, 255];
const KNOB: Rgba = [255, 255, 255, 255];

const TEXT: u32 = 0xFF1C1C1E;
const SUBTEXT: u32 = 0xFF8E8E93;
const TEXT_ACCENT: u32 = 0xFF0A84FF;
const TEXT_RED: u32 = 0xFFFF3B30;
const TEXT_WHITE: u32 = 0xFFFFFFFF;

/// The game font's line height in font pixels.
const LINE: f32 = 8.0;
/// Small text: details, values, pills.
const SMALL: f32 = 0.75;

// How far each layer stands out from its panel, in layout units.
const Z_SHADOW: f32 = -0.4;
const Z_PANEL: f32 = 0.0;
const Z_CARD_SHADOW: f32 = 0.3;
const Z_CARD: f32 = 0.6;
const Z_CONTROL: f32 = 1.0;
const Z_KNOB: f32 = 1.4;
const Z_TEXT: f32 = 1.8;

/// The strips that fill `rect` (`[x, y, w, h]`) with its corners rounded by
/// `radius`, `steps` strips to each corner's height.
pub(crate) fn rounded_strips(rect: [f32; 4], radius: f32, steps: usize) -> Vec<[f32; 4]> {
    let [x, y, w, h] = rect;
    if w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    let r = radius.clamp(0.0, w.min(h) * 0.5);
    if r <= 0.0 || steps == 0 {
        return vec![rect];
    }
    let band = r / steps as f32;
    let mut strips = Vec::with_capacity(steps * 2 + 1);
    for i in 0..steps {
        // The circle's inset at the middle of the band.
        let dy = r - (i as f32 + 0.5) * band;
        let inset = r - (r * r - dy * dy).max(0.0).sqrt();
        let top = y + i as f32 * band;
        let bottom = y + h - (i as f32 + 1.0) * band;
        strips.push([x + inset, top, w - 2.0 * inset, band]);
        strips.push([x + inset, bottom, w - 2.0 * inset, band]);
    }
    if h - 2.0 * r > 0.0 {
        strips.push([x, y + r, w, h - 2.0 * r]);
    }
    strips
}

fn with_alpha(c: Rgba, alpha: f32) -> Rgba {
    [c[0], c[1], c[2], (f32::from(c[3]) * alpha.clamp(0.0, 1.0)) as u8]
}

fn argb_alpha(c: u32, alpha: f32) -> u32 {
    let a = ((c >> 24) as f32 * alpha.clamp(0.0, 1.0)) as u32;
    // Text fully transparent may still be drawn opaque: keep a sliver.
    (a.max(4) << 24) | (c & 0x00FF_FFFF)
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    [l(a[0], b[0]), l(a[1], b[1]), l(a[2], b[2]), l(a[3], b[3])]
}

/// One thing to draw on a panel, in its layout units.
#[derive(Clone)]
pub(crate) enum Cmd {
    Quad([[f32; 3]; 4], Rgba),
    /// The text, where its top-left corner goes, its scale and colour.
    Text(Text, [f32; 3], f32, u32),
}

/// What the open menu draws, panel by panel. Built only when what it shows
/// changes, and replayed every frame: the text is laid out and measured once.
pub(crate) type DisplayList = Vec<(PanelId, Vec<Cmd>)>;

/// Lays out the drawing of the whole menu.
pub(crate) fn build(menu: &MenuState, host: &dyn Host) -> DisplayList {
    [PanelId::Left, PanelId::Center, PanelId::Right]
        .into_iter()
        .map(|panel| {
            let mut p = Painter {
                host,
                cmds: Vec::new(),
            };
            match panel {
                PanelId::Left => left_panel(&mut p, menu),
                PanelId::Center => center_button(&mut p, menu),
                PanelId::Right => right_panel(&mut p, menu),
            }
            (panel, p.cmds)
        })
        .collect()
}

/// Records the drawing of one panel, in its layout units.
struct Painter<'a> {
    host: &'a dyn Host,
    cmds: Vec<Cmd>,
}

impl Painter<'_> {
    fn quad(&mut self, corners: [[f32; 2]; 4], z: f32, color: Rgba) {
        if color[3] == 0 {
            return;
        }
        self.cmds.push(Cmd::Quad(corners.map(|[x, y]| [x, y, z]), color));
    }

    fn rect(&mut self, [x, y, w, h]: [f32; 4], z: f32, color: Rgba) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        // Counterclockwise as the eye sees it: y runs down the panel.
        self.quad([[x, y], [x, y + h], [x + w, y + h], [x + w, y]], z, color);
    }

    fn rounded(&mut self, rect: [f32; 4], radius: f32, z: f32, color: Rgba) {
        // A few strips are enough at the size the menu is seen.
        let steps = if radius >= 9.0 { 3 } else { 2 };
        for strip in rounded_strips(rect, radius, steps) {
            self.rect(strip, z, color);
        }
    }

    /// A shadow under `rect`: one layer, a little wider and lower.
    fn shadow(&mut self, rect: [f32; 4], radius: f32, z: f32, color: Rgba) {
        let [x, y, w, h] = rect;
        let grow = 2.0;
        let r = [x - grow, y - grow + 2.0, w + grow * 2.0, h + grow * 2.0];
        self.rounded(r, radius + grow, z, color);
    }

    fn circle(&mut self, center: [f32; 2], radius: f32, z: f32, color: Rgba) {
        let d = radius * 2.0;
        self.rounded([center[0] - radius, center[1] - radius, d, d], radius, z, color);
    }

    fn triangle(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], z: f32, color: Rgba) {
        self.quad([a, b, c, c], z, color);
    }

    fn width(&self, text: &str, scale: f32) -> f32 {
        self.host.text_width(text) * scale
    }

    /// `text` cut with an ellipsis to fit `max` units at `scale`.
    fn fit(&self, text: &str, max: f32, scale: f32) -> String {
        if self.width(text, scale) <= max {
            return text.to_string();
        }
        let mut chars: Vec<char> = text.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            let cut: String = chars.iter().collect::<String>() + "…";
            if self.width(&cut, scale) <= max {
                return cut;
            }
        }
        String::new()
    }

    fn text(&mut self, text: &str, at: [f32; 2], scale: f32, color: u32) {
        if text.is_empty() {
            return;
        }
        self.cmds.push(Cmd::Text(
            Text::literal(text.to_string()),
            [at[0], at[1], Z_TEXT],
            scale,
            color,
        ));
    }

    /// `text` centred in `rect`.
    fn text_in(&mut self, text: &str, rect: [f32; 4], scale: f32, color: u32) {
        let w = self.width(text, scale);
        let at = [
            rect[0] + (rect[2] - w) * 0.5,
            rect[1] + (rect[3] - LINE * scale) * 0.5,
        ];
        self.text(text, at, scale, color);
    }

    /// `text` ending at `right`.
    fn text_right(&mut self, text: &str, right: f32, y: f32, scale: f32, color: u32) {
        let w = self.width(text, scale);
        self.text(text, [right - w, y], scale, color);
    }

    /// A button: a rounded rectangle with its label in the middle.
    fn button(&mut self, rect: [f32; 4], label: &str, fill: Rgba, color: u32, hovered: bool) {
        let fill = if hovered { mix(fill, ACCENT, 0.18) } else { fill };
        self.rounded(rect, 6.0, Z_CONTROL, fill);
        self.text_in(label, rect, SMALL, color);
    }

    /// The panel's background and its shadow.
    fn panel(&mut self, size: [f32; 2], title: &str, note: &str) {
        let rect = [0.0, 0.0, size[0], size[1]];
        self.shadow(rect, 14.0, Z_SHADOW, PANEL_SHADOW);
        self.rounded(rect, 14.0, Z_PANEL, PANEL);
        self.text(title, [14.0, 14.0], 1.0, TEXT);
        if !note.is_empty() {
            self.text_right(note, size[0] - 34.0, 15.0, SMALL, SUBTEXT);
        }
    }

    fn card(&mut self, rect: [f32; 4], hovered: bool) {
        self.shadow(rect, 10.0, Z_CARD_SHADOW, CARD_SHADOW);
        self.rounded(rect, 10.0, Z_CARD, if hovered { CARD_HOVER } else { CARD });
    }
}

/// Draws the open menu in the world.
pub(crate) fn world_submit(state: &State, host: &dyn Host, event: &mut WorldSubmitEvent<'_>) {
    if state.menu.open {
        draw_menu(state, host, event);
    }
}

fn draw_menu(state: &State, host: &dyn Host, event: &mut WorldSubmitEvent<'_>) {
    let menu = &state.menu;
    let (true, Some(anchor)) = (menu.open, menu.anchor) else {
        return;
    };
    for (panel, pose) in panels(&anchor) {
        let Some((_, cmds)) = menu.display.iter().find(|(p, _)| *p == panel) else {
            continue;
        };
        let unit = pose.unit as f32;
        event.draw.push();
        if event.draw.translate_world(pose.origin) {
            event.draw.rotate_about([0.0, 1.0, 0.0], pose.turn_degrees, [0.0; 3]);
            event.draw.scale([unit, -unit, unit]);
            replay(host, event.draw, cmds);
        }
        event.draw.pop();
    }
}

fn replay(host: &dyn Host, draw: &mut NativeWorldDraw, cmds: &[Cmd]) {
    for cmd in cmds {
        match cmd {
            Cmd::Quad(corners, color) => {
                host.world_quad(draw, WHITE, *corners, WHITE_UV, *color);
            }
            Cmd::Text(text, at, scale, color) => {
                draw.push();
                draw.translate(*at);
                draw.scale([*scale, *scale, 1.0]);
                host.world_text(draw, text, *color, None);
                draw.pop();
            }
        }
    }
}

fn hovered(menu: &MenuState, id: WidgetId) -> bool {
    menu.hovered == Some(id) || menu.flash.is_some_and(|(f, _)| f == id)
}

fn left_panel(p: &mut Painter<'_>, menu: &MenuState) {
    let m = &menu.model;
    p.panel(layout::LEFT_SIZE, "Horizon Video", "");

    // The close button.
    if let Some(w) = menu.layout.widget(WidgetId::Close) {
        let r = w.rect;
        let on = hovered(menu, WidgetId::Close);
        let center = [r[0] + r[2] * 0.5, r[1] + r[3] * 0.5];
        p.circle(center, r[2] * 0.5, Z_CONTROL, if on { RED } else { TRACK });
        p.text_in("×", r, 1.0, if on { TEXT_WHITE } else { SUBTEXT });
    }

    // `Configuration >`: the grid editor.
    if let Some(w) = menu.layout.widget(WidgetId::Configuration) {
        let r = w.rect;
        p.card(r, hovered(menu, WidgetId::Configuration));
        p.text("Configuration", [r[0] + 10.0, r[1] + 10.0], 1.0, TEXT);
        let state = if m.editing { "Editing" } else { "" };
        p.text_right(state, r[0] + r[2] - 22.0, r[1] + 11.0, SMALL, TEXT_ACCENT);
        chevron(p, [r[0] + r[2] - 14.0, r[1] + r[3] * 0.5], false, GRAY);
    }

    // The active screen: its name, status, file and two buttons.
    let r = ACTIVE_CARD;
    p.card(r, false);
    let pill_color = match m.status {
        Status::Playing => GREEN,
        Status::Paused => ORANGE,
        Status::Idle | Status::NoScreen => GRAY,
    };
    let label = m.status.label();
    let pill_w = p.width(label, SMALL) + 12.0;
    let pill = [r[0] + r[2] - 10.0 - pill_w, r[1] + 9.0, pill_w, 12.0];
    p.rounded(pill, 6.0, Z_CONTROL, pill_color);
    p.text_in(label, pill, SMALL, TEXT_WHITE);
    let title = p.fit(&m.title, pill[0] - r[0] - 18.0, 1.0);
    p.text(&title, [r[0] + 10.0, r[1] + 10.0], 1.0, TEXT);
    let file = m.file.as_deref().unwrap_or("No video");
    let file = p.fit(file, r[2] - 20.0, SMALL);
    p.text(&file, [r[0] + 10.0, r[1] + 26.0], SMALL, SUBTEXT);
    let detail = if m.editing {
        "Grid editor on: hold RMB on a handle"
    } else {
        ""
    };
    p.text(detail, [r[0] + 10.0, r[1] + 40.0], SMALL, TEXT_ACCENT);
    for (id, label, fill, color) in [
        (WidgetId::SnapToWall, "Snap to Wall", ACCENT_LIGHT, TEXT_ACCENT),
        (WidgetId::Save, "Save", ACCENT, TEXT_WHITE),
    ] {
        if let Some(w) = menu.layout.widget(id) {
            p.button(w.rect, label, fill, color, hovered(menu, id));
        }
    }

    // The switches.
    let r = TOGGLES_CARD;
    p.card(r, false);
    for (i, kind) in ToggleKind::ALL.into_iter().enumerate() {
        let row = layout::toggle_row(i);
        let id = WidgetId::Toggle(kind);
        if hovered(menu, id) {
            p.rounded([row[0] + 3.0, row[1] + 2.0, row[2] - 6.0, row[3] - 4.0], 7.0, Z_CONTROL - 0.2, CARD_HOVER);
        }
        if i > 0 {
            p.rect([row[0] + 10.0, row[1], row[2] - 20.0, 0.6], Z_CONTROL, SEPARATOR);
        }
        p.text(kind.label(), [row[0] + 10.0, row[1] + 10.0], 1.0, TEXT);
        let switch = layout::toggle_switch(row);
        match kind {
            ToggleKind::Audio => {
                let w = p.width(&m.audio, SMALL) + 14.0;
                let pill = [switch[0] + switch[2] - w, switch[1], w, switch[3]];
                p.rounded(pill, 8.0, Z_CONTROL, ACCENT_LIGHT);
                p.text_in(&m.audio, pill, SMALL, TEXT_ACCENT);
            }
            _ => {
                let t = menu.knobs.get(&kind).copied().unwrap_or(0.0);
                p.rounded(switch, 8.0, Z_CONTROL, mix(TRACK, GREEN, t));
                let x = switch[0] + 8.0 + (switch[2] - 16.0) * t;
                p.circle([x, switch[1] + 8.0], 6.5, Z_KNOB, KNOB);
            }
        }
    }
}

/// A small arrow: right, or down for an open card.
fn chevron(p: &mut Painter<'_>, at: [f32; 2], down: bool, color: Rgba) {
    let [x, y] = at;
    if down {
        p.triangle([x - 4.0, y - 2.0], [x, y + 3.0], [x + 4.0, y - 2.0], Z_CONTROL, color);
    } else {
        p.triangle([x - 2.0, y - 4.0], [x - 2.0, y + 4.0], [x + 3.0, y], Z_CONTROL, color);
    }
}

fn center_button(p: &mut Painter<'_>, menu: &MenuState) {
    let [w, h] = layout::CENTER_SIZE;
    let c = [w * 0.5, h * 0.5];
    let on = hovered(menu, WidgetId::PlayPause);
    p.shadow([0.0, 0.0, w, h], w * 0.5, Z_SHADOW, PANEL_SHADOW);
    p.circle(c, w * 0.5, Z_CONTROL, if on { ACCENT_HOVER } else { ACCENT });
    if menu.model.status == Status::Playing {
        p.rect([c[0] - 7.0, c[1] - 8.0, 5.0, 16.0], Z_KNOB, KNOB);
        p.rect([c[0] + 2.0, c[1] - 8.0, 5.0, 16.0], Z_KNOB, KNOB);
    } else {
        p.triangle(
            [c[0] - 5.0, c[1] - 9.0],
            [c[0] - 5.0, c[1] + 9.0],
            [c[0] + 9.0, c[1]],
            Z_KNOB,
            KNOB,
        );
    }
}

fn right_panel(p: &mut Painter<'_>, menu: &MenuState) {
    let m = &menu.model;
    let count = format!("{}", m.screens.len());
    p.panel(layout::RIGHT_SIZE, "Screens", &count);
    if m.screens.is_empty() {
        let area = [10.0, layout::LIST_TOP, 180.0, 60.0];
        p.text_in("No saved screens yet", area, SMALL, SUBTEXT);
    }
    for frame in &menu.layout.cards {
        screen_card(p, menu, frame);
    }

    // Where the list is scrolled to.
    if menu.layout.max_scroll > 0.0 {
        let view = layout::LIST_BOTTOM - layout::LIST_TOP;
        let content = view + menu.layout.max_scroll;
        let bar = (view * view / content).max(16.0);
        let y = layout::LIST_TOP + (view - bar) * (m.scroll / menu.layout.max_scroll);
        p.rounded([194.0, y, 3.0, bar], 1.5, Z_CONTROL, with_alpha(GRAY, 0.6));
    }

    if let Some(w) = menu.layout.widget(WidgetId::NewScreen) {
        p.button(w.rect, "+ New Screen", ACCENT_LIGHT, TEXT_ACCENT, hovered(menu, WidgetId::NewScreen));
    }
}

fn screen_card(p: &mut Painter<'_>, menu: &MenuState, frame: &CardFrame) {
    let m: &MenuModel = &menu.model;
    let Some(card) = m.screens.get(frame.index) else {
        return;
    };
    let r = frame.rect;
    let open = m.expanded == Some(frame.index);
    p.card(r, hovered(menu, WidgetId::Card(frame.index)));
    if card.active {
        p.circle([r[0] + 9.0, r[1] + 14.0], 2.5, Z_CONTROL, GREEN);
    }
    let title = p.fit(&card.id, r[2] - 40.0, 1.0);
    p.text(&title, [r[0] + 15.0, r[1] + 9.0], 1.0, TEXT);
    let detail = p.fit(&card.detail, r[2] - 40.0, SMALL);
    p.text(&detail, [r[0] + 15.0, r[1] + 23.0], SMALL, SUBTEXT);
    chevron(p, [r[0] + r[2] - 14.0, r[1] + layout::CARD_HEIGHT * 0.5], open, GRAY);
    if !open {
        return;
    }

    let fade = menu.expand_fade;
    p.rect(
        [r[0] + 10.0, r[1] + layout::CARD_HEIGHT - 2.0, r[2] - 20.0, 0.6],
        Z_CONTROL,
        with_alpha(SEPARATOR, fade),
    );
    for s in &m.sliders {
        let Some(w) = menu.layout.widget(WidgetId::Slider(s.kind)) else {
            continue;
        };
        let [x, y, width, _] = w.rect;
        let active = menu.active_slider == Some(s.kind);
        let on = hovered(menu, WidgetId::Slider(s.kind)) || active;
        let label_color = if s.enabled { TEXT } else { SUBTEXT };
        p.text(s.kind.label(), [x, y + 3.0], SMALL, argb_alpha(label_color, fade));
        p.text_right(&s.text, x + width, y + 3.0, SMALL, argb_alpha(SUBTEXT, fade));
        let track = [x, y + 15.0, width, 4.0];
        p.rounded(track, 2.0, Z_CONTROL, with_alpha(TRACK, fade));
        let fill = if s.enabled { ACCENT } else { GRAY };
        p.rounded([x, y + 15.0, width * s.fraction, 4.0], 2.0, Z_CONTROL + 0.1, with_alpha(fill, fade));
        if s.enabled {
            let knob = [x + width * s.fraction, y + 17.0];
            let radius = if active { 6.5 } else if on { 6.0 } else { 5.0 };
            p.circle(knob, radius + 0.8, Z_KNOB - 0.1, with_alpha(SEPARATOR, fade));
            p.circle(knob, radius, Z_KNOB, with_alpha(if active { ACCENT } else { KNOB }, fade));
        }
    }

    let playing = card.active && m.status == Status::Playing;
    let buttons = [
        (
            WidgetId::CardPlay(frame.index),
            if playing { "Pause" } else { "Play" },
            ACCENT,
            TEXT_WHITE,
        ),
        (WidgetId::CardEdit(frame.index), "Edit 3D", ACCENT_LIGHT, TEXT_ACCENT),
        (WidgetId::CardDelete(frame.index), "Delete", RED_LIGHT, TEXT_RED),
    ];
    for (id, label, fill, color) in buttons {
        if let Some(w) = menu.layout.widget(id) {
            p.button(w.rect, label, with_alpha(fill, fade), argb_alpha(color, fade), hovered(menu, id));
        }
    }
}

/// The crosshair over the menu: a white dot on a panel, a red target on a widget.
pub(crate) fn gui_render_post(state: &State, host: &dyn Host, event: &mut GuiRenderPostEvent<'_>) {
    let menu = &state.menu;
    if !menu.aiming() || event.gui_hidden || !host.playing() {
        return;
    }
    let batch = &mut *event.batch;
    let (cx, cy) = ((batch.width * 0.5).floor(), (batch.height * 0.5).floor());
    if menu.hovered.is_none() {
        host.fill(batch, [cx - 2.0, cy - 2.0, 4.0, 4.0], 0x80000000);
        host.fill(batch, [cx - 1.0, cy - 1.0, 2.0, 2.0], 0xFFFFFFFF);
        return;
    }
    let red = 0xFFFF3B30;
    let shade = 0x90000000;
    // A ring of four bars around a dot, each with a dark edge for any background.
    for (rect, color) in [
        ([cx - 6.0, cy - 6.0, 12.0, 1.0], red),
        ([cx - 6.0, cy + 5.0, 12.0, 1.0], red),
        ([cx - 6.0, cy - 6.0, 1.0, 12.0], red),
        ([cx + 5.0, cy - 6.0, 1.0, 12.0], red),
        ([cx - 1.0, cy - 1.0, 2.0, 2.0], red),
    ] {
        host.fill(batch, [rect[0] - 0.5, rect[1] - 0.5, rect[2] + 1.0, rect[3] + 1.0], shade);
        host.fill(batch, rect, color);
    }
    if let Some(kind) = menu.active_slider
        && let Some(s) = menu.model.sliders.iter().find(|s| s.kind == kind)
    {
        let text = format!("{}: {}", kind.label(), s.text);
        let w = host.text_width(&text);
        host.fill(batch, [cx - w * 0.5 - 3.0, cy + 10.0, w + 6.0, 12.0], 0xA0000000);
        host.text(batch, &text, [cx - w * 0.5, cy + 12.0], 0xFFFFFF, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(strips: &[[f32; 4]]) -> f32 {
        strips.iter().map(|s| s[2] * s[3]).sum()
    }

    #[test]
    fn square_corners_are_one_strip() {
        assert_eq!(rounded_strips([1.0, 2.0, 3.0, 4.0], 0.0, 4), vec![[1.0, 2.0, 3.0, 4.0]]);
    }

    #[test]
    fn rounding_takes_off_about_the_corners() {
        let (w, h, r) = (100.0f32, 60.0, 10.0);
        let strips = rounded_strips([0.0, 0.0, w, h], r, 8);
        let expected = w * h - (4.0 - std::f32::consts::PI) * r * r;
        assert!((area(&strips) - expected).abs() < expected * 0.01, "{}", area(&strips));
        // Everything stays inside the rectangle.
        for s in &strips {
            assert!(s[0] >= 0.0 && s[1] >= 0.0 && s[0] + s[2] <= w + 1e-4 && s[1] + s[3] <= h + 1e-4);
        }
    }

    #[test]
    fn a_pill_is_rounded_by_half_its_height() {
        let strips = rounded_strips([0.0, 0.0, 28.0, 16.0], 50.0, 4);
        // No middle strip is left once the radius is half the height.
        assert_eq!(strips.len(), 8);
        let total: f32 = strips.iter().map(|s| s[3]).sum();
        assert!((total - 16.0).abs() < 1e-4);
    }

    #[test]
    fn empty_rects_draw_nothing() {
        assert!(rounded_strips([0.0, 0.0, 0.0, 5.0], 2.0, 4).is_empty());
    }

    #[test]
    fn colours_mix_and_fade() {
        assert_eq!(mix([0, 0, 0, 255], [255, 255, 255, 255], 0.5), [128, 128, 128, 255]);
        assert_eq!(with_alpha([1, 2, 3, 200], 0.5), [1, 2, 3, 100]);
        assert_eq!(argb_alpha(0xFF123456, 1.0), 0xFF123456);
    }
}
