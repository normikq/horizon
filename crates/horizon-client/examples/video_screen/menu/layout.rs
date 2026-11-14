//! What the menu shows and where: the widgets of its three panels in layout
//! units (`x` right, `y` down from each panel's top-left corner), built from a
//! plain model of the mod's state, and the hit test the crosshair uses.
//!
//! Nothing here touches the game, so the layout and the hit test are tested
//! on their own.

/// The left panel: the active screen and its settings.
pub(crate) const LEFT_SIZE: [f32; 2] = [200.0, 270.0];
/// The round play/pause button between the panels.
pub(crate) const CENTER_SIZE: [f32; 2] = [44.0, 44.0];
/// The right panel: the saved screens.
pub(crate) const RIGHT_SIZE: [f32; 2] = [200.0, 270.0];

/// Where the list of saved screens is shown on the right panel (top, bottom).
pub(crate) const LIST_TOP: f32 = 38.0;
pub(crate) const LIST_BOTTOM: f32 = 236.0;
/// A collapsed card's height, which is also an expanded card's header.
pub(crate) const CARD_HEIGHT: f32 = 40.0;
const CARD_GAP: f32 = 6.0;
const SLIDER_HEIGHT: f32 = 24.0;
const CARD_BUTTONS_HEIGHT: f32 = 30.0;
/// An expanded card's height: its header, five sliders and a row of buttons.
pub(crate) const EXPANDED_HEIGHT: f32 =
    CARD_HEIGHT + SLIDER_HEIGHT * SliderKind::ALL.len() as f32 + CARD_BUTTONS_HEIGHT + 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PanelId {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SliderKind {
    Volume,
    Seek,
    Size,
    Curvature,
    Distance,
}

impl SliderKind {
    pub(crate) const ALL: [SliderKind; 5] = [
        SliderKind::Volume,
        SliderKind::Seek,
        SliderKind::Size,
        SliderKind::Curvature,
        SliderKind::Distance,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            SliderKind::Volume => "Volume",
            SliderKind::Seek => "Seek",
            SliderKind::Size => "Size",
            SliderKind::Curvature => "Curvature",
            SliderKind::Distance => "Distance",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ToggleKind {
    GridSnap,
    AutoAspect,
    /// Not on/off: each click goes on to the next sound mode.
    Audio,
}

impl ToggleKind {
    pub(crate) const ALL: [ToggleKind; 3] =
        [ToggleKind::GridSnap, ToggleKind::AutoAspect, ToggleKind::Audio];

    pub(crate) fn label(self) -> &'static str {
        match self {
            ToggleKind::GridSnap => "Grid Snap",
            ToggleKind::AutoAspect => "Auto Aspect",
            ToggleKind::Audio => "Spatial Audio",
        }
    }
}

/// Everything the crosshair can point at and press.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum WidgetId {
    Close,
    /// `Configuration >`: the 3D grid editor on or off.
    Configuration,
    SnapToWall,
    Save,
    Toggle(ToggleKind),
    PlayPause,
    /// The header of saved screen `n` in the list: opens or closes it.
    Card(usize),
    NewScreen,
    /// A slider of the open card.
    Slider(SliderKind),
    CardPlay(usize),
    CardEdit(usize),
    CardDelete(usize),
}

impl WidgetId {
    /// Held down rather than clicked: the value follows the crosshair.
    pub(crate) fn slider(self) -> Option<SliderKind> {
        match self {
            WidgetId::Slider(kind) => Some(kind),
            _ => None,
        }
    }
}

/// One widget: what it is, on which panel and where, `[x, y, w, h]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Widget {
    pub id: WidgetId,
    pub panel: PanelId,
    pub rect: [f32; 4],
}

/// The playback state of the active screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    NoScreen,
    Idle,
    Playing,
    Paused,
}

