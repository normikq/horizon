use super::*;

fn model(n: usize, expanded: Option<usize>, scroll: f32) -> MenuModel {
    MenuModel {
        screens: (0..n)
            .map(|i| CardModel {
                id: format!("s{i}"),
                detail: String::new(),
                active: false,
            })
            .collect(),
        expanded,
        scroll,
        ..MenuModel::default()
    }
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

fn center(rect: [f32; 4]) -> [f32; 2] {
    [rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5]
}

#[test]
fn panel_widgets_exist_within_bounds() {
    let l = layout(&model(0, None, 0.0));
    let left_ids = [
        WidgetId::Close,
        WidgetId::Configuration,
        WidgetId::SnapToWall,
        WidgetId::Save,
        WidgetId::Toggle(ToggleKind::GridSnap),
        WidgetId::Toggle(ToggleKind::AutoAspect),
        WidgetId::Toggle(ToggleKind::Audio),
    ];

    for id in left_ids {
        let w = l.widget(id).expect("missing left widget");
        assert_eq!(w.panel, PanelId::Left);
        assert!(w.rect[0] >= 0.0);
        assert!(w.rect[1] >= 0.0);
        assert!(w.rect[0] + w.rect[2] <= LEFT_SIZE[0]);
        assert!(w.rect[1] + w.rect[3] <= LEFT_SIZE[1]);
    }

    let play_pause = l.widget(WidgetId::PlayPause).expect("missing PlayPause");
    assert_eq!(play_pause.panel, PanelId::Center);
    assert_eq!(play_pause.rect, [0.0, 0.0, CENTER_SIZE[0], CENTER_SIZE[1]]);

    let new_screen = l.widget(WidgetId::NewScreen).expect("missing NewScreen");
    assert_eq!(new_screen.panel, PanelId::Right);
}

#[test]
fn hit_tests_panels_and_empty_space() {
    let l = layout(&model(0, None, 0.0));
    let save_center = center(l.widget(WidgetId::Save).unwrap().rect);

    assert_eq!(hit(&l, PanelId::Left, save_center), Some(WidgetId::Save));
    assert_ne!(hit(&l, PanelId::Right, save_center), Some(WidgetId::Save));
    assert_eq!(hit(&l, PanelId::Left, [5.0, 5.0]), None);
}

#[test]
fn expanded_card_widgets_and_sliders() {
    let l = layout(&model(3, Some(0), 0.0));

    for kind in SliderKind::ALL {
        let w = l.widget(WidgetId::Slider(kind)).expect("missing slider");
        assert_eq!(w.panel, PanelId::Right);
    }

    let vol_rect = l.widget(WidgetId::Slider(SliderKind::Volume)).unwrap().rect;
    assert_eq!(
        hit(&l, PanelId::Right, center(vol_rect)),
        Some(WidgetId::Slider(SliderKind::Volume))
    );

    let card_rect = l.widget(WidgetId::Card(0)).unwrap().rect;
    assert_eq!(
        hit(&l, PanelId::Right, center(card_rect)),
        Some(WidgetId::Card(0))
    );

    assert!(l.widget(WidgetId::CardPlay(0)).is_some());
    assert!(l.widget(WidgetId::CardEdit(0)).is_some());
    assert!(l.widget(WidgetId::CardDelete(0)).is_some());

    let collapsed = layout(&model(3, None, 0.0));
    assert!(collapsed.widgets.iter().all(|w| w.id.slider().is_none()));
}

#[test]
fn visible_cards_stay_within_list_bounds() {
    let ms = max_scroll(&model(10, Some(3), 0.0));
    for scroll in [0.0, 50.0, ms] {
        let l = layout(&model(10, Some(3), scroll));
        for card in l.cards {
            assert!(card.rect[1] >= LIST_TOP - 0.01);
            assert!(card.rect[1] + card.rect[3] <= LIST_BOTTOM + 0.01);
        }
    }
}

#[test]
fn max_scroll_collapsed_and_expanded() {
    for n in 0..=4 {
        assert_eq!(max_scroll(&model(n, None, 0.0)), 0.0);
    }

    let collapsed_ms = max_scroll(&model(10, None, 0.0));
    let expected_collapsed = 10.0 * CARD_HEIGHT + 9.0 * 6.0 - (LIST_BOTTOM - LIST_TOP);
    assert!(approx(collapsed_ms, expected_collapsed));

    let expanded_ms = max_scroll(&model(10, Some(2), 0.0));
    assert!(approx(expanded_ms - collapsed_ms, EXPANDED_HEIGHT - CARD_HEIGHT));
}

#[test]
fn scroll_to_card_brings_card_into_view_and_clamps() {
    let m = model(10, None, 0.0);
    let s = scroll_to_card(&m, 9, 0.0);
    assert!(layout(&model(10, None, s)).cards.iter().any(|c| c.index == 9));

    let ms = max_scroll(&m);
    assert!(approx(scroll_to_card(&m, 0, ms), 0.0));
    assert!(approx(scroll_to_card(&m, 1, 0.0), 0.0));

    for index in 0..10 {
        for scroll in [0.0, 37.0, ms] {
            let res = scroll_to_card(&m, index, scroll);
            assert!(res >= 0.0 && res <= ms);
        }
    }
}

#[test]
fn slider_fraction_clamps_and_handles_zero_width() {
    let rect = [20.0, 0.0, 160.0, 24.0];
    assert!(approx(slider_fraction(rect, 20.0), 0.0));
    assert!(approx(slider_fraction(rect, 180.0), 1.0));
    assert!(approx(slider_fraction(rect, 100.0), 0.5));
    assert!(approx(slider_fraction(rect, -50.0), 0.0));
    assert!(approx(slider_fraction(rect, 500.0), 1.0));
    assert!(slider_fraction([5.0, 0.0, 0.0, 10.0], 5.0).is_finite());
}

#[test]
fn contains_includes_edges_and_excludes_outside() {
    let rect = [0.0, 0.0, 10.0, 10.0];
    assert!(contains(rect, [0.0, 0.0]));
    assert!(contains(rect, [10.0, 10.0]));
    assert!(contains(rect, [10.0, 0.0]));
    assert!(!contains(rect, [10.01, 5.0]));
    assert!(!contains(rect, [-0.01, 5.0]));
}

#[test]
fn widget_id_slider_accessor() {
    assert_eq!(
        WidgetId::Slider(SliderKind::Seek).slider(),
        Some(SliderKind::Seek)
    );
    assert_eq!(WidgetId::Save.slider(), None);
    assert_eq!(WidgetId::Card(0).slider(), None);
}
