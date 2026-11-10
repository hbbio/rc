use super::*;
use rc_core::MouseClickTarget;

fn scroll(app: &mut AppState, x: u16, y: u16, width: u16, height: u16, direction: ScrollDirection) {
    for command in commands_for_scroll(app, x, y, width, height, direction) {
        app.apply(command).unwrap();
    }
}

#[test]
fn wheel_scrolls_the_hovered_panel_using_its_rendered_grid_and_clamps() {
    let root = temp_root("wheel-grid");
    for n in 0..95 {
        fs::write(root.join(format!("f{n:02}")), "data").unwrap();
    }
    let mut app = app_with_loaded_panels(root.clone());
    for format in PanelListingFormat::ALL {
        for panel in [ActivePanel::Left, ActivePanel::Right] {
            set_format(&mut app, panel, format);
        }
        for panel in [ActivePanel::Left, ActivePanel::Right] {
            app.active_panel = panel;
            app.apply(AppCommand::Panel(panel, PanelCommand::SelectAt(0)))
                .unwrap();
            for (width, height) in [(120, 12), (65, 9), (181, 15)] {
                let content = root_areas(Rect::new(0, 0, width, height), &app)[1];
                let area = if uses_single_panel_layout(&app) {
                    content
                } else {
                    dual_panel_areas(content)[panel.index()]
                };
                let listing =
                    panel_content_areas(Block::default().borders(Borders::ALL).inner(area))[0];
                let other = app.panels[panel.other().index()].cursor;
                // Scrolling a passive pane must target that pane, not the active one.
                if !uses_single_panel_layout(&app) {
                    app.active_panel = panel.other();
                }
                for direction in [ScrollDirection::Down, ScrollDirection::Up] {
                    for _ in 0..40 {
                        scroll(
                            &mut app,
                            listing.x,
                            listing.bottom() - 1,
                            width,
                            height,
                            direction,
                        );
                        assert_eq!(app.active_panel, panel);
                        assert_eq!(app.panels[panel.other().index()].cursor, other);
                        let state = &app.panels[panel.index()];
                        let columns = if format == PanelListingFormat::Brief {
                            BriefGrid::new(state.entries.len(), listing.width).columns
                        } else {
                            1
                        };
                        let rows = listing
                            .height
                            .saturating_sub(u16::from(format != PanelListingFormat::Brief))
                            as usize;
                        let (start, end) = state.visible_window(rows, columns);
                        assert!(
                            (start..end)
                                .contains(&(state.cursor % state.entries.len().div_ceil(columns)))
                        );
                    }
                }
                assert_rendered_entries_are_clickable(&app, width, height);
                assert_eq!(
                    app.panels[panel.index()].cwd,
                    root,
                    "scrolling must not open an entry"
                );
            }
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wheel_routes_to_modal_lists_and_help_without_falling_through() {
    let root = temp_root("wheel-modals");
    for n in 0..30 {
        fs::write(root.join(format!("f{n:02}")), "data").unwrap();
    }
    let mut app = app_with_loaded_panels(root.clone());
    app.apply(AppCommand::OpenPanelizeDialog).unwrap();
    let expected = match app.top_route() {
        Route::Dialog(dialog) => match &dialog.kind {
            DialogKind::Listbox(list) => {
                (list.selected + 3).min(list.items.len().saturating_sub(1))
            }
            _ => panic!("expected preset list"),
        },
        _ => panic!("expected preset dialog"),
    };
    let rect = listbox_dialog_layout(ScreenRect::new(0, 0, 120, 40), 2).list;
    scroll(&mut app, rect.x, rect.y, 120, 40, ScrollDirection::Down);
    assert!(
        matches!(app.top_route(), Route::Dialog(dialog) if matches!(&dialog.kind, DialogKind::Listbox(list) if list.selected == expected)),
        "{:?}",
        app.top_route()
    );
    scroll(&mut app, 1, 5, 120, 40, ScrollDirection::Down);
    assert_eq!(app.panels[0].cursor, 0);
    assert_eq!(app.panels[1].cursor, 0);
    app.apply(AppCommand::DialogCancel).unwrap();
    app.apply(AppCommand::OpenOptionsConfiguration).unwrap();
    scroll(&mut app, 60, 20, 120, 40, ScrollDirection::Down);
    assert!(matches!(app.top_route(), Route::Settings(settings) if settings.selected_entry == 3));
    app.apply(AppCommand::DialogCancel).unwrap();
    app.apply(AppCommand::OpenHelp).unwrap();
    scroll(&mut app, 60, 20, 120, 40, ScrollDirection::Down);
    assert!(matches!(app.top_route(), Route::Help(help) if help.scroll() == 3));
    scroll(&mut app, 60, 20, 120, 40, ScrollDirection::Up);
    assert!(matches!(app.top_route(), Route::Help(help) if help.scroll() == 0));
    assert!(app.panels.iter().all(|panel| panel.cursor == 0));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wheel_stops_at_the_end_of_a_short_brief_column() {
    let root = temp_root("wheel-brief-tail");
    for n in 0..25 {
        fs::write(root.join(format!("f{n:02}")), "data").unwrap();
    }
    let mut app = app_with_loaded_panels(root.clone());
    set_format(&mut app, ActivePanel::Left, PanelListingFormat::Brief);
    let last = app.active_panel().entries.len() - 1;
    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::SelectAt(last),
    ))
    .unwrap();
    for _ in 0..10 {
        scroll(&mut app, 5, 5, 120, 12, ScrollDirection::Down);
        assert_eq!(app.active_panel().cursor, last);
    }
    assert_rendered_entries_are_clickable(&app, 120, 12);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wheel_scrolls_viewer_and_quick_preview_without_activating_preview() {
    let root = temp_root("wheel-viewers");
    let text = (0..80).map(|n| format!("line {n}\n")).collect::<String>();
    fs::write(root.join("text"), text).unwrap();
    let mut app = app_with_loaded_panels(root.clone());
    let index = app
        .active_panel()
        .entries
        .iter()
        .position(|entry| entry.name == "text")
        .unwrap();
    app.apply(AppCommand::Panel(
        ActivePanel::Left,
        PanelCommand::SelectAt(index),
    ))
    .unwrap();
    app.apply(AppCommand::ViewEntry).unwrap();
    drain_background(&mut app);
    scroll(&mut app, 20, 5, 120, 40, ScrollDirection::Down);
    assert_eq!(app.active_viewer().unwrap().scroll, 3);
    app.apply(AppCommand::ViewerGoto).unwrap();
    scroll(&mut app, 20, 5, 120, 40, ScrollDirection::Down);
    assert_eq!(
        app.active_viewer().unwrap().scroll,
        3,
        "input dialog blocks viewer scrolling"
    );
    app.apply(AppCommand::DialogCancel).unwrap();
    app.apply(AppCommand::CloseViewer).unwrap();
    app.apply(AppCommand::Panel(
        ActivePanel::Right,
        PanelCommand::SetView(PanelViewMode::QuickView),
    ))
    .unwrap();
    drain_background(&mut app);
    scroll(&mut app, 70, 5, 120, 40, ScrollDirection::Down);
    assert_eq!(
        app.quick_view_state(ActivePanel::Right)
            .viewer()
            .unwrap()
            .scroll,
        3
    );
    assert_eq!(app.active_panel, ActivePanel::Left);
    assert_eq!(app.active_panel().cursor, index);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wheel_ignores_borders_headers_and_tiny_or_outside_viewports() {
    let root = temp_root("wheel-bounds");
    let app = app_with_loaded_panels(root.clone());
    for (x, y) in [
        (0, 5),
        (59, 5),
        (60, 5),
        (119, 5),
        (1, 2),
        (61, 2),
        (1, 14),
        (1, 15),
        (120, 5),
        (1, 40),
    ] {
        assert!(
            commands_for_scroll(&app, x, y, 120, 16, ScrollDirection::Down).is_empty(),
            "{x},{y}"
        );
    }
    for width in 0..6 {
        for height in 0..5 {
            for x in 0..=width {
                for y in 0..=height {
                    assert!(
                        commands_for_scroll(&app, x, y, width, height, ScrollDirection::Down)
                            .is_empty()
                    );
                }
            }
        }
    }
    fs::remove_dir_all(root).unwrap();
}

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