impl Status {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Status::NoScreen => "No screen",
            Status::Idle => "Idle",
            Status::Playing => "Playing",
            Status::Paused => "Paused",
        }
    }
}

/// A slider's value as shown: where its knob is (0 to 1) and its text.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SliderModel {
    pub kind: SliderKind,
    pub fraction: f32,
    pub text: String,
    /// False when there is nothing to move: no video to seek, say.
    pub enabled: bool,
}

/// One saved screen in the list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CardModel {
    pub id: String,
    /// A line of detail: its size, curve and sound.
    pub detail: String,
    /// This saved screen is the one shown in the world.
    pub active: bool,
}

/// What the menu shows, read from the mod's state once a frame.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MenuModel {
    pub title: String,
    pub status: Status,
    pub file: Option<String>,
    pub editing: bool,
    pub grid_snap: bool,
    pub auto_aspect: bool,
    pub audio: String,
    pub screens: Vec<CardModel>,
    /// The open card, an index into `screens`.
    pub expanded: Option<usize>,
    /// The open card's sliders, in `SliderKind::ALL`'s order.
    pub sliders: Vec<SliderModel>,
    /// How far the list is scrolled down, in layout units.
    pub scroll: f32,
}

impl Default for MenuModel {
    fn default() -> Self {
        Self {
            title: String::new(),
            status: Status::NoScreen,
            file: None,
            editing: false,
            grid_snap: false,
            auto_aspect: true,
            audio: String::new(),
            screens: Vec::new(),
            expanded: None,
            sliders: Vec::new(),
            scroll: 0.0,
        }
    }
}

/// A saved screen's card as laid out: its index and its whole rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CardFrame {
    pub index: usize,
    pub rect: [f32; 4],
}

/// The laid-out menu.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Layout {
    /// Pressable widgets, a later one drawn over (and hit before) an earlier one.
    pub widgets: Vec<Widget>,
    /// The cards shown in the list; one scrolled partly out is left out.
    pub cards: Vec<CardFrame>,
    /// The furthest the list scrolls.
    pub max_scroll: f32,
}

impl Layout {
    pub(crate) fn widget(&self, id: WidgetId) -> Option<&Widget> {
        self.widgets.iter().find(|w| w.id == id)
    }
}

/// The card `index`'s height.
pub(crate) fn card_height(model: &MenuModel, index: usize) -> f32 {
    if model.expanded == Some(index) {
        EXPANDED_HEIGHT
    } else {
        CARD_HEIGHT
    }
}

/// The left panel's toggle row `i`.
pub(crate) fn toggle_row(i: usize) -> [f32; 4] {
    [10.0, 176.0 + 28.0 * i as f32, 180.0, 28.0]
}

/// The switch inside a toggle row.
pub(crate) fn toggle_switch(row: [f32; 4]) -> [f32; 4] {
    [row[0] + row[2] - 38.0, row[1] + 6.0, 28.0, 16.0]
}

/// The left panel's card of the active screen.
pub(crate) const ACTIVE_CARD: [f32; 4] = [10.0, 72.0, 180.0, 96.0];
/// The left panel's card of toggles.
pub(crate) const TOGGLES_CARD: [f32; 4] = [10.0, 176.0, 180.0, 84.0];

