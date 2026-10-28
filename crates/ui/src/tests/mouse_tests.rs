use super::*;
use rc_core::MouseClickTarget;

fn set_format(app: &mut AppState, panel: ActivePanel, format: PanelListingFormat) {
    app.apply(AppCommand::Panel(panel, PanelCommand::OpenListingFormat))
        .unwrap();
    let index = PanelListingFormat::ALL
        .iter()
        .position(|item| *item == format)
        .unwrap();
    app.apply(AppCommand::DialogListboxSelectAt(index)).unwrap();
    app.apply(AppCommand::DialogAccept).unwrap();
    if format == PanelListingFormat::User {
        app.apply(AppCommand::DialogAccept).unwrap();
    }
}

fn assert_rendered_entries_are_clickable(app: &AppState, width: u16, height: u16) {
    let buffer = render_to_buffer(app, width, height);
    let content = root_areas(buffer.area, app)[1];
    let areas = if uses_single_panel_layout(app) {
        vec![(app.active_panel, content)]
    } else {
        dual_panel_areas(content)
            .into_iter()
            .enumerate()
            .map(|(index, area)| ([ActivePanel::Left, ActivePanel::Right][index], area))
            .collect()
    };
    for (panel_id, area) in areas {
        let mut hits = 0;
        for y in area.y + 1..area.bottom() - 1 {
            let line: String = (area.x + 1..area.right() - 1)
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            for (index, entry) in app.panels[panel_id.index()].entries.iter().enumerate() {
                if entry.is_parent() {
                    continue;
                }
                let Some(offset) = line.find(&entry.name) else {
                    continue;
                };
                // The fixture names and row contents are ASCII inside the borders.
                let x = area.x + 1 + offset as u16;
                for column in [x, x + entry.name.len() as u16 - 1] {
                    let commands = commands_for_left_click(app, column, y, width, height)
                        .expect("rendered filename should be selectable");
                    assert!(matches!(commands.primary,
                        AppCommand::Panel(clicked_panel, PanelCommand::SelectAtInViewport { index: clicked_index, .. })
                            if clicked_panel == panel_id && clicked_index == index));
                    assert_eq!(
                        commands.target,
                        MouseClickTarget::PanelEntry(panel_id, entry.path.clone())
                    );
                    assert_eq!(commands.activation, Some(AppCommand::OpenEntry));
                }
                hits += 1;
            }
        }
        assert!(
            hits > 0,
            "must check visible entries in {panel_id:?} at {width}x{height}"
        );
    }
}

#[test]
fn panel_clicks_match_rendered_full_and_brief_entries_in_both_panes() {
    let root = temp_root("panel-clicks");
    for index in 0..60 {
        fs::write(root.join(format!("f{index:02}")), "data").unwrap();
    }
    let mut app = app_with_loaded_panels(root.clone());
    for format in [PanelListingFormat::Full, PanelListingFormat::Brief] {
        for panel in [ActivePanel::Left, ActivePanel::Right] {
            set_format(&mut app, panel, format);
        }
        for cursors in [[0, 0], [15, 45]] {
            app.panels[0].cursor = cursors[0];
            app.panels[1].cursor = cursors[1];
            for menu in [false, true] {
                for buttons in [false, true] {
                    app.settings_mut().layout.show_menu_bar = menu;
                    app.settings_mut().layout.show_button_bar = buttons;
                    for width in [65, 120, 121, 181] {
                        assert_rendered_entries_are_clickable(&app, width, 12);
                    }
                }
            }
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn mouse_selection_preserves_rendered_rows_until_keyboard_navigation() {
    let root = temp_root("panel-click-viewport");
    for index in 0..60 {
        fs::write(root.join(format!("f{index:02}")), "data").unwrap();
    }
    let mut app = app_with_loaded_panels(root.clone());
    for format in PanelListingFormat::ALL {
        for panel in [ActivePanel::Left, ActivePanel::Right] {
            set_format(&mut app, panel, format);
        }
        for panel in [ActivePanel::Left, ActivePanel::Right] {
            for cursor in [0, 30] {
                app.apply(AppCommand::Panel(panel, PanelCommand::SelectAt(cursor)))
                    .unwrap();
                let before = render_to_buffer(&app, 120, 12);
                let content = root_areas(before.area, &app)[1];
                let area = if uses_single_panel_layout(&app) {
                    content
                } else {
                    dual_panel_areas(content)[panel.index()]
                };
                let listing =
                    panel_content_areas(Block::default().borders(Borders::ALL).inner(area))[0];
                let x = listing.x;
                let y = listing.bottom() - 1;
                let first = commands_for_left_click(&app, x, y, 120, 12).unwrap();
                app.apply(first.primary).unwrap();
                let after = render_to_buffer(&app, 120, 12);
                for row in listing.y..listing.bottom() {
                    for column in listing.x..listing.right() {
                        assert_eq!(
                            before[(column, row)].symbol(),
                            after[(column, row)].symbol(),
                            "mouse selection shifted {panel:?} {format:?} from cursor {cursor}"
                        );
                    }
                }
                let second = commands_for_left_click(&app, x, y, 120, 12).unwrap();
                assert_eq!(
                    first, second,
                    "second click must resolve the same entry and viewport"
                );
                app.apply(second.primary).unwrap();
                // A resize of a pinned viewport must keep rendering and hit testing consistent.
                assert_rendered_entries_are_clickable(&app, 121, 9);
                let AppCommand::Panel(_, PanelCommand::SelectAtInViewport { viewport, .. }) =
                    first.primary
                else {
                    panic!("expected panel mouse selection");
                };
                app.apply(AppCommand::Navigate(
                    NavigationTarget::FileManager,
                    NavigationMotion::Down,
                ))
                .unwrap();
                let panel_state = app.active_panel();
                let total_rows = panel_state.entries.len().div_ceil(viewport.columns);
                assert_eq!(
                    panel_state.visible_window(viewport.rows, viewport.columns),
                    visible_window(total_rows, panel_state.cursor % total_rows, viewport.rows),
                    "keyboard movement should resume normal scrolling"
                );
            }
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn panel_clicks_match_long_layout_for_either_active_pane() {
    let root = temp_root("long-clicks");
    for index in 0..24 {
        fs::write(root.join(format!("f{index:02}")), "data").unwrap();
    }
    let mut app = app_with_loaded_panels(root.clone());
    for panel in [ActivePanel::Left, ActivePanel::Right] {
        set_format(&mut app, panel, PanelListingFormat::Long);
    }
    for panel in [ActivePanel::Left, ActivePanel::Right] {
        app.active_panel = panel;
        app.panels[panel.index()].cursor = 12;
        assert_rendered_entries_are_clickable(&app, 120, 12);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn panel_clicks_ignore_headers_borders_empty_rows_and_overlays() {
    let root = temp_root("click-bounds");
    fs::write(root.join("entry"), "data").unwrap();
    let mut app = app_with_loaded_panels(root.clone());
    // At 120x16, the panels span rows 1..14, with entries at rows 3 and 4.
    for x in [1, 61] {
        for y in [1, 2, 5, 12, 13, 14, 15, 16] {
            assert_eq!(commands_for_left_click(&app, x, y, 120, 16), None);
        }
    }
    for x in [0, 59, 60, 119, 120] {
        assert_eq!(commands_for_left_click(&app, x, 4, 120, 16), None);
    }
    for mode in [PanelViewMode::Info, PanelViewMode::QuickView] {
        app.apply(AppCommand::Panel(
            ActivePanel::Right,
            PanelCommand::SetView(mode),
        ))
        .unwrap();
        assert_eq!(commands_for_left_click(&app, 61, 4, 120, 16), None);
        assert!(commands_for_left_click(&app, 1, 4, 120, 16).is_some());
    }
    app.apply(AppCommand::OpenHelp).unwrap();
    assert_eq!(commands_for_left_click(&app, 1, 4, 120, 16), None);
    app.apply(AppCommand::CloseHelp).unwrap();
    for width in 0..6 {
        for height in 0..5 {
            for x in 0..=width {
                for y in 1..=height {
                    assert_eq!(commands_for_left_click(&app, x, y, width, height), None);
                }
            }
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn brief_clicks_ignore_column_gaps_and_unoccupied_cells() {
    let root = temp_root("brief-click-bounds");
    for name in ["alpha", "beta", "gamma", "delta"] {
        fs::write(root.join(name), "data").unwrap();
    }
    let mut app = app_with_loaded_panels(root.clone());
    for panel in [ActivePanel::Left, ActivePanel::Right] {
        set_format(&mut app, panel, PanelListingFormat::Brief);
    }
    let buffer = render_to_buffer(&app, 120, 16);
    // Five entries (including ..), arranged as 2 + 2 + 1 across three columns.
    for (start, end) in [(1, 59), (61, 119)] {
        let mut gap_count = 0;
        for x in start..end {
            if commands_for_left_click(&app, x, 2, 120, 16).is_none() {
                assert_eq!(buffer[(x, 2)].symbol(), " ");
                gap_count += 1;
            }
        }
        assert_eq!(
            gap_count, 2,
            "only the two column separators should be ignored"
        );
        assert!(commands_for_left_click(&app, end - 1, 2, 120, 16).is_some());
        assert_eq!(commands_for_left_click(&app, end - 1, 3, 120, 16), None);
        assert_eq!(commands_for_left_click(&app, start, 4, 120, 16), None);
    }
    fs::remove_dir_all(root).unwrap();
}