/// Lays the menu out for `model`.
pub(crate) fn layout(model: &MenuModel) -> Layout {
    let mut widgets = Vec::new();
    let mut push = |id, panel, rect| widgets.push(Widget { id, panel, rect });

    // The left panel.
    push(WidgetId::Close, PanelId::Left, [172.0, 10.0, 18.0, 18.0]);
    push(WidgetId::Configuration, PanelId::Left, [10.0, 38.0, 180.0, 28.0]);
    push(WidgetId::SnapToWall, PanelId::Left, [20.0, 136.0, 76.0, 24.0]);
    push(WidgetId::Save, PanelId::Left, [104.0, 136.0, 76.0, 24.0]);
    for (i, kind) in ToggleKind::ALL.into_iter().enumerate() {
        push(WidgetId::Toggle(kind), PanelId::Left, toggle_row(i));
    }

    // The round button.
    push(
        WidgetId::PlayPause,
        PanelId::Center,
        [0.0, 0.0, CENTER_SIZE[0], CENTER_SIZE[1]],
    );

    // The right panel: the cards, scrolled, then the button under them.
    let mut cards = Vec::new();
    let mut y = LIST_TOP - model.scroll;
    for index in 0..model.screens.len() {
        let h = card_height(model, index);
        let rect = [10.0, y, 180.0, h];
        if y >= LIST_TOP - 0.01 && y + h <= LIST_BOTTOM + 0.01 {
            cards.push(CardFrame { index, rect });
            push(WidgetId::Card(index), PanelId::Right, [10.0, y, 180.0, CARD_HEIGHT]);
            if model.expanded == Some(index) {
                for (k, kind) in SliderKind::ALL.into_iter().enumerate() {
                    let sy = y + CARD_HEIGHT + SLIDER_HEIGHT * k as f32;
                    push(WidgetId::Slider(kind), PanelId::Right, [20.0, sy, 160.0, SLIDER_HEIGHT]);
                }
                let by = y + CARD_HEIGHT + SLIDER_HEIGHT * SliderKind::ALL.len() as f32 + 4.0;
                push(WidgetId::CardPlay(index), PanelId::Right, [20.0, by, 50.0, 22.0]);
                push(WidgetId::CardEdit(index), PanelId::Right, [75.0, by, 50.0, 22.0]);
                push(WidgetId::CardDelete(index), PanelId::Right, [130.0, by, 50.0, 22.0]);
            }
        }
        y += h + CARD_GAP;
    }
    push(WidgetId::NewScreen, PanelId::Right, [10.0, 242.0, 180.0, 22.0]);

    Layout {
        widgets,
        cards,
        max_scroll: max_scroll(model),
    }
}

/// The furthest the list scrolls: until its last card's bottom is at the list's bottom.
pub(crate) fn max_scroll(model: &MenuModel) -> f32 {
    let content: f32 = (0..model.screens.len())
        .map(|i| card_height(model, i) + CARD_GAP)
        .sum::<f32>()
        - CARD_GAP;
    (content - (LIST_BOTTOM - LIST_TOP)).max(0.0)
}

/// The scroll that shows the whole of card `index`, moving as little as it can from `scroll`.
pub(crate) fn scroll_to_card(model: &MenuModel, index: usize, scroll: f32) -> f32 {
    let top: f32 = (0..index).map(|i| card_height(model, i) + CARD_GAP).sum();
    let bottom = top + card_height(model, index);
    let view = LIST_BOTTOM - LIST_TOP;
    let scroll = if top < scroll {
        top
    } else if bottom > scroll + view {
        bottom - view
    } else {
        scroll
    };
    scroll.clamp(0.0, max_scroll(model))
}

pub(crate) fn contains(rect: [f32; 4], point: [f32; 2]) -> bool {
    point[0] >= rect[0]
        && point[1] >= rect[1]
        && point[0] <= rect[0] + rect[2]
        && point[1] <= rect[1] + rect[3]
}

/// The widget at `point` on `panel`: the last one laid out there, as it is drawn on top.
pub(crate) fn hit(layout: &Layout, panel: PanelId, point: [f32; 2]) -> Option<WidgetId> {
    layout
        .widgets
        .iter()
        .rev()
        .find(|w| w.panel == panel && contains(w.rect, point))
        .map(|w| w.id)
}

/// Where along slider `rect` the point `x` is, 0 at its left end to 1 at its right.
pub(crate) fn slider_fraction(rect: [f32; 4], x: f32) -> f32 {
    ((x - rect[0]) / rect[2].max(1e-3)).clamp(0.0, 1.0)
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
